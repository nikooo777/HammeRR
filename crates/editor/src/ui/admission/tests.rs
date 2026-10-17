use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, SharedSession, lock};
use crate::session::{Actor, Session};
use glam::DVec3;
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
