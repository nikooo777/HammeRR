use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, SharedSession, lock};
use crate::session::{Actor, Session};
use glam::DVec3;
use slint::Model;
use std::cell::Cell;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_INTERVAL: Duration = Duration::from_millis(8);

struct Owner {
    session: SharedSession,
    calls: Cell<usize>,
    held: Cell<bool>,
    releases: Cell<usize>,
}

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    DocumentAccess,
    Rc<Owner>,
    AppWindow,
) {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    edit(&mut session, 0.0);
    let session = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(session.clone());
    let access = DocumentAccess::for_document(document.clone());
    let owner = Rc::new(Owner {
        session,
        calls: Cell::new(0),
        held: Cell::new(false),
        releases: Cell::new(0),
    });
    (
        directory,
        document,
        access,
        owner,
        AppWindow::new().unwrap(),
    )
}

fn edit(session: &mut Session, offset: f64) {
    session
        .create_block(
            Actor::User,
            DVec3::splat(offset),
            DVec3::splat(offset + 16.0),
            "test",
            false,
        )
        .unwrap();
}

fn tick() {
    i_slint_backend_testing::mock_elapsed_time(FRAME_INTERVAL);
    slint::platform::update_timers_and_animations();
}

#[test]
fn registered_key_input_acts_live_then_consumes_terminal_calls_without_touching_peer() {
    let (_directory, document, access, owner, app) = fixture();
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    edit(&mut peer, 0.0);
    let peer_before = peer.history_snapshot();
    app.on_key({
        let scope = Callback::new(&app, &owner, &access);
        move |_, _, _, _, _, _| {
            scope.input(true, |_, owner| {
                owner.calls.set(owner.calls.get() + 1);
                edit(&mut lock(&owner.session), 64.0);
                false
            })
        }
    });
    assert!(!app.invoke_key("edit".into(), true, false, false, false, false));
    assert_eq!(owner.calls.get(), 1);
    document.shutdown().unwrap();
    let history = lock(&owner.session).history_snapshot();
    let source = lock(&owner.session).document().vmf().clone();
    assert!(app.invoke_key("edit".into(), true, false, false, false, false));
    assert_eq!(app.get_message(), "the document is closed");
    assert_eq!(owner.calls.get(), 1);
    assert_eq!(lock(&owner.session).history_snapshot(), history);
    assert_eq!(lock(&owner.session).document().vmf(), &source);
    assert_eq!(peer.history_snapshot(), peer_before);
}

#[test]
fn configured_view_keys_use_the_originating_admitted_callback_and_remember_cameras() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let authority =
        editor_core::shortcuts::Authority::configured(Ok(directory.path().join("shortcuts")));
    let mut session = Session::with_keyboard_shortcuts(Err("no assets".into()), authority.clone());
    session.configure_recovery(Ok(directory.path().join("recovery")));
    edit(&mut session, 0.0);
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let peer = Session::with_keyboard_shortcuts(Err("peer assets".into()), authority);
    let peer_camera = peer.camera();
    let owner = Rc::new(Owner {
        session: shared.clone(),
        calls: Cell::new(0),
        held: Cell::new(false),
        releases: Cell::new(0),
    });
    let controller = crate::shortcuts_ui::Controller::new_with_access(&access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert_eq!(
        controller.window.get_commands().row_count(),
        editor_core::shortcuts::Command::ALL.len()
    );
    for (command, key) in [
        ("top", "F10"),
        ("perspective", "F11"),
        ("objects", "F8"),
        ("faces", "F9"),
        ("show_grid", "F4"),
        ("snap_to_grid", "F5"),
        ("tool_textures", "F6"),
        ("selection_outlines", "F7"),
        ("select_tool", "F1"),
        ("move_tool", "F2"),
        ("rotate_tool", "F3"),
        ("scale_tool", "F12"),
    ] {
        controller.window.invoke_choose(command.into());
        assert_eq!(controller.window.get_selected(), command);
        assert_eq!(controller.window.get_primary_key(), 0);
        let key_index = controller
            .window
            .get_keys()
            .iter()
            .position(|value| value == key)
            .unwrap();
        controller.window.set_primary_key(key_index as i32);
        controller.window.set_primary_control(false);
        controller.window.set_primary_shift(false);
        controller.window.invoke_edited();
    }
    for (command, key) in [("draw_brush", "F10"), ("clip_mode", "F11")] {
        controller.window.invoke_choose(command.into());
        assert_eq!(controller.window.get_primary_key(), 0);
        let key_index = controller
            .window
            .get_keys()
            .iter()
            .position(|value| value == key)
            .unwrap();
        controller.window.set_primary_key(key_index as i32);
        controller.window.set_primary_control(false);
        controller.window.set_primary_shift(true);
        controller.window.invoke_edited();
    }
    controller.window.invoke_save();
    assert_eq!(peer.shortcut_bindings(), lock(&shared).shortcut_bindings());
    let app = AppWindow::new().unwrap();
    app.on_key({
        let scope = Callback::new(&app, &owner, &access);
        move |text, down, control, shift, alt, meta| {
            scope.input(true, |app, owner| {
                crate::shortcuts_ui::route_viewport_key(
                    &app,
                    &owner.session,
                    crate::shortcuts_ui::ViewportKey {
                        text: &text,
                        down,
                        control,
                        shift,
                        alt,
                        meta,
                    },
                    |_, _, _| false,
                    |_, _| false,
                )
            })
        }
    });
    let before = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let perspective = lock(&shared).camera();
    let top = lock(&shared).camera_for_projection(render::Projection::Top);
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F10).into(),
        true,
        false,
        false,
        false,
        false
    ));
    assert_eq!(lock(&shared).camera(), top);
    let mut remembered = top;
    remembered.ortho_height += 128.0;
    lock(&shared).set_camera(remembered);
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F11).into(),
        true,
        false,
        false,
        false,
        false
    ));
    assert_eq!(lock(&shared).camera(), perspective);
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F10).into(),
        true,
        false,
        false,
        false,
        false
    ));
    assert_eq!(lock(&shared).camera(), remembered);
    for (key, enabled) in [
        (slint::platform::Key::F9, true),
        (slint::platform::Key::F8, false),
    ] {
        assert!(app.invoke_key(
            crate::viewport::key_text(key).into(),
            true,
            false,
            false,
            false,
            false,
        ));
        assert_eq!(lock(&shared).face_mode(), enabled);
        assert_eq!(lock(&shared).camera(), remembered);
        assert!(!peer.face_mode());
    }
    assert!(lock(&shared).selected_ids().is_empty());
    let toggles = |session: &Session| {
        [
            session.spatial().grid_visible,
            session.spatial().snap,
            session.view().tool_textures,
            session.view().selection_outlines,
        ]
    };
    let peer_toggles = toggles(&peer);
    for (index, key) in [
        slint::platform::Key::F4,
        slint::platform::Key::F5,
        slint::platform::Key::F6,
        slint::platform::Key::F7,
    ]
    .into_iter()
    .enumerate()
    {
        for enabled in [false, true] {
            assert!(app.invoke_key(
                crate::viewport::key_text(key).into(),
                true,
                false,
                false,
                false,
                false
            ));
            assert_eq!(toggles(&lock(&shared))[index], enabled);
            assert_eq!(toggles(&peer), peer_toggles);
        }
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(peer.camera(), peer_camera);
    let peer_tool = peer.transform_tool();
    for (key, mode) in [
        (slint::platform::Key::F2, Some(render::handles::Mode::Move)),
        (
            slint::platform::Key::F3,
            Some(render::handles::Mode::Rotate),
        ),
        (
            slint::platform::Key::F12,
            Some(render::handles::Mode::Scale),
        ),
        (slint::platform::Key::F1, None),
    ] {
        assert!(app.invoke_key(
            crate::viewport::key_text(key).into(),
            true,
            false,
            false,
            false,
            false,
        ));
        assert_eq!(lock(&shared).transform_tool().mode, mode);
        assert_eq!(peer.transform_tool(), peer_tool);
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F2).into(),
        true,
        false,
        false,
        false,
        false,
    ));
    {
        let mut session = lock(&shared);
        session
            .select(
                &[map_core::ObjectId::Solid(before.world.solids[0].id)],
                crate::session::SelectMode::Replace,
            )
            .unwrap();
        session
            .begin_transform_drag(render::handles::Handle::X, None)
            .unwrap();
        session
            .update_transform_drag(None, Some(glam::DVec2::new(32.0, 0.0)))
            .unwrap();
        session.commit_transform_preview(Actor::User).unwrap();
        assert_ne!(session.document().vmf(), &before);
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &before);
    }
    let history = lock(&shared).history_snapshot();
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F11).into(),
        true,
        false,
        false,
        false,
        false,
    ));
    let spatial = lock(&shared).spatial();
    assert_eq!(
        lock(&shared).camera().projection,
        render::Projection::Perspective
    );
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F10).into(),
        true,
        false,
        true,
        false,
        false,
    ));
    assert_eq!(
        lock(&shared).spatial(),
        editor_core::spatial::Settings {
            draw_block: true,
            ..spatial
        }
    );
    assert_eq!(lock(&shared).camera(), remembered);
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    lock(&shared)
        .select(&[], crate::session::SelectMode::Replace)
        .unwrap();
    lock(&shared)
        .preview_block(DVec3::splat(256.0), DVec3::splat(320.0))
        .unwrap();
    let pending = lock(&shared).block_preview();
    let view = lock(&shared).view_serial();
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F10).into(),
        true,
        false,
        true,
        false,
        false,
    ));
    assert_eq!(lock(&shared).block_preview(), pending);
    assert_eq!(lock(&shared).view_serial(), view);
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F11).into(),
        true,
        false,
        true,
        false,
        false,
    ));
    assert!(app.get_message().contains("Select brushes"));
    assert_eq!(lock(&shared).block_preview(), pending);
    assert_eq!(lock(&shared).view_serial(), view);
    assert!(!lock(&shared).clip_tool());
    lock(&shared).cancel_block_preview();
    lock(&shared)
        .select(
            &[map_core::ObjectId::Solid(before.world.solids[0].id)],
            crate::session::SelectMode::Replace,
        )
        .unwrap();
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F11).into(),
        true,
        false,
        true,
        false,
        false,
    ));
    assert!(lock(&shared).clip_tool());
    assert!(!lock(&shared).spatial().draw_block);
    let token = {
        let mut session = lock(&shared);
        session
            .clip_brushes(
                Actor::User,
                &[map_core::ObjectId::Solid(before.world.solids[0].id)],
                map_core::Plane {
                    normal: DVec3::X,
                    dist: 8.0,
                },
                map_core::clip::Keep::Both,
                None,
                true,
            )
            .unwrap();
        let snapshot = session.clip_point_stamp();
        session
            .set_clip_handle_options(
                crate::session::clip_drag::Options {
                    snap: false,
                    ..Default::default()
                },
                &snapshot,
            )
            .unwrap();
        let snapshot = session.clip_point_stamp();
        let token = session
            .begin_clip_drag(render::handles::Handle::X, None, &snapshot)
            .unwrap();
        session
            .update_clip_drag(token, None, Some(glam::DVec2::new(4.0, 0.0)))
            .unwrap();
        token
    };
    let preview = lock(&shared).render_document().vmf().clone();
    let serials = {
        let session = lock(&shared);
        (session.scene_serial(), session.view_serial())
    };
    assert!(app.invoke_key(
        crate::viewport::key_text(slint::platform::Key::F11).into(),
        true,
        false,
        true,
        false,
        false,
    ));
    assert_eq!(lock(&shared).render_document().vmf(), &preview);
    {
        let session = lock(&shared);
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
    }
    lock(&shared).finish_clip_drag(token).unwrap();
    lock(&shared).set_clip_tool(Actor::User, false).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert!(!peer.spatial().draw_block);
    assert!(!peer.clip_tool());
    document.shutdown().unwrap();
    let bytes = std::fs::read(
        directory
            .path()
            .join("shortcuts")
            .join(editor_core::shortcuts::FILE_NAME),
    )
    .unwrap();
    let log = lock(&shared).log_serial();
    let closed_toggles = toggles(&lock(&shared));
    for key in [
        slint::platform::Key::F9,
        slint::platform::Key::F4,
        slint::platform::Key::F5,
        slint::platform::Key::F6,
        slint::platform::Key::F7,
        slint::platform::Key::F1,
        slint::platform::Key::F2,
        slint::platform::Key::F3,
        slint::platform::Key::F12,
    ] {
        assert!(app.invoke_key(
            crate::viewport::key_text(key).into(),
            true,
            false,
            false,
            false,
            false
        ));
    }
    for key in [slint::platform::Key::F10, slint::platform::Key::F11] {
        assert!(app.invoke_key(
            crate::viewport::key_text(key).into(),
            true,
            false,
            true,
            false,
            false
        ));
    }
    assert!(!lock(&shared).spatial().draw_block);
    assert!(!lock(&shared).clip_tool());
    assert_eq!(app.get_message(), "the document is closed");
    assert_eq!(lock(&shared).camera(), remembered);
    assert!(!lock(&shared).face_mode());
    assert_eq!(lock(&shared).log_serial(), log);
    assert_eq!(toggles(&lock(&shared)), closed_toggles);
    assert_eq!(toggles(&peer), peer_toggles);
    assert_eq!(peer.camera(), peer_camera);
    assert_eq!(peer.transform_tool(), peer_tool);
    assert_eq!(
        lock(&shared).transform_tool().mode,
        Some(render::handles::Mode::Move)
    );
    assert_eq!(
        std::fs::read(
            directory
                .path()
                .join("shortcuts")
                .join(editor_core::shortcuts::FILE_NAME)
        )
        .unwrap(),
        bytes
    );
    controller.window.invoke_cancel();
}

#[test]
fn full_registered_pointer_body_remains_admitted_until_exit_capture_can_run() {
    let (directory, document, access, owner, app) = fixture();
    let recovery_directory = directory.path().join("recovery");
    let (entered, start) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let draining = document.clone();
    let shutdown = std::thread::spawn(move || {
        start.recv_timeout(TIMEOUT).unwrap();
        finished.send(draining.shutdown()).unwrap();
    });
    app.on_mouse_button({
        let scope = Callback::new(&app, &owner, &access);
        let observed = document.clone();
        let recovery_directory = recovery_directory.clone();
        move |_, _, _, _, _, _| {
            scope.input((), |_, owner| {
                entered.send(()).unwrap();
                let deadline = Instant::now() + TIMEOUT;
                while observed.lifecycle() == DocumentLifecycle::Live {
                    assert!(
                        Instant::now() < deadline,
                        "shutdown did not close admission"
                    );
                    std::thread::yield_now();
                }
                assert!(done.try_recv().is_err());
                let store = crate::recovery::Store {
                    directory: recovery_directory.clone(),
                };
                assert!(store.list().unwrap().entries.is_empty());
                edit(&mut lock(&owner.session), 96.0);
                owner.calls.set(owner.calls.get() + 1);
            })
        }
    });
    app.invoke_mouse_button(0, true, 10.0, 10.0, false, false);
    shutdown.join().unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(owner.calls.get(), 1);
    let status = lock(&owner.session).recovery_status();
    assert_eq!(status["pending_changes"], false);
    let entries = lock(&owner.session).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = crate::recovery::Store {
        directory: recovery_directory,
    };
    let (_, text) = store.read(&entries[0].id).unwrap();
    assert_eq!(
        source_formats::vmf::Vmf::parse(&text).unwrap(),
        *lock(&owner.session).document().vmf()
    );
    app.invoke_mouse_button(0, true, 10.0, 10.0, false, false);
    assert_eq!(owner.calls.get(), 1);
}

#[test]
fn callback_registrations_retain_neither_owner_nor_runtime_while_waiting() {
    let (directory, document, access, owner, app) = fixture();
    let weak_owner = Rc::downgrade(&owner);
    let shared = owner.session.clone();
    app.on_mouse_moved({
        let scope = Callback::new(&app, &owner, &access);
        move |_, _| {
            scope.input((), |_, owner| {
                owner.calls.set(owner.calls.get() + 1);
                edit(&mut lock(&owner.session), 64.0);
            })
        }
    });
    drop(owner);
    assert!(weak_owner.upgrade().is_none());
    app.invoke_mouse_moved(1.0, 1.0);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    drop(access);
    drop(document);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    assert_eq!(store.list().unwrap().entries.len(), 1);
    let before = lock(&shared).history_snapshot();
    app.invoke_mouse_moved(1.0, 1.0);
    assert_eq!(lock(&shared).history_snapshot(), before);
}

#[test]
fn missing_originating_window_abandons_input_while_fixed_runtime_remains_live() {
    let (_directory, document, access, owner, app) = fixture();
    let scope = Callback::new(&app, &owner, &access);
    drop(app);
    let blocked = scope.input(true, |_, _| panic!("missing window executed input"));
    assert!(blocked);
    scope.frame(|_, _| panic!("missing window executed frame"));
    assert_eq!(owner.calls.get(), 0);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
}

#[test]
fn real_timer_releases_each_frame_admission_and_drop_stops_future_ticks() {
    let (_directory, document, access, owner, app) = fixture();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, FRAME_INTERVAL, {
        let scope = Callback::new(&app, &owner, &access);
        move || scope.frame(|_, owner| owner.calls.set(owner.calls.get() + 1))
    });
    assert!(timer.running());
    tick();
    assert_eq!(owner.calls.get(), 1);
    drop(timer);
    tick();
    assert_eq!(owner.calls.get(), 1);
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, FRAME_INTERVAL, {
        let scope = Callback::new(&app, &owner, &access);
        move || scope.frame(|_, owner| owner.calls.set(owner.calls.get() + 1))
    });
    tick();
    assert_eq!(owner.calls.get(), 2);
    document.shutdown().unwrap();
    app.set_message("Preserve terminal status".into());
    tick();
    assert_eq!(owner.calls.get(), 2);
    assert_eq!(app.get_message(), "Preserve terminal status");
    drop(timer);
}

#[test]
fn focus_loss_releases_local_input_after_terminal_rejection_without_cancelling_session_preview() {
    let (_directory, document, access, owner, app) = fixture();
    app.on_viewport_focus_lost({
        let scope = Callback::new(&app, &owner, &access);
        move || {
            scope.focus_lost(
                |_, owner| {
                    owner.calls.set(owner.calls.get() + 1);
                    lock(&owner.session).cancel_block_preview();
                },
                |owner| {
                    owner.held.set(false);
                    owner.releases.set(owner.releases.get() + 1);
                },
            )
        }
    });
    lock(&owner.session)
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    owner.held.set(true);
    app.set_flying(true);
    app.invoke_viewport_focus_lost();
    assert!(lock(&owner.session).block_preview().is_none());
    assert!(!owner.held.get());
    assert!(!app.get_flying());
    assert_eq!(owner.calls.get(), 1);
    lock(&owner.session)
        .preview_block(DVec3::ZERO, DVec3::splat(64.0))
        .unwrap();
    let preview = lock(&owner.session).block_preview();
    owner.held.set(true);
    app.set_flying(true);
    document.shutdown().unwrap();
    app.invoke_viewport_focus_lost();
    assert_eq!(lock(&owner.session).block_preview(), preview);
    assert_eq!(owner.calls.get(), 1);
    assert_eq!(owner.releases.get(), 2);
    assert!(!owner.held.get());
    assert!(!app.get_flying());
    assert_eq!(app.get_message(), "the document is closed");
}

#[test]
fn failed_terminal_input_preserves_exact_failure_and_frame_rejection_stays_quiet() {
    let (directory, document, access, owner, app) = fixture();
    let scope = Callback::new(&app, &owner, &access);
    let recovery = directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    scope.input((), |_, _| panic!("failed document executed input"));
    assert_eq!(
        app.get_message(),
        format!("document shutdown failed: {error}")
    );
    app.set_message("Keep failure visible".into());
    scope.frame(|_, _| panic!("failed document executed frame"));
    assert_eq!(app.get_message(), "Keep failure visible");
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}
