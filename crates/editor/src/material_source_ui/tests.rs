use super::*;
use crate::session::Actor;
use std::sync::{Arc, Mutex};

#[test]
fn material_browser_source_window_reads_refreshes_reports_errors_and_closes_without_applying() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (root, game) = crate::test_assets::game();
    let path = root.path().join("materials/test/source.vmt");
    std::fs::write(&path, "UnlitGeneric { $basetexture test/checker }").unwrap();
    let mut session = Session::new(Ok(game));
    let id = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::Agent,
            &[id],
            map_core::edit::Transform {
                translation: glam::DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let draft = session.render_document().vmf().clone();
    let snapshot = session.history_snapshot();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let browser = MaterialWindow::new().unwrap();
    let source_window = bind(&browser, shared.clone());
    browser.set_material("test/source".into());
    browser.show().unwrap();
    browser.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&browser, "Source…")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    let controller = source_window.borrow();
    let controller = controller.as_ref().unwrap();
    assert!(controller.window.window().is_visible());
    assert!(
        controller
            .window
            .get_provenance()
            .contains(&path.to_string_lossy().to_string())
    );
    assert!(controller.window.get_resolved().contains("unlitgeneric"));
    assert!(
        controller
            .window
            .get_source()
            .contains("$basetexture test/checker")
    );
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "material-source");
    crate::ui::tests::assert_dialog_button_visible(&controller.window, "Refresh source");
    crate::ui::tests::assert_dialog_button_visible(&controller.window, "Close");
    std::fs::write(&path, "Patch { include materials/test/missing.vmt }").unwrap();
    controller.window.invoke_refresh_source();
    assert!(controller.window.get_status().contains("doesn't exist"));
    assert!(controller.window.get_source().contains("Patch"));
    assert!(controller.window.get_provenance().contains("Not found"));
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    controller.show(&lock(&shared), "test/source").unwrap();
    assert!(controller.window.get_status().contains("doesn't exist"));
    controller.window.invoke_close_dialog();
    assert!(browser.window().is_visible());
    assert_eq!(browser.get_material(), "test/source");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    assert_eq!(lock(&shared).history_snapshot(), snapshot);
    assert_eq!(lock(&shared).camera(), camera);
    browser.hide().unwrap();
}

#[test]
fn software_material_source_shows_shared_water_inventory_and_keeps_declarations_first() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (root, game) = crate::test_assets::game();
    let path = root.path().join("materials/test/water.vmt");
    let text = "Water_DX9_HDR { $normalmap test/checker $basetexture test/checker %tooltexture test/checker $underwateroverlay 0 }";
    std::fs::write(&path, text).unwrap();
    let mut session = Session::new(Ok(game));
    let id = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::Agent,
            &[id],
            map_core::edit::Transform {
                translation: glam::DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let draft = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let selection = session.selected_ids();
    let settings = session.compile_settings();
    let shared = Arc::new(Mutex::new(session));
    let expected = lock(&shared).inspect_material_source("test/water").unwrap();
    assert_eq!(
        expected.water_dependencies.state,
        WaterDependencyState::SupportedStaticInventory
    );
    let browser = MaterialWindow::new().unwrap();
    let state = bind(&browser, shared.clone());
    browser.set_material("test/water".into());
    browser.invoke_inspect_source();
    let window = source_window(&state);
    assert!(window.window().is_visible());
    assert_eq!(
        window.get_source().to_string(),
        expected.raw_source.unwrap()
    );
    let resolved = window.get_resolved().to_string();
    assert!(resolved.starts_with("Resolved shader declaration: water_dx9_hdr"));
    assert!(
        resolved.find("$normalmap = test/checker").unwrap()
            < resolved.find("Water dependencies:").unwrap()
    );
    for label in [
        "Water dependencies: Supported static inventory",
        "CS:S SDK DX90 static Water",
        "Normal texture:",
        "Editor preview texture:",
        "SDK default",
        "Runtime resource",
        "Evidence: SDK",
        "materials/test/checker.vtf",
        "VTF header: 64 × 32",
        "Layout observation: existing VTF parser",
    ] {
        assert!(
            resolved.contains(label),
            "Missing Water inventory row: {label}"
        );
    }
    assert!(!resolved.contains("supported_static_inventory"));
    assert!(window.get_status().contains("Supported static inventory"));
    window.window().take_snapshot().unwrap();
    crate::ui::tests::save_ui_snapshot(window.window(), "material-source-water");
    crate::ui::tests::assert_dialog_button_visible(&window, "Refresh source");
    crate::ui::tests::assert_dialog_button_visible(&window, "Close");

    std::fs::write(
        &path,
        "Water { $normalmap test/checker $flowmap test/unknown }",
    )
    .unwrap();
    window.invoke_refresh_source();
    assert!(window.get_status().contains("Unsupported inventory"));
    assert!(
        window
            .get_resolved()
            .contains("Issue [Unsupported inventory]")
    );
    assert!(window.get_resolved().contains("$flowmap"));
    assert!(window.get_source().contains("Water"));
    std::fs::write(&path, "Water { $normalmap test/missing }").unwrap();
    window.invoke_refresh_source();
    assert!(window.get_status().contains("Source unavailable"));
    assert!(!window.get_status().contains("Resolution error"));
    assert!(window.get_resolved().contains("$normalmap = test/missing"));
    assert!(window.get_resolved().contains("Issue [Source unavailable]"));
    std::fs::write(&path, "invalid").unwrap();
    window.invoke_refresh_source();
    assert!(window.get_status().starts_with("Resolution error:"));
    assert_eq!(window.get_source(), "invalid");
    assert!(!window.get_resolved().contains("Water dependencies:"));
    std::fs::write(&path, "UnlitGeneric { $basetexture test/not-read }").unwrap();
    window.invoke_refresh_source();
    assert_eq!(
        window.get_status(),
        "Source and Patch declarations resolved."
    );
    assert!(!window.get_resolved().contains("Water dependencies:"));
    assert!(
        window
            .get_resolved()
            .contains("$basetexture = test/not-read")
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).selected_ids(), selection);
    assert_eq!(lock(&shared).compile_settings(), settings);
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}

fn admitted_fixture() -> (
    tempfile::TempDir,
    crate::runtime::DocumentHandle,
    SharedSession,
    DocumentAccess,
    MaterialWindow,
    Rc<RefCell<Option<Controller>>>,
) {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, game) = crate::test_assets::game();
    std::fs::write(
        directory.path().join("materials/test/source.vmt"),
        "UnlitGeneric { $basetexture test/checker }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test/checker",
            false,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let browser = MaterialWindow::new().unwrap();
    let state = bind_with_access(&browser, &access);
    browser.set_material("test/source".into());
    (directory, document, shared, access, browser, state)
}

fn source_window(state: &RefCell<Option<Controller>>) -> MaterialSourceWindow {
    state.borrow().as_ref().unwrap().window.clone_strong()
}

fn fields(window: &MaterialSourceWindow) -> serde_json::Value {
    serde_json::json!({
        "material":window.get_material().to_string(), "source":window.get_source().to_string(),
        "resolved":window.get_resolved().to_string(), "provenance":window.get_provenance().to_string()
    })
}

#[test]
fn managed_inspector_rejects_new_reads_and_form_creation_during_drain_and_keeps_peer_live() {
    use crate::runtime::DocumentLifecycle;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let (directory, document, shared, access, browser, state) = admitted_fixture();
    browser.invoke_inspect_source();
    let window = source_window(&state);
    assert!(window.get_source().contains("UnlitGeneric"));
    browser.set_sprite_resource(true);
    browser.invoke_inspect_source();
    assert_eq!(
        *state.borrow().as_ref().unwrap().material.borrow(),
        "materials/test/source.vmt"
    );
    browser.set_material("test/source.bad".into());
    browser.invoke_inspect_source();
    assert!(browser.get_detail().contains("Unsupported Sprite filename"));
    browser.set_sprite_resource(false);
    browser.set_material("test/source".into());
    browser.invoke_inspect_source();
    let expected = fields(&window);
    let (peer_directory, peer_game) = crate::test_assets::game();
    let peer_path = peer_directory.path().join("materials/test/peer.vmt");
    std::fs::write(&peer_path, "VertexLitGeneric { $basetexture test/checker }").unwrap();
    let mut peer = Session::new(Ok(peer_game));
    peer.configure_recovery(Ok(peer_directory.path().join("recovery")));
    let peer_shared = Arc::new(Mutex::new(peer));
    let peer_document = crate::test_assets::document(peer_shared.clone());
    let peer_access = DocumentAccess::for_document(peer_document.clone());
    let peer_browser = MaterialWindow::new().unwrap();
    let peer_state = bind_with_access(&peer_browser, &peer_access);
    peer_browser.set_material("test/peer".into());
    peer_browser.invoke_inspect_source();
    let peer_window = source_window(&peer_state);
    let peer_fields = fields(&peer_window);
    let peer_source = lock(&peer_shared).document().vmf().clone();
    let peer_history = lock(&peer_shared).history_snapshot();
    let cold = MaterialWindow::new().unwrap();
    let cold_state = bind_with_access(&cold, &access);
    cold.set_material("test/unread".into());
    let path = directory.path().join("materials/test/source.vmt");
    let changed = b"Patch { include materials/test/never-read.vmt }";
    std::fs::write(&path, changed).unwrap();
    browser.set_material("test/unread".into());
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let gate = document.admit().unwrap();
    let draining = document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while document.lifecycle() == DocumentLifecycle::Live
        || lock(&shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline, "document did not begin draining");
        std::thread::yield_now();
    }
    browser.invoke_inspect_source();
    window.invoke_refresh_source();
    cold.invoke_inspect_source();
    assert_eq!(browser.get_detail(), "the editor is closing");
    assert_eq!(window.get_status(), "the editor is closing");
    assert_eq!(cold.get_detail(), "the editor is closing");
    assert!(cold_state.borrow().is_none());
    assert_eq!(fields(&window), expected);
    assert_eq!(
        *state.borrow().as_ref().unwrap().material.borrow(),
        "test/source"
    );
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    browser.invoke_inspect_source();
    window.invoke_refresh_source();
    cold.invoke_inspect_source();
    assert_eq!(browser.get_detail(), "the document is closed");
    assert_eq!(window.get_status(), "the document is closed");
    assert!(cold_state.borrow().is_none());
    assert_eq!(fields(&window), expected);
    assert!(window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(fields(&peer_window), peer_fields);
    std::fs::write(&peer_path, "UnlitGeneric { $basetexture test/checker }").unwrap();
    peer_window.invoke_refresh_source();
    assert!(peer_window.get_source().contains("UnlitGeneric"));
    assert_ne!(fields(&peer_window), peer_fields);
    assert_eq!(peer_document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(lock(&peer_shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer_shared).history_snapshot(), peer_history);
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    let weak_session = Arc::downgrade(&shared);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    browser.invoke_inspect_source();
    window.invoke_refresh_source();
    assert_eq!(window.get_status(), "the document is closed");
}

#[test]
fn failed_material_source_admission_preserves_read_and_cold_form_without_retry() {
    let (directory, document, shared, access, browser, state) = admitted_fixture();
    browser.invoke_inspect_source();
    let window = source_window(&state);
    let expected = fields(&window);
    let cold = MaterialWindow::new().unwrap();
    let cold_state = bind_with_access(&cold, &access);
    let recovery = directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    std::fs::remove_file(directory.path().join("materials/test/source.vmt")).unwrap();
    browser.invoke_inspect_source();
    window.invoke_refresh_source();
    cold.invoke_inspect_source();
    let expected_error = format!("document shutdown failed: {error}");
    assert_eq!(browser.get_detail(), expected_error);
    assert_eq!(window.get_status(), expected_error);
    assert_eq!(cold.get_detail(), expected_error);
    assert_eq!(fields(&window), expected);
    assert!(cold_state.borrow().is_none());
    assert!(!lock(&shared).document().vmf().world.solids.is_empty());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn vanished_material_source_window_abandons_refresh_while_document_remains_live() {
    let (_directory, document, _shared, access, browser, state) = admitted_fixture();
    browser.invoke_inspect_source();
    let window = source_window(&state);
    let weak = window.as_weak();
    window.invoke_close_dialog();
    drop(window);
    drop(state);
    drop(browser);
    assert!(
        with_source(&weak, &access.downgrade(), |_, _| panic!(
            "vanished source window executed read"
        ))
        .is_none()
    );
    assert_eq!(
        document.lifecycle(),
        crate::runtime::DocumentLifecycle::Live
    );
}
