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

#[test]
fn model_bounds_drive_region_reports_local_handles_and_helper_fallbacks() {
    use crate::session::{ObjectFilter, Pointer, SelectMode, volume};
    use glam::{DMat3, DVec2, Vec3};
    use map_core::region::Match;
    for actor in [Actor::User, Actor::Agent] {
        let Ok(game) = crate::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        let id = session
            .create_entity(
                actor,
                "prop_static",
                Some(DVec3::ZERO),
                BTreeMap::from([
                    (
                        "model".into(),
                        "models/props_junk/wood_crate001a.mdl".into(),
                    ),
                    ("angles".into(), "0 30 0".into()),
                ]),
            )
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let small = volume::Spec {
            mins: DVec3::splat(-9.0),
            maxs: DVec3::splat(9.0),
            rule: Match::Contained,
            mode: SelectMode::Replace,
            faces: false,
        };
        let token = session.preview_volume_selection(small).unwrap();
        assert!(session.volume_preview().unwrap().matches.objects.is_empty());
        session.cancel_volume_selection(token).unwrap();
        let remote = volume::Spec {
            mins: DVec3::new(20.0, -2.0, -2.0),
            maxs: DVec3::new(24.0, 2.0, 2.0),
            rule: Match::Intersecting,
            ..small
        };
        let token = session.preview_volume_selection(remote).unwrap();
        assert_eq!(session.volume_preview().unwrap().matches.objects, [id]);
        session.cancel_volume_selection(token).unwrap();
        let camera = render::Camera::planar(render::Projection::Top, Vec3::ZERO, 128.0);
        let small_area = session
            .preview_area_selection(
                actor,
                camera,
                Pointer {
                    position: DVec2::new(220.0, 220.0),
                    size: (512, 512),
                },
                DVec2::new(292.0, 292.0),
                Match::Contained,
                SelectMode::Replace,
            )
            .unwrap();
        assert!(small_area.objects.is_empty());
        let remote_area = session
            .preview_area_selection(
                actor,
                camera,
                Pointer {
                    position: DVec2::new(336.0, 248.0),
                    size: (512, 512),
                },
                DVec2::new(352.0, 264.0),
                Match::Intersecting,
                SelectMode::Replace,
            )
            .unwrap();
        assert_eq!(remote_area.objects, [id]);
        session.cancel_area_selection();
        let bounds = session.object_bounds_in(session.document(), &[id]).unwrap();
        assert_eq!(
            session.object_report(&ObjectFilter::default()).unwrap()[0].bounds,
            Some([bounds.0.to_array(), bounds.1.to_array()])
        );
        assert!(
            !session
                .object_report(&ObjectFilter {
                    within: Some([remote.mins.to_array(), remote.maxs.to_array()]),
                    ..Default::default()
                })
                .unwrap()
                .is_empty()
        );
        let basis = session.transform_basis(&[id], true).unwrap();
        let instance = session
            .model_instances(session.document())
            .instances
            .pop()
            .unwrap();
        assert_eq!(
            session
                .object_bounds_in_basis(session.document(), &[id], basis)
                .unwrap(),
            instance.bounds_in_basis(basis)
        );
        assert_eq!(
            session
                .object_bounds_in_basis(session.document(), &[id], DMat3::IDENTITY)
                .unwrap(),
            bounds
        );
        session.set_face_mode(true);
        let token = session
            .preview_volume_selection(volume::Spec {
                faces: true,
                ..remote
            })
            .unwrap();
        assert!(session.volume_preview().unwrap().matches.faces.is_empty());
        session.cancel_volume_selection(token).unwrap();
        session.set_face_mode(false);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        session
            .edit(actor, "hide model", |tx| {
                tx.entity_mut(entity)?.hidden = true;
                Ok(())
            })
            .unwrap();
        let token = session.preview_volume_selection(remote).unwrap();
        assert!(session.volume_preview().unwrap().matches.objects.is_empty());
        session.cancel_volume_selection(token).unwrap();
        session.undo(actor).unwrap();
        session
            .set_entity_properties(actor, entity, &[("modelscale".into(), Some("2".into()))])
            .unwrap();
        let token = session.preview_volume_selection(small).unwrap();
        assert_eq!(session.volume_preview().unwrap().matches.objects, [id]);
        session.cancel_volume_selection(token).unwrap();
        session.undo(actor).unwrap();
        session
            .set_entity_properties(
                actor,
                entity,
                &[("angles".into(), None), ("angle".into(), Some("-1".into()))],
            )
            .unwrap();
        let expected =
            DMat3::from_quat(session.model_instances(session.document()).instances[0].rotation);
        assert_eq!(session.transform_basis(&[id], true).unwrap(), expected);
    }
}

#[test]
fn reference_model_previews_share_pose_class_eligibility_and_owned_choices() {
    for actor in [Actor::User, Actor::Agent] {
        let Ok(game) = crate::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        let path = "models/player/t_phoenix.mdl";
        let ObjectId::Entity(incompatible) = session
            .create_entity(
                actor,
                "prop_static",
                Some(DVec3::ZERO),
                BTreeMap::from([("model".into(), path.into())]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let bad = session.inspect_entity_model(incompatible).unwrap();
        assert_eq!(bad["displayed"], false);
        assert!(bad["error"].as_str().unwrap().contains("prop_static"));
        let ObjectId::Entity(id) = session
            .create_entity(
                actor,
                "prop_dynamic",
                Some(DVec3::new(64.0, 0.0, 0.0)),
                BTreeMap::from([("model".into(), path.into())]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let report = session.inspect_entity_model(id).unwrap();
        assert_eq!(report["asset"]["pose"], "reference");
        assert_eq!(report["asset"]["reference_bones"], 50);
        assert_eq!(report["asset"]["static_prop"], false);
        assert_eq!(report["displayed"], true);
        assert_eq!(session.build_scene().stats.model_entities, 1);
        let instance = session
            .model_instances(session.document())
            .instances
            .pop()
            .unwrap();
        let triangle = instance.asset.model.parts[0].indices[..3]
            .iter()
            .map(|&index| instance.point(instance.asset.model.vertices[index as usize].position))
            .collect::<Vec<_>>();
        let center = (triangle[0] + triangle[1] + triangle[2]) / 3.0;
        let normal = (triangle[1] - triangle[0])
            .cross(triangle[2] - triangle[0])
            .normalize();
        assert_eq!(
            session
                .pick(&Ray {
                    origin: center - normal * 16.0,
                    direction: normal
                })
                .unwrap()
                .object,
            ObjectId::Entity(id)
        );
        let token = session.begin_model_choice(id).unwrap()["token"]
            .as_u64()
            .unwrap();
        session
            .choose_model(token, "models/player/ct_urban.mdl", 0)
            .unwrap();
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        session.apply_model_choice(actor, token).unwrap();
        assert_eq!(
            session.inspect_entity_model(id).unwrap()["asset"]["path"],
            "models/player/ct_urban.mdl"
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }
}
