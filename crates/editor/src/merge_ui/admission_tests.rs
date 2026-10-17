use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use crate::session::SelectMode;
use glam::DVec3;
use std::cell::RefCell;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    directory: tempfile::TempDir,
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    window: MergeWindow,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let mut ids = Vec::new();
    for (mins, maxs) in [
        (DVec3::ZERO, DVec3::splat(64.0)),
        (DVec3::new(64.0, 0.0, 0.0), DVec3::new(128.0, 64.0, 64.0)),
    ] {
        ids.push(
            session
                .create_block(Actor::User, mins, maxs, "test", false)
                .unwrap(),
        );
    }
    session.select(&ids, SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = MergeWindow::new().unwrap();
    bind_with_access(&window, &access);
    refresh(&window, &lock(&shared));
    Fixture {
        directory,
        document,
        access,
        shared,
        window,
    }
}

fn form(window: &MergeWindow) -> serde_json::Value {
    let rows = window
        .get_sources()
        .iter()
        .map(|row| (row.solid.to_string(), row.label.to_string(), row.selected))
        .collect::<Vec<_>>();
    serde_json::json!({
        "rows": rows, "source": window.get_source().to_string(),
        "stamp": window.get_stamp().to_string(), "token": window.get_token().to_string(),
        "summary": window.get_summary().to_string(), "ready": window.get_ready(),
        "can_preview": window.get_can_preview(),
    })
}

fn rejected_actions(window: &MergeWindow, error: &str) {
    let id = window.get_sources().row_data(0).unwrap().solid;
    window.invoke_choose_source(id);
    assert_eq!(window.get_message(), error);
    window.invoke_preview_edit();
    assert_eq!(window.get_message(), error);
    window.invoke_discard();
    assert_eq!(window.get_message(), error);
    window.invoke_apply();
    assert_eq!(window.get_message(), error);
    window.show().unwrap();
    window.invoke_cancel();
    assert_eq!(window.get_message(), error);
    assert!(!window.window().is_visible());
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(window.get_message(), error);
    assert!(!window.window().is_visible());
}

#[test]
fn managed_merge_actions_preserve_retained_review_and_peer_through_real_drain() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let peer = fixture();
    let original = lock(&target.shared).document().vmf().clone();
    let selection = lock(&target.shared).selected_ids();
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    target.window.invoke_discard();
    assert!(target.window.get_token().is_empty());
    target.window.invoke_preview_edit();
    let draft = lock(&target.shared).render_document().vmf().clone();
    target.window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &draft);
    lock(&target.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    lock(&target.shared)
        .select(&selection, SelectMode::Replace)
        .unwrap();
    refresh(&target.window, &lock(&target.shared));
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    let expected_form = form(&target.window);
    let history = lock(&target.shared).history_snapshot();
    let camera = lock(&target.shared).camera();
    let state = lock(&target.shared).merge_report();
    let render = lock(&target.shared).render_document().vmf().clone();
    let peer_source = lock(&peer.shared).document().vmf().clone();
    let peer_state = lock(&peer.shared).merge_report();
    let peer_history = lock(&peer.shared).history_snapshot();
    let peer_form = form(&peer.window);
    let gate = target.document.admit().unwrap();
    let draining = target.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while target.document.lifecycle() == DocumentLifecycle::Live
        || lock(&target.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    rejected_actions(&target.window, "the editor is closing");
    assert_eq!(form(&target.window), expected_form);
    assert_eq!(lock(&target.shared).merge_report(), state);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: target.directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    rejected_actions(&target.window, "the document is closed");
    assert_eq!(form(&target.window), expected_form);
    assert_eq!(lock(&target.shared).merge_report(), state);
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    assert_eq!(lock(&target.shared).render_document().vmf(), &render);
    assert_eq!(lock(&target.shared).history_snapshot(), history);
    assert_eq!(lock(&target.shared).camera(), camera);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer.shared).merge_report(), peer_state);
    assert_eq!(lock(&peer.shared).history_snapshot(), peer_history);
    assert_eq!(form(&peer.window), peer_form);
    peer.window.invoke_preview_edit();
    assert!(peer.window.get_ready(), "{}", peer.window.get_message());
    let peer_draft = lock(&peer.shared).render_document().vmf().clone();
    peer.window.invoke_apply();
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_draft);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let weak = Arc::downgrade(&target.shared);
    let Fixture {
        directory: _directory,
        document,
        access,
        shared,
        window,
    } = target;
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    rejected_actions(&window, "the document is closed");
}

#[test]
fn stale_merge_token_cannot_commit_or_discard_foreign_preview_and_failed_close_is_memoized() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = fixture();
    fixture.window.invoke_preview_edit();
    assert!(fixture.window.get_ready());
    let owned = fixture.window.get_token();
    {
        let mut session = lock(&fixture.shared);
        let sources = parameters(&fixture.window).unwrap();
        let stamp = session.merge_stamp();
        session.preview_merge(&sources, &stamp).unwrap();
    }
    let foreign = lock(&fixture.shared).merge_report();
    assert_ne!(
        foreign["preview"]["token"].as_str().unwrap(),
        owned.as_str()
    );
    let source = lock(&fixture.shared).document().vmf().clone();
    for action in 0..4 {
        fixture.window.set_token(owned.clone());
        fixture.window.set_ready(true);
        fixture.window.show().unwrap();
        match action {
            0 => fixture.window.invoke_apply(),
            1 => fixture.window.invoke_discard(),
            2 => fixture.window.invoke_cancel(),
            _ => fixture
                .window
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested),
        }
        assert_eq!(lock(&fixture.shared).merge_report(), foreign);
        assert_eq!(lock(&fixture.shared).document().vmf(), &source);
    }
    refresh(&fixture.window, &lock(&fixture.shared));
    let expected_form = form(&fixture.window);
    let recovery = fixture.directory.path().join("recovery");
    if recovery.exists() {
        std::fs::remove_dir_all(&recovery).unwrap();
    }
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = fixture.document.shutdown().unwrap_err();
    rejected_actions(
        &fixture.window,
        &format!("document shutdown failed: {error}"),
    );
    assert_eq!(form(&fixture.window), expected_form);
    assert_eq!(lock(&fixture.shared).merge_report(), foreign);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(fixture.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn admitted_merge_choice_completes_while_shutdown_waits_for_its_actual_callback() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = fixture();
    let id = fixture.window.get_sources().row_data(0).unwrap().solid;
    assert!(fixture.window.get_sources().row_data(0).unwrap().selected);
    let draining = fixture.document.clone();
    let observed = fixture.document.clone();
    let shared = fixture.shared.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let completed = Rc::new(RefCell::new(completed));
    let pending = completed.clone();
    let joining = Rc::new(RefCell::new(None));
    let joined = joining.clone();
    BEFORE_ACTION.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            *joined.borrow_mut() = Some(std::thread::spawn(move || {
                done.send(draining.shutdown()).unwrap()
            }));
            let deadline = Instant::now() + TIMEOUT;
            while observed.lifecycle() == DocumentLifecycle::Live
                || lock(&shared).compile_settings()["editable"] != false
            {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(pending.borrow().try_recv().is_err());
        }));
    });
    fixture.window.invoke_choose_source(id);
    assert!(!fixture.window.get_sources().row_data(0).unwrap().selected);
    assert!(fixture.window.get_message().contains("Sources changed"));
    completed.borrow().recv_timeout(TIMEOUT).unwrap().unwrap();
    joining.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
}
