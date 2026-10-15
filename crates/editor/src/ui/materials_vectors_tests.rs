use super::*;

#[test]
fn direct_vector_dialog_keeps_mixed_values_and_commits_only_the_reviewed_preview() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let window = SurfaceWindow::new().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    let mut faces = Vec::new();
    for (index, u_x) in [(0, 1.0), (1, 2.0)] {
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::Agent,
                DVec3::X * f64::from(index) * 128.0,
                DVec3::new(64.0 + f64::from(index) * 128.0, 64.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let face = map_core::surface::FaceId {
            solid: id,
            side: session
                .document()
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        session
            .edit_faces(
                Actor::Agent,
                &[face],
                map_core::surface::SurfaceEdit {
                    u_axis: Some([Some(u_x), None, None]),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        faces.push(face);
    }
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let target = (
        faces.clone(),
        session.document_generation(),
        session.document().revision(),
    );
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    load_surface_vectors(&window, session.document(), &faces);
    assert_eq!(window.get_u_x(), "");
    assert_eq!(window.get_u_y(), "0");
    assert_eq!(window.get_u_z(), "0");
    assert_eq!(window.get_v_y(), "-1");
    window.set_selection_label("Textures on 2 selected faces · mixed U X".into());
    window.set_shift_u("13.125".into());
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    assert!(session.has_face_preview());
    window.set_direct_vectors(true);
    cancel_surface_dialog(&window, &mut session);
    assert!(!session.has_face_preview());
    assert_eq!(session.document().vmf(), &before);
    window.set_u_y("0.375".into());
    window.set_v_x("0.25".into());
    window.set_scale_u("-0.5".into());
    window.set_shift_u("13.125".into());
    assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
    assert_eq!(session.document().vmf(), &before);
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    assert!(window.get_ready());
    window.set_direct_vectors(false);
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    window.set_direct_vectors(true);
    assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    let token = session.face_preview_token();
    let draft = session.face_preview_document().unwrap().vmf().clone();
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.preview_side(faces[0]).unwrap().u_axis.axis.x, 1.0);
    assert_eq!(session.preview_side(faces[1]).unwrap().u_axis.axis.x, 2.0);
    window.set_u_y("NaN".into());
    assert!(apply_surface_dialog(&window, &mut session, &target, true).is_err());
    assert_eq!(session.face_preview_token(), token);
    assert_eq!(session.face_preview_document().unwrap().vmf(), &draft);
    window.set_u_y("0.5".into());
    assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
    assert_eq!(session.document().vmf(), &before);
    window.set_u_y("0.375".into());
    refresh_surface_dialog(&window, &mut session, &target);
    window.window().set_size(slint::PhysicalSize::new(760, 900));
    window.show().unwrap();
    super::super::tests::save_ui_snapshot(window.window(), "uv-vectors");
    for label in ["Cancel", "Preview textures", "Apply textures"] {
        super::super::tests::assert_dialog_button_visible(&window, label);
    }
    cancel_surface_dialog(&window, &mut session);
    assert!(!session.has_face_preview());
    assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    session
        .edit_faces(
            Actor::Agent,
            &faces,
            map_core::surface::SurfaceEdit {
                shift_v: Some(9.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
    let foreign = session.face_preview_token();
    cancel_surface_dialog(&window, &mut session);
    assert_eq!(session.face_preview_token(), foreign);
    apply_surface_dialog(&window, &mut session, &target, true).unwrap();
    apply_surface_dialog(&window, &mut session, &target, false).unwrap();
    assert_eq!(session.document().vmf(), &draft);
    assert_eq!(session.document().undo_labels().count(), history + 1);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &before);
    window.hide().unwrap();
}
