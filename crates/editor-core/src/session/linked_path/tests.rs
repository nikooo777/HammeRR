use super::*;
use source_formats::{fgd::Catalog, vmf::Vmf};

const FGD: &str = r#"
@BaseClass = PathBase [ targetname(target_source) : "Name" target(target_destination) : "Next" : "default_link" wait(float) : "Wait" : "2.5" ]
@PointClass base(PathBase) = path_corner [ speed(float) : "Speed" : "80" ]
@PointClass base(PathBase) = path_track [ orientationtype(choices) : "Orientation" : 1 = [ 0 : "None" 1 : "Face" ] ]
@PointClass = info_target [ targetname(target_source) : "Name" target(target_destination) : "Target" notes(string) : "Notes" ]
"#;

pub(crate) fn fixture() -> Session {
    let mut session = Session::new(Err("no game required".into()));
    session.set_definitions(Actor::User, Catalog::parse(FGD).unwrap());
    session
}

pub(crate) fn request() -> Request {
    Request {
        classname: "path_corner".into(),
        prefix: "route".into(),
        points: vec![
            DVec3::new(0.125, -32.25, 8.5),
            DVec3::new(64.75, 0.5, 16.125),
            DVec3::new(128.25, 64.125, 24.5),
        ],
        closed: false,
    }
}

fn preview(session: &mut Session, actor: Actor, request: Request) -> Value {
    session
        .preview_linked_path(actor, request, &session.linked_path_stamp())
        .unwrap()
}

fn token(state: &Value) -> u64 {
    state["preview"]["token"].as_u64().unwrap()
}

#[test]
fn both_actors_create_open_and_closed_paths_with_defaults_one_undo_and_saved_links() {
    for actor in [Actor::User, Actor::Agent] {
        for classname in ["path_corner", "path_track"] {
            for closed in [false, true] {
                let mut session = fixture();
                let source = session.document().vmf().clone();
                let selection = session.selected_ids();
                let camera = session.camera();
                let options = Request {
                    classname: classname.into(),
                    closed,
                    ..request()
                };
                let state = preview(&mut session, actor, options.clone());
                assert_eq!(state["preview"]["owner"], actor.label());
                assert_eq!(state["preview"]["valid"], true);
                assert_eq!(session.document().vmf(), &source);
                assert_eq!(session.document().undo_labels().count(), 0);
                assert_eq!(session.selected_ids(), selection);
                assert_eq!(session.camera(), camera);
                let expected = session.render_document().vmf().clone();
                assert_eq!(expected.entities.len(), options.points.len());
                for (index, entity) in expected.entities.iter().enumerate() {
                    assert_eq!(entity.classname(), Some(classname));
                    assert_eq!(entity.origin(), Some(options.points[index]));
                    assert_eq!(
                        entity.properties.get("targetname"),
                        Some(format!("route_{:03}", index + 1).as_str())
                    );
                    assert_eq!(entity.properties.get("wait"), Some("2.5"));
                    if classname == "path_corner" {
                        assert_eq!(entity.properties.get("speed"), Some("80"));
                    } else {
                        assert_eq!(entity.properties.get("orientationtype"), Some("1"));
                    }
                    let target = expected
                        .entities
                        .get(index + 1)
                        .or_else(|| closed.then(|| &expected.entities[0]));
                    assert_eq!(
                        entity.properties.get("target"),
                        target.and_then(|next| next.properties.get("targetname"))
                    );
                    assert!(session.entity_references(entity.id).is_err());
                }
                session.cancel_linked_path(actor, token(&state)).unwrap();
                assert_eq!(session.render_document().vmf(), &source);
                assert_eq!(session.document().undo_labels().count(), 0);
                let state = preview(&mut session, actor, options);
                let result = session.apply_linked_path(actor, token(&state)).unwrap();
                assert_eq!(
                    result["objects"].as_array().unwrap().len(),
                    expected.entities.len()
                );
                assert_eq!(session.document().vmf(), &expected);
                assert_eq!(
                    session.document().undo_labels().collect::<Vec<_>>(),
                    ["create linked path"]
                );
                assert!(session.linked_path()["preview"].is_null());
                assert_eq!(session.selected_ids().len(), expected.entities.len());
                for (index, entity) in expected.entities.iter().enumerate() {
                    assert_eq!(
                        session.entity_references(entity.id).unwrap().len(),
                        usize::from(index > 0 || closed)
                    );
                }
                session.undo(actor).unwrap();
                assert_eq!(session.document().vmf(), &source);
                session.redo(actor).unwrap();
                assert_eq!(session.document().vmf(), &expected);
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("linked-path.vmf");
                session.save(actor, Some(&path)).unwrap();
                let saved = std::fs::read_to_string(&path).unwrap();
                assert_eq!(Vmf::parse(&saved).unwrap().entities, expected.entities);
                session.open(actor, &path, true).unwrap();
                assert_eq!(session.document().vmf().entities, expected.entities);
            }
        }
    }
}

#[test]
fn names_avoid_case_insensitive_hidden_collisions_and_reference_acquisitions_reject() {
    let mut session = fixture();
    for name in ["ROUTE_001", "route_002"] {
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::Agent,
                "info_target",
                Some(DVec3::ZERO),
                BTreeMap::from([("targetname".into(), name.into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        session
            .edit(Actor::Agent, "hide fixture", |tx| {
                tx.entity_mut(id)?.hidden = true;
                Ok(())
            })
            .unwrap();
    }
    for (key, value) in [
        ("target", "guard_001"),
        ("target", "trap_*"),
        ("notes", "text_001"),
    ] {
        session
            .create_entity(
                Actor::Agent,
                "info_target",
                Some(DVec3::ZERO),
                BTreeMap::from([(key.into(), value.into())]),
            )
            .unwrap();
    }
    session
        .edit(Actor::Agent, "unresolved I/O fixture", |tx| {
            let ObjectId::Entity(id) = map_core::edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::ZERO),
                &[],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            tx.entity_mut(id)?.connections = Some(vec![source_formats::vmf::Connection {
                output: "OnTrigger".into(),
                value: "wire_*,Trigger,,0,-1".into(),
            }]);
            Ok(())
        })
        .unwrap();
    let state = preview(&mut session, Actor::Agent, request());
    assert_eq!(state["preview"]["nodes"][0]["targetname"], "route_001_2");
    assert_eq!(state["preview"]["nodes"][1]["targetname"], "route_002_2");
    assert_eq!(state["preview"]["nodes"][2]["targetname"], "route_003");
    let source = session.document().vmf().clone();
    let rendered = session.render_document().vmf().clone();
    for prefix in ["guard", "trap", "text", "wire"] {
        let error = session
            .preview_linked_path(
                Actor::Agent,
                Request {
                    prefix: prefix.into(),
                    ..request()
                },
                &session.linked_path_stamp(),
            )
            .unwrap_err();
        assert!(
            error.contains("would acquire an existing reference"),
            "{error}"
        );
        assert_eq!(session.linked_path(), state);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &rendered);
    }
}

#[test]
fn wildcard_path_targets_without_fgd_cannot_acquire_new_nodes() {
    for actor in [Actor::User, Actor::Agent] {
        for classname in ["path_corner", "path_track"] {
            let mut session = Session::new(Err("no definitions or assets".into()));
            assert!(session.definitions.class(classname).is_none());
            session
                .create_entity(
                    actor,
                    classname,
                    Some(DVec3::ZERO),
                    BTreeMap::from([
                        ("targetname".into(), "existing_path".into()),
                        ("target".into(), "route_*".into()),
                    ]),
                )
                .unwrap();
            let scan = references::scan(&session.document, &session.definitions);
            let reference = scan.iter().find(|item| item.target == "route_*").unwrap();
            assert!(!reference.declared);
            let source = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let state = preview(
                &mut session,
                actor,
                Request {
                    prefix: "safe".into(),
                    ..request()
                },
            );
            let rendered = session.render_document().vmf().clone();
            let error = session
                .preview_linked_path(actor, request(), &session.linked_path_stamp())
                .unwrap_err();
            assert!(
                error.contains("would acquire an existing reference"),
                "{error}"
            );
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.document().undo_labels().count(), history);
            assert_eq!(session.render_document().vmf(), &rendered);
            assert_eq!(session.linked_path(), state);
        }
    }
}

#[test]
fn invalid_requests_and_stale_snapshots_preserve_the_current_preview() {
    let mut session = fixture();
    let state = preview(&mut session, Actor::User, request());
    let rendered = session.render_document().vmf().clone();
    let history = session.document().undo_labels().count();
    let mut invalid = Vec::new();
    for prefix in ["", "1route", " route", "wild*", "route/name"] {
        invalid.push(Request {
            prefix: prefix.into(),
            ..request()
        });
    }
    invalid.push(Request {
        prefix: "r".repeat(MAX_PREFIX_BYTES + 1),
        ..request()
    });
    invalid.push(Request {
        classname: "func_tracktrain".into(),
        ..request()
    });
    for points in [
        vec![],
        vec![DVec3::ZERO],
        vec![DVec3::ZERO; MAX_POINTS + 1],
        vec![DVec3::ZERO, DVec3::ZERO],
        vec![DVec3::ZERO, DVec3::splat(f64::NAN)],
        vec![DVec3::ZERO, DVec3::splat(f64::INFINITY)],
        vec![DVec3::ZERO, DVec3::X * (crate::spatial::MAX_EXTENT + 0.125)],
    ] {
        invalid.push(Request {
            points,
            ..request()
        });
    }
    for options in invalid {
        assert!(
            session
                .preview_linked_path(Actor::User, options, &session.linked_path_stamp())
                .is_err()
        );
        assert_eq!(session.linked_path(), state);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.document().undo_labels().count(), history);
    }
    assert!(
        session
            .preview_linked_path(Actor::User, request(), "stale")
            .is_err()
    );
    assert_eq!(session.linked_path(), state);
}

#[test]
fn foreign_actions_require_review_and_old_tokens_cannot_change_replacement_work() {
    let mut session = fixture();
    let state = preview(&mut session, Actor::Agent, request());
    let old = token(&state);
    assert!(session.apply_linked_path(Actor::User, old).is_err());
    assert!(session.cancel_linked_path(Actor::User, old).is_err());
    assert!(
        session
            .preview_linked_path(Actor::User, request(), &session.linked_path_stamp())
            .is_err()
    );
    assert_eq!(session.linked_path(), state);
    let reviewed = session.review_linked_path(Actor::User, old).unwrap();
    let current = token(&reviewed);
    assert_ne!(current, old);
    assert_eq!(reviewed["preview"]["owner"], Actor::User.label());
    for actor in [Actor::User, Actor::Agent] {
        assert!(session.cancel_linked_path(actor, old).is_err());
        assert!(session.apply_linked_path(actor, old).is_err());
    }
    assert!(session.cancel_linked_path(Actor::Agent, current).is_err());
    assert_eq!(session.linked_path(), reviewed);
    session.cancel_linked_path(Actor::User, current).unwrap();
    let state = preview(&mut session, Actor::Agent, request());
    let replacement = preview(
        &mut session,
        Actor::Agent,
        Request {
            closed: true,
            ..request()
        },
    );
    assert!(
        session
            .cancel_linked_path(Actor::Agent, token(&state))
            .is_err()
    );
    assert_eq!(session.linked_path(), replacement);
    session
        .apply_linked_path(Actor::Agent, token(&replacement))
        .unwrap();
}

#[test]
fn definitions_edits_selection_and_other_geometry_previews_invalidate_old_tokens() {
    let mut session = fixture();
    let existing = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap();
    let state = preview(&mut session, Actor::User, request());
    session.definitions_serial += 1;
    assert_eq!(session.linked_path()["preview"]["valid"], false);
    assert!(
        session
            .apply_linked_path(Actor::User, token(&state))
            .is_err()
    );
    assert!(
        session
            .review_linked_path(Actor::Agent, token(&state))
            .is_err()
    );
    session
        .cancel_linked_path(Actor::User, token(&state))
        .unwrap();
    let state = preview(&mut session, Actor::User, request());
    session.select(&[], SelectMode::Replace).unwrap();
    assert!(session.linked_path()["preview"].is_null());
    assert!(
        session
            .apply_linked_path(Actor::User, token(&state))
            .is_err()
    );
    let state = preview(&mut session, Actor::User, request());
    session
        .preview_transform(
            Actor::User,
            &[existing],
            map_core::edit::Transform {
                translation: DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let replacement = session.render_document().vmf().clone();
    assert!(
        session
            .cancel_linked_path(Actor::User, token(&state))
            .is_err()
    );
    assert_eq!(session.render_document().vmf(), &replacement);
    session.cancel_transform_preview();
    let state = preview(&mut session, Actor::User, request());
    session.set_definitions(Actor::Agent, Catalog::parse(FGD).unwrap());
    assert!(
        session
            .apply_linked_path(Actor::User, token(&state))
            .is_err()
    );
    assert!(session.linked_path()["preview"].is_null());
}

#[test]
fn incompatible_fgd_classes_and_read_only_link_fields_reject_atomically() {
    for fgd in [
        "@SolidClass = path_corner []",
        "@PointClass = path_corner [ targetname(string) readonly : \"Name\" ]",
        "@PointClass = path_corner [ target(target_destination) readonly : \"Next\" ]",
        "@PointClass = path_corner [ targetname(integer) : \"Name\" ]",
    ] {
        let mut session = fixture();
        session.set_definitions(Actor::Agent, Catalog::parse(fgd).unwrap());
        let source = session.document().vmf().clone();
        assert!(
            session
                .preview_linked_path(Actor::Agent, request(), &session.linked_path_stamp())
                .is_err()
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), 0);
        assert!(session.linked_path()["preview"].is_null());
    }
}

#[test]
fn changed_preview_bytes_reject_without_partial_entities_or_history() {
    let mut session = fixture();
    let state = preview(&mut session, Actor::Agent, request());
    let preview = session.geometry.linked_path.preview.as_mut().unwrap();
    preview
        .document
        .edit("alter preview", |tx| {
            tx.world_properties_mut().set("message", "changed");
            Ok(())
        })
        .unwrap();
    let source = session.document().vmf().clone();
    assert!(
        session
            .apply_linked_path(Actor::Agent, token(&state))
            .unwrap_err()
            .contains("differs from its preview")
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), 0);
    assert_eq!(session.linked_path()["preview"]["token"], token(&state));
}
