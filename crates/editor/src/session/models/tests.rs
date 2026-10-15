use super::*;
use map_core::query::Ray;

#[test]
fn model_previews_share_real_assets_picks_bounds_cache_and_source_history() {
    for actor in [Actor::User, Actor::Agent] {
        let Ok(game) = crate::mount_game(None) else {
            eprintln!("CS:S not installed; skipping");
            return;
        };
        let mut session = Session::new(Ok(game));
        let ObjectId::Entity(id) = session
            .create_entity(
                actor,
                "prop_static",
                Some(DVec3::new(64.0, 32.0, 16.0)),
                BTreeMap::from([
                    (
                        "model".into(),
                        "models/props_junk/wood_crate001a.mdl".into(),
                    ),
                    ("angles".into(), "0 90 0".into()),
                ]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session.inspect_entity_model(id).unwrap();
        assert_eq!(report["displayed"], true);
        assert!(report["asset"]["vertices"].as_u64().unwrap() > 24);
        let mesh = session.build_scene();
        assert_eq!(mesh.stats.model_entities, 1);
        let models = session.model_instances(session.document());
        let instance = &models.instances[0];
        let asset = instance.asset.clone();
        let triangle = &asset.model.parts[0].indices[..3];
        let points = triangle
            .iter()
            .map(|&index| instance.point(asset.model.vertices[index as usize].position))
            .collect::<Vec<_>>();
        let center = (points[0] + points[1] + points[2]) / 3.0;
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        let ray = Ray {
            origin: center - normal * 16.0,
            direction: normal,
        };
        let bounds = instance.bounds();
        drop(models);
        let hit = session.pick(&ray).unwrap();
        assert_eq!(hit.object, ObjectId::Entity(id));
        assert!((hit.distance - 16.0).abs() < 1e-4);
        assert_eq!(session.selection_bounds().unwrap(), bounds);
        session.frame_selection(actor, 1.5).unwrap();
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert!(std::sync::Arc::ptr_eq(
            &asset,
            &session
                .models
                .borrow_mut()
                .get(&session.game, "MODELS/props_junk/wood_crate001a.mdl")
                .unwrap()
        ));
        session.refresh_model_assets(actor);
        assert!(!std::sync::Arc::ptr_eq(
            &asset,
            &session
                .models
                .borrow_mut()
                .get(&session.game, "models/props_junk/wood_crate001a.mdl")
                .unwrap()
        ));
        assert_eq!(session.document().vmf(), &source);
        session
            .set_entity_properties(actor, id, &[("skin".into(), Some("99999".into()))])
            .unwrap();
        assert_eq!(session.build_scene().stats.model_entities, 0);
        assert!(session.inspect_entity_model(id).is_err());
        session.undo(actor).unwrap();
        assert_eq!(session.build_scene().stats.model_entities, 1);
        session
            .edit(actor, "hide model", |tx| {
                tx.entity_mut(id)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(session.build_scene().stats.model_entities, 0);
        assert!(session.pick(&ray).is_none());
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
}
