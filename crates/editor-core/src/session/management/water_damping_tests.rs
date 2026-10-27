use super::*;

fn begin(session: &mut Session) -> u64 {
    session.begin_property_edit(PropertyTarget::World).unwrap();
    session.property_draft().unwrap().token
}

fn set(value: &str) -> PropertyChange {
    PropertyChange::Set {
        key: compiler::WATER_DAMPING_KEY.into(),
        value: value.into(),
    }
}

fn damping(preview: &PropertyPreview) -> &ManagedProperty {
    preview
        .properties
        .iter()
        .find(|property| {
            property
                .field
                .key
                .eq_ignore_ascii_case(compiler::WATER_DAMPING_KEY)
        })
        .unwrap()
}

#[test]
fn water_damping_world_discovery_preserves_absence_fgd_defaults_and_readonly() {
    let key = compiler::WATER_DAMPING_KEY;
    for kind in [None, Some("float"), Some("vector")] {
        let mut session = Session::new(Err("property editing needs no assets".into()));
        if let Some(kind) = kind {
            session.set_definitions(
                Actor::User,
                source_formats::fgd::Catalog::parse(&format!(
                    "@SolidClass = worldspawn [ {key}({kind}) : \"Custom label\" : \"0.005\" ]"
                ))
                .unwrap(),
            );
        }
        let before = (session.document().vmf().clone(), session.history_snapshot());
        let preview = session.inspect_properties(PropertyTarget::World).unwrap();
        let property = damping(&preview);
        assert_eq!(property.field.label, "Liquid pressure damping");
        assert_eq!(property.field.kind, "float");
        assert_eq!(property.field.control, inspector::Control::Number);
        assert_eq!(property.field.range, Some([0.0, 0.01]));
        assert!(!property.field.advanced);
        assert_eq!(property.field.value, "0.01");
        assert!(property.field.components.is_empty());
        assert!(property.field.description.contains("Absent means 0.01"));
        assert!(
            property
                .field
                .description
                .contains("fixed pressure term remains")
        );
        assert_eq!(property.stored_count, 0);
        assert_eq!(property.values[0].stored, None);
        assert_eq!(property.values[0].effective, "0.01");
        assert_eq!(property.values[0].default.as_deref(), kind.map(|_| "0.005"));
        assert_eq!(property.can_reset, kind.is_some());
        assert_eq!(
            (session.document().vmf().clone(), session.history_snapshot()),
            before
        );
        let token = begin(&mut session);
        if kind.is_some() {
            session
                .append_property_change(token, PropertyChange::Default { key: key.into() })
                .unwrap();
            assert_eq!(
                damping(&session.preview_property_edit().unwrap())
                    .field
                    .value,
                "0.005"
            );
        } else {
            assert!(
                session
                    .append_property_change(token, PropertyChange::Default { key: key.into() })
                    .unwrap_err()
                    .contains("no FGD default")
            );
        }
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &before.0);
        if kind.is_some() {
            let token = begin(&mut session);
            session
                .update_property_edit(token, vec![set(" 5e-3 ")])
                .unwrap();
            let preview = session.preview_property_edit().unwrap();
            assert_eq!(damping(&preview).field.control, inspector::Control::Number);
            assert_eq!(damping(&preview).field.value, " 5e-3 ");
            session.cancel_property_edit(token).unwrap();
        }
    }
    let mut session = Session::new(Err("property editing needs no assets".into()));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(&format!(
            "@SolidClass = worldspawn [ {key}(float) readonly : \"Locked\" : \"0.005\" ]"
        ))
        .unwrap(),
    );
    let preview = session.inspect_properties(PropertyTarget::World).unwrap();
    assert!(damping(&preview).field.read_only);
    let before = (session.document().vmf().clone(), session.history_snapshot());
    let token = begin(&mut session);
    for change in [
        set("0.005"),
        PropertyChange::Default { key: key.into() },
        PropertyChange::Remove { key: key.into() },
    ] {
        assert!(
            session
                .update_property_edit(token, vec![change])
                .unwrap_err()
                .contains("read-only")
        );
        assert!(session.property_draft().unwrap().changes.is_empty());
    }
    session.cancel_property_edit(token).unwrap();
    assert_eq!(
        (session.document().vmf().clone(), session.history_snapshot()),
        before
    );
}

#[test]
fn water_damping_drafts_are_atomic_and_share_actor_history_and_exact_saved_text() {
    let key = compiler::WATER_DAMPING_KEY;
    let raw = " \t5e-3  ";
    let mut actor_outputs = Vec::new();
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("property editing needs no assets".into()));
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let selection = session.selected_ids();
        let token = begin(&mut session);
        session.append_property_change(token, set(raw)).unwrap();
        let preview = session.preview_property_edit().unwrap();
        assert_eq!(damping(&preview).field.control, inspector::Control::Number);
        assert_eq!(damping(&preview).field.value, raw);
        assert_eq!(damping(&preview).values[0].stored.as_deref(), Some(raw));
        assert_eq!(damping(&preview).values[0].effective, raw);
        let draft = serde_json::to_value(session.property_draft()).unwrap();
        let oversized = "0".repeat(compiler::MAX_WATER_DAMPING_VALUE_BYTES + 1);
        for value in [
            "",
            "0 0",
            "NaN",
            "inf",
            "-0.001",
            "0.010000000000000002",
            "1e-9999",
            "1e-46",
            "0x0",
            "0.005suffix",
            "0,005",
            "\u{a0}0.005",
            oversized.as_str(),
        ] {
            let error = session
                .update_property_edit(
                    token,
                    vec![
                        PropertyChange::Set {
                            key: "skyname".into(),
                            value: "must_not_apply".into(),
                        },
                        PropertyChange::Set {
                            key: key.to_ascii_uppercase(),
                            value: value.into(),
                        },
                    ],
                )
                .unwrap_err();
            assert!(error.contains(key), "{error}");
            assert_eq!(
                serde_json::to_value(session.property_draft()).unwrap(),
                draft
            );
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
        }
        assert!(
            session
                .append_property_change(
                    token,
                    PropertyChange::Component {
                        key: key.into(),
                        index: 0,
                        value: 0.005
                    }
                )
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(session.property_draft()).unwrap(),
            draft
        );
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        let token = begin(&mut session);
        session.update_property_edit(token, vec![set(raw)]).unwrap();
        session.commit_property_edit(actor, token).unwrap();
        assert_eq!(session.document().undo_labels().count(), 1);
        assert_eq!(
            session.document().vmf().world.properties.get(key),
            Some(raw)
        );
        let authored = session.document().vmf().clone();
        actor_outputs.push(authored.clone());
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &authored);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("water-damping.vmf");
        session.save(actor, Some(&path)).unwrap();
        let mut reopened = Session::new(Err("property editing needs no assets".into()));
        reopened.open(actor, &path, false).unwrap();
        assert_eq!(
            reopened.document().vmf().world.properties.get(key),
            Some(raw)
        );
        let preview = reopened.inspect_properties(PropertyTarget::World).unwrap();
        assert_eq!(damping(&preview).field.control, inspector::Control::Number);
        assert_eq!(damping(&preview).values[0].effective, raw);
        let token = begin(&mut reopened);
        reopened
            .append_property_change(token, PropertyChange::Remove { key: key.into() })
            .unwrap();
        reopened.commit_property_edit(actor, token).unwrap();
        let preview = reopened.inspect_properties(PropertyTarget::World).unwrap();
        assert_eq!(damping(&preview).field.value, "0.01");
        assert_eq!(damping(&preview).stored_count, 0);
        assert_eq!(damping(&preview).values[0].stored, None);
        for value in ["0", "0.01"] {
            let token = begin(&mut reopened);
            reopened
                .update_property_edit(token, vec![set(value)])
                .unwrap();
            reopened.commit_property_edit(actor, token).unwrap();
            let preview = reopened.inspect_properties(PropertyTarget::World).unwrap();
            assert_eq!(damping(&preview).stored_count, 1);
            assert_eq!(damping(&preview).values[0].stored.as_deref(), Some(value));
            assert_eq!(damping(&preview).values[0].default, None);
            reopened.undo(actor).unwrap();
            assert_eq!(reopened.document().vmf().world.properties.get(key), None);
            reopened.redo(actor).unwrap();
            assert_eq!(
                reopened.document().vmf().world.properties.get(key),
                Some(value)
            );
            reopened.undo(actor).unwrap();
        }
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), selection);
    }
    assert_eq!(actor_outputs[0], actor_outputs[1]);
}

#[test]
fn water_damping_default_writes_fgd_value_and_remove_restores_intrinsic_value() {
    let key = compiler::WATER_DAMPING_KEY;
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("property editing needs no assets".into()));
        session.set_definitions(
            actor,
            source_formats::fgd::Catalog::parse(&format!(
                "@SolidClass = worldspawn [ {key}(float) : \"Damping\" : \"0.005\" ]"
            ))
            .unwrap(),
        );
        let token = begin(&mut session);
        session
            .append_property_change(token, PropertyChange::Default { key: key.into() })
            .unwrap();
        session.commit_property_edit(actor, token).unwrap();
        assert_eq!(
            session.document().vmf().world.properties.get(key),
            Some("0.005")
        );
        let token = begin(&mut session);
        session
            .append_property_change(token, PropertyChange::Remove { key: key.into() })
            .unwrap();
        session.commit_property_edit(actor, token).unwrap();
        let preview = session.inspect_properties(PropertyTarget::World).unwrap();
        assert_eq!(damping(&preview).field.value, "0.01");
        assert_eq!(damping(&preview).values[0].stored, None);
        assert_eq!(
            damping(&preview).values[0].default.as_deref(),
            Some("0.005")
        );
        assert!(damping(&preview).can_reset);
        session.undo(actor).unwrap();
        assert_eq!(
            session.document().vmf().world.properties.get(key),
            Some("0.005")
        );
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf().world.properties.get(key), None);
    }
    let mut session = Session::new(Err("property editing needs no assets".into()));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(&format!(
            "@SolidClass = worldspawn [ {key}(float) : \"Damping\" : \"0.02\" ]"
        ))
        .unwrap(),
    );
    let token = begin(&mut session);
    session.append_property_change(token, set("0.005")).unwrap();
    let draft = serde_json::to_value(session.property_draft()).unwrap();
    assert!(
        session
            .append_property_change(token, PropertyChange::Default { key: key.into() })
            .unwrap_err()
            .contains(key)
    );
    assert_eq!(
        serde_json::to_value(session.property_draft()).unwrap(),
        draft
    );
}

#[test]
fn water_damping_validation_override_is_world_only_and_retains_keyvalues_guards() {
    let key = compiler::WATER_DAMPING_KEY;
    let mut session = Session::new(Err("property editing needs no assets".into()));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(&format!(
            "@PointClass = fixture [ {key}(float) : \"Ordinary float\" ]"
        ))
        .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(Actor::User, "fixture", None, Default::default())
        .unwrap()
    else {
        panic!("expected entity");
    };
    session
        .set_entity_properties(Actor::Agent, id, &[(key.into(), Some("0.02".into()))])
        .unwrap();
    let before = (session.document().vmf().clone(), session.history_snapshot());
    assert!(
        session
            .set_entity_properties(Actor::Agent, id, &[(key.into(), Some(" 5e-3 ".into()))])
            .is_err()
    );
    assert_eq!(
        (session.document().vmf().clone(), session.history_snapshot()),
        before
    );
    let token = begin(&mut session);
    let error = session
        .update_property_edit(token, vec![set("\"0.005\"")])
        .unwrap_err();
    assert!(error.contains(key));
    assert!(session.property_draft().unwrap().changes.is_empty());
    session.cancel_property_edit(token).unwrap();
}

#[test]
fn water_damping_imported_invalid_values_and_duplicates_remain_visible_and_unchanged() {
    let key = compiler::WATER_DAMPING_KEY;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("imported.vmf");
    for raw in ["malformed", "0.02", "1e-9999", "1e-46", "\u{a0}0.005"] {
        let mut vmf = Session::new(Err("property editing needs no assets".into()))
            .document()
            .vmf()
            .clone();
        vmf.world.properties.set(key.to_ascii_uppercase(), raw);
        std::fs::write(&path, vmf.write(&Default::default())).unwrap();
        let mut session = Session::new(Err("property editing needs no assets".into()));
        session.open(Actor::User, &path, false).unwrap();
        let before = (session.document().vmf().clone(), session.history_snapshot());
        let preview = session.inspect_properties(PropertyTarget::World).unwrap();
        let property = damping(&preview);
        assert_eq!(property.field.value, raw);
        assert_eq!(property.field.control, inspector::Control::Text);
        assert!(
            property
                .field
                .description
                .contains("Stored damping is invalid:")
        );
        assert_eq!(property.values[0].stored.as_deref(), Some(raw));
        assert_eq!(property.values[0].effective, raw);
        assert_eq!(property.stored_count, 1);
        assert_eq!(
            (session.document().vmf().clone(), session.history_snapshot()),
            before
        );
        let token = begin(&mut session);
        session.append_property_change(token, set("0.005")).unwrap();
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &before.0);
        session.save(Actor::User, Some(&path)).unwrap();
        let saved =
            source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.world.properties.get(key), Some(raw));
    }
    let mut vmf = Session::new(Err("property editing needs no assets".into()))
        .document()
        .vmf()
        .clone();
    vmf.world.properties.0.push((key.into(), "0.005".into()));
    vmf.world
        .properties
        .0
        .push((key.to_ascii_uppercase(), "0.01".into()));
    std::fs::write(&path, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Err("property editing needs no assets".into()));
    session.open(Actor::User, &path, false).unwrap();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        std::fs::read(&path).unwrap(),
    );
    let preview = session.inspect_properties(PropertyTarget::World).unwrap();
    assert_eq!(damping(&preview).field.control, inspector::Control::Text);
    assert!(
        damping(&preview)
            .field
            .description
            .contains("duplicate damping keys")
    );
    let token = begin(&mut session);
    for change in [set("0.005"), PropertyChange::Remove { key: key.into() }] {
        assert!(
            session
                .update_property_edit(token, vec![change])
                .unwrap_err()
                .contains("duplicate")
        );
    }
    session.cancel_property_edit(token).unwrap();
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            std::fs::read(&path).unwrap()
        ),
        before
    );
}
