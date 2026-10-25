use super::*;

fn opened() -> (tempfile::TempDir, Session, PathBuf, PathBuf, u32) {
    let (directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let id = session.document.vmf().entities.last().unwrap().id;
    (directory, session, parent, child, id)
}

fn begin(session: &mut Session, actor: Actor, id: Option<u32>) -> u64 {
    session.begin_instance_reference(actor, id).unwrap()["token"]
        .as_u64()
        .unwrap()
}

#[test]
fn instance_reference_create_edit_and_noop_preserve_fields_with_one_undo_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, parent, child, existing) = opened();
        let original = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let camera = session.camera();
        let disk = std::fs::read(&parent).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let token = begin(&mut session, actor, None);
        session
            .update_instance_reference(
                actor,
                token,
                Changes {
                    file: Some("./child.vmf".into()),
                    origin: Some("80 16 8".into()),
                    angles: Some("0 90 0".into()),
                },
            )
            .unwrap();
        let checked = session.check_instance_reference(actor, token).unwrap();
        assert_eq!(checked["presence_checked"], true);
        assert_eq!(checked["contents_validation"], "not_checked");
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.undo_labels().count(), history);
        let result = session.apply_instance_reference(actor, token).unwrap();
        let ObjectId::Entity(id) =
            crate::identity::parse_id(result["id"].as_str().unwrap()).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(session.document.undo_labels().count(), history + 1);
        let created = session.document.entity(id).unwrap();
        assert_eq!(created.properties.get("file"), Some("./child.vmf"));
        assert_eq!(created.origin(), Some(DVec3::new(80.0, 16.0, 8.0)));
        assert_eq!(created.properties.get("angles"), Some("0 90 0"));
        assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
        assert_eq!(session.camera(), camera);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &original);
        session.redo(actor).unwrap();
        let created = session.document.entity(id).unwrap().clone();
        let token = begin(&mut session, actor, Some(id));
        session
            .update_instance_reference(
                actor,
                token,
                Changes {
                    origin: Some("10 20 30".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        session.apply_instance_reference(actor, token).unwrap();
        let mut expected = created;
        expected.properties.set("origin", "10 20 30");
        assert_eq!(session.document.entity(id).unwrap(), &expected);
        session.undo(actor).unwrap();
        session
            .set_entity_properties(
                actor,
                existing,
                &[("origin".into(), None), ("angles".into(), None)],
            )
            .unwrap();
        let source = session.document.vmf().clone();
        let history = session.history_snapshot();
        let token = begin(&mut session, actor, Some(existing));
        session.apply_instance_reference(actor, token).unwrap();
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(std::fs::read(&parent).unwrap(), disk);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        assert_eq!(
            std::fs::read(parent.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(parent.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

#[test]
fn instance_reference_presence_does_not_claim_malformed_or_runtime_child_admission() {
    let (_directory, mut session, _parent, child, _) = opened();
    for content in [
        "world {",
        "world { \"id\" \"1\" } entity { \"id\" \"2\" \"classname\" \"info_target\" \"origin\" \"0 0 8\" }",
    ] {
        std::fs::write(&child, content).unwrap();
        let token = begin(&mut session, Actor::Agent, None);
        session
            .update_instance_reference(
                Actor::Agent,
                token,
                Changes {
                    file: Some("child.vmf".into()),
                    origin: Some("0 0 0".into()),
                    angles: Some("0 0 0".into()),
                },
            )
            .unwrap();
        assert_eq!(
            session
                .check_instance_reference(Actor::Agent, token)
                .unwrap()["presence_checked"],
            true
        );
        assert_eq!(
            session
                .apply_instance_reference(Actor::Agent, token)
                .unwrap()["contents_validation"],
            "not_checked"
        );
        let source = session.document.vmf().clone();
        let history = session.history_snapshot();
        let request = session
            .begin_instance_contents_refresh(Actor::Agent)
            .unwrap();
        let result = request.run();
        let report = session
            .finish_instance_contents_refresh(Actor::Agent, result)
            .unwrap();
        assert_eq!(report["state"], "failed");
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        session.undo(Actor::Agent).unwrap();
    }
}

#[test]
fn instance_reference_ownership_source_and_selected_file_guards_preserve_existing_state() {
    let (_directory, mut session, parent, child, id) = opened();
    let token = begin(&mut session, Actor::User, Some(id));
    assert!(
        session
            .begin_instance_reference(Actor::Agent, None)
            .is_err()
    );
    assert!(
        session
            .cancel_instance_reference(Actor::Agent, token)
            .is_err()
    );
    let reviewed = session
        .review_instance_reference(Actor::Agent, token)
        .unwrap()["token"]
        .as_u64()
        .unwrap();
    assert_ne!(reviewed, token);
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    session
        .cancel_instance_reference(Actor::Agent, reviewed)
        .unwrap();
    let token = begin(&mut session, Actor::User, Some(id));
    session.save(Actor::User, None).unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    session
        .cancel_instance_reference(Actor::User, token)
        .unwrap();
    let token = begin(&mut session, Actor::User, Some(id));
    let disk = std::fs::read(&parent).unwrap();
    std::fs::write(&parent, [disk.as_slice(), b"\n"].concat()).unwrap();
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    session
        .cancel_instance_reference(Actor::User, token)
        .unwrap();
    let token = begin(&mut session, Actor::User, Some(id));
    session
        .instance_reference_file(Actor::User, token, &child)
        .unwrap();
    let bytes = std::fs::read(&child).unwrap();
    std::fs::write(&child, [bytes.as_slice(), b"\n"].concat()).unwrap();
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    session
        .cancel_instance_reference(Actor::User, token)
        .unwrap();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let draft = session.property_draft().unwrap().token;
    assert!(session.begin_instance_reference(Actor::User, None).is_err());
    assert_eq!(session.property_draft().unwrap().token, draft);
}

#[test]
fn instance_reference_invalid_imported_data_and_missing_source_are_never_repaired() {
    let (_directory, mut session, _parent, _child, id) = opened();
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[
                ("file".into(), Some("missing.vmf".into())),
                ("replace01".into(), Some("$1name 255 220 180 400".into())),
            ],
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let token = begin(&mut session, Actor::User, Some(id));
    let report = session.get_instance_reference();
    assert_eq!(report["valid"], false);
    assert!(report["error"].as_str().unwrap().contains("replace01"));
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    session
        .cancel_instance_reference(Actor::User, token)
        .unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    session
        .edit(Actor::Agent, "import invalid transform", |tx| {
            let entity = tx.entity_mut(id)?;
            entity.properties.remove("replace01");
            entity.properties.set("file", "child.vmf");
            entity.properties.set("angles", "bad rotation");
            entity
                .editor
                .get_or_insert_with(Default::default)
                .set("color", "40 80 120");
            Ok(())
        })
        .unwrap();
    let token = begin(&mut session, Actor::User, Some(id));
    let report = session.get_instance_reference();
    assert_eq!(report["angles"], "bad rotation");
    assert!(report["angle_components"].is_null());
    assert_eq!(report["valid"], false);
    let source = session.document.entity(id).unwrap().clone();
    session
        .update_instance_reference(
            Actor::User,
            token,
            Changes {
                angles: Some("0 90 0".into()),
                ..Default::default()
            },
        )
        .unwrap();
    session
        .apply_instance_reference(Actor::User, token)
        .unwrap();
    let mut expected = source;
    expected.properties.set("angles", "0 90 0");
    assert_eq!(session.document.entity(id).unwrap(), &expected);
    let (_directory, game) = crate::test_assets::game();
    let mut unsaved = Session::new(Ok(game));
    let token = begin(&mut unsaved, Actor::Agent, None);
    assert_eq!(unsaved.get_instance_reference()["source_admitted"], false);
    assert!(
        unsaved
            .apply_instance_reference(Actor::Agent, token)
            .is_err()
    );
    assert!(unsaved.document.undo_labels().next().is_none());
}

#[test]
fn instance_reference_late_edits_and_competing_drafts_reject_without_clearing_them() {
    let (_directory, mut session, _parent, child, id) = opened();
    let token = begin(&mut session, Actor::User, Some(id));
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("targetname".into(), Some("changed_by_agent".into()))],
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    assert!(
        session
            .instance_reference_file(Actor::User, token, &child)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    session.undo(Actor::Agent).unwrap();
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    session
        .cancel_instance_reference(Actor::User, token)
        .unwrap();
    let token = begin(&mut session, Actor::User, Some(id));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let draft = session.property_draft().unwrap().token;
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    assert!(
        session
            .apply_instance_reference(Actor::User, token)
            .is_err()
    );
    assert!(
        session
            .instance_reference_file(Actor::User, token, &child)
            .is_err()
    );
    assert_eq!(session.property_draft().unwrap().token, draft);
    session
        .cancel_instance_reference(Actor::User, token)
        .unwrap();
    assert_eq!(session.property_draft().unwrap().token, draft);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn instance_reference_creation_uses_fgd_defaults_and_definition_changes_stale_the_form() {
    let (_directory, mut session, _parent, _child, _id) = opened();
    let definitions=Catalog::parse(r#"@PointClass = func_instance [ file(instance_file) : "VMF Filename" angles(angle) : "Rotation" : "0 0 0" fixup_style(integer) : "Fixup" : 2 targetname(string) : "Fix Up Name" : "assembly" ]"#).unwrap();
    session.set_definitions(Actor::User, definitions.clone());
    let token = begin(&mut session, Actor::Agent, None);
    session
        .update_instance_reference(
            Actor::Agent,
            token,
            Changes {
                file: Some("child.vmf".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let result = session
        .apply_instance_reference(Actor::Agent, token)
        .unwrap();
    let ObjectId::Entity(id) = crate::identity::parse_id(result["id"].as_str().unwrap()).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        session
            .document
            .entity(id)
            .unwrap()
            .properties
            .get("fixup_style"),
        Some("2")
    );
    assert_eq!(
        session
            .document
            .entity(id)
            .unwrap()
            .properties
            .get("targetname"),
        Some("assembly")
    );
    let token = begin(&mut session, Actor::Agent, Some(id));
    session.set_definitions(Actor::User, definitions);
    assert!(
        session
            .apply_instance_reference(Actor::Agent, token)
            .is_err()
    );
    session
        .cancel_instance_reference(Actor::Agent, token)
        .unwrap();
}

#[test]
fn forwarding_templates_share_property_authoring_and_editable_reference_forms_but_need_parent_scope()
 {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, parent, child, _) = opened();
        let ObjectId::Entity(id) = session
            .create_entity(
                actor,
                "func_instance",
                Some(DVec3::ZERO),
                BTreeMap::from([
                    ("file".into(), "child.vmf".into()),
                    ("replace01".into(), "$Lamp $Outer".into()),
                ]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        session
            .set_entity_properties(
                actor,
                id,
                &[("replace01".into(), Some("$Lamp $Other".into()))],
            )
            .unwrap();
        let before = session.document.vmf().clone();
        session
            .begin_property_edit(PropertyTarget::Entities(vec![id]))
            .unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .update_property_edit(
                token,
                vec![PropertyChange::Set {
                    key: "replace01".into(),
                    value: "$Lamp $Lamp".into(),
                }],
            )
            .unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.commit_property_edit(actor, token).unwrap();
        let authored = session.document.vmf().clone();
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document.vmf(), &authored);
        session.save(actor, None).unwrap();
        session.open(actor, &parent, false).unwrap();
        assert_eq!(
            session
                .document
                .entity(id)
                .unwrap()
                .properties
                .get("replace01"),
            Some("$Lamp $Lamp")
        );
        let token = begin(&mut session, actor, Some(id));
        assert_eq!(session.get_instance_reference()["valid"], true);
        assert_eq!(
            session.get_instance_reference()["contents_validation"],
            "not_checked"
        );
        session
            .update_instance_reference(
                actor,
                token,
                Changes {
                    origin: Some("16 8 0".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        session.apply_instance_reference(actor, token).unwrap();
        assert_eq!(
            session
                .document
                .entity(id)
                .unwrap()
                .properties
                .get("replace01"),
            Some("$Lamp $Lamp")
        );
        let source = session.document.vmf().clone();
        let history = session.history_snapshot();
        let disk = std::fs::read(&parent).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let error = match session.begin_instance_contents_refresh(actor) {
            Ok(_) => panic!("Standalone forwarding reached the worker"),
            Err(error) => error,
        };
        assert!(error.contains("parent binding scope"), "{error}");
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        let result = result.as_ref().unwrap();
        assert!(result.output.bsp.is_none() && !result.published());
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(std::fs::read(&parent).unwrap(), disk);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        assert_eq!(
            std::fs::read(parent.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(parent.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        session
            .set_entity_properties(
                actor,
                id,
                &[("replace01".into(), Some("$Lamp prefix$Lamp".into()))],
            )
            .unwrap();
        let token = begin(&mut session, actor, Some(id));
        assert_eq!(session.get_instance_reference()["valid"], false);
        assert!(session.apply_instance_reference(actor, token).is_err());
        session.cancel_instance_reference(actor, token).unwrap();
    }
}
