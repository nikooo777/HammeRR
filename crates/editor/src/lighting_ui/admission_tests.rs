use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use crate::session::SelectMode;
use map_core::ObjectId;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    shared: SharedSession,
    access: DocumentAccess,
    controller: Controller,
    faces: Vec<FaceId>,
    directory: tempfile::TempDir,
}

fn software() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("No assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let ObjectId::Solid(id) = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let faces = session.document().solid(id).unwrap().sides[..2]
        .iter()
        .map(|side| FaceId {
            solid: id,
            side: side.id,
        })
        .collect::<Vec<_>>();
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    controller.window.invoke_toggle_group(2, true);
    controller.window.set_lightmap_scale("4".into());
    controller.window.invoke_edited();
    controller.window.invoke_preview_edit();
    assert!(lock(&shared).face_preview_token().is_some());
    assert_eq!(
        controller.target.borrow().as_ref().unwrap().preview,
        lock(&shared).face_preview_token()
    );
    Fixture {
        document,
        shared,
        access,
        controller,
        faces,
        directory,
    }
}

fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    result
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    map: source_formats::vmf::Vmf,
    preview: Option<source_formats::vmf::Vmf>,
    preview_token: Option<u64>,
    history: String,
    faces: Vec<FaceId>,
    camera: render::Camera,
    serials: [u64; 4],
    view: (render::SurfaceView, u32),
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn snapshot(fixture: &Fixture) -> Snapshot {
    let session = lock(&fixture.shared);
    Snapshot {
        map: session.document().vmf().clone(),
        preview: session
            .face_preview_document()
            .map(|document| document.vmf().clone()),
        preview_token: session.face_preview_token(),
        history: session.history_snapshot(),
        faces: session.selected_faces(),
        camera: session.camera(),
        serials: [
            session.scene_serial(),
            session.view_serial(),
            session.log_serial(),
            session.camera_serial(),
        ],
        view: (session.view().surface, session.view().smoothing_group),
        files: files(fixture.directory.path()),
    }
}

fn form_snapshot(controller: &Controller) -> serde_json::Value {
    let target = controller.target.borrow();
    let window = &controller.window;
    serde_json::json!({
        "target": target.as_ref().map(|target| (target.preview, &target.initial_scale, format!("{:?}", target.context.faces))),
        "groups": window.get_groups().iter().map(|group| (group.number,group.state,group.action)).collect::<Vec<_>>(),
        "scale": window.get_lightmap_scale().to_string(),
        "label": window.get_selection_label().to_string(),
        "stale": window.get_stale(),
        "can_apply": window.get_can_apply(),
        "view": [window.get_view_mode(), window.get_view_group()],
    })
}

fn retained_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    window.set_lightmap_scale("8".into());
    window.set_view_mode(render::SurfaceView::SmoothingGroups.index());
    window.set_view_group(7);
    window.show().unwrap();
    let form = form_snapshot(controller);
    window.invoke_edited();
    window.invoke_toggle_group(31, true);
    window.invoke_clear_groups();
    window.invoke_reset_groups();
    window.invoke_view_changed();
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(window.get_message(), error);
    assert_eq!(form_snapshot(controller), form);
    assert!(window.window().is_visible());
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(form_snapshot(controller), form);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(window.get_message(), error);
    assert_eq!(form_snapshot(controller), form);
}

#[test]
fn lighting_actions_act_live_then_preserve_owned_preview_and_forms_through_drain_with_live_peer() {
    software();
    let target = fixture();
    let original = lock(&target.shared).document().vmf().clone();
    target.controller.window.invoke_apply();
    assert_ne!(lock(&target.shared).document().vmf(), &original);
    assert!(!lock(&target.shared).has_face_preview());
    lock(&target.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    target
        .controller
        .initialize(&mut lock(&target.shared))
        .unwrap();
    target.controller.window.invoke_toggle_group(2, true);
    target.controller.window.set_lightmap_scale("4".into());
    target.controller.window.invoke_edited();
    target.controller.window.invoke_preview_edit();
    assert!(lock(&target.shared).has_face_preview());
    let peer = fixture();
    let peer_source = lock(&peer.shared).document().vmf().clone();
    let gate = target.document.admit().unwrap();
    let draining = target.document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while target.document.lifecycle() == DocumentLifecycle::Live
        || lock(&target.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let before = snapshot(&target);
    let peer_before = snapshot(&peer);
    retained_actions(&target.controller, "the editor is closing");
    assert_eq!(snapshot(&target), before);
    assert_eq!(snapshot(&peer), peer_before);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
    let before = snapshot(&target);
    retained_actions(&target.controller, "the document is closed");
    assert_eq!(snapshot(&target), before);
    assert_eq!(snapshot(&peer), peer_before);
    peer.controller.window.invoke_apply();
    assert_ne!(lock(&peer.shared).document().vmf(), &peer_source);
    assert!(!lock(&peer.shared).has_face_preview());
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(
        lock(&target.shared)
            .recovery_entries()
            .unwrap()
            .entries
            .len(),
        1
    );
    peer.document.shutdown().unwrap();
}

#[test]
fn managed_lighting_guards_foreign_preview_and_changed_face_context_without_cleanup_theft() {
    software();
    let fixture = fixture();
    let own = fixture.controller.target.borrow().as_ref().unwrap().preview;
    lock(&fixture.shared)
        .edit_faces(
            Actor::Agent,
            &fixture.faces,
            SurfaceEdit {
                lightmap_scale: Some(32),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let foreign = lock(&fixture.shared).face_preview_token();
    assert_ne!(own, foreign);
    let before = snapshot(&fixture);
    fixture.controller.window.invoke_apply();
    assert!(
        fixture
            .controller
            .window
            .get_message()
            .contains("Another face preview")
    );
    assert_eq!(snapshot(&fixture), before);
    fixture.controller.window.invoke_cancel();
    assert_eq!(
        fixture.controller.target.borrow().as_ref().unwrap().preview,
        None
    );
    assert_eq!(snapshot(&fixture), before);
    fixture.controller.window.show().unwrap();
    fixture
        .controller
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(snapshot(&fixture), before);
    fixture
        .controller
        .initialize(&mut lock(&fixture.shared))
        .unwrap();
    fixture.controller.window.set_lightmap_scale("8".into());
    fixture.controller.window.invoke_edited();
    lock(&fixture.shared)
        .select_faces(&fixture.faces[..1], SelectMode::Replace)
        .unwrap();
    let before = snapshot(&fixture);
    fixture.controller.window.invoke_preview_edit();
    fixture.controller.window.invoke_apply();
    assert!(
        fixture
            .controller
            .window
            .get_message()
            .contains("Map or face selection changed")
    );
    assert_eq!(snapshot(&fixture), before);
    fixture.document.shutdown().unwrap();
}

#[test]
fn retained_lighting_window_releases_runtime_and_session_and_vanished_window_abandons_work() {
    software();
    let fixture = fixture();
    let cold = Controller::new_with_access(&fixture.access).unwrap();
    let vanished = cold.window.as_weak();
    drop(cold);
    assert!(vanished.upgrade().is_none());
    callback(&vanished, &fixture.access.downgrade(), |_, _| {
        panic!("a vanished lighting window accessed Session")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    let weak_session = Arc::downgrade(&fixture.shared);
    let continuation = fixture.access.downgrade();
    let window = fixture.controller.window.clone_strong();
    drop(fixture);
    assert!(weak_session.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    window.invoke_edited();
    window.invoke_toggle_group(0, true);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}
