use super::*;
use crate::session::{SelectMode, volume};

#[test]
fn model_scale_session_edits_undo_reload_choices_bounds_and_framing_share_source() {
    for actor in [Actor::User, Actor::Agent] {
        let Ok(game) = crate::startup::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        let path = "models/props_junk/wood_crate001a.mdl";
        let origin = DVec3::new(128.0, 64.0, 32.0);
        let ObjectId::Entity(entity) = session
            .create_entity(
                actor,
                "prop_dynamic",
                Some(origin),
                BTreeMap::from([
                    ("model".into(), path.into()),
                    ("rendercolor".into(), "64 128 255 032".into()),
                    ("rendermode".into(), "2".into()),
                    ("renderamt".into(), "128".into()),
                ]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let camera = session.camera();
        let selection = session.selected_ids();
        let unit = session.inspect_entity_model(entity).unwrap();
        assert_eq!(unit["model_scale"]["effective"], 1.0);
        assert!(unit["model_scale"]["authored"].is_null());
        session
            .set_entity_properties(actor, entity, &[("modelscale".into(), Some("2".into()))])
            .unwrap();
        let large = session.inspect_entity_model(entity).unwrap();
        assert_eq!(large["model_scale"]["effective"], 2.0);
        assert_eq!(large["model_scale"]["applied"], true);
        assert_eq!(large["tint"], unit["tint"]);
        assert_eq!(large["appearance"], unit["appearance"]);
        let bounds = |report: &Value, row: usize| {
            DVec3::from_array(std::array::from_fn(|axis| {
                report["world_bounds"][row][axis].as_f64().unwrap()
            }))
        };
        for row in 0..2 {
            assert!(
                bounds(&large, row).abs_diff_eq(origin + (bounds(&unit, row) - origin) * 2.0, 1e-8)
            );
        }
        let id = ObjectId::Entity(entity);
        assert_eq!(
            session.object_bounds_in(session.document(), &[id]),
            Some((bounds(&large, 0), bounds(&large, 1)))
        );
        assert_eq!(
            session.object_bounds_in_basis(session.document(), &[id], DMat3::IDENTITY),
            session.object_bounds_in(session.document(), &[id])
        );
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
        session.redo(actor).unwrap();
        assert_eq!(session.inspect_entity_model(entity).unwrap(), large);
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        session.refresh_model_assets(actor);
        assert_eq!(session.inspect_entity_model(entity).unwrap(), large);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        let token = session
            .preview_volume_selection(volume::Spec {
                mins: bounds(&large, 0) - DVec3::ONE,
                maxs: bounds(&large, 1) + DVec3::ONE,
                rule: map_core::region::Match::Contained,
                mode: SelectMode::Replace,
                faces: false,
            })
            .unwrap();
        assert_eq!(session.volume_preview().unwrap().matches.objects, [id]);
        session.cancel_volume_selection(token).unwrap();
        let expected_camera = camera.frame_bounds(
            bounds(&large, 0).as_vec3(),
            bounds(&large, 1).as_vec3(),
            1.6,
        );
        session.frame_selection(actor, 1.6).unwrap();
        assert_eq!(session.camera(), expected_camera);
        let choice = session.begin_model_choice(entity).unwrap()["token"]
            .as_u64()
            .unwrap();
        session.choose_model(choice, path, 0).unwrap();
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        session.apply_model_choice(actor, choice).unwrap();
        assert_eq!(
            session
                .document()
                .entity(entity)
                .unwrap()
                .properties
                .get("modelscale"),
            Some("2")
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
        let choice = session.begin_model_choice(entity).unwrap()["token"]
            .as_u64()
            .unwrap();
        session.choose_model(choice, path, 0).unwrap();
        session
            .set_entity_properties(actor, entity, &[("modelscale".into(), Some("0.5".into()))])
            .unwrap();
        assert!(session.apply_model_choice(actor, choice).is_err());
        session.cancel_model_choice(choice).unwrap();
        for raw in ["0", "NaN", "65"] {
            session
                .set_entity_properties(actor, entity, &[("modelscale".into(), Some(raw.into()))])
                .unwrap();
            let unsupported = session.inspect_entity_model(entity).unwrap();
            assert_eq!(unsupported["model_scale"]["supported"], false);
            assert_eq!(unsupported["model_scale"]["authored"], raw);
            assert_eq!(unsupported["displayed"], false);
            assert_eq!(unsupported["helper"], true);
            assert!(
                unsupported["error"]
                    .as_str()
                    .unwrap()
                    .contains("Model scale")
            );
        }
    }
}

#[test]
fn model_scale_surface_placement_and_grid_snap_use_scaled_support_points() {
    use crate::session::placement::{Method, Options};

    for actor in [Actor::User, Actor::Agent] {
        let Ok(game) = crate::startup::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        session
            .create_block(
                actor,
                DVec3::new(-256.0, -256.0, 256.0),
                DVec3::new(256.0, 256.0, 272.0),
                "dev/dev_measuregeneric01",
                false,
            )
            .unwrap();
        let id = session
            .create_entity(
                actor,
                "prop_dynamic",
                Some(DVec3::new(3.0, 5.0, 64.0)),
                BTreeMap::from([
                    (
                        "model".into(),
                        "models/props_junk/wood_crate001a.mdl".into(),
                    ),
                    ("modelscale".into(), "2".into()),
                ]),
            )
            .unwrap();
        let original = session.document().vmf().clone();
        let history = session.history_snapshot();
        let bounds = session.object_bounds_in(session.document(), &[id]).unwrap();
        let camera = session.camera();
        let report = session
            .preview_surface_placement(
                actor,
                &[id],
                Method::Ceiling,
                None,
                Options {
                    align: false,
                    offset: 0.0,
                    texture_lock: true,
                    source_corner: None,
                },
            )
            .unwrap();
        assert!(
            (report["plan"]["translation"][2].as_f64().unwrap() - (256.0 - bounds.1.z)).abs()
                < 1e-8
        );
        assert_eq!(report["plan"]["normal"], json!([0.0, 0.0, -1.0]));
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        session
            .apply_surface_placement(actor, report["token"].as_u64().unwrap())
            .unwrap();
        assert!(
            (session
                .object_bounds_in(session.document(), &[id])
                .unwrap()
                .1
                .z
                - 256.0)
                .abs()
                < 1e-8
        );
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        assert_eq!(
            session
                .document()
                .entity(entity)
                .unwrap()
                .properties
                .get("modelscale"),
            Some("2")
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
        let history = session.history_snapshot();
        let snap = session.preview_snap_to_grid(actor, &[id]).unwrap();
        assert_eq!(snap.bounds_minimum, bounds.0.to_array());
        assert_ne!(snap.bounds_minimum, [3.0 - 8.0, 5.0 - 8.0, 64.0 - 8.0]);
        assert!(session.refresh_model_assets(actor));
        assert!(session.transform_preview().is_none());
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
    }
}
