use super::*;
use source_formats::vmf::Entity;

fn fixture(count: u32) -> (tempfile::TempDir, Session, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("Comparison needs no game".into()));
    session
        .edit(Actor::User, "comparison fixture", |tx| {
            for id in 10..10 + count {
                let mut entity = Entity::new(id, "logic_relay");
                entity.properties.set("targetname", format!("Relay {id}"));
                entity.properties.set("origin", format!("{} 0 64", id * 8));
                tx.add_entity(entity)?;
            }
            Ok(())
        })
        .unwrap();
    let path = directory.path().join("external.vmf");
    std::fs::write(&path, session.document.vmf().write(&Default::default())).unwrap();
    (directory, session, path)
}
fn prepare(session: &mut Session, path: &Path) -> (u64, Value) {
    let request = session
        .begin_external_comparison(Actor::User, path)
        .unwrap();
    let token = request.token();
    let page = session
        .finish_external_comparison(Actor::User, request.run())
        .unwrap();
    (token, page)
}
fn query(session: &mut Session, token: u64, query: Query) -> Result<Value, String> {
    let request = session.query_external_comparison(Actor::User, token, query)?;
    session.finish_external_comparison(Actor::User, request.run())
}

#[test]
fn comparison_pages_select_complete_filtered_current_set_without_edits() {
    let (_directory, mut session, path) = fixture(70);
    let original = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let (token, page) = prepare(&mut session, &path);
    assert_eq!(page["rows"].as_array().unwrap().len(), 64);
    assert_eq!(page["current_matches"], 70);
    let page = query(
        &mut session,
        token,
        Query {
            offset: 64,
            ..Query::default()
        },
    )
    .unwrap();
    assert_eq!(page["rows"].as_array().unwrap().len(), 6);
    let page = query(
        &mut session,
        token,
        Query {
            offset: 64,
            limit: 1,
            action: Action::Select(SelectMode::Replace),
            ..Query::default()
        },
    )
    .unwrap();
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(session.selection.len(), 70);
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.document.undo_labels().count(), history);
    query(
        &mut session,
        token,
        Query {
            filter: Filter {
                status: Some(comparison::Status::Identical),
                search: "Relay 1".into(),
            },
            action: Action::Select(SelectMode::Replace),
            ..Query::default()
        },
    )
    .unwrap();
    assert_eq!(session.selection.len(), 10);
}

#[test]
fn changed_source_clears_rows_and_never_silently_rebaselines() {
    let (_directory, mut session, path) = fixture(2);
    let (token, _) = prepare(&mut session, &path);
    let request = session
        .query_external_comparison(Actor::User, token, Query::default())
        .unwrap();
    let completed = request.run();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{text}\n// external edit\n")).unwrap();
    assert!(
        session
            .finish_external_comparison(Actor::User, completed)
            .is_err()
    );
    let state = session.external_comparison_state();
    assert_eq!(state["status"], "failed");
    assert!(state["page"].is_null());
    assert!(
        session
            .query_external_comparison(Actor::User, token, Query::default())
            .is_err()
    );
    let (_, page) = prepare(&mut session, &path);
    assert_eq!(page["current_matches"], 2);
}

#[test]
fn ownership_cancellation_and_replacement_retire_exact_work() {
    let (_directory, mut session, path) = fixture(2);
    let request = session
        .begin_external_comparison(Actor::User, &path)
        .unwrap();
    let token = request.token();
    assert!(
        session
            .cancel_external_comparison(Actor::Agent, token)
            .is_err()
    );
    session
        .cancel_external_comparison(Actor::User, token)
        .unwrap();
    assert!(session.instance_workers_busy());
    assert!(
        session
            .begin_external_comparison(Actor::User, &path)
            .is_err()
    );
    assert!(
        session
            .finish_external_comparison(Actor::User, request.run())
            .is_err()
    );
    assert!(!session.instance_workers_busy());
    let (token, _) = prepare(&mut session, &path);
    let state = session
        .review_external_comparison(Actor::Agent, token)
        .unwrap();
    assert_ne!(state["token"], token);
    assert!(
        session
            .query_external_comparison(Actor::User, token, Query::default())
            .is_err()
    );
    let token = state["token"].as_u64().unwrap();
    let request = session
        .query_external_comparison(Actor::Agent, token, Query::default())
        .unwrap();
    session.new_map(Actor::User, true).unwrap();
    assert!(
        session
            .finish_external_comparison(Actor::Agent, request.run())
            .is_err()
    );
    assert!(session.external_comparison_state()["token"].is_null());
}

#[test]
fn stale_current_map_and_shutdown_do_not_publish_completions() {
    let (_directory, mut session, path) = fixture(2);
    let request = session
        .begin_external_comparison(Actor::User, &path)
        .unwrap();
    session
        .edit(Actor::User, "changed map", |tx| {
            tx.entity_mut(10)?.properties.set("targetname", "changed");
            Ok(())
        })
        .unwrap();
    assert!(
        session
            .finish_external_comparison(Actor::User, request.run())
            .is_err()
    );
    assert!(session.external_comparison_state()["page"].is_null());
    let request = session
        .begin_external_comparison(Actor::User, &path)
        .unwrap();
    let control = request.control.clone();
    session.shutdown();
    assert!(control.cancelled());
    assert!(
        session
            .finish_external_comparison(Actor::User, request.run())
            .is_err()
    );
    assert!(!session.instance_workers_busy());
}

#[test]
fn actions_preserve_intervening_intent_and_frame_requires_every_member_bounds() {
    fn ready_selection(session: &mut Session, token: u64) -> Completed {
        session
            .query_external_comparison(
                Actor::User,
                token,
                Query {
                    action: Action::Select(SelectMode::Replace),
                    ..Query::default()
                },
            )
            .unwrap()
            .run()
    }
    let (_directory, mut session, path) = fixture(2);
    let ObjectId::Solid(solid) = session
        .edit(Actor::User, "intent face fixture", |tx| {
            map_core::edit::create_block(
                tx,
                glam::DVec3::ZERO,
                glam::DVec3::splat(64.0),
                "brick/test",
            )
        })
        .unwrap()
    else {
        panic!("expected world brush")
    };
    let face = map_core::surface::FaceId {
        solid,
        side: session.document.vmf().world.solids[0].sides[0].id,
    };
    std::fs::write(&path, session.document.vmf().write(&Default::default())).unwrap();
    let (token, _) = prepare(&mut session, &path);
    let request = session
        .query_external_comparison(
            Actor::User,
            token,
            Query {
                action: Action::Frame(1.5),
                ..Query::default()
            },
        )
        .unwrap();
    let completed = request.run();
    let mut camera = session.camera();
    camera.position.x += 128.0;
    session.publish_camera(camera);
    assert!(
        session
            .finish_external_comparison(Actor::User, completed)
            .unwrap_err()
            .contains("Camera changed")
    );
    assert_eq!(session.camera(), camera);
    let request = session
        .query_external_comparison(Actor::User, token, Query::default())
        .unwrap();
    let completed = request.run();
    camera.position.y += 32.0;
    session.publish_camera(camera);
    session
        .finish_external_comparison(Actor::User, completed)
        .unwrap();
    assert!(session.selected_ids().is_empty());
    assert!(!session.face_mode);
    let completed = ready_selection(&mut session, token);
    session.select_faces(&[], SelectMode::Replace).unwrap();
    assert!(
        session
            .finish_external_comparison(Actor::User, completed)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert!(session.face_mode);
    assert!(session.selected_faces().is_empty());
    assert!(session.selected_ids().is_empty());
    let completed = ready_selection(&mut session, token);
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    assert!(
        session
            .finish_external_comparison(Actor::User, completed)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert!(session.face_mode);
    assert_eq!(session.selected_faces(), [face]);
    assert!(session.selected_ids().is_empty());
    session
        .select(&[ObjectId::Solid(solid)], SelectMode::Replace)
        .unwrap();
    let completed = ready_selection(&mut session, token);
    let component = map_core::subobject::Component {
        solid,
        sides: vec![face.side],
    };
    session
        .select_components(
            std::slice::from_ref(&component),
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    let serial = session.geometry.components.serial;
    assert!(
        session
            .finish_external_comparison(Actor::User, completed)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert_eq!(session.selected_ids(), [ObjectId::Solid(solid)]);
    assert!(session.component_selection().contains(&component));
    assert_eq!(session.geometry.components.serial, serial);
    let completed = ready_selection(&mut session, token);
    session
        .select(&[ObjectId::Entity(11)], SelectMode::Replace)
        .unwrap();
    assert!(
        session
            .finish_external_comparison(Actor::User, completed)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert_eq!(session.selected_ids(), [ObjectId::Entity(11)]);
    let completed = ready_selection(&mut session, token);
    camera.position.z += 16.0;
    session.publish_camera(camera);
    session
        .finish_external_comparison(Actor::User, completed)
        .unwrap();
    assert_eq!(session.selection.len(), 3);
    session
        .edit(Actor::User, "missing member bounds", |tx| {
            tx.entity_mut(11)?.properties.remove("origin");
            Ok(())
        })
        .unwrap();
    std::fs::write(&path, session.document.vmf().write(&Default::default())).unwrap();
    let (token, _) = prepare(&mut session, &path);
    let before = session.camera();
    let error = query(
        &mut session,
        token,
        Query {
            action: Action::Frame(1.5),
            ..Query::default()
        },
    )
    .unwrap_err();
    assert!(
        error.contains("entity 11") && error.contains("bounds"),
        "{error}"
    );
    assert_eq!(session.camera(), before);
}

#[test]
fn ambiguous_and_external_only_rows_are_never_selected() {
    let (_directory, mut session, path) = fixture(2);
    let mut external = session.document.vmf().clone();
    external.entities.push(external.entities[0].clone());
    external.entities.push(Entity::new(99, "logic_relay"));
    std::fs::write(&path, external.write(&Default::default())).unwrap();
    let (token, page) = prepare(&mut session, &path);
    assert_eq!(page["current_matches"], 1);
    query(
        &mut session,
        token,
        Query {
            action: Action::Select(SelectMode::Replace),
            ..Query::default()
        },
    )
    .unwrap();
    assert_eq!(session.selected_ids(), [ObjectId::Entity(11)]);
    assert!(
        query(
            &mut session,
            token,
            Query {
                filter: Filter {
                    status: Some(comparison::Status::Ambiguous),
                    search: String::new()
                },
                action: Action::Frame(1.0),
                ..Query::default()
            }
        )
        .unwrap_err()
        .contains("No unambiguous")
    );
}

#[test]
fn retired_owner_and_dropped_session_cancel_only_their_original_controls() {
    let (_directory, mut session, path) = fixture(1);
    let (token, _) = prepare(&mut session, &path);
    let request = session
        .query_external_comparison(Actor::User, token, Query::default())
        .unwrap();
    let former = request.cancellation();
    session
        .finish_external_comparison(Actor::User, request.run())
        .unwrap();
    let next = session
        .review_external_comparison(Actor::Agent, token)
        .unwrap()["token"]
        .as_u64()
        .unwrap();
    former.cancel().unwrap();
    let request = session
        .query_external_comparison(Actor::Agent, next, Query::default())
        .unwrap();
    let current = request.cancellation();
    assert!(!current.cancelled());
    session
        .finish_external_comparison(Actor::Agent, request.run())
        .unwrap();
    let (_peer_directory, mut peer, peer_path) = fixture(1);
    let request = peer
        .begin_external_comparison(Actor::User, &peer_path)
        .unwrap();
    let peer_control = request.cancellation();
    drop(session);
    assert!(current.cancelled());
    assert!(!peer_control.cancelled());
    peer.finish_external_comparison(Actor::User, request.run())
        .unwrap();
    let token = peer.external_comparison_state()["token"].as_u64().unwrap();
    let request = peer
        .query_external_comparison(Actor::User, token, Query::default())
        .unwrap();
    let control = request.cancellation();
    drop(peer);
    assert!(control.cancelled());
    assert!(request.run().result.is_err());
}

#[test]
fn unrepresentable_source_paths_refuse_before_snapshot_or_worker() {
    use std::os::unix::ffi::OsStringExt;
    let (_directory, mut session, _path) = fixture(1);
    let source = session.document.vmf().clone();
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"external-\xff.vmf".to_vec()));
    assert!(
        session
            .begin_external_comparison(Actor::User, &path)
            .err()
            .unwrap()
            .contains("UTF-8")
    );
    assert!(session.external_comparison_state()["token"].is_null());
    assert!(!session.instance_workers_busy());
    assert_eq!(session.document.vmf(), &source);
}
