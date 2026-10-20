use super::*;

fn begin(session: &mut Session) -> u64 {
    session.begin_property_edit(PropertyTarget::World).unwrap();
    session.property_draft().unwrap().token
}

fn set(value: &str) -> PropertyChange {
    PropertyChange::Set {
        key: compiler::WATER_CURRENT_VELOCITY_KEY.into(),
        value: value.into(),
    }
}

#[test]
fn water_current_world_field_is_discoverable_without_source_mutation_and_respects_fgd_readonly() {
    let key = compiler::WATER_CURRENT_VELOCITY_KEY;
    for definitions in [
        None,
        Some(format!(
            "@SolidClass = worldspawn [ {key}(vector) readonly : \"Custom label\" : \"5 0 0\" ]"
        )),
    ] {
        let mut session = Session::new(Err("property editing needs no assets".into()));
        if let Some(definitions) = definitions {
            session.set_definitions(
                Actor::User,
                source_formats::fgd::Catalog::parse(&definitions).unwrap(),
            );
        }
        let before = (session.document().vmf().clone(), session.history_snapshot());
        let preview = session.inspect_properties(PropertyTarget::World).unwrap();
        let property = preview
            .properties
            .iter()
            .find(|property| property.field.key == key)
            .unwrap();
        assert_eq!(property.field.label, "Water current · world units/s");
        assert_eq!(property.field.kind, "vector");
        assert_eq!(property.field.control, inspector::Control::Vector);
        assert_eq!(property.field.components, ["0", "0", "0"]);
        assert_eq!(property.field.component_labels, ["X", "Y", "Z"]);
        assert!(!property.field.advanced);
        assert_eq!(property.field.value, "0 0 0");
        assert_eq!(property.stored_count, 0);
        assert_eq!(property.values[0].stored, None);
        assert_eq!(property.values[0].effective, "0 0 0");
        assert!(property.field.description.contains("Absent means 0 0 0"));
        assert!(
            property
                .field
                .description
                .contains("Water pressure damping is a separate World setting")
        );
        if property.field.read_only {
            let token = begin(&mut session);
            assert!(
                session
                    .update_property_edit(token, vec![set("32 0 0")])
                    .unwrap_err()
                    .contains("read-only")
            );
            session.cancel_property_edit(token).unwrap();
        } else {
            assert!(!property.can_reset);
            assert_eq!(property.values[0].default, None);
        }
        assert_eq!(
            (session.document().vmf().clone(), session.history_snapshot()),
            before
        );
    }
}

#[test]
fn water_current_drafts_validate_atomically_share_user_agent_undo_and_preserve_saved_values() {
    let key = compiler::WATER_CURRENT_VELOCITY_KEY;
    let mut actor_outputs = Vec::new();
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("property editing needs no assets".into()));
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let selection = session.selected_ids();
        let token = begin(&mut session);
        session
            .append_property_change(
                token,
                PropertyChange::Component {
                    key: key.into(),
                    index: 0,
                    value: 64.0,
                },
            )
            .unwrap();
        let preview = session.preview_property_edit().unwrap();
        let field = &preview
            .properties
            .iter()
            .find(|property| property.field.key == key)
            .unwrap()
            .field;
        assert_eq!(field.components, ["64", "0", "0"]);
        let draft = serde_json::to_value(session.property_draft()).unwrap();
        let oversized = "0".repeat(129);
        for value in [
            "",
            "1 2",
            "1 2 3 4",
            "NaN 0 0",
            "129 0 0",
            "100 100 0",
            "1e-9999 0 0",
            "1e-46 0 0",
            oversized.as_str(),
        ] {
            let error = session
                .update_property_edit(
                    token,
                    vec![
                        PropertyChange::Set {
                            key: "skyname".into(),
                            value: "not_committed".into(),
                        },
                        set(value),
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
        for (index, value) in [(3, 1.0), (1, f64::NAN), (1, 120.0)] {
            assert!(
                session
                    .append_property_change(
                        token,
                        PropertyChange::Component {
                            key: key.into(),
                            index,
                            value
                        }
                    )
                    .is_err()
            );
            assert_eq!(
                serde_json::to_value(session.property_draft()).unwrap(),
                draft
            );
        }
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        let token = begin(&mut session);
        session
            .update_property_edit(token, vec![set("6.4e1 -16 0")])
            .unwrap();
        session.commit_property_edit(actor, token).unwrap();
        assert_eq!(
            session.document().vmf().world.properties.get(key),
            Some("6.4e1 -16 0")
        );
        assert_eq!(session.document().undo_labels().count(), 1);
        let authored = session.document().vmf().clone();
        actor_outputs.push(authored.clone());
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &authored);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("water-current.vmf");
        session.save(actor, Some(&path)).unwrap();
        let mut reopened = Session::new(Err("property editing needs no assets".into()));
        reopened.open(actor, &path, false).unwrap();
        assert_eq!(
            reopened.document().vmf().world.properties.get(key),
            Some("6.4e1 -16 0")
        );
        let token = begin(&mut reopened);
        reopened
            .append_property_change(token, PropertyChange::Remove { key: key.into() })
            .unwrap();
        reopened.commit_property_edit(actor, token).unwrap();
        assert_eq!(reopened.document().vmf().world.properties.get(key), None);
        assert_eq!(
            reopened
                .inspect_properties(PropertyTarget::World)
                .unwrap()
                .properties
                .iter()
                .find(|property| property.field.key == key)
                .unwrap()
                .field
                .value,
            "0 0 0"
        );
        let token = begin(&mut reopened);
        reopened
            .update_property_edit(token, vec![set("0 0 0")])
            .unwrap();
        reopened.commit_property_edit(actor, token).unwrap();
        let preview = reopened.inspect_properties(PropertyTarget::World).unwrap();
        let explicit_zero = preview
            .properties
            .iter()
            .find(|property| property.field.key == key)
            .unwrap();
        assert_eq!(explicit_zero.stored_count, 1);
        assert_eq!(explicit_zero.values[0].stored.as_deref(), Some("0 0 0"));
        reopened.undo(actor).unwrap();
        assert_eq!(reopened.document().vmf().world.properties.get(key), None);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), selection);
    }
    assert_eq!(actor_outputs[0], actor_outputs[1]);
}

#[test]
fn water_current_source_duplicates_and_invalid_stored_values_remain_explicit() {
    let key = compiler::WATER_CURRENT_VELOCITY_KEY;
    let mut vmf = Session::new(Err("property editing needs no assets".into()))
        .document()
        .vmf()
        .clone();
    vmf.world.properties.0.push((key.into(), "32 0 0".into()));
    vmf.world
        .properties
        .0
        .push((key.to_ascii_uppercase(), "16 0 0".into()));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("duplicate.vmf");
    std::fs::write(&path, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Err("property editing needs no assets".into()));
    session.open(Actor::User, &path, false).unwrap();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        std::fs::read(&path).unwrap(),
    );
    let token = begin(&mut session);
    assert!(
        session
            .update_property_edit(token, vec![set("0 0 0")])
            .unwrap_err()
            .contains("duplicate")
    );
    session.cancel_property_edit(token).unwrap();
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            std::fs::read(&path).unwrap()
        ),
        before
    );
    for value in ["malformed", "200 0 0", "1e-9999 0 0"] {
        let mut world = Entity::new(1, "worldspawn");
        world.properties.set(key, value);
        let inspected = inspector::inspect(&world, None);
        let field = inspected
            .properties
            .iter()
            .find(|field| field.key == key)
            .unwrap();
        assert_eq!(field.value, value);
        assert_eq!(field.control, inspector::Control::Text);
        assert_eq!(field.components.len(), 0);
        assert!(field.description.contains("Stored current is invalid:"));
    }
}
