use super::*;
use source_formats::fgd::Catalog;

const FGD: &str = r#"
@BaseClass = LightValues [
    _light(color255) : "Brightness" : "255 240 220 350"
    angles(angle) : "Orientation" : "0 30 0"
    pitch(float) : "Pitch" : -25
    spawnflags(flags) = [ 1 : "Enabled" : 1 ]
]
@PointClass base(LightValues) = light_spot []
@SolidClass = func_detail []
"#;

fn fixture() -> Session {
    let mut session = Session::new(Err("No assets needed".into()));
    session.set_definitions(Actor::User, Catalog::parse(FGD).unwrap());
    for (low, high) in [
        ([-128.0, -128.0, -16.0], [128.0, 128.0, 0.0]),
        ([-128.0, 64.0, 0.0], [128.0, 80.0, 128.0]),
    ] {
        session
            .create_block(
                Actor::User,
                DVec3::from_array(low),
                DVec3::from_array(high),
                "brick/test",
                false,
            )
            .unwrap();
    }
    session
}

#[test]
fn resource_visibility_and_repeated_values_preserve_entity_placement_review() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = fixture();
        let token = begin(&mut session, actor, request("info_target"));
        let expected = session
            .pick_entity_placement(actor, token, floor_ray())
            .unwrap();
        let document = session.render_document().vmf().clone();
        let source = session.document().vmf().clone();
        let serial = session.scene_serial();
        for enabled in [false, false, true] {
            session.set_view(render::ViewOptions {
                entity_images: enabled,
                entity_model_guides: enabled,
                ..session.view()
            });
            assert_eq!(session.get_entity_placement(), expected);
            assert_eq!(session.render_document().vmf(), &document);
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.scene_serial(), serial);
        }
        session.cancel_entity_placement(actor, token).unwrap();
    }
}

fn request(classname: &str) -> Request {
    Request {
        classname: classname.into(),
        properties: BTreeMap::new(),
        offset: 2.5,
        align: true,
        snap: false,
    }
}

fn begin(session: &mut Session, actor: Actor, request: Request) -> u64 {
    let report = session
        .begin_entity_placement(actor, request, &session.entity_placement_stamp())
        .unwrap();
    report["draft"]["token"].as_u64().unwrap()
}

fn floor_ray() -> Ray {
    Ray {
        origin: DVec3::new(3.1, -5.7, 96.0),
        direction: DVec3::NEG_Z,
    }
}
fn wall_ray() -> Ray {
    Ray {
        origin: DVec3::new(3.1, 0.0, 60.0),
        direction: DVec3::Y,
    }
}

#[test]
fn point_entity_placement_reuses_creation_contact_defaults_and_source_light_orientation() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = fixture();
        let mut direct = fixture();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let selection = session.selected_ids();
        let token = begin(&mut session, actor, request("LIGHT_SPOT"));
        let report = session
            .pick_entity_placement(actor, token, wall_ray())
            .unwrap();
        let plan = &report["draft"]["plan"];
        let id = direct
            .create_entity(actor, "LIGHT_SPOT", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let expected = direct
            .preview_surface_placement(
                actor,
                &[id],
                Method::Ray,
                Some(wall_ray()),
                Options {
                    align: true,
                    offset: 2.5,
                    texture_lock: false,
                    source_corner: None,
                },
            )
            .unwrap();
        assert!((plan["contact_offset"].as_f64().unwrap() - 2.5).abs() < SURFACE_EPSILON);
        assert_eq!(plan["normal"], expected["plan"]["normal"]);
        let ObjectId::Entity(number) = id else {
            unreachable!()
        };
        let preview_entity = session.render_document().entity(number).unwrap().clone();
        assert_eq!(
            Some(&preview_entity),
            direct.render_document().entity(number)
        );
        assert_eq!(
            preview_entity.properties.get("_light"),
            Some("255 240 220 350")
        );
        assert_eq!(preview_entity.properties.get("spawnflags"), Some("1"));
        let pitch = preview_entity
            .properties
            .get("pitch")
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let angles =
            source_formats::vmf::parse_vec3(preview_entity.properties.get("angles").unwrap())
                .unwrap();
        assert!((pitch + angles.x).abs() < SURFACE_EPSILON);
        assert!(
            query::local_basis(session.render_document(), id)
                .unwrap()
                .z_axis
                .abs_diff_eq(DVec3::NEG_Y, SURFACE_EPSILON)
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.selected_ids(), selection);
        assert!(session.transform_guide().is_none());
        let mesh = session.build_scene();
        assert!(
            mesh.lines
                .iter()
                .any(|line| line.color == [80, 200, 255, 255])
        );
        let applied = session.apply_entity_placement(actor, token).unwrap();
        assert_eq!(applied["created"], crate::identity::id_string(id));
        assert_eq!(session.document().entity(number), Some(&preview_entity));
        assert_eq!(session.document().undo_labels().count(), history + 1);
        assert_eq!(session.selected_ids(), vec![id]);
        assert!(session.get_entity_placement()["draft"].is_null());
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
        session.redo(actor).unwrap();
        assert_eq!(session.document().entity(number), Some(&preview_entity));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("placement.vmf");
        session.save(actor, Some(&path)).unwrap();
        session.open(actor, &path, true).unwrap();
        assert_eq!(session.document().entity(number), Some(&preview_entity));
    }
}

#[test]
fn point_entity_grid_snap_rechecks_actual_surface_and_preserves_last_valid_draft() {
    let mut session = fixture();
    let low = DVec3::new(-64.0, -64.0, 160.0);
    let id = session
        .create_block(
            Actor::User,
            low,
            low + DVec3::new(128.0, 128.0, 8.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .edit(Actor::User, "sloped target", |tx| {
            edit::transform(
                tx,
                &[id],
                edit::Transform {
                    rotation: DVec3::new(0.0, 20.0, 0.0),
                    pivot: low,
                    ..edit::Transform::default()
                },
            )
        })
        .unwrap();
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(0.25),
                ..Default::default()
            },
        )
        .unwrap();
    let mut options = request("info_target");
    options.snap = true;
    let token = begin(&mut session, Actor::User, options.clone());
    let ray = Ray {
        origin: DVec3::new(3.1, -5.7, 256.0),
        direction: DVec3::NEG_Z,
    };
    let good = session
        .pick_entity_placement(Actor::User, token, ray)
        .unwrap();
    let plan = &good["draft"]["plan"];
    assert_eq!(plan["target"], crate::identity::id_string(id));
    assert!((plan["point"][0].as_f64().unwrap() - 3.0).abs() < SURFACE_EPSILON);
    assert!((plan["point"][1].as_f64().unwrap() + 5.75).abs() < SURFACE_EPSILON);
    assert!((plan["normal"][0].as_f64().unwrap().abs()) > 0.3);
    for ray in [
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
            direction: DVec3::Z,
        },
    ] {
        assert!(
            session
                .pick_entity_placement(Actor::User, token, ray)
                .is_err()
        );
        assert_eq!(session.get_entity_placement(), good);
    }
    for class in ["func_detail", "info_overlay", "infodecal", ""] {
        let mut invalid = options.clone();
        invalid.classname = class.into();
        assert!(
            session
                .begin_entity_placement(Actor::User, invalid, &session.entity_placement_stamp())
                .is_err()
        );
        assert_eq!(session.get_entity_placement(), good);
    }
    for offset in [f64::NAN, f64::INFINITY, crate::spatial::MAX_EXTENT + 1.0] {
        let mut invalid = options.clone();
        invalid.offset = offset;
        assert!(
            session
                .begin_entity_placement(Actor::User, invalid, &session.entity_placement_stamp())
                .is_err()
        );
        assert_eq!(session.get_entity_placement(), good);
    }
    session.cancel_entity_placement(Actor::User, token).unwrap();
    let tiny = session
        .create_block(
            Actor::User,
            DVec3::new(2.0, 2.0, 320.0),
            DVec3::new(4.0, 4.0, 328.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(16.0),
                ..Default::default()
            },
        )
        .unwrap();
    let token = begin(&mut session, Actor::User, options);
    assert!(
        session
            .pick_entity_placement(
                Actor::User,
                token,
                Ray {
                    origin: DVec3::new(3.0, 3.0, 360.0),
                    direction: DVec3::NEG_Z
                }
            )
            .is_err()
    );
    assert_eq!(session.get_entity_placement()["draft"]["picking"], true);
    assert!(session.document().contains(tiny));
}

#[test]
fn point_entity_tokens_own_review_cancel_and_preserve_replacement_history() {
    let mut session = fixture();
    session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::new(48.0, 0.0, 48.0)),
            BTreeMap::new(),
        )
        .unwrap();
    session.undo(Actor::User).unwrap();
    let solid = session.document().vmf().world.solids[0].id;
    session
        .select(&[ObjectId::Solid(solid)], SelectMode::Replace)
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let redo = session.document().redo_labels().count();
    let token = begin(&mut session, Actor::Agent, request("info_target"));
    session
        .pick_entity_placement(Actor::Agent, token, floor_ray())
        .unwrap();
    assert!(session.apply_entity_placement(Actor::User, token).is_err());
    assert!(session.cancel_entity_placement(Actor::User, token).is_err());
    let reviewed = session.review_entity_placement(Actor::User, token).unwrap();
    let human_token = reviewed["draft"]["token"].as_u64().unwrap();
    assert_ne!(token, human_token);
    assert!(session.apply_entity_placement(Actor::Agent, token).is_err());
    assert!(
        session
            .cancel_entity_placement(Actor::Agent, token)
            .is_err()
    );
    session
        .cancel_entity_placement(Actor::User, human_token)
        .unwrap();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.document().redo_labels().count(), redo);
    let token = begin(&mut session, Actor::User, request("info_target"));
    session
        .pick_entity_placement(Actor::User, token, floor_ray())
        .unwrap();
    let selected = session.selected_ids();
    session
        .preview_transform(
            Actor::Agent,
            &selected,
            edit::Transform {
                translation: DVec3::X,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let replacement = session.render_document().vmf().clone();
    assert!(session.apply_entity_placement(Actor::User, token).is_err());
    assert!(session.cancel_entity_placement(Actor::User, token).is_err());
    assert_eq!(session.render_document().vmf(), &replacement);
}

#[test]
fn point_entity_guards_grid_map_generation_bounds_and_save_build_play() {
    let mut session = fixture();
    let token = begin(&mut session, Actor::Agent, request("info_target"));
    assert!(session.apply_entity_placement(Actor::Agent, token).is_err());
    assert!(session.begin_compile(Actor::Agent, None).is_err());
    assert!(
        session
            .launch_readiness()
            .unwrap_err()
            .contains("entity placement")
    );
    assert!(
        session
            .save(Actor::Agent, None)
            .unwrap_err()
            .contains("entity placement")
    );
    session
        .pick_entity_placement(Actor::Agent, token, floor_ray())
        .unwrap();
    let old_snapshot = session.entity_placement_stamp();
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(8.0),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.apply_entity_placement(Actor::Agent, token).is_err());
    assert!(
        session
            .begin_entity_placement(Actor::Agent, request("info_target"), &old_snapshot)
            .is_err()
    );
    let token = begin(&mut session, Actor::Agent, request("info_target"));
    let mut invalid = request("info_target");
    invalid.offset = crate::spatial::MAX_EXTENT;
    let invalid_token = begin(&mut session, Actor::Agent, invalid);
    assert_ne!(token, invalid_token);
    assert!(
        session
            .pick_entity_placement(Actor::Agent, invalid_token, floor_ray())
            .is_err()
    );
    assert_eq!(session.get_entity_placement()["draft"]["picking"], true);
    let old_generation = session.document_generation();
    session.new_map(Actor::Agent, true).unwrap();
    assert_ne!(session.document_generation(), old_generation);
    assert!(
        session
            .apply_entity_placement(Actor::Agent, invalid_token)
            .is_err()
    );
    let new_token = begin(&mut session, Actor::Agent, request("info_target"));
    assert!(new_token > invalid_token);
    assert!(
        session
            .cancel_entity_placement(Actor::Agent, invalid_token)
            .is_err()
    );
    assert_eq!(session.get_entity_placement()["draft"]["token"], new_token);
}

#[test]
fn point_entity_pick_uses_actual_terrain_sheet_and_skips_hidden_targets() {
    let mut session = fixture();
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::new(-64.0, -64.0, 128.0),
            DVec3::new(64.0, 64.0, 144.0),
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
        .edit(Actor::User, "raised terrain sheet", |tx| {
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid, side }],
                2,
                0,
            )?;
            let brush = tx.solid_mut(solid)?;
            let info = brush
                .sides
                .iter_mut()
                .find(|face| face.id == side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap();
            info.elevation = 16.0;
            Ok(())
        })
        .unwrap();
    let ray = Ray {
        origin: DVec3::new(3.1, -5.7, 256.0),
        direction: DVec3::NEG_Z,
    };
    let token = begin(&mut session, Actor::Agent, request("info_target"));
    let report = session
        .pick_entity_placement(Actor::Agent, token, ray)
        .unwrap();
    assert_eq!(report["draft"]["plan"]["solid"], solid);
    assert!(
        (report["draft"]["plan"]["point"][2].as_f64().unwrap() - 160.0).abs() < SURFACE_EPSILON
    );
    session
        .cancel_entity_placement(Actor::Agent, token)
        .unwrap();
    session
        .edit(Actor::User, "hide terrain", |tx| {
            tx.solid_mut(solid)?.hidden = true;
            Ok(())
        })
        .unwrap();
    let token = begin(&mut session, Actor::Agent, request("info_target"));
    let report = session
        .pick_entity_placement(Actor::Agent, token, ray)
        .unwrap();
    assert_ne!(report["draft"]["plan"]["solid"], solid);
    assert_eq!(report["draft"]["plan"]["point"][2], 0.0);
}

#[test]
fn point_entity_stale_content_and_visibility_allow_owned_rearm_and_review_cleanup() {
    let mut session = fixture();
    let token = begin(&mut session, Actor::Agent, request("info_target"));
    session
        .pick_entity_placement(Actor::Agent, token, floor_ray())
        .unwrap();
    let old_snapshot = session.entity_placement_stamp();
    session.refresh_model_assets(Actor::Agent);
    assert_eq!(session.get_entity_placement()["draft"]["valid"], false);
    assert!(
        session
            .begin_entity_placement(Actor::Agent, request("info_target"), &old_snapshot)
            .is_err()
    );
    let reviewed = session.review_entity_placement(Actor::User, token).unwrap();
    let token = reviewed["draft"]["token"].as_u64().unwrap();
    assert_eq!(reviewed["draft"]["valid"], false);
    assert!(session.apply_entity_placement(Actor::User, token).is_err());
    let token = begin(&mut session, Actor::User, request("info_target"));
    session
        .pick_entity_placement(Actor::User, token, floor_ray())
        .unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: !session.view().tool_textures,
        ..session.view()
    });
    assert_eq!(session.get_entity_placement()["draft"]["valid"], false);
    assert!(session.apply_entity_placement(Actor::User, token).is_err());
    let token = begin(&mut session, Actor::User, request("info_target"));
    let report = session
        .pick_entity_placement(Actor::User, token, floor_ray())
        .unwrap();
    assert_eq!(report["draft"]["valid"], true);
    assert!(
        session
            .pick_entity_placement(Actor::User, token, wall_ray())
            .is_err()
    );
    assert_eq!(session.get_entity_placement(), report);
    session.cancel_entity_placement(Actor::User, token).unwrap();
}
