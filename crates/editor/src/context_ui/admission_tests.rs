use super::*;
use crate::runtime::{DocumentLifecycle, lock};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

thread_local! {
    static ACTION_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

pub(super) fn before_action() {
    let hook = ACTION_HOOK.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[test]
fn accepted_context_apply_finishes_before_real_shutdown_final_recovery() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let recovery = directory.path().join("recovery");
    let mut session = Session::new(Err("No assets required".into()));
    session.configure_recovery(Ok(recovery.clone()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = ContextWindow::new().unwrap();
    bind_with_access(&window, &access);
    window.invoke_begin("map".into());
    window.set_summary("Accepted before shutdown".into());
    window.set_body("Final recovery must include this admitted note.".into());
    window.invoke_edit();
    let finished = Arc::new(AtomicBool::new(false));
    let thread = Rc::new(RefCell::new(None));
    let (send, receive) = mpsc::channel();
    ACTION_HOOK.with(|hook| {
        let document = document.clone();
        let finished = finished.clone();
        let thread = thread.clone();
        *hook.borrow_mut() = Some(Box::new(move || {
            let draining = document.clone();
            let completed = finished.clone();
            *thread.borrow_mut() = Some(std::thread::spawn(move || {
                let result = draining.shutdown();
                completed.store(true, Ordering::SeqCst);
                send.send(result).unwrap();
            }));
            let deadline = Instant::now() + TIMEOUT;
            while document.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert_eq!(document.lifecycle(), DocumentLifecycle::Draining);
            assert!(!finished.load(Ordering::SeqCst));
            assert!(!recovery.exists());
        }));
    });
    window.invoke_apply();
    assert!(window.get_message().is_empty());
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.borrow_mut().take().unwrap().join().unwrap();
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert!(lock(&shared).context_draft().is_none());
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert!(recovered.contains("Accepted before shutdown"));
    assert!(recovered.contains("Final recovery must include this admitted note."));
    let before = lock(&shared).context_state();
    window.invoke_apply();
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(lock(&shared).context_state(), before);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
}

#[test]
fn vanished_context_window_abandons_before_admission_while_document_stays_live() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("No assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = ContextWindow::new().unwrap();
    bind_with_access(&window, &access);
    window.invoke_begin("map".into());
    let before = lock(&shared).context_state();
    window.show().unwrap();
    window.hide().unwrap();
    let weak = window.as_weak();
    drop(window);
    assert!(weak.upgrade().is_none());
    assert!(
        with_session(&weak, &access.downgrade(), |_, _| panic!(
            "vanished child accessed Session"
        ))
        .is_none()
    );
    assert_eq!(lock(&shared).context_state(), before);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
}
