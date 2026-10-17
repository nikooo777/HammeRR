use super::*;
use std::collections::BTreeSet;

thread_local! {
    static FAIL_HIDES: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

pub(super) fn fail_hide(label: &str) -> bool {
    FAIL_HIDES.with(|labels| labels.borrow_mut().remove(label))
}

use crate::runtime::DocumentLifecycle;
use map_core::surface::FaceId;
use serde_json::{Value, json};
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct Fixture {
    owner: DocumentWindow,
    document: DocumentHandle,
    shared: SharedSession,
    directory: tempfile::TempDir,
}

struct OpenWindows {
    app: AppWindow,
    properties: PropertiesWindow,
    color: ColorWindow,
    property_material: MaterialWindow,
    sky: SkyAssetsWindow,
    material: MaterialWindow,
    source: MaterialSourceWindow,
    surface: SurfaceWindow,
    compile: CompileWindow,
    baked: BakedLightmapsWindow,
    prefabs: PrefabsWindow,
    paste: PasteWindow,
    checks: ChecksWindow,
    context: ContextWindow,
    contents: InstanceContentsWindow,
    reference: InstanceReferenceWindow,
    extraction: InstanceExtractionWindow,
}

impl OpenWindows {
    fn windows(&self) -> [&slint::Window; 17] {
        [
            self.app.window(),
            self.properties.window(),
            self.color.window(),
            self.property_material.window(),
            self.sky.window(),
            self.material.window(),
            self.source.window(),
            self.surface.window(),
            self.compile.window(),
            self.baked.window(),
            self.prefabs.window(),
            self.paste.window(),
            self.checks.window(),
            self.context.window(),
            self.contents.window(),
            self.reference.window(),
            self.extraction.window(),
        ]
    }
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

fn fixture(ctx: GpuContext) -> Fixture {
    let (directory, game) = crate::test_assets::game();
    let sky_directory = directory.path().join("materials/skybox");
    std::fs::create_dir_all(&sky_directory).unwrap();
    for suffix in render::sky::SUFFIXES {
        std::fs::write(
            sky_directory.join(format!("retirement{suffix}.vmt")),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
    }
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", "retirement");
    let source = directory.path().join("retirement.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.set_definitions(Actor::User, Catalog::parse(
        "@PointClass = retirement [ tint(color255) : \"Tint\" : \"32 64 128\" material(material) : \"Material\" : \"test/checker\" ]"
    ).unwrap());
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .create_entity(
            Actor::User,
            "retirement",
            Some(DVec3::splat(96.0)),
            BTreeMap::new(),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let owner = create_document_window(document.clone(), ctx, "MCP test".into(), None).unwrap();
    Fixture {
        owner,
        document,
        shared,
        directory,
    }
}

fn open_windows(f: &Fixture) -> OpenWindows {
    let app = f.owner.app().clone_strong();
    app.show().unwrap();
    app.invoke_batch_properties();
    let ui = f.owner.ui.as_ref().unwrap();
    let properties = ui
        .borrow()
        .properties_window
        .as_ref()
        .unwrap()
        .clone_strong();
    properties.invoke_pick_color("tint".into());
    properties.invoke_pick_material("material".into());
    let (color, property_material) = {
        let ui = ui.borrow();
        let pickers = ui.property_pickers.as_ref().unwrap();
        (
            pickers.color.clone_strong(),
            pickers.material.clone_strong(),
        )
    };
    assert!(color.window().is_visible());
    assert!(property_material.window().is_visible());
    app.invoke_world_settings();
    properties.invoke_inspect_sky_assets();
    let sky = ui
        .borrow()
        .property_pickers
        .as_ref()
        .unwrap()
        .sky
        .borrow()
        .as_ref()
        .unwrap()
        .window
        .clone_strong();
    assert!(sky.window().is_visible());
    let face = {
        let session = lock(&f.shared);
        let brush = &session.document().vmf().world.solids[0];
        FaceId {
            solid: brush.id,
            side: brush.sides[0].id,
        }
    };
    lock(&f.shared)
        .select_faces(&[face], SelectMode::Replace)
        .unwrap();
    show_surface_window(&app, ui);
    let surface = ui.borrow().surface_window.as_ref().unwrap().clone_strong();
    surface.set_shift_u("7".into());
    surface.invoke_preview_edit();
    assert!(lock(&f.shared).has_face_preview());
    surface.invoke_browse();
    let material = ui.borrow().material_window.as_ref().unwrap().clone_strong();
    material.set_material("test/checker".into());
    material.invoke_inspect_source();
    let source = ui
        .borrow()
        .material_source
        .as_ref()
        .unwrap()
        .borrow()
        .as_ref()
        .unwrap()
        .window
        .clone_strong();
    app.invoke_build_status();
    let (compile, baked) = {
        let ui = ui.borrow();
        let compile = ui.compile_status.as_ref().unwrap();
        (
            compile.window.clone_strong(),
            compile.baked_lightmaps_window().clone_strong(),
        )
    };
    baked.show().unwrap();
    app.invoke_prefabs();
    let (prefabs, paste) = {
        let ui = ui.borrow();
        let prefabs = ui.prefabs.as_ref().unwrap();
        (
            prefabs.window.clone_strong(),
            prefabs.paste_window().clone_strong(),
        )
    };
    paste.show().unwrap();
    app.invoke_map_checks();
    let checks = ui.borrow().checks_window.as_ref().unwrap().clone_strong();
    checks.invoke_run_checks();
    app.invoke_design_context();
    let context = ui.borrow().context_window.as_ref().unwrap().clone_strong();
    context.invoke_begin("map".into());
    context.set_summary("Retained context draft".into());
    context.set_body("Retirement must not apply or cancel this draft.".into());
    context.invoke_edit();
    app.invoke_inspect_instance_contents();
    app.invoke_create_instance_reference();
    app.invoke_extract_instance();
    let contents = f.owner.contents.as_ref().unwrap().window().clone_strong();
    let reference = f.owner.reference.as_ref().unwrap().window().clone_strong();
    let extraction = f.owner.extraction.as_ref().unwrap().window().clone_strong();
    let opened = OpenWindows {
        app,
        properties,
        color,
        property_material,
        sky,
        material,
        source,
        surface,
        compile,
        baked,
        prefabs,
        paste,
        checks,
        context,
        contents,
        reference,
        extraction,
    };
    for window in opened.windows() {
        assert!(window.is_visible());
    }
    opened
}

fn state(shared: &SharedSession) -> Value {
    let session = lock(shared);
    json!({
        "map": session.document().vmf().write(&Default::default()),
        "history": session.history_snapshot(),
        "selection": session.selected_ids().iter().map(ToString::to_string).collect::<Vec<_>>(),
        "faces": format!("{:?}", session.selected_faces()),
        "camera": format!("{:?}", session.camera()),
        "context": session.context_state(),
        "properties": session.property_draft(),
        "face_preview": session.face_preview_token(),
        "instance_reference": session.get_instance_reference(),
        "view": session.view_serial(),
        "scene": session.scene_serial(),
    })
}

#[test]
#[ignore = "requires a GPU adapter for the real DocumentWindow viewport"]
fn owned_document_retirement_hides_nested_windows_stops_timers_and_preserves_peer() {
    software();
    let ctx = GpuContext::new().unwrap();
    let mut target = fixture(ctx.clone());
    let mut peer = fixture(ctx.clone());
    let windows = open_windows(&target);
    let peer_app = peer.owner.app().clone_strong();
    peer_app.show().unwrap();
    assert!(target.owner.frame_timer.as_ref().unwrap().running());
    assert!(target.owner.contents.as_ref().unwrap().timer_running());
    assert!(target.owner.reference.as_ref().unwrap().timer_running());
    assert!(target.owner.extraction.as_ref().unwrap().timer_running());
    windows.contents.set_summary("pending timer refresh".into());
    windows.reference.set_status("pending timer refresh".into());
    windows
        .extraction
        .set_summary("pending timer refresh".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300));
    assert!(!windows.app.get_camera().is_empty());
    assert_ne!(windows.contents.get_summary(), "pending timer refresh");
    assert_ne!(windows.reference.get_status(), "pending timer refresh");
    assert_ne!(windows.extraction.get_summary(), "pending timer refresh");
    let local_ui = target.owner.ui.as_ref().unwrap().clone();
    let weak_ui = Rc::downgrade(&local_ui);
    let contents = Rc::downgrade(target.owner.contents.as_ref().unwrap());
    let reference = Rc::downgrade(target.owner.reference.as_ref().unwrap());
    let extraction = Rc::downgrade(target.owner.extraction.as_ref().unwrap());
    {
        let mut ui = local_ui.borrow_mut();
        let mut session = lock(&target.shared);
        let mesh = session.build_scene();
        ui.viewport.set_scene(&ctx, &mesh, session.materials());
        let image = ui.viewport.render(&ctx, (320, 240), 1.0).unwrap();
        assert_eq!(image.size().width, 320);
        assert_eq!(image.size().height, 240);
        windows.app.set_frame(image);
        ui.viewport.key("w", true);
        ui.pick_release = true;
    }
    let target_before = state(&target.shared);
    let peer_before = state(&peer.shared);
    assert_eq!(target.owner.retire(), Ok(UiRetired));
    assert_eq!(target.owner.retire(), Ok(UiRetired));
    for window in windows.windows() {
        assert!(!window.is_visible());
    }
    assert_eq!(windows.app.get_frame().size().width, 0);
    assert_eq!(windows.app.get_frame().size().height, 0);
    assert!(target.owner.frame_timer.is_none());
    assert!(contents.upgrade().is_none());
    assert!(reference.upgrade().is_none());
    assert!(extraction.upgrade().is_none());
    {
        let mut ui = local_ui.borrow_mut();
        assert!(!ui.pick_release);
        assert!(!ui.object_cycle.active());
        assert!(!ui.interaction.active());
        let camera = ui.viewport.camera;
        ui.viewport.update(1.0, 100.0);
        assert_eq!(ui.viewport.camera, camera);
    }
    drop(local_ui);
    assert!(weak_ui.upgrade().is_none());
    assert_eq!(state(&target.shared), target_before);
    assert_eq!(state(&peer.shared), peer_before);
    windows.contents.set_summary("retired timer".into());
    windows.reference.set_status("retired timer".into());
    windows.extraction.set_summary("retired timer".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300));
    assert_eq!(windows.contents.get_summary(), "retired timer");
    assert_eq!(windows.reference.get_status(), "retired timer");
    assert_eq!(windows.extraction.get_summary(), "retired timer");
    windows.app.invoke_inspect_instance_contents();
    windows.app.invoke_create_instance_reference();
    windows.app.invoke_extract_instance();
    windows.app.invoke_map_checks();
    assert_eq!(state(&target.shared), target_before);
    windows.app.show().unwrap();
    windows
        .app
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!windows.app.window().is_visible());
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Live);
    peer_app.invoke_map_checks();
    let peer_checks = peer
        .owner
        .ui
        .as_ref()
        .unwrap()
        .borrow()
        .checks_window
        .as_ref()
        .unwrap()
        .clone_strong();
    peer_checks.invoke_run_checks();
    assert!(lock(&peer.shared).map_check_report().is_some());
    assert!(peer_checks.window().is_visible());
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(peer.owner.retire(), Ok(UiRetired));
    target.document.shutdown().unwrap();
    peer.document.shutdown().unwrap();
}

#[test]
#[ignore = "requires a GPU adapter for the real DocumentWindow viewport"]
fn retirement_error_path_aggregates_hide_failures_and_never_acknowledges_a_repeat() {
    software();
    let mut target = fixture(GpuContext::new().unwrap());
    let windows = open_windows(&target);
    let weak_ui = Rc::downgrade(target.owner.ui.as_ref().unwrap());
    let before = state(&target.shared);
    FAIL_HIDES.with(|labels| {
        labels
            .borrow_mut()
            .extend(["checks_window".into(), "Editor".into()]);
    });
    let error = target
        .owner
        .finish(Err("event loop failed".into()))
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("event loop failed; UI retirement failed: "));
    assert!(error.contains("checks_window: injected hide failure"));
    assert!(error.contains("Editor: injected hide failure"));
    assert!(weak_ui.upgrade().is_none());
    assert!(target.owner.frame_timer.is_none());
    assert!(target.owner.contents.is_none());
    assert!(target.owner.reference.is_none());
    assert!(target.owner.extraction.is_none());
    for window in windows.windows().into_iter().skip(1) {
        if !std::ptr::eq(window, windows.checks.window()) {
            assert!(!window.is_visible());
        }
    }
    assert!(windows.app.window().is_visible());
    assert!(windows.checks.window().is_visible());
    let retired_error = target.owner.retire().unwrap_err();
    assert_eq!(target.owner.retire().unwrap_err(), retired_error);
    assert!(error.ends_with(&retired_error));
    assert_eq!(state(&target.shared), before);
    windows.app.hide().unwrap();
    windows.checks.hide().unwrap();
    target.document.shutdown().unwrap();
}

#[test]
#[ignore = "requires a GPU adapter for the real DocumentWindow viewport"]
fn ui_acknowledgement_releases_ui_while_a_fixed_mcp_server_keeps_its_session() {
    software();
    let ctx = GpuContext::new().unwrap();
    let mut target = fixture(ctx.clone());
    let windows = open_windows(&target);
    let server = EditorServer::for_document(target.document.clone(), Some(ctx));
    let weak_session = Arc::downgrade(&target.shared);
    let weak_ui = Rc::downgrade(target.owner.ui.as_ref().unwrap());
    let weak_app = windows.app.as_weak();
    let weak_source = windows.source.as_weak();
    let weak_sky = windows.sky.as_weak();
    assert_eq!(target.owner.retire(), Ok(UiRetired));
    assert!(weak_ui.upgrade().is_none());
    assert!(weak_session.upgrade().is_some());
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Live);
    assert!(
        windows
            .windows()
            .into_iter()
            .all(|window| !window.is_visible())
    );
    let Fixture {
        owner,
        document,
        shared,
        directory: _directory,
    } = target;
    drop(owner);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_some());
    drop(server);
    assert!(weak_session.upgrade().is_none());
    assert!(weak_app.upgrade().is_some());
    drop(windows);
    assert!(weak_app.upgrade().is_none());
    assert!(weak_source.upgrade().is_none());
    assert!(weak_sky.upgrade().is_none());
}

#[test]
#[ignore = "requires a GPU adapter for the real DocumentWindow viewport"]
fn retirement_reports_a_borrowed_ui_and_still_stops_every_timer_and_hides_other_windows() {
    software();
    let mut target = fixture(GpuContext::new().unwrap());
    let windows = open_windows(&target);
    let ui = target.owner.ui.as_ref().unwrap().clone();
    let borrowed = ui.borrow();
    let error = target.owner.retire().unwrap_err();
    assert!(error.contains("UI ownership is still borrowed"));
    assert!(target.owner.frame_timer.is_none());
    assert!(target.owner.contents.is_none());
    assert!(target.owner.reference.is_none());
    assert!(target.owner.extraction.is_none());
    assert!(
        windows
            .windows()
            .into_iter()
            .all(|window| !window.is_visible())
    );
    drop(borrowed);
    drop(ui);
    assert_eq!(target.owner.retire().unwrap_err(), error);
    target.document.shutdown().unwrap();
}
