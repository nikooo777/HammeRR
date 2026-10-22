use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use slint::Model;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn recent_open_callback_rejects_review_from_before_peer_save_as() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.vmf");
    let copy = directory.path().join("copy.vmf");
    std::fs::write(&path, "world { id 1 classname worldspawn }").unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state.configure_recovery(Ok(directory.path().join("recovery")));
    state.configure_recent_maps(Ok(directory.path().join("preferences")));
    state.open(Actor::User, &path, false).unwrap();
    let shared = Arc::new(Mutex::new(state));
    let document = crate::test_assets::document(shared.clone());
    let window = RecentMapsWindow::new().unwrap();
    bind_with_access(
        &window,
        &shared,
        DocumentAccess::for_document(document.clone()),
    );
    refresh(&window, &lock(&shared));
    window.set_selected_path(path.display().to_string().into());
    window.set_discard_changes(true);
    let review = window.get_map_guard();
    let revision = lock(&shared).document().revision();
    lock(&shared).save(Actor::Agent, Some(&copy)).unwrap();
    assert_eq!(lock(&shared).document().revision(), revision);
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let saved = std::fs::read(&copy).unwrap();
    window.invoke_open_selected();
    assert!(window.get_message().contains("Map changed"));
    assert_ne!(window.get_map_guard(), review);
    assert!(!window.get_discard_changes());
    assert_eq!(lock(&shared).path(), Some(copy.as_path()));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(std::fs::read(&copy).unwrap(), saved);
    window.invoke_open_selected();
    assert_eq!(lock(&shared).path(), Some(path.as_path()));
}

#[test]
fn reload_saved_list_callback_recovers_a_conflict_and_preserves_the_owned_review() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.vmf");
    let second = directory.path().join("second.vmf");
    for path in [&first, &second] {
        std::fs::write(path, "world { id 1 classname worldspawn }").unwrap();
    }
    let mut state = Session::new(Err("no assets needed".into()));
    state.configure_recent_maps(Ok(directory.path().into()));
    state.open(Actor::User, &first, false).unwrap();
    state
        .preview_measurement(
            Actor::Agent,
            editor_core::session::measurements::Request {
                id: None,
                name: "Owned clearance".into(),
                start: glam::DVec3::ZERO,
                end: glam::DVec3::X,
            },
            &state.measurement_stamp(),
        )
        .unwrap();
    let owned = state.measurement_state()["draft"].clone();
    let document = state.document().vmf().clone();
    let camera = state.camera();
    let shared = Arc::new(Mutex::new(state));
    let window = RecentMapsWindow::new().unwrap();
    bind(&window, &shared);
    refresh(&window, &lock(&shared));
    window.show().unwrap();
    window.window().set_size(slint::PhysicalSize::new(800, 760));
    crate::ui::tests::assert_dialog_button_visible(&window, "Reload saved list");
    let mut other = Session::new(Err("no assets needed".into()));
    other.configure_recent_maps(Ok(directory.path().into()));
    other.open(Actor::Agent, &second, false).unwrap();
    let path = directory.path().join(crate::recent_maps::FILE_NAME);
    let saved = std::fs::read(&path).unwrap();
    window.set_confirm_clear(true);
    window.invoke_clear_list();
    assert!(!window.get_writable());
    assert!(window.get_warning().contains("Reload saved list"));
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    crate::ui::tests::save_ui_snapshot(window.window(), "recent-map-conflicts");
    window.invoke_refresh();
    assert!(window.get_writable());
    assert!(window.get_warning().is_empty());
    assert_eq!(window.get_rows().row_count(), 2);
    assert!(!window.get_confirm_clear());
    std::fs::write(&path, b"broken preferences").unwrap();
    window.invoke_refresh();
    assert!(!window.get_writable());
    assert_eq!(window.get_rows().row_count(), 2);
    std::fs::write(&path, &saved).unwrap();
    window.invoke_refresh();
    assert!(window.get_writable());
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &document);
    assert_eq!(state.camera(), camera);
    assert_eq!(state.measurement_state()["draft"], owned);
    assert_eq!(state.path(), Some(first.as_path()));
    assert_eq!(state.document().undo_labels().count(), 0);
    window.invoke_cancel();
}

#[test]
fn recent_map_callbacks_share_the_list_without_changing_the_other_document() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let authority =
        editor_core::recent_maps::Authority::configured(Ok(directory.path().to_owned()));
    let first = Arc::new(Mutex::new(Session::with_recent_maps(
        Err("no assets".into()),
        authority.clone(),
    )));
    let second = Arc::new(Mutex::new(Session::with_recent_maps(
        Err("no assets".into()),
        authority,
    )));
    let a = directory.path().join("a.vmf");
    let b = directory.path().join("b.vmf");
    for (session, path) in [(&first, &a), (&second, &b)] {
        std::fs::write(path, "world { id 1 classname worldspawn }").unwrap();
        lock(session).open(Actor::User, path, false).unwrap();
    }
    {
        let mut session = lock(&second);
        let stamp = session.measurement_stamp();
        session
            .preview_measurement(
                Actor::Agent,
                editor_core::session::measurements::Request {
                    id: None,
                    name: "Owned clearance".into(),
                    start: glam::DVec3::ZERO,
                    end: glam::DVec3::X,
                },
                &stamp,
            )
            .unwrap();
    }
    let source = lock(&second).document().vmf().clone();
    let history = lock(&second).history_snapshot();
    let draft = lock(&second).measurement_state()["draft"].clone();
    let camera = lock(&second).camera();
    let options = lock(&second).compile_options();
    let window = RecentMapsWindow::new().unwrap();
    bind(&window, &first);
    refresh(&window, &lock(&first));
    assert_eq!(window.get_rows().row_count(), 2);
    window.set_selected_path(b.display().to_string().into());
    window.invoke_remove_selected();
    assert_eq!(window.get_rows().row_count(), 1);
    assert_eq!(lock(&second).recent_maps_snapshot().entries.len(), 1);
    assert_eq!(lock(&second).path(), Some(b.as_path()));
    let revision = lock(&second).recent_maps_serial();
    let saved = std::fs::read(directory.path().join(crate::recent_maps::FILE_NAME)).unwrap();
    std::fs::write(
        directory.path().join(crate::recent_maps::FILE_NAME),
        b"broken",
    )
    .unwrap();
    window.invoke_refresh();
    assert!(!window.get_writable());
    assert!(lock(&second).recent_maps_serial() > revision);
    assert_eq!(lock(&second).recent_maps()["persistence"], "blocked");
    std::fs::write(directory.path().join(crate::recent_maps::FILE_NAME), saved).unwrap();
    window.invoke_refresh();
    assert!(window.get_writable());
    assert!(lock(&second).recent_maps()["error"].is_null());
    let session = lock(&second);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.measurement_state()["draft"], draft);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_options(), options);
    window.invoke_cancel();
}

fn admitted_fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    DocumentAccess,
    RecentMapsWindow,
    crate::recent_maps::Authority,
) {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let authority =
        crate::recent_maps::Authority::configured(Ok(directory.path().join("preferences")));
    let mut state = Session::with_recent_maps(Err("no assets required".into()), authority.clone());
    state.configure_recovery(Ok(directory.path().join("recovery")));
    for name in ["target.vmf", "source.vmf"] {
        let path = directory.path().join(name);
        std::fs::write(&path, "world { id 1 classname worldspawn }").unwrap();
        state.open(Actor::User, &path, true).unwrap();
    }
    state
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(state));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = RecentMapsWindow::new().unwrap();
    bind_with_access(&window, &shared, access.clone());
    access
        .with_session(|session| {
            refresh(&window, session);
            Ok(())
        })
        .unwrap();
    (directory, document, shared, access, window, authority)
}

fn form(window: &RecentMapsWindow) -> serde_json::Value {
    let rows = window.get_rows();
    serde_json::json!({
        "selected":window.get_selected_path().to_string(), "detail":window.get_selected_detail().to_string(),
        "review":window.get_map_guard().to_string(), "discard":window.get_discard_changes(),
        "confirm":window.get_confirm_clear(), "summary":window.get_summary().to_string(),
        "rows":(0..rows.row_count()).map(|index| rows.row_data(index).unwrap().path.to_string()).collect::<Vec<_>>()
    })
}

fn terminal_actions(window: &RecentMapsWindow) {
    window.invoke_clear_list();
    window.invoke_remove_selected();
    window.invoke_select_map("never-opened.vmf".into());
    window.invoke_refresh();
    window.invoke_open_selected();
}

#[test]
fn recent_dialog_preserves_review_form_files_and_live_peer_during_and_after_real_drain() {
    let (directory, document, shared, access, window, authority) = admitted_fixture();
    let target = directory.path().join("target.vmf");
    window.invoke_select_map(target.display().to_string().into());
    window.set_confirm_clear(true);
    window.set_discard_changes(true);
    let expected_form = form(&window);
    let mut peer = Session::with_recent_maps(Err("peer assets unavailable".into()), authority);
    peer.configure_recovery(Ok(directory.path().join("peer-recovery")));
    peer.open(Actor::Agent, &target, true).unwrap();
    let peer_shared = Arc::new(Mutex::new(peer));
    let peer_document = crate::test_assets::document(peer_shared.clone());
    let peer_access = DocumentAccess::for_document(peer_document.clone());
    let peer_window = RecentMapsWindow::new().unwrap();
    bind_with_access(&peer_window, &peer_shared, peer_access.clone());
    peer_access
        .with_session(|session| {
            refresh(&peer_window, session);
            Ok(())
        })
        .unwrap();
    let peer_source = lock(&peer_shared).document().vmf().clone();
    let peer_history = lock(&peer_shared).history_snapshot();
    let list = lock(&shared).recent_maps();
    let preferences = directory
        .path()
        .join("preferences")
        .join(crate::recent_maps::FILE_NAME);
    let saved = std::fs::read(&preferences).unwrap();
    let input = std::fs::read(&target).unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let gate = document.admit().unwrap();
    let draining = document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while document.lifecycle() == DocumentLifecycle::Live
        || lock(&shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline, "document did not begin draining");
        std::thread::yield_now();
    }
    window.show().unwrap();
    terminal_actions(&window);
    assert_eq!(window.get_message(), "the editor is closing");
    assert_eq!(form(&window), expected_form);
    assert!(window.window().is_visible());
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let snapshot = store.read(&entries[0].id).unwrap().1;
    terminal_actions(&window);
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(form(&window), expected_form);
    assert!(window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).recent_maps(), list);
    assert_eq!(std::fs::read(&preferences).unwrap(), saved);
    assert_eq!(std::fs::read(&target).unwrap(), input);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, snapshot);
    assert_eq!(lock(&peer_shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer_shared).history_snapshot(), peer_history);
    peer_window.invoke_select_map(target.display().to_string().into());
    peer_window.invoke_remove_selected();
    assert!(
        !lock(&peer_shared)
            .recent_maps_snapshot()
            .entries
            .iter()
            .any(|entry| entry.path == target)
    );
    assert_eq!(peer_document.lifecycle(), DocumentLifecycle::Live);
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    drop(access);
}

#[test]
fn retained_recent_window_does_not_keep_session_or_document_owner_alive() {
    let (_directory, document, shared, access, window, _authority) = admitted_fixture();
    let weak = Arc::downgrade(&shared);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    window.invoke_refresh();
    window.invoke_open_selected();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}

#[test]
fn admitted_actual_recent_reload_finishes_its_form_body_before_final_recovery() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let (directory, document, shared, _access, window, _authority) = admitted_fixture();
    let source = lock(&shared).document().vmf().clone();
    window.set_confirm_clear(true);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let (done, completed) = mpsc::sync_channel(1);
    let completed = Rc::new(completed);
    let worker = Rc::new(RefCell::new(None));
    let observed = Rc::new(Cell::new(false));
    BEFORE_ACTION.with(|hook| {
        let (completed, worker, observed, document, store) = (
            completed.clone(),
            worker.clone(),
            observed.clone(),
            document.clone(),
            store.clone(),
        );
        *hook.borrow_mut() = Some(Box::new(move || {
            let draining = document.clone();
            *worker.borrow_mut() = Some(std::thread::spawn(move || {
                done.send(draining.shutdown()).unwrap()
            }));
            let deadline = Instant::now() + Duration::from_secs(5);
            while document.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline, "document did not begin draining");
                std::thread::yield_now();
            }
            assert!(completed.try_recv().is_err());
            assert!(store.list().unwrap().entries.is_empty());
            observed.set(true);
        }));
    });
    window.invoke_refresh();
    assert!(observed.get());
    assert!(!window.get_confirm_clear());
    assert_eq!(
        window.get_message(),
        "Saved list reloaded; the current map is unchanged."
    );
    completed
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    worker.borrow_mut().take().unwrap().join().unwrap();
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        source_formats::vmf::Vmf::parse(&store.read(&entries[0].id).unwrap().1).unwrap(),
        source
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
}
