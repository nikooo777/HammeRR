use super::*;

use super::fixtures::fixture;

fn options() -> Options {
    Options {
        align: false,
        offset: 0.0,
        texture_lock: true,
    }
}

#[test]
fn owned_brush_placement_uses_the_nearest_child_surface_and_keeps_atomic_history() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("Brush placement needs no assets".into()));
        session
            .create_block(
                actor,
                DVec3::new(-32.0, -32.0, -16.0),
                DVec3::new(32.0, 32.0, 0.0),
                "brick/test",
                false,
            )
            .unwrap();
        let (owner, solid) = session
            .edit(actor, "owned placement target", |tx| {
                let solid = map_core::edit::block_solid(
                    tx,
                    DVec3::new(-32.0, -32.0, 16.0),
                    DVec3::new(32.0, 32.0, 24.0),
                    "brick/test",
                )?;
                let id = solid.id;
                let owner =
                    map_core::edit::create_entity(tx, "func_detail", None, &[], vec![solid])?;
                Ok((owner, id))
            })
            .unwrap();
        let selected = session
            .create_block(
                actor,
                DVec3::new(-4.0, -4.0, 48.0),
                DVec3::new(4.0, 4.0, 56.0),
                "brick/test",
                false,
            )
            .unwrap();
        session
            .select(&[selected], super::super::SelectMode::Replace)
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session
            .preview_surface_placement(actor, &[selected], Method::Floor, None, options())
            .unwrap();
        assert_eq!(report["plan"]["target"], object_id(owner));
        assert_eq!(report["plan"]["solid"], solid);
        assert_eq!(report["plan"]["point"], json!([0.0, 0.0, 24.0]));
        assert_eq!(report["plan"]["normal"], json!([0.0, 0.0, 1.0]));
        assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, -24.0]));
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .apply_surface_placement(actor, report["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        assert_eq!(
            query::bounds(session.document(), selected).unwrap().0.z,
            24.0
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
}

#[test]
fn surface_placement_floor_ceiling_alignment_and_undo_share_contact_math() {
    let (mut session, id) = fixture();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let report = session
        .preview_surface_placement(Actor::Agent, &[id], Method::Floor, None, options())
        .unwrap();
    assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, -32.0]));
    assert_eq!(report["plan"]["normal"], json!([0.0, 0.0, 1.0]));
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
    let token = report["token"].as_u64().unwrap();
    session.apply_surface_placement(Actor::User, token).unwrap();
    assert_eq!(session.document().undo_labels().count(), history + 1);
    assert!(query::bounds(session.document(), id).unwrap().0.z.abs() < 1e-8);
    let history = session.document().undo_labels().count();
    let report = session
        .preview_surface_placement(Actor::User, &[id], Method::Floor, None, options())
        .unwrap();
    assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, 0.0]));
    session
        .apply_surface_placement(Actor::User, report["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(session.document().undo_labels().count(), history);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
    let report = session
        .preview_surface_placement(Actor::User, &[id], Method::Ceiling, None, options())
        .unwrap();
    assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, 80.0]));
    session
        .cancel_surface_placement(report["token"].as_u64().unwrap())
        .unwrap();
    let report = session
        .preview_surface_placement(
            Actor::Agent,
            &[id],
            Method::Ray,
            Some(Ray {
                origin: DVec3::new(0.0, 0.0, 64.0),
                direction: DVec3::Y,
            }),
            Options {
                align: true,
                offset: 4.0,
                ..options()
            },
        )
        .unwrap();
    assert!((report["plan"]["contact_offset"].as_f64().unwrap() - 4.0).abs() < 1e-8);
    let (_, high) = query::bounds(session.render_document(), id).unwrap();
    assert!((high.y - 60.0).abs() < 1e-8);
    session
        .apply_surface_placement(Actor::Agent, report["token"].as_u64().unwrap())
        .unwrap();
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn surface_placement_misses_and_foreign_transforms_preserve_owned_state() {
    let (mut session, id) = fixture();
    let source = session.document().vmf().clone();
    let report = session
        .preview_surface_placement(Actor::User, &[id], Method::Floor, None, options())
        .unwrap();
    let token = report["token"].as_u64().unwrap();
    let miss = Ray {
        origin: DVec3::splat(1000.0),
        direction: DVec3::Z,
    };
    assert!(
        session
            .preview_surface_placement(Actor::Agent, &[id], Method::Ray, Some(miss), options())
            .is_err()
    );
    assert_eq!(session.surface_placement()["token"], token);
    let transform = session.transform_preview().unwrap().transform;
    session
        .preview_transform(Actor::Agent, &[id], transform, Some(transform.pivot))
        .unwrap();
    assert!(session.surface_transform_owner().is_none());
    let foreign = session.render_document().vmf().clone();
    assert!(
        session
            .apply_surface_placement(Actor::Agent, token)
            .is_err()
    );
    session.cancel_surface_placement(token).unwrap();
    assert_eq!(session.render_document().vmf(), &foreign);
    session.cancel_transform_preview();
    let pick = session
        .begin_surface_placement_pick(&[id], options())
        .unwrap();
    let token = pick["token"].as_u64().unwrap();
    assert!(
        session
            .pick_surface_placement(Actor::User, token, miss)
            .is_err()
    );
    assert_eq!(session.surface_placement()["picking"], true);
    assert!(session.cancel_surface_placement(token + 1).is_err());
    session
        .pick_surface_placement(
            Actor::Agent,
            token,
            Ray {
                origin: DVec3::new(0.0, 0.0, 100.0),
                direction: DVec3::NEG_Z,
            },
        )
        .unwrap();
    assert_eq!(session.surface_placement()["picking"], false);
    session
        .create_entity(
            Actor::Agent,
            "info_target",
            Some(DVec3::new(100.0, 100.0, 16.0)),
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(session.surface_placement()["valid"], false);
    assert!(
        session
            .apply_surface_placement(Actor::Agent, token)
            .is_err()
    );
    session.cancel_surface_placement(token).unwrap();
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn surface_placement_keeps_point_helpers_axis_aligned_on_slopes() {
    let (mut session, _) = fixture();
    let floor = ObjectId::Solid(session.document().vmf().world.solids[0].id);
    session
        .transform_objects(
            Actor::Agent,
            &[floor],
            Transform {
                rotation: DVec3::new(0.0, 30.0, 0.0),
                ..Transform::default()
            },
            Some(DVec3::ZERO),
        )
        .unwrap();
    let helper = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::new(0.0, -128.0, 64.0)),
            BTreeMap::new(),
        )
        .unwrap();
    let report = session
        .preview_surface_placement(
            Actor::Agent,
            &[helper],
            Method::Floor,
            None,
            Options {
                align: true,
                offset: 3.0,
                ..options()
            },
        )
        .unwrap();
    let plan = report["plan"].as_object().unwrap();
    let vector = |key| DVec3::from_array(serde_json::from_value(plan[key].clone()).unwrap());
    let normal = vector("normal");
    let point = vector("point");
    let (low, high) = query::bounds(session.render_document(), helper).unwrap();
    let mut minimum = f64::INFINITY;
    for x in [low.x, high.x] {
        for y in [low.y, high.y] {
            for z in [low.z, high.z] {
                minimum = minimum.min(normal.dot(DVec3::new(x, y, z) - point));
            }
        }
    }
    assert!((minimum - 3.0).abs() < 1e-7);
}
