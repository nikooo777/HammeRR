use std::sync::{Arc, Mutex};

use super::*;
use crate::SurfaceWindow;
use glam::DVec3;
use map_core::{ObjectId, surface};
use slint::platform::{PointerEventButton, WindowEvent};

fn image_position(window: &AtlasWindow, point: [f32; 2]) -> slint::LogicalPosition {
    window.window().take_snapshot().unwrap();
    let image = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        window,
        "Atlas image rectangle",
    )
    .next()
    .unwrap();
    let origin = image.absolute_position();
    let size = image.size();
    assert!((size.width / size.height - 2.0).abs() < 1e-5);
    slint::LogicalPosition::new(
        origin.x + point[0] * size.width,
        origin.y + point[1] * size.height,
    )
}

fn image_press(window: &AtlasWindow, point: [f32; 2]) {
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: image_position(window, point),
        button: PointerEventButton::Left,
    });
}

fn image_release(window: &AtlasWindow, point: [f32; 2]) {
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: image_position(window, point),
            button: PointerEventButton::Left,
        });
}

#[test]
fn image_rectangle_mapping_uses_original_pixel_edges_and_clips_only_continuing_drags() {
    assert_eq!(
        image_pixel([0.25, 0.75], [128, 64], false).unwrap(),
        [32, 48]
    );
    assert_eq!(image_pixel([0.25, 0.75], [32, 64], false).unwrap(), [8, 48]);
    assert_eq!(
        image_pixel([1.0, 1.0], [8192, 4096], false).unwrap(),
        [8192, 4096]
    );
    assert_eq!(
        image_pixel([-0.25, 1.25], [128, 64], true).unwrap(),
        [0, 64]
    );
    assert_eq!(image_pixel([0.125, 0.375], [12, 4], false).unwrap(), [2, 2]);
    assert!(image_pixel([-0.01, 0.5], [128, 64], false).is_err());
    assert!(image_pixel([f32::NAN, 0.5], [128, 64], true).is_err());
}

#[test]
fn atlas_dialog_preview_apply_cancel_and_material_reload_share_session_guards() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    std::fs::write(
        directory.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/checker $bumpmap test/atlas_normal }",
    )
    .unwrap();
    let texture = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    let mut normal = bytes.clone();
    for pixel in normal[80..].as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[128, 128, 255, 255]);
    }
    std::fs::write(
        directory.path().join("materials/test/atlas_normal.vtf"),
        normal,
    )
    .unwrap();
    for y in 0..32 {
        for x in 0..64 {
            let rgba = match (x < 32, y < 16) {
                (true, true) => [210, 70, 48, 255],
                (false, true) => [55, 170, 220, 255],
                (true, false) => [75, 190, 100, 255],
                (false, false) => [225, 175, 55, 255],
            };
            let offset = 80 + (y * 64 + x) * 4;
            bytes[offset..offset + 4].copy_from_slice(&rgba);
        }
    }
    std::fs::write(texture, bytes).unwrap();
    let mut session = Session::new(Ok(game));
    let mut faces = Vec::new();
    for x in [0.0, 64.0] {
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
        faces.push(surface::FaceId { solid, side });
    }
    session
        .edit_faces(
            Actor::User,
            &faces[1..],
            surface::SurfaceEdit {
                rotation: Some(90.0),
                scale_u: Some(-0.5),
                ..surface::SurfaceEdit::default()
            },
            false,
            false,
        )
        .unwrap();
    session
        .select_faces(&faces, crate::session::SelectMode::Replace)
        .unwrap();
    let original = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Rc::new(Controller::new(&shared).unwrap());
    let parent = SurfaceWindow::new().unwrap();
    parent.set_material("test/checker".into());
    parent.on_fit_atlas({
        let (controller, shared) = (controller.clone(), shared.clone());
        move || controller.show(&mut lock(&shared), "test/checker").unwrap()
    });
    parent.invoke_fit_atlas();
    assert!(controller.window.window().is_visible());
    assert!(controller.window.get_loaded());
    assert_eq!(controller.window.get_fit_mode(), 0);
    controller.window.set_region_x("16".into());
    controller.window.set_region_y("8".into());
    controller.window.set_region_pixels_wide("32".into());
    controller.window.set_region_pixels_high("16".into());
    controller.window.set_region_padding("2".into());
    controller.window.invoke_edited();
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    assert!(
        lock(&shared).atlas_fit_state()["preview"]
            .get("mode")
            .is_none()
    );
    assert!(controller.window.get_valid_usable_region());
    assert!(controller.window.get_has_padding());
    assert_eq!(controller.window.get_usable_u(), 0.28125);
    assert_eq!(controller.window.get_usable_v(), 0.3125);
    assert_eq!(controller.window.get_usable_width(), 0.4375);
    assert_eq!(controller.window.get_usable_height(), 0.375);
    assert!(controller.window.get_usable_details().contains("28 × 12"));
    let token = lock(&shared).face_preview_token();
    let draft = lock(&shared).render_document().vmf().clone();
    let previous_rectangle = rectangle_fields(&controller.window);
    let size = controller.window.window().size();
    controller
        .window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
    controller
        .window
        .window()
        .dispatch_event(WindowEvent::Resized {
            size: size.to_logical(controller.window.window().scale_factor()),
        });
    image_press(&controller.window, [0.5, -0.1]);
    image_release(&controller.window, [0.75, 0.75]);
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    image_press(&controller.window, [0.75, 0.75]);
    assert!(controller.window.get_rectangle_dragging());
    controller.window.invoke_preview_fit();
    assert!(!controller.window.get_ready());
    image_release(&controller.window, [0.25, 0.25]);
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    assert!(!controller.window.get_ready());
    assert_eq!(lock(&shared).face_preview_token(), token);
    image_press(&controller.window, [0.25, 0.25]);
    controller
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: image_position(&controller.window, [1.25, 1.25]),
        });
    assert_eq!(
        rectangle(&controller.window).unwrap(),
        Rectangle {
            x: 16,
            y: 8,
            width: 48,
            height: 24
        }
    );
    controller
        .window
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    controller
        .window
        .window()
        .dispatch_event(WindowEvent::KeyReleased {
            text: slint::platform::Key::Escape.into(),
        });
    assert!(controller.window.window().is_visible());
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    assert_eq!(lock(&shared).face_preview_token(), token);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    image_release(&controller.window, [1.25, 1.25]);
    image_press(&controller.window, [0.125, 0.125]);
    image_release(&controller.window, [0.875, 0.875]);
    assert_eq!(
        rectangle(&controller.window).unwrap(),
        Rectangle {
            x: 8,
            y: 4,
            width: 48,
            height: 24
        }
    );
    assert!(!controller.window.get_ready());
    assert_eq!(lock(&shared).face_preview_token(), token);
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    assert_eq!(
        lock(&shared).atlas_fit_state()["preview"]["rectangle"],
        serde_json::json!({"x":8,"y":4,"width":48,"height":24})
    );
    image_press(&controller.window, [0.75, 0.75]);
    image_release(&controller.window, [0.25, 0.25]);
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    let token = lock(&shared).face_preview_token();
    let draft = lock(&shared).render_document().vmf().clone();
    image_press(&controller.window, [0.5, 0.5]);
    image_release(&controller.window, [0.5, 0.5]);
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    assert!(
        controller
            .window
            .get_message()
            .contains("positive width and height")
    );
    image_press(&controller.window, [0.25, 0.25]);
    controller.window.invoke_rectangle_pointer(1, 0.875, 0.875);
    controller
        .window
        .window()
        .dispatch_event(WindowEvent::PointerExited);
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    image_press(&controller.window, [0.25, 0.25]);
    controller.window.invoke_rectangle_pointer(1, 0.875, 0.875);
    controller.window.invoke_inspect_material();
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    assert_eq!(lock(&shared).face_preview_token(), token);
    image_release(&controller.window, [0.875, 0.875]);
    image_press(&controller.window, [0.25, 0.25]);
    controller.window.invoke_rectangle_pointer(1, 0.875, 0.875);
    assert!(
        controller
            .initialize(&mut lock(&shared), "test/checker")
            .is_err()
    );
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    assert_eq!(lock(&shared).face_preview_token(), token);
    image_release(&controller.window, [0.875, 0.875]);
    let material_path = directory.path().join("materials/test/checker.vmt");
    let material_source = std::fs::read_to_string(&material_path).unwrap();
    image_press(&controller.window, [0.25, 0.25]);
    controller.window.invoke_rectangle_pointer(1, 0.875, 0.875);
    std::fs::write(
        &material_path,
        "LightmappedGeneric { $basetexture test/checker $color \"[1 0 0]\" }",
    )
    .unwrap();
    image_release(&controller.window, [0.875, 0.875]);
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous_rectangle);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    assert_eq!(lock(&shared).face_preview_token(), token);
    std::fs::write(material_path, material_source).unwrap();
    controller.window.invoke_rectangle_pointer(0, 0.25, 0.25);
    controller.window.invoke_rectangle_pointer(1, 0.875, 0.875);
    controller.window.set_region_x("12".into());
    controller.window.invoke_edited();
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(
        rectangle(&controller.window).unwrap(),
        Rectangle {
            x: 12,
            y: 8,
            width: 32,
            height: 16
        }
    );
    assert_eq!(lock(&shared).face_preview_token(), token);
    controller.window.set_region_x("16".into());
    controller.window.invoke_edited();
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    let token = lock(&shared).face_preview_token();
    let draft = lock(&shared).render_document().vmf().clone();
    controller.window.set_region_padding("8".into());
    controller.window.invoke_edited();
    controller.window.invoke_preview_fit();
    assert!(!controller.window.get_ready());
    assert!(controller.window.get_valid_region());
    assert!(!controller.window.get_valid_usable_region());
    assert!(controller.window.get_message().contains("positive usable"));
    assert_eq!(lock(&shared).face_preview_token(), token);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    controller.window.set_region_padding("2".into());
    controller.window.invoke_edited();
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    controller.window.set_fit_mode(1);
    controller.window.invoke_edited();
    controller.window.invoke_apply_fit();
    assert_eq!(lock(&shared).document().vmf(), &original);
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    assert_eq!(
        lock(&shared).atlas_fit_state()["preview"]["mode"],
        "together"
    );
    let grouped_draft = lock(&shared).render_document().vmf().clone();
    controller.window.set_reference_index(1);
    controller.window.invoke_apply_fit();
    assert!(!controller.window.get_ready());
    assert_eq!(lock(&shared).render_document().vmf(), &grouped_draft);
    assert_eq!(lock(&shared).document().vmf(), &original);
    controller.window.invoke_edited();
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    assert_eq!(
        lock(&shared).atlas_fit_state()["preview"]["reference"],
        serde_json::json!({"solid":faces[1].solid,"side":faces[1].side})
    );
    assert_ne!(lock(&shared).render_document().vmf(), &grouped_draft);
    let generation = lock(&shared).materials().generation();
    {
        let mut session = lock(&shared);
        let mut view = session.view();
        view.normal_maps = true;
        session.set_view(view);
        let normals = session.normal_map_preview_state();
        assert_eq!(
            normals["report"]["materials"][0]["state"], "ready",
            "{normals}"
        );
    }
    assert_ne!(lock(&shared).materials().generation(), generation);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_ready());
    assert!(controller.window.get_loaded());
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert!(controller.window.get_valid_region());
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "atlas-face-fit");
    controller.window.set_region_pixels_wide("0".into());
    controller.window.invoke_edited();
    assert!(!controller.window.get_ready());
    controller.window.invoke_apply_fit();
    assert_eq!(lock(&shared).document().vmf(), &original);
    let previous = rectangle_fields(&controller.window);
    controller.window.invoke_rectangle_pointer(0, 0.25, 0.25);
    controller.window.invoke_rectangle_pointer(1, 0.75, 0.75);
    controller.window.invoke_cancel();
    assert!(!controller.window.get_rectangle_dragging());
    assert_eq!(rectangle_fields(&controller.window), previous);
    assert!(!lock(&shared).has_face_preview());
    controller.show(&mut lock(&shared), "test/checker").unwrap();
    controller.window.set_region_x("16".into());
    controller.window.set_region_y("8".into());
    controller.window.set_region_pixels_wide("32".into());
    controller.window.set_region_pixels_high("16".into());
    controller.window.set_region_padding("2".into());
    controller.window.set_fit_mode(1);
    controller.window.set_reference_index(1);
    controller.window.invoke_preview_fit();
    let reviewed = lock(&shared).render_document().vmf().clone();
    controller.window.invoke_apply_fit();
    assert!(!controller.window.window().is_visible());
    assert_eq!(lock(&shared).document().vmf(), &reviewed);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    controller.show(&mut lock(&shared), "test/checker").unwrap();
    controller.window.invoke_preview_fit();
    lock(&shared).reload_material_assets(Actor::Agent);
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_ready());
    controller.window.invoke_apply_fit();
    assert_eq!(lock(&shared).document().vmf(), &original);
    controller.window.invoke_cancel();
    assert_eq!(lock(&shared).camera(), camera);
}
