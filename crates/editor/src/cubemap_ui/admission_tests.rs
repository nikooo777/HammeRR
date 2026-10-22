use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use map_core::ObjectId;
use serde_json::json;
use slint::Model;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    controller: Controller,
    helper: u32,
    face: FaceId,
    directory: tempfile::TempDir,
    _assets: tempfile::TempDir,
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
    let (assets, mut session, _near, helper, face) =
        editor_core::session::cubemap_bindings::test_support::fixture();
    let directory = tempfile::tempdir().unwrap();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .select(&[ObjectId::Entity(helper)], SelectMode::Replace)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    lock(&shared)
        .select_faces(&[face], SelectMode::Replace)
        .unwrap();
    controller.refresh(&lock(&shared));
    controller.window.invoke_bind();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    Fixture {
        document,
        access,
        shared,
        controller,
        helper,
        face,
        directory,
        _assets: assets,
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

fn snapshot(target: &Fixture) -> (source_formats::vmf::Vmf, Value, BTreeMap<PathBuf, Vec<u8>>) {
    let session = lock(&target.shared);
    (
        session.document().vmf().clone(),
        json!({
            "bindings":session.cubemap_binding_state(),
            "report":session.cubemap_binding_report_stamp(),
            "history":session.history_snapshot(),
            "faces":session.selected_faces().into_iter().map(|face|(face.solid,face.side)).collect::<Vec<_>>(),
            "objects":session.selected_ids().into_iter().map(|id|id.to_string()).collect::<Vec<_>>(),
            "camera":format!("{:?}",session.camera()),
            "serials":[session.scene_serial(),session.view_serial(),session.log_serial(),session.camera_serial()],
        }),
        files(target.directory.path()),
    )
}

fn forms(controller: &Controller) -> Value {
    let form = controller.form.borrow();
    let window = &controller.window;
    json!({
        "form": {
            "helper":form.helper,"generation":form.generation,"owned":form.owned_token,"reviewed":form.reviewed_token,
            "snapshot":form.snapshot,"seen":form.seen,"report":form.report,"report_snapshot":form.report_snapshot,
            "report_query":form.report_query,"offset":form.offset,
        },
        "search":window.get_search().to_string(),"helper":window.get_helper_label().to_string(),
        "selection":window.get_selection_label().to_string(),"inspection":window.get_inspection_label().to_string(),
        "draft":window.get_draft_label().to_string(),"page":window.get_page_label().to_string(),
        "ready":window.get_ready(),"review":window.get_can_review(),
        "rows":window.get_assignments().iter().map(|row|(row.index,row.label.to_string(),row.choice.to_string(),row.material.to_string(),row.alias.to_string(),row.selected)).collect::<Vec<_>>(),
    })
}

fn retained_actions(target: &Fixture, expected: &str) {
    let window = &target.controller.window;
    window.set_search("pending query".into());
    let before = snapshot(target);
    let form = forms(&target.controller);
    for invoke in [
        CubemapFacesWindow::invoke_refresh,
        CubemapFacesWindow::invoke_select_bound,
        CubemapFacesWindow::invoke_bind,
        CubemapFacesWindow::invoke_remove,
        CubemapFacesWindow::invoke_clear_bindings,
        CubemapFacesWindow::invoke_review,
        CubemapFacesWindow::invoke_apply,
        CubemapFacesWindow::invoke_cancel,
    ] {
        window.show().unwrap();
        invoke(window);
        assert_eq!(window.get_message(), expected);
        assert_eq!(snapshot(target), before);
        assert_eq!(forms(&target.controller), form);
    }
    assert!(!window.window().is_visible());
    window.invoke_page_change(1);
    assert_eq!(window.get_message(), expected);
    window.invoke_select_face(0);
    assert_eq!(window.get_message(), expected);
    assert_eq!(snapshot(target), before);
    assert_eq!(forms(&target.controller), form);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(window.get_message(), expected);
    assert_eq!(snapshot(target), before);
    assert_eq!(forms(&target.controller), form);
}

#[test]
fn managed_cubemap_actions_apply_exact_live_reviews_and_keep_stale_inspection_and_foreign_drafts() {
    software();
    let target = fixture();
    let original = lock(&target.shared).document().vmf().clone();
    target.controller.window.invoke_apply();
    assert_ne!(lock(&target.shared).document().vmf(), &original);
    target.controller.window.invoke_refresh();
    target.controller.window.invoke_select_bound();
    assert_eq!(lock(&target.shared).selected_faces(), [target.face]);
    target.controller.window.invoke_select_face(0);
    assert!(lock(&target.shared).selected_faces().is_empty());
    target.controller.window.invoke_select_face(0);
    assert_eq!(lock(&target.shared).selected_faces(), [target.face]);
    target.controller.window.invoke_remove();
    assert!(target.controller.window.get_ready());
    target.controller.window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    target.controller.window.invoke_refresh();
    target.controller.window.invoke_bind();
    let foreign = {
        let mut session = lock(&target.shared);
        let stamp = session.cubemap_bindings_stamp();
        session
            .preview_cubemap_bindings(
                Actor::Agent,
                Request {
                    helper: target.helper,
                    faces: vec![target.face],
                    operation: Operation::Bind,
                },
                &stamp,
            )
            .unwrap()
    };
    target.controller.window.invoke_apply();
    target.controller.window.invoke_cancel();
    assert_eq!(
        lock(&target.shared).cubemap_binding_state()["draft"]["token"],
        foreign["draft"]["token"]
    );
    target.controller.window.invoke_review();
    assert!(target.controller.window.get_ready());
    target.controller.window.invoke_apply();
    target.controller.window.invoke_refresh();
    target.controller.window.invoke_clear_bindings();
    assert!(target.controller.window.get_ready());
    target.controller.window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    target.controller.window.invoke_refresh();
    lock(&target.shared).reload_material_assets(Actor::Agent);
    let before = snapshot(&target);
    let form = forms(&target.controller);
    target.controller.window.invoke_page_change(1);
    assert!(
        target
            .controller
            .window
            .get_message()
            .contains("Refresh assignments")
    );
    target.controller.window.invoke_select_face(0);
    assert!(
        target
            .controller
            .window
            .get_message()
            .contains("Refresh assignments")
    );
    assert_eq!(snapshot(&target), before);
    assert_eq!(forms(&target.controller), form);
}

#[test]
fn cubemap_callbacks_keep_forms_and_recovery_during_real_drain_finish_and_live_peer_activity() {
    software();
    let target = fixture();
    let peer = fixture();
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
    let peer_before = snapshot(&peer);
    retained_actions(&target, "the editor is closing");
    assert_eq!(snapshot(&peer), peer_before);
    peer.controller.window.invoke_apply();
    assert_ne!(snapshot(&peer).0, peer_before.0);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
    let entries = lock(&target.shared).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = crate::recovery::Store {
        directory: target.directory.path().join("recovery"),
    };
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert_eq!(
        &source_formats::vmf::Vmf::parse(&recovered).unwrap(),
        lock(&target.shared).document().vmf()
    );
    retained_actions(&target, "the document is closed");
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    peer.document.shutdown().unwrap();
}

#[test]
fn retained_cubemap_window_releases_session_runtime_and_vanished_origin_abandons_work() {
    software();
    let target = fixture();
    let cold = Controller::new_with_access(&target.access).unwrap();
    let vanished = cold.window.as_weak();
    drop(cold);
    assert!(vanished.upgrade().is_none());
    callback(&vanished, &target.access.downgrade(), |_, _| {
        panic!("vanished Cubemap window accessed Session")
    });
    let window = target.controller.window.clone_strong();
    window.hide().unwrap();
    let weak = Arc::downgrade(&target.shared);
    let continuation = target.access.downgrade();
    drop(target);
    assert!(weak.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    window.invoke_refresh();
    window.invoke_bind();
    window.invoke_review();
    window.invoke_apply();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}
