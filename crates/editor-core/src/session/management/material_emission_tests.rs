use super::*;

const KEY: &str = compiler::MATERIAL_EMISSION_FILE_KEY;
const RAW: &str = "  lighting/room.rad  ";

fn begin(session: &mut Session) -> u64 {
    session.begin_property_edit(PropertyTarget::World).unwrap();
    session.property_draft().unwrap().token
}

fn set(value: &str) -> PropertyChange {
    PropertyChange::Set {
        key: KEY.into(),
        value: value.into(),
    }
}

fn emission(preview: &PropertyPreview) -> &ManagedProperty {
    preview
        .properties
        .iter()
        .find(|property| property.field.key == KEY)
        .unwrap()
}

#[test]
fn material_emission_is_intrinsic_without_fgd_or_with_conflicting_fgd_metadata() {
    for definition in [
        None,
        Some(format!(
            "@SolidClass = worldspawn [ {KEY}(vector) readonly : \"Locked\" : \"1 2 3\" ]"
        )),
        Some(format!(
            "@SolidClass = worldspawn [ {}(integer) : \"Other\" : 42 ]",
            KEY.to_ascii_uppercase()
        )),
    ] {
        let mut session = Session::new(Err("authoring needs no assets".into()));
        if let Some(definition) = definition {
            session.set_definitions(
                Actor::User,
                source_formats::fgd::Catalog::parse(&definition).unwrap(),
            );
        }
        let source = session.document().vmf().clone();
        let preview = session.inspect_properties(PropertyTarget::World).unwrap();
        let property = emission(&preview);
        assert_eq!(property.field.label, "Emission file");
        assert_eq!(property.field.kind, "string");
        assert_eq!(property.field.control, inspector::Control::Text);
        assert!(!property.field.advanced);
        assert!(!property.field.read_only);
        assert_eq!(property.field.value, "");
        assert_eq!(property.stored_count, 0);
        assert_eq!(property.values[0].stored, None);
        assert_eq!(property.values[0].default, None);
        assert_eq!(property.values[0].effective, "");
        assert!(!property.can_reset);
        let token = begin(&mut session);
        session.update_property_edit(token, vec![set(RAW)]).unwrap();
        assert_eq!(
            emission(&session.preview_property_edit().unwrap())
                .field
                .value,
            RAW
        );
        for change in [
            PropertyChange::Default { key: KEY.into() },
            PropertyChange::Component {
                key: KEY.into(),
                index: 0,
                value: 1.0,
            },
            PropertyChange::Flag {
                key: KEY.into(),
                flag: 1,
                enabled: true,
            },
        ] {
            assert!(session.update_property_edit(token, vec![change]).is_err());
        }
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
}

#[test]
fn material_emission_actor_drafts_are_atomic_cancelable_undoable_and_persistent() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("authoring needs no assets".into()));
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let token = begin(&mut session);
        session.update_property_edit(token, vec![set(RAW)]).unwrap();
        let staged = session.property_edit_report().unwrap();
        let oversized = "x".repeat(compiler::MAX_MATERIAL_EMISSION_FILE_PATH_BYTES + 1);
        for value in [
            "../room.rad",
            "/room.rad",
            "a//room.rad",
            "a\\room.rad",
            "room/é.rad",
            oversized.as_str(),
        ] {
            assert!(
                session
                    .update_property_edit(
                        token,
                        vec![
                            PropertyChange::Set {
                                key: "skyname".into(),
                                value: "must_not_apply".into()
                            },
                            set(value),
                        ]
                    )
                    .is_err()
            );
            assert_eq!(session.property_edit_report().unwrap(), staged);
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
        }
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        let token = begin(&mut session);
        session.update_property_edit(token, vec![set(RAW)]).unwrap();
        session.commit_property_edit(actor, token).unwrap();
        let authored = session.document().vmf().clone();
        assert_eq!(authored.world.properties.get(KEY), Some(RAW));
        assert_eq!(session.document().undo_labels().count(), 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &authored);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("emission.vmf");
        session.save(actor, Some(&path)).unwrap();
        let mut reopened = Session::new(Err("authoring needs no assets".into()));
        reopened.open(actor, &path, false).unwrap();
        let preview = reopened.inspect_properties(PropertyTarget::World).unwrap();
        assert_eq!(emission(&preview).values[0].stored.as_deref(), Some(RAW));
        for change in [set(""), PropertyChange::Remove { key: KEY.into() }] {
            let token = begin(&mut reopened);
            reopened.update_property_edit(token, vec![change]).unwrap();
            reopened.commit_property_edit(actor, token).unwrap();
            let preview = reopened.inspect_properties(PropertyTarget::World).unwrap();
            assert_eq!(emission(&preview).field.value, "");
            reopened.undo(actor).unwrap();
            assert_eq!(
                reopened.document().vmf().world.properties.get(KEY),
                Some(RAW)
            );
        }
        let token = begin(&mut reopened);
        reopened
            .update_property_edit(token, vec![set("lighting/other.rad")])
            .unwrap();
        reopened.set_definitions(actor, source_formats::fgd::Catalog::default());
        assert!(reopened.commit_property_edit(actor, token).is_err());
        assert_eq!(
            reopened.document().vmf().world.properties.get(KEY),
            Some(RAW)
        );
        reopened.cancel_property_edit(token).unwrap();
    }
}

#[test]
fn material_emission_malformed_values_and_noncanonical_duplicates_remain_inspectable() {
    let aliases = [
        KEY.to_ascii_uppercase(),
        format!("{KEY}#2"),
        format!("{KEY} "),
    ];
    for alias in aliases {
        for duplicate in [false, true] {
            let mut session = Session::new(Err("authoring needs no assets".into()));
            session
                .edit(Actor::User, "import malformed World settings", |tx| {
                    let properties = tx.world_properties_mut();
                    properties.0.push((alias.clone(), "../bad.rad".into()));
                    if duplicate {
                        properties.0.push((KEY.into(), RAW.into()));
                    }
                    Ok(())
                })
                .unwrap();
            let source = session.document().vmf().clone();
            let preview = session.inspect_properties(PropertyTarget::World).unwrap();
            let stored = preview
                .properties
                .iter()
                .find(|property| property.field.key == alias)
                .unwrap();
            assert_eq!(stored.field.value, "../bad.rad");
            assert_eq!(stored.values[0].stored.as_deref(), Some("../bad.rad"));
            assert!(stored.field.description.contains("invalid"));
            assert!(stored.field.description.contains("canonical"));
            assert_eq!(stored.field.control, inspector::Control::Text);
            let token = begin(&mut session);
            for key in [alias.as_str(), KEY] {
                assert!(
                    session
                        .update_property_edit(
                            token,
                            vec![PropertyChange::Set {
                                key: key.into(),
                                value: "lighting/fixed.rad".into(),
                            }]
                        )
                        .unwrap_err()
                        .contains("canonical")
                );
            }
            assert_eq!(session.document().vmf(), &source);
        }
    }
    let mut session = Session::new(Err("authoring needs no assets".into()));
    session
        .edit(Actor::User, "import duplicate emission paths", |tx| {
            tx.world_properties_mut()
                .0
                .extend([(KEY.into(), RAW.into()), (KEY.into(), "../bad.rad".into())]);
            Ok(())
        })
        .unwrap();
    let preview = session.inspect_properties(PropertyTarget::World).unwrap();
    let duplicate_fields = preview
        .properties
        .iter()
        .filter(|property| property.field.key == KEY)
        .collect::<Vec<_>>();
    assert_eq!(duplicate_fields.len(), 2);
    assert_eq!(duplicate_fields[0].field.value, RAW);
    assert_eq!(duplicate_fields[1].field.value, "../bad.rad");
    assert_eq!(
        duplicate_fields[1].values[0].stored.as_deref(),
        Some("../bad.rad")
    );
    assert!(duplicate_fields[1].field.description.contains("invalid"));
    let token = begin(&mut session);
    assert!(
        session
            .update_property_edit(token, vec![set(RAW)])
            .unwrap_err()
            .contains("canonical")
    );
    let mut session = Session::new(Err("authoring needs no assets".into()));
    session
        .edit(Actor::User, "import malformed emission path", |tx| {
            tx.world_properties_mut().set(KEY, "../bad.rad");
            Ok(())
        })
        .unwrap();
    let preview = session.inspect_properties(PropertyTarget::World).unwrap();
    assert_eq!(emission(&preview).field.value, "../bad.rad");
    assert!(emission(&preview).field.description.contains("invalid"));
    let token = begin(&mut session);
    session.update_property_edit(token, vec![set(RAW)]).unwrap();
    session.commit_property_edit(Actor::User, token).unwrap();
    assert_eq!(
        session.document().vmf().world.properties.get(KEY),
        Some(RAW)
    );
}

#[cfg(feature = "mcp")]
mod dispatch {
    use super::*;
    use crate::mcp::EditorServer;
    use rmcp::ServerHandler;
    use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, NumberOrString};
    use rmcp::service::{RequestContext, RunningService, serve_directly};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    async fn call(
        server: &EditorServer,
        running: &RunningService<rmcp::RoleServer, EditorServer>,
        name: &'static str,
        args: Value,
    ) -> CallToolResult {
        let request =
            CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
        match server.call_tool(request, context).await.unwrap() {
            CallToolResponse::Complete(result) => result,
            response => panic!("unexpected tool response: {response:?}"),
        }
    }

    fn value(result: CallToolResult) -> Value {
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
    }

    #[tokio::test]
    async fn material_emission_actual_mcp_dispatch_uses_the_shared_world_draft() {
        let shared = Arc::new(Mutex::new(Session::new(Err(
            "authoring needs no assets".into()
        ))));
        let server = EditorServer::new(shared.clone(), None);
        let (transport, _client) = tokio::io::duplex(4096);
        let running =
            serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
        let inspected = value(
            call(
                &server,
                &running,
                "get_property_inspector",
                json!({"world":true}),
            )
            .await,
        );
        assert_eq!(
            inspected,
            serde_json::to_value(
                shared
                    .lock()
                    .unwrap()
                    .inspect_properties(PropertyTarget::World)
                    .unwrap()
            )
            .unwrap()
        );
        let source = shared.lock().unwrap().document().vmf().clone();
        let opened = value(
            call(
                &server,
                &running,
                "begin_property_edit",
                json!({"world":true}),
            )
            .await,
        );
        let token = opened["draft"]["token"].as_u64().unwrap();
        let staged = value(
            call(
                &server,
                &running,
                "update_property_edit",
                json!({"token":token,"changes":[{"action":"set","key":KEY,"value":RAW}]}),
            )
            .await,
        );
        assert_eq!(
            staged,
            shared.lock().unwrap().property_edit_report().unwrap()
        );
        let invalid = call(
            &server,
            &running,
            "update_property_edit",
            json!({"token":token,"changes":[{"action":"set","key":KEY,"value":"../bad.rad"}]}),
        )
        .await;
        assert_eq!(invalid.is_error, Some(true));
        assert_eq!(
            value(call(&server, &running, "get_property_edit", json!({})).await),
            staged
        );
        assert_eq!(shared.lock().unwrap().document().vmf(), &source);
        value(
            call(
                &server,
                &running,
                "commit_property_edit",
                json!({"token":token}),
            )
            .await,
        );
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .document()
                .vmf()
                .world
                .properties
                .get(KEY),
            Some(RAW)
        );
        assert_eq!(shared.lock().unwrap().document().undo_labels().count(), 1);
        shared.lock().unwrap().undo(Actor::User).unwrap();
        assert_eq!(shared.lock().unwrap().document().vmf(), &source);
        running.cancel().await.unwrap();
    }
}
