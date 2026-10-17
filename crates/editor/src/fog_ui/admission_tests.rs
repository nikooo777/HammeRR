use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use crate::session::Actor;
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
    fog: Controller,
    statistics: crate::StatisticsWindow,
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
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .create_entity(
            Actor::User,
            "env_fog_controller",
            None,
            [
                ("targetname", "courtyard"),
                ("fogcolor", "128 160 255"),
                ("fogstart", "100"),
                ("fogend", "500"),
                ("fogenable", "1"),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let fog = Controller::new_with_access(&access).unwrap();
    fog.show(&lock(&shared)).unwrap();
    fog.window.set_preview_enabled(true);
    fog.window.set_controller_index(1);
    fog.window.invoke_choice_changed();
    assert!(fog.window.get_can_apply());
    fog.window.invoke_apply();
    assert_eq!(lock(&shared).fog_preview_state()["enabled"], true);
    fog.show(&lock(&shared)).unwrap();
    let statistics = crate::StatisticsWindow::new().unwrap();
    crate::statistics_ui::bind_with_access(&statistics, &access);
    statistics.invoke_refresh();
    assert_eq!(statistics.get_brushes(), "1");
    assert_eq!(
        statistics.get_rows().row_data(0).unwrap().name,
        "brick/test"
    );
    Fixture {
        document,
        shared,
        access,
        fog,
        statistics,
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
    history: String,
    selection: Vec<map_core::ObjectId>,
    camera: render::Camera,
    serials: [u64; 4],
    fog: Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn snapshot(fixture: &Fixture) -> Snapshot {
    let session = lock(&fixture.shared);
    Snapshot {
        map: session.document().vmf().clone(),
        history: session.history_snapshot(),
        selection: session.selected_ids(),
        camera: session.camera(),
        serials: [
            session.scene_serial(),
            session.view_serial(),
            session.log_serial(),
            session.camera_serial(),
        ],
        fog: session.fog_preview_state(),
        files: files(fixture.directory.path()),
    }
}

fn form_snapshot(fixture: &Fixture) -> Value {
    let form = fixture.fog.form.borrow();
    let fog = &fixture.fog.window;
    let statistics = &fixture.statistics;
    serde_json::json!({
        "fog_snapshot": form.snapshot,
        "fog_choices": form.choices,
        "fog_controllers": fog.get_controllers().iter().map(|row| row.to_string()).collect::<Vec<_>>(),
        "fog_fields": [fog.get_current().to_string(), fog.get_detail().to_string()],
        "fog_enabled": fog.get_preview_enabled(),
        "fog_controller": fog.get_controller_index(),
        "fog_apply": fog.get_can_apply(),
        "statistics_cards": [statistics.get_brushes().to_string(), statistics.get_brush_detail().to_string(), statistics.get_faces().to_string(), statistics.get_face_detail().to_string(), statistics.get_entities().to_string(), statistics.get_entity_detail().to_string(), statistics.get_details().to_string()],
        "statistics_rows": statistics.get_rows().iter().map(|row| (row.name.to_string(),row.count.to_string())).collect::<Vec<_>>(),
        "statistics_section": statistics.get_section(),
        "statistics_page": statistics.get_page(),
        "statistics_next": statistics.get_has_next(),
        "statistics_generation": statistics.get_generation().to_string(),
        "statistics_revision": statistics.get_revision().to_string(),
    })
}

fn retained_actions(fixture: &Fixture, error: &str) {
    let fog = &fixture.fog.window;
    let statistics = &fixture.statistics;
    fog.set_preview_enabled(false);
    fog.set_can_apply(false);
    fog.set_detail("Retained pending choice".into());
    fog.set_current("Retained authored fog".into());
    statistics.set_section(2);
    statistics.set_page(99);
    fog.show().unwrap();
    statistics.show().unwrap();
    let form = form_snapshot(fixture);
    fog.invoke_choice_changed();
    fog.invoke_refresh();
    fog.invoke_apply();
    statistics.invoke_refresh();
    assert_eq!(fog.get_message(), error);
    assert_eq!(statistics.get_summary(), error);
    assert_eq!(form_snapshot(fixture), form);
    assert!(fog.window().is_visible());
    assert!(statistics.window().is_visible());
    fog.invoke_cancel();
    statistics.invoke_cancel();
    assert!(!fog.window().is_visible());
    assert!(!statistics.window().is_visible());
    assert_eq!(form_snapshot(fixture), form);
}

#[test]
fn retained_fog_and_statistics_callbacks_reject_real_drain_and_finished_with_live_peer() {
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
    let before = snapshot(&target);
    let peer_before = snapshot(&peer);
    retained_actions(&target, "the editor is closing");
    assert_eq!(snapshot(&target), before);
    assert_eq!(snapshot(&peer), peer_before);
    peer.fog.window.set_preview_enabled(false);
    peer.fog.window.invoke_choice_changed();
    peer.fog.window.invoke_apply();
    assert_eq!(lock(&peer.shared).fog_preview_state()["enabled"], false);
    peer.statistics.set_section(1);
    peer.statistics.invoke_refresh();
    assert_eq!(
        peer.statistics.get_rows().row_data(0).unwrap().name,
        "env_fog_controller"
    );
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let peer_after = snapshot(&peer);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
    let before = snapshot(&target);
    retained_actions(&target, "the document is closed");
    assert_eq!(snapshot(&target), before);
    assert_eq!(snapshot(&peer), peer_after);
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
fn failed_fog_and_statistics_admission_preserves_forms_and_memoized_failure_after_storage_repair() {
    software();
    let fixture = fixture();
    let recovery = fixture.directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = fixture.document.shutdown().unwrap_err();
    let expected = format!("document shutdown failed: {error}");
    let before = snapshot(&fixture);
    retained_actions(&fixture, &expected);
    assert_eq!(snapshot(&fixture), before);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    let before = snapshot(&fixture);
    retained_actions(&fixture, &expected);
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(fixture.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn retained_fog_and_statistics_windows_release_owners_and_vanished_fog_abandons_admission() {
    software();
    let fixture = fixture();
    let cold = Controller::new_with_access(&fixture.access).unwrap();
    let vanished = cold.window.as_weak();
    drop(cold);
    assert!(vanished.upgrade().is_none());
    callback(&vanished, &fixture.access.downgrade(), |_, _| {
        panic!("a vanished fog window accessed Session")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    let weak_session = Arc::downgrade(&fixture.shared);
    let continuation = fixture.access.downgrade();
    let fog = fixture.fog.window.clone_strong();
    let statistics = fixture.statistics.clone_strong();
    drop(fixture);
    assert!(weak_session.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    fog.invoke_choice_changed();
    fog.invoke_refresh();
    fog.invoke_apply();
    statistics.invoke_refresh();
    assert_eq!(fog.get_message(), "the document is closed");
    assert_eq!(statistics.get_summary(), "the document is closed");
    fog.invoke_cancel();
    statistics.invoke_cancel();
    assert!(!fog.window().is_visible());
    assert!(!statistics.window().is_visible());
}
