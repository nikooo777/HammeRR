use super::*;
use map_core::displacement::Operation;
use map_core::displacement::brush::{Axis, Brush, Sculpt};
use serde_json::{Value, json};

fn fixture() -> (Session, ObjectId, ObjectId) {
    let mut session = Session::new(Err("no assets needed".into()));
    let source = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(128.0),
            "test",
            false,
        )
        .unwrap();
    let cutter = session
        .create_block(
            Actor::Agent,
            DVec3::new(48.0, -16.0, -16.0),
            DVec3::new(80.0, 144.0, 144.0),
            "test",
            false,
        )
        .unwrap();
    session.select(&[source], SelectMode::Replace).unwrap();
    (session, source, cutter)
}

fn token(report: &Value, kind: &str, legacy: &Value) -> u64 {
    let token = report["handle"]["token"].as_str().unwrap();
    assert_eq!(report["handle"]["kind"], kind);
    let token = token.parse::<u64>().unwrap();
    if let Some(legacy) = legacy.as_str() {
        assert_eq!(token.to_string(), legacy);
    } else {
        assert_eq!(legacy.as_u64(), Some(token));
    }
    token
}

#[test]
fn geometry_handles_track_owners_and_report_selection_invalidation() {
    let (mut session, source, cutter) = fixture();
    let before = session.document().vmf().clone();
    let history = session.history_snapshot();
    assert!(session.hollow_state()["handle"].is_null());
    assert!(session.carve_state()["handle"].is_null());
    assert!(session.repeat_array()["handle"].is_null());
    session
        .preview_hollow(
            &[source],
            map_core::hollow::Spec {
                thickness: 8.0,
                outward: false,
                open_faces: Default::default(),
            },
            &session.hollow_stamp(),
        )
        .unwrap();
    let hollow = session.hollow_state();
    let hollow_token = token(&hollow, "hollow", &hollow["preview"]["token"]);
    let unchanged = session
        .select_grouped_report(&[source], SelectMode::Replace)
        .unwrap();
    assert_eq!(unchanged["cleared_preview_kinds"], json!([]));
    assert_eq!(session.hollow_state(), hollow);

    let repeated = session
        .preview_repeat_array(&[source], repeat::tests::options(), &session.repeat_stamp())
        .unwrap();
    let repeat_token = token(&repeated, "repeat_array", &repeated["preview"]["token"]);
    assert!(session.hollow_state()["handle"].is_null());
    assert!(session.cancel_hollow(hollow_token).is_err());
    assert_eq!(session.repeat_array(), repeated);
    let cleared = session
        .select_grouped_report(&[cutter], SelectMode::Replace)
        .unwrap();
    assert_eq!(cleared["cleared_preview_kinds"], json!(["repeat_array"]));
    assert!(session.repeat_array()["handle"].is_null());
    assert!(
        session
            .apply_repeat_array(Actor::Agent, repeat_token)
            .is_err()
    );

    let [ObjectId::Solid(source_id), ObjectId::Solid(cutter_id)] = [source, cutter] else {
        panic!()
    };
    session
        .preview_carve(cutter_id, &[source_id], &session.carve_stamp())
        .unwrap();
    let carved = session.carve_state();
    let carve_token = token(&carved, "carve", &carved["preview"]["token"]);
    let current = session.selected_ids();
    let unchanged = session
        .select_grouped_report(&current, SelectMode::Replace)
        .unwrap();
    assert_eq!(unchanged["cleared_preview_kinds"], json!([]));
    assert_eq!(session.carve_state(), carved);
    let cleared = session
        .select_grouped_report(&[], SelectMode::Replace)
        .unwrap();
    assert_eq!(cleared["cleared_preview_kinds"], json!(["carve"]));
    assert!(session.carve_state()["handle"].is_null());
    assert!(session.commit_carve(Actor::Agent, carve_token).is_err());
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn property_and_connection_handles_keep_captured_targets_and_stale_guards() {
    let (mut session, source, _) = fixture();
    let ObjectId::Entity(entity) = session
        .create_entity(
            Actor::Agent,
            "logic_relay",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    session.begin_property_edit(PropertyTarget::World).unwrap();
    session.begin_connection_edit(entity, None).unwrap();
    let property = session.property_edit_report().unwrap();
    let property_token = token(&property, "property_edit", &property["draft"]["token"]);
    let connection = session.connection_draft_report();
    let connection_token = token(&connection, "connection_edit", &connection["token"]);
    session
        .select_grouped_report(&[source], SelectMode::Replace)
        .unwrap();
    assert_eq!(session.property_edit_report().unwrap(), property);
    assert_eq!(session.connection_draft_report(), connection);
    session
        .begin_property_edit(PropertyTarget::Entities(vec![entity]))
        .unwrap();
    session.begin_connection_edit(entity, None).unwrap();
    assert!(session.cancel_property_edit(property_token).is_err());
    assert!(session.check_connection_draft(connection_token).is_err());
    let replacement = session.property_edit_report().unwrap();
    let replacement_token = token(
        &replacement,
        "property_edit",
        &replacement["draft"]["token"],
    );
    session.cancel_property_edit(replacement_token).unwrap();
    assert!(session.property_edit_report().is_err());
    assert!(session.property_draft().is_none());
    session
        .edit(Actor::Agent, "move source", |tx| {
            map_core::edit::translate(tx, &[source], DVec3::X, true)
        })
        .unwrap();
    assert!(session.connection_draft_report()["handle"].is_null());
    assert!(session.commit_connection_edit(Actor::Agent).is_err());
    session.cancel_connection_edit();
    assert!(session.connection_draft_report().is_null());
}

#[test]
fn terrain_handles_distinguish_strokes_face_previews_and_foreign_ownership() {
    let (mut session, source, _) = fixture();
    let ObjectId::Solid(solid) = source else {
        panic!()
    };
    let side = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    session.create_terrain(Actor::Agent, &[face], 3, 2).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let operation = Operation::Sculpt {
        brush: Brush {
            center: DVec3::new(64.0, 64.0, 128.0),
            radius: 32.0,
            axis: Axis::WorldZ,
        },
        operation: Sculpt::Raise(4.0),
    };
    let stroke = session
        .begin_terrain_stroke(Actor::Agent, &[face], operation.clone(), None)
        .unwrap();
    let begun = session.terrain_stroke_json();
    assert_eq!(token(&begun, "terrain_stroke", &begun["token"]), stroke);
    assert!(session.face_preview_handle().is_null());
    session
        .update_terrain_stroke(Actor::Agent, stroke, &[DVec3::new(64.0, 64.0, 128.0)])
        .unwrap();
    let face_handle = session.face_preview_handle();
    let preview = session.face_preview_token().unwrap();
    assert_eq!(
        face_handle,
        json!({"kind":"face_preview","token":preview.to_string()})
    );
    let updated = session.terrain_stroke_json();
    assert_eq!(token(&updated, "terrain_stroke", &updated["token"]), stroke);
    assert_eq!(
        session.finish_terrain_stroke(Actor::Agent, stroke).unwrap(),
        preview
    );
    assert!(session.terrain_stroke_json()["handle"].is_null());
    assert_eq!(session.face_preview_handle(), face_handle);
    session.cancel_face_preview();
    assert!(session.face_preview_handle().is_null());

    let stroke = session
        .begin_terrain_stroke(Actor::Agent, &[face], operation.clone(), None)
        .unwrap();
    session
        .update_terrain_stroke(Actor::Agent, stroke, &[DVec3::new(64.0, 64.0, 128.0)])
        .unwrap();
    session
        .preview_terrain_edit(Actor::Agent, &[face], operation)
        .unwrap();
    let foreign = session.face_preview_handle();
    assert_eq!(session.terrain_stroke_json()["stale"], true);
    assert!(session.terrain_stroke_json()["handle"].is_null());
    assert!(session.finish_terrain_stroke(Actor::Agent, stroke).is_err());
    session.cancel_terrain_stroke(stroke).unwrap();
    assert_eq!(session.face_preview_handle(), foreign);
    session.cancel_face_preview();
    assert!(session.face_preview_handle().is_null());
}
