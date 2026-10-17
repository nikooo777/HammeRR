use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use slint::Model;
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
    recovery: PathBuf,
    library: PathBuf,
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

fn create_fixture(directory: &Path, authority: &editor_core::prefabs::Authority) -> Fixture {
    let mut session =
        Session::with_prefabs(Err("No model assets required".into()), authority.clone());
    let recovery = directory.join("recovery");
    session.configure_recovery(Ok(recovery.clone()));
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    let library = session.prefab_report("", 0, PAGE, None)["directory"]
        .as_str()
        .unwrap()
        .into();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&shared, &access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    Fixture {
        document,
        shared,
        access,
        controller,
        recovery,
        library,
    }
}

fn seed_review(fixture: &Fixture) {
    let controller = &fixture.controller;
    controller.window.invoke_choose("Assembly".into());
    controller.window.invoke_preview();
    assert_eq!(
        lock(&fixture.shared).clipboard_report()["preview"]["valid"],
        true
    );
    controller.window.set_query("Broken".into());
    controller.window.invoke_search();
    controller.window.invoke_render_gallery();
    assert!(controller.form.borrow().gallery.is_some());
    assert_eq!(controller.window.get_gallery_status(), "0 of 1 images");
    assert_eq!(
        lock(&fixture.shared).prefab_thumbnail_report("Broken")["graphics"]["initialized"],
        false
    );
    controller.window.invoke_begin_rename();
    controller.window.set_prefab_name("Retained rename".into());
    controller.window.set_center_x("123".into());
    controller.window.set_rotation_z("45".into());
}

fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return result,
        Err(error) => panic!("Cannot inspect {}: {error}", directory.display()),
    };
    for entry in entries {
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
    render: source_formats::vmf::Vmf,
    history: String,
    selected: Vec<map_core::ObjectId>,
    camera: render::Camera,
    serials: [u64; 6],
    clipboard: serde_json::Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn snapshot(fixture: &Fixture) -> Snapshot {
    let session = lock(&fixture.shared);
    let mut stored = files(&fixture.library);
    stored.extend(files(&fixture.recovery));
    Snapshot {
        map: session.document().vmf().clone(),
        render: session.render_document().vmf().clone(),
        history: session.history_snapshot(),
        selected: session.selected_ids(),
        camera: session.camera(),
        serials: [
            session.scene_serial(),
            session.view_serial(),
            session.log_serial(),
            session.camera_serial(),
            session.prefab_registry_serial(),
            session.prefab_thumbnail_serial(),
        ],
        clipboard: session.clipboard_report(),
        files: stored,
    }
}

fn form_snapshot(controller: &Controller) -> serde_json::Value {
    let form = controller.form.borrow();
    let window = &controller.window;
    serde_json::json!({
        "selected": form.selected,
        "offset": form.offset,
        "stamp": form.stamp,
        "action": form.action.as_ref().map(|draft| (match draft.action {Action::Save => "save", Action::Delete => "delete", Action::Rename => "rename"}, &draft.stamp, &draft.source)),
        "seen": form.seen,
        "gallery": form.gallery.is_some(),
        "library_message": form.library_message,
        "fields": [window.get_selected().to_string(), window.get_query().to_string(), window.get_prefab_name().to_string(), window.get_center_x().to_string(), window.get_rotation_z().to_string(), window.get_detail().to_string(), window.get_thumbnail_status().to_string(), window.get_gallery_status().to_string()],
        "flags": [window.get_editing(), window.get_saving(), window.get_deleting(), window.get_renaming(), window.get_gallery_rendered(), window.get_has_thumbnail(), window.get_can_render_thumbnail(), window.get_can_render_gallery()],
        "thumbnail": window.get_thumbnail().to_rgba8().map(|image| image.as_bytes().to_vec()),
        "entries": window.get_entries().iter().map(|row| (row.name.to_string(), row.detail.to_string(), row.status.to_string(), row.has_image, row.thumbnail.to_rgba8().map(|image|image.as_bytes().to_vec()))).collect::<Vec<_>>(),
    })
}

fn retained_actions(fixture: &Fixture, message: &str) {
    let controller = &fixture.controller;
    controller.form.borrow_mut().offset = PAGE;
    controller
        .window
        .set_thumbnail(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
            slint::Rgba8Pixel,
        >::clone_from_slice(
            &[11, 22, 33, 255], 1, 1
        )));
    controller.window.set_has_thumbnail(true);
    controller.window.show().unwrap();
    let before = form_snapshot(controller);
    controller.window.invoke_choose("Broken".into());
    controller.window.invoke_refresh();
    controller.window.invoke_search();
    controller.window.invoke_page(1);
    controller.window.invoke_begin_save();
    controller.window.invoke_begin_delete();
    controller.window.invoke_begin_rename();
    controller.window.invoke_preview();
    controller.window.invoke_render_gallery();
    controller.window.invoke_render_thumbnail();
    controller.window.invoke_thumbnail_style_changed();
    assert_eq!(controller.window.get_message(), message);
    assert_eq!(form_snapshot(controller), before);
    for action in [Action::Save, Action::Delete, Action::Rename] {
        let stamp = controller.form.borrow().stamp.clone();
        controller.form.borrow_mut().action = Some(Draft {
            action,
            stamp,
            source: Some("Assembly".into()),
        });
        controller.window.set_prefab_name("Denied write".into());
        let before = form_snapshot(controller);
        match action {
            Action::Save => controller.window.invoke_save(),
            Action::Delete => controller.window.invoke_delete(),
            Action::Rename => controller.window.invoke_rename(),
        }
        assert_eq!(controller.window.get_message(), message);
        assert_eq!(form_snapshot(controller), before);
    }
    controller.window.invoke_cancel();
    assert!(controller.form.borrow().action.is_none());
    assert!(!controller.window.get_editing());
    assert!(controller.window.window().is_visible());
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    assert_eq!(controller.window.get_message(), message);
    assert!(controller.form.borrow().gallery.is_some());
    assert_eq!(controller.form.borrow().offset, PAGE);
    assert_eq!(
        lock(&fixture.shared).prefab_thumbnail_report("Broken")["graphics"]["initialized"],
        false
    );
}

#[test]
fn prefab_callbacks_reject_real_drain_preserving_review_storage_and_owned_paste_with_live_peer() {
    software();
    let directory = tempfile::tempdir().unwrap();
    let library = directory.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let authority = editor_core::prefabs::Authority::from_directory(Ok(library.clone()));
    let fixture = create_fixture(&directory.path().join("target"), &authority);
    fixture.controller.window.invoke_begin_save();
    fixture.controller.window.set_prefab_name("Assembly".into());
    fixture.controller.window.invoke_save();
    assert!(library.join("Assembly.vmf").exists());
    std::fs::write(library.join("Broken.vmf"), "invalid source").unwrap();
    let peer = create_fixture(&directory.path().join("peer"), &authority);
    seed_review(&fixture);
    seed_review(&peer);
    let gate = fixture.document.admit().unwrap();
    let draining = fixture.document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while fixture.document.lifecycle() == DocumentLifecycle::Live
        || lock(&fixture.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let before = snapshot(&fixture);
    let peer_before = snapshot(&peer);
    retained_actions(&fixture, "the editor is closing");
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(snapshot(&peer), peer_before);
    peer.controller.window.invoke_cancel();
    peer.controller.window.invoke_begin_save();
    peer.controller.window.set_prefab_name("Peer saved".into());
    peer.controller.window.invoke_save();
    assert!(library.join("Peer saved.vmf").exists());
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let peer_after = snapshot(&peer);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
    let before = snapshot(&fixture);
    retained_actions(&fixture, "the document is closed");
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(snapshot(&peer), peer_after);
    assert_eq!(
        lock(&fixture.shared)
            .recovery_entries()
            .unwrap()
            .entries
            .len(),
        1
    );
    peer.document.shutdown().unwrap();
}

#[test]
fn retained_prefab_and_nested_paste_windows_keep_neither_session_nor_runtime_alive() {
    software();
    let directory = tempfile::tempdir().unwrap();
    let authority =
        editor_core::prefabs::Authority::from_directory(Ok(directory.path().join("library")));
    let fixture = create_fixture(directory.path(), &authority);
    let weak_session = Arc::downgrade(&fixture.shared);
    let continuation = fixture.access.downgrade();
    let window = fixture.controller.window.clone_strong();
    let paste = fixture.controller.paste.window.clone_strong();
    drop(fixture);
    assert!(weak_session.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    window.invoke_choose("discarded".into());
    window.invoke_render_gallery();
    window.invoke_save();
    paste.invoke_preview();
    paste.invoke_apply();
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(paste.get_message(), "the document is closed");
    window.invoke_close_dialog();
    paste.invoke_cancel();
    assert!(!window.window().is_visible());
    assert!(!paste.window().is_visible());
}

#[test]
fn vanished_prefab_window_abandons_admission_and_releases_nested_paste() {
    software();
    let directory = tempfile::tempdir().unwrap();
    let authority =
        editor_core::prefabs::Authority::from_directory(Ok(directory.path().join("library")));
    let fixture = create_fixture(directory.path(), &authority);
    let window = fixture.controller.window.as_weak();
    let paste = fixture.controller.paste.window.as_weak();
    let continuation = fixture.access.downgrade();
    fixture.controller.window.hide().unwrap();
    drop(fixture.controller);
    assert!(window.upgrade().is_none());
    assert!(paste.upgrade().is_none());
    callback(&window, &continuation, |_, _| {
        panic!("a vanished prefab window accessed Session")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    fixture.document.shutdown().unwrap();
}
