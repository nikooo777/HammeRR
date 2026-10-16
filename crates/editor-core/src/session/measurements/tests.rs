use super::*;
use map_core::{Document, query::Ray};
use render::Projection;
use source_formats::{keyvalues::Node, vmf::Vmf};

fn request(id: Option<u32>) -> Request {
    Request {
        id,
        name: "Stair clearance".into(),
        start: DVec3::new(4.0, 7.0, 20.0),
        end: DVec3::new(1.0, 11.0, 8.0),
    }
}

fn preview(session: &mut Session, actor: Actor, request: Request) -> u64 {
    let state = session
        .preview_measurement(actor, request, &session.measurement_stamp())
        .unwrap();
    state["draft"]["token"].as_u64().unwrap()
}

#[test]
fn both_actors_preview_cancel_save_edit_delete_and_undo_without_geometry_changes() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        let original = session.document.vmf().clone();
        let token = preview(&mut session, actor, request(None));
        assert_eq!(
            session.measurement_state()["draft"]["measurement"]["distance"],
            13.0
        );
        assert_eq!(
            session.measurement_state()["draft"]["measurement"]["components"],
            json!([-3.0, 4.0, -12.0])
        );
        assert!(!session.build_scene().lines.is_empty());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.undo_labels().count(), 0);
        session.cancel_measurement(actor, token).unwrap();
        assert!(session.build_scene().lines.is_empty());
        assert_eq!(session.document.vmf(), &original);
        let token = preview(&mut session, actor, request(None));
        session.apply_measurement(actor, token).unwrap();
        assert_eq!(session.document.undo_labels().count(), 1);
        assert_eq!(session.document.vmf().world, original.world);
        assert_eq!(session.document.vmf().entities, original.entities);
        assert!(session.selection.is_empty());
        let saved = session.document.vmf().clone();
        let mut changed = request(Some(1));
        changed.name = "Clearance updated".into();
        changed.end.z = 32.0;
        let token = preview(&mut session, actor, changed);
        assert_eq!(
            session.measurements().unwrap().measurements[0].name,
            "Stair clearance"
        );
        session.apply_measurement(actor, token).unwrap();
        assert_eq!(
            session.measurements().unwrap().measurements[0].name,
            "Clearance updated"
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &saved);
        session.redo(actor).unwrap();
        session
            .delete_measurement(actor, 1, &session.measurement_stamp())
            .unwrap();
        assert!(session.measurements().unwrap().measurements.is_empty());
        session.undo(actor).unwrap();
        assert_eq!(session.measurements().unwrap().measurements.len(), 1);
        let reopened = Vmf::parse(&session.document.vmf().write(&Default::default())).unwrap();
        assert_eq!(
            Measurements::parse(&reopened).unwrap(),
            session.measurements().unwrap()
        );
    }
}

#[test]
fn ownership_replaced_tokens_stale_snapshots_and_map_generations_guard_mutation() {
    let mut session = Session::new(Err("no assets needed".into()));
    let initial = session.measurement_stamp();
    let token = preview(&mut session, Actor::Agent, request(None));
    let source = session.document.vmf().clone();
    assert!(session.apply_measurement(Actor::User, token).is_err());
    assert!(session.cancel_measurement(Actor::User, token).is_err());
    assert!(
        session
            .preview_measurement(Actor::User, request(None), &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .preview_measurement(Actor::Agent, request(None), &initial)
            .is_err()
    );
    let reviewed = session.review_measurement(Actor::User, token).unwrap();
    let current = reviewed["draft"]["token"].as_u64().unwrap();
    assert_ne!(current, token);
    assert!(session.apply_measurement(Actor::Agent, current).is_err());
    assert!(session.cancel_measurement(Actor::Agent, token).is_err());
    let mut invalid = request(None);
    invalid.end = invalid.start;
    assert!(
        session
            .preview_measurement(Actor::User, invalid, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.measurement_state()["draft"]["token"], current);
    assert_eq!(session.document.vmf(), &source);
    session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    assert_eq!(session.measurement_state()["draft"]["valid"], false);
    assert!(session.apply_measurement(Actor::User, current).is_err());
    session.cancel_measurement(Actor::User, current).unwrap();
    let old = preview(&mut session, Actor::User, request(None));
    session.new_map(Actor::Agent, true).unwrap();
    let new = preview(&mut session, Actor::User, request(None));
    assert_ne!(old, new);
    assert!(session.apply_measurement(Actor::User, old).is_err());
    assert!(session.cancel_measurement(Actor::User, old).is_err());
    assert_eq!(session.measurement_state()["draft"]["token"], new);
}

#[test]
fn geometry_previews_are_preserved_and_import_errors_are_inspectable() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let block = session.block_preview;
    assert!(block.is_some());
    assert!(
        session
            .preview_measurement(Actor::User, request(None), &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.block_preview, block);
    assert!(session.measurement_state()["draft"].is_null());
    let mut vmf = Document::empty().into_vmf();
    vmf.extra.push(Node::block(
        measurements::KEY,
        vec![Node::string("version", "future")],
    ));
    session.replace_document(Document::new(vmf.clone()), None);
    assert!(
        session.measurement_state()["metadata_error"]
            .as_str()
            .unwrap()
            .contains("Unsupported")
    );
    assert!(
        session
            .preview_measurement(Actor::Agent, request(None), &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .delete_measurement(Actor::Agent, 1, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &vmf);
    assert!(session.build_scene().lines.is_empty());
}

#[test]
fn guides_do_not_enter_picking_and_framing_and_visibility_leave_history_unchanged() {
    let mut session = Session::new(Err("no assets needed".into()));
    let token = preview(&mut session, Actor::User, request(None));
    session.apply_measurement(Actor::User, token).unwrap();
    let original = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    assert!(
        session
            .pick(&Ray {
                origin: DVec3::new(4.0, 7.0, 200.0),
                direction: -DVec3::Z
            })
            .is_none()
    );
    for projection in [
        Projection::Perspective,
        Projection::Top,
        Projection::Front,
        Projection::Side,
    ] {
        session.set_projection(Actor::User, projection);
        for aspect in [0.5, 16.0 / 9.0, 3.0] {
            session
                .frame_measurement(Some(1), None, aspect, &session.measurement_stamp())
                .unwrap();
            let matrix = session.camera.view_projection(aspect);
            for point in [request(None).start, request(None).end] {
                let clip = matrix * point.as_vec3().extend(1.0);
                let ndc = clip.truncate() / clip.w;
                assert!(
                    ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0,
                    "{projection:?} {aspect}: {ndc:?}"
                );
            }
        }
    }
    let visible = session.build_scene().lines;
    assert!(!visible.is_empty());
    session
        .set_measurement_guides(false, &session.measurement_stamp())
        .unwrap();
    assert!(session.build_scene().lines.is_empty());
    session
        .set_measurement_guides(true, &session.measurement_stamp())
        .unwrap();
    assert_eq!(session.build_scene().lines, visible);
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.document.undo_labels().count(), history);
    assert!(
        session
            .frame_measurement(Some(1), Some(1), 1.0, &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .frame_measurement(Some(1), None, f32::NAN, &session.measurement_stamp())
            .is_err()
    );
}

#[test]
fn saved_guides_remain_visible_when_all_scene_objects_are_hidden() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    let token = preview(&mut session, Actor::User, request(None));
    session.apply_measurement(Actor::User, token).unwrap();
    session
        .select(&[], super::super::SelectMode::Replace)
        .unwrap();
    let rulers = session.build_scene().lines;
    session
        .set_visibility(Actor::User, "hide", &[object])
        .unwrap();
    assert_eq!(session.build_scene().lines, rulers);
    assert!(
        session
            .pick(&Ray {
                origin: DVec3::splat(16.0) + DVec3::Z * 100.0,
                direction: -DVec3::Z
            })
            .is_none()
    );
}

#[test]
fn extreme_framing_returns_an_error_without_changing_the_camera() {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ruler = request(None);
    ruler.start = DVec3::splat(-measurements::MAX_COORDINATE);
    ruler.end = DVec3::splat(measurements::MAX_COORDINATE);
    let token = preview(&mut session, Actor::User, ruler);
    session.apply_measurement(Actor::User, token).unwrap();
    session.set_projection(Actor::User, Projection::Top);
    let camera = session.camera();
    assert!(
        session
            .frame_measurement(Some(1), None, 0.5, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    assert!(
        session
            .frame_measurement(
                Some(1),
                None,
                f32::MIN_POSITIVE,
                &session.measurement_stamp()
            )
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    session.set_projection(Actor::User, Projection::Perspective);
    session
        .frame_measurement(Some(1), None, 0.5, &session.measurement_stamp())
        .unwrap();
    assert!(session.camera().position.is_finite());
}

fn surface_fixture() -> Session {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
}

fn arm(session: &mut Session, actor: Actor, token: u64, endpoint: Endpoint) -> u64 {
    let state = session
        .begin_measurement_pick(
            actor,
            token,
            endpoint,
            session.camera(),
            (511, 511),
            &session.measurement_pick_stamp(),
        )
        .unwrap();
    state["draft"]["token"].as_u64().unwrap()
}

fn precise_ray() -> Ray {
    Ray {
        origin: DVec3::new(3.123456789012, -5.987654321012, 96.0),
        direction: DVec3::NEG_Z * 7.0,
    }
}

#[test]
fn endpoint_surface_picks_preserve_numeric_precision_and_commit_only_on_apply() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = surface_fixture();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let selection = session.selected_ids();
        let token = preview(&mut session, actor, request(None));
        let token = arm(&mut session, actor, token, Endpoint::Start);
        let state = session
            .pick_measurement_ray(actor, token, precise_ray())
            .unwrap();
        let picked = [precise_ray().origin.x, precise_ray().origin.y, 0.0];
        assert_eq!(state["draft"]["measurement"]["start"], json!(picked));
        assert_eq!(
            state["draft"]["measurement"]["end"],
            json!(request(None).end.to_array())
        );
        assert_eq!(
            state["draft"]["last_pick"]["normal"],
            json!([0.0, 0.0, 1.0])
        );
        assert_eq!(state["draft"]["last_pick"]["endpoint"], "start");
        assert!(state["draft"]["picking"].is_null());
        let current = state["draft"]["token"].as_u64().unwrap();
        assert_ne!(token, current);
        assert!(session.cancel_measurement(actor, token).is_err());
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.selected_ids(), selection);
        session.apply_measurement(actor, current).unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        assert_eq!(
            session.measurements().unwrap().measurements[0]
                .start
                .to_array(),
            picked
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
        session.redo(actor).unwrap();
        assert_eq!(
            session.measurements().unwrap().measurements[0]
                .start
                .to_array(),
            picked
        );
    }
}

#[test]
fn endpoint_misses_invalid_rays_coincidence_and_foreign_actors_preserve_the_entire_draft() {
    let mut session = surface_fixture();
    let mut ruler = request(None);
    ruler.end = DVec3::new(precise_ray().origin.x, precise_ray().origin.y, 0.0);
    let token = preview(&mut session, Actor::Agent, ruler);
    let token = arm(&mut session, Actor::Agent, token, Endpoint::Start);
    let original = session.measurement_state();
    for ray in [
        precise_ray(),
        Ray {
            origin: DVec3::splat(1000.0),
            direction: DVec3::Z,
        },
        Ray {
            origin: DVec3::ZERO,
            direction: DVec3::ZERO,
        },
        Ray {
            origin: DVec3::splat(f64::NAN),
            direction: DVec3::NEG_Z,
        },
        Ray {
            origin: DVec3::Z * 96.0,
            direction: DVec3::splat(f64::INFINITY),
        },
    ] {
        assert!(
            session
                .pick_measurement_ray(Actor::Agent, token, ray)
                .is_err()
        );
        assert_eq!(session.measurement_state(), original);
    }
    assert!(
        session
            .pick_measurement_ray(Actor::User, token, precise_ray())
            .is_err()
    );
    assert!(session.cancel_measurement_pick(Actor::User, token).is_err());
    assert!(session.apply_measurement(Actor::Agent, token).is_err());
    assert_eq!(session.measurement_state(), original);
    let reviewed = session.review_measurement(Actor::User, token).unwrap();
    let current = reviewed["draft"]["token"].as_u64().unwrap();
    assert!(
        session
            .cancel_measurement_pick(Actor::Agent, token)
            .is_err()
    );
    let cancelled = session
        .cancel_measurement_pick(Actor::User, current)
        .unwrap();
    assert!(cancelled["draft"]["picking"].is_null());
    assert_eq!(
        cancelled["draft"]["measurement"],
        original["draft"]["measurement"]
    );
    session
        .apply_measurement(Actor::User, cancelled["draft"]["token"].as_u64().unwrap())
        .unwrap();
}

#[test]
fn endpoint_picks_guard_scene_camera_map_and_viewport_but_allow_owned_stale_cleanup() {
    for change in ["camera", "published_camera", "view", "assets", "map"] {
        let mut session = surface_fixture();
        let token = preview(&mut session, Actor::User, request(None));
        let token = arm(&mut session, Actor::User, token, Endpoint::End);
        let before = session.measurement_state()["draft"]["measurement"].clone();
        match change {
            "camera" => {
                let mut camera = session.camera();
                camera.position.x += 1.0;
                session.set_camera(camera);
            }
            "published_camera" => {
                let mut camera = session.camera();
                camera.position.y += 1.0;
                session.publish_camera(camera);
            }
            "view" => session.set_view(render::ViewOptions {
                tool_textures: !session.view().tool_textures,
                ..session.view()
            }),
            "assets" => {
                session.refresh_model_assets(Actor::Agent);
            }
            "map" => {
                session
                    .create_block(
                        Actor::Agent,
                        DVec3::splat(200.0),
                        DVec3::splat(216.0),
                        "brick/test",
                        false,
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let state = session.measurement_state();
        assert_eq!(state["draft"]["picking"]["valid"], false, "{change}");
        assert!(
            session
                .pick_measurement_ray(Actor::User, token, precise_ray())
                .is_err(),
            "{change}"
        );
        assert_eq!(session.measurement_state(), state);
        let cancelled = session.cancel_measurement_pick(Actor::User, token).unwrap();
        assert_eq!(cancelled["draft"]["measurement"], before);
        session
            .cancel_measurement(Actor::User, cancelled["draft"]["token"].as_u64().unwrap())
            .unwrap();
    }
    let mut session = surface_fixture();
    let token = preview(&mut session, Actor::User, request(None));
    let stale = session.measurement_pick_stamp();
    session.set_camera(render::Camera::planar(
        Projection::Top,
        glam::Vec3::ZERO,
        128.0,
    ));
    assert!(
        session
            .begin_measurement_pick(
                Actor::User,
                token,
                Endpoint::End,
                session.camera(),
                (511, 511),
                &stale
            )
            .is_err()
    );
    let token = arm(&mut session, Actor::User, token, Endpoint::End);
    let state = session.measurement_state();
    for pointer in [
        Pointer {
            position: glam::DVec2::splat(255.5),
            size: (512, 511),
        },
        Pointer {
            position: glam::DVec2::splat(-0.5),
            size: (511, 511),
        },
        Pointer {
            position: glam::DVec2::splat(511.0),
            size: (511, 511),
        },
        Pointer {
            position: glam::DVec2::splat(f64::NAN),
            size: (511, 511),
        },
    ] {
        assert!(
            session
                .pick_measurement_pixel(Actor::User, token, pointer)
                .is_err()
        );
        assert_eq!(session.measurement_state(), state);
    }
}

#[test]
fn viewport_pixels_use_the_captured_temporary_camera_in_every_projection() {
    for projection in [
        Projection::Perspective,
        Projection::Top,
        Projection::Front,
        Projection::Side,
    ] {
        let mut session = surface_fixture();
        if matches!(projection, Projection::Front | Projection::Side) {
            session
                .create_block(
                    Actor::User,
                    DVec3::new(32.0, 32.0, 0.0),
                    DVec3::new(48.0, 48.0, 64.0),
                    "brick/test",
                    false,
                )
                .unwrap();
        }
        let token = preview(&mut session, Actor::Agent, request(None));
        let camera = if projection == Projection::Perspective {
            render::Camera::looking_at(
                glam::Vec3::new(3.125, -5.25, 96.0),
                glam::Vec3::new(3.125, -5.25, 0.0),
            )
        } else {
            render::Camera::planar(projection, glam::Vec3::new(40.0, 40.0, 32.0), 128.0)
        };
        let user_camera = session.camera();
        let state = session
            .begin_measurement_pick(
                Actor::Agent,
                token,
                Endpoint::End,
                camera,
                (511, 511),
                &session.measurement_pick_stamp(),
            )
            .unwrap();
        let token = state["draft"]["token"].as_u64().unwrap();
        let (origin, direction) = camera.ray(0.0, 0.0, 1.0);
        let expected = map_core::query::pick_surface(
            session.document(),
            &Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
            |_| true,
        )
        .unwrap();
        let picked = session
            .pick_measurement_pixel(
                Actor::Agent,
                token,
                Pointer {
                    position: glam::DVec2::splat(255.5),
                    size: (511, 511),
                },
            )
            .unwrap();
        assert_eq!(
            picked["draft"]["measurement"]["end"],
            json!(expected.hit.point.to_array()),
            "{projection:?}"
        );
        assert_eq!(session.camera(), user_camera);
        assert_eq!(
            session.document().undo_labels().count(),
            if matches!(projection, Projection::Front | Projection::Side) {
                2
            } else {
                1
            }
        );
    }
}

#[test]
fn endpoint_picks_use_raised_terrain_and_never_use_hidden_surfaces_or_entity_helpers() {
    let mut session = surface_fixture();
    let map_core::ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::new(-64.0, -64.0, 16.0),
            DVec3::new(64.0, 64.0, 32.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let shape =
        map_core::BrushGeometry::from_solid(session.document().solid(solid).unwrap()).unwrap();
    let face = shape
        .faces
        .iter()
        .find(|face| face.plane.normal.z > 0.99)
        .unwrap();
    let side = session.document().solid(solid).unwrap().sides[face.side].id;
    session
        .edit(Actor::User, "raised terrain", |tx| {
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid, side }],
                2,
                0,
            )?;
            tx.solid_mut(solid)?
                .sides
                .iter_mut()
                .find(|face| face.id == side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap()
                .elevation = 16.0;
            Ok(())
        })
        .unwrap();
    let source = session.document().vmf().clone();
    let token = preview(&mut session, Actor::User, request(None));
    let token = arm(&mut session, Actor::User, token, Endpoint::Start);
    let state = session
        .pick_measurement_ray(Actor::User, token, precise_ray())
        .unwrap();
    assert_eq!(state["draft"]["last_pick"]["solid"], solid);
    assert_eq!(state["draft"]["measurement"]["start"][2], 48.0);
    assert_eq!(session.document().vmf(), &source);
    session
        .cancel_measurement(Actor::User, state["draft"]["token"].as_u64().unwrap())
        .unwrap();
    session
        .set_visibility(Actor::User, "hide", &[map_core::ObjectId::Solid(solid)])
        .unwrap();
    let token = preview(&mut session, Actor::User, request(None));
    let token = arm(&mut session, Actor::User, token, Endpoint::Start);
    let state = session
        .pick_measurement_ray(Actor::User, token, precise_ray())
        .unwrap();
    assert_eq!(state["draft"]["measurement"]["start"][2], 0.0);
    session
        .cancel_measurement(Actor::User, state["draft"]["token"].as_u64().unwrap())
        .unwrap();
    let floor = session
        .document()
        .objects()
        .find(|id| matches!(id,map_core::ObjectId::Solid(id) if *id!=solid))
        .unwrap();
    session
        .set_visibility(Actor::User, "hide", &[floor])
        .unwrap();
    session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(precise_ray().origin - DVec3::Z * 32.0),
            std::collections::BTreeMap::from([("model".into(), "models/missing.mdl".into())]),
        )
        .unwrap();
    let token = preview(&mut session, Actor::User, request(None));
    let token = arm(&mut session, Actor::User, token, Endpoint::Start);
    let state = session.measurement_state();
    assert!(
        session
            .pick_measurement_ray(Actor::User, token, precise_ray())
            .is_err()
    );
    assert_eq!(session.measurement_state(), state);
}

#[test]
fn endpoint_arming_preserves_competing_entity_and_geometry_drafts() {
    let mut session = surface_fixture();
    let token = preview(&mut session, Actor::Agent, request(None));
    let entity = session
        .begin_entity_placement(
            Actor::User,
            super::super::entity_placement::Request {
                classname: "info_target".into(),
                properties: Default::default(),
                offset: 0.0,
                align: false,
                snap: false,
            },
            &session.entity_placement_stamp(),
        )
        .unwrap();
    let state = session.measurement_state();
    assert!(
        session
            .begin_measurement_pick(
                Actor::Agent,
                token,
                Endpoint::End,
                session.camera(),
                (511, 511),
                &session.measurement_pick_stamp()
            )
            .is_err()
    );
    assert_eq!(session.get_entity_placement(), entity);
    assert_eq!(session.measurement_state(), state);
    session
        .cancel_entity_placement(Actor::User, entity["draft"]["token"].as_u64().unwrap())
        .unwrap();
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let block = session.block_preview;
    assert!(
        session
            .begin_measurement_pick(
                Actor::Agent,
                token,
                Endpoint::End,
                session.camera(),
                (511, 511),
                &session.measurement_pick_stamp()
            )
            .is_err()
    );
    assert_eq!(session.block_preview, block);
    assert_eq!(session.measurement_state()["draft"]["token"], token);
}

#[test]
fn endpoint_picks_hit_actual_model_triangles_and_skip_hidden_models() {
    let Ok(game) = crate::startup::mount_game(None) else {
        return;
    };
    let mut session = Session::new(Ok(game));
    let id = session
        .create_entity(
            Actor::Agent,
            "prop_static",
            Some(DVec3::new(64.0, 32.0, 16.0)),
            std::collections::BTreeMap::from([
                (
                    "model".into(),
                    "models/props_junk/wood_crate001a.mdl".into(),
                ),
                ("angles".into(), "0 27 0".into()),
            ]),
        )
        .unwrap();
    let instances = session.model_instances(session.document());
    let instance = &instances.instances[0];
    let points = instance.asset.model.parts[0].indices[..3]
        .iter()
        .map(|&index| instance.point(instance.asset.model.vertices[index as usize].position))
        .collect::<Vec<_>>();
    let center = (points[0] + points[1] + points[2]) / 3.0;
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .normalize();
    let ray = Ray {
        origin: center - normal * 16.0,
        direction: normal,
    };
    let expected = instance.pick_surface(&ray).unwrap();
    drop(instances);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let token = preview(&mut session, Actor::Agent, request(None));
    let token = arm(&mut session, Actor::Agent, token, Endpoint::End);
    let state = session
        .pick_measurement_ray(Actor::Agent, token, ray)
        .unwrap();
    assert_eq!(
        state["draft"]["last_pick"]["target"],
        crate::identity::id_string(id)
    );
    let draft = session.measurements.draft.as_ref().unwrap();
    let stored = draft.measurement.clone();
    let (_, surface) = draft.last_pick.unwrap();
    assert_eq!(stored.start, request(None).start);
    assert_eq!(stored.end, surface.hit.point);
    assert_eq!(
        state["draft"]["measurement"]["end"],
        json!(stored.end.to_array())
    );
    for (actual, expected) in stored
        .end
        .to_array()
        .into_iter()
        .zip(expected.hit.point.to_array())
    {
        assert!(
            (expected.next_down()..=expected.next_up()).contains(&actual),
            "Stored surface point {actual} differs from independent model intersection {expected}"
        );
    }
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    session
        .apply_measurement(Actor::Agent, state["draft"]["token"].as_u64().unwrap())
        .unwrap();
    let saved = session.measurements().unwrap().measurements[0].clone();
    assert_eq!(saved.start, stored.start);
    assert_eq!(saved.end, stored.end);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
    session.set_visibility(Actor::Agent, "hide", &[id]).unwrap();
    let token = preview(&mut session, Actor::Agent, request(None));
    let token = arm(&mut session, Actor::Agent, token, Endpoint::End);
    let state = session.measurement_state();
    assert!(
        session
            .pick_measurement_ray(Actor::Agent, token, ray)
            .is_err()
    );
    assert_eq!(session.measurement_state(), state);
}

#[test]
fn endpoint_arming_rejects_live_paint_area_volume_and_transform_operations_atomically() {
    for actor in [Actor::User, Actor::Agent] {
        for operation in 0..4 {
            let mut session = surface_fixture();
            session.set_camera(Camera::planar(Projection::Top, glam::Vec3::ZERO, 256.0));
            let ruler_token = preview(&mut session, actor, request(None));
            let pointer = Pointer {
                position: glam::DVec2::new(100.0, 100.0),
                size: (511, 511),
            };
            match operation {
                0 => {
                    session
                        .begin_paint_selection(super::super::paint::Spec {
                            camera: session.camera(),
                            pointer,
                            radius: 6.0,
                            mode: super::super::SelectMode::Replace,
                            faces: false,
                        })
                        .unwrap();
                }
                1 => {
                    session
                        .preview_area_selection(
                            Actor::User,
                            session.camera(),
                            pointer,
                            glam::DVec2::new(300.0, 300.0),
                            map_core::region::Match::Intersecting,
                            super::super::SelectMode::Replace,
                        )
                        .unwrap();
                }
                2 => {
                    session
                        .preview_volume_selection(super::super::volume::Spec {
                            mins: DVec3::splat(-32.0),
                            maxs: DVec3::splat(32.0),
                            rule: map_core::region::Match::Intersecting,
                            mode: super::super::SelectMode::Replace,
                            faces: false,
                        })
                        .unwrap();
                }
                3 => {
                    let mut tool = session.transform_tool();
                    tool.mode = Some(render::handles::Mode::Move);
                    session.set_transform_tool(actor, tool).unwrap();
                    session
                        .begin_transform_drag(render::handles::Handle::X, None)
                        .unwrap();
                    assert!(session.transform_preview().is_none());
                }
                _ => unreachable!(),
            }
            let state = session.measurement_state();
            let source = session.document().vmf().clone();
            let selection = session.selected_ids();
            let history = session.document().undo_labels().count();
            let paint = session.paint_selection_json(0, 50);
            let volume = session.volume_selection_json(0, 50);
            let scene_serial = session.scene_serial();
            let view_serial = session.view_serial();
            let error = session
                .begin_measurement_pick(
                    actor,
                    ruler_token,
                    Endpoint::End,
                    session.camera(),
                    pointer.size,
                    &session.measurement_pick_stamp(),
                )
                .unwrap_err();
            assert!(error.contains("active viewport tool"), "{error}");
            assert_eq!(session.measurement_state(), state);
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.selected_ids(), selection);
            assert_eq!(session.document().undo_labels().count(), history);
            assert_eq!(session.paint_selection_json(0, 50), paint);
            assert_eq!(session.volume_selection_json(0, 50), volume);
            assert_eq!(session.scene_serial(), scene_serial);
            assert_eq!(session.view_serial(), view_serial);
            match operation {
                0 => {
                    let token = session.paint_stroke().unwrap().token;
                    session
                        .update_paint_selection(token, &[glam::DVec2::new(110.0, 110.0)])
                        .unwrap();
                    session.commit_paint_selection(actor, token).unwrap();
                }
                1 => {
                    session.commit_area_selection().unwrap();
                }
                2 => {
                    let token = session.volume_preview().unwrap().token;
                    session.commit_volume_selection(actor, token).unwrap();
                }
                3 => {
                    assert!(session.transform_drag().is_some());
                }
                _ => unreachable!(),
            }
        }
    }
}
