use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use glam::DVec3;
use map_core::{ObjectId, surface};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    directory: tempfile::TempDir,
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    controller: Controller,
}

fn fixture() -> Fixture {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        panic!()
    };
    let face = surface::FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session
        .select_faces(&[face], crate::session::SelectMode::Replace)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    access
        .with_session(|session| controller.initialize(session, "test/checker"))
        .unwrap();
    Fixture {
        directory,
        document,
        access,
        shared,
        controller,
    }
}

fn form(controller: &Controller) -> (Value, Vec<u8>) {
    let window = &controller.window;
    let target = controller.target.borrow();
    let target = target.as_ref().unwrap();
    (
        serde_json::json!({
            "fields": fields(window), "selection": window.get_selection_label().to_string(),
            "details": window.get_details().to_string(), "loaded": window.get_loaded(),
            "ready": window.get_ready(), "stale": window.get_stale(),
            "width": window.get_texture_width(), "height": window.get_texture_height(),
            "valid": window.get_valid_region(), "u": window.get_region_u(), "v": window.get_region_v(),
            "region_width": window.get_region_width(), "region_height": window.get_region_height(),
            "valid_usable": window.get_valid_usable_region(), "has_padding": window.get_has_padding(),
            "usable_u": window.get_usable_u(), "usable_v": window.get_usable_v(),
            "usable_width": window.get_usable_width(), "usable_height": window.get_usable_height(),
            "usable_details": window.get_usable_details().to_string(),
            "atlas": target.atlas, "token": target.token, "reviewed": target.reviewed,
        }),
        window.get_preview().to_rgba8().unwrap().as_bytes().to_vec(),
    )
}

fn rejected(window: &AtlasWindow, error: &str) {
    for action in [
        AtlasWindow::invoke_edited,
        AtlasWindow::invoke_inspect_material,
        AtlasWindow::invoke_preview_fit,
        AtlasWindow::invoke_apply_fit,
    ] {
        action(window);
        assert_eq!(window.get_message(), error);
    }
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
fn managed_atlas_retains_report_review_and_peer_during_draining_finished_and_owner_release() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let peer = fixture();
    let original = lock(&target.shared).document().vmf().clone();
    target.controller.window.set_region_pixels_wide("32".into());
    target.controller.window.set_region_padding("2".into());
    target.controller.window.set_fit_mode(1);
    target.controller.window.invoke_edited();
    target.controller.window.invoke_preview_fit();
    assert!(target.controller.window.get_ready());
    let draft = lock(&target.shared).render_document().vmf().clone();
    target.controller.window.invoke_apply_fit();
    assert_eq!(lock(&target.shared).document().vmf(), &draft);
    lock(&target.shared).undo(Actor::User).unwrap();
    target
        .access
        .with_session(|session| target.controller.initialize(session, "test/checker"))
        .unwrap();
    target.controller.window.set_region_padding("2".into());
    target.controller.window.set_fit_mode(1);
    target.controller.window.invoke_edited();
    target.controller.window.invoke_preview_fit();
    assert!(target.controller.window.get_ready());
    assert!(target.controller.window.get_has_padding());
    assert_eq!(
        lock(&target.shared).atlas_fit_state()["preview"]["mode"],
        "together"
    );
    let expected = form(&target.controller);
    let preview = lock(&target.shared).face_preview_token();
    let render = lock(&target.shared).render_document().vmf().clone();
    let history = lock(&target.shared).history_snapshot();
    let camera = lock(&target.shared).camera();
    let peer_form = form(&peer.controller);
    let peer_source = lock(&peer.shared).document().vmf().clone();
    let peer_history = lock(&peer.shared).history_snapshot();
    std::fs::write(
        target.directory.path().join("materials/test/checker.vmt"),
        "Patch { include materials/test/missing.vmt }",
    )
    .unwrap();
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
    rejected(&target.controller.window, "the editor is closing");
    assert_eq!(form(&target.controller), expected);
    assert_eq!(lock(&target.shared).face_preview_token(), preview);
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
    rejected(&target.controller.window, "the document is closed");
    assert_eq!(form(&target.controller), expected);
    assert_eq!(lock(&target.shared).face_preview_token(), preview);
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    assert_eq!(lock(&target.shared).render_document().vmf(), &render);
    assert_eq!(lock(&target.shared).history_snapshot(), history);
    assert_eq!(lock(&target.shared).camera(), camera);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(form(&peer.controller), peer_form);
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer.shared).history_snapshot(), peer_history);
    peer.controller.window.invoke_preview_fit();
    assert!(peer.controller.window.get_ready());
    let peer_draft = lock(&peer.shared).render_document().vmf().clone();
    peer.controller.window.invoke_apply_fit();
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_draft);
    assert!(!lock(&peer.shared).has_face_preview());
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let weak = Arc::downgrade(&target.shared);
    let Fixture {
        directory: _directory,
        document,
        access,
        shared,
        controller,
    } = target;
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    rejected(&controller.window, "the document is closed");
}

#[test]
fn managed_atlas_foreign_preview_and_failed_shutdown_preserve_exact_ownership() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    target.controller.window.invoke_preview_fit();
    assert!(target.controller.window.get_ready());
    let owned = target.controller.target.borrow().as_ref().unwrap().token;
    {
        let mut session = lock(&target.shared);
        let faces = session.selected_faces();
        session
            .edit_faces(
                Actor::Agent,
                &faces,
                surface::SurfaceEdit {
                    shift_u: Some(19.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
    }
    let foreign = lock(&target.shared).face_preview_token();
    assert_ne!(owned, foreign);
    target.controller.window.invoke_apply_fit();
    target.controller.window.invoke_cancel();
    assert_eq!(lock(&target.shared).face_preview_token(), foreign);
    let expected = form(&target.controller);
    let recovery = target.directory.path().join("recovery");
    if recovery.exists() {
        std::fs::rename(&recovery, target.directory.path().join("parked-recovery")).unwrap();
    }
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = target.document.shutdown().unwrap_err();
    rejected(
        &target.controller.window,
        &format!("document shutdown failed: {error}"),
    );
    assert_eq!(form(&target.controller), expected);
    assert_eq!(lock(&target.shared).face_preview_token(), foreign);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(target.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}
