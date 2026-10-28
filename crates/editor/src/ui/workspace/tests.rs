use super::*;
use crate::runtime::DocumentLifecycle;
use editor_core::startup::Application;
use slint::Model;
use std::cell::Cell;
use std::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(10);

fn software(threading: bool) {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: false,
            threading,
        },
    )))
    .unwrap();
}

async fn wait_for(condition: impl Fn() -> bool) -> Result<(), String> {
    let started = Instant::now();
    while !condition() {
        if started.elapsed() > TIMEOUT {
            return Err("Timed out waiting for actual workspace UI completion".into());
        }
        let (done, completed) = oneshot::channel();
        slint::Timer::single_shot(Duration::from_millis(10), move || {
            let _ = done.send(());
        });
        completed
            .await
            .map_err(|_| "Workspace fixture timer was lost")?;
    }
    Ok(())
}

fn host(owner: &SharedOwner, selector: &str) -> Result<SharedHost, String> {
    owner
        .borrow()
        .hosts
        .iter()
        .find(|(id, _)| id == selector)
        .map(|(_, host)| host.clone())
        .ok_or_else(|| "Expected actual document UI host".into())
}

fn attach(
    application: &Application,
    ctx: GpuContext,
) -> (SharedOwner, mcp::WorkspaceOwner, String, AppWindow) {
    let factory = application.workspace();
    let document = application.document();
    let initial = factory
        .documents()
        .into_iter()
        .find(|entry| entry.document().id() == document.id())
        .unwrap();
    let owner = Owner::new(ctx.clone());
    let window = create_document_window(
        document.clone(),
        ctx.clone(),
        "Workspace fixture".into(),
        None,
    )
    .unwrap();
    let host = DocumentHost::new(window, document.clone());
    owner
        .borrow_mut()
        .hosts
        .push((initial.selector().to_owned(), host.clone()));
    let commands = mcp::WorkspaceOwner::start(
        factory.clone(),
        Some(ctx),
        Some(owner.borrow().proxy.callbacks()),
    )
    .unwrap();
    let workspace = commands.workspace();
    {
        let mut state = owner.borrow_mut();
        state.workspace = Some(workspace.clone());
        state
            .bind_host(
                initial.selector(),
                &host,
                workspace.terminal_for(document.id()).unwrap(),
            )
            .unwrap();
    }
    let app = host.borrow().window.app().clone_strong();
    app.show().unwrap();
    factory
        .set_visibility(document.id(), DocumentVisibility::Visible)
        .unwrap();
    (owner, commands, initial.selector().to_owned(), app)
}

#[test]
fn cancelled_presentation_cannot_acknowledge_work_and_cleanup_quit_survives_waiter_loss() {
    software(false);
    let document =
        crate::test_assets::document(Arc::new(Mutex::new(Session::new(Err("no assets".into())))));
    let proxy = Proxy::default();
    let pending = proxy.request(Action::Show(document.id(), true));
    assert_eq!(proxy.0.lock().unwrap().commands.len(), 1);
    drop(pending);
    assert!(
        proxy.0.lock().unwrap().commands[0]
            .cancelled
            .load(Ordering::Acquire)
    );
    let cleanup = proxy.request(Action::Retire(document.id()));
    drop(cleanup);
    assert_eq!(proxy.0.lock().unwrap().commands.len(), 1);
    assert!(
        !proxy.0.lock().unwrap().commands[0]
            .cancelled
            .load(Ordering::Acquire)
    );
    let quit = proxy.request(Action::Quit);
    drop(quit);
    assert!(proxy.0.lock().unwrap().terminal_quit.is_some());
    assert!(
        !proxy
            .0
            .lock()
            .unwrap()
            .terminal_quit
            .as_ref()
            .unwrap()
            .cancelled
            .load(Ordering::Acquire)
    );
    document.shutdown().unwrap();
}

#[test]
#[ignore = "requires a GPU adapter and the software Slint event loop"]
fn two_actual_hosts_keep_peer_edits_and_release_closed_claims_before_reopen_and_last_close() {
    software(true);
    let (directory, game) = crate::test_assets::game();
    let application = crate::test_assets::application(Ok(game), directory.path().join("recovery"));
    let factory = application.workspace();
    let a = application.document();
    let source = directory.path().join("workspace-a.vmf");
    let peer_source = directory.path().join("workspace-b.vmf");
    {
        let mut session = lock(a.session());
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(16.0),
                "test/checker",
                false,
            )
            .unwrap();
        session.save(Actor::User, Some(&source)).unwrap();
    }
    let source_bytes = std::fs::read(&source).unwrap();
    let ctx = GpuContext::new().unwrap();
    let retained_server = mcp::EditorServer::for_document(a.clone(), Some(ctx.clone()));
    let (owner, commands, selector_a, app_a) = attach(&application, ctx.clone());
    let result = Rc::new(RefCell::new(None));
    let final_document = Rc::new(RefCell::new(None));
    let completed = Rc::new(Cell::new(false));
    let (stop_watchdog, watchdog_stop) = mpsc::sync_channel(1);
    let watchdog = std::thread::spawn(move || {
        if watchdog_stop.recv_timeout(TIMEOUT * 3).is_err() {
            let _ = slint::quit_event_loop();
        }
    });
    let (flow_owner, flow_result, flow_final, flow_completed) = (
        owner.clone(),
        result.clone(),
        final_document.clone(),
        completed.clone(),
    );
    let workspace = commands.workspace();
    slint::spawn_local(async move {
        let flow = async {
            let created = workspace.new_document(Actor::Agent, false).await?;
            let selector_b = created["document_id"]
                .as_str()
                .ok_or("Missing second selector")?
                .to_owned();
            let b = factory.resolve(Some(&selector_b))?.document();
            let host_b = host(&flow_owner, &selector_b)?;
            let app_b = host_b.borrow().window.app().clone_strong();
            if app_b.window().is_visible() {
                return Err("MCP creation unexpectedly showed the second host".into());
            }
            app_a.invoke_map_checks();
            let host_a = host(&flow_owner, &selector_a)?;
            let weak_a = host_a.borrow().window.weak_ui();
            let checks = weak_a
                .upgrade()
                .unwrap()
                .borrow()
                .checks_window
                .as_ref()
                .unwrap()
                .clone_strong();
            app_a.invoke_open_documents();
            let controller = flow_owner.borrow().dialog.as_ref().unwrap().clone();
            let list = controller.window.clone_strong();
            if list.get_rows().row_count() != 2 || list.get_can_create() {
                return Err("Actual document list did not reflect the two-host cap".into());
            }
            crate::ui::tests::save_ui_snapshot_at_current_time(list.window(), "open-documents");
            app_a
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            if a.lifecycle() != DocumentLifecycle::Live
                || !list.get_quit_review()
                || b.lifecycle() != DocumentLifecycle::Live
                || !list.get_quit_description().contains("hidden")
            {
                return Err(
                    "Last visible Close did not offer the complete reviewed Quit all workflow"
                        .into(),
                );
            }
            list.invoke_cancel_quit_all();
            if list.get_quit_review() || app_b.window().is_visible() {
                return Err("Cancelling Quit all changed the hidden peer".into());
            }
            list.invoke_show_document(selector_b.clone().into());
            wait_for(|| {
                factory
                    .resolve(Some(&selector_b))
                    .is_ok_and(|entry| entry.visibility() == DocumentVisibility::Visible)
                    && !list.get_busy()
            })
            .await?;
            if !app_b.window().is_visible() {
                return Err("Show document did not present the actual peer host".into());
            }
            drop(controller);
            let b_id = {
                let mut session = lock(b.session());
                session.create_block(
                    Actor::User,
                    DVec3::splat(32.0),
                    DVec3::splat(48.0),
                    "test/checker",
                    false,
                )?
            };
            let before_b = lock(b.session()).document().vmf().clone();
            let history_b = lock(b.session()).history_snapshot();
            app_a
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            wait_for(|| {
                factory
                    .documents()
                    .iter()
                    .all(|entry| entry.selector() != selector_a)
            })
            .await?;
            if a.lifecycle() != DocumentLifecycle::Finished
                || weak_a.upgrade().is_some()
                || checks.window().is_visible()
            {
                return Err("Closing A did not physically retire its actual owned UI".into());
            }
            checks.invoke_run_checks();
            list.invoke_new_document();
            app_a.invoke_open_documents();
            if flow_owner.borrow().dialog.is_some() || factory.documents().len() != 1 {
                return Err(
                    "Retained closed UI callbacks created another workspace operation".into(),
                );
            }
            if lock(b.session()).document().vmf() != &before_b
                || lock(b.session()).history_snapshot() != history_b
            {
                return Err("Closing A changed its live peer".into());
            }
            app_b.invoke_open_documents();
            let live_controller = flow_owner.borrow().dialog.as_ref().unwrap().clone();
            live_controller.window.invoke_new_document();
            wait_for(|| factory.documents().len() == 2 && !live_controller.window.get_busy())
                .await?;
            let created = factory
                .documents()
                .into_iter()
                .find(|entry| entry.selector() != selector_b)
                .ok_or("UI New did not create a second document")?;
            if !host(&flow_owner, created.selector())?
                .borrow()
                .window
                .app()
                .window()
                .is_visible()
            {
                return Err("UI New did not show the new actual host".into());
            }
            let created_document = created.document();
            workspace.close_native(
                created.selector(),
                &created_document.document_close_review()?,
                false,
            )?;
            wait_for(|| factory.documents().len() == 1).await?;
            controls::submit(
                &Rc::downgrade(&flow_owner),
                &Rc::downgrade(&live_controller),
                controls::Operation::Open(source.clone()),
            );
            wait_for(|| factory.documents().len() == 2 && !live_controller.window.get_busy())
                .await?;
            let reopened = factory
                .documents()
                .into_iter()
                .find(|entry| entry.selector() != selector_b)
                .ok_or("UI Open did not enroll the saved source")?;
            let selector_c = reopened.selector().to_owned();
            let c = reopened.document();
            if selector_c == selector_a
                || a.admit().is_ok()
                || std::fs::read(&source).map_err(|error| error.to_string())? != source_bytes
            {
                return Err("Closed source identity or saved bytes were reused incorrectly".into());
            }
            let review = c.document_close_review()?;
            workspace.close_native(&selector_c, &review, false)?;
            wait_for(|| {
                factory
                    .documents()
                    .iter()
                    .all(|entry| entry.selector() != selector_c)
            })
            .await?;
            app_b.invoke_undo();
            if lock(b.session()).document().contains(b_id) {
                return Err("The remaining window's actual Undo callback was not usable".into());
            }
            lock(b.session()).save(Actor::User, Some(&peer_source))?;
            if b.document_close_review()?.requires_discard() {
                return Err("The saved peer still requires discard consent".into());
            }
            {
                let weak = host_b.borrow().window.weak_ui();
                let ui = weak.upgrade().ok_or("The peer UI was retired")?;
                let mut ui = ui.borrow_mut();
                let mut session = lock(b.session());
                let mesh = session.build_scene();
                ui.viewport.set_scene(&ctx, &mesh, session.materials())?;
                let frame = ui
                    .viewport
                    .render(&ctx, (160, 120), 1.0, &mut session)
                    .ok_or("The remaining real viewport did not render")?;
                app_b.set_frame(frame);
            }
            drop(retained_server);
            *flow_final.borrow_mut() = Some(b.clone());
            app_b
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            if b.lifecycle() == DocumentLifecycle::Live {
                return Err("The final native Close was not accepted".into());
            }
            flow_completed.set(true);
            Ok::<(), String>(())
        }
        .await;
        let failed = flow.is_err();
        *flow_result.borrow_mut() = Some(flow);
        if failed {
            let _ = slint::quit_event_loop();
        }
    })
    .unwrap();
    let primary = slint::run_event_loop_until_quit().map_err(Into::into);
    let quit = owner.borrow().proxy.0.lock().unwrap().quit.clone();
    let _ = stop_watchdog.send(());
    watchdog.join().unwrap();
    let ui_result = owner.borrow_mut().finish(primary);
    let close_result = commands.finish(Ok(()));
    let application_result = application.finish(Ok(()));
    assert_eq!(result.borrow_mut().take().unwrap(), Ok(()));
    assert!(completed.get());
    assert_eq!(quit, Some(Ok(())));
    ui_result.unwrap();
    close_result.unwrap();
    application_result.unwrap();
    assert_eq!(
        final_document.borrow().as_ref().unwrap().lifecycle(),
        DocumentLifecycle::Finished
    );
}

#[test]
#[ignore = "requires a GPU adapter and the software Slint event loop"]
fn actual_failed_retirement_remains_visible_and_final_peer_close_reports_the_error() {
    software(true);
    let (directory, game) = crate::test_assets::game();
    let application = crate::test_assets::application(Ok(game), directory.path().join("recovery"));
    let peer_source = directory.path().join("workspace-b.vmf");
    let factory = application.workspace();
    let a = application.document();
    let ctx = GpuContext::new().unwrap();
    let (owner, commands, selector_a, app_a) = attach(&application, ctx);
    let result = Rc::new(RefCell::new(None));
    let (flow_owner, flow_result) = (owner.clone(), result.clone());
    let workspace = commands.workspace();
    let (done, completion) = mpsc::sync_channel(1);
    let watchdog = std::thread::spawn(move || {
        if completion.recv_timeout(TIMEOUT * 3).is_err() {
            let _ = slint::quit_event_loop();
        }
    });
    slint::spawn_local(async move {
        let flow = async {
            let created = workspace.new_document(Actor::Agent, true).await?;
            let selector_b = created["document_id"]
                .as_str()
                .ok_or("Missing peer selector")?
                .to_owned();
            let b = factory.resolve(Some(&selector_b))?.document();
            let app_b = host(&flow_owner, &selector_b)?
                .borrow()
                .window
                .app()
                .clone_strong();
            super::super::document_window::tests::inject_hide_failure("Editor");
            workspace.close_native(&selector_a, &a.document_close_review()?, false)?;
            wait_for(|| {
                workspace.list_documents().is_ok_and(|report| {
                    report["documents"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|row| row["document_id"] == selector_a && row["lifecycle"] == "failed")
                })
            })
            .await?;
            if a.admit().is_ok() || b.lifecycle() != DocumentLifecycle::Live {
                return Err("A failed Close changed peer admission or reopened its target".into());
            }
            app_b.invoke_open_documents();
            let list = flow_owner
                .borrow()
                .dialog
                .as_ref()
                .unwrap()
                .window
                .clone_strong();
            if list.get_rows().row_count() != 2 || list.get_can_create() {
                return Err("Failed retirement did not retain the visible capacity record".into());
            }
            let created = lock(b.session()).create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(16.0),
                "test/checker",
                false,
            )?;
            app_b.invoke_undo();
            if lock(b.session()).document().contains(created) {
                return Err("The peer could not edit and Undo after a failed Close".into());
            }
            lock(b.session()).save(Actor::User, Some(&peer_source))?;
            if b.document_close_review()?.requires_discard() {
                return Err("The saved peer still requires discard consent".into());
            }
            app_b
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            if b.lifecycle() == DocumentLifecycle::Live {
                return Err("The final peer Close was not accepted".into());
            }
            Ok::<(), String>(())
        }
        .await;
        let failed = flow.is_err();
        *flow_result.borrow_mut() = Some(flow);
        if failed {
            let _ = slint::quit_event_loop();
        }
    })
    .unwrap();
    let primary = slint::run_event_loop_until_quit().map_err(Into::into);
    let quit = owner.borrow().proxy.0.lock().unwrap().quit.clone();
    let _ = done.send(());
    watchdog.join().unwrap();
    let ui_error = owner.borrow_mut().finish(primary).unwrap_err().to_string();
    let workspace_error = commands.finish(Ok(())).unwrap_err().to_string();
    let application_error = application.finish(Ok(())).unwrap_err().to_string();
    app_a.hide().unwrap();
    assert_eq!(result.borrow_mut().take().unwrap(), Ok(()));
    assert_eq!(quit, Some(Ok(())));
    assert!(ui_error.contains("injected hide failure"));
    assert!(workspace_error.contains("injected hide failure"));
    assert!(application_error.contains("injected hide failure"));
}

#[test]
#[ignore = "requires a GPU adapter and the software Slint event loop"]
fn actual_save_all_button_reports_unnamed_peer_then_saves_two_named_maps_without_showing_it() {
    software(true);
    let (directory, game) = crate::test_assets::game();
    let application = crate::test_assets::application(Ok(game), directory.path().join("recovery"));
    let factory = application.workspace();
    let a = application.document();
    let first_path = directory.path().join("save-all-a.vmf");
    let second_path = directory.path().join("save-all-b.vmf");
    lock(a.session())
        .save(Actor::User, Some(&first_path))
        .unwrap();
    let ctx = GpuContext::new().unwrap();
    let (owner, commands, selector_a, app_a) = attach(&application, ctx);
    let workspace = commands.workspace();
    let result = Rc::new(RefCell::new(None));
    let (flow_owner, flow_result) = (owner.clone(), result.clone());
    let (stop, stopped) = mpsc::sync_channel(1);
    let watchdog = std::thread::spawn(move || {
        if stopped.recv_timeout(TIMEOUT * 3).is_err() {
            let _ = slint::quit_event_loop();
        }
    });
    slint::spawn_local(async move {
        let flow = async {
            let created = workspace.new_document(Actor::Agent, false).await?;
            let selector_b = created["document_id"]
                .as_str()
                .ok_or("Missing peer selector")?
                .to_owned();
            let b = factory.resolve(Some(&selector_b))?.document();
            let app_b = host(&flow_owner, &selector_b)?
                .borrow()
                .window
                .app()
                .clone_strong();
            for (document, x) in [(&a, 0.0), (&b, 64.0)] {
                lock(document.session()).create_block(
                    Actor::User,
                    DVec3::new(x, 0.0, 0.0),
                    DVec3::new(x + 16.0, 16.0, 16.0),
                    "test/checker",
                    false,
                )?;
            }
            let peer_source = lock(b.session()).document().vmf().clone();
            let peer_camera = lock(b.session()).camera();
            let peer_history = lock(b.session()).history_snapshot();
            app_a.invoke_open_documents();
            let controller = flow_owner.borrow().dialog.as_ref().unwrap().clone();
            let list = controller.window.clone_strong();
            list.window().set_size(slint::PhysicalSize::new(900, 850));
            list.window()
                .take_snapshot()
                .map_err(|error| error.to_string())?;
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &list,
                "Save all named maps",
            )
            .next()
            .ok_or("Missing actual Save all button")?
            .invoke_accessible_default_action();
            wait_for(|| !list.get_busy() && list.get_message().contains("not complete")).await?;
            let rows = list.get_rows();
            if !rows.iter().any(|row| {
                row.selector == selector_b
                    && row.detail.contains("Needs Save as")
                    && row.detail.contains("Save as")
            }) {
                return Err(
                    "The actual list omitted the unnamed peer's Save as requirement".into(),
                );
            }
            if app_b.window().is_visible()
                || lock(b.session()).document().vmf() != &peer_source
                || lock(b.session()).camera() != peer_camera
                || lock(b.session()).history_snapshot() != peer_history
            {
                return Err("Save all showed or changed the unnamed peer".into());
            }
            crate::ui::tests::save_ui_snapshot_at_current_time(
                list.window(),
                "save-all-named-maps",
            );
            crate::ui::tests::assert_dialog_button_visible(&list, "Save all named maps");
            lock(b.session()).save(Actor::User, Some(&second_path))?;
            for (document, x) in [(&a, 128.0), (&b, 192.0)] {
                lock(document.session()).create_block(
                    Actor::User,
                    DVec3::new(x, 0.0, 0.0),
                    DVec3::new(x + 16.0, 16.0, 16.0),
                    "test/checker",
                    false,
                )?;
            }
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &list,
                "Save all named maps",
            )
            .next()
            .ok_or("Missing retained Save all button")?
            .invoke_accessible_default_action();
            wait_for(|| !list.get_busy() && list.get_message().contains("2 saved")).await?;
            if lock(a.session()).document().is_dirty()
                || lock(b.session()).document().is_dirty()
                || app_b.window().is_visible()
            {
                return Err(
                    "Save all did not save both fixed maps or showed the hidden peer".into(),
                );
            }
            if !list
                .get_rows()
                .iter()
                .all(|row| row.detail.contains("Last Save all result: Saved"))
            {
                return Err("The actual list lost per-map successful save results".into());
            }
            workspace.close_native(&selector_b, &b.document_close_review()?, false)?;
            wait_for(|| factory.documents().len() == 1).await?;
            workspace.close_native(&selector_a, &a.document_close_review()?, false)?;
            Ok::<(), String>(())
        }
        .await;
        let failed = flow.is_err();
        *flow_result.borrow_mut() = Some(flow);
        if failed {
            let _ = slint::quit_event_loop();
        }
    })
    .unwrap();
    let primary = slint::run_event_loop_until_quit().map_err(Into::into);
    let _ = stop.send(());
    watchdog.join().unwrap();
    let ui_result = owner.borrow_mut().finish(primary);
    let workspace_result = commands.finish(Ok(()));
    let application_result = application.finish(Ok(()));
    assert_eq!(result.borrow_mut().take().unwrap(), Ok(()));
    ui_result.unwrap();
    workspace_result.unwrap();
    application_result.unwrap();
}

#[test]
#[ignore = "requires a GPU adapter and the software Slint event loop"]
fn actual_quit_all_review_cancels_rejects_hidden_changes_and_closes_both_hosts() {
    software(true);
    let (directory, game) = crate::test_assets::game();
    let application = crate::test_assets::application(Ok(game), directory.path().join("recovery"));
    let factory = application.workspace();
    let a = application.document();
    let ctx = GpuContext::new().unwrap();
    let (owner, commands, _, app_a) = attach(&application, ctx);
    let workspace = commands.workspace();
    let result = Rc::new(RefCell::new(None));
    let (flow_owner, flow_result) = (owner.clone(), result.clone());
    let peer = Rc::new(RefCell::new(None));
    let flow_peer = peer.clone();
    let (stop, stopped) = mpsc::sync_channel(1);
    let watchdog = std::thread::spawn(move || {
        if stopped.recv_timeout(TIMEOUT * 3).is_err() {
            let _ = slint::quit_event_loop();
        }
    });
    slint::spawn_local(async move {
        let flow = async {
            let created = workspace.new_document(Actor::Agent, false).await?;
            let selector = created["document_id"]
                .as_str()
                .ok_or("Missing peer selector")?;
            let b = factory.resolve(Some(selector))?.document();
            *flow_peer.borrow_mut() = Some(b.clone());
            let app_b = host(&flow_owner, selector)?
                .borrow()
                .window
                .app()
                .clone_strong();
            app_a
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            let controller = flow_owner
                .borrow()
                .dialog
                .as_ref()
                .ok_or("Native Close did not open the review")?
                .clone();
            let list = controller.window.clone_strong();
            if !list.get_quit_review()
                || list.get_quit_discard_required()
                || !list.get_quit_description().contains("hidden")
                || a.lifecycle() != DocumentLifecycle::Live
                || b.lifecycle() != DocumentLifecycle::Live
            {
                return Err(
                    "Clean Quit all review omitted a hidden map or accepted without confirmation"
                        .into(),
                );
            }
            list.invoke_cancel_quit_all();
            if list.get_quit_review() || app_b.window().is_visible() {
                return Err("Cancel quit changed the hidden document".into());
            }
            lock(a.session()).create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(16.0),
                "test/checker",
                false,
            )?;
            lock(b.session()).preview_block(DVec3::ZERO, DVec3::splat(16.0))?;
            list.window().set_size(slint::PhysicalSize::new(1000, 900));
            list.window()
                .take_snapshot()
                .map_err(|error| error.to_string())?;
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(&list, "Quit all…")
                .next()
                .ok_or("Missing discoverable Quit all button")?
                .invoke_accessible_default_action();
            if !list.get_quit_review()
                || !list.get_quit_discard_required()
                || !list.get_quit_description().contains("block")
            {
                return Err("Quit all did not review the hidden pending draft".into());
            }
            lock(b.session()).preview_block(DVec3::splat(32.0), DVec3::splat(48.0))?;
            list.invoke_confirm_quit_all();
            if list.get_quit_review()
                || !list.get_message().contains("changed")
                || a.lifecycle() != DocumentLifecycle::Live
                || b.lifecycle() != DocumentLifecycle::Live
                || app_b.window().is_visible()
            {
                return Err("Stale Quit all accepted or changed a live host".into());
            }
            list.invoke_quit_all();
            list.window()
                .take_snapshot()
                .map_err(|error| error.to_string())?;
            crate::ui::tests::save_ui_snapshot_at_current_time(
                list.window(),
                "quit-all-reviewed-hidden-draft",
            );
            crate::ui::tests::assert_dialog_button_visible(
                &list,
                "Discard reviewed changes and quit all",
            );
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &list,
                "Discard reviewed changes and quit all",
            )
            .next()
            .ok_or("Missing explicit discard-and-quit button")?
            .invoke_accessible_default_action();
            if !list.get_busy() {
                return Err("Reviewed Quit all was not accepted".into());
            }
            Ok::<(), String>(())
        }
        .await;
        let failed = flow.is_err();
        *flow_result.borrow_mut() = Some(flow);
        if failed {
            let _ = slint::quit_event_loop();
        }
    })
    .unwrap();
    let primary = slint::run_event_loop_until_quit().map_err(Into::into);
    let _ = stop.send(());
    watchdog.join().unwrap();
    let ui_result = owner.borrow_mut().finish(primary);
    let workspace_result = commands.finish(Ok(()));
    assert!(application.workspace().documents().is_empty());
    assert_eq!(
        application.document().lifecycle(),
        DocumentLifecycle::Finished
    );
    assert_eq!(
        peer.borrow().as_ref().unwrap().lifecycle(),
        DocumentLifecycle::Finished
    );
    let application_result = application.finish(Ok(()));
    assert_eq!(result.borrow_mut().take().unwrap(), Ok(()));
    ui_result.unwrap();
    workspace_result.unwrap();
    application_result.unwrap();
}
