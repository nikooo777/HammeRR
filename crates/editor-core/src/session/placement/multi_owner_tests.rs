use super::*;

fn options(source: Component) -> Options {
    Options {
        source_corner: Some(source),
        align: false,
        offset: 0.0,
        texture_lock: true,
    }
}

#[test]
fn multi_owner_corner_snap_moves_every_root_and_hidden_child_with_one_offset() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, selected) = fixtures::fixture();
        let ids = fixtures::moving_roots(&mut session, selected);
        let choices = session.surface_placement_source_corners(&ids).unwrap();
        let source = choices.iter().find(|choice| matches!(choice.owner, ObjectId::Entity(id) if !session.document().entity(id).unwrap().solids.is_empty())).unwrap();
        let source_position = source.position;
        let options = options(source.component.clone());
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let ray = Ray {
            origin: DVec3::new(-200.0, -200.0, 100.0),
            direction: DVec3::NEG_Z,
        };
        let report = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options.clone())
            .unwrap();
        assert_eq!(
            report["ids"],
            json!(ids.iter().copied().map(object_id).collect::<Vec<_>>())
        );
        assert_eq!(
            report["plan"]["corner"]["position"],
            json!([-256.0, -256.0, 0.0])
        );
        assert_eq!(report["plan"]["rotation"], json!([0.0, 0.0, 0.0]));
        let delta = DVec3::new(-256.0, -256.0, 0.0) - source_position;
        for brush in original.solids() {
            let owner = session.document().owner_of_solid(brush.id).unwrap();
            let (_, after) = subobject::solid(session.render_document(), brush.id).unwrap();
            if !ids.contains(&owner) {
                assert_eq!(after, brush);
                continue;
            }
            assert_eq!(after.hidden, brush.hidden);
            let before = subobject::topology(brush).unwrap();
            let after = subobject::topology(after).unwrap();
            for vertex in before.vertices {
                assert!(
                    after.vertices[after.resolve(&vertex.sides).unwrap()[0]]
                        .position
                        .abs_diff_eq(vertex.position + delta, 1e-8)
                );
            }
        }
        for entity in original
            .entities
            .iter()
            .filter(|entity| ids.contains(&ObjectId::Entity(entity.id)))
        {
            if let Some(origin) = entity.origin() {
                let after = session.render_document().entity(entity.id).unwrap();
                assert!(after.origin().unwrap().abs_diff_eq(origin + delta, 1e-8));
                assert_eq!(
                    after.properties.get("angles"),
                    entity.properties.get("angles")
                );
            }
        }
        assert_eq!(session.document().vmf(), &original);
        session
            .cancel_surface_placement(report["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history);
        let report = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options.clone())
            .unwrap();
        session
            .apply_surface_placement(actor, report["token"].as_u64().unwrap())
            .unwrap();
        let applied = session.document().vmf().clone();
        let again = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options)
            .unwrap();
        assert_eq!(again["plan"]["translation"], json!([0.0, 0.0, 0.0]));
        session
            .apply_surface_placement(actor, again["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().vmf(), &applied);
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
    }
}

#[test]
fn multi_owner_corner_snap_rejects_invalid_companions_before_replacing_plan_or_pick() {
    let (mut session, selected) = fixtures::fixture();
    let ids = fixtures::moving_roots(&mut session, selected);
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    let source = session
        .surface_placement_source_corners(&ids)
        .unwrap()
        .into_iter()
        .find(|choice| choice.component.solid == solid)
        .unwrap();
    let options = options(source.component);
    let overflow = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::new(-65500.0, 0.0, 0.0)),
            BTreeMap::new(),
        )
        .unwrap();
    let malformed = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap();
    let ObjectId::Entity(malformed_id) = malformed else {
        unreachable!()
    };
    session
        .edit(Actor::User, "invalid imported origin", |tx| {
            tx.entity_mut(malformed_id)?
                .properties
                .set("origin", "invalid");
            Ok(())
        })
        .unwrap();
    let child = session
        .document()
        .vmf()
        .entities
        .iter()
        .find(|entity| ids.contains(&ObjectId::Entity(entity.id)) && !entity.solids.is_empty())
        .unwrap()
        .solids[0]
        .id;
    let ray = Ray {
        origin: DVec3::new(-200.0, -200.0, 100.0),
        direction: DVec3::NEG_Z,
    };
    for picking in [false, true] {
        let report = if picking {
            session
                .begin_surface_placement_pick(&ids, options.clone())
                .unwrap()
        } else {
            session
                .preview_surface_placement(
                    Actor::User,
                    &ids,
                    Method::BrushCorner,
                    Some(ray),
                    options.clone(),
                )
                .unwrap()
        };
        let document = session.document().vmf().clone();
        let displayed = session.render_document().vmf().clone();
        let selection = session.selected_ids();
        let history = session.document().undo_labels().count();
        for invalid in [
            ObjectId::Solid(child),
            malformed,
            overflow,
            ObjectId::Entity(u32::MAX),
        ] {
            let mut batch = ids.clone();
            batch.push(invalid);
            assert!(
                session
                    .preview_surface_placement(
                        Actor::Agent,
                        &batch,
                        Method::BrushCorner,
                        Some(ray),
                        options.clone()
                    )
                    .is_err()
            );
            assert_eq!(session.surface_placement(), report);
            assert_eq!(session.document().vmf(), &document);
            assert_eq!(session.render_document().vmf(), &displayed);
            assert_eq!(session.selected_ids(), selection);
            assert_eq!(session.document().undo_labels().count(), history);
        }
        for invalid in [
            ObjectId::Solid(child),
            malformed,
            ObjectId::Entity(u32::MAX),
        ] {
            let mut batch = ids.clone();
            batch.push(invalid);
            assert!(
                session
                    .begin_surface_placement_pick(&batch, options.clone())
                    .is_err()
            );
            assert_eq!(session.surface_placement(), report);
            assert_eq!(session.render_document().vmf(), &displayed);
        }
        session
            .cancel_surface_placement(report["token"].as_u64().unwrap())
            .unwrap();
    }
}

#[test]
fn multi_owner_corner_snap_rejects_ambiguous_companion_identities() {
    for duplicate_entity in [false, true] {
        let (mut session, selected) = fixtures::fixture();
        let ids = fixtures::moving_roots(&mut session, selected);
        let ObjectId::Solid(solid) = selected else {
            unreachable!()
        };
        let source = session
            .surface_placement_source_corners(&ids)
            .unwrap()
            .into_iter()
            .find(|choice| choice.component.solid == solid)
            .unwrap();
        let ambiguous = if duplicate_entity {
            *ids.iter()
                .find(|id| matches!(id, ObjectId::Entity(_)))
                .unwrap()
        } else {
            session
                .create_block(
                    Actor::User,
                    DVec3::new(500.0, 500.0, 48.0),
                    DVec3::new(516.0, 516.0, 64.0),
                    "brick/test",
                    false,
                )
                .unwrap()
        };
        let mut imported = session.document().vmf().clone();
        match ambiguous {
            ObjectId::Entity(id) => imported.entities.push(
                imported
                    .entities
                    .iter()
                    .find(|entity| entity.id == id)
                    .unwrap()
                    .clone(),
            ),
            ObjectId::Solid(id) => imported.world.solids.push(
                imported
                    .world
                    .solids
                    .iter()
                    .find(|brush| brush.id == id)
                    .unwrap()
                    .clone(),
            ),
        }
        session.document = map_core::Document::new(imported);
        let ray = Ray {
            origin: DVec3::new(-100.0, -100.0, 100.0),
            direction: DVec3::NEG_Z,
        };
        let options = options(source.component);
        let report = session
            .preview_surface_placement(
                Actor::User,
                &[selected],
                Method::BrushCorner,
                Some(ray),
                options.clone(),
            )
            .unwrap();
        let original = session.document().vmf().clone();
        let displayed = session.render_document().vmf().clone();
        assert!(
            session
                .preview_surface_placement(
                    Actor::Agent,
                    &[selected, ambiguous],
                    Method::BrushCorner,
                    Some(ray),
                    options
                )
                .is_err()
        );
        assert_eq!(session.surface_placement(), report);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.render_document().vmf(), &displayed);
    }
}
