use super::*;
use map_core::surface::{FaceId, SurfaceEdit};
use std::sync::{Arc, Mutex};

#[test]
fn seeded_uv_variation_surface_controls_review_owned_drafts_and_render_readable_controls() {
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
    let faces = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .map(|side| FaceId {
            solid,
            side: side.id,
        })
        .collect::<Vec<_>>();
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let target = (
        faces.clone(),
        session.document_generation(),
        session.document().revision(),
    );
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document);
    let window = SurfaceWindow::new().unwrap();
    bind_surface_edits_with_access(&window, &access, || true, move || Some(target.clone()));
    window.set_selection_label("Textures on 6 selected faces".into());
    window.set_operation_index(5);
    lock(&shared)
        .edit_faces(
            Actor::Agent,
            &faces,
            SurfaceEdit {
                shift_v: Some(55.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    window.set_variation_max_u("0".into());
    window.set_variation_max_v("0".into());
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert!(window.get_owned_token().is_empty());
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.set_operation_index(0);
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.set_operation_index(5);
    window.set_variation_seed("2048".into());
    window.set_variation_max_u("32".into());
    window.set_variation_max_v("16".into());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(window.get_message().contains("Preview variation"));
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    let draft = lock(&shared).render_document().vmf().clone();
    let token = lock(&shared).face_preview_token();
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    window.set_variation_seed("4294967296".into());
    window.invoke_preview_edit();
    assert!(window.get_message().contains("Seed must be an integer"));
    assert_eq!(lock(&shared).face_preview_token(), token);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    window.set_variation_seed("2048".into());
    window.set_variation_max_u("-1".into());
    window.invoke_preview_edit();
    assert!(window.get_message().contains("nonnegative"));
    assert_eq!(lock(&shared).face_preview_token(), token);
    window.set_variation_max_u("32".into());
    window.invoke_preview_edit();
    window.window().set_size(slint::PhysicalSize::new(700, 760));
    window.show().unwrap();
    super::super::tests::save_ui_snapshot(window.window(), "uv-variation");
    for label in ["Cancel", "Preview variation", "Apply textures"] {
        super::super::tests::assert_dialog_button_visible(&window, label);
    }
    window.set_variation_max_u("33".into());
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(window.get_message().contains("Variation fields changed"));
    window.invoke_cancel();
    assert!(!lock(&shared).has_face_preview());
    lock(&shared)
        .edit_faces(
            Actor::Agent,
            &faces,
            SurfaceEdit {
                shift_v: Some(99.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    window.set_variation_max_u("0".into());
    window.set_variation_max_v("0".into());
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    assert!(window.get_message().starts_with("No variation"));
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &source);
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.set_variation_max_u("32".into());
    window.set_variation_max_v("16".into());
    window.invoke_preview_edit();
    lock(&shared)
        .edit_faces(
            Actor::Agent,
            &faces,
            SurfaceEdit {
                shift_v: Some(101.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let replacement = lock(&shared).face_preview_token();
    window.invoke_apply();
    assert!(window.get_message().contains("closed or replaced"));
    window.invoke_cancel();
    assert_eq!(lock(&shared).face_preview_token(), replacement);
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &draft);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    window.invoke_preview_edit();
    assert!(
        window
            .get_message()
            .contains("Map or face selection changed")
    );
    assert!(!lock(&shared).has_face_preview());
}
