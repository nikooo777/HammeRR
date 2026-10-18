use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn color_space_tools_convert_read_only_and_apply_validated_colors_atomically() {
    use crate::color::{Input, Space};

    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(r#"@PointClass = fixture [ tint(color1) : "Tint" : "0.5 0.25 0.75" ]"#)
            .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(Actor::User, "fixture", None, BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let converted = value(server.convert_color(Parameters(Input {
        space: Space::Hsv,
        channels: [240.0, 100.0, 100.0],
    })));
    assert_eq!(converted["rgb255"], json!([0, 0, 255]));
    assert_eq!(converted["hsl"], json!([240.0, 100.0, 50.0]));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    let invalid = server
        .set_entity_color_space(Parameters(EntityColorSpaceArgs {
            id: Some(format!("entity:{id}")),
            key: "tint".into(),
            color: Input {
                space: Space::Hsl,
                channels: [361.0, 100.0, 50.0],
            },
            intensity: None,
        }))
        .unwrap();
    assert!(invalid.is_error.unwrap());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    value(
        server.set_entity_color_space(Parameters(EntityColorSpaceArgs {
            id: Some(format!("entity:{id}")),
            key: "tint".into(),
            color: Input {
                space: Space::Hsl,
                channels: [120.0, 100.0, 50.0],
            },
            intensity: None,
        })),
    );
    assert_eq!(
        lock(&shared)
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("tint"),
        Some("0 1 0")
    );
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[tokio::test]
async fn map_size_report_is_shared_read_only_and_does_not_limit_authored_positions() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root.fgd");
    let base = directory.path().join("base.fgd");
    std::fs::write(
        &base,
        "// Author preference\n@mapsize(64, -64)\n@PointClass = info_target []",
    )
    .unwrap();
    std::fs::write(&root, "@include \"base.fgd\"\n@mapsize(7, 7)").unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(Actor::User, Catalog::default());
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert!(value(server.get_entity_definitions())["map_size"].is_null());
    let loaded = value(
        server
            .load_entity_definitions(Parameters(DefinitionsArgs {
                path: root.display().to_string(),
            }))
            .await,
    );
    assert_eq!(loaded["classes"], 1);
    assert_eq!(loaded["map_size"]["authored"], json!([64, -64]));
    assert_eq!(loaded["map_size"]["bounds"], json!([-64, 64]));
    assert_eq!(
        loaded["map_size"]["source"],
        base.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(loaded["map_size"]["line"], 2);
    assert_eq!(value(server.get_entity_definitions()), loaded);
    let serial = lock(&shared).definitions_serial();
    {
        let session = lock(&shared);
        assert_eq!(json!(session.entity_definitions_report()), loaded);
        assert!(
            session
                .entity_definitions_report()
                .status()
                .contains("-64 to 64")
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
    std::fs::write(&root, "@mapsize(0, 2147483648)").unwrap();
    assert!(
        server
            .load_entity_definitions(Parameters(DefinitionsArgs {
                path: root.display().to_string(),
            }))
            .await
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(value(server.get_entity_definitions()), loaded);
    {
        let session = lock(&shared);
        assert_eq!(session.definitions_serial(), serial);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
    let created = value(server.create_entity(Parameters(CreateEntityArgs {
        classname: "info_target".into(),
        origin: Some([1024.0, -1024.0, 256.0]),
        properties: None,
    })));
    assert!(
        created["properties"]
            .as_array()
            .unwrap()
            .contains(&json!(["origin", "1024 -1024 256"]))
    );
    std::fs::write(&root, "@PointClass = info_target []").unwrap();
    let reloaded = value(
        server
            .load_entity_definitions(Parameters(DefinitionsArgs {
                path: root.display().to_string(),
            }))
            .await,
    );
    assert!(reloaded["map_size"].is_null());
    assert!(
        lock(&shared)
            .entity_definitions_report()
            .status()
            .contains("No effective @mapsize")
    );
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[tokio::test]
async fn extensions_share_the_catalog_and_preserve_map_data_on_reload_and_failure() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base.fgd");
    let extended = directory.path().join("extended.fgd");
    std::fs::write(
        &base,
        r#"
        @BaseClass = Named [ targetname(string) : "Name" ]
        @PointClass base(Named) = logic_relay : "Relay" [
            mode(choices) : "Mode" : 1 = [ 1 : "Original mode" ]
            spawnflags(flags) = [ 1 : "Enabled" : 1 ]
            input Trigger(void) : "Trigger relay"
            output OnTrigger(void) : "Triggered"
        ]
        "#,
    )
    .unwrap();
    std::fs::write(
        &extended,
        r#"
        @ExtendClass = absent [ ignored(string) ]
        @ExtendClass = logic_relay : "Ignored" [
            mode(choices) : "Mode" : 2 = [ 2 : "Extended mode" ]
            new_key(integer) : "New setting" : 7
            input Pulse(float) : "Pulse relay"
            output OnPulse(float) : "Pulsed"
        ]
        @include "base.fgd"
        "#,
    )
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(Actor::User, Catalog::load(&base).unwrap());
    let ObjectId::Entity(entity) = session
        .create_entity(
            Actor::User,
            "logic_relay",
            Some(DVec3::ZERO),
            BTreeMap::from([("custom_key".into(), "Keep me".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    session.begin_connection_edit(entity, None).unwrap();
    session
        .update_connection_draft(ConnectionFields {
            output: "OnTrigger".into(),
            target: "!self".into(),
            input: "Trigger".into(),
            parameter: String::new(),
            delay: "0".into(),
            times: "1".into(),
        })
        .unwrap();
    session.commit_connection_edit(Actor::User).unwrap();
    session
        .begin_property_edit(PropertyTarget::Entities(vec![entity]))
        .unwrap();
    let draft = session.property_draft().unwrap().token;
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let serial = session.definitions_serial();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let loaded = value(
        server
            .load_entity_definitions(Parameters(DefinitionsArgs {
                path: extended.display().to_string(),
            }))
            .await,
    );
    assert_eq!(loaded["classes"], 1);
    assert_eq!(loaded["sources"].as_array().unwrap().len(), 2);
    let class = value(server.get_entity_class(Parameters(EntityClassArgs {
        classname: "LOGIC_RELAY".into(),
    })));
    assert_eq!(class["description"], "Relay");
    assert_eq!(class["defaults"]["mode"], "2");
    assert_eq!(class["defaults"]["new_key"], "7");
    assert_eq!(class["defaults"]["spawnflags"], "1");
    assert_eq!(class["inputs"].as_array().unwrap().len(), 2);
    assert_eq!(class["outputs"].as_array().unwrap().len(), 2);
    {
        let session = lock(&shared);
        assert_eq!(session.definitions_serial(), serial + 1);
        assert!(session.check_property_draft(draft).is_err());
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.connection_records(entity).unwrap().len(), 1);
        assert!(
            session
                .connection_inputs(entity, "!self")
                .iter()
                .any(|input| input.name == "Pulse" && input.kind == "float")
        );
        let inspector = session.entity_inspector(entity).unwrap();
        for (key, expected) in [("new_key", "7"), ("mode", "1"), ("custom_key", "Keep me")] {
            let property = inspector
                .properties
                .iter()
                .find(|field| field.key == key)
                .unwrap();
            assert_eq!(property.value, expected);
        }
    }
    assert!(
        server
            .set_entity_properties(Parameters(SetPropertiesArgs {
                id: Some(format!("entity:{entity}")),
                set: Some(BTreeMap::from([("new_key".into(), "invalid".into())])),
                remove: None,
            }))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    std::fs::write(&extended, "@PointClass base(Missing) = broken []").unwrap();
    assert!(
        server
            .load_entity_definitions(Parameters(DefinitionsArgs {
                path: extended.display().to_string(),
            }))
            .await
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(
        value(server.get_entity_class(Parameters(EntityClassArgs {
            classname: "logic_relay".into(),
        }))),
        class
    );
    let session = lock(&shared);
    assert_eq!(session.definitions_serial(), serial + 1);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
