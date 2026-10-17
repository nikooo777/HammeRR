use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use map_core::surface::{FaceId, SurfaceEdit};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    directory: tempfile::TempDir,
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    window: SurfaceWindow,
    target: Rc<RefCell<SurfaceTarget>>,
    owner: Rc<()>,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let ObjectId::Solid(solid) = session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap()
    else {
        panic!()
    };
    let face = FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let target = Rc::new(RefCell::new((
        vec![face],
        session.document_generation(),
        session.document().revision(),
    )));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = SurfaceWindow::new().unwrap();
    window.set_direct_vectors(true);
    load_surface_vectors(&window, lock(&shared).document(), &[face]);
    window.set_shift_u("13.125".into());
    let owner = Rc::new(());
    let weak_owner = Rc::downgrade(&owner);
    let chosen = target.clone();
    bind_surface_edits_with_access(
        &window,
        &access,
        move || weak_owner.upgrade().is_some(),
        move || Some(chosen.borrow().clone()),
    );
    Fixture {
        directory,
        document,
        access,
        shared,
        window,
        target,
        owner,
    }
}

fn form(window: &SurfaceWindow) -> serde_json::Value {
    serde_json::json!({
        "fields": surface_fields(window), "owned": window.get_owned_token().to_string(),
        "reviewed": window.get_reviewed_token().to_string(),
        "reviewed_fields": window.get_reviewed_fields().to_string(),
        "ready": window.get_ready(), "stale": window.get_stale(),
        "selection": window.get_selection_label().to_string(),
    })
}

fn rejected(window: &SurfaceWindow, error: &str) {
    window.invoke_preview_edit();
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
fn managed_surface_retains_exact_vector_review_and_peer_through_real_drain() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let peer = fixture();
    let original = lock(&target.shared).document().vmf().clone();
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    target.window.invoke_cancel();
    assert!(!lock(&target.shared).has_face_preview());
    target.window.invoke_preview_edit();
    let draft = lock(&target.shared).render_document().vmf().clone();
    target.window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &draft);
    lock(&target.shared).undo(Actor::User).unwrap();
    target.target.borrow_mut().2 = lock(&target.shared).document().revision();
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    let expected = form(&target.window);
    let owned = lock(&target.shared).face_preview_token();
    let render = lock(&target.shared).render_document().vmf().clone();
    let history = lock(&target.shared).history_snapshot();
    let camera = lock(&target.shared).camera();
    let peer_source = lock(&peer.shared).document().vmf().clone();
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
    rejected(&target.window, "the editor is closing");
    assert_eq!(form(&target.window), expected);
    assert_eq!(lock(&target.shared).face_preview_token(), owned);
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
    rejected(&target.window, "the document is closed");
    assert_eq!(form(&target.window), expected);
    assert_eq!(lock(&target.shared).face_preview_token(), owned);
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    assert_eq!(lock(&target.shared).render_document().vmf(), &render);
    assert_eq!(lock(&target.shared).history_snapshot(), history);
    assert_eq!(lock(&target.shared).camera(), camera);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer.shared).history_snapshot(), peer_history);
    assert_eq!(form(&peer.window), peer_form);
    assert!(!lock(&peer.shared).has_face_preview());
    peer.window.invoke_preview_edit();
    assert!(peer.window.get_ready());
    peer.window.invoke_apply();
    assert_ne!(lock(&peer.shared).document().vmf(), &peer_source);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let weak = Arc::downgrade(&target.shared);
    let Fixture {
        directory: _directory,
        document,
        access,
        shared,
        window,
        target: _target,
        owner: _owner,
    } = target;
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    rejected(&window, "the document is closed");
}

#[test]
fn managed_surface_foreign_preview_failed_close_and_vanished_owner_keep_cleanup_local() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready());
    let owned = lock(&target.shared).face_preview_token();
    {
        let mut session = lock(&target.shared);
        let faces = session.selected_faces();
        session
            .edit_faces(
                Actor::Agent,
                &faces,
                SurfaceEdit {
                    shift_v: Some(29.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
    }
    let foreign = lock(&target.shared).face_preview_token();
    assert_ne!(foreign, owned);
    target.window.invoke_apply();
    target.window.invoke_cancel();
    assert_eq!(lock(&target.shared).face_preview_token(), foreign);
    let expected = form(&target.window);
    let recovery = target.directory.path().join("recovery");
    if recovery.exists() {
        std::fs::rename(&recovery, target.directory.path().join("parked-recovery")).unwrap();
    }
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = target.document.shutdown().unwrap_err();
    rejected(
        &target.window,
        &format!("document shutdown failed: {error}"),
    );
    assert_eq!(form(&target.window), expected);
    assert_eq!(lock(&target.shared).face_preview_token(), foreign);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(target.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
    let live = fixture();
    let expected = form(&live.window);
    drop(live.owner);
    live.window.invoke_preview_edit();
    live.window.invoke_apply();
    live.window.show().unwrap();
    live.window.invoke_cancel();
    assert!(!live.window.window().is_visible());
    assert_eq!(form(&live.window), expected);
    assert!(!lock(&live.shared).has_face_preview());
    let weak_window = live.window.as_weak();
    drop(live.window);
    assert!(
        with_surface_window(&weak_window, &live.access.downgrade(), |_, _| panic!(
            "vanished surface performed work"
        ))
        .is_none()
    );
}

#[test]
fn actual_surface_preview_holds_admission_through_target_observation_and_review() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = fixture();
    let draining = fixture.document.clone();
    let observed = fixture.document.clone();
    let shared = fixture.shared.clone();
    let chosen = fixture.target.borrow().clone();
    let (done, completed) = mpsc::sync_channel(1);
    let completed = Rc::new(RefCell::new(completed));
    let pending = completed.clone();
    let joining = Rc::new(RefCell::new(None));
    let joined = joining.clone();
    let begin = RefCell::new(Some(move || {
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
    bind_surface_edits_with_access(
        &fixture.window,
        &fixture.access,
        || true,
        move || {
            let begin = begin.borrow_mut().take();
            if let Some(begin) = begin {
                begin();
            }
            Some(chosen.clone())
        },
    );
    fixture.window.invoke_preview_edit();
    assert!(
        fixture.window.get_ready(),
        "{}",
        fixture.window.get_message()
    );
    assert!(!fixture.window.get_reviewed_token().is_empty());
    completed.borrow().recv_timeout(TIMEOUT).unwrap().unwrap();
    joining.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
}
