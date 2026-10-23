use std::sync::{Arc, Mutex};

use super::*;
use crate::SurfaceWindow;
use glam::DVec3;
use map_core::{ObjectId, surface};

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
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let face = surface::FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session
        .select_faces(&[face], crate::session::SelectMode::Replace)
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
    controller.window.set_region_x("16".into());
    controller.window.set_region_y("8".into());
    controller.window.set_region_pixels_wide("32".into());
    controller.window.set_region_pixels_high("16".into());
    controller.window.set_region_padding("2".into());
    controller.window.invoke_edited();
    controller.window.invoke_preview_fit();
    assert!(controller.window.get_ready());
    assert!(controller.window.get_valid_usable_region());
    assert!(controller.window.get_has_padding());
    assert_eq!(controller.window.get_usable_u(), 0.28125);
    assert_eq!(controller.window.get_usable_v(), 0.3125);
    assert_eq!(controller.window.get_usable_width(), 0.4375);
    assert_eq!(controller.window.get_usable_height(), 0.375);
    assert!(controller.window.get_usable_details().contains("28 × 12"));
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
    controller.window.invoke_cancel();
    assert!(!lock(&shared).has_face_preview());
    controller.show(&mut lock(&shared), "test/checker").unwrap();
    controller.window.set_region_x("16".into());
    controller.window.set_region_y("8".into());
    controller.window.set_region_pixels_wide("32".into());
    controller.window.set_region_pixels_high("16".into());
    controller.window.set_region_padding("2".into());
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
