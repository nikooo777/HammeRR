use super::*;
use crate::identity::id_string;
use source_formats::fgd::Catalog;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[test]
pub(crate) fn selection_outlines_control_shares_default_and_agent_view_without_map_changes() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap();
    let source = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    let selection = state.selected_ids();
    let shared = Arc::new(Mutex::new(state));
    let app = AppWindow::new().unwrap();
    bind_view_controls(&app, shared.clone());
    refresh_view_controls(&app, &lock(&shared));
    assert!(app.get_selection_outlines());
    app.set_selection_outlines(false);
    app.invoke_options_changed();
    assert!(!lock(&shared).view().selection_outlines);
    {
        let mut session = lock(&shared);
        let view = session.view();
        session.set_view(render::ViewOptions {
            selection_outlines: true,
            ..view
        });
    }
    refresh_view_controls(&app, &lock(&shared));
    assert!(app.get_selection_outlines());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert_eq!(lock(&shared).selected_ids(), selection);
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    save_ui_snapshot(app.window(), "selection-outlines");
}

#[test]
pub(crate) fn entity_picker_shows_shared_map_size_provenance_and_clears_on_reload() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.fgd");
    std::fs::write(
        &path,
        "// Map size preference\n@mapsize(16384, -16384)\n@PointClass = info_target []",
    )
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(Actor::Agent, Catalog::load(&path).unwrap());
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let window = EntityWindow::new().unwrap();
    window.set_classname("info_target".into());
    refresh_entity_window(&window, &session);
    assert_eq!(
        window.get_catalog_status().as_str(),
        session.entity_definitions_report().status()
    );
    assert!(window.get_catalog_status().contains("-16384 to 16384"));
    assert!(
        window
            .get_catalog_status()
            .contains("authored 16384, -16384")
    );
    assert!(window.get_catalog_status().contains("catalog.fgd:2"));
    assert!(window.get_catalog_status().contains("Informational only"));
    assert!(window.get_definition().contains("info_target (point)"));
    window.show().unwrap();
    assert_dialog_button_visible(&window, "Place entity");
    assert_dialog_button_visible(&window, "Load FGD…");
    save_ui_snapshot(window.window(), "entity-catalog-mapsize");
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    session.set_definitions(
        Actor::User,
        Catalog::parse("@PointClass = info_target []").unwrap(),
    );
    refresh_entity_window(&window, &session);
    assert!(
        window
            .get_catalog_status()
            .contains("No effective @mapsize")
    );
    assert!(!window.get_catalog_status().contains("catalog.fgd"));
}

#[test]
pub(crate) fn rope_preview_control_uses_shared_view_and_reports_bad_links_without_map_edits() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets".into()));
    state
        .create_entity(
            Actor::Agent,
            "move_rope",
            Some(DVec3::new(0.0, 0.0, 128.0)),
            [("NextKey".into(), "missing_endpoint".into())]
                .into_iter()
                .collect(),
        )
        .unwrap();
    let source = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(state));
    let app = AppWindow::new().unwrap();
    bind_view_controls(&app, shared.clone());
    refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_rope_preview());
    app.set_rope_preview(true);
    app.invoke_options_changed();
    assert!(lock(&shared).view().rope_preview);
    assert!(app.get_rope_preview_status().contains("no authored target"));
    assert_eq!(lock(&shared).rope_preview_state()["enabled"], true);
    {
        let mut session = lock(&shared);
        let view = session.view();
        session.set_view(render::ViewOptions {
            rope_preview: false,
            ..view
        });
    }
    refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_rope_preview());
    assert_eq!(app.get_rope_preview_status(), "Rope preview is off");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
}

#[test]
pub(crate) fn fog_preview_controls_choose_apply_cancel_and_reject_stale_or_unsupported_settings() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets".into()));
    let mut ids = Vec::new();
    for (name, blend) in [("courtyard_haze", "0"), ("directional_fog", "1")] {
        let id = state
            .create_entity(
                Actor::Agent,
                "env_fog_controller",
                None,
                [
                    ("targetname", name),
                    ("fogcolor", "130 170 210"),
                    ("fogstart", "256"),
                    ("fogend", "2048"),
                    ("fogmaxdensity", "0.8"),
                    ("fogenable", "1"),
                    ("fogblend", blend),
                ]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            )
            .unwrap();
        ids.push(id);
    }
    let source = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(state));
    let controller = fog_ui::Controller::new(&shared).unwrap();
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    window.window().set_size(slint::PhysicalSize::new(740, 650));
    assert_eq!(window.get_controller_index(), 0);
    window.set_preview_enabled(true);
    window.invoke_choice_changed();
    assert!(!window.get_can_apply());
    window.set_controller_index(1);
    window.invoke_choice_changed();
    assert!(window.get_can_apply());
    assert!(window.get_detail().contains("courtyard_haze"));
    assert!(
        window
            .get_detail()
            .lines()
            .any(|line| line == "Maximum density: 0.8 · Authored fog: enabled")
    );
    save_ui_snapshot(window.window(), "fog-preview");
    assert_dialog_button_visible(window, "Apply preview");
    assert_dialog_button_visible(window, "Cancel");
    window.invoke_cancel();
    assert_eq!(lock(&shared).fog_preview_state()["enabled"], false);
    controller.show(&lock(&shared)).unwrap();
    window.set_preview_enabled(true);
    window.set_controller_index(1);
    window.invoke_choice_changed();
    window.invoke_apply();
    assert_eq!(
        lock(&shared).fog_preview_state()["controller"],
        id_string(ids[0])
    );
    assert!(lock(&shared).view().fog.is_some());
    controller.show(&lock(&shared)).unwrap();
    window.set_controller_index(2);
    window.invoke_choice_changed();
    assert!(!window.get_can_apply());
    assert!(window.get_detail().contains("fogblend"));
    save_ui_snapshot(window.window(), "fog-preview-unsupported");
    window.invoke_apply();
    assert!(window.get_message().contains("fogblend"));
    window.set_controller_index(1);
    window.invoke_choice_changed();
    lock(&shared)
        .set_surface_view(render::SurfaceView::Flat, 1)
        .unwrap();
    window.invoke_apply();
    assert!(window.get_message().contains("changed"));
    window.invoke_refresh();
    window.set_preview_enabled(false);
    window.invoke_choice_changed();
    window.invoke_apply();
    assert!(lock(&shared).view().fog.is_none());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
}

#[test]
pub(crate) fn manipulation_controls_share_agent_previews_and_selection_without_desktop_input() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let id = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::new(64.0, 64.0, 64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let original = state.document().vmf().clone();
    state.set_camera(render::Camera::planar(
        Projection::Top,
        Vec3::new(32.0, 32.0, 32.0),
        256.0,
    ));
    let shared = Arc::new(Mutex::new(state));
    let app = AppWindow::new().unwrap();
    bind_manipulation_controls(&app, shared.clone());
    refresh_panel(&app, &mut lock(&shared));
    refresh_interaction(&app, &lock(&shared));
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    app.set_handles_mode_index(2);
    app.set_transform_space_index(1);
    app.set_rotation_step("22.5".into());
    app.set_transform_pivot_mode(2);
    app.set_pivot_x("10".into());
    app.set_pivot_y("20".into());
    app.set_pivot_z("30".into());
    app.invoke_transform_tool_changed();
    let settings = lock(&shared).transform_tool();
    assert_eq!(settings.mode, Some(render::handles::Mode::Rotate));
    assert!(settings.local);
    assert_eq!(settings.pivot, Some(DVec3::new(10.0, 20.0, 30.0)));
    assert_eq!(settings.angle_step, 22.5);
    app.set_rotation_step("-1".into());
    app.invoke_transform_tool_changed();
    assert_eq!(lock(&shared).transform_tool(), settings);
    assert!(app.get_message().contains("rotation snap"));
    for apply in [false, true] {
        lock(&shared)
            .preview_transform(
                Actor::Agent,
                &[id],
                edit::Transform {
                    translation: DVec3::new(32.0, 0.0, 0.0),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        refresh_interaction(&app, &lock(&shared));
        save_ui_snapshot(app.window(), "transform-handles-controls");
        for label in ["Apply transform", "Cancel"] {
            assert_dialog_button_visible(&app, label);
        }
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &app,
            if apply { "Apply transform" } else { "Cancel" },
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(!app.get_has_transform_preview());
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            if apply { 2 } else { 1 }
        );
    }
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    app.set_area_rule_index(1);
    app.set_area_mode_index(1);
    app.invoke_area_settings_changed();
    assert_eq!(
        lock(&shared).area_rule(),
        map_core::region::Match::Contained
    );
    assert_eq!(lock(&shared).area_mode(), SelectMode::Add);
    lock(&shared).select(&[], SelectMode::Replace).unwrap();
    for apply in [false, true] {
        let mut state = lock(&shared);
        let camera = state.camera();
        state
            .preview_area_selection(
                Actor::Agent,
                camera,
                Pointer {
                    position: DVec2::new(300.0, 300.0),
                    size: (1000, 1000),
                },
                DVec2::new(700.0, 700.0),
                map_core::region::Match::Contained,
                SelectMode::Add,
            )
            .unwrap();
        refresh_interaction(&app, &state);
        drop(state);
        save_ui_snapshot(app.window(), "area-selection-controls");
        for label in ["Apply selection", "Cancel"] {
            assert_dialog_button_visible(&app, label);
        }
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &app,
            if apply { "Apply selection" } else { "Cancel" },
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(!app.get_has_area_preview());
        assert_eq!(
            lock(&shared).selected_ids(),
            if apply { vec![id] } else { vec![] }
        );
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    }
    app.set_rotation_step("15".into());
    app.set_handles_mode_index(4);
    app.set_paint_radius(24.0);
    app.invoke_transform_tool_changed();
    assert!(lock(&shared).paint_enabled());
    assert_eq!(lock(&shared).paint_radius(), 24.0);
    app.set_paint_radius(32.0);
    app.invoke_paint_settings_changed();
    assert_eq!(lock(&shared).paint_radius(), 32.0);
    lock(&shared).select(&[], SelectMode::Replace).unwrap();
    for apply in [false, true] {
        let mut state = lock(&shared);
        let spec = session::paint::Spec {
            camera: state.camera(),
            pointer: Pointer {
                position: DVec2::new(400.0, 300.0),
                size: (800, 600),
            },
            radius: state.paint_radius(),
            mode: SelectMode::Replace,
            faces: false,
        };
        let old = state.begin_paint_selection(spec).unwrap();
        refresh_interaction(&app, &state);
        let current = state.begin_paint_selection(spec).unwrap();
        drop(state);
        app.invoke_cancel_interaction();
        assert_eq!(lock(&shared).paint_stroke().unwrap().token, current);
        assert_ne!(old, current);
        refresh_interaction(&app, &lock(&shared));
        save_ui_snapshot(app.window(), "paint-selection-controls");
        for label in ["Apply selection", "Cancel"] {
            assert_dialog_button_visible(&app, label);
        }
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &app,
            if apply { "Apply selection" } else { "Cancel" },
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(!app.get_has_paint_preview());
        assert_eq!(
            lock(&shared).selected_ids(),
            if apply { vec![id] } else { vec![] }
        );
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    }
}
#[test]
pub(crate) fn texture_brush_controls_preview_apply_and_cancel_without_desktop_input() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (assets, game) = crate::test_assets::game();
    let materials = assets.path().join("materials/metal");
    std::fs::create_dir(&materials).unwrap();
    std::fs::write(
        materials.join("sample.vmt"),
        "LightmappedGeneric { $basetexture test/checker }",
    )
    .unwrap();
    let mut state = Session::new(Ok(game));
    state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let ObjectId::Solid(source) = state
        .create_block(
            Actor::Agent,
            DVec3::new(256.0, 0.0, 0.0),
            DVec3::new(320.0, 64.0, 64.0),
            "metal/sample",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let side = state
        .document()
        .solid(source)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId {
        solid: source,
        side,
    };
    state.build_scene();
    state.sample_face_texture(face).unwrap();
    state.select_faces(&[face], SelectMode::Replace).unwrap();
    state.set_camera(render::Camera::planar(
        Projection::Top,
        Vec3::splat(32.0),
        256.0,
    ));
    let shared = Arc::new(Mutex::new(state));
    let app = AppWindow::new().unwrap();
    bind_manipulation_controls(&app, shared.clone());
    refresh_panel(&app, &mut lock(&shared));
    refresh_texture_brush_sample(&app, &mut lock(&shared));
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    app.set_handles_mode_index(5);
    app.set_texture_paint_mode(1);
    app.invoke_transform_tool_changed();
    assert_eq!(
        lock(&shared).paint_operation(),
        session::paint::Operation::Projection
    );
    assert_eq!(app.get_texture_sample_material(), "metal/sample");
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).document().undo_labels().count();
    let initial_image = app.get_texture_sample_preview().to_rgba8().unwrap();
    let texture = assets.path().join("materials/test/checker.vtf");
    let original = std::fs::read(&texture).unwrap();
    let mut changed = original.clone();
    for pixel in changed[80..].as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[15, 30, 240, 255]);
    }
    std::fs::write(&texture, changed).unwrap();
    lock(&shared).reload_material_assets(Actor::Agent);
    refresh_texture_brush_sample(&app, &mut lock(&shared));
    assert_eq!(
        app.get_texture_sample_preview()
            .to_rgba8()
            .unwrap()
            .as_bytes(),
        initial_image.as_bytes()
    );
    refresh_texture_brush_sample_assets(&app, &lock(&shared));
    assert_eq!(app.get_texture_sample_material(), "metal/sample");
    assert_ne!(
        app.get_texture_sample_preview()
            .to_rgba8()
            .unwrap()
            .as_bytes(),
        initial_image.as_bytes()
    );
    std::fs::write(&texture, b"broken VTF").unwrap();
    lock(&shared).reload_material_assets(Actor::User);
    refresh_texture_brush_sample_assets(&app, &lock(&shared));
    assert_eq!(app.get_texture_sample_material(), "metal/sample");
    assert_eq!(app.get_texture_sample_preview().size().width, 0);
    std::fs::write(&texture, original).unwrap();
    lock(&shared).reload_material_assets(Actor::Agent);
    refresh_texture_brush_sample_assets(&app, &lock(&shared));
    assert_eq!(
        app.get_texture_sample_preview()
            .to_rgba8()
            .unwrap()
            .as_bytes(),
        initial_image.as_bytes()
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);

    for apply in [false, true] {
        let mut state = lock(&shared);
        let before = state.document().vmf().clone();
        let history = state.document().undo_labels().count();
        let spec = session::paint::Spec {
            camera: state.camera(),
            pointer: Pointer {
                position: DVec2::new(400.0, 300.0),
                size: (800, 600),
            },
            radius: 12.0,
            mode: SelectMode::Replace,
            faces: true,
        };
        let old = state
            .begin_texture_stroke(spec, session::paint::Operation::Projection)
            .unwrap();
        refresh_interaction(&app, &state);
        let token = state
            .begin_texture_stroke(spec, session::paint::Operation::Projection)
            .unwrap();
        let preview = state.render_document().vmf().clone();
        drop(state);
        app.invoke_apply_area_selection();
        assert_ne!(old, token);
        assert_eq!(lock(&shared).paint_stroke().unwrap().token, token);
        refresh_interaction(&app, &lock(&shared));
        save_ui_snapshot(app.window(), "texture-stroke-controls");
        for label in ["Apply textures", "Cancel"] {
            assert_dialog_button_visible(&app, label);
        }
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &app,
            if apply { "Apply textures" } else { "Cancel" },
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(lock(&shared).paint_stroke().is_none());
        assert_eq!(
            lock(&shared).document().vmf(),
            if apply { &preview } else { &before }
        );
        assert_eq!(lock(&shared).selected_faces(), vec![face]);
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            history + usize::from(apply)
        );
    }
}

#[test]
pub(crate) fn clipping_controls_preview_apply_cancel_and_escape_without_desktop_input() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let id = state
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::new(128.0, 96.0, 64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let original = state.document().vmf().clone();
    let revision = state.document().revision();
    let shared = Arc::new(Mutex::new(state));
    let app = AppWindow::new().unwrap();
    bind_clip_controls(&app, shared.clone());
    app.on_key({
        let (app, shared) = (app.as_weak(), shared.clone());
        move |text, down, _, _, _, _| {
            if down && text == key_text(Key::Escape) {
                cancel_clip(&app.unwrap(), &mut lock(&shared));
                true
            } else {
                false
            }
        }
    });
    refresh_panel(&app, &mut lock(&shared));
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    app.window().take_snapshot().unwrap();
    scroll_inspector_to_control(&app, "Clip selected brushes").invoke_accessible_default_action();
    assert!(lock(&shared).clip_tool());
    app.set_clip_distance("64".into());
    for keep in [0, 1, 2] {
        app.set_clip_keep_index(keep);
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Preview plane")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        let state = lock(&shared);
        assert_eq!(state.document().vmf(), &original);
        assert_eq!(state.document().revision(), revision);
        let ObjectId::Solid(number) = id else {
            unreachable!()
        };
        let shape =
            map_core::BrushGeometry::from_solid(state.render_document().solid(number).unwrap())
                .unwrap();
        if keep == 0 {
            assert_eq!(shape.mins.x, 64.0);
        }
        if keep == 1 {
            assert_eq!(shape.maxs.x, 64.0);
        }
        assert_eq!(
            state.render_document().vmf().world.solids.len(),
            if keep == 2 { 2 } else { 1 }
        );
    }
    save_ui_snapshot(app.window(), "clipping-controls");
    for label in ["Preview plane", "Apply cut", "Cancel cut"] {
        assert_dialog_button_visible(&app, label);
        let button = i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, label)
            .next()
            .unwrap();
        assert!(button.absolute_position().x + button.size().width <= 752.5);
        assert!(button.absolute_position().y + button.size().height <= 342.5);
    }
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Cancel cut")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(lock(&shared).clip_preview().is_none());
    assert!(!lock(&shared).clip_tool());
    assert_eq!(lock(&shared).document().vmf(), &original);
    app.invoke_start_clip();
    app.invoke_preview_clip_plane();
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
    assert!(lock(&shared).clip_preview().is_none());
    assert!(!lock(&shared).clip_tool());
    app.invoke_start_clip();
    app.set_clip_normal_x("0".into());
    app.invoke_preview_clip_plane();
    assert!(!app.get_has_clip_preview());
    assert!(app.get_message().contains("nonzero"));
    app.set_clip_normal_x("1".into());
    app.invoke_preview_clip_plane();
    app.invoke_clip_inputs_changed();
    assert!(!app.get_has_clip_preview());
    app.invoke_preview_clip_plane();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Apply cut")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(lock(&shared).document().vmf().world.solids.len(), 2);
    assert_eq!(lock(&shared).document().undo_labels().count(), 2);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    app.invoke_start_clip();
    app.invoke_clip_points_action(0);
    assert!(app.get_clip_point_mode());
    let points = vec![
        DVec3::new(64.0, 0.0, 0.0),
        DVec3::new(64.0, 96.0, 0.0),
        DVec3::new(64.0, 96.0, 64.0),
    ];
    change_clip_points(&app, &mut lock(&shared), points.clone()).unwrap();
    refresh_clip(&app, &lock(&shared));
    assert!(app.get_has_clip_preview());
    app.set_clip_material("tools/toolsnodraw".into());
    app.invoke_clip_inputs_changed();
    assert_eq!(lock(&shared).clip_points().unwrap().points, points);
    assert_eq!(
        lock(&shared).clip_preview().unwrap().material.as_deref(),
        Some("tools/toolsnodraw")
    );
    save_ui_snapshot(app.window(), "three-point-clipping");
    app.set_clip_handles_mode(2);
    app.set_clip_angle_step("30".into());
    app.invoke_clip_handles_changed();
    let (token, before_plane) = {
        let mut state = lock(&shared);
        let before = state.clip_preview().unwrap().plane;
        let stamp = state.clip_point_stamp();
        let token = state
            .begin_clip_drag(render::handles::Handle::Z, None, &stamp)
            .unwrap();
        state
            .update_clip_drag(token, None, Some(DVec2::new(31.0, 0.0)))
            .unwrap();
        refresh_clip(&app, &state);
        (token, before)
    };
    assert!(app.get_clip_positioning());
    assert_dialog_button_visible(&app, "Cancel positioning");
    save_ui_snapshot(app.window(), "cut-plane-handles");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Cancel positioning")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!app.get_clip_positioning());
    assert_eq!(lock(&shared).clip_preview().unwrap().plane, before_plane);
    assert!(lock(&shared).finish_clip_drag(token).is_err());
    assert_eq!(lock(&shared).document().vmf(), &original);
    app.set_clip_angle_step("0".into());
    app.invoke_clip_handles_changed();
    assert!(app.get_message().contains("angle step"));

    for label in [
        "Use plane / line",
        "Remove last point",
        "Start again",
        "Apply cut",
        "Cancel cut",
    ] {
        assert_dialog_button_visible(&app, label);
    }
    app.invoke_clip_points_action(1);
    assert_eq!(app.get_clip_point_count(), 2);
    assert!(!app.get_has_clip_preview());
    app.invoke_cancel_clip();
    assert!(lock(&shared).clip_points().is_none());
    assert_eq!(lock(&shared).document().vmf(), &original);
}

#[test]
pub(crate) fn stale_cut_gestures_cannot_preview_after_mcp_cancel_or_map_replacement() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::splat(-32.0),
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    session.set_clip_tool(Actor::User, true).unwrap();
    let drag = BrushDrag {
        start: DVec2::new(-16.0, -32.0),
        epoch: session.clip_epoch(),
        generation: session.document_generation(),
        revision: session.document().revision(),
        projection: session.camera().projection,
    };
    let end = DVec2::new(16.0, 32.0);
    assert!(clip_drag_plane(&session, drag, end).is_ok());
    session.cancel_clip_preview();
    assert!(clip_drag_plane(&session, drag, end).is_err());
    session.new_map(Actor::Agent, true).unwrap();
    assert!(clip_drag_plane(&session, drag, end).is_err());
}

#[test]
pub(crate) fn connection_controls_share_agent_drafts_and_reject_stale_forms() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(Actor::Agent, Catalog::parse(r#"
@PointClass = logic_relay [ targetname(string) : "Name" input Trigger(void) : "Trigger the relay" output OnTrigger(void) : "Fires when triggered" ]
"#).unwrap());
    let ObjectId::Entity(source) = session
        .create_entity(
            Actor::Agent,
            "logic_relay",
            Some(DVec3::ZERO),
            BTreeMap::from([("targetname".into(), "courtyard_relay".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    let original = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let window = ConnectionWindow::new().unwrap();
    connection_ui::bind(&window, &shared);
    window.set_source_id(id_string(ObjectId::Entity(source)).into());
    connection_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Add connection")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    window.set_target("!self".into());
    window.set_input("Trigger".into());
    window.set_delay("0.25".into());
    window.set_times("1".into());
    window.invoke_fields_changed();
    assert!(window.get_can_apply());
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(
        lock(&shared).connection_draft().unwrap().fields.delay,
        "0.25"
    );
    save_ui_snapshot(window.window(), "entity-connections");
    assert_dialog_button_visible(&window, "Cancel");
    assert_dialog_button_visible(&window, "Apply connection");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Apply connection")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
    assert_eq!(lock(&shared).incoming_connections(source).unwrap().len(), 1);
    window.invoke_choose(id_string(ObjectId::Entity(source)).into(), 0);
    save_ui_snapshot(window.window(), "entity-connections-edit");
    let old_token = lock(&shared).connection_draft().unwrap().token;
    lock(&shared).begin_connection_edit(source, None).unwrap();
    window.invoke_apply();
    assert!(window.get_message().contains("draft changed"));
    assert_ne!(lock(&shared).connection_draft().unwrap().token, old_token);
    assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
    window.set_target("!self".into());
    window.set_input("Trigger".into());
    window.set_delay("-1".into());
    window.invoke_fields_changed();
    assert!(!window.get_can_apply());
    window.invoke_apply();
    assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Cancel")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(lock(&shared).connection_draft().is_none());
    window.show().unwrap();
    connection_ui::refresh(&window, &lock(&shared));
    window.invoke_add();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
    assert!(lock(&shared).connection_draft().is_none());
    assert!(!window.window().is_visible());
    window.show().unwrap();
    connection_ui::refresh(&window, &lock(&shared));
    window.invoke_delete_connection(0);
    assert!(lock(&shared).connection_records(source).unwrap().is_empty());
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
}

#[test]
pub(crate) fn rename_controls_share_previews_review_cancel_and_stale_guards() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(Actor::Agent, Catalog::parse(r#"
@PointClass = logic_relay [ targetname(target_source) : "Name" target(target_destination) : "Target" parentname(target_destination) : "Parent" ]
"#).unwrap());
    let ObjectId::Entity(target) = session
        .create_entity(
            Actor::Agent,
            "logic_relay",
            Some(DVec3::ZERO),
            BTreeMap::from([("targetname".into(), "courtyard_relay".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    let ObjectId::Entity(source) = session
        .create_entity(
            Actor::Agent,
            "logic_relay",
            Some(DVec3::X * 64.0),
            BTreeMap::from([
                ("targetname".into(), "entrance_trigger".into()),
                ("target".into(), "courtyard_relay".into()),
                ("parentname".into(), "courtyard_relay,hinge".into()),
                ("notes".into(), "courtyard_relay".into()),
            ]),
        )
        .unwrap()
    else {
        panic!()
    };
    session
        .set_entity_connection(
            Actor::Agent,
            source,
            None,
            &map_core::connections::Spec {
                output: "OnTrigger".into(),
                target: "courtyard*".into(),
                input: "Trigger".into(),
                parameter: "".into(),
                delay: 0.25,
                times: -1,
            },
        )
        .unwrap();
    let original = session.document().vmf().clone();
    session.begin_entity_rename(target).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let window = RenameWindow::new().unwrap();
    rename_ui::bind(&window, &shared);
    rename_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    window.set_new_name("garden_relay".into());
    window.invoke_fields_changed();
    assert!(window.get_requires_review());
    assert!(!window.get_can_apply());
    assert_eq!(window.get_changes().row_count(), 2);
    assert_eq!(window.get_attention().row_count(), 2);
    assert!(!window.get_show_details());
    for row in window.get_references().iter() {
        assert!(!row.label.contains("entity:"));
        assert!(!row.label.contains("logic_relay"));
    }
    let parent = window
        .get_changes()
        .iter()
        .find(|row| row.label.ends_with(" · Parent"))
        .unwrap();
    assert!(
        parent.label.contains("Entrance trigger · Parent"),
        "{}",
        parent.label
    );
    assert!(parent.reason.contains("Attachment “hinge” stays the same"));
    let wildcard = window
        .get_attention()
        .iter()
        .find(|row| row.change.contains("courtyard*"))
        .unwrap();
    assert!(wildcard.label.contains("When triggered · recipient"));
    assert!(wildcard.reason.contains("will no longer include"));
    window.set_show_details(true);
    window.invoke_fields_changed();
    assert!(lock(&shared).rename_draft().unwrap().show_details);
    assert!(
        window
            .get_changes()
            .row_data(0)
            .unwrap()
            .details
            .contains("logic_relay")
    );
    assert!(!window.get_can_apply());
    window.set_show_details(false);
    window.invoke_fields_changed();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.set_accept_review(true);
    window.invoke_fields_changed();
    assert!(window.get_can_apply());
    save_ui_snapshot(window.window(), "entity-rename");
    assert_dialog_button_visible(&window, "New entity name");
    assert_dialog_button_visible(&window, "Current entity name");
    let name_field = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &window,
        "New entity name",
    )
    .next()
    .unwrap();
    let summary =
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Rename summary")
            .next()
            .unwrap();
    assert!(
        name_field.absolute_position().y + name_field.size().height
            <= summary.absolute_position().y,
        "Name field overlaps the rename summary"
    );
    assert_dialog_button_visible(&window, "Cancel");
    assert_dialog_button_visible(&window, "Apply rename");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Apply rename")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(lock(&shared).rename_draft().is_none());
    assert_eq!(
        lock(&shared)
            .document()
            .entity(source)
            .unwrap()
            .properties
            .get("parentname"),
        Some("garden_relay,hinge")
    );
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    lock(&shared).begin_entity_rename(target).unwrap();
    rename_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    window.set_new_name("another_name".into());
    window.invoke_fields_changed();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Cancel")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(lock(&shared).rename_draft().is_none());
    assert_eq!(lock(&shared).document().vmf(), &original);
    lock(&shared).begin_entity_rename(target).unwrap();
    rename_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    lock(&shared).begin_entity_rename(source).unwrap();
    window.invoke_apply();
    assert!(window.get_message().contains("draft changed"));
    assert_eq!(lock(&shared).rename_draft().unwrap().entity, source);
    rename_ui::refresh(&window, &lock(&shared));
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
    assert!(lock(&shared).rename_draft().is_none());
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &original);
}

#[test]
pub(crate) fn management_controls_stage_cancel_apply_and_report_without_desktop_input() {
    use slint::Model;
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("test assets".into()));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(
            r#"
@PointClass = light [
    _light(color255) : "Light color" : "255 255 255 200"
    style(choices) : "Appearance" : 0 = [ 0 : "Normal" 1 : "Flicker" ]
    spawnflags(flags) = [ 1 : "Initially dark" : 0 ]
]
"#,
        )
        .unwrap(),
    );
    let mut ids = Vec::new();
    for (i, value) in ["255 190 100 300", "100 170 255 600"].iter().enumerate() {
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::Agent,
                "light",
                Some(DVec3::new(i as f64 * 100.0, 0.0, 64.0)),
                BTreeMap::from([("_light".into(), value.to_string())]),
            )
            .unwrap()
        else {
            panic!()
        };
        ids.push(id);
    }
    let before = session.document().vmf().clone();
    session
        .begin_property_edit(session::PropertyTarget::Entities(ids.clone()))
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let window = PropertiesWindow::new().unwrap();
    management_ui::bind_properties(&window, &shared).unwrap();
    management_ui::refresh_properties(&window, &lock(&shared));
    window.show().unwrap();
    window.invoke_component_edit("_light".into(), 3, "450".into());
    assert!(window.get_can_apply());
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert!(
        window
            .get_fields()
            .iter()
            .any(|row| row.status.contains("Mixed"))
    );
    save_ui_snapshot(window.window(), "batch-properties");
    for label in ["Cancel", "Apply properties"] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_cancel();
    assert!(lock(&shared).property_draft().is_none());
    assert_eq!(lock(&shared).document().vmf(), &before);
    lock(&shared)
        .begin_property_edit(session::PropertyTarget::Entities(ids.clone()))
        .unwrap();
    management_ui::refresh_properties(&window, &lock(&shared));
    window.invoke_edit("style".into(), "1".into());
    window.invoke_apply();
    assert_eq!(
        lock(&shared)
            .document()
            .entity(ids[1])
            .unwrap()
            .properties
            .get("style"),
        Some("1")
    );
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    lock(&shared)
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    management_ui::refresh_properties(&window, &lock(&shared));
    let old = window.get_draft_token();
    lock(&shared)
        .begin_property_edit(session::PropertyTarget::Entities(ids.clone()))
        .unwrap();
    window.invoke_edit("skyname".into(), "sky_bad".into());
    assert!(window.get_message().contains("replaced"));
    window.set_draft_token(old);
    window.invoke_cancel();
    assert!(lock(&shared).property_draft().is_some());
    let report = ReportWindow::new().unwrap();
    management_ui::bind_report(&report, &shared);
    report.set_classname("light".into());
    report.invoke_search();
    assert_eq!(report.get_rows().row_count(), 2);
    report.set_limit_bounds(true);
    report.set_min_x("80".into());
    report.set_max_x("120".into());
    report.invoke_search();
    assert_eq!(report.get_rows().row_count(), 1);
    report.invoke_select_all(false);
    assert_eq!(lock(&shared).selected_ids(), [ObjectId::Entity(ids[1])]);
    report.set_min_x("oops".into());
    report.invoke_search();
    assert_eq!(report.get_rows().row_count(), 0);
    report.set_limit_bounds(false);
    report.invoke_search();
    report.invoke_select_all(true);
    assert_eq!(lock(&shared).selected_ids().len(), 2);
    report.invoke_visibility_action("hide".into());
    report.set_visibility(2);
    report.invoke_search();
    assert_eq!(report.get_rows().row_count(), 2);
    report.show().unwrap();
    save_ui_snapshot(report.window(), "object-report");
    for label in [
        "Close",
        "Show all",
        "Restore visibility",
        "Select all matches",
    ] {
        assert_dialog_button_visible(&report, label);
    }
    report.invoke_visibility_action("restore".into());
    assert_eq!(report.get_rows().row_count(), 0);
    report.set_visibility(0);
    report.invoke_search();
    let id = report.get_rows().row_data(0).unwrap().id;
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    report.invoke_select_result(id, false);
    assert!(report.get_message().contains("Map changed"));
    assert!(lock(&shared).selected_ids().is_empty());
}

#[test]
pub(crate) fn statistics_controls_refresh_page_and_close_without_editing() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    for index in 0..54 {
        let min = DVec3::X * index as f64 * 96.0;
        state
            .create_block(
                Actor::Agent,
                min,
                min + DVec3::splat(64.0),
                &format!("brick/example_{index:02}"),
                false,
            )
            .unwrap();
    }
    let before = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(state));
    let window = StatisticsWindow::new().unwrap();
    statistics_ui::bind(&window, &shared);
    window.invoke_refresh();
    assert_eq!(window.get_brushes(), "54");
    assert_eq!(window.get_faces(), "324");
    assert_eq!(window.get_rows().row_count(), 50);
    assert!(window.get_has_next());
    window.set_page(1);
    window.invoke_refresh();
    assert_eq!(window.get_rows().row_count(), 4);
    assert!(!window.get_has_next());
    window.set_page(0);
    window.invoke_refresh();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "map-statistics");
    for label in ["Refresh", "Close", "Next"] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    window.set_section(1);
    window.invoke_refresh();
    assert_eq!(window.get_rows().row_count(), 0);
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    statistics_ui::refresh_changed(&window, &lock(&shared));
    assert_eq!(window.get_brushes(), "0");
}

#[test]
pub(crate) fn pointfile_controls_load_navigate_unload_and_close_without_map_edits() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("courtyard.lin");
    std::fs::write(&path, "0 0 64\n128 0 64\n128 128 64\n256 128 128\n").unwrap();
    let state = Session::new(Err("no assets needed".into()));
    let before = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = PointfileWindow::new().unwrap();
    pointfile_ui::bind(&window, &shared);
    pointfile_ui::refresh(&window, &lock(&shared));
    assert!(!window.get_current());
    window.set_path(path.display().to_string().into());
    window.invoke_load();
    assert_eq!(window.get_total(), 4);
    assert!(window.get_current());
    window.invoke_navigate(1);
    assert_eq!(window.get_point(), 2);
    assert_eq!(lock(&shared).camera().position, Vec3::new(128.0, 0.0, 64.0));
    window.show().unwrap();
    save_ui_snapshot(window.window(), "pointfile");
    for label in ["Load", "Go", "Frame path", "Unload path", "Close"] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_frame();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert!(lock(&shared).pointfile_points().is_some());
    window.invoke_clear();
    assert!(!window.get_has_path());
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
}

#[test]
pub(crate) fn component_controls_select_preview_reject_apply_and_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let id = state.component_solids()[0];
    let shape = state.component_topology(&[id]).unwrap().remove(0);
    let edge = shape
        .edges
        .iter()
        .find(|edge| {
            edge.vertices
                .iter()
                .all(|index| shape.vertices[*index].position.z == 64.0)
        })
        .unwrap();
    let key = format!(
        "{}:{}",
        id,
        edge.sides
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    components_ui::refresh(&window, &lock(&shared));
    window.invoke_toggle_picking();
    assert!(window.get_picking());
    assert_eq!(lock(&shared).component_solids(), vec![id]);
    window.set_kind(1);
    window.invoke_change_kind();
    assert_eq!(
        lock(&shared).component_picking().unwrap().kind,
        session::component_picking::Kind::Vertex
    );
    window.set_kind(0);
    window.invoke_change_kind();
    window.invoke_choose(key.into());
    assert!(lock(&shared).component_picking().is_some());
    assert_eq!(lock(&shared).component_selection().len(), 1);
    window.set_move_z("16".into());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(lock(&shared).document().vmf(), &original);
    let valid = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-components");
    assert_dialog_button_visible(&window, "Stop viewport picking");
    for label in [
        "Cancel",
        "Apply move",
        "Preview move",
        "Select all shown",
        "Clear selection",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_move_z("-64".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(!window.get_message().is_empty());
    assert_eq!(lock(&shared).render_document().vmf(), &valid);
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.set_move_z("16".into());
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &valid);
    window.invoke_preview_edit();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &valid);
    assert_eq!(lock(&shared).render_document().vmf(), &valid);
    assert!(lock(&shared).component_selection().is_empty());
    window.set_kind(2);
    window.invoke_refresh();
    let bottom = window
        .get_rows()
        .iter()
        .find(|row| row.label.starts_with("Bottom"))
        .unwrap();
    window.invoke_choose(bottom.key);
    window.set_move_z("-16".into());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(
        lock(&shared)
            .component_selection()
            .iter()
            .next()
            .unwrap()
            .sides
            .len(),
        1
    );
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-faces");
    window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &valid);
    lock(&shared).undo(Actor::Agent).unwrap();
    window.invoke_refresh();
    let top = window
        .get_rows()
        .iter()
        .find(|row| row.label.starts_with("Top"))
        .unwrap();
    window.invoke_choose(top.key);
    window.set_move_z("16".into());
    window.set_texture_lock(true);
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(
        lock(&shared).component_report(&[]).unwrap()["texture_mode"],
        "attached"
    );
    window.show().unwrap();
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    save_ui_snapshot(window.window(), "brush-texture-attachment");
    window.invoke_apply();
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_cancel();
}

#[test]
pub(crate) fn partition_texture_controls_share_preview_inheritance_keep_cancel_and_undo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_assets, game) = crate::test_assets::game();
    let mut state = Session::new(Ok(game));
    let ObjectId::Solid(id) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let side = state
        .document()
        .solid(id)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    state
        .select_components(
            &[map_core::subobject::Component {
                solid: id,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &state.component_stamp(),
        )
        .unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(2);
    window.set_operation(6);
    components_ui::refresh(&window, &lock(&shared));
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.set_show_top_texture(true);
    window.set_texture_shift_u("13.125".into());
    window.set_texture_shift_v("-27.5".into());
    window.set_texture_scale_u("-0.375".into());
    window.set_texture_scale_v("0.625".into());
    window.set_texture_rotation("37".into());
    window.invoke_edited();
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_message().contains("texture placement"));
    window.invoke_preview_partition_texture();
    assert!(window.get_ready());
    assert_eq!(window.get_texture_shift_u(), "13.125");
    window.set_texture_scale_u("0".into());
    window.invoke_preview_partition_texture();
    assert!(window.get_message().contains("nonzero scales"));
    window.set_texture_scale_u("-0.375".into());
    window.invoke_preview_partition_texture();
    assert!(window.get_ready());
    let reviewed = lock(&shared).partition_preview_token();
    let cut_inputs = (window.get_cut_first(), window.get_cut_second());
    window.set_cut_first("25".into());
    window.set_cut_second("75".into());
    window.set_texture_justify_u(3);
    for fit in [true, false] {
        if fit {
            window.invoke_fit_partition_texture();
        } else {
            window.invoke_justify_partition_texture();
        }
        assert!(window.get_message().contains("Preview the dividing edge"));
        assert_eq!(lock(&shared).partition_preview_token(), reviewed);
        assert_eq!(window.get_cut_first(), "25");
        assert_eq!(window.get_cut_second(), "75");
    }
    window.set_cut_first(cut_inputs.0);
    window.set_cut_second(cut_inputs.1);
    let refs = components_ui::partition_material_target(&lock(&shared)).2;
    let displayed_scope = window.get_top_material_scope();
    let displayed_shift = window.get_texture_shift_u();
    lock(&shared)
        .select_component_partitions(reviewed.unwrap(), &refs[1..], SelectMode::Replace)
        .unwrap();
    for fit in [true, false] {
        if fit {
            window.invoke_fit_partition_texture();
        } else {
            window.invoke_justify_partition_texture();
        }
        assert!(window.get_message().contains("highlighted pieces changed"));
        assert_eq!(lock(&shared).partition_preview_token(), reviewed);
        assert_eq!(window.get_top_material_scope(), displayed_scope);
        assert_eq!(window.get_texture_shift_u(), displayed_shift);
    }
    lock(&shared)
        .select_component_partitions(reviewed.unwrap(), &refs, SelectMode::Replace)
        .unwrap();
    components_ui::refresh(&window, &lock(&shared));
    let reviewed = lock(&shared).partition_preview_token();
    window.set_texture_rotation("42".into());
    window.invoke_fit_partition_texture();
    assert!(window.get_message().contains("Preview texture placement"));
    assert_eq!(lock(&shared).partition_preview_token(), reviewed);
    window.set_texture_rotation("37".into());
    window.invoke_fit_partition_texture();
    assert_ne!(lock(&shared).partition_preview_token(), reviewed);
    assert!(window.get_message().starts_with("Top textures fitted."));
    assert!(window.get_ready());
    assert_ne!(window.get_texture_scale_u(), "-0.375");
    assert!(window.get_texture_scale_u().parse::<f64>().unwrap() < 0.0);
    assert_eq!(window.get_texture_rotation(), "37");
    window.set_texture_shift_u("13.125".into());
    window.set_texture_shift_v("-27.5".into());
    window.set_texture_scale_u("-0.375".into());
    window.set_texture_scale_v("0.625".into());
    window.invoke_preview_partition_texture();
    assert!(
        window
            .get_message()
            .starts_with("Top texture placement updated.")
    );
    let reviewed = lock(&shared).partition_preview_token();
    let scales = (window.get_texture_scale_u(), window.get_texture_scale_v());
    window.set_texture_justify_u(3);
    window.set_texture_justify_v(2);
    window.invoke_justify_partition_texture();
    assert_ne!(lock(&shared).partition_preview_token(), reviewed);
    assert!(window.get_message().starts_with("Top textures justified."));
    assert!(window.get_ready());
    assert_eq!(
        (window.get_texture_scale_u(), window.get_texture_scale_v()),
        scales
    );
    assert_eq!(window.get_texture_rotation(), "37");
    window.show().unwrap();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1100, 1100));
    save_ui_snapshot(window.window(), "partition-texture-placement");
    assert_dialog_button_visible(&window, "Cancel");
    assert_dialog_button_visible(&window, "Fit each piece");
    assert_dialog_button_visible(&window, "Preview justification");
    window.invoke_inherit_partition_texture();
    assert!(window.get_top_texture_summary().contains("inherited"));
    window.invoke_choose_texture_rotation(90.0);
    assert!(!window.get_ready());
    window.invoke_preview_partition_texture();
    window.invoke_apply();
    let kept = lock(&shared).document().vmf().clone();
    assert_ne!(kept, original);
    window.invoke_resume_saved_partition();
    assert_eq!(window.get_texture_rotation(), "90");
    window.set_texture_rotation("-45".into());
    window.invoke_preview_partition_texture();
    window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &kept);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
}

#[test]
pub(crate) fn flat_partition_controls_stage_highlight_frame_raise_flatten_apply_and_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(if std::env::var_os("HAMMERR_UI_SNAPSHOTS").is_some() {
        editor_core::startup::mount_game(None)
    } else {
        Err("no assets needed".into())
    });
    let ObjectId::Solid(id) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let side = state
        .document()
        .solid(id)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    state
        .set_component_picking(
            Some(session::component_picking::Kind::Face),
            &[id],
            &state.component_stamp(),
        )
        .unwrap();
    state
        .select_components(
            &[map_core::subobject::Component {
                solid: id,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &state.component_stamp(),
        )
        .unwrap();
    let source = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    components_ui::refresh(&window, &lock(&shared));
    window.set_operation(6);
    window.set_kind(2);
    window.show().unwrap();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1100, 900));
    assert_dialog_button_visible(&window, "Stage flat partition");
    window.invoke_preview_edit();
    save_ui_snapshot(window.window(), "flat-face-partition");
    assert_eq!(window.get_operation(), 6);
    assert!(window.get_has_partition_preview() && window.get_partition_flat());
    assert!(window.get_ready());
    assert_eq!(window.get_partition_rows().row_count(), 2);
    window.set_show_top_material(true);
    window.set_top_material("brick/brickwall003a".into());
    window.invoke_edited();
    assert!(!window.get_ready());
    components_ui::refresh(&window, &lock(&shared));
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_message().contains("planned top material"));
    assert!(!window.get_ready());
    window.invoke_preview_partition_material();
    assert!(window.get_ready());
    assert_eq!(window.get_top_material(), "brick/brickwall003a");
    assert!(
        window
            .get_partition_rows()
            .iter()
            .all(|row| row.detail.contains("planned"))
    );
    let picker_target = {
        let state = lock(&shared);
        components_ui::partition_material_target(&state)
    };
    let row = window.get_partition_rows().row_data(0).unwrap();
    window.invoke_choose_partition(row.key.clone());
    assert!(
        components_ui::choose_partition_material(
            &window,
            &mut lock(&shared),
            &picker_target,
            Some("wood/stale")
        )
        .is_err()
    );
    window.invoke_choose_partition(row.key);
    for label in [
        "Preview material",
        "Restore inheritance",
        "Keep flat pieces",
        "Cancel",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.window().set_size(slint::PhysicalSize::new(880, 540));
    assert_dialog_button_visible(&window, "Keep flat pieces");
    assert_dialog_button_visible(&window, "Cancel");
    window
        .window()
        .set_size(slint::PhysicalSize::new(1100, 900));
    save_ui_snapshot(window.window(), "partition-top-material");
    window.invoke_inherit_partition_material();
    assert!(window.get_ready());
    assert_eq!(window.get_top_material(), "brick/test");
    window.set_show_top_material(false);

    assert!(
        window
            .get_preview_summary()
            .contains("2 flat pieces staged")
    );
    for label in [
        "Frame highlighted pieces",
        "Partition ridge height",
        "Keep roof texture at corners",
        "Preview ridge",
        "Return to flat",
        "Keep flat pieces",
        "Discard preview",
        "Cancel",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    assert_eq!(lock(&shared).document().vmf(), &source);
    let first = window.get_partition_rows().row_data(0).unwrap();
    window.invoke_choose_partition(first.key);
    assert!(!window.get_partition_rows().row_data(0).unwrap().selected);
    window.invoke_frame_partitions();
    assert!(window.get_message().is_empty());
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    window.set_show_cut_settings(true);
    window.invoke_choose_cut_position(0, 20.0);
    window.invoke_choose_cut_position(1, 80.0);
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_message().contains("Preview the cut positions"));
    window.invoke_preview_cut();
    assert!(window.get_ready());
    assert_eq!(
        lock(&shared).component_report(&[]).unwrap()["preview"]["partition"]["edge_parameters"],
        serde_json::json!([0.2, 0.8])
    );
    save_ui_snapshot(window.window(), "adjusted-flat-partition");
    for label in [
        "Partition edge A position",
        "Partition edge B position",
        "Preview cut",
        "Keep flat pieces",
        "Cancel",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    let cut_token = lock(&shared).partition_preview_token();
    window.invoke_choose_cut_position(0, 0.0);
    window.invoke_preview_cut();
    assert!(!window.get_ready());
    assert_eq!(lock(&shared).partition_preview_token(), cut_token);
    window.invoke_choose_cut_position(0, 13.7);
    window.invoke_choose_cut_position(1, 68.3);
    window.invoke_preview_cut();
    assert!(window.get_ready());
    assert!((window.get_cut_first_value() - 13.7).abs() < 0.001);
    assert!((window.get_cut_second_value() - 68.3).abs() < 0.001);
    assert_dialog_button_visible(&window, "Reset cut to midpoint");
    window.invoke_choose_rise(16.0);
    window.invoke_preview_edit();
    assert!(window.get_message().contains("nonplanar"));
    window.invoke_choose_cut_position(1, 13.7);
    window.invoke_preview_cut();
    window.set_show_cut_settings(false);
    window.set_roof_texture_lock(true);
    window.invoke_choose_rise(16.125);
    window.invoke_preview_edit();
    assert!(window.get_ready() && !window.get_partition_flat());
    let report = lock(&shared).component_report(&[id]).unwrap();
    assert!(
        (report["preview"]["partition"]["edge_parameters"][0]
            .as_f64()
            .unwrap()
            - 0.137)
            .abs()
            < 1e-10
    );
    assert!(!window.get_cut_is_midpoint());
    assert_eq!(window.get_cut_first(), "13.7");
    assert_eq!(window.get_cut_second(), "13.7");
    window.set_show_cut_settings(false);
    save_ui_snapshot(window.window(), "adjusted-partition-ridge");
    window.invoke_flatten_partitions();
    assert!(window.get_ready() && window.get_partition_flat());

    window.invoke_choose_cut_position(0, 50.0);
    window.invoke_choose_cut_position(1, 50.0);
    window.invoke_preview_cut();
    assert!(window.get_ready());
    window.set_show_cut_settings(false);
    window.set_roof_texture_lock(true);
    window.invoke_choose_rise(16.125);
    window.invoke_preview_edit();
    save_ui_snapshot(window.window(), "raised-face-partition");
    assert!(window.get_ready() && !window.get_partition_flat());
    assert_eq!(
        lock(&shared).component_report(&[id]).unwrap()["texture_mode"],
        "roof_attached"
    );
    let raised = lock(&shared).render_document().vmf().clone();
    window.invoke_flatten_partitions();
    assert!(window.get_partition_flat() && window.get_ready());
    assert_eq!(lock(&shared).render_document().vmf(), &source);
    assert!(!window.get_partition_rows().row_data(0).unwrap().selected);
    window.set_split_rise("0.01".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(window.get_message().contains("0.125"));
    assert!(window.get_has_partition_preview());
    window.invoke_choose_rise(16.125);
    window.invoke_preview_edit();
    assert_eq!(lock(&shared).render_document().vmf(), &raised);
    window.invoke_discard_preview();
    assert!(!window.get_has_partition_preview());
    assert_eq!(lock(&shared).document().vmf(), &source);
    window.invoke_preview_edit();
    window.set_roof_texture_lock(true);
    window.invoke_choose_rise(16.125);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &raised);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    components_ui::refresh(&window, &lock(&shared));
    window.set_kind(2);
    window.invoke_change_kind();
    window.set_operation(6);
    let top = window
        .get_rows()
        .iter()
        .find(|row| row.key == format!("{id}:{side}"))
        .unwrap();
    window.invoke_choose(top.key);
    window.invoke_preview_edit();
    assert!(window.get_has_partition_preview());
    window.invoke_apply();
    assert!(window.get_message().contains("Kept 1 flat partitions"));
    assert!(!window.get_has_partition_preview());
    assert_eq!(window.get_saved_partitions().row_count(), 1);
    let stored = lock(&shared).document().vmf().clone();
    assert_eq!(stored.world, source.world);
    save_ui_snapshot(window.window(), "saved-face-partitions");
    assert_dialog_button_visible(&window, "Resume pieces");
    assert_dialog_button_visible(&window, "Remove saved");
    window.invoke_resume_saved_partition();
    assert!(window.get_has_partition_preview() && window.get_partition_flat());
    assert_eq!(lock(&shared).document().vmf(), &stored);
    window.set_partition_mode(1);
    window.invoke_edited();
    window.set_extrusion_texture_lock(true);
    window.invoke_choose_extrusion_height(32.125);
    let target = {
        let session = lock(&shared);
        (
            session.partition_preview_token(),
            session.document_generation(),
        )
    };
    components_ui::choose_extrusion_material(&window, &mut lock(&shared), target, "concrete/test")
        .unwrap();
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_ready() && window.get_has_extrusion_preview());
    save_ui_snapshot(window.window(), "partition-extrusion-controls");
    for label in [
        "Extrusion depth",
        "Keep top texture at corners",
        "Choose wall material",
        "Browse…",
        "Preview extrusion",
        "Apply extrusion",
        "Cancel",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.window().set_size(slint::PhysicalSize::new(880, 720));
    save_ui_snapshot(window.window(), "partition-extrusion-compact");
    assert_dialog_button_visible(&window, "Apply extrusion");
    assert_dialog_button_visible(&window, "Cancel");
    window
        .window()
        .set_size(slint::PhysicalSize::new(1100, 900));
    assert_eq!(lock(&shared).document().vmf(), &stored);
    assert!(
        components_ui::choose_extrusion_material(&window, &mut lock(&shared), target, "brick/test")
            .is_err()
    );
    let token = lock(&shared).partition_preview_token().unwrap();
    window.invoke_choose_partition(window.get_partition_rows().row_data(0).unwrap().key);
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_ne!(lock(&shared).partition_preview_token(), Some(token));
    window.invoke_apply();
    assert!(window.get_message().contains("Added 1 volumes"));
    assert_eq!(lock(&shared).selected_faces().len(), 6);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &stored);
    components_ui::refresh(&window, &lock(&shared));
    window.invoke_resume_saved_partition();
    window.set_partition_mode(1);
    window.set_extrusion_direction(1);
    window.invoke_choose_extrusion_height(16.125);
    window.invoke_preview_edit();
    assert!(window.get_ready() && window.get_has_extrusion_preview());
    assert_eq!(window.get_extrusion_height(), "16.125");
    assert_eq!(window.get_extrusion_direction(), 1);
    assert!(
        window
            .get_preview_summary()
            .contains("Cuts 16.125 units inward")
    );
    save_ui_snapshot(window.window(), "partition-inward-controls");
    assert_dialog_button_visible(&window, "Extrusion direction");
    assert_dialog_button_visible(&window, "Keep floor texture at corners");
    let inward = lock(&shared).render_document().vmf().clone();
    window.invoke_choose_extrusion_height(128.0);
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(window.get_message().contains("entire source"));
    assert_eq!(lock(&shared).render_document().vmf(), &inward);
    window.invoke_choose_extrusion_height(16.125);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.set_extrusion_direction(0);
    window.invoke_edited();
    assert!(!window.get_ready());
    window.set_extrusion_direction(1);
    window.invoke_edited();
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(lock(&shared).render_document().vmf(), &inward);
    window.invoke_apply();
    assert!(window.get_message().contains("convex fragments"));
    assert_eq!(lock(&shared).document().vmf(), &inward);
    assert_eq!(window.get_saved_partitions().row_count(), 0);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &stored);
    components_ui::refresh(&window, &lock(&shared));
    window.set_extrusion_direction(0);
    window.invoke_resume_saved_partition();
    window.set_partition_mode(1);
    window.invoke_preview_edit();
    assert_eq!(lock(&shared).render_document().vmf().world.solids.len(), 3);
    {
        let session = lock(&shared);
        assert_eq!(
            session.render_document().solid(id),
            session.document().solid(id)
        );
    }
    window.invoke_flatten_partitions();
    assert!(!window.get_has_extrusion_preview());
    assert_eq!(lock(&shared).render_document().vmf(), &stored);
    window.invoke_discard_preview();
    window.invoke_remove_saved_partition();
    assert_eq!(lock(&shared).document().vmf(), &source);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &stored);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    components_ui::refresh(&window, &lock(&shared));
    window.set_kind(2);
    window.invoke_change_kind();
    window.set_operation(6);
    let top = window
        .get_rows()
        .iter()
        .find(|row| row.key == format!("{id}:{side}"))
        .unwrap();
    window.invoke_choose(top.key);
    window.set_operation(5);
    window.invoke_refresh();
    assert_eq!(
        window
            .get_crease_rows()
            .row_data(0)
            .unwrap()
            .choices
            .row_count(),
        2
    );
    let agent_token = {
        let mut session = lock(&shared);
        let choices = session.component_partition_choices().unwrap();
        let stamp = session.component_stamp();
        session
            .preview_component_partition(
                &[crate::session::components::CreaseEdges {
                    solid: id,
                    edges: choices[0].1[3].edges,
                }],
                &stamp,
            )
            .unwrap()
    };
    window.invoke_refresh();
    assert_eq!(window.get_operation(), 6);
    assert_eq!(
        window
            .get_crease_rows()
            .row_data(0)
            .unwrap()
            .choices
            .row_count(),
        6
    );
    assert_eq!(window.get_crease_rows().row_data(0).unwrap().choice, 3);
    lock(&shared).cancel_component_move(agent_token).unwrap();
    window.invoke_refresh();
    let row = window.get_crease_rows().row_data(0).unwrap();
    assert_eq!(row.choices.row_count(), 6);
    assert!(row.choices.row_data(2).unwrap().starts_with("Trim corner"));
    window.invoke_choose_crease_pair(row.key, 2);
    window.invoke_preview_edit();
    assert!(window.get_has_partition_preview() && window.get_ready());
    assert!(!window.get_can_raise_partition());
    assert!(
        window
            .get_partition_rows()
            .row_data(0)
            .unwrap()
            .label
            .contains("triangle")
    );
    assert!(
        window
            .get_partition_rows()
            .row_data(1)
            .unwrap()
            .label
            .contains("pentagon")
    );
    window.set_show_cut_settings(true);
    window.invoke_choose_cut_position(0, 20.0);
    window.invoke_choose_cut_position(1, 80.0);
    window.invoke_preview_cut();
    assert!(window.get_ready());
    save_ui_snapshot(window.window(), "boundary-partition-controls");
    window.invoke_apply();
    assert!(!window.get_has_partition_preview());
    let boundary_stored = lock(&shared).document().vmf().clone();
    window.invoke_resume_saved_partition();
    assert!(window.get_ready() && window.get_partition_flat());
    assert!(!window.get_can_raise_partition());
    window.invoke_preview_edit();
    assert!(window.get_ready() && window.get_partition_flat());
    window.set_partition_mode(1);
    window.invoke_choose_extrusion_height(16.125);
    window.invoke_preview_edit();
    assert!(window.get_ready() && window.get_has_extrusion_preview());
    window.set_show_cut_settings(false);
    save_ui_snapshot(window.window(), "boundary-partition-extrusion");
    window.invoke_cancel();
    assert_eq!(lock(&shared).partition_preview_token(), None);
    assert_eq!(lock(&shared).document().vmf(), &boundary_stored);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
}

#[test]
pub(crate) fn polygon_partition_controls_offer_complete_choices_keep_resume_and_cancel_extrusion() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    for corners in [3, 5, 6, 8] {
        let mut state = Session::new(Err("no assets needed".into()));
        let ids = state
            .create_primitive(
                Actor::Agent,
                map_core::primitive::Spec {
                    shape: map_core::primitive::Shape::Cylinder,
                    segments: corners,
                    mins: DVec3::ZERO,
                    maxs: DVec3::splat(64.0),
                    material: "brick/test".into(),
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        let ObjectId::Solid(id) = ids[0] else {
            unreachable!()
        };
        let side = state
            .document()
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        state
            .set_component_picking(
                Some(session::component_picking::Kind::Face),
                &[id],
                &state.component_stamp(),
            )
            .unwrap();
        state
            .select_components(
                &[map_core::subobject::Component {
                    solid: id,
                    sides: vec![side],
                }],
                SelectMode::Replace,
                &state.component_stamp(),
            )
            .unwrap();
        let source = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(2);
        window.set_operation(6);
        components_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        window
            .window()
            .set_size(slint::PhysicalSize::new(1100, 900));
        let row = window.get_crease_rows().row_data(0).unwrap();
        assert_eq!(
            row.choices.row_count(),
            (corners * (corners - 1) / 2) as usize
        );
        assert!(
            row.choices
                .iter()
                .any(|label| label.starts_with("Trim corner"))
        );
        let choice = if corners == 3 {
            0
        } else {
            row.choices
                .iter()
                .position(|label| label.starts_with("Cut from"))
                .unwrap() as i32
        };
        window.set_operation(5);
        window.invoke_refresh();
        assert_eq!(window.get_crease_rows().row_count(), 0);
        let agent_token = {
            let mut state = lock(&shared);
            let pair = state.component_partition_choices().unwrap()[0].1[choice as usize].edges;
            let stamp = state.component_stamp();
            state
                .preview_component_partition(
                    &[session::components::CreaseEdges {
                        solid: id,
                        edges: pair,
                    }],
                    &stamp,
                )
                .unwrap()
        };
        window.invoke_refresh();
        assert_eq!(window.get_operation(), 6);
        assert_eq!(
            window
                .get_crease_rows()
                .row_data(0)
                .unwrap()
                .choices
                .row_count(),
            (corners * (corners - 1) / 2) as usize
        );
        assert_eq!(window.get_crease_rows().row_data(0).unwrap().choice, choice);
        lock(&shared).cancel_component_move(agent_token).unwrap();
        window.invoke_refresh();
        window.invoke_choose_crease_pair(row.key, choice);
        window.invoke_preview_edit();
        assert!(window.get_ready() && window.get_partition_flat());
        assert!(!window.get_can_raise_partition());
        assert_eq!(window.get_partition_rows().row_count(), 2);
        assert!(
            window
                .get_preview_summary()
                .contains("extrude highlighted pieces")
        );
        assert!(!window.get_preview_summary().contains("shared ridge"));
        let edge_labels: Vec<_> = window
            .get_rows()
            .iter()
            .filter(|row| row.label.contains("Edge midpoint"))
            .map(|row| row.label.to_string())
            .collect();
        assert_eq!(
            edge_labels,
            [
                format!("Brush {id} · Edge midpoint 1"),
                format!("Brush {id} · Edge midpoint 2"),
            ]
        );
        window.set_show_cut_settings(true);
        window.invoke_choose_cut_position(0, 20.0);
        window.invoke_choose_cut_position(1, 80.0);
        window.invoke_preview_cut();
        assert!(window.get_ready());
        window.set_show_top_material(true);
        window.set_top_material("brick/polygon".into());
        window.invoke_edited();
        window.invoke_preview_partition_material();
        assert!(window.get_ready());
        window.invoke_frame_partitions();
        assert!(window.get_message().is_empty());
        assert_dialog_button_visible(&window, "Keep flat pieces");
        assert_dialog_button_visible(&window, "Cancel");
        save_ui_snapshot(
            window.window(),
            &format!("polygon-{corners}-partition-controls"),
        );
        window.invoke_apply();
        assert!(window.get_message().contains("Kept 1 flat partitions"));
        let stored = lock(&shared).document().vmf().clone();
        assert_eq!(stored.world, source.world);
        window.invoke_resume_saved_partition();
        assert!(window.get_ready() && window.get_partition_flat());
        assert!(!window.get_can_raise_partition());
        assert!(
            window
                .get_partition_rows()
                .iter()
                .all(|row| row.detail.contains("brick/polygon"))
        );
        window.set_partition_mode(1);
        window.invoke_choose_extrusion_height(16.125);
        window.invoke_preview_edit();
        assert!(window.get_ready() && window.get_has_extrusion_preview());
        assert_dialog_button_visible(&window, "Apply extrusion");
        assert_dialog_button_visible(&window, "Cancel");
        let draft = lock(&shared).component_report(&[]).unwrap();
        assert_eq!(
            draft["preview"]["extrusion"]["pieces"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        window.invoke_cancel();
        assert_eq!(lock(&shared).partition_preview_token(), None);
        assert_eq!(lock(&shared).document().vmf(), &stored);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
    }
}

#[test]
pub(crate) fn component_source_controls_page_large_owners_and_edit_one_child() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for index in 0..105 {
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::new(index as f64 * 96.0, 0.0, 0.0),
                DVec3::new(index as f64 * 96.0 + 64.0, 64.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        ids.push(id);
    }
    let owner = state
        .edit(Actor::Agent, "owner", |tx| {
            map_core::edit::tie_to_entity(tx, &ids, "func_detail", &[])
        })
        .unwrap();
    state.select(&[owner], SelectMode::Replace).unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(2);
    components_ui::refresh(&window, &lock(&shared));
    assert_eq!(window.get_source_total(), 105);
    assert!(window.get_rows().row_count() == 0);
    assert!(!window.get_source_all());
    assert!(window.get_summary().contains("Choose a brush"));
    window.invoke_page_sources(1);
    assert_eq!(window.get_source_ids().row_count(), 5);
    assert_eq!(window.get_source_summary(), "101–105 of 105");
    window.set_source_index(1);
    window.invoke_choose_source();
    assert!(window.get_picking());
    assert_eq!(window.get_rows().row_count(), 6);
    assert_eq!(lock(&shared).component_solids(), vec![ids[100]]);
    assert_eq!(lock(&shared).selected_ids(), vec![owner]);
    window.show().unwrap();
    save_ui_snapshot(window.window(), "component-brush-scope");
    for label in [
        "Previous brushes",
        "Next brushes",
        "All selected",
        "Cancel",
        "Preview move",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_choose(window.get_rows().row_data(0).unwrap().key);
    window.set_move_z("8".into());
    window.set_snap(false);
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    window.invoke_apply();
    let changed = lock(&shared).document().vmf().clone();
    assert_eq!(
        &changed.entities[0].solids[..100],
        &original.entities[0].solids[..100]
    );
    assert_eq!(
        &changed.entities[0].solids[101..],
        &original.entities[0].solids[101..]
    );
    assert_ne!(
        changed.entities[0].solids[100],
        original.entities[0].solids[100]
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    components_ui::refresh(&window, &lock(&shared));
    window.set_source_index(1);
    window.invoke_choose_source();
    window.set_source_mode(1);
    window.invoke_page_sources(-1);
    window.set_source_index(1);
    window.invoke_choose_source();
    assert_eq!(lock(&shared).component_solids(), vec![ids[0], ids[100]]);
    assert_eq!(window.get_rows().row_count(), 12);
    assert!(
        window
            .get_rows()
            .row_data(0)
            .unwrap()
            .label
            .starts_with(&format!("Brush {} · ", ids[0]))
    );
    assert!(
        window
            .get_rows()
            .row_data(6)
            .unwrap()
            .label
            .starts_with(&format!("Brush {} · ", ids[100]))
    );
    save_ui_snapshot(window.window(), "component-brush-subset");
    window.set_source_index(1);
    let stamp = lock(&shared).component_stamp();
    window.invoke_choose_source();
    assert_eq!(lock(&shared).component_stamp(), stamp);
    window.set_source_mode(2);
    window.invoke_page_sources(1);
    window.set_source_index(1);
    window.invoke_choose_source();
    assert_eq!(lock(&shared).component_solids(), vec![ids[0]]);
    window.invoke_page_sources(-1);
    window.set_source_index(1);
    window.invoke_choose_source();
    assert!(!window.get_picking());
    assert_eq!(window.get_source_mode(), 0);
    assert_eq!(lock(&shared).selected_ids(), vec![owner]);
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_cancel();
}

#[test]
pub(crate) fn component_scale_rotate_controls_share_pivots_and_preserve_valid_drafts() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let shape = state.component_topology(&[id]).unwrap().remove(0);
    let side = shape
        .faces
        .iter()
        .find(|face| {
            face.vertices
                .iter()
                .all(|index| shape.vertices[*index].position.z == 64.0)
        })
        .unwrap()
        .side;
    state
        .select_components(
            &[map_core::subobject::Component {
                solid: id,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &state.component_stamp(),
        )
        .unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(2);
    window.set_operation(3);
    components_ui::refresh(&window, &lock(&shared));
    assert!(window.get_transform_center().contains("(32, 32, 64)"));
    window.invoke_preview_edit();
    assert!(window.get_ready());
    let draft = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "component-scale");
    for label in [
        "Cancel",
        "Apply scale",
        "Discard preview",
        "Preview scale",
        "Component transform X",
        "Component transform Y",
        "Component transform Z",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_scale_x("0".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    window.invoke_discard_preview();
    window.set_operation(4);
    window.set_pivot_mode(1);
    window.invoke_choose_pivot();
    assert_eq!(window.get_pivot_x(), "32");
    assert_eq!(window.get_pivot_y(), "32");
    assert_eq!(window.get_pivot_z(), "64");
    window.invoke_preview_edit();
    assert!(window.get_ready());
    save_ui_snapshot(window.window(), "component-rotate");
    for label in [
        "Apply rotation",
        "Preview rotation",
        "Component pivot X",
        "Component pivot Y",
        "Component pivot Z",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    let rotated = lock(&shared).render_document().vmf().clone();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &rotated);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    let mut session = lock(&shared);
    let stamp = session.component_stamp();
    session
        .select_components(
            &[map_core::subobject::Component {
                solid: id,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &stamp,
        )
        .unwrap();
    let stamp = session.component_stamp();
    session
        .preview_component_transform(
            map_core::subobject::ComponentTransform::Scale(DVec3::new(0.75, 0.5, 1.0)),
            Some(DVec3::new(16.125, 24.25, 64.0)),
            false,
            &stamp,
        )
        .unwrap();
    components_ui::refresh(&window, &session);
    assert_eq!(window.get_operation(), 3);
    assert_eq!(window.get_scale_x(), "0.75");
    assert_eq!(window.get_pivot_x(), "16.125");
    assert!(window.get_ready());
    drop(session);
    window.invoke_cancel();
}

#[test]
pub(crate) fn component_weld_target_controls_preview_existing_corners_and_world_positions() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let selected = state.component_topology(&[id]).unwrap()[0]
        .vertices
        .iter()
        .filter(|vertex| vertex.position.z == 64.0)
        .map(|vertex| map_core::subobject::Component {
            solid: id,
            sides: vertex.sides.clone(),
        })
        .collect::<Vec<_>>();
    state
        .select_components(&selected, SelectMode::Replace, &state.component_stamp())
        .unwrap();
    let original = state.document().vmf().clone();
    let corner_index = state
        .component_weld_targets()
        .unwrap()
        .iter()
        .position(|(component, _)| component == &selected[0])
        .unwrap();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(1);
    window.set_operation(1);
    components_ui::refresh(&window, &lock(&shared));
    assert_eq!(window.get_weld_corner_labels().row_count(), 8);
    window.set_weld_target_mode(2);
    window.invoke_choose_weld_mode();
    assert_eq!(window.get_weld_x(), "32");
    assert_eq!(window.get_weld_y(), "32");
    assert_eq!(window.get_weld_z(), "64");
    window.set_weld_x("16.125".into());
    window.set_weld_y("24.25".into());
    window.set_weld_z("96.5".into());
    window.set_texture_lock(true);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(
        window
            .get_preview_summary()
            .contains("Target: (16.125, 24.25, 96.5)")
    );
    let draft = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-weld-position");
    for label in [
        "Cancel",
        "Apply weld",
        "Discard preview",
        "Preview weld",
        "Weld target X",
        "Weld target Y",
        "Weld target Z",
        "Keep textures at surviving corners",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_weld_z("65537".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.set_weld_z("96.5".into());
    window.invoke_preview_edit();
    window.invoke_discard_preview();
    assert_eq!(lock(&shared).render_document().vmf(), &original);
    window.set_weld_target_mode(1);
    window.set_weld_corner(corner_index as i32);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(window.get_weld_target_mode(), 1);
    save_ui_snapshot(window.window(), "brush-weld-corner");
    window.invoke_apply();
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_cancel();
}

#[test]
pub(crate) fn component_split_controls_show_new_corners_validate_rise_and_restore_selection() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(2);
    window.set_operation(2);
    components_ui::refresh(&window, &lock(&shared));
    let top = window
        .get_rows()
        .iter()
        .find(|row| row.label.starts_with("Top"))
        .unwrap();
    window.invoke_choose(top.key);
    window.set_texture_lock(true);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(window.get_topology_preview());
    assert_eq!(
        lock(&shared).component_report(&[id]).unwrap()["texture_mode"],
        "attached"
    );
    assert_eq!(window.get_kind(), 1);
    assert_eq!(window.get_rows().row_count(), 9);
    assert!(
        window
            .get_preview_summary()
            .contains("Add a corner and 3 faces")
    );
    let draft = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-face-split");
    for label in [
        "Apply split",
        "Discard preview",
        "Cancel",
        "Keep textures at corners",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_split_rise("0".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(!window.get_message().is_empty());
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_choose_rise(32.0);
    assert_eq!(window.get_split_rise(), "32");
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.invoke_discard_preview();
    assert_eq!(window.get_kind(), 2);
    assert_eq!(lock(&shared).component_selection().len(), 1);
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(window.get_kind(), 1);
    assert_eq!(lock(&shared).component_selection().len(), 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);

    window.set_kind(0);
    window.invoke_refresh();
    let shape = lock(&shared).component_topology(&[id]).unwrap().remove(0);
    let edge = shape
        .edges
        .iter()
        .find(|edge| {
            edge.vertices
                .iter()
                .all(|index| shape.vertices[*index].position.z == 64.0)
        })
        .unwrap();
    let key = format!(
        "{}:{}",
        id,
        edge.sides
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    window.invoke_choose(key.into());
    window.set_split_rise("16".into());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-edge-split");
    window.invoke_apply();
    assert_eq!(
        lock(&shared).component_topology(&[id]).unwrap()[0]
            .faces
            .len(),
        10
    );
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_cancel();
}

#[test]
pub(crate) fn ridge_controls_share_edge_choices_valid_drafts_cancel_apply_and_undo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(2);
    window.set_operation(5);
    components_ui::refresh(&window, &lock(&shared));
    let top = window
        .get_rows()
        .iter()
        .find(|row| row.label.starts_with("Top"))
        .unwrap();
    window.invoke_choose(top.key);
    assert_eq!(window.get_crease_rows().row_count(), 1);
    let row = window.get_crease_rows().row_data(0).unwrap();
    assert_eq!(row.choices.row_count(), 2);
    assert!(row.choices.row_data(0).unwrap().contains("direction"));
    assert!(row.choices.row_data(1).unwrap().contains("direction"));
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(window.get_topology_preview());
    assert_eq!(window.get_kind(), 1);
    assert_eq!(window.get_rows().row_count(), 10);
    assert!(
        window
            .get_preview_summary()
            .contains("Add 2 corners and 1 face")
    );
    let draft = lock(&shared).render_document().vmf().clone();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-roof-ridge");
    for label in ["Apply ridge", "Discard preview", "Cancel", "Ridge rise"] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_split_rise("0".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_choose_rise(24.0);
    window.invoke_choose_crease_pair(row.key.clone(), 1);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_ne!(lock(&shared).render_document().vmf(), &draft);
    window.invoke_discard_preview();
    assert_eq!(window.get_kind(), 2);
    assert_eq!(lock(&shared).component_selection().len(), 1);
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).component_selection().len(), 2);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.set_kind(2);
    window.invoke_refresh();
    let top = window
        .get_rows()
        .iter()
        .find(|row| row.label.starts_with("Top"))
        .unwrap();
    window.invoke_choose(top.key);
    let report = lock(&shared).component_report(&[]).unwrap();
    let edges = report["crease_choices"][0]["pairs"][0]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_u64().unwrap() as u32)
        .collect::<Vec<_>>();
    let id = report["crease_choices"][0]["component"]["solid"]
        .as_u64()
        .unwrap() as u32;
    {
        let mut session = lock(&shared);
        let stamp = session.component_stamp();
        session
            .preview_component_crease(
                &[session::components::CreaseEdges {
                    solid: id,
                    edges: [edges[0], edges[1]],
                }],
                12.5,
                &stamp,
            )
            .unwrap();
    }
    components_ui::refresh(&window, &lock(&shared));
    assert_eq!(window.get_split_rise(), "12.5");
    assert_eq!(window.get_crease_rows().row_data(0).unwrap().choice, 0);
    assert!(window.get_ready());
    window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(lock(&shared).component_report(&[]).unwrap()["preview"].is_null());
}

#[test]
pub(crate) fn component_weld_controls_preview_discard_apply_and_undo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = ComponentsWindow::new().unwrap();
    components_ui::bind(&window, &shared);
    window.set_kind(1);
    window.set_operation(1);
    components_ui::refresh(&window, &lock(&shared));
    let shape = lock(&shared).component_topology(&[id]).unwrap().remove(0);
    for vertex in shape
        .vertices
        .iter()
        .filter(|vertex| vertex.position.z == 64.0)
    {
        let key = format!(
            "{}:{}",
            id,
            vertex
                .sides
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        window.invoke_choose(key.into());
    }
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(window.get_topology_preview());
    assert!(window.get_summary().contains("1 selected"));
    assert!(
        window
            .get_preview_summary()
            .contains("Merge 4 corners into 1")
    );
    assert_eq!(lock(&shared).document().vmf(), &original);
    let draft = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-weld");
    assert_dialog_button_visible(&window, "Cancel");
    assert_dialog_button_visible(&window, "Apply weld");
    assert_dialog_button_visible(&window, "Discard preview");
    window.invoke_discard_preview();
    assert!(!window.get_topology_preview());
    assert!(!window.get_ready());
    assert_eq!(lock(&shared).component_selection().len(), 4);
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &draft);
    assert_eq!(lock(&shared).component_selection().len(), 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_refresh();
    window.invoke_cancel();
}

#[test]
pub(crate) fn merge_controls_keep_sources_and_share_preview_discard_cancel_apply() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for (mins, maxs) in [
        (DVec3::ZERO, DVec3::splat(64.0)),
        (DVec3::new(64.0, 0.0, 0.0), DVec3::new(128.0, 64.0, 64.0)),
        (DVec3::splat(256.0), DVec3::splat(288.0)),
    ] {
        ids.push(
            state
                .create_block(Actor::Agent, mins, maxs, "brick/test", false)
                .unwrap(),
        );
    }
    state.select(&ids, SelectMode::Replace).unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = MergeWindow::new().unwrap();
    merge_ui::bind(&window, &shared);
    merge_ui::refresh(&window, &lock(&shared));
    assert_eq!(window.get_sources().row_count(), 3);
    let excluded = window.get_sources().row_data(2).unwrap().solid;
    window.invoke_choose_source(excluded.clone());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(window.get_sources().row_count(), 3);
    assert!(!window.get_sources().row_data(2).unwrap().selected);
    assert!(window.get_summary().contains("2 brushes"));
    assert_eq!(lock(&shared).document().vmf(), &original);
    let draft = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-merging");
    for label in ["Cancel", "Discard preview", "Preview merge", "Apply merge"] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_choose_source(excluded.clone());
    assert!(!window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_choose_source(excluded);
    window.invoke_discard();
    assert!(window.get_token().is_empty());
    assert_eq!(lock(&shared).render_document().vmf(), &original);
    window.invoke_preview_edit();
    window.invoke_cancel();
    assert!(lock(&shared).merge_report()["preview"].is_null());
    merge_ui::refresh(&window, &lock(&shared));
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &draft);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
}

#[test]
pub(crate) fn carving_multiple_cutters_exposes_roles_and_preserves_preview_cancel_apply() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for (mins, maxs) in [
        (DVec3::ZERO, DVec3::splat(128.0)),
        (DVec3::splat(16.0), DVec3::splat(80.0)),
        (DVec3::splat(48.0), DVec3::splat(112.0)),
        (DVec3::splat(256.0), DVec3::splat(288.0)),
    ] {
        ids.push(
            state
                .create_block(Actor::Agent, mins, maxs, "brick/test", false)
                .unwrap(),
        );
    }
    state.select(&ids, SelectMode::Replace).unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = CarveWindow::new().unwrap();
    carve_ui::bind(&window, &shared);
    carve_ui::refresh(&window, &lock(&shared));
    window.set_cutter(1);
    window.invoke_choose_cutter();
    window.set_multiple_cutters(true);
    window.invoke_toggle_multiple();
    let second = window.get_targets().row_data(2).unwrap().solid;
    let ignored = window.get_targets().row_data(3).unwrap().solid;
    window.invoke_choose_cut_role(second.clone());
    window.invoke_choose_target(ignored);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(window.get_multiple_cutters());
    assert_eq!(
        window.get_targets().iter().filter(|row| row.cutter).count(),
        2
    );
    assert!(!window.get_targets().row_data(3).unwrap().selected);
    let draft = lock(&shared).render_document().vmf().clone();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.show().unwrap();
    save_ui_snapshot(window.window(), "carving-multiple-cutters");
    for label in [
        "Cancel",
        "Discard preview",
        "Preview carve",
        "Apply carve",
        "Use multiple cutter brushes",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_choose_cut_role(second.clone());
    assert!(!window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_choose_cut_role(second);
    window.invoke_preview_edit();
    window.invoke_discard();
    assert_eq!(lock(&shared).render_document().vmf(), &original);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &draft);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    lock(&shared).select(&ids, SelectMode::Replace).unwrap();
    carve_ui::refresh(&window, &lock(&shared));
    window.set_cutter(1);
    window.invoke_choose_cutter();
    window.set_multiple_cutters(true);
    window.invoke_toggle_multiple();
    window.invoke_choose_cut_role(window.get_targets().row_data(2).unwrap().solid);
    window.invoke_choose_target(window.get_targets().row_data(3).unwrap().solid);
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    window.invoke_cancel();
    assert_eq!(lock(&shared).render_document().vmf(), &original);
}

#[test]
pub(crate) fn carve_controls_keep_targets_show_preview_and_share_apply_discard_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let target = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let untouched = state
        .create_block(
            Actor::Agent,
            DVec3::splat(256.0),
            DVec3::splat(288.0),
            "brick/test",
            false,
        )
        .unwrap();
    let cutter = state
        .create_block(
            Actor::Agent,
            DVec3::splat(16.0),
            DVec3::splat(48.0),
            "brick/test",
            false,
        )
        .unwrap();
    state
        .select(&[target, untouched, cutter], SelectMode::Replace)
        .unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = CarveWindow::new().unwrap();
    carve_ui::bind(&window, &shared);
    carve_ui::refresh(&window, &lock(&shared));
    assert_eq!(window.get_cutter(), 2);
    let untouched_key = window.get_targets().row_data(1).unwrap().solid;
    window.invoke_choose_target(untouched_key.clone());
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(window.get_summary().contains("6 fragments"));
    assert_eq!(window.get_targets().row_count(), 3);
    assert!(!window.get_targets().row_data(1).unwrap().selected);
    assert_eq!(lock(&shared).document().vmf(), &original);
    let draft = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "brush-carving");
    for label in ["Cancel", "Discard preview", "Preview carve", "Apply carve"] {
        assert_dialog_button_visible(&window, label);
    }
    let target_key = window.get_targets().row_data(0).unwrap().solid;
    window.invoke_choose_target(target_key.clone());
    assert!(!window.get_ready());
    assert!(!window.get_can_preview());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.invoke_choose_target(target_key);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.invoke_discard();
    assert!(!window.get_ready());
    assert_eq!(lock(&shared).render_document().vmf(), &original);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &draft);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    lock(&shared)
        .select(&[target, untouched, cutter], SelectMode::Replace)
        .unwrap();
    carve_ui::refresh(&window, &lock(&shared));
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).render_document().vmf(), &original);
    assert!(lock(&shared).carve_state()["preview"].is_null());
}

#[test]
pub(crate) fn hollow_controls_preview_validate_look_inside_apply_and_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(256.0),
            "brick/test",
            false,
        )
        .unwrap();
    let original = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let window = HollowWindow::new().unwrap();
    hollow_ui::bind(&window, &shared);
    hollow_ui::refresh(&window, &lock(&shared));
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(lock(&shared).document().vmf(), &original);
    let valid = lock(&shared).render_document().vmf().clone();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "hollow-brushes");
    for label in ["Cancel", "Create walls", "Preview walls", "Look inside"] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_look_inside();
    assert_eq!(lock(&shared).camera().position, Vec3::splat(128.0));
    window.set_thickness("128".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(!window.get_message().is_empty());
    assert_eq!(lock(&shared).render_document().vmf(), &valid);
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    window.set_thickness("16".into());
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &valid);
    lock(&shared).undo(Actor::Agent).unwrap();
    hollow_ui::refresh(&window, &lock(&shared));
    window.invoke_preview_edit();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).render_document().vmf(), &original);
    hollow_ui::refresh(&window, &lock(&shared));
    let top = window
        .get_faces()
        .iter()
        .find(|face| face.label.starts_with("Top"))
        .unwrap();
    window.invoke_choose_opening(top.key);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(
        lock(&shared).hollow_state()["preview"]["result"]["brushes"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    window.show().unwrap();
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    save_ui_snapshot(window.window(), "hollow-openings");
    let open = lock(&shared).render_document().vmf().clone();
    for row in window
        .get_faces()
        .iter()
        .filter(|face| !face.open)
        .collect::<Vec<_>>()
    {
        window.invoke_choose_opening(row.key);
    }
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(window.get_message().contains("at least one wall"));
    assert_eq!(lock(&shared).render_document().vmf(), &open);
    assert_dialog_button_visible(&window, "Close all openings");
    window.invoke_close_openings();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(
        lock(&shared).hollow_state()["preview"]["result"]["brushes"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    window.invoke_cancel();
    assert_eq!(lock(&shared).render_document().vmf(), &original);
}

pub(crate) fn save_ui_snapshot(window: &slint::Window, name: &str) {
    window.take_snapshot().unwrap();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    let image = window.take_snapshot().unwrap();
    assert!(image.width() > 0 && image.height() > 0);
    if let Some(directory) = std::env::var_os("HAMMERR_UI_SNAPSHOTS") {
        let path = PathBuf::from(directory).join(format!("{name}.png"));
        let mut png = png::Encoder::new(
            std::fs::File::create(path).unwrap(),
            image.width(),
            image.height(),
        );
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()
            .unwrap()
            .write_image_data(image.as_bytes())
            .unwrap();
    }
}

#[test]
pub(crate) fn saved_view_controls_restore_rename_replace_cancel_and_guard_map_changes() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets".into()))));
    let controller = saved_views_ui::Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller.window.set_view_name("Courtyard entrance".into());
    controller.window.invoke_save_new();
    let original = lock(&shared).camera();
    assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 1);
    lock(&shared).set_projection(Actor::Agent, render::Projection::Top);
    controller.window.set_view_name("Top · route layout".into());
    controller.window.invoke_save_new();
    assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 2);
    controller.window.invoke_choose("1".into());
    let history = lock(&shared).document().undo_labels().count();
    controller.window.invoke_restore_view();
    assert_eq!(lock(&shared).camera(), original);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    controller.window.set_view_name("Entrance overview".into());
    controller.window.invoke_rename_view();
    assert_eq!(
        lock(&shared).camera_views().unwrap().views[0].name,
        "Entrance overview"
    );
    lock(&shared).set_projection(Actor::Agent, render::Projection::Front);
    controller.window.invoke_replace_view();
    assert_eq!(
        lock(&shared).camera_views().unwrap().views[0].projection,
        "front"
    );
    let before = lock(&shared).document().vmf().clone();
    controller.window.set_view_name("top · route layout".into());
    controller.window.invoke_rename_view();
    assert!(controller.window.get_message().contains("already"));
    assert_eq!(lock(&shared).document().vmf(), &before);
    controller.window.invoke_choose("1".into());
    save_ui_snapshot(controller.window.window(), "saved-views");
    controller.window.invoke_close_dialog();
    assert_eq!(lock(&shared).document().vmf(), &before);
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_choose("1".into());
    lock(&shared)
        .rename_camera_view(Actor::Agent, 1, "Agent name", None)
        .unwrap();
    controller.window.invoke_delete_view();
    assert!(controller.window.get_message().contains("Map changed"));
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_selected_id().is_empty());
    controller.window.invoke_choose("1".into());
    controller.window.invoke_delete_view();
    assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 2);
}

#[test]
pub(crate) fn compile_controls_share_cancel_status_and_guard_replaced_jobs() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, mut game, _) = crate::test_assets::game_process(true);
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    std::fs::write(materials.join("water.vmt"), "Water { $abovewater 1 }").unwrap();
    std::fs::write(
        materials.join("nodraw.vmt"),
        "UnlitGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    std::fs::write(materials.join("sky.vmt"), "UnlitGeneric { %compilesky 1 }").unwrap();
    std::fs::write(
        materials.join("hint.vmt"),
        "UnlitGeneric { %compilehint 1 }",
    )
    .unwrap();
    std::fs::write(
        materials.join("skip.vmt"),
        "UnlitGeneric { %compileskip 1 }",
    )
    .unwrap();
    std::fs::create_dir(materials.join("dev")).unwrap();
    std::fs::write(
        materials.join("dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric {}",
    )
    .unwrap();
    game.mounts.push(source_formats::gamefs::MountedPath {
        ids: vec!["game".into()],
        mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
    });
    std::fs::create_dir(directory.path().join("models")).unwrap();
    let mut model = vec![0; 240];
    model[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
        model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [116, 140] {
        for axis in 0..3 {
            model[offset + axis * 4..offset + axis * 4 + 4]
                .copy_from_slice(&32.0_f32.to_le_bytes());
        }
    }
    std::fs::write(directory.path().join("models/check.mdl"), model).unwrap();
    let physics_root = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|path| path.join(".local/share/Steam/steamapps/common/Counter-Strike Source"));
    let native_physics = physics_root
        .as_ref()
        .is_some_and(|root| root.join("bin/linux64/vphysics.so").is_file())
        && std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("hammerr-physics")
            .is_file();
    if native_physics {
        game.base_dir = physics_root.unwrap();
    }
    let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
    let controller = compile_ui::Controller::new(&shared).unwrap();
    controller.refresh(&lock(&shared));
    let app = AppWindow::new().unwrap();
    app.on_build_status({
        let window = controller.window.as_weak();
        move || window.unwrap().show().unwrap()
    });
    app.show().unwrap();
    assert!(app.get_compile_label().is_empty());
    save_ui_snapshot(app.window(), "build-settings-toolbar");
    assert_dialog_button_visible(&app, "Build settings…");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Build settings…")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(controller.window.window().is_visible());
    assert!(lock(&shared).compile_status()["job"].is_null());
    assert!(!controller.window.get_cancellable());
    assert_eq!(controller.window.get_lighting_choice(), 0);
    assert_eq!(controller.window.get_selected_lighting(), "Fullbright");
    assert_eq!(
        controller.window.get_selected_physics(),
        "No physics bodies"
    );
    controller.window.invoke_change_physics(1);
    controller.refresh(&lock(&shared));
    assert_eq!(
        controller.window.get_selected_physics(),
        "CS:S engine physics"
    );
    assert_eq!(lock(&shared).compile_settings()["physics"], "engine");
    controller.window.invoke_change_physics(0);
    controller.refresh(&lock(&shared));
    assert_eq!(
        controller.window.get_selected_visibility(),
        "Fast (conservative)"
    );
    controller.window.invoke_change_visibility(1);
    controller.refresh(&lock(&shared));
    assert_eq!(
        controller.window.get_selected_visibility(),
        "Detailed (portal clipping)"
    );
    assert_eq!(lock(&shared).compile_settings()["visibility"], "detailed");
    assert_eq!(
        controller.window.get_selected_physics(),
        "No physics bodies"
    );
    controller.window.invoke_change_lighting(1);
    assert_eq!(lock(&shared).compile_settings()["lighting"], "direct_ldr");
    controller.refresh(&lock(&shared));
    assert_eq!(controller.window.get_lighting_choice(), 1);
    assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
    assert!(!controller.window.get_lighting_description().is_empty());
    controller.window.invoke_change_sky_quality(1);
    controller.refresh(&lock(&shared));
    assert_eq!(lock(&shared).compile_settings()["sky_quality"], "balanced");
    assert_eq!(controller.window.get_selected_bounces(), "Direct only");
    for choice in [2, 3, 4] {
        controller.window.invoke_change_bounces(choice);
        controller.refresh(&lock(&shared));
        assert_eq!(
            controller.window.get_selected_bounces(),
            compiler::bounce_label(choice as u8)
        );
        assert_eq!(lock(&shared).compile_settings()["bounces"], choice);
    }
    controller.window.invoke_change_bounces(1);
    controller.refresh(&lock(&shared));
    assert_eq!(
        controller.window.get_selected_bounces(),
        "One diffuse bounce"
    );
    assert_eq!(lock(&shared).compile_settings()["bounces"], 1);
    assert_eq!(controller.window.get_smoothing_angle(), 0);
    controller.window.invoke_change_smoothing_angle(45);
    controller.refresh(&lock(&shared));
    assert_eq!(controller.window.get_smoothing_angle(), 45);
    assert_eq!(lock(&shared).compile_settings()["smoothing_angle"], 45);
    save_ui_snapshot(controller.window.window(), "build-settings");
    assert!(lock(&shared).compile_status()["job"].is_null());
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    controller.window.invoke_change_sky_quality(0);
    controller.window.invoke_change_visibility(0);
    controller.window.invoke_change_smoothing_angle(0);
    controller.window.invoke_change_bounces(0);
    controller.window.invoke_change_lighting(0);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_summary().contains("No build"));
    assert!(!controller.window.get_launch_ready());
    let path = directory.path().join("room.bsp");
    let request = lock(&shared)
        .begin_compile(Actor::User, Some(&path))
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_cancellable());
    assert!(!controller.window.get_launch_ready());
    assert!(controller.window.get_summary().contains("Waiting"));
    controller.window.invoke_cancel_build();
    let result = request.run();
    lock(&shared).finish_compile(Actor::User, &result);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_summary().contains("cancelled"));
    assert!(!controller.window.get_cancellable());
    let request = lock(&shared)
        .begin_compile(Actor::Agent, Some(&path))
        .unwrap();
    controller.window.invoke_cancel_build();
    assert!(controller.window.get_message().contains("replaced"));
    assert_eq!(lock(&shared).compile_status()["job"]["status"], "running");
    let result = request.run();
    lock(&shared).finish_compile(Actor::Agent, &result);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_summary().contains("failed"));
    assert!(!controller.window.get_diagnostics().is_empty());
    let vmf_path = directory.path().join("room.vmf");
    let mut room = map_core::Document::new(test_assets::room());
    room.edit("cubemap sample", |tx| {
        map_core::edit::create_entity(
            tx,
            "env_cubemap",
            Some(DVec3::new(0.0, 0.0, 64.0)),
            &[("cubemapsize".into(), "9".into())],
            Vec::new(),
        )?;
        Ok(())
    })
    .unwrap();
    room.edit("pool", |tx| {
        let ObjectId::Solid(id) = map_core::edit::create_block(
            tx,
            DVec3::new(-128.0, -128.0, 0.0),
            DVec3::new(128.0, 128.0, 64.0),
            "nodraw",
        )?
        else {
            unreachable!()
        };
        for side in &mut tx.solid_mut(id)?.sides {
            if map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                side.material = "water".into();
            }
        }
        Ok(())
    })
    .unwrap();
    room.edit("sky ceiling", |tx| {
        let id = tx
            .vmf()
            .world
            .solids
            .iter()
            .find(|solid| solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0))
            .unwrap()
            .id;
        for side in &mut tx.solid_mut(id)?.sides {
            if map_core::Plane::from_points(side.plane.0).unwrap().normal.z < -0.99 {
                side.material = "sky".into();
            }
        }
        Ok(())
    })
    .unwrap();
    room.edit("hint partition", |tx| {
        let ObjectId::Solid(id) = map_core::edit::create_block(
            tx,
            DVec3::new(-8.0, -64.0, 8.0),
            DVec3::new(8.0, 64.0, 192.0),
            "skip",
        )?
        else {
            unreachable!()
        };
        for side in &mut tx.solid_mut(id)?.sides {
            if map_core::Plane::from_points(side.plane.0).unwrap().normal.x > 0.99 {
                side.material = "hint".into();
            }
        }
        Ok(())
    })
    .unwrap();
    room.edit("baked point light", |tx| {
        map_core::edit::create_entity(
            tx,
            "light_spot",
            Some(DVec3::new(0.0, -160.0, 128.0)),
            &[
                ("_light".into(), "255 170 100 300".into()),
                ("_quadratic_attn".into(), "1".into()),
                ("pitch".into(), "-90".into()),
                ("_inner_cone".into(), "20".into()),
                ("_cone".into(), "45".into()),
                ("_exponent".into(), "1".into()),
            ],
            Vec::new(),
        )
    })
    .unwrap();
    room.edit("static prop", |tx| {
        map_core::edit::create_entity(
            tx,
            "prop_static",
            Some(DVec3::new(-200.0, 160.0, 8.0)),
            &[
                ("model".into(), "models/check.mdl".into()),
                ("solid".into(), "0".into()),
                ("disableshadows".into(), "1".into()),
            ],
            Vec::new(),
        )
    })
    .unwrap();
    room.edit("terrain surface", |tx| {
        let ObjectId::Solid(id) = map_core::edit::create_block(
            tx,
            DVec3::new(-224.0, -224.0, 0.0),
            DVec3::new(-160.0, -160.0, 8.0),
            "dev/dev_measuregeneric01",
        )?
        else {
            unreachable!()
        };
        let solid = tx.solid_mut(id)?;
        let geometry = map_core::BrushGeometry::from_solid(solid).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap();
        solid.sides[face.side].displacement = Some(source_formats::vmf::DispInfo {
            power: 2,
            start_position: face.vertices[0],
            flags: Some(0),
            elevation: 0.0,
            subdivided: false,
            normals: None,
            distances: None,
            offsets: None,
            offset_normals: None,
            alphas: None,
            triangle_tags: None,
            allowed_verts: None,
            extra_properties: Default::default(),
            extra: Vec::new(),
        });
        Ok(())
    })
    .unwrap();
    room.edit("world smoothing", |tx| {
        let ids = tx
            .vmf()
            .world
            .solids
            .iter()
            .map(|solid| solid.id)
            .collect::<Vec<_>>();
        for id in ids {
            for side in &mut tx.solid_mut(id)?.sides {
                side.smoothing_groups = Some(1);
            }
        }
        Ok(())
    })
    .unwrap();
    std::fs::write(&vmf_path, room.vmf().write(&Default::default())).unwrap();
    lock(&shared).open(Actor::User, &vmf_path, true).unwrap();
    controller.window.invoke_change_lighting(1);
    if native_physics {
        controller.window.invoke_change_physics(1);
    }
    lock(&shared)
        .update_compile_options(Actor::User, None, None, None, None, Some(1))
        .unwrap();
    let request = lock(&shared)
        .begin_compile(Actor::Agent, Some(&path))
        .unwrap();
    let result = request.run();
    lock(&shared).finish_compile(Actor::Agent, &result);
    controller.show(&lock(&shared)).unwrap();
    assert!(controller.window.get_summary().starts_with("Compiled in"));
    assert!(
        controller
            .window
            .get_details()
            .contains("Cubemap samples: 1")
    );
    assert_eq!(
        lock(&shared).compile_status()["job"]["result"]["stats"]["cubemap_samples"],
        1
    );
    controller.window.invoke_change_sky_quality(1);
    controller.refresh(&lock(&shared));
    assert_eq!(lock(&shared).compile_settings()["sky_quality"], "balanced");
    assert_eq!(
        controller.window.get_selected_sky_quality().as_str(),
        "Balanced (128 directions)"
    );
    controller.window.invoke_change_sky_quality(0);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_details().contains("Visibility:"));
    let stats = lock(&shared).compile_status()["job"]["result"]["stats"].clone();
    assert_eq!(stats["hint_surfaces"], 1);
    assert!(stats["hint_splits"].as_u64().unwrap() > 0);
    assert!(controller.window.get_details().contains("Hint surfaces: 1"));
    assert!(
        controller
            .window
            .get_details()
            .contains("Visibility: Fast (conservative) · Sampling: Fast (32 directions)")
    );
    assert_eq!(stats["static_props"], 1);
    assert_eq!(stats["static_prop_models"], 1);
    assert!(controller.window.get_details().contains("Static props: 1"));
    assert_eq!(stats["prop_shadow_disabled"], 1);
    assert!(
        controller
            .window
            .get_details()
            .contains("Prop mesh shadows: 0 props · 0 triangles · 1 disabled")
    );
    assert_eq!(stats["displacements"], 1);
    assert_eq!(stats["displacement_vertices"], 25);
    assert_eq!(stats["displacement_triangles"], 32);
    assert_eq!(stats["displacement_neighbor_pairs"], 0);
    assert!(
        controller
            .window
            .get_details()
            .contains("Neighbor pairs: 0")
    );
    assert!(
        controller
            .window
            .get_details()
            .contains("Terrain surfaces: 1")
    );
    assert!(stats["smoothed_faces"].as_u64().unwrap() > 0);
    assert!(stats["smoothed_corners"].as_u64().unwrap() > 0);
    assert!(controller.window.get_details().contains("Brush smoothing:"));
    assert_eq!(stats["bounces"], 1);
    assert!(stats["bounce_rays"].as_u64().unwrap() > 0);
    assert!(
        controller
            .window
            .get_details()
            .contains("Indirect light: One diffuse bounce")
    );
    assert_eq!(stats["lighting_mode"], "direct_ldr");
    assert_eq!(stats["world_lights"], 1);
    if native_physics {
        assert_eq!(stats["physics_mode"], "engine");
        assert!(stats["physics_convexes"].as_u64().unwrap() > 0);
        assert_eq!(stats["physics_fluids"], 1);
        assert_eq!(stats["physics_mesh_triangles"], 32);
        assert!(
            controller
                .window
                .get_details()
                .contains("Terrain triangles: 32")
        );
        assert!(controller.window.get_details().contains("1 fluid region"));
    }
    assert!(controller.window.get_details().contains("Baked lighting:"));
    assert!(stats["clusters"].as_u64().unwrap() > 1);
    assert_eq!(stats["water_regions"], 1);
    let leaves = stats["water_leaves"].as_u64().unwrap();
    assert!(leaves > 0);
    assert!(
        controller
            .window
            .get_details()
            .contains(&format!("Water: 1 region · {leaves} submerged"))
    );
    assert!(
        controller
            .window
            .get_diagnostics()
            .contains("depth-specific material")
    );
    let sky = stats["sky_clusters"].as_u64().unwrap();
    assert!(sky > 0);
    assert!(
        controller
            .window
            .get_details()
            .contains(&format!("Sky visible from: {sky} clusters"))
    );
    assert!(controller.window.get_launch_ready());
    assert!(controller.window.get_launch_details().contains("room.bsp"));
    assert!(
        controller
            .window
            .get_details()
            .contains("current map snapshot")
    );
    let details = controller.window.get_details();
    controller
        .window
        .set_details((details.to_string() + &"Extra build detail.\n".repeat(80)).into());
    controller
        .window
        .set_diagnostics("A long diagnostic message.\n".repeat(80).into());
    assert_dialog_button_visible(&controller.window, "Close");
    assert_dialog_button_visible(&controller.window, "Cancel build");
    assert_dialog_button_visible(&controller.window, "Play built map");
    controller.refresh(&lock(&shared));
    save_ui_snapshot(controller.window.window(), "build-status");
    assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    assert_eq!(lock(&shared).compile_status()["job"]["status"], "completed");
    controller.window.invoke_play_build();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while lock(&shared).launching() {
        assert!(
            std::time::Instant::now() < deadline,
            "game launch timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(lock(&shared).game_running());
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_launch_ready());
    lock(&shared).stop_game(Actor::User).unwrap();
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_launch_ready());
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    controller.window.invoke_play_build();
    assert!(!lock(&shared).launching());
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_launch_ready());
}

#[test]
pub(crate) fn volume_controls_preview_frame_cancel_and_guard_shared_selection() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets".into()));
    let a = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let b = session
        .create_block(
            Actor::User,
            DVec3::new(128.0, 0.0, 0.0),
            DVec3::new(192.0, 64.0, 64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let controller = volume_ui::Controller::new(&shared, || 0.4).unwrap();
    controller.initialize(&mut lock(&shared));
    let window = &controller.window;
    window.show().unwrap();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(lock(&shared).selected_ids(), [b]);
    window.invoke_frame();
    assert!(lock(&shared).volume_preview().is_some());
    {
        let session = lock(&shared);
        let spec = session.volume_preview().unwrap().spec;
        let matrix = session.camera().view_projection(0.4);
        for x in [spec.mins.x, spec.maxs.x] {
            for y in [spec.mins.y, spec.maxs.y] {
                for z in [spec.mins.z, spec.maxs.z] {
                    let point = matrix.project_point3(Vec3::new(x as f32, y as f32, z as f32));
                    assert!(point.x.abs() <= 1.0001 && point.y.abs() <= 1.0001);
                }
            }
        }
    }
    save_ui_snapshot(window.window(), "volume-selection");
    for label in [
        "Cancel",
        "Frame box",
        "Preview matches",
        "Select matches",
        "Use selection bounds",
    ] {
        assert_dialog_button_visible(window, label);
    }
    let foreign = lock(&shared)
        .preview_volume_selection(session::volume::Spec {
            mins: DVec3::splat(-1.0),
            maxs: DVec3::splat(65.0),
            rule: map_core::region::Match::Contained,
            mode: SelectMode::Replace,
            faces: false,
        })
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).selected_ids(), [b]);
    window.invoke_cancel();
    assert_eq!(lock(&shared).volume_preview().unwrap().token, foreign);
    controller.initialize(&mut lock(&shared));
    window.set_min_x("0".into());
    window.set_max_x("64".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.invoke_apply();
    assert_eq!(lock(&shared).selected_ids(), [a]);
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    controller.initialize(&mut lock(&shared));
    window.set_max_x("-100".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(window.get_message().contains("positive"));
    window.invoke_use_bounds();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_ready());
    window.invoke_apply();
    assert!(lock(&shared).selected_ids().is_empty());
}

#[test]
pub(crate) fn grouped_surface_controls_apply_exact_preview_and_preserve_foreign_drafts() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game) = test_assets::game();
    let mut session = Session::new(Ok(game));
    let mut faces = Vec::new();
    for x in [0.0, 128.0] {
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::User,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 128.0, 64.0, 16.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            panic!()
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
        faces.push(map_core::surface::FaceId { solid, side });
    }
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = surface_group_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    window.show().unwrap();
    window.invoke_preview_edit();
    assert!(lock(&shared).has_face_preview());
    save_ui_snapshot(window.window(), "grouped-textures");
    for label in [
        "Cancel",
        "Choose material…",
        "Preview mapping",
        "Apply mapping",
    ] {
        assert_dialog_button_visible(window, label);
    }
    window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert!(!lock(&shared).has_face_preview());
    window.invoke_preview_edit();
    lock(&shared)
        .edit_faces(
            Actor::Agent,
            &faces,
            map_core::surface::SurfaceEdit {
                shift_u: Some(25.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    window.invoke_apply();
    assert!(window.get_message().contains("Another face preview"));
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.set_projection(1);
    window.invoke_edited();
    window.set_repeats_u("2".into());
    window.invoke_preview_edit();
    assert_ne!(lock(&shared).face_preview_token(), foreign);
    save_ui_snapshot(window.window(), "view-projected-textures");
    let draft = lock(&shared).face_preview_document().unwrap().vmf().clone();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &draft);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    controller.initialize(&mut lock(&shared)).unwrap();
    lock(&shared)
        .select_faces(&faces[..1], SelectMode::Replace)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(window.get_stale());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &before);
}

#[test]
pub(crate) fn material_replacement_controls_preview_cancel_and_reject_foreign_drafts() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(editor_core::startup::mount_game(None));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "dev/dev_measuregeneric01",
            false,
        )
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    let controller = material_replace_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared), "dev/dev_measuregeneric01");
    let window = &controller.window;
    window.invoke_find();
    assert!(window.get_ready());
    controller.choose_material(&mut lock(&shared), true, "dev/dev_measurewall01a");
    window.invoke_preview_edit();
    assert!(lock(&shared).has_face_preview());
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.show().unwrap();
    save_ui_snapshot(window.window(), "material-replacement");
    for id in ["source-thumbnail", "replacement-thumbnail"] {
        let image = i_slint_backend_testing::ElementHandle::find_by_element_id(
            window,
            &format!("MaterialReplaceWindow::{id}"),
        )
        .next()
        .unwrap();
        assert!(image.size().width <= 80.0 && image.size().height <= 80.0);
    }

    for label in [
        "Cancel",
        "Find faces",
        "Select matches",
        "Preview replacement",
        "Replace faces",
    ] {
        assert_dialog_button_visible(window, label);
    }
    window.invoke_cancel();
    assert!(!lock(&shared).has_face_preview());
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.invoke_preview_edit();
    let token = lock(&shared).material_search_report().unwrap().token;
    lock(&shared)
        .replace_materials(Actor::Agent, token, "wood/foreign", true)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_can_apply());
    window.invoke_apply();
    assert!(window.get_message().contains("Another face preview"));
    let foreign = lock(&shared).face_preview_token();
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.invoke_preview_edit();
    window.set_replacement("metal/changed".into());
    window.invoke_replacement_edited();
    assert!(!lock(&shared).has_face_preview());
    window.invoke_apply();
    assert!(
        lock(&shared)
            .document()
            .vmf()
            .solids()
            .flat_map(|solid| &solid.sides)
            .all(|side| side.material == "METAL/CHANGED")
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.invoke_find();
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_ready());
    window.invoke_apply();
    assert!(lock(&shared).document().vmf().world.solids.is_empty());
}

#[test]
pub(crate) fn lighting_controls_preserve_mixed_groups_guard_previews_and_cancel() {
    use map_core::surface::{self, FaceId, SurfaceEdit};
    use slint::Model;
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        panic!()
    };
    let faces = session.document().solid(id).unwrap().sides[..2]
        .iter()
        .map(|side| FaceId {
            solid: id,
            side: side.id,
        })
        .collect::<Vec<_>>();
    session
        .edit_faces(
            Actor::Agent,
            &faces[..1],
            SurfaceEdit {
                lightmap_scale: Some(8),
                smoothing_groups: Some(1),
                ..Default::default()
            },
            false,
            false,
        )
        .unwrap();
    session
        .edit_faces(
            Actor::Agent,
            &faces[1..],
            SurfaceEdit {
                smoothing_groups: Some(2),
                ..Default::default()
            },
            false,
            false,
        )
        .unwrap();
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = lighting_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    assert!(window.get_lightmap_scale().is_empty());
    assert_eq!(window.get_groups().row_data(0).unwrap().state, -1);
    assert_eq!(window.get_groups().row_data(1).unwrap().state, -1);
    window.show().unwrap();
    window.invoke_toggle_group(2, true);
    window.set_lightmap_scale("4".into());
    window.invoke_edited();
    let original_history = lock(&shared).document().undo_labels().count();
    for mode in render::SurfaceView::ALL {
        window.set_view_mode(mode.index());
        window.invoke_view_changed();
        assert_eq!(lock(&shared).view().surface, mode);
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            original_history
        );
    }
    window.set_view_mode(render::SurfaceView::LightmapDensity.index());
    window.invoke_view_changed();
    window.invoke_preview_edit();
    assert_eq!(lock(&shared).document().vmf(), &before);
    for (face, mask) in faces.iter().zip([5, 6]) {
        let session = lock(&shared);
        let side = session.preview_side(*face).unwrap();
        assert_eq!(side.smoothing_groups, Some(mask));
        assert_eq!(side.lightmap_scale, 4);
    }
    controller.refresh(&lock(&shared));
    save_ui_snapshot(window.window(), "face-lighting");
    for label in ["Cancel", "Preview settings", "Apply settings"] {
        assert_dialog_button_visible(window, label);
    }
    window.set_lightmap_scale("0".into());
    window.invoke_edited();
    assert!(!lock(&shared).has_face_preview());
    window.invoke_preview_edit();
    assert!(!lock(&shared).has_face_preview());
    assert!(window.get_message().contains("1–1024"));
    window.set_lightmap_scale("8".into());
    window.invoke_edited();
    window.invoke_preview_edit();
    let old = lock(&shared).face_preview_token();
    lock(&shared)
        .edit_faces(
            Actor::Agent,
            &faces,
            SurfaceEdit {
                lightmap_scale: Some(32),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_can_apply());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &before);
    let replacement = lock(&shared).face_preview_token();
    assert_ne!(old, replacement);
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), replacement);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.show().unwrap();
    window.invoke_clear_groups();
    window.invoke_preview_edit();
    window.invoke_apply();
    for face in &faces {
        assert_eq!(
            surface::side(lock(&shared).document(), *face)
                .unwrap()
                .smoothing_groups,
            Some(0)
        );
    }
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.show().unwrap();
    window.invoke_toggle_group(31, true);
    window.invoke_reset_groups();
    assert_eq!(window.get_groups().row_data(31).unwrap().action, 0);
    window.invoke_preview_edit();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    assert!(!window.window().is_visible());
    assert!(!lock(&shared).has_face_preview());
    assert_eq!(lock(&shared).document().vmf(), &before);

    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_lightmap_scale("32".into());
    window.invoke_edited();
    lock(&shared)
        .select_faces(&faces[..1], SelectMode::Replace)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(window.get_stale());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.invoke_cancel();

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("imported-lightmap-scale.vmf");
    let mut imported = before.clone();
    for side in &mut imported.world.solids[0].sides {
        side.lightmap_scale = 2048;
    }
    std::fs::write(
        &path,
        imported.write(&source_formats::keyvalues::WriteOptions::default()),
    )
    .unwrap();
    lock(&shared).open(Actor::User, &path, true).unwrap();
    lock(&shared)
        .select_faces(&faces, SelectMode::Replace)
        .unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    assert_eq!(window.get_lightmap_scale(), "2048");
    window.invoke_toggle_group(31, true);
    window.invoke_apply();
    for face in &faces {
        let session = lock(&shared);
        let side = surface::side(session.document(), *face).unwrap();
        assert_eq!(side.lightmap_scale, 2048);
        assert_ne!(side.smoothing_groups.unwrap() & (1 << 31), 0);
    }
}

#[test]
pub(crate) fn primitive_controls_preview_validate_frame_apply_and_cancel_shared_geometry() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let session = Session::new(editor_core::startup::mount_game(None));
    let shared = Arc::new(Mutex::new(session));
    let window = PrimitiveWindow::new().unwrap();
    primitive_ui::bind(&window, &shared);
    primitive_ui::initialize(&window, &mut lock(&shared), "dev/dev_measuregeneric01");
    window.show().unwrap();
    let before = lock(&shared).document().vmf().clone();
    window.set_shape(4);
    window.set_axis(1);
    window.set_min_y("-16".into());
    window.set_max_y("16".into());
    window.set_min_z("-128".into());
    window.set_max_z("128".into());
    window.set_min_x("-128".into());
    window.set_max_x("128".into());
    window.invoke_edited();
    assert!(window.get_can_preview());
    window.invoke_preview_shape();
    assert!(!window.get_token().is_empty());
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).primitive_preview().unwrap().ids.len(), 12);
    window.invoke_focus_preview();
    assert!(lock(&shared).primitive_preview().is_some());
    save_ui_snapshot(window.window(), "brush-shapes");
    for label in [
        "Preview shape",
        "Focus preview",
        "Add shape",
        "Browse…",
        "Cancel",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_min_x("-".into());
    window.invoke_edited();
    assert!(!window.get_can_preview());
    assert!(window.get_token().is_empty());
    assert!(lock(&shared).primitive_preview().is_none());
    window.set_min_x("-128".into());
    window.invoke_edited();
    window.invoke_preview_shape();
    let old = window.get_token();
    let material_target = {
        let session = lock(&shared);
        (
            session.primitive_preview().map(|preview| preview.token),
            session.document_generation(),
        )
    };
    lock(&shared)
        .create_primitive(Actor::Agent, map_core::primitive::Spec::default(), true)
        .unwrap();
    assert!(
        primitive_ui::choose_material(
            &window,
            &mut lock(&shared),
            material_target,
            "brick/changed"
        )
        .is_err()
    );
    window.set_token(old);
    window.invoke_apply();
    assert!(window.get_message().contains("replaced"));
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(window.get_shape(), 0);
    window.invoke_cancel();
    assert!(lock(&shared).primitive_preview().is_none());
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.show().unwrap();
    window.set_shape(3);
    window.set_segments(12);
    window.set_bands(6);
    window.invoke_edited();
    window.invoke_preview_shape();
    window.invoke_apply();
    assert!(lock(&shared).primitive_preview().is_none());
    assert_eq!(lock(&shared).document().vmf().world.solids.len(), 1);
    assert_eq!(
        lock(&shared).document().vmf().world.solids[0].sides.len(),
        72
    );
    assert_eq!(lock(&shared).selected_ids().len(), 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
}

#[test]
pub(crate) fn design_context_controls_share_drafts_queries_regions_and_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            true,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let window = ContextWindow::new().unwrap();
    context_ui::bind(&window, &shared);
    context_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    let before = lock(&shared).document().vmf().clone();
    window.invoke_begin("map".into());
    window.set_summary("Courtyard lighting".into());
    window.set_body("A quiet service courtyard at dusk. Keep warm light near the entrance and cooler light along the escape route.".into());
    window.invoke_edit();
    assert!(window.get_can_apply());
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.invoke_apply();
    assert_eq!(window.get_rows().row_count(), 1);
    assert_eq!(
        lock(&shared)
            .get_context_note(map_core::context::Scope::Map)
            .unwrap()["note"]["author"],
        "human"
    );
    window.invoke_begin("new_region".into());
    window.set_summary("Keep the entrance readable".into());
    window.set_body("Leave the doorway clear and give it a warm pool of light.\n\nAvoid props above waist height along the approach. The player should see the exit from the center of the courtyard.".into());
    window.set_min_x("-128".into());
    window.set_min_y("-96".into());
    window.set_min_z("0".into());
    window.set_max_x("128".into());
    window.set_max_y("96".into());
    window.set_max_z("160".into());
    window.invoke_edit();
    assert!(window.get_can_apply());
    window.invoke_apply();
    window.invoke_begin("region:1".into());
    save_ui_snapshot(window.window(), "design-context");
    for label in [
        "Map brief",
        "Note on selection",
        "New region",
        "Focus in map",
        "Cancel edit",
        "Save note",
        "Delete region",
        "Cancel / close",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_focus_note();
    assert_eq!(lock(&shared).context_state()["focused_region"], 1);
    window.set_filter_mode(4);
    window.invoke_filter();
    assert_eq!(window.get_rows().row_count(), 2);
    window.set_filter_mode(5);
    window.set_query_min_x("10000".into());
    window.set_include_map(false);
    window.invoke_filter();
    assert_eq!(window.get_rows().row_count(), 0);
    window.set_query_min_x("0".into());
    window.invoke_filter();
    assert_eq!(window.get_rows().row_count(), 1);
    window.set_filter_mode(6);
    window.set_query("props".into());
    window.invoke_filter();
    assert_eq!(window.get_rows().row_count(), 1);
    window.set_query("".into());
    window.set_filter_mode(0);
    window.invoke_filter();
    let saved = lock(&shared).document().vmf().clone();
    window.set_min_x("unfinished".into());
    window.invoke_edit();
    assert!(!window.get_can_apply());
    window.invoke_cancel_edit();
    assert_eq!(lock(&shared).document().vmf(), &saved);
    window.invoke_begin("region:1".into());
    let old_token = window.get_token();
    lock(&shared)
        .begin_context_edit(Some(map_core::context::Scope::Map))
        .unwrap();
    window.set_token(old_token);
    window.invoke_apply();
    assert!(window.get_message().contains("replaced"));
    assert_eq!(lock(&shared).document().vmf(), &saved);
    window.invoke_cancel_edit();
    window.invoke_begin("region:1".into());
    window.invoke_delete_note();
    assert!(window.get_message().contains("Confirm"));
    window.set_confirm_delete(true);
    window.invoke_delete_note();
    assert_eq!(lock(&shared).context_state()["notes"], 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &saved);
    window.invoke_begin("map".into());
    window.set_body("Unapplied change".into());
    window.invoke_edit();
    window.invoke_cancel();
    assert!(lock(&shared).context_draft().is_none());
    assert_eq!(lock(&shared).document().vmf(), &saved);
}

#[test]
pub(crate) fn map_checks_controls_share_selection_repair_cancel_and_stale_guards() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let mut vmf = session.document().vmf().clone();
    vmf.world.solids[0].sides[0].u_axis.scale = 0.0;
    vmf.world.solids[0].sides[1].lightmap_scale = 0;
    let path = directory.path().join("courtyard.vmf");
    std::fs::write(
        &path,
        vmf.write(&source_formats::keyvalues::WriteOptions::default()),
    )
    .unwrap();
    session.open(Actor::User, &path, true).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let window = ChecksWindow::new().unwrap();
    checks_ui::bind(&window, &shared);
    checks_ui::refresh(&window, &lock(&shared));
    assert!(!window.get_can_repair());
    window.show().unwrap();
    window.invoke_run_checks();
    assert!(window.get_rows().row_count() >= 6);
    window.invoke_inspect("0".into());
    assert_eq!(window.get_detail_title(), "Invalid texture projection");
    assert!(window.get_can_select() && window.get_can_repair());
    window.invoke_navigate(true);
    assert_eq!(lock(&shared).selected_faces().len(), 1);
    let before = lock(&shared).document().vmf().clone();
    window.invoke_review_repair();
    assert!(!window.get_preview_token().is_empty());
    assert_eq!(lock(&shared).document().vmf(), &before);
    save_ui_snapshot(window.window(), "map-checks");
    for label in [
        "Run checks",
        "Select issue",
        "Focus issue",
        "Review repair…",
        "Cancel repair",
        "Apply repair",
        "Cancel / close",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.invoke_cancel_repair();
    assert!(lock(&shared).map_fix_draft().is_none());
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.invoke_review_repair();
    window.invoke_apply_repair();
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    assert_ne!(lock(&shared).document().vmf(), &before);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    checks_ui::refresh(&window, &lock(&shared));
    assert!(window.get_stale());
    assert!(!window.get_can_repair());
    window.invoke_run_checks();
    window.invoke_inspect("0".into());
    window.invoke_review_repair();
    let old = window.get_preview_token();
    let report = lock(&shared).map_check_report().unwrap().token;
    lock(&shared).preview_map_fixes(report, &[1]).unwrap();
    window.set_preview_token(old);
    window.invoke_apply_repair();
    assert!(window.get_message().contains("replaced"));
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.set_query("missing_team_spawn".into());
    window.invoke_filter();
    assert_eq!(window.get_rows().row_count(), 2);
    window.invoke_cancel();
    assert!(lock(&shared).map_fix_draft().is_none());
    assert_eq!(lock(&shared).document().vmf(), &before);
}

#[test]
pub(crate) fn recent_map_controls_share_reopen_and_guard_discard_remove_clear_and_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("courtyard.vmf");
    let second = directory.path().join("station.vmf");
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recent_maps(Ok(directory.path().join("preferences")));
    session.save(Actor::User, Some(&first)).unwrap();
    session
        .set_compile_settings(Actor::Agent, "direct_ldr")
        .unwrap();
    session.new_map(Actor::User, false).unwrap();
    session.save(Actor::User, Some(&second)).unwrap();
    session
        .set_compile_settings(Actor::Agent, "fullbright")
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let window = RecentMapsWindow::new().unwrap();
    recent_maps_ui::bind(&window, &shared);
    recent_maps_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    assert_eq!(window.get_rows().row_count(), 2);
    window.invoke_select_map(first.to_string_lossy().to_string().into());
    assert!(window.get_selected_detail().contains("direct_ldr"));
    save_ui_snapshot(window.window(), "recent-maps");
    for label in [
        "Refresh list",
        "Open selected map",
        "Remove from list",
        "Clear list",
        "Cancel / close",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    let before = lock(&shared).document().vmf().clone();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.show().unwrap();
    window.invoke_open_selected();
    assert_eq!(lock(&shared).path(), Some(first.as_path()));
    assert_eq!(
        lock(&shared).compile_options().lighting,
        compiler::LightingMode::DirectLdr
    );
    lock(&shared)
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    recent_maps_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    window.invoke_select_map(second.to_string_lossy().to_string().into());
    window.invoke_open_selected();
    assert!(window.get_message().contains("unsaved"));
    window.set_discard_changes(true);
    lock(&shared)
        .create_block(
            Actor::Agent,
            DVec3::splat(128.0),
            DVec3::splat(192.0),
            "brick/test",
            false,
        )
        .unwrap();
    let dirty = lock(&shared).document().vmf().clone();
    window.invoke_open_selected();
    assert!(window.get_message().contains("changed"));
    assert!(!window.get_discard_changes());
    assert_eq!(lock(&shared).document().vmf(), &dirty);
    window.set_discard_changes(true);
    window.invoke_open_selected();
    assert_eq!(lock(&shared).path(), Some(second.as_path()));
    assert_eq!(
        lock(&shared).compile_options().lighting,
        compiler::LightingMode::Fullbright
    );
    window.invoke_select_map(first.to_string_lossy().to_string().into());
    window.invoke_remove_selected();
    assert_eq!(window.get_rows().row_count(), 1);
    assert!(first.is_file());
    let clean = lock(&shared).document().vmf().clone();
    window.invoke_clear_list();
    assert_eq!(window.get_rows().row_count(), 1);
    window.set_confirm_clear(true);
    window.invoke_clear_list();
    assert_eq!(window.get_rows().row_count(), 0);
    assert_eq!(lock(&shared).document().vmf(), &clean);
    assert!(second.is_file());
}

#[test]
pub(crate) fn recovery_controls_guard_settings_and_restore_and_share_snapshot_operations() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .save(Actor::User, Some(&directory.path().join("courtyard.vmf")))
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let entry = recovery::capture(&shared, Actor::User, recovery::Kind::Manual).unwrap();
    lock(&shared)
        .create_block(
            Actor::Agent,
            DVec3::splat(128.0),
            DVec3::splat(192.0),
            "extra",
            false,
        )
        .unwrap();
    let window = RecoveryWindow::new().unwrap();
    recovery_ui::bind(&window, &shared);
    recovery_ui::refresh(&window, &lock(&shared));
    window.show().unwrap();
    window.invoke_select_snapshot(entry.id.clone().into());
    save_ui_snapshot(window.window(), "recovery");
    for label in [
        "Cancel / close",
        "Apply settings",
        "Save snapshot now",
        "Open recovered copy",
        "Delete snapshot",
    ] {
        assert_dialog_button_visible(&window, label);
    }
    window.set_interval_seconds(60);
    window.set_settings_dirty(true);
    window.invoke_apply_settings();
    assert_eq!(lock(&shared).recovery_settings().interval_seconds, 60);
    window.set_keep_count(3);
    window.set_settings_dirty(true);
    let newer = recovery::Settings {
        enabled: false,
        interval_seconds: 90,
        keep_per_map: 7,
    };
    lock(&shared)
        .set_recovery_settings(Actor::Agent, newer.clone(), None)
        .unwrap();
    window.invoke_apply_settings();
    assert!(window.get_message().contains("changed elsewhere"));
    assert_eq!(lock(&shared).recovery_settings(), &newer);
    window.invoke_reload_settings();
    assert_eq!(window.get_keep_count(), 7);
    let before = lock(&shared).document().vmf().clone();
    window.invoke_restore_snapshot();
    assert!(window.get_message().contains("unsaved changes"));
    assert_eq!(lock(&shared).document().vmf(), &before);
    window.set_discard_changes(true);
    lock(&shared)
        .create_block(
            Actor::Agent,
            DVec3::splat(256.0),
            DVec3::splat(320.0),
            "extra",
            false,
        )
        .unwrap();
    window.invoke_restore_snapshot();
    assert!(window.get_message().contains("Map changed"));
    assert!(!window.get_discard_changes());
    window.set_discard_changes(true);
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &window,
        "Open recovered copy",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert_eq!(lock(&shared).document().objects().count(), 1);
    {
        let session = lock(&shared);
        assert!(session.document().is_dirty() && session.path().is_none());
    }
    window.show().unwrap();
    window.invoke_select_snapshot(entry.id.clone().into());
    window.invoke_delete_snapshot();
    assert_eq!(lock(&shared).recovery_entries().unwrap().entries.len(), 1);
    window.set_confirm_delete(true);
    window.invoke_delete_snapshot();
    assert!(lock(&shared).recovery_entries().unwrap().entries.is_empty());
    window.invoke_snapshot_now();
    for _ in 0..200 {
        if !lock(&shared).recovery_busy() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!lock(&shared).recovery_busy());
    assert_eq!(lock(&shared).recovery_entries().unwrap().entries.len(), 1);
    window.set_settings_dirty(true);
    window.set_interval_seconds(300);
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).recovery_settings().interval_seconds, 90);
}

pub(crate) fn scroll_inspector_to_control(
    app: &AppWindow,
    label: &str,
) -> i_slint_backend_testing::ElementHandle {
    let size = app.window().size();
    let scale = app.window().scale_factor();
    let width = size.width as f32 / scale;
    let height = size.height as f32 / scale;
    let position = slint::LogicalPosition::new(width - 180.0, height * 0.5);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: 10_000.0,
        });
    for _ in 0..32 {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        if let Some(control) =
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(app, label).next()
        {
            let top = control.absolute_position().y;
            let bottom = top + control.size().height;
            if top >= 44.0 && bottom <= height - 32.0 {
                return control;
            }
        }
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: -160.0,
            });
    }
    panic!("Inspector control {label:?} cannot be scrolled into view");
}

pub(crate) fn assert_dialog_button_visible<C: ComponentHandle>(component: &C, label: &str) {
    let button = i_slint_backend_testing::ElementHandle::find_by_accessible_label(component, label)
        .next()
        .unwrap();
    let position = button.absolute_position();
    let size = button.size();
    let window = component.window().size();
    assert!(size.width > 0.0 && size.height > 0.0);
    assert!(position.x >= 0.0 && position.y >= 0.0);
    assert!(
        position.x + size.width <= window.width as f32 + 1.0,
        "{label} outside window"
    );
    assert!(
        position.y + size.height <= window.height as f32 + 1.0,
        "{label} outside window"
    );
}

#[test]
#[cfg(unix)]
pub(crate) fn failed_material_discovery_clears_stale_choices_and_can_be_retried() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let window = MaterialWindow::new().unwrap();
    window.set_rows(ModelRc::new(VecModel::from(vec![MaterialTileRow {
        tiles: ModelRc::new(VecModel::from(vec![MaterialTile {
            name: "stale/material".into(),
            ..Default::default()
        }])),
    }])));
    window.set_available(true);
    let bad = directory.path().join("materials/loop.vmt");
    std::os::unix::fs::symlink("loop.vmt", &bad).unwrap();
    refresh_material_catalog(&window, &mut session);
    assert_eq!(window.get_rows().row_count(), 0);
    assert!(!window.get_available());
    assert_eq!(window.get_catalog_label(), "Material catalog unavailable");
    assert!(window.get_detail().contains("loop.vmt"));
    std::fs::remove_file(bad).unwrap();
    refresh_material_catalog(&window, &mut session);
    assert_eq!(window.get_total(), 1);
    assert_eq!(
        window
            .get_rows()
            .row_data(0)
            .unwrap()
            .tiles
            .row_data(0)
            .unwrap()
            .name,
        "test/checker"
    );
}

#[test]
pub(crate) fn material_catalog_and_texture_dialog_render_without_desktop_input() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let material = MaterialWindow::new().unwrap();
    let mut session = Session::new(editor_core::startup::mount_game(None));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "dev/dev_measuregeneric01",
            false,
        )
        .unwrap();
    session
        .favorite_material("brick/brickwall001a", true)
        .unwrap();
    material.set_material("dev/dev_measuregeneric01".into());
    refresh_material_catalog(&material, &mut session);
    refresh_material_window(&material, &mut session);
    material.show().unwrap();
    save_ui_snapshot(material.window(), "material-browser");
    for label in ["Cancel", "Use material", "Next"] {
        assert_dialog_button_visible(&material, label);
    }
    assert!(material.get_total() > 0);
    assert_eq!(
        material.get_favorites_status(),
        "Favorites last for this session"
    );
    let concrete = session
        .material_catalog("", "category:Concrete", 0, 36)
        .unwrap();
    if let Some(first) = concrete.entries.first() {
        material.set_group("category:Concrete".into());
        material.set_material(first.name.clone().into());
        refresh_material_catalog(&material, &mut session);
        refresh_material_window(&material, &mut session);
        save_ui_snapshot(material.window(), "materials-grouped");
        let preview = i_slint_backend_testing::ElementHandle::find_by_element_id(
            &material,
            "MaterialWindow::preview-image",
        )
        .next()
        .unwrap();
        assert!(
            preview.absolute_position().x + preview.size().width
                <= material.window().size().width as f32
        );
    }
    material.set_group("used".into());
    refresh_material_catalog(&material, &mut session);
    assert_eq!(material.get_total(), 1);
    let cancelled = Rc::new(std::cell::Cell::new(false));
    material.on_cancel({
        let cancelled = cancelled.clone();
        move || cancelled.set(true)
    });
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&material, "Cancel")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(cancelled.get());
    let favorites_directory = tempfile::tempdir().unwrap();
    session.configure_recovery(Ok(favorites_directory.path().join("recovery")));
    session.favorite_material("brick/wall", true).unwrap();
    material.set_group("all".into());
    material.set_offset(0);
    material.set_material("dev/dev_measuregeneric01".into());
    refresh_material_catalog(&material, &mut session);
    refresh_material_window(&material, &mut session);
    assert_eq!(
        material.get_favorites_status(),
        "Favorites saved across sessions"
    );
    save_ui_snapshot(material.window(), "material-favorites-persistent");
    let favorite_path = favorites_directory
        .path()
        .join("recovery")
        .join(material_favorites::FILE_NAME);
    let mut permissions = std::fs::metadata(&favorite_path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&favorite_path, permissions).unwrap();
    assert!(session.favorite_material("wood/floor", true).is_err());
    refresh_material_catalog(&material, &mut session);
    refresh_material_window(&material, &mut session);
    assert!(material.get_favorites_status().contains("could not save"));
    save_ui_snapshot(material.window(), "material-favorites-error");
    let status = i_slint_backend_testing::ElementHandle::find_by_element_id(
        &material,
        "MaterialWindow::favorites-state",
    )
    .next()
    .unwrap();
    assert!(
        status.absolute_position().y + status.size().height
            <= material.window().size().height as f32
    );
    material.hide().unwrap();
    let surface = SurfaceWindow::new().unwrap();
    surface.set_selection_label("Textures on 3 selected faces".into());
    surface.set_material("dev/dev_measuregeneric01".into());
    surface.set_preview(
        session
            .material_preview("dev/dev_measuregeneric01")
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    surface.set_scale_u("0.25".into());
    surface.set_scale_v("0.25".into());
    surface.set_shift_u("0".into());
    surface.set_shift_v("0".into());
    surface.set_rotation("0".into());
    surface.set_message("Preview changes before applying, or Cancel to discard.".into());
    surface.show().unwrap();
    save_ui_snapshot(surface.window(), "face-textures");
    for label in ["Cancel", "Preview textures", "Apply textures"] {
        assert_dialog_button_visible(&surface, label);
    }
    surface.set_operation_index(3);
    surface.set_sample_present(true);
    surface.set_sample_label("dev/dev_measuregeneric01 · brush 2, face 6".into());
    surface.set_sample_preview(surface.get_preview());
    save_ui_snapshot(surface.window(), "face-wrap");
    surface.set_operation_index(4);
    save_ui_snapshot(surface.window(), "face-chain");
    for label in ["Cancel", "Preview textures", "Apply textures"] {
        assert_dialog_button_visible(&surface, label);
    }
    surface.hide().unwrap();
}

#[test]
pub(crate) fn texture_dialog_paints_sampled_material_and_preview_matches_apply() {
    i_slint_backend_testing::init_no_event_loop();
    let window = SurfaceWindow::new().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/source",
            false,
        )
        .unwrap()
    else {
        panic!()
    };
    let faces: Vec<_> = session
        .document()
        .solid(id)
        .unwrap()
        .sides
        .iter()
        .map(|side| map_core::surface::FaceId {
            solid: id,
            side: side.id,
        })
        .collect();
    session
        .edit_faces(
            Actor::Agent,
            &[faces[0]],
            map_core::surface::SurfaceEdit {
                material: Some("metal/sample".into()),
                ..Default::default()
            },
            false,
            false,
        )
        .unwrap();
    session.sample_face_texture(faces[0]).unwrap();
    session
        .select_faces(&[faces[1]], SelectMode::Replace)
        .unwrap();
    let target = (
        vec![faces[1]],
        session.document_generation(),
        session.document().revision(),
    );
    refresh_surface_sample(&window, &mut session);
    assert!(window.get_sample_present());
    assert!(window.get_sample_label().contains("metal/sample"));
    window.set_operation_index(1);
    let original = session.document().vmf().clone();
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    assert_eq!(session.document().vmf(), &original);
    let preview = session.face_preview_document().unwrap().vmf().clone();
    apply_surface_dialog(&window, &mut session, &target, false).unwrap();
    assert_eq!(session.document().vmf(), &preview);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &original);
    assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
    let targets = session
        .document()
        .solid(id)
        .unwrap()
        .sides
        .iter()
        .filter(|side| {
            let normal = map_core::Plane::from_points(side.plane.0).unwrap().normal;
            normal == DVec3::NEG_Y || normal == DVec3::NEG_Z
        })
        .map(|side| map_core::surface::FaceId {
            solid: id,
            side: side.id,
        })
        .collect::<Vec<_>>();
    session.select_faces(&targets, SelectMode::Replace).unwrap();
    let target = (
        session.selected_faces(),
        session.document_generation(),
        session.document().revision(),
    );
    window.set_operation_index(4);
    assert_eq!(
        apply_surface_dialog(&window, &mut session, &target, true).unwrap(),
        2
    );
    let preview = session.face_preview_document().unwrap().vmf().clone();
    session.cancel_face_preview();
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(
        apply_surface_dialog(&window, &mut session, &target, false).unwrap(),
        2
    );
    assert_eq!(session.document().vmf(), &preview);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &original);
    window.set_alignment_index(2);
    window.set_justify_u_index(1);
    window.set_justify_v_index(3);
    let patch = surface_patch(&window).unwrap();
    assert_eq!(patch.alignment, Some(map_core::surface::Alignment::Face));
    assert_eq!(
        patch.justify_u,
        Some((map_core::surface::Justify::Start, 0))
    );
    assert_eq!(patch.justify_v, Some((map_core::surface::Justify::End, 0)));
}

#[test]
pub(crate) fn texture_dialog_buttons_preview_cancel_apply_and_reject_stale_selections() {
    i_slint_backend_testing::init_no_event_loop();
    let window = SurfaceWindow::new().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(id) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/wall",
            false,
        )
        .unwrap()
    else {
        panic!()
    };
    let face = map_core::surface::FaceId {
        solid: id,
        side: session.document().solid(id).unwrap().sides[0].id,
    };
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let target = (
        vec![face],
        session.document_generation(),
        session.document().revision(),
    );
    let before = session.document().vmf().clone();
    let session = Rc::new(RefCell::new(session));
    window.set_material("wood/floor".into());
    window.set_scale_u("0.5".into());
    window.set_rotation("45".into());
    window.on_preview_edit({
        let (session, window, target) = (session.clone(), window.as_weak(), target.clone());
        move || {
            let window = window.unwrap();
            if let Err(error) =
                apply_surface_dialog(&window, &mut session.borrow_mut(), &target, true)
            {
                window.set_message(error.into());
            }
        }
    });
    window.on_apply({
        let (session, window, target) = (session.clone(), window.as_weak(), target.clone());
        move || {
            let window = window.unwrap();
            if let Err(error) =
                apply_surface_dialog(&window, &mut session.borrow_mut(), &target, false)
            {
                window.set_message(error.into());
            }
        }
    });
    window.on_cancel({
        let session = session.clone();
        move || session.borrow_mut().cancel_face_preview()
    });
    window.show().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Preview textures")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(session.borrow().has_face_preview());
    assert_eq!(session.borrow().document().vmf(), &before);
    session
        .borrow_mut()
        .edit_faces(
            Actor::Agent,
            &[face],
            map_core::surface::SurfaceEdit {
                rotation: Some(90.0),
                shift_u: Some(64.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    refresh_surface_dialog(&window, &mut session.borrow_mut(), &target);
    assert_eq!(window.get_rotation(), "90");
    assert_eq!(window.get_shift_u(), "64");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Cancel")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!session.borrow().has_face_preview());
    assert_eq!(session.borrow().document().vmf(), &before);
    window.set_material("wood/floor".into());
    window.invoke_preview_edit();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    assert!(!session.borrow().has_face_preview());
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Apply textures")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(
        map_core::surface::side(session.borrow().document(), face)
            .unwrap()
            .material,
        "WOOD/FLOOR"
    );
    session.borrow_mut().undo(Actor::Agent).unwrap();
    assert_eq!(session.borrow().document().vmf(), &before);
    window.invoke_apply();
    assert!(window.get_message().contains("changed"));
    refresh_surface_dialog(&window, &mut session.borrow_mut(), &target);
    assert!(window.get_stale());
    window.hide().unwrap();
}

use slint::Model;
use source_formats::vmf::Entity;

#[test]
pub(crate) fn replacement_transform_drafts_display_shared_operations_without_editing_source() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let id = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    state
        .set_transform_tool(Actor::User, Default::default())
        .unwrap();
    let before = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    app.set_has_selection(true);
    app.set_can_apply_material(true);
    app.set_show_transform(true);
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    let shear = map_core::transform::Shear {
        axis: 0,
        reference: 1,
        factor: 0.5,
    };
    for previous in [3, 4] {
        for (mode, spec, values) in [
            (
                0,
                edit::Transform {
                    translation: DVec3::new(12.125, -7.25, 3.5),
                    ..Default::default()
                },
                ["12.125", "-7.25", "3.5"],
            ),
            (
                1,
                edit::Transform {
                    rotation: DVec3::new(17.0, -23.0, 31.0),
                    ..Default::default()
                },
                ["17", "-23", "31"],
            ),
            (
                2,
                edit::Transform {
                    scale: DVec3::new(1.25, 0.75, 1.5),
                    ..Default::default()
                },
                ["1.25", "0.75", "1.5"],
            ),
            (
                0,
                edit::Transform {
                    shear: Some(map_core::transform::Shear {
                        factor: 0.0,
                        ..shear
                    }),
                    ..Default::default()
                },
                ["0", "0", "0"],
            ),
        ] {
            state
                .preview_transform(
                    Actor::Agent,
                    &[id],
                    edit::Transform {
                        reflection: [previous == 3, false, false],
                        shear: (previous == 4).then_some(shear),
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            refresh_interaction(&app, &state);
            save_ui_snapshot(app.window(), "transform-before-replacement");
            assert_eq!(app.get_transform_mode(), previous);
            state
                .preview_transform(Actor::Agent, &[id], spec, None)
                .unwrap();
            let expected = state.transform_preview().unwrap().document.vmf().clone();
            let summary = state.transform_preview().unwrap().presentation().summary;
            refresh_interaction(&app, &state);
            save_ui_snapshot(
                app.window(),
                &format!("transform-replacement-{previous}-{mode}"),
            );
            assert_eq!(app.get_transform_mode(), mode);
            assert_eq!(
                [
                    app.get_transform_x(),
                    app.get_transform_y(),
                    app.get_transform_z()
                ],
                values.map(SharedString::from)
            );
            assert_eq!(app.get_transform_preview_summary(), summary);
            assert_eq!(state.transform_tool().mode, None);
            assert_eq!(state.document().vmf(), &before);
            assert_eq!(state.document().undo_labels().count(), history);
            state.cancel_transform_preview();
            refresh_interaction(&app, &state);
            assert_eq!(state.document().vmf(), &before);
            assert_eq!(app.get_transform_preview_summary(), "");
            state
                .preview_transform(Actor::Agent, &[id], spec, None)
                .unwrap();
            state.commit_transform_preview(Actor::User).unwrap();
            assert_eq!(state.document().vmf(), &expected);
            if expected != before {
                assert_eq!(state.document().undo_labels().count(), history + 1);
                state.undo(Actor::Agent).unwrap();
            }
            assert_eq!(state.document().vmf(), &before);
        }
    }
    state
        .preview_transform(
            Actor::Agent,
            &[id],
            edit::Transform {
                translation: DVec3::new(8.0, 0.0, 0.0),
                rotation: DVec3::new(0.0, 0.0, 15.0),
                scale: DVec3::new(1.25, 1.0, 1.0),
                reflection: [true, false, false],
                shear: Some(shear),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let presentation = state.transform_preview().unwrap().presentation();
    assert_eq!(
        presentation
            .operations
            .iter()
            .map(|op| op.name())
            .collect::<Vec<_>>(),
        ["move", "rotate", "scale", "mirror", "shear"]
    );
    refresh_interaction(&app, &state);
    save_ui_snapshot(app.window(), "transform-compound-summary");
    assert_eq!(app.get_transform_mode(), 4);
    assert_eq!(app.get_transform_preview_summary(), presentation.summary);
    for label in [
        "Move: 8, 0, 0 units",
        "Rotate: 0, 0, 15 degrees",
        "Scale: 1.25, 1, 1 factors",
        "Mirror: X",
        "Shear: X by Y × 0.5",
    ] {
        assert!(app.get_transform_preview_summary().contains(label));
    }
    assert_dialog_button_visible(&app, "Apply transform");
    assert_dialog_button_visible(&app, "Cancel");
    assert_eq!(state.document().vmf(), &before);
    assert_eq!(state.document().undo_labels().count(), history);
}

#[test]
pub(crate) fn shear_controls_share_numeric_apply_and_agent_draft_values() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let id = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let before = state.document().vmf().clone();
    app.set_has_selection(true);
    app.set_can_apply_material(true);
    app.set_show_transform(true);
    app.set_transform_mode(4);
    app.set_transform_shear_axis(0);
    app.set_transform_shear_reference(1);
    app.set_transform_shear_factor("0.5".into());
    app.set_transform_pivot_mode(1);
    apply_selection_transform(&app, &mut state).unwrap();
    let bounds = map_core::query::bounds_of(state.document(), &[id]).unwrap();
    assert!(bounds.1.abs_diff_eq(DVec3::new(96.0, 64.0, 64.0), 1e-8));
    state.undo(Actor::User).unwrap();
    assert_eq!(state.document().vmf(), &before);
    state
        .preview_transform(
            Actor::Agent,
            &[id],
            edit::Transform {
                shear: Some(map_core::transform::Shear {
                    axis: 2,
                    reference: 0,
                    factor: -0.75,
                }),
                texture_scale_lock: Some(false),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    refresh_interaction(&app, &state);
    app.show().unwrap();
    scroll_inspector_to_control(&app, "Keep textures attached while shearing");
    save_ui_snapshot(app.window(), "brush-shear");
    assert_eq!(app.get_transform_mode(), 4);
    assert_eq!(app.get_transform_shear_axis(), 2);
    assert_eq!(app.get_transform_shear_reference(), 0);
    assert_eq!(app.get_transform_shear_factor(), "-0.75");
    assert_dialog_button_visible(&app, "Keep textures attached while shearing");
    state.cancel_transform_preview();
    assert_eq!(state.document().vmf(), &before);
}

#[test]
pub(crate) fn terrain_creation_controls_use_shared_conversion_and_explain_existing_grids() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let side = state
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let target = map_core::surface::FaceId { solid, side };
    state
        .select_faces(&[target], session::SelectMode::Replace)
        .unwrap();
    let before = state.document().vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    bind_terrain_controls(&app, shared.clone());
    app.set_has_faces(true);
    app.set_face_mode(true);
    app.set_surface_view(2);
    app.set_can_apply_material(true);
    app.set_material("brick/test".into());
    if let Some(frame) = std::env::var_os("HAMMERR_TERRAIN_FRAME") {
        app.set_frame(slint::Image::load_from_path(Path::new(&frame)).unwrap());
    }
    app.window()
        .set_size(slint::LogicalSize::new(1100.0, 800.0));
    app.show().unwrap();
    refresh_terrain_controls(&app, &lock(&shared));
    app.set_selection_title("1 face selected".into());
    save_ui_snapshot(app.window(), "terrain-creation");
    let corners = lock(&shared).terrain_creation_faces(&[target]).unwrap()[0].corners;
    app.invoke_create_terrain(4, 3);
    assert_eq!(
        lock(&shared)
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| side.id == target.side)
            .unwrap()
            .displacement
            .as_ref()
            .unwrap()
            .start_position,
        corners[3]
    );
    assert!(app.get_message().contains("Created 1 terrain surface"));
    assert!(
        app.get_terrain_creation_error()
            .contains("already has terrain")
    );
    let after = lock(&shared).document().vmf().clone();
    app.invoke_create_terrain(2, 0);
    assert_eq!(*lock(&shared).document().vmf(), after);
    assert!(app.get_can_remove_terrain());
    app.invoke_remove_terrain();
    assert!(app.get_message().contains("Removed 1 terrain surface"));
    assert!(!app.get_can_remove_terrain());
    assert_eq!(*lock(&shared).document().vmf(), before);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), after);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), before);
    app.hide().unwrap();
}

#[test]
pub(crate) fn terrain_resolution_controls_keep_cancel_and_guard_foreign_previews() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let target = map_core::surface::FaceId {
        solid,
        side: session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    };
    session
        .select_faces(&[target], session::SelectMode::Replace)
        .unwrap();
    session
        .create_terrain(Actor::Agent, &[target], 2, 3)
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-resolution");
    assert!(!window.get_ready());
    window.set_grid_index(2);
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(*lock(&shared).document().vmf(), before);
    assert_eq!(
        lock(&shared).terrain_grids(&[target], true).unwrap()[0].power,
        4
    );
    window.invoke_cancel();
    assert!(!lock(&shared).has_face_preview());
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_grid_index(1);
    window.invoke_preview_edit();
    lock(&shared)
        .preview_terrain_resolution(Actor::Agent, &[target], 4)
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    controller.refresh(&lock(&shared));
    assert!(!window.get_ready());
    window.invoke_apply();
    assert_eq!(*lock(&shared).document().vmf(), before);
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_grid_index(1);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(
        lock(&shared).terrain_grids(&[target], false).unwrap()[0].power,
        3
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.invoke_preview_edit();
    lock(&shared)
        .remove_terrain(Actor::Agent, &[target])
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(window.get_stale());
    window.invoke_apply();
    assert!(lock(&shared).terrain_grids(&[target], false).is_err());
}

#[test]
pub(crate) fn terrain_sculpt_and_blend_controls_drive_shared_previews() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(-64.0),
            glam::DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let target = map_core::surface::FaceId {
        solid,
        side: session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    };
    session
        .select_faces(&[target], session::SelectMode::Replace)
        .unwrap();
    session
        .create_terrain(Actor::Agent, &[target], 2, 0)
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    window.set_operation_index(1);
    window.set_radius("32".into());
    window.set_amount("16".into());
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-sculpt");
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert_eq!(
        lock(&shared).terrain_grids(&[target], true).unwrap()[0]
            .mesh
            .positions[12]
            .z,
        80.0
    );
    window.invoke_apply();
    assert_eq!(
        lock(&shared).terrain_grids(&[target], false).unwrap()[0]
            .mesh
            .positions[12]
            .z,
        80.0
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.initialize(&mut lock(&shared)).unwrap();
    for mode in [2, 3, 4, 5, 6, 7, 12] {
        window.set_operation_index(mode);
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(window.get_ready(), "mode {mode}: {}", window.get_message());
        if mode == 12 {
            assert!(lock(&shared).terrain_grids(&[target], true).unwrap()[0].subdivided);
            window.show().unwrap();
            save_ui_snapshot(window.window(), "terrain-subdivision-controls");
        }
        window.invoke_cancel();
        assert_eq!(*lock(&shared).document().vmf(), before);
    }
    window.set_operation_index(5);
    window.show().unwrap();
    window.invoke_edited();
    save_ui_snapshot(window.window(), "terrain-blend");
    window.invoke_preview_edit();
    window.set_blend_value("256".into());
    window.invoke_edited();
    assert!(!lock(&shared).has_face_preview());
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(window.get_message().contains("0–255"));
    window.invoke_cancel();
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(1);
    window.set_radius("32".into());
    window.set_amount("4".into());
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-stroke-controls");
    window.invoke_draw_stroke();
    assert!(controller.is_stroking(), "{}", window.get_message());
    for x in [-32.0, 0.0, 32.0] {
        controller
            .update_stroke(
                &mut lock(&shared),
                &Ray {
                    origin: DVec3::new(x, 0.0, 128.0),
                    direction: -DVec3::Z,
                },
            )
            .unwrap();
    }
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.finish_stroke(&mut lock(&shared)).unwrap();
    assert!(window.get_ready());
    assert!(!controller.is_stroking());
    window.invoke_apply();
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(5);
    window.set_blend_value("200".into());
    window.invoke_draw_stroke();
    controller
        .update_stroke(
            &mut lock(&shared),
            &Ray {
                origin: DVec3::new(0.0, 0.0, 128.0),
                direction: -DVec3::Z,
            },
        )
        .unwrap();
    controller.cancel_stroke(&mut lock(&shared));
    assert!(!lock(&shared).has_face_preview());
    assert!(!controller.is_stroking());
    assert_eq!(*lock(&shared).document().vmf(), before);
}

#[test]
pub(crate) fn terrain_collision_and_sewing_controls_use_shared_owned_previews() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let mut faces = Vec::new();
    for x in [-64.0, 0.0] {
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 64.0, 64.0, 16.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let face = map_core::surface::FaceId {
            solid,
            side: session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        };
        session
            .create_terrain(Actor::Agent, &[face], 2, faces.len() as u32)
            .unwrap();
        faces.push(face);
    }
    session
        .select_faces(&faces, session::SelectMode::Replace)
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    window.set_operation_index(8);
    window.set_physics_index(2);
    window.set_player_index(2);
    window.set_ray_index(2);
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-collision");
    assert_eq!(window.get_physics_current(), "enabled");
    window.invoke_preview_edit();
    assert!(window.get_ready());
    assert!(
        lock(&shared)
            .terrain_grids(&faces, true)
            .unwrap()
            .iter()
            .all(|grid| grid.flags == 14)
    );
    window.invoke_cancel();
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(9);
    window.set_sew_blend(true);
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-sewing");
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    window.invoke_apply();
    assert_eq!(*lock(&shared).document().vmf(), before);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(10);
    window.set_smoothing_index(2);
    window.set_smoothing_angle("45.5".into());
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-border-smoothing");
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    assert!(
        lock(&shared)
            .terrain_grids(&faces, true)
            .unwrap()
            .iter()
            .all(|grid| grid.smoothing_angle == Some(45.5))
    );
    window.invoke_cancel();
    assert_eq!(*lock(&shared).document().vmf(), before);
}

#[test]
pub(crate) fn terrain_triangle_controls_preserve_owned_cancel_and_apply_subsets() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
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
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId { solid, side };
    session.create_terrain(Actor::Agent, &[face], 2, 0).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_triangles_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    assert_eq!(window.get_triangles().row_count(), 32);
    window.invoke_toggle_triangle(0, true);
    window.invoke_toggle_triangle(7, true);
    window.set_walkable_index(3);
    window.set_buildable_index(2);
    window.invoke_edited();
    window.show().unwrap();
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    assert!(
        window
            .get_triangles()
            .row_data(0)
            .unwrap()
            .detail
            .contains("no · forced")
    );
    save_ui_snapshot(window.window(), "terrain-triangle-tags");
    assert_eq!(*lock(&shared).document().vmf(), source);
    assert!(!lock(&shared).terrain_triangles(&[face], true).unwrap()[0].triangles[0].walkable);
    window.invoke_cancel();
    assert!(!lock(&shared).has_face_preview());
    controller.initialize(&mut lock(&shared)).unwrap();
    window.invoke_toggle_triangle(0, true);
    window.set_walkable_index(3);
    window.invoke_preview_edit();
    lock(&shared)
        .preview_terrain_resolution(Actor::Agent, &[face], 3)
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    controller.refresh(&lock(&shared));
    assert!(!window.get_ready());
    window.invoke_apply();
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.invoke_toggle_triangle(0, true);
    window.set_walkable_index(3);
    window.invoke_preview_edit();
    window.invoke_apply();
    let rows = lock(&shared).terrain_triangles(&[face], false).unwrap();
    assert!(!rows[0].triangles[0].walkable);
    assert!(rows[0].triangles[1].walkable);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), source);
}

#[test]
pub(crate) fn terrain_triangle_batch_controls_pick_frame_and_keep_foreign_guides() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let mut faces = Vec::new();
    for (index, power) in [2, 3].into_iter().enumerate() {
        let low = DVec3::X * index as f64 * 96.0;
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                low,
                low + DVec3::new(64.0, 64.0, 16.0),
                "brick/test",
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
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let face = map_core::surface::FaceId { solid, side };
        session
            .create_terrain(Actor::Agent, &[face], power, index as u32)
            .unwrap();
        faces.push(face);
    }
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_triangles_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    window.invoke_toggle_triangle(0, true);
    window.invoke_toggle_triangle(7, true);
    window.set_face_index(1);
    window.invoke_face_changed();
    window.invoke_toggle_triangle(31, true);
    window.set_face_index(0);
    window.invoke_face_changed();
    assert!(window.get_triangles().row_data(0).unwrap().selected);
    assert!(
        window
            .get_selection_label()
            .contains("3 triangles across 2")
    );
    window.set_walkable_index(3);
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    assert_eq!(
        lock(&shared).terrain_triangles(&faces, true).unwrap()[1].triangles[31].walkable_override,
        Some(false)
    );
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-triangle-picking");
    let camera = lock(&shared).camera();
    window.invoke_frame_selected();
    assert_ne!(lock(&shared).camera(), camera);
    let preview = lock(&shared).face_preview_token();
    window.invoke_pick_triangle();
    assert!(controller.is_picking());
    controller.cancel_pick();
    assert!(!controller.is_picking());
    assert_eq!(lock(&shared).face_preview_token(), preview);
    window.invoke_pick_triangle();
    let center =
        lock(&shared).terrain_triangles(&[faces[1]], true).unwrap()[0].triangles[127].centroid;
    assert_eq!(
        controller
            .select_at_ray(
                &mut lock(&shared),
                &Ray {
                    origin: center + DVec3::Z * 64.0,
                    direction: DVec3::NEG_Z
                }
            )
            .unwrap(),
        127
    );
    assert_eq!(window.get_face_index(), 1);
    assert!(window.get_triangles().row_data(31).unwrap().selected);
    assert!(window.get_triangles().row_data(127).unwrap().selected);
    window.invoke_preview_edit();
    window.invoke_apply();
    let rows = lock(&shared).terrain_triangles(&faces, false).unwrap();
    assert!(!rows[0].triangles[0].walkable && !rows[1].triangles[127].walkable);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), source);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.invoke_toggle_triangle(0, true);
    let foreign = lock(&shared)
        .highlight_terrain_triangles(Actor::Agent, &[(faces[1], vec![7])])
        .unwrap();
    window.invoke_cancel();
    assert_eq!(
        lock(&shared).terrain_triangle_highlight_json()["token"],
        foreign.to_string()
    );
    assert_eq!(*lock(&shared).document().vmf(), source);
}

#[test]
pub(crate) fn terrain_mask_controls_retain_selection_and_confine_sculpt_and_blend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::new(64.0, 64.0, 16.0),
            "brick/test",
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
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId { solid, side };
    session.create_terrain(Actor::Agent, &[face], 2, 0).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let picker = terrain_triangles_ui::Controller::new(&shared).unwrap();
    picker.initialize(&mut lock(&shared)).unwrap();
    picker.window.invoke_toggle_triangle(0, true);
    picker.window.invoke_use_selection();
    let mask = lock(&shared).terrain_vertex_mask(&[face], None).unwrap();
    assert_eq!(mask.faces[0].vertices, [0, 5, 6]);
    let controller = terrain_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    assert!(window.get_mask_available());
    window.set_operation_index(1);
    window.set_use_mask(true);
    window.set_radius("8192".into());
    window.set_amount("16".into());
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-vertex-mask");
    let before = lock(&shared)
        .terrain_grids(&[face], false)
        .unwrap()
        .remove(0)
        .mesh;
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    let draft = lock(&shared)
        .terrain_grids(&[face], true)
        .unwrap()
        .remove(0)
        .mesh;
    for (index, (old, new)) in before.positions.iter().zip(&draft.positions).enumerate() {
        assert_eq!(old != new, mask.faces[0].vertices.contains(&index));
    }
    window.invoke_apply();
    assert!(
        lock(&shared)
            .terrain_vertex_mask(&[face], Some(mask.token))
            .is_ok()
    );
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(6);
    window.set_use_mask(true);
    window.set_blend_value("200".into());
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    let alpha = lock(&shared)
        .terrain_grids(&[face], true)
        .unwrap()
        .remove(0)
        .mesh
        .alphas;
    assert_eq!(alpha.iter().filter(|&&value| value == 200.0).count(), 3);
    window.invoke_cancel();
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), source);
    picker.initialize(&mut lock(&shared)).unwrap();
    picker.window.set_kind_index(1);
    picker.window.invoke_kind_changed();
    picker.window.invoke_toggle_triangle(12, true);
    picker.window.show().unwrap();
    save_ui_snapshot(picker.window.window(), "terrain-explicit-vertices");
    picker.window.invoke_frame_selected();
    picker.window.invoke_pick_triangle();
    let point = before.positions[12];
    assert_eq!(
        picker
            .select_at_ray(
                &mut lock(&shared),
                &Ray {
                    origin: point + DVec3::new(0.01, 0.01, 64.0),
                    direction: -DVec3::Z
                }
            )
            .unwrap(),
        12
    );
    picker.window.invoke_use_selection();
    let mask = lock(&shared).terrain_vertex_mask(&[face], None).unwrap();
    assert_eq!(mask.faces[0].vertices, [12]);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(1);
    window.set_use_mask(true);
    window.set_radius("8192".into());
    window.set_amount("16".into());
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    let draft = lock(&shared)
        .terrain_grids(&[face], true)
        .unwrap()
        .remove(0)
        .mesh;
    for (index, (a, b)) in before.positions.iter().zip(&draft.positions).enumerate() {
        assert_eq!(a != b, index == 12);
    }
    window.invoke_cancel();
    assert_eq!(*lock(&shared).document().vmf(), source);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(11);
    window.set_vertex_x("3.5".into());
    window.set_vertex_y("-2".into());
    window.set_vertex_z("12".into());
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-vertex-move");
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    let draft = lock(&shared)
        .terrain_grids(&[face], true)
        .unwrap()
        .remove(0)
        .mesh;
    for (index, (a, b)) in before.positions.iter().zip(&draft.positions).enumerate() {
        assert_eq!(
            *b,
            *a + if index == 12 {
                DVec3::new(3.5, -2.0, 12.0)
            } else {
                DVec3::ZERO
            }
        );
    }
    window.invoke_cancel();
    assert_eq!(*lock(&shared).document().vmf(), source);
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(11);
    window.invoke_edited();
    window.show().unwrap();
    assert_eq!(window.window().size().height, 470);
    lock(&shared).set_camera(render::Camera::planar(
        Projection::Front,
        point.as_vec3(),
        256.0,
    ));
    window.invoke_move_handles();
    assert!(controller.is_handling(), "{}", window.get_message());
    let camera = lock(&shared).camera();
    let guide = lock(&shared).terrain_move_guide().unwrap();
    let shape = render::handles::shapes(&camera, (1000, 800), &guide)
        .into_iter()
        .find(|shape| shape.handle == render::handles::Handle::Z)
        .unwrap();
    let center = (shape.segments[0][0] + shape.segments[0][1]) * 0.5;
    let pointer = Pointer {
        position: render::handles::project(&camera, center, (1000, 800))
            .unwrap()
            .as_dvec2(),
        size: (1000, 800),
    };
    controller
        .begin_handle(&mut lock(&shared), pointer)
        .unwrap();
    let end = Pointer {
        position: render::handles::project(&camera, center + Vec3::Z * 16.0, pointer.size)
            .unwrap()
            .as_dvec2(),
        ..pointer
    };
    controller.update_handle(&mut lock(&shared), end).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), source);
    controller.finish_handle(&mut lock(&shared)).unwrap();
    assert!(!controller.is_handling());
    assert!(window.get_ready());
    assert_eq!(window.get_vertex_z().as_str(), "16");
    save_ui_snapshot(window.window(), "terrain-handle-review");
    window.invoke_apply();
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(*lock(&shared).document().vmf(), source);
    lock(&shared)
        .highlight_terrain_vertices(Actor::User, &[(face, vec![12])])
        .unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    window.set_operation_index(11);
    window.invoke_move_handles();
    controller
        .begin_handle(&mut lock(&shared), pointer)
        .unwrap();
    controller.update_handle(&mut lock(&shared), end).unwrap();
    controller.cancel_handles(&mut lock(&shared), false);
    assert!(!controller.is_handling());
    assert!(!window.window().is_visible());
    assert!(!lock(&shared).has_face_preview());
    assert_eq!(*lock(&shared).document().vmf(), source);
}

#[test]
pub(crate) fn terrain_volume_control_selects_triangles_without_changing_the_map() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::new(64.0, 64.0, 16.0),
            "brick/test",
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
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId { solid, side };
    session.create_terrain(Actor::Agent, &[face], 2, 0).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_triangles_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let spec = session::volume::Spec {
        mins: DVec3::new(2.0, 10.0, 15.0),
        maxs: DVec3::new(6.0, 14.0, 17.0),
        rule: map_core::region::Match::Intersecting,
        mode: SelectMode::Replace,
        faces: true,
    };
    let token = lock(&shared).preview_volume_selection(spec).unwrap();
    controller.window.invoke_select_volume();
    assert!(
        controller
            .window
            .get_triangles()
            .row_data(0)
            .unwrap()
            .selected
    );
    assert!(
        !controller
            .window
            .get_triangles()
            .row_data(1)
            .unwrap()
            .selected
    );
    controller.window.show().unwrap();
    save_ui_snapshot(controller.window.window(), "terrain-volume-triangles");
    assert_eq!(*lock(&shared).document().vmf(), source);
    assert_eq!(lock(&shared).volume_preview().unwrap().token, token);
    controller.window.invoke_use_selection();
    assert_eq!(
        lock(&shared)
            .terrain_vertex_mask(&[face], None)
            .unwrap()
            .faces[0]
            .vertices,
        [0, 5, 6]
    );
}

#[test]
pub(crate) fn terrain_center_picking_uses_rendered_hits_and_preserves_foreign_drafts() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            DVec3::new(-64.0, -64.0, 0.0),
            DVec3::new(64.0, 64.0, 16.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let target = map_core::surface::FaceId {
        solid,
        side: session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    };
    session
        .select_faces(&[target], session::SelectMode::Replace)
        .unwrap();
    session
        .create_terrain(Actor::Agent, &[target], 2, 0)
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = terrain_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    let window = &controller.window;
    window.set_operation_index(1);
    window.set_radius("32".into());
    window.set_amount("64".into());
    window.invoke_edited();
    window.show().unwrap();
    save_ui_snapshot(window.window(), "terrain-center-picker");
    window.invoke_preview_edit();
    let own = lock(&shared).face_preview_token();
    window.invoke_pick_center();
    assert!(controller.is_picking());
    let miss = Ray {
        origin: DVec3::new(1000.0, 0.0, 200.0),
        direction: DVec3::NEG_Z,
    };
    assert!(controller.select_center(&mut lock(&shared), &miss).is_err());
    assert!(controller.is_picking());
    controller.cancel_center_pick();
    assert_eq!(lock(&shared).face_preview_token(), own);
    window.invoke_pick_center();
    let ray = Ray {
        origin: DVec3::new(0.0, 0.0, 200.0),
        direction: DVec3::NEG_Z,
    };
    assert_eq!(
        controller.select_center(&mut lock(&shared), &ray).unwrap(),
        DVec3::new(0.0, 0.0, 80.0)
    );
    assert!(!controller.is_picking());
    assert_eq!(window.get_center_z(), "80");
    assert!(!lock(&shared).has_face_preview());
    assert_eq!(*lock(&shared).document().vmf(), before);
    lock(&shared)
        .preview_terrain_resolution(Actor::Agent, &[target], 3)
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    window.invoke_pick_center();
    assert_eq!(
        controller
            .select_center(&mut lock(&shared), &ray)
            .unwrap()
            .z,
        16.0
    );
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.invoke_pick_center();
    lock(&shared)
        .remove_terrain(Actor::Agent, &[target])
        .unwrap();
    assert!(controller.select_center(&mut lock(&shared), &ray).is_err());
    assert!(!controller.is_picking());
    assert!(window.get_stale());
    window.invoke_cancel();
}

#[test]
pub(crate) fn mirror_controls_share_brush_transform_and_agent_preview() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let id = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let before = state.document().vmf().clone();
    app.set_has_selection(true);
    app.set_can_apply_material(true);
    app.set_show_transform(true);
    app.set_transform_mode(3);
    app.set_transform_reflect_x(true);
    app.set_transform_reflect_y(false);
    app.set_transform_reflect_z(false);
    app.set_transform_pivot_mode(1);
    apply_selection_transform(&app, &mut state).unwrap();
    let bounds = map_core::query::bounds_of(state.document(), &[id]).unwrap();
    assert!(bounds.0.abs_diff_eq(DVec3::new(-64.0, 0.0, 0.0), 1e-8));
    state.undo(Actor::User).unwrap();
    assert_eq!(state.document().vmf(), &before);
    state
        .preview_transform(
            Actor::Agent,
            &[id],
            edit::Transform {
                reflection: [false, true, true],
                texture_scale_lock: Some(false),
                ..Default::default()
            },
            Some(DVec3::new(12.125, 7.25, 3.5)),
        )
        .unwrap();
    refresh_interaction(&app, &state);
    app.show().unwrap();
    save_ui_snapshot(app.window(), "brush-reflection");
    assert_eq!(app.get_transform_mode(), 3);
    assert!(!app.get_transform_reflect_x());
    assert!(app.get_transform_reflect_y());
    assert!(app.get_transform_reflect_z());
    assert!(!app.get_transform_texture_scale_lock());
    for label in [
        "Mirror X",
        "Mirror Y",
        "Mirror Z",
        "Mirror textures with geometry",
    ] {
        scroll_inspector_to_control(&app, label);
        assert_dialog_button_visible(&app, label);
    }
    state.cancel_transform_preview();
    assert_eq!(state.document().vmf(), &before);
    app.set_transform_reflect_y(false);
    app.set_transform_reflect_z(false);
    assert!(
        apply_selection_transform(&app, &mut state)
            .unwrap_err()
            .contains("at least one")
    );
    assert_eq!(state.document().vmf(), &before);
}

#[test]
pub(crate) fn snap_to_grid_control_previews_the_named_anchor_before_apply() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state
        .create_block(
            Actor::User,
            DVec3::new(-3.0, 5.0, 7.0),
            DVec3::new(61.0, 69.0, 71.0),
            "brick/test",
            false,
        )
        .unwrap();
    let before = state.document().vmf().clone();
    let history = state.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(state));
    bind_manipulation_controls(&app, shared.clone());
    app.set_show_transform(true);
    app.invoke_preview_snap_to_grid();
    assert!(app.get_has_transform_preview());
    assert!(
        app.get_transform_preview_summary()
            .contains("bounds minimum")
    );
    assert!(app.get_transform_preview_summary().contains("grid 16"));
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    app.show().unwrap();
    save_ui_snapshot(app.window(), "snap-to-grid-preview");
    app.invoke_apply_transform_preview();
    let state = lock(&shared);
    let bounds = state.selection_bounds().unwrap();
    assert!(bounds.0.abs_diff_eq(DVec3::ZERO, 1e-8));
    assert_eq!(state.document().undo_labels().count(), history + 1);
    assert!(state.transform_preview().is_none());
}

#[test]
pub(crate) fn independent_texture_controls_share_numeric_and_agent_preview_modes() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    let id = state
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    let before = state.document().vmf().clone();
    app.set_has_selection(true);
    app.set_can_apply_material(true);
    app.set_show_transform(true);
    app.set_transform_mode(2);
    app.set_transform_x("2".into());
    app.set_transform_y("1".into());
    app.set_transform_z("1".into());
    app.set_transform_texture_lock(true);
    app.set_transform_texture_scale_lock(false);
    let tool = transform_tool_from_controls(&app).unwrap();
    assert!(tool.texture_lock);
    assert_eq!(tool.texture_scale_lock, Some(false));
    state.set_transform_tool(Actor::User, tool).unwrap();
    apply_selection_transform(&app, &mut state).unwrap();
    let ObjectId::Solid(solid) = id else {
        unreachable!()
    };
    for (old, new) in before.world.solids[0]
        .sides
        .iter()
        .zip(&state.document().solid(solid).unwrap().sides)
    {
        assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
    }
    state.undo(Actor::User).unwrap();
    assert_eq!(state.document().vmf(), &before);
    state
        .preview_transform(
            Actor::Agent,
            &[id],
            edit::Transform {
                scale: DVec3::new(2.0, 1.0, 1.0),
                texture_lock: false,
                texture_scale_lock: Some(true),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    refresh_interaction(&app, &state);
    assert!(!app.get_transform_texture_lock());
    assert!(app.get_transform_texture_scale_lock());
    app.show().unwrap();
    save_ui_snapshot(app.window(), "independent-texture-scale-lock");
    assert_eq!(app.get_transform_x(), "2");
    assert_eq!(app.get_transform_y(), "1");
    assert_eq!(app.get_transform_z(), "1");
    for label in ["Follow move and rotation", "Stretch textures when scaling"] {
        scroll_inspector_to_control(&app, label);
        assert_dialog_button_visible(&app, label);
    }
    state.cancel_transform_preview();
    assert_eq!(state.document().vmf(), &before);
}

#[test]
pub(crate) fn transform_controls_apply_usable_defaults_and_shared_selection_operations() {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().unwrap();
    app.show().unwrap();
    app.set_transform_x("90".into());
    app.set_transform_y("-45".into());
    app.set_transform_z("30".into());
    app.set_transform_mode(2);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        input_vector([
            app.get_transform_x(),
            app.get_transform_y(),
            app.get_transform_z()
        ])
        .unwrap(),
        DVec3::ONE
    );
    app.set_transform_x("2".into());
    app.set_transform_mode(1);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        input_vector([
            app.get_transform_x(),
            app.get_transform_y(),
            app.get_transform_z()
        ])
        .unwrap(),
        DVec3::ZERO
    );
    let mut session = Session::new(Err("no assets needed".into()));
    let id = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::new(64.0, 32.0, 16.0),
            "brick/wall",
            false,
        )
        .unwrap();
    let session = Rc::new(RefCell::new(session));
    app.on_apply_transform({
        let session = session.clone();
        let app = app.as_weak();
        move || {
            let app = app.unwrap();
            if let Err(message) = apply_selection_transform(&app, &mut session.borrow_mut()) {
                app.set_message(message.into());
            }
        }
    });
    app.set_has_selection(true);
    app.set_show_transform(true);
    app.set_can_apply_material(true);
    app.set_transform_mode(2);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
    app.set_transform_x("2".into());
    for (pivot_mode, expected) in [
        (
            0,
            (DVec3::new(-32.0, 0.0, 0.0), DVec3::new(96.0, 32.0, 16.0)),
        ),
        (1, (DVec3::ZERO, DVec3::new(128.0, 32.0, 16.0))),
        (
            2,
            (DVec3::new(-10.0, 0.0, 0.0), DVec3::new(118.0, 32.0, 16.0)),
        ),
    ] {
        app.set_transform_pivot_mode(pivot_mode);
        app.set_pivot_x("10".into());
        app.set_pivot_y("20".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        let button = scroll_inspector_to_control(&app, "Scale selection");
        button.invoke_accessible_default_action();
        assert_eq!(
            query::bounds(session.borrow().document(), id),
            Some(expected)
        );
        session.borrow_mut().undo(Actor::Agent).unwrap();
    }
    let before = session.borrow().document().vmf().clone();
    app.set_transform_y("0".into());
    scroll_inspector_to_control(&app, "Scale selection").invoke_accessible_default_action();
    assert_eq!(session.borrow().document().vmf(), &before);
    assert!(app.get_message().contains("positive"));
    app.set_transform_pivot_mode(0);
    app.set_transform_mode(1);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
    app.set_transform_z("90".into());
    scroll_inspector_to_control(&app, "Rotate selection").invoke_accessible_default_action();
    let (mins, maxs) = query::bounds(session.borrow().document(), id).unwrap();
    assert!(mins.abs_diff_eq(DVec3::new(16.0, -16.0, 0.0), 1e-8));
    assert!(maxs.abs_diff_eq(DVec3::new(48.0, 48.0, 16.0), 1e-8));
    session.borrow_mut().undo(Actor::Agent).unwrap();
    app.set_transform_mode(0);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
    app.set_transform_x("0.125".into());
    app.set_transform_y("-0.25".into());
    app.set_transform_z("0.5".into());
    scroll_inspector_to_control(&app, "Move selection").invoke_accessible_default_action();
    assert_eq!(
        query::bounds(session.borrow().document(), id),
        Some((
            DVec3::new(0.125, -0.25, 0.5),
            DVec3::new(64.125, 31.75, 16.5)
        ))
    );
    app.set_transform_mode(2);
    slint::platform::update_timers_and_animations();
    app.set_transform_mode(0);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        input_vector([
            app.get_transform_x(),
            app.get_transform_y(),
            app.get_transform_z()
        ])
        .unwrap(),
        DVec3::ZERO
    );
}

#[test]
pub(crate) fn stale_brush_drags_cannot_create_geometry_in_a_replaced_document() {
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_projection(Actor::User, Projection::Top);
    let drag = BrushDrag {
        start: DVec2::ZERO,
        epoch: session.brush_epoch(),
        generation: session.document_generation(),
        revision: session.document().revision(),
        projection: Projection::Top,
    };
    assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_ok());
    session.new_map(Actor::Agent, true).unwrap();
    assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_err());
    assert!(session.document().vmf().world.solids.is_empty());
}

#[test]
pub(crate) fn shared_cancellation_and_settings_invalidate_active_human_drags() {
    for preview in [false, true] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_projection(Actor::User, Projection::Top);
        let drag = BrushDrag {
            start: DVec2::ZERO,
            epoch: session.brush_epoch(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            projection: Projection::Top,
        };
        if preview {
            session
                .preview_block(DVec3::ZERO, DVec3::splat(32.0))
                .unwrap();
        }
        assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_ok());
        session.cancel_block_preview();
        assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_err());
        let drag = BrushDrag {
            epoch: session.brush_epoch(),
            ..drag
        };
        session
            .set_spatial(
                Actor::Agent,
                spatial::Update {
                    grid_size: Some(8.0),
                    ..spatial::Update::default()
                },
            )
            .unwrap();
        assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_err());
        assert!(session.document().vmf().world.solids.is_empty());
        assert!(session.document().undo_labels().next().is_none());
    }
}

#[test]
pub(crate) fn a_collapsed_snapped_preview_can_grow_into_a_valid_footprint() {
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_projection(Actor::User, Projection::Top);
    let drag = BrushDrag {
        start: DVec2::ZERO,
        epoch: session.brush_epoch(),
        generation: session.document_generation(),
        revision: session.document().revision(),
        projection: Projection::Top,
    };
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    assert!(drag_bounds(&session, drag, DVec2::ONE).is_err());
    session.clear_block_preview();
    assert!(session.block_preview().is_none());
    let (mins, maxs) = drag_bounds(&session, drag, DVec2::splat(32.0)).unwrap();
    session.preview_block(mins, maxs).unwrap();
    assert_eq!(session.block_preview(), Some((mins, maxs)));
    assert!(session.document().undo_labels().next().is_none());
}

#[test]
pub(crate) fn property_controls_preserve_imported_values_and_show_inherited_defaults() {
    let mut session = Session::new(Err("no game required".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"@BaseClass = Base [
                    _light(color255) : "Brightness" : "255 255 255 200"
                    spawnflags(flags) = [ 1 : "Enabled" : 0 2 : "Special" : 0 ]
                ]
                @PointClass base(Base) = light [
                    angles(angle) : "Orientation" : "0 0 0"
                    pitch(angle) : "Pitch" : 0
                    style(choices) : "Style" : 0 = [ 0 : "Steady" 1 : "Pulse" ]
                    editor_id(integer) readonly : "Editor ID"
                ]"#,
        )
        .unwrap(),
    );
    let mut entity = Entity::new(1, "light");
    entity.properties.set("style", "99");
    entity.properties.set("spawnflags", "9");
    entity.properties.set("custom_key", "keep me");
    let rows = entity_property_rows(&session, &entity);
    let row = |key| rows.iter().find(|row| row.key == key).unwrap();
    let style = row("style");
    assert_eq!(style.value, "99");
    assert_eq!(style.choice_index, 0);
    assert_eq!(style.choice_values.row_data(0).unwrap(), "99");
    assert_eq!(style.choice_values.row_count(), 3);
    let flags = row("spawnflags");
    assert_eq!(flags.value, "9");
    assert!(flags.flags.row_data(0).unwrap().checked);
    assert!(!flags.flags.row_data(1).unwrap().checked);
    assert_eq!(row("_light").value, "255 255 255 200");
    assert_eq!(row("_light").type_name, "color255");
    assert!(!row("angles").numeric);
    assert!(row("pitch").numeric);
    assert!(row("editor_id").read_only);
    assert_eq!(row("custom_key").value, "keep me");
    assert_eq!(row("custom_key").type_name, "string");
    assert!(entity.properties.get("_light").is_none());
}

#[test]
pub(crate) fn material_reload_button_replaces_failed_cached_previews_without_source_edits() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    session
        .edit(Actor::User, "remove fixture sky", |tx| {
            tx.world_properties_mut().set("skyname", "");
            Ok(())
        })
        .unwrap();
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session.build_scene();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let window = MaterialWindow::new().unwrap();
    window.set_material("test/checker".into());
    materials::bind_material_reload(&window, shared.clone());
    refresh_material_catalog(&window, &mut lock(&shared));
    refresh_material_window(&window, &mut lock(&shared));
    assert!(window.get_available());
    window
        .window()
        .set_size(slint::PhysicalSize::new(1000, 760));
    window.show().unwrap();
    assert_dialog_button_visible(&window, "Reload material assets");
    let texture = directory.path().join("materials/test/checker.vtf");
    let original_texture = std::fs::read(&texture).unwrap();
    std::fs::write(&texture, b"broken VTF").unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &window,
        "Reload material assets",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(!window.get_available());
    assert!(window.get_detail().contains("checker.vtf"));
    assert!(window.get_reload_status().contains("test/checker:"));
    std::fs::write(&texture, original_texture).unwrap();
    lock(&shared).reload_material_assets(Actor::Agent);
    assert!(!materials::review_material_choice(
        &window,
        &mut lock(&shared)
    ));
    assert!(window.get_available());
    assert!(window.get_detail().contains("Review the refreshed preview"));
    assert!(materials::review_material_choice(
        &window,
        &mut lock(&shared)
    ));
    assert_eq!(lock(&shared).document().vmf(), &source);
    refresh_material_window(&window, &mut lock(&shared));
    save_ui_snapshot(window.window(), "material-reload");
    window.hide().unwrap();
}

#[test]
pub(crate) fn entity_panel_shows_shared_fgd_box_status_and_invalid_definition_diagnostics() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(-24 -12 0, 40 20 72) sphere(radius) = sample []").unwrap(),
    );
    let id = session
        .create_entity(Actor::Agent, "sample", Some(DVec3::ZERO), BTreeMap::new())
        .unwrap();
    let ObjectId::Entity(entity) = id else {
        unreachable!()
    };
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let app = AppWindow::new().unwrap();
    refresh_panel(&app, &mut session);
    assert_eq!(
        app.get_entity_helper_status().as_str(),
        session.entity_helper_report(entity).unwrap().status
    );
    assert!(app.get_entity_helper_status().contains("64 × 32 × 72"));
    assert!(app.get_entity_helper_status().contains("sphere"));
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    save_ui_snapshot(app.window(), "fgd-size-helpers");
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(0 0 0) = sample []").unwrap(),
    );
    refresh_panel(&app, &mut session);
    assert!(app.get_entity_helper_status().contains("Invalid FGD size"));
    assert!(
        app.get_entity_helper_status()
            .contains("positive dimensions")
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    app.hide().unwrap();
}
