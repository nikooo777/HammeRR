use super::*;

fn options(session: &Session, selected: ObjectId) -> Options {
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    let topology = subobject::topology(session.document().solid(solid).unwrap()).unwrap();
    Options {
        align: false,
        offset: 0.0,
        texture_lock: true,
        source_corner: Some(Component {
            solid,
            sides: topology.vertices[0].sides.clone(),
        }),
    }
}

#[test]
fn model_corner_snap_uses_cached_scaled_mesh_and_keeps_source_and_history_atomic() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, selected, model, ray) = fixtures::model_target();
        let mut ids = fixtures::moving_roots(&mut session, selected);
        let moving_model = session
            .create_entity(
                actor,
                "prop_dynamic",
                Some(DVec3::new(100.0, 200.0, 80.0)),
                BTreeMap::from([
                    ("model".into(), "models/test/receiver.mdl".into()),
                    ("angles".into(), "0 90 0".into()),
                    ("modelscale".into(), "2".into()),
                ]),
            )
            .unwrap();
        ids.push(moving_model);
        ids.sort();
        let options = options(&session, selected);
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options.clone())
            .unwrap();
        let token = report["token"].as_u64().unwrap();
        let target = &report["plan"]["corner"]["target"];
        assert_eq!(target["kind"], "model_vertex");
        assert_eq!(target["entity"], object_id(model));
        assert_eq!(target["model"], "models/test/receiver.mdl");
        assert_eq!(target["checksum"], 7);
        assert_eq!(target["body"], 0);
        assert_eq!(target["skin"], 1);
        assert_eq!(target["scale"], 2.0);
        assert_eq!(target["origin"], json!([100.0, 200.0, 40.0]));
        assert_eq!(target["provenance"], "cached_decoded_model_mesh");
        assert_eq!(
            (
                target["part"].as_u64(),
                target["triangle"].as_u64(),
                target["vertex"].as_u64()
            ),
            (Some(0), Some(0), Some(0))
        );
        assert!(target["solid"].is_null() && target["side"].is_null() && target["sides"].is_null());
        let position = DVec3::from_array(
            serde_json::from_value(report["plan"]["corner"]["position"].clone()).unwrap(),
        );
        assert!(position.abs_diff_eq(DVec3::new(148.0, 152.0, 40.0), 1e-10));
        let ObjectId::Solid(solid) = selected else {
            unreachable!()
        };
        let shape = subobject::topology(session.render_document().solid(solid).unwrap()).unwrap();
        let component = options.source_corner.as_ref().unwrap();
        assert!(
            shape.vertices[shape.resolve(&component.sides).unwrap()[0]]
                .position
                .abs_diff_eq(position, 1e-8)
        );
        let ObjectId::Entity(moving_entity) = moving_model else {
            unreachable!()
        };
        let delta = position
            - DVec3::from_array(
                serde_json::from_value::<[f64; 3]>(report["plan"]["corner"]["source"].clone())
                    .unwrap(),
            );
        assert!(
            session
                .render_document()
                .entity(moving_entity)
                .unwrap()
                .origin()
                .unwrap()
                .abs_diff_eq(DVec3::new(100.0, 200.0, 80.0) + delta, 1e-8)
        );
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.document().undo_labels().count(), history);
        assert!(
            session
                .preview_surface_placement(
                    actor,
                    &ids,
                    Method::BrushCorner,
                    Some(Ray {
                        origin: ray.origin,
                        direction: DVec3::ZERO
                    }),
                    options.clone()
                )
                .is_err()
        );
        assert_eq!(session.surface_placement(), report);
        session.cancel_surface_placement(token).unwrap();
        assert_eq!(session.document().vmf(), &original);
        let report = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options.clone())
            .unwrap();
        session
            .apply_surface_placement(actor, report["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        let ObjectId::Entity(entity) = model else {
            unreachable!()
        };
        assert_eq!(
            session.document().entity(entity),
            original.entities.iter().find(|source| source.id == entity)
        );
        let again = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options)
            .unwrap();
        assert_eq!(again["plan"]["translation"], json!([0.0, 0.0, 0.0]));
        session
            .apply_surface_placement(actor, again["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
    }
}

#[test]
fn model_corner_snap_shares_first_hit_visibility_and_reload_guards() {
    let (directory, mut session, selected, model, ray) = fixtures::model_target();
    let options = options(&session, selected);
    let blocker = session
        .create_block(
            Actor::User,
            DVec3::new(136.0, 144.0, 80.0),
            DVec3::new(160.0, 168.0, 88.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .select(&[selected], crate::session::SelectMode::Replace)
        .unwrap();
    let brush = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(ray),
            options.clone(),
        )
        .unwrap();
    assert_eq!(brush["plan"]["target"], object_id(blocker));
    assert_eq!(brush["plan"]["corner"]["target"]["kind"], "brush_corner");
    session
        .cancel_surface_placement(brush["token"].as_u64().unwrap())
        .unwrap();
    let ObjectId::Solid(blocker) = blocker else {
        unreachable!()
    };
    session
        .edit(Actor::User, "hide foreground brush", |tx| {
            tx.solid_mut(blocker)?.hidden = true;
            Ok(())
        })
        .unwrap();
    let report = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(ray),
            options.clone(),
        )
        .unwrap();
    assert_eq!(report["plan"]["target"], object_id(model));
    let token = report["token"].as_u64().unwrap();
    let mesh_path = directory.path().join("models/test/receiver.vvd");
    let mut bytes = std::fs::read(&mesh_path).unwrap();
    bytes[80..84].copy_from_slice(&(-23f32).to_le_bytes());
    std::fs::write(mesh_path, bytes).unwrap();
    assert_eq!(session.surface_placement(), report);
    session.refresh_model_assets(Actor::Agent);
    assert!(session.apply_surface_placement(Actor::User, token).is_err());
    session.cancel_surface_placement(token).unwrap();
    let refreshed = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(ray),
            options.clone(),
        )
        .unwrap();
    assert_eq!(
        refreshed["plan"]["corner"]["target"]["kind"],
        "model_vertex"
    );
    assert_ne!(
        refreshed["plan"]["corner"]["position"],
        report["plan"]["corner"]["position"]
    );
    let ObjectId::Entity(entity) = model else {
        unreachable!()
    };
    let token = refreshed["token"].as_u64().unwrap();
    session
        .set_entity_properties(
            Actor::Agent,
            entity,
            &[("origin".into(), Some("100 200 41".into()))],
        )
        .unwrap();
    assert!(session.apply_surface_placement(Actor::User, token).is_err());
    session.cancel_surface_placement(token).unwrap();
    session
        .edit(Actor::Agent, "hide model target", |tx| {
            tx.entity_mut(entity)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert!(
        session
            .preview_surface_placement(
                Actor::User,
                &[selected],
                Method::BrushCorner,
                Some(ray),
                options.clone()
            )
            .is_err()
    );
    let mut duplicate = session.document().vmf().clone();
    duplicate
        .entities
        .iter_mut()
        .find(|target| target.id == entity)
        .unwrap()
        .hidden = false;
    duplicate.entities.push(
        duplicate
            .entities
            .iter()
            .find(|target| target.id == entity)
            .unwrap()
            .clone(),
    );
    session.replace_document(map_core::Document::new(duplicate), None);
    session
        .select(&[selected], crate::session::SelectMode::Replace)
        .unwrap();
    let error = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(ray),
            options,
        )
        .unwrap_err();
    assert!(error.contains("ambiguous entity identity"), "{error}");
}
