use super::*;
use map_core::{ObjectId, edit::Transform};

fn session() -> Option<(Session, u32)> {
    let game = crate::mount_game(None).ok()?;
    let mut session = Session::new(Ok(game));
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::User,
            "prop_static",
            Some(DVec3::ZERO),
            BTreeMap::from([("model".into(), "".into())]),
        )
        .unwrap()
    else {
        unreachable!()
    };
    Some((session, id))
}

#[test]
fn model_choices_catalog_preview_and_atomic_apply_preserve_the_active_scene() {
    for actor in [Actor::User, Actor::Agent] {
        let Some((mut session, id)) = session() else {
            return;
        };
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let camera = session.camera;
        let serial = session.scene_serial;
        let catalog = session
            .browse_models("wood crate", "folder:models/props_junk", 0, 1)
            .unwrap();
        assert!(catalog["total"].as_u64().unwrap() > 0);
        assert_eq!(catalog["entries"].as_array().unwrap().len(), 1);
        assert!(session.browse_models("", "wrong", 0, 60).is_err());
        assert!(session.browse_models("", "all", 0, 101).is_err());
        let token = session.begin_model_choice(id).unwrap()["token"]
            .as_u64()
            .unwrap();
        assert!(session.apply_model_choice(actor, token).is_err());
        let path = "models/props_junk/wood_crate001a.mdl";
        session.choose_model(token, path, 1).unwrap();
        let (scene, _, _) = session.model_preview_scene(path, 1, 1.6).unwrap();
        assert_eq!(scene.stats.model_entities, 1);
        assert_eq!(scene.stats.brushes, 0);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.camera, camera);
        assert_eq!(session.scene_serial, serial);
        session.apply_model_choice(actor, token).unwrap();
        assert_eq!(
            session.document.entity(id).unwrap().properties.get("model"),
            Some(path)
        );
        assert_eq!(
            session.document.entity(id).unwrap().properties.get("skin"),
            Some("1")
        );
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert_eq!(
            session.browse_models("wood crate", "used", 0, 60).unwrap()["total"],
            1
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &source);
        session.redo(actor).unwrap();
        assert_eq!(
            session.document.entity(id).unwrap().properties.get("skin"),
            Some("1")
        );
    }
}

#[test]
fn model_choice_tokens_stale_maps_and_geometry_previews_cannot_cross_ownership() {
    let Some((mut session, id)) = session() else {
        return;
    };
    let path = "models/props_junk/wood_crate001a.mdl";
    let first = session.begin_model_choice(id).unwrap()["token"]
        .as_u64()
        .unwrap();
    let second = session.begin_model_choice(id).unwrap()["token"]
        .as_u64()
        .unwrap();
    assert!(session.cancel_model_choice(first).is_err());
    assert!(session.choose_model(first, path, 0).is_err());
    session.choose_model(second, path, 1).unwrap();
    assert!(session.choose_model(second, path, 99).is_err());
    assert_eq!(session.model_choice(second).unwrap()["asset"]["skin"], 1);
    session
        .preview_transform(
            Actor::Agent,
            &[ObjectId::Entity(id)],
            Transform {
                translation: DVec3::X,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert!(session.begin_model_choice(id).is_err());
    assert!(session.apply_model_choice(Actor::User, second).is_err());
    session.cancel_model_choice(second).unwrap();
    assert!(!std::ptr::eq(session.render_document(), session.document()));
    session.cancel_transform_preview();
    let token = session.begin_model_choice(id).unwrap()["token"]
        .as_u64()
        .unwrap();
    session.choose_model(token, path, 0).unwrap();
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("targetname".into(), Some("changed".into()))],
        )
        .unwrap();
    assert!(session.apply_model_choice(Actor::User, token).is_err());
    session.cancel_model_choice(token).unwrap();
    assert_eq!(
        session.document.entity(id).unwrap().properties.get("model"),
        Some("")
    );
    let token = session.begin_model_choice(id).unwrap()["token"]
        .as_u64()
        .unwrap();
    session
        .set_entity_properties(Actor::User, id, &[("modelscale".into(), Some("2".into()))])
        .unwrap();
    session.cancel_model_choice(token).unwrap();
    let token = session.begin_model_choice(id).unwrap()["token"]
        .as_u64()
        .unwrap();
    assert!(
        session
            .choose_model(token, path, 0)
            .unwrap_err()
            .contains("Scaled")
    );
}
