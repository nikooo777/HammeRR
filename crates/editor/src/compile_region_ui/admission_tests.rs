use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use crate::session::SelectMode;
use crate::{surface_group_ui, volume_ui};
use map_core::{ObjectId, surface::FaceId};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum Family {
    Surface,
    Volume,
    Region,
}

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    surface: surface_group_ui::Controller,
    volume: volume_ui::Controller,
    region: Controller,
    faces: Vec<FaceId>,
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

fn fixture(family: Family) -> Fixture {
    let (assets, game) = crate::test_assets::game();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let mut faces = Vec::new();
    for x in [0.0, 128.0] {
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::User,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 64.0, 64.0, 64.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .id;
        faces.push(FaceId { solid, side });
    }
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let surface = surface_group_ui::Controller::new_with_access(&access).unwrap();
    let volume = volume_ui::Controller::new_with_access(&access, || Some(16.0 / 9.0)).unwrap();
    let region = Controller::new_with_access(&access, || Some(16.0 / 9.0)).unwrap();
    surface.initialize(&mut lock(&shared)).unwrap();
    volume.initialize(&mut lock(&shared));
    region.show(&lock(&shared)).unwrap();
    surface.window.set_repeats_u("2".into());
    match family {
        Family::Surface => surface.window.invoke_preview_edit(),
        Family::Volume => {
            volume.window.set_target(0);
            volume.window.invoke_preview_edit();
        }
        Family::Region => {
            region.window.set_region_enabled(true);
            region.window.invoke_preview();
        }
    }
    Fixture {
        document,
        access,
        shared,
        surface,
        volume,
        region,
        faces,
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

fn snapshot(fixture: &Fixture) -> (source_formats::vmf::Vmf, Value, BTreeMap<PathBuf, Vec<u8>>) {
    let session = lock(&fixture.shared);
    (
        session.document().vmf().clone(),
        json!({
            "preview": session.face_preview_document().map(|document| format!("{:?}", document.vmf())),
            "face_token": session.face_preview_token(),
            "volume": session.volume_selection_json(0, 500),
            "region": session.compile_region_state(),
            "history": session.history_snapshot(),
            "objects": session.selected_ids().into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "faces": session.selected_faces().into_iter().map(|face| (face.solid, face.side)).collect::<Vec<_>>(),
            "camera": format!("{:?}", session.camera()),
            "serials": [session.scene_serial(),session.view_serial(),session.log_serial(),session.camera_serial()],
        }),
        files(fixture.directory.path()),
    )
}

fn forms(fixture: &Fixture) -> Value {
    let surface = &fixture.surface.window;
    let volume = &fixture.volume.window;
    let region = &fixture.region.window;
    let form = fixture.region.form.borrow();
    json!({
        "surface": {
            "fields": [surface.get_material().to_string(),surface.get_repeats_u().to_string(),surface.get_repeats_v().to_string()],
            "projection":surface.get_projection(),"reference":surface.get_reference_index(),
            "ready":surface.get_can_apply(),"stale":surface.get_stale(),
            "label":surface.get_selection_label().to_string(),"preview":surface.get_preview_material().to_string(),
        },
        "volume": {
            "fields":[volume.get_min_x().to_string(),volume.get_min_y().to_string(),volume.get_min_z().to_string(),volume.get_max_x().to_string(),volume.get_max_y().to_string(),volume.get_max_z().to_string()],
            "target":volume.get_target(),"mode":volume.get_mode(),"rule":volume.get_rule(),
            "ready":volume.get_ready(),"summary":volume.get_summary().to_string(),
        },
        "region": {
            "fields":[region.get_min_x().to_string(),region.get_min_y().to_string(),region.get_min_z().to_string(),region.get_max_x().to_string(),region.get_max_y().to_string(),region.get_max_z().to_string()],
            "enabled":region.get_region_enabled(),"ready":region.get_ready(),"review":region.get_can_review(),
            "token":form.token,"seen":form.seen,"reviewed":format!("{:?}",form.reviewed),
            "committed":region.get_committed().to_string(),
        }
    })
}

fn retained_actions(fixture: &Fixture, expected: &str) {
    let before = snapshot(fixture);
    let form = forms(fixture);
    for invoke in [
        crate::SurfaceGroupWindow::invoke_edited,
        crate::SurfaceGroupWindow::invoke_load_material,
        crate::SurfaceGroupWindow::invoke_preview_edit,
        crate::SurfaceGroupWindow::invoke_apply,
        crate::SurfaceGroupWindow::invoke_cancel,
    ] {
        fixture.surface.window.show().unwrap();
        invoke(&fixture.surface.window);
        assert_eq!(fixture.surface.window.get_message(), expected);
        assert_eq!(snapshot(fixture), before);
        assert_eq!(forms(fixture), form);
    }
    assert!(!fixture.surface.window.window().is_visible());
    for invoke in [
        crate::VolumeWindow::invoke_edited,
        crate::VolumeWindow::invoke_use_bounds,
        crate::VolumeWindow::invoke_preview_edit,
        crate::VolumeWindow::invoke_frame,
        crate::VolumeWindow::invoke_apply,
        crate::VolumeWindow::invoke_cancel,
    ] {
        fixture.volume.window.show().unwrap();
        invoke(&fixture.volume.window);
        assert_eq!(fixture.volume.window.get_message(), expected);
        assert_eq!(snapshot(fixture), before);
        assert_eq!(forms(fixture), form);
    }
    assert!(!fixture.volume.window.window().is_visible());
    for invoke in [
        crate::CompileRegionWindow::invoke_edited,
        crate::CompileRegionWindow::invoke_use_bounds,
        crate::CompileRegionWindow::invoke_preview,
        crate::CompileRegionWindow::invoke_preview_clear,
        crate::CompileRegionWindow::invoke_review,
        crate::CompileRegionWindow::invoke_frame,
        crate::CompileRegionWindow::invoke_apply,
        crate::CompileRegionWindow::invoke_cancel,
    ] {
        fixture.region.window.show().unwrap();
        invoke(&fixture.region.window);
        assert_eq!(fixture.region.window.get_message(), expected);
        assert_eq!(snapshot(fixture), before);
        assert_eq!(forms(fixture), form);
    }
    assert!(!fixture.region.window.window().is_visible());
    for window in [
        fixture.surface.window.window(),
        fixture.volume.window.window(),
        fixture.region.window.window(),
    ] {
        window.show().unwrap();
        window.dispatch_event(slint::platform::WindowEvent::CloseRequested);
        assert!(!window.is_visible());
        assert_eq!(snapshot(fixture), before);
        assert_eq!(forms(fixture), form);
    }
}

#[test]
fn surface_volume_region_actions_work_live_and_keep_exact_foreign_and_stale_targets() {
    software();
    let surface = fixture(Family::Surface);
    let original = lock(&surface.shared).document().vmf().clone();
    assert!(lock(&surface.shared).face_preview_token().is_some());
    surface.surface.window.invoke_apply();
    assert_ne!(lock(&surface.shared).document().vmf(), &original);
    lock(&surface.shared).undo(Actor::User).unwrap();
    surface
        .surface
        .initialize(&mut lock(&surface.shared))
        .unwrap();
    surface.surface.window.set_repeats_u("2".into());
    surface.surface.window.invoke_preview_edit();
    lock(&surface.shared)
        .edit_faces(
            Actor::Agent,
            &surface.faces,
            map_core::surface::SurfaceEdit {
                shift_u: Some(12.0),
                ..Default::default()
            },
            true,
        )
        .unwrap();
    let before = snapshot(&surface);
    surface.surface.window.invoke_apply();
    assert!(
        surface
            .surface
            .window
            .get_message()
            .contains("Another face preview")
    );
    surface.surface.window.invoke_cancel();
    assert_eq!(snapshot(&surface), before);
    surface
        .surface
        .initialize(&mut lock(&surface.shared))
        .unwrap();
    lock(&surface.shared)
        .select_faces(&surface.faces[..1], SelectMode::Replace)
        .unwrap();
    let before = snapshot(&surface);
    surface.surface.window.invoke_preview_edit();
    assert!(
        surface
            .surface
            .window
            .get_message()
            .contains("Map or face selection changed")
    );
    assert_eq!(snapshot(&surface), before);
    let volume = fixture(Family::Volume);
    let camera = lock(&volume.shared).camera();
    volume.volume.window.invoke_frame();
    assert_ne!(lock(&volume.shared).camera(), camera);
    volume.volume.window.invoke_apply();
    assert!(lock(&volume.shared).volume_preview().is_none());
    assert!(!lock(&volume.shared).selected_ids().is_empty());
    volume.volume.initialize(&mut lock(&volume.shared));
    volume.volume.window.invoke_preview_edit();
    let spec = lock(&volume.shared).volume_preview().unwrap().spec;
    lock(&volume.shared).preview_volume_selection(spec).unwrap();
    let before = snapshot(&volume);
    volume.volume.window.invoke_apply();
    volume.volume.window.invoke_cancel();
    assert_eq!(snapshot(&volume), before);
    let region = fixture(Family::Region);
    assert!(region.region.window.get_ready());
    region.region.window.invoke_frame();
    region.region.window.invoke_apply();
    assert_eq!(
        lock(&region.shared).compile_region_state()["committed"]["enabled"],
        true
    );
    region.region.window.invoke_preview_clear();
    let foreign = {
        let mut session = lock(&region.shared);
        let stamp = session.compile_region_stamp();
        session
            .preview_compile_region(Actor::Agent, None, &stamp)
            .unwrap()
    };
    region.region.window.invoke_apply();
    region.region.window.invoke_cancel();
    assert_eq!(
        lock(&region.shared).compile_region_state()["draft"]["token"],
        foreign["draft"]["token"]
    );
    region.region.window.invoke_review();
    assert!(region.region.window.get_ready());
    region.region.window.invoke_apply();
    assert!(lock(&region.shared).compile_region_state()["committed"].is_null());
}

#[test]
fn all_region_family_actions_preserve_forms_and_final_recovery_through_real_drain_and_finish() {
    software();
    for family in [Family::Surface, Family::Volume, Family::Region] {
        let target = fixture(family);
        let peer = fixture(Family::Region);
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
        peer.region.window.invoke_apply();
        assert_eq!(
            lock(&peer.shared).compile_region_state()["committed"]["enabled"],
            true
        );
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
}

#[test]
fn failed_region_family_admission_remains_terminal_after_recovery_storage_repair() {
    software();
    for family in [Family::Surface, Family::Volume, Family::Region] {
        let target = fixture(family);
        let recovery = target.directory.path().join("recovery");
        std::fs::write(&recovery, b"not a directory").unwrap();
        let error = target.document.shutdown().unwrap_err();
        assert!(matches!(
            target.document.lifecycle(),
            DocumentLifecycle::Failed(_)
        ));
        let expected = format!("document shutdown failed: {error}");
        retained_actions(&target, &expected);
        std::fs::remove_file(&recovery).unwrap();
        std::fs::create_dir(&recovery).unwrap();
        retained_actions(&target, &expected);
        assert_eq!(target.document.shutdown().unwrap_err(), error);
        assert_eq!(std::fs::read_dir(recovery).unwrap().count(), 0);
    }
}

#[test]
fn retained_region_family_windows_release_document_session_and_vanished_origins_abandon_work() {
    software();
    let target = fixture(Family::Region);
    let cold = Controller::new_with_access(&target.access, || Some(1.0)).unwrap();
    let vanished = cold.window.as_weak();
    drop(cold);
    assert!(vanished.upgrade().is_none());
    callback(&vanished, &target.access.downgrade(), |_, _| {
        panic!("vanished window accessed Session")
    });
    let weak = Arc::downgrade(&target.shared);
    let continuation = target.access.downgrade();
    let surface = target.surface.window.clone_strong();
    let volume = target.volume.window.clone_strong();
    let region = target.region.window.clone_strong();
    region.hide().unwrap();
    drop(target);
    assert!(weak.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    surface.invoke_edited();
    volume.invoke_frame();
    region.invoke_review();
    assert_eq!(surface.get_message(), "the document is closed");
    assert_eq!(volume.get_message(), "the document is closed");
    assert_eq!(region.get_message(), "the document is closed");
    surface.invoke_cancel();
    volume.invoke_cancel();
    region.invoke_cancel();
}

#[test]
fn region_frame_actions_abandon_vanished_originating_parent_without_an_aspect_fallback() {
    software();
    let target = fixture(Family::Region);
    let parent = crate::AppWindow::new().unwrap();
    let volume = volume_ui::Controller::new_with_access(&target.access, {
        let parent = parent.as_weak();
        move || {
            let parent = parent.upgrade()?;
            Some(parent.get_viewport_width() / parent.get_viewport_height().max(1.0))
        }
    })
    .unwrap();
    let region = Controller::new_with_access(&target.access, {
        let parent = parent.as_weak();
        move || {
            let parent = parent.upgrade()?;
            Some(parent.get_viewport_width() / parent.get_viewport_height().max(1.0))
        }
    })
    .unwrap();
    volume.initialize(&mut lock(&target.shared));
    volume.window.invoke_preview_edit();
    region.show(&lock(&target.shared)).unwrap();
    region.window.invoke_preview();
    drop(parent);
    let before = snapshot(&target);
    volume.window.invoke_frame();
    region.window.invoke_frame();
    assert_eq!(snapshot(&target), before);
    region.window.hide().unwrap();
}
