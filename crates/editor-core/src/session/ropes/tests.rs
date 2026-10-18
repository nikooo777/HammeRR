use std::collections::BTreeMap;

use glam::DVec3;
use map_core::{ObjectId, query::Ray};

use crate::session::{Actor, SelectMode, Session, working_region};

fn node(session: &mut Session, class: &str, origin: DVec3, values: &[(&str, &str)]) -> ObjectId {
    session
        .create_entity(
            Actor::Agent,
            class,
            Some(origin),
            values
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
}

fn fixture() -> (Session, ObjectId, ObjectId) {
    let mut session = Session::new(Err("no mounted assets".into()));
    let target = node(
        &mut session,
        "keyframe_rope",
        DVec3::new(256.0, 0.0, 128.0),
        &[("targetname", "end")],
    );
    let source = node(
        &mut session,
        "move_rope",
        DVec3::new(0.0, 0.0, 128.0),
        &[
            ("targetname", "start"),
            ("NextKey", "end"),
            ("Slack", "64"),
            ("Width", "12"),
        ],
    );
    session.select(&[], SelectMode::Replace).unwrap();
    (session, source, target)
}

#[test]
fn rope_preview_settings_and_picking_share_geometry_without_map_or_history_changes() {
    let (mut session, source, _) = fixture();
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let geometry = session.rope_preview_geometry();
    let link = &geometry.links[0];
    let point = link.points[link.points.len() / 2];
    let ray = Ray {
        origin: point - DVec3::Y * 100.0,
        direction: DVec3::Y,
    };
    assert_eq!(session.rope_preview_state()["enabled"], false);
    assert!(session.pick(&ray).is_none());
    assert_eq!(
        session
            .pick_with_rope_preview(&ray, Some(true))
            .unwrap()
            .object,
        source
    );
    assert!(!session.view().rope_preview);
    let serial = session.scene_serial();
    session.set_view(render::ViewOptions {
        rope_preview: true,
        tool_textures: false,
        ..session.view()
    });
    assert_eq!(session.scene_serial(), serial);
    assert_eq!(session.pick(&ray).unwrap().object, source);
    assert!(session.pick_with_rope_preview(&ray, Some(false)).is_none());
    assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 1);
    session
        .set_selection_filters(
            Actor::Agent,
            crate::session::selection_filters::SelectionFilters {
                point_entities: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.pick_for_selection(&ray).is_none());
    assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 0);
    assert_eq!(session.pick(&ray).unwrap().object, source);
    session
        .set_selection_filters(Actor::User, Default::default())
        .unwrap();
    session.select(&[source], SelectMode::Replace).unwrap();
    let mesh = session.build_scene();
    assert!(
        mesh.batches
            .iter()
            .any(|batch| batch.rope && batch.index_count > 0)
    );
    assert!(!mesh.rope_lines.is_empty());
    assert_eq!(mesh.stats.point_entities, 2);
    let endpoint = Ray {
        origin: DVec3::new(0.0, -100.0, 128.0),
        direction: DVec3::Y,
    };
    assert_eq!(session.pick(&endpoint).unwrap().object, source);
    session.set_view(render::ViewOptions {
        rope_preview: false,
        ..session.view()
    });
    assert!(session.pick(&ray).is_none());
    assert_eq!(session.pick(&endpoint).unwrap().object, source);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.selected_ids(), [source]);
    session.set_view(render::ViewOptions {
        rope_preview: true,
        ..session.view()
    });
    session.select_faces(&[], SelectMode::Replace).unwrap();
    assert!(session.pick(&ray).is_none());
    session.select(&[source], SelectMode::Replace).unwrap();
    assert_eq!(session.pick(&ray).unwrap().object, source);
}

#[test]
fn rope_preview_diagnostics_follow_authored_edits_visibility_working_regions_and_undo() {
    let (mut session, _, target) = fixture();
    let ObjectId::Entity(target_id) = target else {
        unreachable!()
    };
    session.set_view(render::ViewOptions {
        rope_preview: true,
        ..session.view()
    });
    session
        .set_entity_properties(
            Actor::Agent,
            target_id,
            &[("targetname".into(), Some("renamed".into()))],
        )
        .unwrap();
    let report = session.rope_preview_state();
    assert!(report["links"].as_array().unwrap().is_empty());
    assert!(
        report["summary"]
            .as_str()
            .unwrap()
            .contains("no authored target")
    );
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session.undo(Actor::Agent).unwrap();
    assert_eq!(
        session.rope_preview_state()["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    session
        .set_visibility(Actor::Agent, "hide", &[target])
        .unwrap();
    assert!(
        session.rope_preview_state()["summary"]
            .as_str()
            .unwrap()
            .contains("hidden")
    );
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session.undo(Actor::Agent).unwrap();
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [
                    DVec3::new(-16.0, -16.0, 100.0),
                    DVec3::new(100.0, 16.0, 160.0),
                ],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert!(
        session.rope_preview_state()["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        session.rope_preview_state()["summary"]
            .as_str()
            .unwrap()
            .contains("working region")
    );
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Clear,
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(
        session.rope_preview_state()["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rope_preview_reports_remain_compact_for_oversized_imported_identifiers_even_when_disabled() {
    let (mut session, source, target) = fixture();
    let (ObjectId::Entity(source), ObjectId::Entity(target)) = (source, target) else {
        unreachable!()
    };
    let oversized = "é".repeat(render::ropes::MAX_IDENTIFIER_BYTES * 32);
    session
        .set_entity_properties(
            Actor::Agent,
            source,
            &[("NextKey".into(), Some(oversized.clone()))],
        )
        .unwrap();
    session
        .set_entity_properties(
            Actor::Agent,
            target,
            &[("targetname".into(), Some(oversized.clone()))],
        )
        .unwrap();
    for enabled in [false, true] {
        let report = session.rope_preview_report(enabled);
        assert!(report["links"].as_array().unwrap().is_empty());
        assert_eq!(report["issues"].as_array().unwrap().len(), 2);
        assert!(report.to_string().len() < 4096);
        assert!(report["summary"].as_str().unwrap().len() < 512);
        assert!(report["issues"].as_array().unwrap().iter().all(|issue| {
            issue["message"]
                .as_str()
                .unwrap()
                .contains("identifier exceeds")
        }));
    }
    assert!(!session.view().rope_preview);
    assert_eq!(
        session
            .document()
            .entity(source)
            .unwrap()
            .properties
            .get("NextKey"),
        Some(oversized.as_str())
    );
    assert_eq!(
        session
            .document()
            .entity(target)
            .unwrap()
            .properties
            .get("targetname"),
        Some(oversized.as_str())
    );
}

#[test]
fn temporary_editor_visibility_removes_rope_spans_and_keeps_authored_nodes_available() {
    let (mut session, source, target) = fixture();
    session.set_view(render::ViewOptions {
        rope_preview: true,
        ..session.view()
    });
    let geometry = session.rope_preview_geometry();
    let point = geometry.links[0].points[geometry.links[0].points.len() / 2];
    let ray = Ray {
        origin: point - DVec3::Y * 100.0,
        direction: DVec3::Y,
    };
    assert_eq!(session.pick(&ray).unwrap().object, source);
    let before = session.document().vmf().clone();
    session
        .set_editor_visibility(
            Actor::Agent,
            crate::session::editor_visibility::Action::Hide(vec![target]),
            &session.working_region_stamp(),
        )
        .unwrap();
    assert!(session.rope_preview_geometry().links.is_empty());
    assert!(session.pick(&ray).is_none());
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::ShowAll,
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(session.pick(&ray).unwrap().object, source);
    assert_eq!(session.document().vmf(), &before);
}
