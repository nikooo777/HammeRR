use super::*;
use map_core::{ObjectId, edit};

fn fixture(actor: Actor, detail: bool) -> (tempfile::TempDir, Session, FaceId) {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let face = session
        .edit(actor, "tilted quad", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-64.125, -48.375, 8.625),
                DVec3::new(63.875, 47.625, 24.625),
                "test/checker",
            )?
            else {
                unreachable!()
            };
            let side = tx
                .solid_mut(solid)?
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            edit::transform(
                tx,
                &[ObjectId::Solid(solid)],
                edit::Transform {
                    rotation: DVec3::new(17.125, -23.375, 8.625),
                    translation: DVec3::new(0.125, -0.375, 64.625),
                    ..edit::Transform::default()
                },
            )?;
            if detail {
                edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
            }
            Ok(FaceId { solid, side })
        })
        .unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    (directory, session, face)
}

fn request(source: FaceId) -> Request {
    Request {
        source,
        options: Options {
            material: "test/checker".into(),
            targetname: Some("authored_overlay".into()),
            render_order: 2,
            u: [-0.25, 1.75],
            v: [0.125, 0.875],
            fade_min: 64.0,
            fade_max: 512.0,
        },
    }
}

fn preview(session: &mut Session, actor: Actor, source: FaceId) -> u64 {
    session
        .preview_overlay(actor, request(source), &session.overlay_stamp())
        .unwrap()["draft"]["token"]
        .as_u64()
        .unwrap()
}

#[test]
fn fitted_fractional_world_and_detail_quads_share_preview_outline_and_atomic_history() {
    for actor in [Actor::User, Actor::Agent] {
        for detail in [false, true] {
            let (_directory, mut session, source) = fixture(actor, detail);
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let selection = session.selected_faces();
            let camera = session.camera();
            let initial_lines = session.build_scene().lines.len();
            let token = preview(&mut session, actor, source);
            let draft = session.overlay.draft.as_ref().unwrap();
            let definition = draft.definition.clone();
            assert!(DVec3::from_array(definition.basis_normal).z < 0.99);
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            assert_eq!(session.selected_faces(), selection);
            assert_eq!(session.camera(), camera);
            let mesh = session.build_scene();
            assert_eq!(mesh.lines.len(), initial_lines + 8);
            for point in definition.corners() {
                assert!(mesh.lines[initial_lines..].iter().any(|line| {
                    line.position == DVec3::from_array(point).as_vec3().to_array()
                }));
            }
            let result = session.apply_overlay(actor, token).unwrap();
            let created = crate::identity::parse_id(result["created"].as_str().unwrap()).unwrap();
            let ObjectId::Entity(entity) = created else {
                unreachable!()
            };
            let actual = overlay::parse(session.document.entity(entity).unwrap()).unwrap();
            assert_eq!(actual, definition);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(session.selected_ids(), [created]);
            assert!(!session.face_mode());
            assert!(session.get_overlay_state()["draft"].is_null());
            for solid in before.solids() {
                assert_eq!(
                    map_core::surface::solid(&session.document, solid.id).unwrap(),
                    solid
                );
            }
            let applied = session.document.vmf().clone();
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &applied);
        }
    }
}

#[test]
fn compilation_and_view_refreshes_preserve_owned_overlay_drafts_and_source_history() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        session.replace_document(map_core::Document::new(crate::test_assets::room()), None);
        let solid = &session.document.vmf().world.solids[0];
        let source = FaceId {
            solid: solid.id,
            side: solid
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        };
        session
            .select_faces(&[source], SelectMode::Replace)
            .unwrap();
        let before = session.document.vmf().clone();
        let revision = session.document.revision();
        let history = session.document.undo_labels().count();
        let token = preview(&mut session, actor, source);
        let state = session.get_overlay_state();
        let scene_serial = session.scene_serial();
        let path = directory.path().join("pending-overlay.bsp");
        let result = session.begin_compile(actor, Some(&path)).unwrap().run();
        assert!(result.is_ok(), "{:?}", result.as_ref().err());
        assert!(result.as_ref().unwrap().output.bsp.is_some());
        session.finish_compile(actor, &result);
        assert!(session.scene_serial() > scene_serial);
        assert_eq!(session.get_overlay_state(), state);
        assert_eq!(session.compile_status()["job"]["status"], "completed");
        assert_eq!(session.get_overlay_state()["draft"]["valid"], true);
        session.build_scene();
        session.set_view(render::ViewOptions {
            tool_textures: false,
            ..session.view()
        });
        session.set_camera(session.camera());
        session.camera_for_projection(render::Projection::Top);
        session.build_scene();
        assert_eq!(session.get_overlay_state(), state);
        let mut invalid = request(source);
        invalid.options.u = [1.0, 1.0];
        assert!(
            session
                .preview_overlay(actor, invalid, &session.overlay_stamp())
                .is_err()
        );
        assert_eq!(session.get_overlay_state(), state);
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.revision(), revision);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.selected_faces(), [source]);
        session.apply_overlay(actor, token).unwrap();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }
}

#[test]
fn explicit_review_transfers_foreign_drafts_and_invalidates_the_previous_token() {
    for (owner, reviewer) in [(Actor::Agent, Actor::User), (Actor::User, Actor::Agent)] {
        let (_directory, mut session, source) = fixture(owner, false);
        let before = session.document.vmf().clone();
        let token = preview(&mut session, owner, source);
        assert!(session.apply_overlay(reviewer, token).is_err());
        assert!(session.cancel_overlay(reviewer, token).is_err());
        assert_eq!(session.document.vmf(), &before);
        let reviewed = session.review_overlay(reviewer, token).unwrap();
        let replacement = reviewed["draft"]["token"].as_u64().unwrap();
        assert_ne!(replacement, token);
        assert_eq!(reviewed["draft"]["owner"], reviewer.label());
        assert!(session.cancel_overlay(owner, token).is_err());
        assert!(session.apply_overlay(owner, replacement).is_err());
        assert_eq!(session.document.vmf(), &before);
        session.apply_overlay(reviewer, replacement).unwrap();
        session.undo(reviewer).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }
}

#[test]
fn invalid_and_stale_updates_preserve_the_last_valid_draft_and_redo() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source) = fixture(actor, false);
        session
            .edit(actor, "temporary edit", |tx| {
                edit::translate(tx, &[ObjectId::Solid(source.solid)], DVec3::X, true)
            })
            .unwrap();
        session.undo(actor).unwrap();
        let before = session.document.vmf().clone();
        let initial_stamp = session.overlay_stamp();
        let token = preview(&mut session, actor, source);
        let valid = session.get_overlay_state();
        for case in 0..6 {
            let mut invalid = request(source);
            match case {
                0 => invalid.options.material.clear(),
                1 => invalid.options.u = [1.0, 1.0],
                2 => invalid.options.render_order = 4,
                3 => invalid.options.fade_min = f32::INFINITY,
                4 => invalid.source.side = u32::MAX,
                5 => invalid.options.material = "../outside".into(),
                _ => unreachable!(),
            }
            assert!(
                session
                    .preview_overlay(actor, invalid, &session.overlay_stamp())
                    .is_err(),
                "case {case}"
            );
            assert_eq!(session.get_overlay_state(), valid);
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.redo_labels().count(), 1);
        }
        assert!(
            session
                .preview_overlay(actor, request(source), &initial_stamp)
                .is_err()
        );
        assert_eq!(session.get_overlay_state(), valid);
        let replacement = preview(&mut session, actor, source);
        assert_ne!(replacement, token);
        assert!(session.cancel_overlay(actor, token).is_err());
        session.cancel_overlay(actor, replacement).unwrap();
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.redo_labels().count(), 1);
    }
}

#[test]
fn map_selection_and_generation_changes_reject_apply_but_allow_owned_stale_cleanup() {
    for actor in [Actor::User, Actor::Agent] {
        for change in 0..5 {
            let (directory, mut session, source) = fixture(actor, false);
            let token = preview(&mut session, actor, source);
            match change {
                0 => {
                    session
                        .edit(actor, "move source", |tx| {
                            edit::translate(tx, &[ObjectId::Solid(source.solid)], DVec3::X, true)
                        })
                        .unwrap();
                }
                1 => session.select_faces(&[], SelectMode::Replace).unwrap(),
                2 => session.new_map(actor, true).unwrap(),
                3 => {
                    session.select_faces(&[], SelectMode::Replace).unwrap();
                    session
                        .select_faces(&[source], SelectMode::Replace)
                        .unwrap();
                }
                4 => {
                    session
                        .save(actor, Some(&directory.path().join("overlay.vmf")))
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = session.document.vmf().clone();
            assert_eq!(session.get_overlay_state()["draft"]["valid"], false);
            assert!(session.apply_overlay(actor, token).is_err());
            assert!(session.review_overlay(actor, token).is_err());
            assert_eq!(session.document.vmf(), &before);
            let mut mesh = session.build_scene();
            let lines = mesh.lines.len();
            session.append_overlay_guide(&mut mesh);
            assert_eq!(mesh.lines.len(), lines);
            session.cancel_overlay(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &before);
        }
    }
}

#[test]
fn selection_eligibility_uses_the_core_fit_and_never_changes_the_source() {
    let (_directory, mut session, source) = fixture(Actor::User, false);
    let before = session.document.vmf().clone();
    let state = session.get_overlay_state();
    assert_eq!(state["selected_source"]["solid"], source.solid);
    assert_eq!(state["selected_source"]["side"], source.side);
    assert!(state["selected_source"]["corners"].is_array());
    assert!(state["selection_error"].is_null());
    session.select_faces(&[], SelectMode::Replace).unwrap();
    assert!(session.get_overlay_state()["selected_source"].is_null());
    session
        .select_faces(&[source], SelectMode::Replace)
        .unwrap();
    session
        .edit(Actor::User, "hide source", |tx| {
            tx.solid_mut(source.solid)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert!(session.get_overlay_state()["selected_source"].is_null());
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &before);
}

#[test]
fn other_geometry_previews_block_overlay_changes_without_being_discarded() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source) = fixture(actor, false);
        let token = preview(&mut session, actor, source);
        let before = session.document.vmf().clone();
        session
            .edit_faces(
                actor,
                &[source],
                map_core::surface::SurfaceEdit {
                    shift_u: Some(12.125),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        let face_token = session.face_preview_token();
        let rendered = session.render_document().vmf().clone();
        assert!(
            session
                .preview_overlay(actor, request(source), &session.overlay_stamp())
                .is_err()
        );
        assert!(session.apply_overlay(actor, token).is_err());
        session.cancel_overlay(actor, token).unwrap();
        assert_eq!(session.face_preview_token(), face_token);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.document.vmf(), &before);
    }
}
