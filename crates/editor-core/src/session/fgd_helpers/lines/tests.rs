use super::*;
use crate::session::{Actor, SelectMode, working_region};
use glam::DVec3;
use map_core::{edit::Transform, query::Ray};
use source_formats::fgd::Catalog;

const FGD: &str = "@PointClass size(8 8 8) line(12 34 56,targetname,aim) line(65 43 21,targetname,start,targetname,aim) = sample [ aim(string) : \"Target\" start(string) : \"Start\" ] @PointClass = target [] @PointClass line(12 34 56,targetname,aim) = light [ aim(string) : \"Target\" ] @PointClass studio(missing.mdl) line(12 34 56,targetname,aim) = model [ aim(string) : \"Target\" ]";

const NODE_FGD: &str = "@PointClass size(8 8 8) line(12 34 56,nodeid,start,nodeid,finish) = link [ start(node_dest) : \"Start\" : 0 finish(node_dest) : \"End\" : 42 ] @PointClass size(8 8 8) studio(missing.mdl) = node [ nodeid(integer) readonly : \"Node ID\" ]";

fn fixture(actor: Actor) -> (Session, u32, u32, u32) {
    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    let mut create = |class, origin, values| {
        let ObjectId::Entity(id) = session
            .create_entity(actor, class, Some(origin), values)
            .unwrap()
        else {
            panic!()
        };
        id
    };
    let target = create(
        "target",
        DVec3::new(120.0, 40.0, 80.0),
        BTreeMap::from([("targetname".into(), "End".into())]),
    );
    let start = create(
        "target",
        DVec3::new(-120.0, 40.0, 80.0),
        BTreeMap::from([("targetname".into(), "Start".into())]),
    );
    let source = create(
        "sample",
        DVec3::ZERO,
        BTreeMap::from([
            ("aim".into(), "End".into()),
            ("start".into(), "Start".into()),
        ]),
    );
    (session, source, target, start)
}

fn vertices(mesh: &render::SceneMesh) -> Vec<render::scene::LineVertex> {
    mesh.lines
        .iter()
        .filter(|line| [[12, 34, 56, 255], [65, 43, 21, 255]].contains(&line.color))
        .copied()
        .collect()
}

#[test]
fn line_geometry_color_preview_and_inspector_share_one_read_only_path() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, source, target, _) = fixture(actor);
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = session.object_bounds_in(session.document(), &[ObjectId::Entity(source)]);
        let mesh = session.build_scene();
        let lines = vertices(&mesh);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].position, [0.0; 3]);
        assert_eq!(lines[1].position, [120.0, 40.0, 80.0]);
        assert_eq!(lines[2].position, [-120.0, 40.0, 80.0]);
        assert_eq!(lines[3].position, lines[1].position);
        let report = session.entity_inspector(source).unwrap().helpers.unwrap();
        assert!(report.lines.iter().all(|line| line.displayed));
        assert!(report.status.contains("2 displayed"));
        assert!(report.unsupported.is_empty());
        assert!(
            session
                .pick(&Ray {
                    origin: DVec3::new(60.0, -100.0, 40.0),
                    direction: DVec3::Y
                })
                .is_none()
        );
        session
            .preview_transform(
                actor,
                &[ObjectId::Entity(source), ObjectId::Entity(target)],
                Transform {
                    translation: DVec3::new(16.0, 24.0, 32.0),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let preview = session.entity_helper_report(source).unwrap();
        assert!(preview.lines.iter().all(|line| line.displayed));
        assert_eq!(
            preview.lines[0].endpoints[0].position,
            Some([16.0, 24.0, 32.0])
        );
        assert_eq!(
            preview.lines[0].endpoints[1].position,
            Some([136.0, 64.0, 112.0])
        );
        assert_eq!(
            preview.lines[1].endpoints[0].position,
            Some([-120.0, 40.0, 80.0])
        );
        assert_eq!(
            vertices(&session.build_scene())
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>(),
            [
                [16.0, 24.0, 32.0],
                [136.0, 64.0, 112.0],
                [-120.0, 40.0, 80.0],
                [136.0, 64.0, 112.0]
            ]
        );
        session.cancel_transform_preview();
        session.select(&[], SelectMode::Replace).unwrap();
        let plain = session.build_scene();
        assert!(vertices(&plain).is_empty());
        assert_eq!((mesh.mins, mesh.maxs), (plain.mins, plain.maxs));
        assert_eq!(
            mesh.vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>(),
            plain
                .vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>()
        );
        assert_eq!(mesh.indices, plain.indices);
        assert_eq!(
            session.entity_helper_report(source).unwrap().lines[0].suppression_reason,
            Some("not_selected")
        );
        assert_eq!(
            session.object_bounds_in(session.document(), &[ObjectId::Entity(source)]),
            bounds
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn hidden_ambiguous_filtered_and_specialized_cases_explain_suppression() {
    let (mut session, source, target, _) = fixture(Actor::Agent);
    session
        .edit(Actor::Agent, "hide target", |tx| {
            tx.entity_mut(target)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        session.entity_helper_report(source).unwrap().lines[0].suppression_reason,
        Some("endpoint_hidden")
    );
    assert!(vertices(&session.build_scene()).is_empty());
    session.undo(Actor::Agent).unwrap();
    session
        .create_entity(
            Actor::Agent,
            "target",
            Some(DVec3::X * 200.0),
            BTreeMap::from([("targetname".into(), "End".into())]),
        )
        .unwrap();
    session
        .select(&[ObjectId::Entity(source)], SelectMode::Replace)
        .unwrap();
    let report = session.entity_helper_report(source).unwrap();
    assert_eq!(report.lines[0].state, "ambiguous");
    assert!(report.status.contains("ambiguous"));
    session.undo(Actor::Agent).unwrap();
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [DVec3::splat(-8.0), DVec3::splat(8.0)],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(
        session.entity_helper_report(source).unwrap().lines[0].suppression_reason,
        Some("endpoint_outside_working_region")
    );
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Clear,
            &session.working_region_stamp(),
        )
        .unwrap();
    session
        .set_entity_properties(
            Actor::Agent,
            source,
            &[("aim".into(), Some("Missing".into()))],
        )
        .unwrap();
    let report = session.entity_helper_report(source).unwrap();
    assert_eq!(report.lines[0].state, "missing");
    assert!(report.status.contains("missing"));
    session.undo(Actor::Agent).unwrap();
    for (class, reason) in [("light", "specialized_light"), ("model", "model_helper")] {
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::Agent,
                class,
                Some(DVec3::ZERO),
                BTreeMap::from([("aim".into(), "End".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            session.entity_helper_report(id).unwrap().lines[0].suppression_reason,
            Some(reason)
        );
    }
    session
        .select(&[ObjectId::Entity(source)], SelectMode::Replace)
        .unwrap();
    for class in ["light", "model"] {
        session
            .edit(Actor::Agent, "specialized endpoint", |tx| {
                tx.entity_mut(target)?.properties.set("classname", class);
                Ok(())
            })
            .unwrap();
        assert!(session.entity_helper_report(source).unwrap().lines[0].displayed);
        assert_eq!(vertices(&session.build_scene()).len(), 4);
    }
}

#[test]
fn scene_line_budget_is_deterministic_and_inspection_reports_omitted_guides() {
    let (mut session, source, _, _) = fixture(Actor::Agent);
    let mut sources = vec![ObjectId::Entity(source)];
    for index in 0..70 {
        sources.push(
            session
                .create_entity(
                    Actor::Agent,
                    "sample",
                    Some(DVec3::X * index as f64),
                    BTreeMap::from([
                        ("aim".into(), "End".into()),
                        ("start".into(), "Start".into()),
                    ]),
                )
                .unwrap(),
        );
    }
    session.select(&sources, SelectMode::Replace).unwrap();
    assert_eq!(
        vertices(&session.build_scene()).len(),
        MAX_DISPLAYED_LINES * 2
    );
    let ObjectId::Entity(last) = *sources.last().unwrap() else {
        unreachable!()
    };
    let report = session.entity_helper_report(last).unwrap();
    assert!(
        report
            .lines
            .iter()
            .all(|line| line.state == "ready"
                && line.suppression_reason == Some("scene_guide_limit"))
    );
}

#[test]
fn authored_nodeid_guides_share_preview_visibility_and_read_only_spatial_identity() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets".into()));
        let mut ids = Vec::new();
        for (node_id, origin) in [
            ("0", DVec3::new(-120.0, 40.0, 80.0)),
            ("42", DVec3::new(120.0, 40.0, 80.0)),
        ] {
            ids.push(
                session
                    .create_entity(
                        actor,
                        "node",
                        Some(origin),
                        BTreeMap::from([("nodeid".into(), node_id.into())]),
                    )
                    .unwrap(),
            );
        }
        session.set_definitions(actor, Catalog::parse(NODE_FGD).unwrap());
        let ObjectId::Entity(source) = session
            .create_entity(actor, "link", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap()
        else {
            panic!()
        };
        let ObjectId::Entity(target) = ids[1] else {
            panic!()
        };
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = session.selection_bounds();
        let mesh = session.build_scene();
        assert_eq!(
            vertices(&mesh)
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>(),
            [[-120.0, 40.0, 80.0], [120.0, 40.0, 80.0]]
        );
        let report = session.entity_inspector(source).unwrap().helpers.unwrap();
        assert!(report.lines[0].displayed);
        assert_eq!(report.lines[0].endpoints[0].lookup_property, Some("nodeid"));
        assert_eq!(report.lines[0].endpoints[0].value_source, "fgd_default");
        assert!(report.status.contains("authored nodeid"));
        assert!(
            session
                .set_entity_properties(actor, target, &[("nodeid".into(), Some("43".into()))])
                .unwrap_err()
                .contains("read-only")
        );
        assert!(
            session
                .pick(&Ray {
                    origin: DVec3::new(60.0, -100.0, 80.0),
                    direction: DVec3::Y
                })
                .is_none()
        );
        session
            .preview_transform(
                actor,
                &[ObjectId::Entity(source), ids[1]],
                Transform {
                    translation: DVec3::new(16.0, 24.0, 32.0),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let preview = session.entity_helper_report(source).unwrap();
        assert!(preview.lines[0].displayed);
        assert_eq!(
            preview.lines[0].endpoints[0].position,
            Some([-120.0, 40.0, 80.0])
        );
        assert_eq!(
            preview.lines[0].endpoints[1].position,
            Some([136.0, 64.0, 112.0])
        );
        assert_eq!(
            vertices(&session.build_scene())[1].position,
            [136.0, 64.0, 112.0]
        );
        session.cancel_transform_preview();
        session
            .select(&[ObjectId::Entity(source)], SelectMode::Replace)
            .unwrap();
        assert_eq!(session.selection_bounds(), bounds);
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .edit(actor, "hide node", |tx| {
                tx.entity_mut(target)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            session.entity_helper_report(source).unwrap().lines[0].suppression_reason,
            Some("endpoint_hidden")
        );
        assert!(vertices(&session.build_scene()).is_empty());
        session.undo(actor).unwrap();
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::splat(-8.0), DVec3::splat(8.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert_eq!(
            session.entity_helper_report(source).unwrap().lines[0].suppression_reason,
            Some("endpoint_outside_working_region")
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Clear,
                &session.working_region_stamp(),
            )
            .unwrap();
        session.select(&[], SelectMode::Replace).unwrap();
        let plain = session.build_scene();
        assert!(vertices(&plain).is_empty());
        assert_eq!((mesh.mins, mesh.maxs), (plain.mins, plain.maxs));
        assert_eq!(mesh.indices, plain.indices);
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}
