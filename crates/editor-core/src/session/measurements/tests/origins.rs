use super::*;
use map_core::{ObjectId, edit};

fn point_entity(session: &mut Session, origin: DVec3) -> ObjectId {
    session
        .edit(Actor::User, "source entity", |tx| {
            edit::create_entity(tx, "logic_relay", Some(origin), &[], Vec::new())
        })
        .unwrap()
}

fn token(state: &Value) -> u64 {
    state["draft"]["token"].as_u64().unwrap()
}

fn attached_preview(session: &mut Session, actor: Actor, object: ObjectId) -> u64 {
    let preview = preview(session, actor, request(None));
    token(
        &session
            .attach_measurement_origin(actor, preview, Endpoint::Start, object)
            .unwrap(),
    )
}

fn saved_preview(session: &mut Session, actor: Actor) -> u64 {
    let measurement = session.measurements().unwrap().measurements.remove(0);
    let start = measurement
        .resolve_endpoint(session.document.vmf(), measurements::Endpoint::Start)
        .unwrap_or(measurement.start);
    let end = measurement
        .resolve_endpoint(session.document.vmf(), measurements::Endpoint::End)
        .unwrap_or(measurement.end);
    preview(
        session,
        actor,
        Request {
            id: Some(measurement.id),
            name: measurement.name,
            start,
            end,
        },
    )
}

#[test]
fn both_actors_attach_cancel_and_apply_exact_origins_in_one_undo_step() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        let start = point_entity(&mut session, DVec3::new(0.12345678901234567, -17.25, 31.5));
        let end = point_entity(&mut session, DVec3::new(100.25, 47.125, 65.75));
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let current = attached_preview(&mut session, actor, start);
        let state = session
            .attach_measurement_origin(actor, current, Endpoint::End, end)
            .unwrap();
        let current = token(&state);
        assert_eq!(
            state["draft"]["measurement"]["start"],
            json!([0.12345678901234567, -17.25, 31.5])
        );
        assert_eq!(
            state["draft"]["measurement"]["end"],
            json!([100.25, 47.125, 65.75])
        );
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        session.cancel_measurement(actor, current).unwrap();
        assert_eq!(session.document.vmf(), &source);
        let current = attached_preview(&mut session, actor, start);
        let state = session
            .attach_measurement_origin(actor, current, Endpoint::End, end)
            .unwrap();
        session.apply_measurement(actor, token(&state)).unwrap();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert_eq!(session.document.vmf().entities, source.entities);
        let metadata = session.measurements().unwrap();
        let saved = session.document.vmf().clone();
        let reopened = Vmf::parse(&saved.write(&Default::default())).unwrap();
        assert_eq!(Measurements::parse(&reopened).unwrap(), metadata);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &source);
        session.redo(actor).unwrap();
        assert_eq!(session.document.vmf(), &saved);
    }
}

#[test]
fn transform_preview_guides_follow_rendered_origin_while_reports_use_committed_source() {
    let mut session = Session::new(Err("no assets needed".into()));
    let origin = DVec3::new(16.25, 32.5, 64.75);
    let object = point_entity(&mut session, origin);
    let current = attached_preview(&mut session, Actor::User, object);
    session.apply_measurement(Actor::User, current).unwrap();
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let offset = DVec3::new(3.125, 5.25, 7.5);
    session
        .preview_transform(
            Actor::User,
            &[object],
            edit::Transform {
                translation: offset,
                ..Default::default()
            },
            Some(DVec3::ZERO),
        )
        .unwrap();
    let state = session.measurement_state();
    assert_eq!(state["coordinate_source"], "committed_document");
    assert_eq!(state["measurements"][0]["start"], json!(origin.to_array()));
    let mut mesh = session.build_scene();
    mesh.lines.clear();
    session.append_measurement_guides(&mut mesh);
    let start = mesh
        .lines
        .iter()
        .find(|line| line.color == [255, 210, 100, 255])
        .unwrap();
    assert_eq!(start.position, (origin + offset).as_vec3().to_array());
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    session.cancel_transform_preview();
    assert_eq!(
        session.measurement_state()["measurements"][0]["start"],
        json!(origin.to_array())
    );
}

#[test]
fn attached_origin_edits_follow_real_transforms_and_leave_metadata_bytes_unchanged() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = point_entity(&mut session, DVec3::new(16.25, 32.5, 64.75));
    let current = attached_preview(&mut session, Actor::User, object);
    session.apply_measurement(Actor::User, current).unwrap();
    let metadata = session.document.vmf().extra.clone();
    for transform in [
        edit::Transform {
            translation: DVec3::new(3.125, 5.25, 7.5),
            ..Default::default()
        },
        edit::Transform {
            rotation: DVec3::Z * 90.0,
            pivot: DVec3::splat(8.0),
            ..Default::default()
        },
        edit::Transform {
            scale: DVec3::new(2.0, 0.5, 1.5),
            pivot: DVec3::splat(4.0),
            ..Default::default()
        },
    ] {
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        session
            .transform_objects(Actor::Agent, &[object], transform, Some(transform.pivot))
            .unwrap();
        let ObjectId::Entity(id) = object else {
            panic!()
        };
        let expected = session.document.entity(id).unwrap().origin().unwrap();
        assert_eq!(
            session.measurement_state()["measurements"][0]["start"],
            json!(expected.to_array())
        );
        assert_eq!(session.document.vmf().extra, metadata);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        let transformed = session.document.vmf().clone();
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &source);
        session.redo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &transformed);
    }
    let current = saved_preview(&mut session, Actor::User);
    let detached = session
        .detach_measurement_origin(Actor::User, current, Endpoint::Start)
        .unwrap();
    let frozen = detached["draft"]["measurement"]["start"].clone();
    session
        .apply_measurement(Actor::User, token(&detached))
        .unwrap();
    session
        .edit(Actor::Agent, "move entity", |tx| {
            edit::translate(tx, &[object], DVec3::splat(20.0), true)
        })
        .unwrap();
    assert_eq!(
        session.measurement_state()["measurements"][0]["start"],
        frozen
    );
    assert!(session.measurement_state()["measurements"][0]["attachments"]["start"].is_null());
}

#[test]
fn missing_references_remain_unavailable_and_require_explicit_world_repair() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = point_entity(&mut session, DVec3::new(16.25, 32.5, 64.75));
    let current = attached_preview(&mut session, Actor::Agent, object);
    session.apply_measurement(Actor::Agent, current).unwrap();
    let original_reference = session.measurements().unwrap();
    let copies = session
        .edit(Actor::Agent, "duplicate entity", |tx| {
            edit::duplicate(tx, &[object], DVec3::splat(100.0))
        })
        .unwrap();
    assert_eq!(session.measurements().unwrap(), original_reference);
    let before_delete = session.document.vmf().clone();
    session
        .edit(Actor::Agent, "delete entity", |tx| {
            edit::delete(tx, &[object])
        })
        .unwrap();
    assert!(session.document.contains(copies[0]));
    let state = session.measurement_state();
    assert!(state["measurements"][0]["start"].is_null());
    assert!(state["measurements"][0]["distance"].is_null());
    assert_eq!(state["measurements"][0]["available"], false);
    assert!(
        state["measurements"][0]["attachments"]["start"]["error"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
    assert_eq!(session.measurements().unwrap(), original_reference);
    let mut mesh = session.build_scene();
    mesh.lines.clear();
    session.append_measurement_guides(&mut mesh);
    assert!(mesh.lines.is_empty());
    let camera = session.camera();
    assert!(
        session
            .frame_measurement(Some(1), None, 16.0 / 9.0, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &before_delete);
    assert_eq!(
        session.measurement_state()["measurements"][0]["available"],
        true
    );
    session.redo(Actor::User).unwrap();
    let current = saved_preview(&mut session, Actor::Agent);
    let before = session.measurement_state();
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    assert!(
        session
            .detach_measurement_origin(Actor::Agent, current, Endpoint::Start)
            .is_err()
    );
    assert!(session.apply_measurement(Actor::Agent, current).is_err());
    assert_eq!(session.measurement_state(), before);
    let repaired = session
        .set_measurement_world_endpoint(
            Actor::Agent,
            current,
            Endpoint::Start,
            DVec3::new(-8.125, 2.25, 4.5),
        )
        .unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    session
        .apply_measurement(Actor::Agent, token(&repaired))
        .unwrap();
    assert_eq!(session.document.undo_labels().count(), history + 1);
    assert_eq!(
        session.measurement_state()["measurements"][0]["start"],
        json!([-8.125, 2.25, 4.5])
    );
    assert!(session.measurement_state()["measurements"][0]["attachments"]["start"].is_null());
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.measurements().unwrap(), original_reference);
}

#[test]
fn origin_actions_guard_ownership_tokens_unsupported_targets_and_numeric_or_pick_replacement() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = point_entity(&mut session, DVec3::splat(16.0));
    let current = attached_preview(&mut session, Actor::Agent, object);
    let before = session.measurement_state();
    for result in [
        session.detach_measurement_origin(Actor::User, current, Endpoint::Start),
        session.attach_measurement_origin(Actor::User, current, Endpoint::End, object),
        session.set_measurement_world_endpoint(Actor::User, current, Endpoint::Start, DVec3::ZERO),
        session.attach_measurement_origin(
            Actor::Agent,
            current,
            Endpoint::End,
            ObjectId::Solid(999),
        ),
        session.attach_measurement_origin(
            Actor::Agent,
            current,
            Endpoint::End,
            ObjectId::Entity(999),
        ),
        session.attach_measurement_origin(Actor::Agent, current, Endpoint::End, object),
        session.set_measurement_world_endpoint(
            Actor::Agent,
            current,
            Endpoint::Start,
            DVec3::splat(f64::NAN),
        ),
    ] {
        assert!(result.is_err());
    }
    assert_eq!(session.measurement_state(), before);
    let mut changed = request(None);
    changed.start = DVec3::ZERO;
    assert!(
        session
            .preview_measurement(Actor::Agent, changed, &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .begin_measurement_pick(
                Actor::Agent,
                current,
                Endpoint::Start,
                session.camera(),
                (512, 512),
                &session.measurement_pick_stamp()
            )
            .is_err()
    );
    assert_eq!(session.measurement_state(), before);
    let reviewed = session.review_measurement(Actor::User, current).unwrap();
    let reviewed = token(&reviewed);
    assert_ne!(current, reviewed);
    assert!(
        session
            .detach_measurement_origin(Actor::Agent, reviewed, Endpoint::Start)
            .is_err()
    );
    assert!(
        session
            .detach_measurement_origin(Actor::User, current, Endpoint::Start)
            .is_err()
    );
    let state = session
        .detach_measurement_origin(Actor::User, reviewed, Endpoint::Start)
        .unwrap();
    assert_eq!(
        state["draft"]["measurement"]["start"],
        json!([16.0, 16.0, 16.0])
    );
    session
        .edit(Actor::Agent, "move", |tx| {
            edit::translate(tx, &[object], DVec3::X, true)
        })
        .unwrap();
    let stale = token(&state);
    assert!(
        session
            .attach_measurement_origin(Actor::User, stale, Endpoint::End, object)
            .is_err()
    );
    assert!(
        session
            .set_measurement_world_endpoint(Actor::User, stale, Endpoint::End, DVec3::Y)
            .is_err()
    );
    session.cancel_measurement(Actor::User, stale).unwrap();
}

#[test]
fn local_capture_renews_owned_tokens_and_guides_follow_actual_rotated_preview() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = point_entity(&mut session, DVec3::new(16.25, 32.5, 64.75));
    let ObjectId::Entity(id) = object else {
        panic!()
    };
    session
        .edit(Actor::User, "source orientation", |tx| {
            edit::set_properties(tx, id, &[("angles".into(), Some("20 35 -15".into()))])
        })
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let current = preview(&mut session, Actor::Agent, request(None));
    let before = session.measurement_state();
    assert!(
        session
            .attach_measurement_local(Actor::User, current, Endpoint::Start, object)
            .is_err()
    );
    assert_eq!(session.measurement_state(), before);
    let reviewed = session.review_measurement(Actor::User, current).unwrap();
    let reviewed_token = token(&reviewed);
    let stamp = session.measurement_stamp();
    let captured = session
        .attach_measurement_local(Actor::User, reviewed_token, Endpoint::Start, object)
        .unwrap();
    assert_ne!(token(&captured), reviewed_token);
    assert_ne!(session.measurement_stamp(), stamp);
    assert_eq!(
        captured["draft"]["measurement"]["attachments"]["start"]["kind"],
        "entity_local"
    );
    assert_eq!(
        captured["draft"]["measurement"]["stored_start"],
        json!(request(None).start.to_array())
    );
    let point: [f64; 3] =
        serde_json::from_value(captured["draft"]["measurement"]["start"].clone()).unwrap();
    assert!(DVec3::from_array(point).abs_diff_eq(request(None).start, 1e-10));
    assert!(
        session
            .attach_measurement_local(Actor::User, reviewed_token, Endpoint::End, object)
            .is_err()
    );
    assert_eq!(session.measurement_state(), captured);
    session
        .cancel_measurement(Actor::User, token(&captured))
        .unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    let current = preview(&mut session, Actor::User, request(None));
    let captured = session
        .attach_measurement_local(Actor::User, current, Endpoint::Start, object)
        .unwrap();
    session
        .apply_measurement(Actor::User, token(&captured))
        .unwrap();
    assert_eq!(session.document.undo_labels().count(), history + 1);
    let metadata = session.document.vmf().extra.clone();
    let transform = edit::Transform {
        rotation: DVec3::Z * 90.0,
        translation: DVec3::new(-4.0, 7.0, 10.0),
        ..Default::default()
    };
    let original = request(None).start;
    let expected = DVec3::new(-original.y, original.x, original.z) + transform.translation;
    session
        .preview_transform(Actor::User, &[object], transform, Some(DVec3::ZERO))
        .unwrap();
    let state = session.measurement_state();
    let committed: [f64; 3] =
        serde_json::from_value(state["measurements"][0]["start"].clone()).unwrap();
    assert!(DVec3::from_array(committed).abs_diff_eq(original, 1e-10));
    assert_eq!(state["coordinate_source"], "committed_document");
    let mut mesh = session.build_scene();
    mesh.lines.clear();
    session.append_measurement_guides(&mut mesh);
    let line = mesh
        .lines
        .iter()
        .find(|line| line.color == [255, 210, 100, 255])
        .unwrap();
    assert!(glam::Vec3::from_array(line.position).abs_diff_eq(expected.as_vec3(), 1e-4));
    assert_eq!(session.document.vmf().extra, metadata);
    session.cancel_transform_preview();
    session
        .transform_objects(Actor::Agent, &[object], transform, Some(DVec3::ZERO))
        .unwrap();
    let moved: [f64; 3] =
        serde_json::from_value(session.measurement_state()["measurements"][0]["start"].clone())
            .unwrap();
    assert!(DVec3::from_array(moved).abs_diff_eq(expected, 1e-5));
    assert_eq!(session.document.vmf().extra, metadata);

    session
        .edit(Actor::User, "invalid orientation", |tx| {
            edit::set_properties(tx, id, &[("angles".into(), Some("invalid".into()))])
        })
        .unwrap();
    let state = session.measurement_state();
    assert!(state["measurements"][0]["start"].is_null());
    assert!(state["measurements"][0]["distance"].is_null());
    assert_eq!(
        state["measurements"][0]["stored_start"],
        json!(original.to_array())
    );
    mesh.lines.clear();
    session.append_measurement_guides(&mut mesh);
    assert!(mesh.lines.is_empty());
    let current = saved_preview(&mut session, Actor::User);
    let unavailable = session.measurement_state();
    assert!(
        session
            .detach_measurement_origin(Actor::User, current, Endpoint::Start)
            .is_err()
    );
    assert!(session.apply_measurement(Actor::User, current).is_err());
    assert!(
        session
            .frame_measurement(
                None,
                Some(current),
                16.0 / 9.0,
                &session.measurement_stamp()
            )
            .is_err()
    );
    assert_eq!(session.measurement_state(), unavailable);
    let repaired = session
        .set_measurement_world_endpoint(Actor::User, current, Endpoint::Start, original)
        .unwrap();
    assert!(repaired["draft"]["measurement"]["attachments"]["start"].is_null());
    let origin_only = session
        .attach_measurement_origin(Actor::User, token(&repaired), Endpoint::Start, object)
        .unwrap();
    assert_eq!(
        origin_only["draft"]["measurement"]["attachments"]["start"]["kind"],
        "entity_origin"
    );
    assert_eq!(origin_only["draft"]["measurement"]["available"], true);
    session
        .apply_measurement(Actor::User, token(&origin_only))
        .unwrap();
    assert_eq!(
        session.document.vmf().extra[0]
            .children()
            .unwrap()
            .iter()
            .find(|node| node.key == "version")
            .unwrap()
            .as_str(),
        Some("2")
    );
    session.undo(Actor::User).unwrap();
    assert_eq!(
        session.measurement_state()["measurements"][0]["attachments"]["start"]["kind"],
        "entity_local"
    );
    assert_eq!(
        session.measurement_state()["measurements"][0]["available"],
        false
    );
}
