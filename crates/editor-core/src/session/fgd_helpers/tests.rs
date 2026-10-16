use super::*;
use crate::session::{Actor, SelectMode, working_region};
use glam::{DMat3, DVec3};
use source_formats::fgd::Catalog;

const DEFINITION: &str = "@BaseClass size(-24 -12 0, 40 20 72) color(12 34 56) = shared [] @PointClass base(shared) sphere(radius) = sample [ angles(angle) : \"Angles\" : \"0 90 0\" radius(float) : \"Radius\" : 64 ]";

#[test]
fn fgd_size_drives_shared_scene_picks_bounds_regions_and_inspector_without_map_edits() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(actor, Catalog::parse(DEFINITION).unwrap());
        let id = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        session.select(&[], SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = (DVec3::new(-24.0, -12.0, 0.0), DVec3::new(40.0, 20.0, 72.0));
        assert_eq!(
            session.object_bounds_in(session.document(), &[id]),
            Some(bounds)
        );
        assert_eq!(
            session.object_bounds_in_basis(session.document(), &[id], DMat3::IDENTITY),
            Some(bounds)
        );
        let mesh = session.build_scene();
        assert_eq!(mesh.mins.as_dvec3(), bounds.0);
        assert_eq!(mesh.maxs.as_dvec3(), bounds.1);
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.color[..3] == [12, 34, 56])
        );
        let ray = Ray {
            origin: DVec3::new(30.0, -100.0, 64.0),
            direction: DVec3::Y,
        };
        assert_eq!(session.pick(&ray).unwrap().object, id);
        assert_eq!(
            session.pick(&ray).unwrap().point,
            DVec3::new(30.0, -12.0, 64.0)
        );
        assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 1);
        let inspector = session.entity_inspector(entity).unwrap();
        let report = inspector.helpers.unwrap();
        session.select(&[id], SelectMode::Replace).unwrap();
        let selected = session.build_scene();
        assert!(!selected.selection_outline_ranges.is_empty());
        assert_eq!(selected.vertices.len(), mesh.vertices.len());
        session.select(&[], SelectMode::Replace).unwrap();
        assert_eq!(report.bounds_source, "fgd_size");
        assert_eq!(report.unsupported, ["sphere"]);
        assert!(report.status.contains("world-aligned"));
        assert_eq!(
            session
                .select_region(
                    &map_core::region::Region::from_bounds(
                        DVec3::new(25.0, -20.0, 50.0),
                        DVec3::new(45.0, 25.0, 80.0)
                    )
                    .unwrap(),
                    map_core::region::Match::Intersecting,
                    false
                )
                .objects,
            [id]
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::new(25.0, -20.0, 50.0), DVec3::new(45.0, 25.0, 80.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(
            session
                .working_evaluation(session.document())
                .objects
                .contains(&id)
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::splat(100.0), DVec3::splat(200.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(session.pick(&ray).is_none());
        assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 0);
        assert_eq!(session.build_scene().stats.point_entities, 0);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn invalid_size_keeps_a_diagnostic_marker_visible_and_reload_updates_every_shared_bound() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(
            actor,
            Catalog::parse("@PointClass size(0 0 0) = sample []").unwrap(),
        );
        let id = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        session.select(&[], SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session.entity_helper_report(entity).unwrap();
        assert!(report.invalid_size);
        assert_eq!(report.bounds_source, "invalid_fgd_size");
        assert!(report.local_bounds.is_none());
        assert!(report.status.contains("generic editor marker"));
        assert!(!report.errors.is_empty());
        assert!(
            session
                .build_scene()
                .vertices
                .iter()
                .all(|vertex| vertex.color[..3] == [255, 176, 48])
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::splat(100.0), DVec3::splat(200.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(
            session
                .working_evaluation(session.document())
                .objects
                .contains(&id)
        );
        let records = session.get_working_region(0, 50).unwrap();
        assert_eq!(records["objects"][0]["bounds_source"], "invalid_fgd_size");
        assert!(!records["objects"][0]["bounds_error"].is_null());
        session.set_definitions(actor, Catalog::parse(DEFINITION).unwrap());
        assert!(
            !session
                .working_evaluation(session.document())
                .objects
                .contains(&id)
        );
        assert_eq!(
            session.entity_helper_report(entity).unwrap().bounds_source,
            "fgd_size"
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn fgd_boxes_do_not_replace_specialized_lights_or_loaded_models() {
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(2 2 2) color(1 2 3) = light []").unwrap(),
    );
    let id = session
        .create_entity(Actor::Agent, "light", Some(DVec3::ZERO), BTreeMap::new())
        .unwrap();
    let ObjectId::Entity(entity) = id else {
        unreachable!()
    };
    assert_eq!(
        session.entity_helper_report(entity).unwrap().bounds_source,
        "specialized_light"
    );
    assert_eq!(
        session.object_bounds_in(session.document(), &[id]),
        Some((DVec3::splat(-18.0), DVec3::splat(18.0)))
    );
    assert!(session.build_scene().vertices.len() > 24);
}

#[test]
fn loaded_model_geometry_takes_precedence_over_a_conflicting_fgd_box() {
    let Ok(game) = crate::startup::mount_game(None) else {
        return;
    };
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(2 2 2) = prop_static []").unwrap(),
    );
    let id = session
        .create_entity(
            Actor::Agent,
            "prop_static",
            Some(DVec3::ZERO),
            BTreeMap::from([(
                "model".into(),
                "models/props_junk/wood_crate001a.mdl".into(),
            )]),
        )
        .unwrap();
    let ObjectId::Entity(entity) = id else {
        unreachable!()
    };
    let report = session.entity_helper_report(entity).unwrap();
    assert_eq!(report.bounds_source, "model");
    assert_eq!(
        serde_json::to_value(report.world_bounds).unwrap(),
        session.inspect_entity_model(entity).unwrap()["world_bounds"]
    );
    assert!(session.build_scene().vertices.len() > 24);
    assert!(
        !session
            .working_evaluation(session.document())
            .helpers
            .contains_key(&id)
    );
}
