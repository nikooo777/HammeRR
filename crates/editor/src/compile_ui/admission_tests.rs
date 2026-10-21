use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use slint::Model;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

#[derive(Debug, PartialEq)]
struct Snapshot {
    map: source_formats::vmf::Vmf,
    history: String,
    selected: Vec<map_core::ObjectId>,
    camera: render::Camera,
    serials: [u64; 3],
    settings: serde_json::Value,
    recipes: serde_json::Value,
    capture: serde_json::Value,
    measurement: serde_json::Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
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

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no native runtime required".into()));
    session.configure_build_recipes(Ok(directory.path().join("settings")));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .save(Actor::User, Some(&directory.path().join("source.vmf")))
        .unwrap();
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            true,
        )
        .unwrap();
    session
        .preview_measurement(
            Actor::Agent,
            editor_core::session::measurements::Request {
                id: None,
                name: "Retained review".into(),
                start: glam::DVec3::ZERO,
                end: glam::DVec3::X,
            },
            &session.measurement_stamp(),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&shared, access.clone()).unwrap();
    controller.refresh(&lock(&shared));
    Fixture {
        directory,
        document,
        access,
        shared,
        controller,
    }
}

fn snapshot(fixture: &Fixture) -> Snapshot {
    let session = lock(&fixture.shared);
    Snapshot {
        map: session.document().vmf().clone(),
        history: session.history_snapshot(),
        selected: session.selected_ids(),
        camera: session.camera(),
        serials: [
            session.scene_serial(),
            session.view_serial(),
            session.log_serial(),
        ],
        settings: session.compile_settings(),
        recipes: session.build_recipes(),
        capture: session.native_capture_runtime(),
        measurement: session.measurement_state(),
        files: files(fixture.directory.path()),
    }
}

fn retained_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    window.invoke_choose_recipe(0);
    window.invoke_apply_recipe();
    window.set_recipe_name("Retained recipe edit".into());
    window.invoke_save_recipe();
    window.invoke_replace_recipe();
    window.invoke_rename_recipe();
    window.invoke_delete_recipe();
    window.invoke_cancel_recipe_edit();
    window.invoke_reload_recipes();
    window.invoke_change_lighting(2);
    window.invoke_change_physics(1);
    window.invoke_change_sky_quality(1);
    window.invoke_change_visibility(1);
    window.invoke_change_bounces(1);
    window.invoke_change_smoothing_angle(45);
    window.invoke_change_pack_sky_assets(true);
    window.invoke_change_pack_water_assets(true);
    window.invoke_change_bake_static_prop_vertices(true);
    window.invoke_change_reflections(true);
    window.invoke_begin_capture_edit();
    window.set_capture_engine_root("/unapplied/engine".into());
    window.invoke_apply_capture_runtime();
    window.invoke_choose_capture_root(0);
    window.invoke_play_build();
    controller.token.set(Some(u64::MAX));
    window.invoke_cancel_build();
    assert_eq!(window.get_message(), error);
    let baked = controller.baked_lightmaps.window();
    baked.set_build_state("Retained inspection".into());
    baked.set_selected("Retained sample".into());
    window.invoke_inspect_baked_lightmaps();
    baked.invoke_inspect_face();
    baked.invoke_inspect_selected_authored();
    baked.invoke_inspect_origin(0);
    baked.invoke_find_matches();
    baked.invoke_inspect_match(0);
    assert_eq!(baked.get_build_state(), "Retained inspection");
    assert_eq!(baked.get_selected(), "Retained sample");
    assert_eq!(baked.get_diagnostics(), error);
    baked.invoke_close_dialog();
    let props = controller.baked_prop_vertices.window();
    window.invoke_inspect_baked_prop_vertices();
    props.invoke_refresh_inspection();
    props.invoke_change_channel();
    props.invoke_inspect_prop(0);
    props.invoke_inspect_section(0);
    props.invoke_page_props(1);
    props.invoke_page_sections(1);
    props.invoke_page_vertices(1);
    assert_eq!(props.get_diagnostics(), error);
    assert_eq!(props.get_vertex_rows().row_count(), 0);
    props.invoke_close_dialog();
    window.invoke_cancel_capture_edit();
    assert!(!window.get_capture_editing());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}

#[test]
fn retained_compile_controls_reject_real_drain_and_terminal_owner_without_touching_peer() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let fixture = fixture();
    let peer = self::fixture();
    fixture.controller.window.invoke_change_physics(1);
    assert_eq!(
        lock(&fixture.shared).compile_settings()["physics"],
        "engine"
    );
    fixture
        .controller
        .window
        .set_recipe_name("Before drain".into());
    fixture.controller.window.invoke_save_recipe();
    fixture.controller.window.invoke_begin_capture_edit();
    assert!(fixture.controller.window.get_capture_editing());
    fixture.controller.window.show().unwrap();
    let gate = fixture.document.admit().unwrap();
    let draining = fixture.document.clone();
    let (send, receive) = mpsc::channel();
    let shutdown = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
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
    retained_actions(&fixture.controller, "the editor is closing");
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(snapshot(&peer), peer_before);
    peer.controller.window.invoke_change_lighting(1);
    peer.controller
        .window
        .set_recipe_name("Peer remains live".into());
    peer.controller.window.invoke_save_recipe();
    assert_eq!(
        lock(&peer.shared).compile_settings()["lighting"],
        "direct_ldr"
    );
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let peer_after = snapshot(&peer);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    shutdown.join().unwrap();
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
    let before = snapshot(&fixture);
    retained_actions(&fixture.controller, "the document is closed");
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
    let continuation = fixture.access.downgrade();
    drop(fixture.access);
    drop(fixture.document);
    assert!(continuation.upgrade().is_err());
    fixture.controller.window.invoke_change_lighting(0);
    assert_eq!(
        fixture.controller.window.get_message(),
        "the document is closed"
    );
    let weak_session = Arc::downgrade(&fixture.shared);
    drop(fixture.shared);
    assert!(weak_session.upgrade().is_none());
    assert_eq!(snapshot(&peer), peer_after);
    peer.document.shutdown().unwrap();
}

#[test]
fn vanished_compile_windows_and_runtime_are_not_retained_by_callbacks() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = fixture();
    let weak_window = fixture.controller.window.as_weak();
    let target = fixture.access.downgrade();
    let baked = fixture.controller.baked_lightmaps.window().clone_strong();
    let props = fixture
        .controller
        .baked_prop_vertices
        .window()
        .clone_strong();
    drop(fixture.controller);
    assert!(weak_window.upgrade().is_none());
    admitted(&weak_window, &target, |_, _| {
        panic!("vanished compile window ran")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    drop(fixture.access);
    drop(fixture.document);
    assert!(target.upgrade().is_err());
    let weak_session = Arc::downgrade(&fixture.shared);
    drop(fixture.shared);
    assert!(weak_session.upgrade().is_none());
    baked.invoke_inspect_face();
    assert_eq!(baked.get_diagnostics(), "the document is closed");
    baked.invoke_close_dialog();
    props.invoke_refresh_inspection();
    assert_eq!(props.get_diagnostics(), "the document is closed");
    props.invoke_close_dialog();
}

#[test]
fn compile_callback_admission_lasts_until_its_complete_body_returns() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = fixture();
    let draining = fixture.document.clone();
    let (entered, start) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        start.recv_timeout(TIMEOUT).unwrap();
        finished.send(draining.shutdown()).unwrap();
    });
    let calls = Cell::new(0);
    admitted(
        &fixture.controller.window.as_weak(),
        &fixture.access.downgrade(),
        |_, shared| {
            entered.send(()).unwrap();
            let deadline = Instant::now() + TIMEOUT;
            while fixture.document.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(done.try_recv().is_err());
            assert!(lock(shared).recovery_entries().unwrap().entries.is_empty());
            calls.set(calls.get() + 1);
            assert!(done.try_recv().is_err());
        },
    );
    done.recv_timeout(TIMEOUT).unwrap().unwrap();
    shutdown.join().unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(
        lock(&fixture.shared)
            .recovery_entries()
            .unwrap()
            .entries
            .len(),
        1
    );
}
