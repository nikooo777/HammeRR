use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

fn platform() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

#[test]
fn gallery_callbacks_keep_errors_headless_and_preserve_selected_forms_and_placement_values() {
    platform();
    let (session, directory, _) = crate::test_assets::prefabs();
    for index in 0..9 {
        std::fs::write(
            directory.path().join(format!("Broken {index:02}.vmf")),
            "invalid source",
        )
        .unwrap();
    }
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert_eq!(controller.window.get_entries().row_count(), PAGE);
    assert_eq!(controller.window.get_gallery_rows().row_count(), 4);
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image)
    );
    controller.window.set_center_x("123".into());
    controller.window.set_rotation_z("45".into());
    controller.window.invoke_render_gallery();
    assert!(controller.window.get_gallery_rendered());
    assert_eq!(controller.window.get_gallery_status(), "0 of 8 images");
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image && !entry.status.is_empty())
    );
    assert_eq!(
        lock(&shared).prefab_thumbnail_report("Broken 00")["graphics"]["initialized"],
        false
    );
    let seen = controller.form.borrow().seen;
    controller.refresh(&lock(&shared));
    assert_eq!(controller.form.borrow().seen, seen);
    let card = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &controller.window,
        "Broken 00",
    )
    .find(|element| {
        element.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button)
    })
    .unwrap();
    card.invoke_accessible_default_action();
    assert_eq!(controller.window.get_selected(), "Broken 00");
    controller.window.invoke_begin_rename();
    controller.window.set_prefab_name("Typed rename".into());
    let frozen = controller
        .form
        .borrow()
        .action
        .as_ref()
        .unwrap()
        .stamp
        .clone();
    std::fs::write(directory.path().join("Another.vmf"), "invalid source").unwrap();
    lock(&shared).prefab_report("", 0, PAGE, None);
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_gallery_rendered());
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image)
    );
    assert_eq!(controller.window.get_selected(), "Broken 00");
    assert_eq!(controller.window.get_prefab_name(), "Typed rename");
    assert_eq!(
        controller.form.borrow().action.as_ref().unwrap().stamp,
        frozen
    );
    assert_eq!(controller.window.get_center_x(), "123");
    assert_eq!(controller.window.get_rotation_z(), "45");
    controller.window.invoke_cancel();
    controller.window.set_thumbnail_style(1);
    controller.window.invoke_thumbnail_style_changed();
    controller.window.invoke_render_gallery();
    assert_eq!(controller.window.get_selected(), "Broken 00");
    assert!(controller.window.get_gallery_rendered());
    controller.window.invoke_page(1);
    assert!(!controller.window.get_gallery_rendered());
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image)
    );
    controller.window.set_query("Broken 00".into());
    controller.window.invoke_search();
    assert_eq!(controller.form.borrow().offset, 0);
    assert_eq!(controller.window.get_entries().row_count(), 1);
    controller.window.invoke_render_gallery();
    assert_eq!(controller.window.get_gallery_status(), "0 of 1 images");
    controller.window.invoke_render_thumbnail();
    assert!(!controller.window.get_gallery_rendered());
    assert_eq!(controller.window.get_gallery_status(), "");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
}

#[test]
#[ignore = "requires a graphics adapter for actual prefab gallery UI validation"]
fn prefab_gallery_controls_render_actual_cards_and_capture_gallery_without_mutating_owned_paste() {
    platform();
    let (mut session, _directory, assets, base) = crate::test_assets::textured_prefabs();
    let upright = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::new(16.0, 64.0, 112.0),
            "test/checker",
            false,
        )
        .unwrap();
    let step = session
        .create_block(
            Actor::User,
            glam::DVec3::new(0.0, 32.0, 24.0),
            glam::DVec3::new(48.0, 64.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    let corner = session
        .create_block(
            Actor::User,
            glam::DVec3::new(64.0, 0.0, 24.0),
            glam::DVec3::new(96.0, 64.0, 80.0),
            "test/checker",
            false,
        )
        .unwrap();
    for (name, ids) in [
        ("Gallery 01 Platform", vec![base]),
        ("Gallery 02 Wall panel", vec![upright]),
        ("Gallery 03 Stair step", vec![base, step]),
        ("Gallery 04 Corner ledge", vec![base, corner]),
        ("Spare 01 Platform", vec![base]),
        ("Spare 02 Wall panel", vec![upright]),
        ("Spare 03 Stair step", vec![base, step]),
        ("Spare 04 Corner ledge", vec![base, corner]),
    ] {
        let stamp = session.prefab_report("", 0, PAGE, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        session
            .save_prefab(Actor::User, name, &ids, &stamp)
            .unwrap();
    }
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert_eq!(
        lock(&shared).prefab_thumbnail_report("Gallery 01 Platform")["graphics"]["initialized"],
        false
    );
    controller
        .window
        .invoke_choose("Gallery 01 Platform".into());
    controller.window.set_center_x("160".into());
    controller.window.invoke_preview();
    let clipboard = lock(&shared).clipboard_report();
    let rendered = lock(&shared).render_document().vmf().clone();
    let camera = lock(&shared).camera();
    let serials = {
        let session = lock(&shared);
        (
            session.scene_serial(),
            session.view_serial(),
            session.camera_serial(),
        )
    };
    controller.window.invoke_render_gallery();
    assert!(
        controller.window.get_gallery_rendered(),
        "{}",
        controller.window.get_gallery_status()
    );
    assert_eq!(
        controller
            .window
            .get_entries()
            .iter()
            .filter(|entry| entry.has_image)
            .count(),
        MAX_GALLERY_ENTRIES
    );
    assert!(!controller.window.get_has_thumbnail());
    assert_eq!(controller.window.get_gallery_status(), "8 of 8 images");
    controller.window.set_query("Gallery".into());
    controller.window.invoke_search();
    assert!(!controller.window.get_gallery_rendered());
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image)
    );
    controller.window.set_thumbnail_style(1);
    controller.window.invoke_thumbnail_style_changed();
    controller.window.invoke_render_gallery();
    assert_eq!(controller.window.get_gallery_status(), "4 of 4 images");
    assert_eq!(
        controller
            .window
            .get_entries()
            .iter()
            .filter(|entry| entry.has_image)
            .count(),
        4
    );
    assert!(!controller.window.get_has_thumbnail());
    assert_eq!(controller.window.get_center_x(), "160");
    assert_eq!(controller.window.get_selected(), "Gallery 01 Platform");
    let platform = lock(&shared)
        .cached_prefab_thumbnail_with_style("Gallery 01 Platform", ThumbnailStyle::Textured)
        .unwrap()
        .unwrap();
    let stair = lock(&shared)
        .cached_prefab_thumbnail_with_style("Gallery 03 Stair step", ThumbnailStyle::Textured)
        .unwrap()
        .unwrap();
    assert_ne!(platform.image.pixels, stair.image.pixels);
    assert!(
        platform
            .image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 3
    );
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-gallery");
    lock(&shared)
        .prefab_thumbnail_with_style("Gallery 01 Platform", true, ThumbnailStyle::Textured)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_gallery_rendered());
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image)
    );
    assert!(controller.window.get_has_thumbnail());
    controller.window.invoke_render_gallery();
    assert_eq!(controller.window.get_gallery_status(), "4 of 4 images");
    let texture = assets.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    bytes[80] ^= 0xff;
    std::fs::write(texture, bytes).unwrap();
    controller.window.invoke_refresh();
    assert!(!controller.window.get_gallery_rendered());
    assert!(
        controller
            .window
            .get_entries()
            .iter()
            .all(|entry| !entry.has_image)
    );
    assert!(!controller.window.get_has_thumbnail());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).render_document().vmf(), &rendered);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).clipboard_report(), clipboard);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(
        {
            let session = lock(&shared);
            (
                session.scene_serial(),
                session.view_serial(),
                session.camera_serial(),
            )
        },
        serials
    );
}
