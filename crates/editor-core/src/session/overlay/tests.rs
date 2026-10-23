use super::*;
use map_core::{ObjectId, edit};

mod handle_tests;

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

fn multiface_fixture(actor: Actor, triangle: bool) -> (tempfile::TempDir, Session, [FaceId; 2]) {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let faces: [FaceId; 2] = session
        .edit(actor, "separated coplanar faces", |tx| {
            let mut faces = Vec::new();
            for x in [-96.0, 32.0] {
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(x, -32.0, 0.0),
                    DVec3::new(x + 80.0, 32.0, 16.0),
                    "test/checker",
                )?
                else {
                    unreachable!()
                };
                let brush = tx.solid_mut(solid)?;
                let side = brush
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                    })
                    .unwrap()
                    .id;
                if triangle && faces.is_empty() {
                    brush
                        .sides
                        .iter_mut()
                        .find(|side| {
                            map_core::Plane::from_points(side.plane.0).unwrap().normal.x > 0.99
                        })
                        .unwrap()
                        .plane
                        .0 = [
                        DVec3::new(x, 32.0, 0.0),
                        DVec3::new(x + 80.0, -32.0, 0.0),
                        DVec3::new(x + 80.0, -32.0, 16.0),
                    ];
                }
                if !faces.is_empty() {
                    edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                }
                faces.push(FaceId { solid, side });
            }
            Ok(faces.try_into().unwrap())
        })
        .unwrap();
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    (directory, session, faces)
}

#[test]
fn coplanar_multiface_selection_uses_first_eligible_quad_and_one_owned_history_step() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, faces) = multiface_fixture(actor, true);
        assert_eq!(
            map_core::surface::face_vertices(&session.document, faces[0])
                .unwrap()
                .len(),
            3
        );
        let state = session.get_overlay_state();
        assert_eq!(state["selected_source"]["solid"], faces[1].solid);
        assert_eq!(state["selected_source"]["source_count"], 2);
        let sources = [faces[1], faces[0]];
        assert_eq!(state["selected_source"]["sources"], sources_json(&sources));
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let camera = session.camera();
        let state = session
            .preview_overlay_faces(
                actor,
                request(sources[0]),
                &sources[1..],
                &session.overlay_stamp(),
            )
            .unwrap();
        let token = state["draft"]["token"].as_u64().unwrap();
        assert_eq!(state["draft"]["source_count"], 2);
        assert_eq!(state["draft"]["sources"], sources_json(&sources));
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.selected_faces(), faces);
        assert_eq!(session.camera(), camera);
        let definition = session.overlay.draft.as_ref().unwrap().definition.clone();
        let reviewer = if actor == Actor::User {
            Actor::Agent
        } else {
            Actor::User
        };
        assert!(session.apply_overlay(reviewer, token).is_err());
        let reviewed = session.review_overlay(reviewer, token).unwrap();
        let replacement = reviewed["draft"]["token"].as_u64().unwrap();
        assert_ne!(replacement, token);
        assert!(session.apply_overlay(actor, token).is_err());
        let applied = session.apply_overlay(reviewer, replacement).unwrap();
        let ObjectId::Entity(id) =
            crate::identity::parse_id(applied["created"].as_str().unwrap()).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            overlay::parse(session.document.entity(id).unwrap()).unwrap(),
            definition
        );
        assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert!(
            overlay::fit_existing(session.document.vmf(), id)
                .unwrap_err()
                .contains("exactly one")
        );
        let after = session.document.vmf().clone();
        session.undo(reviewer).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.redo(reviewer).unwrap();
        assert_eq!(session.document.vmf(), &after);
    }
}

#[test]
fn coplanar_multiface_invalid_requests_preserve_owned_preview_and_stale_cleanup() {
    let (directory, mut session, faces) = multiface_fixture(Actor::Agent, false);
    std::fs::write(
        directory.path().join("materials/test/hidden.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let invalid_faces = session
        .edit(Actor::Agent, "unsupported targets", |tx| {
            let mut targets = Vec::new();
            for case in 0..5 {
                let x = 192.0 + f64::from(case) * 96.0;
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(x, -32.0, 0.0),
                    DVec3::new(x + 64.0, 32.0, if case == 3 { 32.0 } else { 16.0 }),
                    if case == 4 {
                        "test/hidden"
                    } else {
                        "test/checker"
                    },
                )?
                else {
                    unreachable!()
                };
                let brush = tx.solid_mut(solid)?;
                let side = brush
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                    })
                    .unwrap()
                    .id;
                let face = FaceId { solid, side };
                match case {
                    0 => tx.solid_mut(solid)?.hidden = true,
                    1 => {
                        edit::tie_to_entity(tx, &[solid], "func_brush", &[])?;
                    }
                    2 => {
                        map_core::displacement::edit::create(tx, &[face], 2, 0)?;
                    }
                    _ => {}
                }
                targets.push(face);
            }
            Ok(targets)
        })
        .unwrap();
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let before = session.document.vmf().clone();
    let history = session.history_snapshot();
    let valid = session
        .preview_overlay_faces(
            Actor::Agent,
            request(faces[0]),
            &faces[1..],
            &session.overlay_stamp(),
        )
        .unwrap();
    let token = valid["draft"]["token"].as_u64().unwrap();
    let mut invalid = invalid_faces
        .into_iter()
        .map(|face| vec![face])
        .collect::<Vec<_>>();
    invalid.push(vec![faces[0]]);
    invalid.push(vec![FaceId {
        solid: faces[1].solid,
        side: faces[0].side,
    }]);
    invalid.push(vec![faces[1]; source_formats::bsp::MAX_OVERLAY_FACES]);
    for sources in invalid {
        assert!(
            session
                .preview_overlay_faces(
                    Actor::Agent,
                    request(faces[0]),
                    &sources,
                    &session.overlay_stamp()
                )
                .is_err()
        );
        assert_eq!(session.get_overlay_state(), valid);
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
    }
    session
        .select_faces(&faces[..1], SelectMode::Replace)
        .unwrap();
    assert!(session.apply_overlay(Actor::Agent, token).is_err());
    assert_eq!(session.document.vmf(), &before);
    assert!(session.cancel_overlay(Actor::User, token).is_err());
    session.cancel_overlay(Actor::Agent, token).unwrap();
    assert!(session.get_overlay_state()["draft"].is_null());
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let token = session
        .preview_overlay_faces(
            Actor::Agent,
            request(faces[0]),
            &faces[1..],
            &session.overlay_stamp(),
        )
        .unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: !session.view().tool_textures,
        ..session.view()
    });
    assert!(
        !session.get_overlay_state()["draft"]["valid"]
            .as_bool()
            .unwrap()
    );
    assert!(session.apply_overlay(Actor::Agent, token).is_err());
    session.cancel_overlay(Actor::Agent, token).unwrap();
    assert_eq!(session.document.vmf(), &before);
}

#[test]
fn coplanar_multiface_hidden_sky_faces_reject_when_tool_textures_are_hidden() {
    let (directory, mut session, faces) = multiface_fixture(Actor::Agent, false);
    std::fs::write(
        directory.path().join("materials/test/sky.vmt"),
        "LightmappedGeneric { %compilesky 1 }",
    )
    .unwrap();
    session
        .edit(Actor::Agent, "sky face", |tx| {
            edit::set_material(
                tx,
                &[edit::FaceTarget::Side {
                    solid: faces[1].solid,
                    side: faces[1].side,
                }],
                "test/sky",
            )?;
            Ok(())
        })
        .unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: true,
        ..session.view()
    });
    let valid = session
        .preview_overlay_faces(
            Actor::Agent,
            request(faces[0]),
            &faces[1..],
            &session.overlay_stamp(),
        )
        .unwrap();
    let token = valid["draft"]["token"].as_u64().unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: false,
        ..session.view()
    });
    let hidden = session.get_overlay_state();
    let error = session
        .preview_overlay_faces(
            Actor::Agent,
            request(faces[0]),
            &faces[1..],
            &session.overlay_stamp(),
        )
        .unwrap_err();
    assert!(error.contains("hidden by the tool-texture"), "{error}");
    assert_eq!(session.get_overlay_state(), hidden);
    assert!(session.apply_overlay(Actor::Agent, token).is_err());
    session.cancel_overlay(Actor::Agent, token).unwrap();
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
            let state = session.get_overlay_state();
            assert!(state["draft"].get("sources").is_none());
            assert!(state["draft"].get("source_count").is_none());
            assert!(state["selected_source"].get("sources").is_none());
            assert!(state["selected_source"].get("source_count").is_none());
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

#[test]
fn refit_existing_binding_shares_owned_review_and_updates_one_entity_atomically() {
    for (actor, reviewer) in [(Actor::User, Actor::Agent), (Actor::Agent, Actor::User)] {
        for detail in [false, true] {
            let (_directory, mut session, source) = fixture(actor, detail);
            let token = preview(&mut session, actor, source);
            let created = session.apply_overlay(actor, token).unwrap()["created"]
                .as_str()
                .unwrap()
                .to_owned();
            let ObjectId::Entity(id) = crate::identity::parse_id(&created).unwrap() else {
                unreachable!()
            };
            let source_owner = session.document.owner_of_solid(source.solid).unwrap();
            session
                .edit(actor, "source moved; imported strings", |tx| {
                    tx.entity_mut(id)?.properties.set("StartU", "-0.25000");
                    tx.entity_mut(id)?
                        .properties
                        .set("material", "Materials/Test/Checker.VMT");
                    edit::translate(tx, &[source_owner], DVec3::new(3.125, -2.375, 4.625), true)
                })
                .unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let initial = session.get_overlay_state();
            assert_eq!(initial["selected_overlay"]["entity"], created);
            assert!(initial["refit_error"].is_null());
            let state = session
                .preview_overlay_refit(actor, id, &session.overlay_stamp())
                .unwrap();
            let token = state["draft"]["token"].as_u64().unwrap();
            assert_eq!(state["draft"]["operation"], "refit");
            assert_eq!(state["draft"]["entity"], created);
            assert_eq!(
                state["draft"]["source"],
                json!({"solid":source.solid,"side":source.side})
            );
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
            assert!(
                session
                    .preview_overlay_refit(actor, u32::MAX, &session.overlay_stamp())
                    .is_err()
            );
            assert!(
                session
                    .preview_overlay_refit(actor, id, initial["snapshot"].as_str().unwrap())
                    .is_err()
            );
            assert_eq!(session.get_overlay_state(), state);
            assert!(session.apply_overlay(reviewer, token).is_err());
            assert!(session.cancel_overlay(reviewer, token).is_err());
            let reviewed = session.review_overlay(reviewer, token).unwrap();
            assert_eq!(reviewed["draft"]["operation"], "refit");
            assert_eq!(reviewed["draft"]["entity"], created);
            let reviewed_token = reviewed["draft"]["token"].as_u64().unwrap();
            let expected = overlay::fit_existing(&before, id).unwrap().1;
            let result = session.apply_overlay(reviewer, reviewed_token).unwrap();
            assert_eq!(result["operation"], "refit");
            assert_eq!(result["updated"], created);
            assert!(result["created"].is_null());
            assert_eq!(session.document.vmf().entities.len(), before.entities.len());
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
            let actual = session.document.entity(id).unwrap();
            assert_eq!(overlay::parse(actual).unwrap(), expected);
            assert_eq!(actual.properties.get("StartU"), Some("-0.25000"));
            assert_eq!(
                actual.properties.get("material"),
                Some("Materials/Test/Checker.VMT")
            );
            let applied = session.document.vmf().clone();
            session.undo(reviewer).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(reviewer).unwrap();
            assert_eq!(session.document.vmf(), &applied);
        }
    }
}

#[test]
fn rejected_multiside_hidden_and_nonquad_refits_preserve_the_last_valid_draft() {
    for actor in [Actor::User, Actor::Agent] {
        for case in 0..3 {
            let (_directory, mut session, source) = fixture(actor, false);
            let id = session
                .edit(actor, "unsupported imported overlay", |tx| {
                    let ObjectId::Solid(solid) = edit::create_block(
                        tx,
                        DVec3::new(-64.0, -64.0, 0.0),
                        DVec3::new(64.0, 64.0, 16.0),
                        "test/checker",
                    )?
                    else {
                        unreachable!()
                    };
                    let brush = tx.solid_mut(solid)?;
                    let side = brush
                        .sides
                        .iter()
                        .find(|side| {
                            map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                        })
                        .unwrap()
                        .id;
                    let other = brush
                        .sides
                        .iter()
                        .find(|candidate| candidate.id != side)
                        .unwrap()
                        .id;
                    let ObjectId::Entity(id) =
                        overlay::create(tx, FaceId { solid, side }, request(source).options)?
                    else {
                        unreachable!()
                    };
                    match case {
                        0 => tx
                            .entity_mut(id)?
                            .properties
                            .set("sides", format!("{side} {other}")),
                        1 => tx.entity_mut(id)?.hidden = true,
                        2 => {
                            tx.solid_mut(solid)?
                                .sides
                                .iter_mut()
                                .find(|side| {
                                    map_core::Plane::from_points(side.plane.0).unwrap().normal.x
                                        > 0.99
                                })
                                .unwrap()
                                .plane
                                .0 = [
                                DVec3::new(-64.0, 64.0, 0.0),
                                DVec3::new(64.0, -64.0, 0.0),
                                DVec3::new(64.0, -64.0, 16.0),
                            ];
                        }
                        _ => unreachable!(),
                    }
                    Ok(id)
                })
                .unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let token = preview(&mut session, actor, source);
            let valid = session.get_overlay_state();
            let error = session
                .preview_overlay_refit(actor, id, &session.overlay_stamp())
                .unwrap_err();
            assert!(
                error.contains(match case {
                    0 => "exactly one",
                    1 => "visible info_overlay",
                    2 => "quad face",
                    _ => unreachable!(),
                }),
                "{error}"
            );
            assert_eq!(session.get_overlay_state(), valid);
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            session.cancel_overlay(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &before);
        }
    }
}

#[test]
fn rebind_keeps_destination_selection_until_owned_apply_updates_the_same_entity_atomically() {
    for actor in [Actor::User, Actor::Agent] {
        for detail in [false, true] {
            let (_directory, mut session, original) = fixture(actor, !detail);
            let token = preview(&mut session, actor, original);
            let created = session.apply_overlay(actor, token).unwrap();
            let ObjectId::Entity(id) =
                crate::identity::parse_id(created["created"].as_str().unwrap()).unwrap()
            else {
                unreachable!()
            };
            let destination = session
                .edit(actor, "rebind destination", |tx| {
                    let ObjectId::Solid(solid) = edit::create_block(
                        tx,
                        DVec3::new(-140.125, 16.375, 128.625),
                        DVec3::new(-76.125, 112.375, 144.625),
                        "test/checker",
                    )?
                    else {
                        unreachable!()
                    };
                    let side = tx
                        .solid_mut(solid)?
                        .sides
                        .iter()
                        .find(|side| {
                            map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                        })
                        .unwrap()
                        .id;
                    if detail {
                        edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                    }
                    tx.entity_mut(id)?.properties.set("StartU", "-00.2500");
                    Ok(FaceId { solid, side })
                })
                .unwrap();
            session
                .select_faces(&[destination], SelectMode::Replace)
                .unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let camera = session.camera();
            let snapshot = session.overlay_stamp();
            let state = session
                .preview_overlay_rebind(actor, id, destination, &snapshot)
                .unwrap();
            let token = state["draft"]["token"].as_u64().unwrap();
            assert_eq!(state["draft"]["operation"], "rebind");
            assert_eq!(
                state["draft"]["previous_source"],
                json!({"solid":original.solid,"side":original.side})
            );
            assert_eq!(
                state["draft"]["source"],
                json!({"solid":destination.solid,"side":destination.side})
            );
            assert_eq!(session.selected_faces(), [destination]);
            assert!(session.face_mode());
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            assert_eq!(session.camera(), camera);
            for (entity, face, stamp) in [
                (id, original, state["snapshot"].as_str().unwrap()),
                (u32::MAX, destination, state["snapshot"].as_str().unwrap()),
                (
                    id,
                    FaceId {
                        solid: u32::MAX,
                        side: u32::MAX,
                    },
                    state["snapshot"].as_str().unwrap(),
                ),
                (id, destination, snapshot.as_str()),
            ] {
                assert!(
                    session
                        .preview_overlay_rebind(actor, entity, face, stamp)
                        .is_err()
                );
                assert_eq!(session.get_overlay_state(), state);
            }
            session.cancel_overlay(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &before);
            let state = session
                .preview_overlay_rebind(actor, id, destination, &session.overlay_stamp())
                .unwrap();
            let old = state["draft"]["token"].as_u64().unwrap();
            let other = if actor == Actor::User {
                Actor::Agent
            } else {
                Actor::User
            };
            assert!(session.apply_overlay(other, old).is_err());
            assert!(session.cancel_overlay(other, old).is_err());
            let reviewed = session.review_overlay(other, old).unwrap();
            assert!(session.apply_overlay(actor, old).is_err());
            let result = session
                .apply_overlay(other, reviewed["draft"]["token"].as_u64().unwrap())
                .unwrap();
            assert_eq!(result["operation"], "rebind");
            assert_eq!(result["updated"], format!("entity:{id}"));
            assert!(result.get("created").is_none());
            assert_eq!(
                session.document.entity(id).unwrap().properties.get("sides"),
                Some(destination.side.to_string().as_str())
            );
            assert_eq!(
                session
                    .document
                    .entity(id)
                    .unwrap()
                    .properties
                    .get("StartU"),
                Some("-00.2500")
            );
            assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            let after = session.document.vmf().clone();
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &after);
        }
    }
}

#[test]
fn overlay_target_choices_search_and_page_without_changing_face_selection_or_hiding_rejections() {
    let (_directory, mut session, source) = fixture(Actor::User, false);
    let ids = session
        .edit(Actor::User, "overlay choices", |tx| {
            let mut ids = Vec::new();
            for index in 0..3 {
                let ObjectId::Entity(id) = overlay::create(
                    tx,
                    source,
                    Options {
                        targetname: Some(if index == 2 {
                            "Hidden mark".into()
                        } else {
                            format!("Mark {index}")
                        }),
                        material: "test/checker".into(),
                        ..Default::default()
                    },
                )?
                else {
                    unreachable!()
                };
                if index == 2 {
                    tx.entity_mut(id)?.hidden = true;
                }
                ids.push(id);
            }
            Ok(ids)
        })
        .unwrap();
    let before = session.document.vmf().clone();
    let selected = session.selected_faces();
    let first = session
        .browse_overlay_targets("MARK checker", 0, 1)
        .unwrap();
    let second = session.browse_overlay_targets("mark", 1, 1).unwrap();
    let unsupported = session.browse_overlay_targets("hidden", 0, 50).unwrap();
    assert_eq!(first["total"], 3);
    assert_eq!(first["entries"][0]["id"], ids[0]);
    assert_eq!(second["entries"][0]["id"], ids[1]);
    assert_eq!(
        session
            .browse_overlay_targets(&format!("entity:{}", ids[1]), 0, 20)
            .unwrap()["entries"][0]["id"],
        ids[1]
    );
    assert_eq!(unsupported["total"], 1);
    assert_eq!(unsupported["entries"][0]["eligible"], false);
    assert!(
        unsupported["entries"][0]["error"]
            .as_str()
            .unwrap()
            .contains("visible")
    );
    assert!(session.browse_overlay_targets("", 0, 0).is_err());
    assert!(
        session
            .browse_overlay_targets(&"x".repeat(257), 0, 20)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &before);
    assert_eq!(session.selected_faces(), selected);
    assert!(session.face_mode());
    assert!(session.get_overlay_state()["draft"].is_null());
}

#[test]
fn corner_drafts_share_review_atomic_apply_and_exact_undo_without_refitting() {
    let corners = [
        [-24.5, -16.25],
        [-20.0, 24.75],
        [28.5, 16.5],
        [32.25, -20.125],
    ];
    for actor in [Actor::User, Actor::Agent] {
        for detail in [false, true] {
            let (_directory, mut session, source) = fixture(actor, detail);
            let token = preview(&mut session, actor, source);
            let result = session.apply_overlay(actor, token).unwrap();
            let ObjectId::Entity(id) =
                crate::identity::parse_id(result["created"].as_str().unwrap()).unwrap()
            else {
                unreachable!()
            };
            let initial = session.get_overlay_state();
            assert_eq!(initial["selected_corners"]["id"], id);
            assert!(initial["selected_corners"]["local_corners"].is_array());
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let state = session
                .preview_overlay_corners(actor, id, corners, &session.overlay_stamp())
                .unwrap();
            assert_eq!(state["draft"]["operation"], "corners");
            assert_eq!(state["draft"]["local_corners"], json!(corners));
            assert_eq!(
                state["draft"]["source"],
                json!({"solid":source.solid,"side":source.side})
            );
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            let token = state["draft"]["token"].as_u64().unwrap();
            let other = if actor == Actor::User {
                Actor::Agent
            } else {
                Actor::User
            };
            assert!(session.apply_overlay(other, token).is_err());
            assert!(session.cancel_overlay(other, token).is_err());
            for (entity, positions, stamp) in [
                (id, [[0.0; 2]; 4], state["snapshot"].as_str().unwrap()),
                (u32::MAX, corners, state["snapshot"].as_str().unwrap()),
                (id, corners, initial["snapshot"].as_str().unwrap()),
            ] {
                assert!(
                    session
                        .preview_overlay_corners(actor, entity, positions, stamp)
                        .is_err()
                );
                assert_eq!(session.get_overlay_state(), state);
            }
            let reviewed = session.review_overlay(other, token).unwrap();
            assert!(session.cancel_overlay(actor, token).is_err());
            assert!(session.apply_overlay(actor, token).is_err());
            let expected = overlay::with_corners(&before, id, corners).unwrap().1;
            let applied = session
                .apply_overlay(other, reviewed["draft"]["token"].as_u64().unwrap())
                .unwrap();
            assert_eq!(applied["operation"], "corners");
            assert_eq!(applied["updated"], format!("entity:{id}"));
            assert!(applied.get("created").is_none());
            assert_eq!(
                overlay::parse(session.document.entity(id).unwrap()).unwrap(),
                expected
            );
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
            let after = session.document.vmf().clone();
            session.undo(other).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(other).unwrap();
            assert_eq!(session.document.vmf(), &after);
        }
    }
}

#[test]
fn corner_drafts_restore_after_competing_preview_and_cancel_safely_after_source_changes() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source) = fixture(actor, false);
        let token = preview(&mut session, actor, source);
        let result = session.apply_overlay(actor, token).unwrap();
        let ObjectId::Entity(id) =
            crate::identity::parse_id(result["created"].as_str().unwrap()).unwrap()
        else {
            unreachable!()
        };
        let state = session
            .preview_overlay_corners(
                actor,
                id,
                [[-24.0, -16.0], [-20.0, 24.0], [28.0, 16.0], [32.0, -20.0]],
                &session.overlay_stamp(),
            )
            .unwrap();
        let token = state["draft"]["token"].as_u64().unwrap();
        session
            .edit_faces(
                actor,
                &[source],
                map_core::surface::SurfaceEdit {
                    shift_u: Some(8.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        assert_eq!(session.get_overlay_state()["draft"]["valid"], false);
        assert!(session.apply_overlay(actor, token).is_err());
        session.cancel_face_preview();
        assert_eq!(session.get_overlay_state(), state);
        let owner = session.document.owner_of_solid(source.solid).unwrap();
        session
            .edit(actor, "move bound source", |tx| {
                edit::translate(tx, &[owner], DVec3::Z * 16.0, true)
            })
            .unwrap();
        assert_eq!(session.get_overlay_state()["draft"]["valid"], false);
        assert!(session.get_overlay_state()["selected_corners"].is_null());
        assert!(
            session.get_overlay_state()["corners_error"]
                .as_str()
                .unwrap()
                .contains("Refit selected overlay")
        );
        let before = session.document.vmf().clone();
        assert!(session.apply_overlay(actor, token).is_err());
        assert!(session.review_overlay(actor, token).is_err());
        session.cancel_overlay(actor, token).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }
}
