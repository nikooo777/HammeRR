use super::*;
use map_core::{Document, query::Ray};
use render::Projection;
use source_formats::{keyvalues::Node, vmf::Vmf};

fn request(id: Option<u32>) -> Request {
    Request {
        id,
        name: "Stair clearance".into(),
        start: DVec3::new(4.0, 7.0, 20.0),
        end: DVec3::new(1.0, 11.0, 8.0),
    }
}

fn preview(session: &mut Session, actor: Actor, request: Request) -> u64 {
    let state = session
        .preview_measurement(actor, request, &session.measurement_stamp())
        .unwrap();
    state["draft"]["token"].as_u64().unwrap()
}

#[test]
fn both_actors_preview_cancel_save_edit_delete_and_undo_without_geometry_changes() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        let original = session.document.vmf().clone();
        let token = preview(&mut session, actor, request(None));
        assert_eq!(
            session.measurement_state()["draft"]["measurement"]["distance"],
            13.0
        );
        assert_eq!(
            session.measurement_state()["draft"]["measurement"]["components"],
            json!([-3.0, 4.0, -12.0])
        );
        assert!(!session.build_scene().lines.is_empty());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.undo_labels().count(), 0);
        session.cancel_measurement(actor, token).unwrap();
        assert!(session.build_scene().lines.is_empty());
        assert_eq!(session.document.vmf(), &original);
        let token = preview(&mut session, actor, request(None));
        session.apply_measurement(actor, token).unwrap();
        assert_eq!(session.document.undo_labels().count(), 1);
        assert_eq!(session.document.vmf().world, original.world);
        assert_eq!(session.document.vmf().entities, original.entities);
        assert!(session.selection.is_empty());
        let saved = session.document.vmf().clone();
        let mut changed = request(Some(1));
        changed.name = "Clearance updated".into();
        changed.end.z = 32.0;
        let token = preview(&mut session, actor, changed);
        assert_eq!(
            session.measurements().unwrap().measurements[0].name,
            "Stair clearance"
        );
        session.apply_measurement(actor, token).unwrap();
        assert_eq!(
            session.measurements().unwrap().measurements[0].name,
            "Clearance updated"
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &saved);
        session.redo(actor).unwrap();
        session
            .delete_measurement(actor, 1, &session.measurement_stamp())
            .unwrap();
        assert!(session.measurements().unwrap().measurements.is_empty());
        session.undo(actor).unwrap();
        assert_eq!(session.measurements().unwrap().measurements.len(), 1);
        let reopened = Vmf::parse(&session.document.vmf().write(&Default::default())).unwrap();
        assert_eq!(
            Measurements::parse(&reopened).unwrap(),
            session.measurements().unwrap()
        );
    }
}

#[test]
fn ownership_replaced_tokens_stale_snapshots_and_map_generations_guard_mutation() {
    let mut session = Session::new(Err("no assets needed".into()));
    let initial = session.measurement_stamp();
    let token = preview(&mut session, Actor::Agent, request(None));
    let source = session.document.vmf().clone();
    assert!(session.apply_measurement(Actor::User, token).is_err());
    assert!(session.cancel_measurement(Actor::User, token).is_err());
    assert!(
        session
            .preview_measurement(Actor::User, request(None), &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .preview_measurement(Actor::Agent, request(None), &initial)
            .is_err()
    );
    let reviewed = session.review_measurement(Actor::User, token).unwrap();
    let current = reviewed["draft"]["token"].as_u64().unwrap();
    assert_ne!(current, token);
    assert!(session.apply_measurement(Actor::Agent, current).is_err());
    assert!(session.cancel_measurement(Actor::Agent, token).is_err());
    let mut invalid = request(None);
    invalid.end = invalid.start;
    assert!(
        session
            .preview_measurement(Actor::User, invalid, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.measurement_state()["draft"]["token"], current);
    assert_eq!(session.document.vmf(), &source);
    session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    assert_eq!(session.measurement_state()["draft"]["valid"], false);
    assert!(session.apply_measurement(Actor::User, current).is_err());
    session.cancel_measurement(Actor::User, current).unwrap();
    let old = preview(&mut session, Actor::User, request(None));
    session.new_map(Actor::Agent, true).unwrap();
    let new = preview(&mut session, Actor::User, request(None));
    assert_ne!(old, new);
    assert!(session.apply_measurement(Actor::User, old).is_err());
    assert!(session.cancel_measurement(Actor::User, old).is_err());
    assert_eq!(session.measurement_state()["draft"]["token"], new);
}

#[test]
fn geometry_previews_are_preserved_and_import_errors_are_inspectable() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let block = session.block_preview;
    assert!(block.is_some());
    assert!(
        session
            .preview_measurement(Actor::User, request(None), &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.block_preview, block);
    assert!(session.measurement_state()["draft"].is_null());
    let mut vmf = Document::empty().into_vmf();
    vmf.extra.push(Node::block(
        measurements::KEY,
        vec![Node::string("version", "future")],
    ));
    session.replace_document(Document::new(vmf.clone()), None);
    assert!(
        session.measurement_state()["metadata_error"]
            .as_str()
            .unwrap()
            .contains("Unsupported")
    );
    assert!(
        session
            .preview_measurement(Actor::Agent, request(None), &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .delete_measurement(Actor::Agent, 1, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &vmf);
    assert!(session.build_scene().lines.is_empty());
}

#[test]
fn guides_do_not_enter_picking_and_framing_and_visibility_leave_history_unchanged() {
    let mut session = Session::new(Err("no assets needed".into()));
    let token = preview(&mut session, Actor::User, request(None));
    session.apply_measurement(Actor::User, token).unwrap();
    let original = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    assert!(
        session
            .pick(&Ray {
                origin: DVec3::new(4.0, 7.0, 200.0),
                direction: -DVec3::Z
            })
            .is_none()
    );
    for projection in [
        Projection::Perspective,
        Projection::Top,
        Projection::Front,
        Projection::Side,
    ] {
        session.set_projection(Actor::User, projection);
        for aspect in [0.5, 16.0 / 9.0, 3.0] {
            session
                .frame_measurement(Some(1), None, aspect, &session.measurement_stamp())
                .unwrap();
            let matrix = session.camera.view_projection(aspect);
            for point in [request(None).start, request(None).end] {
                let clip = matrix * point.as_vec3().extend(1.0);
                let ndc = clip.truncate() / clip.w;
                assert!(
                    ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0,
                    "{projection:?} {aspect}: {ndc:?}"
                );
            }
        }
    }
    let visible = session.build_scene().lines;
    assert!(!visible.is_empty());
    session
        .set_measurement_guides(false, &session.measurement_stamp())
        .unwrap();
    assert!(session.build_scene().lines.is_empty());
    session
        .set_measurement_guides(true, &session.measurement_stamp())
        .unwrap();
    assert_eq!(session.build_scene().lines, visible);
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.document.undo_labels().count(), history);
    assert!(
        session
            .frame_measurement(Some(1), Some(1), 1.0, &session.measurement_stamp())
            .is_err()
    );
    assert!(
        session
            .frame_measurement(Some(1), None, f32::NAN, &session.measurement_stamp())
            .is_err()
    );
}

#[test]
fn saved_guides_remain_visible_when_all_scene_objects_are_hidden() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    let token = preview(&mut session, Actor::User, request(None));
    session.apply_measurement(Actor::User, token).unwrap();
    session
        .select(&[], super::super::SelectMode::Replace)
        .unwrap();
    let rulers = session.build_scene().lines;
    session
        .set_visibility(Actor::User, "hide", &[object])
        .unwrap();
    assert_eq!(session.build_scene().lines, rulers);
    assert!(
        session
            .pick(&Ray {
                origin: DVec3::splat(16.0) + DVec3::Z * 100.0,
                direction: -DVec3::Z
            })
            .is_none()
    );
}

#[test]
fn extreme_framing_returns_an_error_without_changing_the_camera() {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ruler = request(None);
    ruler.start = DVec3::splat(-measurements::MAX_COORDINATE);
    ruler.end = DVec3::splat(measurements::MAX_COORDINATE);
    let token = preview(&mut session, Actor::User, ruler);
    session.apply_measurement(Actor::User, token).unwrap();
    session.set_projection(Actor::User, Projection::Top);
    let camera = session.camera();
    assert!(
        session
            .frame_measurement(Some(1), None, 0.5, &session.measurement_stamp())
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    assert!(
        session
            .frame_measurement(
                Some(1),
                None,
                f32::MIN_POSITIVE,
                &session.measurement_stamp()
            )
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    session.set_projection(Actor::User, Projection::Perspective);
    session
        .frame_measurement(Some(1), None, 0.5, &session.measurement_stamp())
        .unwrap();
    assert!(session.camera().position.is_finite());
}
