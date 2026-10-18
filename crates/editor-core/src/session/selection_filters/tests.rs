use super::*;
use crate::session::{Pointer, SelectMode, paint, volume};
use glam::{DVec2, DVec3, Vec3};
use map_core::{
    edit, groups,
    query::Ray,
    region::{Match, Region},
};
use render::{Camera, Projection};

fn fixture() -> (Session, [ObjectId; 3]) {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for z in [64.0, 32.0] {
        ids.push(
            session
                .create_block(
                    Actor::User,
                    DVec3::new(-16.0, -16.0, z),
                    DVec3::new(16.0, 16.0, z + 16.0),
                    "test",
                    false,
                )
                .unwrap(),
        );
    }
    let ObjectId::Solid(solid) = ids[1] else {
        unreachable!()
    };
    ids[1] = session
        .edit(Actor::User, "brush owner", |tx| {
            edit::tie_to_entity(tx, &[solid], "func_detail", &[])
        })
        .unwrap();
    ids.push(
        session
            .edit(Actor::User, "point helper", |tx| {
                edit::create_entity(tx, "info_target", Some(DVec3::ZERO), &[], Vec::new())
            })
            .unwrap(),
    );
    session.select(&[], SelectMode::Replace).unwrap();
    session.set_camera(Camera::planar(Projection::Top, Vec3::ZERO, 256.0));
    (session, ids.try_into().unwrap())
}

fn only(index: usize) -> SelectionFilters {
    SelectionFilters {
        world_brushes: index == 0,
        brush_entities: index == 1,
        point_entities: index == 2,
    }
}

fn ray() -> Ray {
    Ray {
        origin: DVec3::Z * 128.0,
        direction: DVec3::NEG_Z,
    }
}

fn paint_spec(session: &Session, faces: bool) -> paint::Spec {
    paint::Spec {
        camera: session.camera(),
        pointer: Pointer {
            position: DVec2::new(320.0, 240.0),
            size: (640, 480),
        },
        radius: 2.0,
        mode: SelectMode::Replace,
        faces,
    }
}

#[test]
fn type_filters_choose_eligible_depth_hits_without_changing_raw_queries_or_source() {
    let (mut session, ids) = fixture();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    assert_eq!(session.pick_for_selection(&ray()), session.pick(&ray()));
    for (index, id) in ids.into_iter().enumerate() {
        let serials = (
            session.scene_serial(),
            session.view_serial(),
            session.interaction_epoch(),
        );
        session
            .set_selection_filters(Actor::Agent, only(index))
            .unwrap();
        assert_eq!(
            (
                session.scene_serial(),
                session.view_serial(),
                session.interaction_epoch()
            ),
            serials
        );
        assert_eq!(session.pick_for_selection(&ray()).unwrap().object, id);
        assert_eq!(session.pick(&ray()).unwrap().object, ids[0]);
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(report["total"], 1);
        assert_eq!(
            report["candidates"][0]["object"],
            crate::identity::id_string(id)
        );
        session.cycle_object_hit(Actor::User, &ray()).unwrap();
        assert_eq!(session.selected_ids(), [id]);
    }
    let selected = session.selected_ids();
    session
        .set_selection_filters(
            Actor::User,
            SelectionFilters {
                world_brushes: false,
                brush_entities: false,
                point_entities: false,
            },
        )
        .unwrap();
    assert!(session.pick_for_selection(&ray()).is_none());
    assert_eq!(session.selected_ids(), selected);
    session.select_grouped(&ids, SelectMode::Replace).unwrap();
    assert_eq!(session.selected_ids().len(), 3);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    let flags = session.selection_filters();
    let revision = session.selection_filter_revision();
    session.set_selection_filters(Actor::Agent, flags).unwrap();
    assert_eq!(session.selection_filter_revision(), revision);
    session.new_map(Actor::User, true).unwrap();
    assert_eq!(session.selection_filters(), flags);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("map.vmf");
    std::fs::write(&path, source.write(&Default::default())).unwrap();
    session.open(Actor::Agent, &path, true).unwrap();
    assert_eq!(session.selection_filters(), flags);
    assert_eq!(
        Session::new(Err("peer".into())).selection_filters(),
        SelectionFilters::default()
    );
}

#[test]
fn enabled_hits_seed_complete_groups_and_containment_still_checks_disabled_peers() {
    let (mut session, ids) = fixture();
    let group = session
        .edit(Actor::User, "mixed group", |tx| {
            groups::apply(
                tx,
                &groups::Edit {
                    group: None,
                    name: "Mixed".into(),
                    parent: None,
                    members: vec![
                        groups::Member::Object(ids[0]),
                        groups::Member::Object(ids[2]),
                    ],
                    ungroup: false,
                },
            )
        })
        .unwrap();
    session
        .set_selection_filters(Actor::Agent, only(2))
        .unwrap();
    let mut unit = vec![ids[0], ids[2]];
    unit.sort_unstable();
    let report = session.object_hit_candidates(&ray()).unwrap();
    assert_eq!(report["total"], 1);
    assert_eq!(
        report["candidates"][0]["object"],
        crate::identity::id_string(ids[2])
    );
    session.cycle_object_hit(Actor::User, &ray()).unwrap();
    assert_eq!(session.selected_ids(), unit);
    let full = Region::from_bounds(DVec3::splat(-24.0), DVec3::new(24.0, 24.0, 88.0)).unwrap();
    assert_eq!(
        session
            .select_region(&full, Match::Contained, false)
            .objects,
        unit
    );
    let point = Region::from_bounds(DVec3::splat(-12.0), DVec3::splat(12.0)).unwrap();
    assert!(
        session
            .select_region(&point, Match::Contained, false)
            .objects
            .is_empty()
    );
    assert_eq!(
        session
            .select_region(&point, Match::Intersecting, false)
            .objects,
        unit
    );
    let world =
        Region::from_bounds(DVec3::new(-24.0, -24.0, 60.0), DVec3::new(24.0, 24.0, 88.0)).unwrap();
    assert!(
        session
            .select_region(&world, Match::Intersecting, false)
            .objects
            .is_empty()
    );
    session.set_group_selection_bypass(Actor::User, true);
    assert_eq!(
        session
            .select_region(&full, Match::Contained, false)
            .objects,
        [ids[2]]
    );
    session
        .select_object_group(group, SelectMode::Replace, false)
        .unwrap();
    assert_eq!(session.selected_ids(), unit);
}

#[test]
fn object_marquee_volume_and_cached_paint_share_filters_and_retire_old_tokens() {
    let (mut session, ids) = fixture();
    session.set_selection_filters(Actor::User, only(2)).unwrap();
    let camera = session.camera();
    let start = Pointer {
        position: DVec2::new(200.0, 120.0),
        size: (640, 480),
    };
    let matches = session
        .preview_area_selection(
            Actor::Agent,
            camera,
            start,
            DVec2::new(440.0, 360.0),
            Match::Contained,
            SelectMode::Replace,
        )
        .unwrap();
    assert_eq!(matches.objects, [ids[2]]);
    let filters = session.selection_filters();
    let serials = (session.scene_serial(), session.view_serial());
    session.set_selection_filters(Actor::User, filters).unwrap();
    assert!(session.area_preview().is_some());
    assert_eq!((session.scene_serial(), session.view_serial()), serials);
    session.set_selection_filters(Actor::User, only(1)).unwrap();
    assert!(session.commit_area_selection().is_err());
    let token = session
        .preview_volume_selection(volume::Spec {
            mins: DVec3::splat(-24.0),
            maxs: DVec3::new(24.0, 24.0, 88.0),
            rule: Match::Contained,
            mode: SelectMode::Replace,
            faces: false,
        })
        .unwrap();
    assert_eq!(session.volume_preview().unwrap().matches.objects, [ids[1]]);
    session.set_selection_filters(Actor::User, only(2)).unwrap();
    assert!(
        session
            .commit_volume_selection(Actor::Agent, token)
            .is_err()
    );
    let token = session
        .begin_paint_selection(paint_spec(&session, false))
        .unwrap();
    assert_eq!(
        session.paint_stroke().unwrap().objects,
        [ids[2]].into_iter().collect()
    );
    session
        .set_selection_filters(Actor::Agent, only(1))
        .unwrap();
    assert!(
        session
            .update_paint_selection(token, &[DVec2::new(322.0, 240.0)])
            .is_err()
    );
    assert!(session.commit_paint_selection(Actor::User, token).is_err());
    let token = session
        .begin_paint_selection(paint_spec(&session, false))
        .unwrap();
    assert_eq!(
        session.paint_stroke().unwrap().objects,
        [ids[1]].into_iter().collect()
    );
    session.commit_paint_selection(Actor::User, token).unwrap();
    assert_eq!(session.selected_ids(), [ids[1]]);
}

#[test]
fn filter_changes_preserve_face_previews_and_pending_geometry_without_global_invalidation() {
    let (mut session, ids) = fixture();
    let spec = volume::Spec {
        mins: DVec3::splat(-24.0),
        maxs: DVec3::new(24.0, 24.0, 88.0),
        rule: Match::Intersecting,
        mode: SelectMode::Replace,
        faces: true,
    };
    let token = session.preview_volume_selection(spec).unwrap();
    let matches = session.volume_preview().unwrap().matches.clone();
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.interaction_epoch(),
    );
    session
        .set_selection_filters(Actor::Agent, only(2))
        .unwrap();
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.interaction_epoch()
        ),
        serials
    );
    assert_eq!(
        session.commit_volume_selection(Actor::User, token).unwrap(),
        matches
    );
    session.set_face_mode(true);
    assert_eq!(session.pick_for_selection(&ray()), session.pick(&ray()));
    let token = session
        .begin_paint_selection(paint_spec(&session, true))
        .unwrap();
    let faces = session.paint_stroke().unwrap().faces.clone();
    session
        .set_selection_filters(Actor::Agent, only(1))
        .unwrap();
    assert_eq!(session.paint_stroke().unwrap().token, token);
    assert_eq!(session.paint_stroke().unwrap().faces, faces);
    session.commit_paint_selection(Actor::User, token).unwrap();
    session.select(&[ids[0]], SelectMode::Replace).unwrap();
    session
        .preview_transform(
            Actor::User,
            &[ids[0]],
            edit::Transform {
                translation: DVec3::X,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let preview = session.render_document().vmf().clone();
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.interaction_epoch(),
    );
    session
        .set_selection_filters(Actor::Agent, only(0))
        .unwrap();
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.interaction_epoch()
        ),
        serials
    );
    session.commit_transform_preview(Actor::User).unwrap();
}

#[test]
fn decoded_models_and_fgd_helpers_follow_the_point_entity_seed_filter() {
    let (_directory, game) = crate::test_assets::model_lighting::game();
    let mut session = Session::new(Ok(game));
    let model = session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::ZERO),
            [("model".into(), "models/test/receiver.mdl".into())]
                .into_iter()
                .collect(),
        )
        .unwrap();
    let foreground = session
        .create_block(
            Actor::User,
            DVec3::new(-16.0, -16.0, 32.0),
            DVec3::new(16.0, 16.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    session.select(&[], SelectMode::Replace).unwrap();
    assert_eq!(
        session.pick_for_selection(&ray()).unwrap().object,
        foreground
    );
    session
        .set_selection_filters(Actor::Agent, only(2))
        .unwrap();
    assert_eq!(session.pick_for_selection(&ray()).unwrap().object, model);
    assert_eq!(session.object_hit_candidates(&ray()).unwrap()["total"], 1);
    session.set_selection_filters(Actor::User, only(0)).unwrap();
    assert_eq!(
        session.pick_for_selection(&ray()).unwrap().object,
        foreground
    );
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(
            "@PointClass size(-40 -24 -16, 40 24 16) = test_helper []",
        )
        .unwrap(),
    );
    let helper = session
        .edit(Actor::User, "FGD helper", |tx| {
            edit::create_entity(
                tx,
                "test_helper",
                Some(DVec3::new(128.0, 0.0, 0.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    let helper_ray = Ray {
        origin: DVec3::new(160.0, 0.0, 128.0),
        ..ray()
    };
    assert_eq!(session.pick(&helper_ray).unwrap().object, helper);
    assert!(session.pick_for_selection(&helper_ray).is_none());
    session
        .set_selection_filters(Actor::Agent, only(2))
        .unwrap();
    assert_eq!(
        session.pick_for_selection(&helper_ray).unwrap().object,
        helper
    );
}

#[test]
fn exhausted_filter_revision_rejects_before_retiring_an_owned_selection_draft() {
    let (mut session, _) = fixture();
    let token = session
        .begin_paint_selection(paint_spec(&session, false))
        .unwrap();
    session.selection_filters.revision = u64::MAX;
    let state = session.paint_selection_json(0, 100);
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.log_serial(),
    );
    assert!(session.set_selection_filters(Actor::User, only(2)).is_err());
    assert_eq!(session.paint_selection_json(0, 100), state);
    assert_eq!(session.paint_stroke().unwrap().token, token);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.log_serial()
        ),
        serials
    );
}
