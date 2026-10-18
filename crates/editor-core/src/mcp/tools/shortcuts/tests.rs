use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn shortcut_tools_share_persistence_and_the_typed_command_dispatcher() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().to_owned()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let initial = value(server.get_keyboard_shortcuts());
    let mut bindings = Bindings::default();
    bindings.frame_selection[0] = Some(crate::shortcuts::Chord {
        key: "F8".into(),
        control: false,
        shift: false,
    });
    let saved = value(
        server.set_keyboard_shortcuts(Parameters(ShortcutsArgs::Save {
            snapshot: initial["snapshot"].as_str().unwrap().into(),
            bindings: Box::new(bindings.clone()),
        })),
    );
    assert_eq!(saved["ready"], true);
    assert_eq!(shared.lock().unwrap().shortcut_bindings(), bindings);
    assert_eq!(shared.lock().unwrap().document().vmf(), &before);
    value(server.execute_editor_command(Parameters(EditorCommandArgs {
        command: Command::DuplicateSelection,
        aspect: None,
    })));
    assert_ne!(shared.lock().unwrap().document().vmf(), &before);
    value(server.execute_editor_command(Parameters(EditorCommandArgs {
        command: Command::Undo,
        aspect: None,
    })));
    assert_eq!(shared.lock().unwrap().document().vmf(), &before);
    for invalid in [
        json!({"action":"save","snapshot":"0"}),
        json!({"action":"reset","snapshot":"0","bindings":bindings}),
        json!({"command":"open_map"}),
        json!({"command":"undo","key":"Z"}),
    ] {
        if invalid.get("action").is_some() {
            assert!(serde_json::from_value::<ShortcutsArgs>(invalid).is_err());
        } else {
            assert!(serde_json::from_value::<EditorCommandArgs>(invalid).is_err());
        }
    }
}

#[tokio::test]
async fn view_commands_share_the_schema_and_reject_after_originating_document_drain() {
    use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(5);

    let directory = tempfile::tempdir().unwrap();
    let authority = crate::shortcuts::Authority::configured(Ok(directory.path().join("shortcuts")));
    let mut session = Session::with_keyboard_shortcuts(Err("no assets".into()), authority.clone());
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap();
    let peer = Session::with_keyboard_shortcuts(Err("peer assets".into()), authority);
    let peer_camera = peer.camera();
    let peer_source = peer.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    let schema = server
        .get_tool("execute_editor_command")
        .unwrap()
        .input_schema;
    let schema = Value::Object((*schema).clone());
    let commands = schema
        .pointer("/properties/command/enum")
        .or_else(|| schema.pointer("/$defs/Command/enum"))
        .unwrap();
    assert_eq!(commands, &json!(Command::ALL));
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let before = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let selection = lock(&shared).selected_ids();
    let bindings = peer.keyboard_shortcuts_state();
    for (name, projection) in [
        ("top", Projection::Top),
        ("front", Projection::Front),
        ("side", Projection::Side),
        ("perspective", Projection::Perspective),
        ("top", Projection::Top),
    ] {
        let expected = lock(&shared).camera_for_projection(projection);
        let request = CallToolRequestParams::new("execute_editor_command")
            .with_arguments(json!({"command":name}).as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
        let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
            .await
            .unwrap()
            .unwrap()
        {
            CallToolResponse::Complete(result) => value(Ok(result)),
            result => panic!("unexpected command response: {result:?}"),
        };
        assert_eq!(result["command"], name);
        assert_eq!(result["projection"], name);
        assert_eq!(lock(&shared).camera(), expected);
        assert_eq!(document.active_operations(), 0);
        if projection == Projection::Top {
            let mut camera = expected;
            camera.ortho_height += 128.0;
            lock(&shared).set_camera(camera);
        }
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).selected_ids(), selection);
    let camera = lock(&shared).camera();
    for (name, enabled, changed) in [
        ("objects", false, false),
        ("faces", true, true),
        ("faces", true, false),
        ("objects", false, true),
    ] {
        let request = CallToolRequestParams::new("execute_editor_command")
            .with_arguments(json!({"command":name}).as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(2), service.peer().clone());
        let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
            .await
            .unwrap()
            .unwrap()
        {
            CallToolResponse::Complete(result) => value(Ok(result)),
            result => panic!("unexpected command response: {result:?}"),
        };
        assert_eq!(result["mode"], name);
        assert_eq!(result["mode_changed"], changed);
        assert_eq!(lock(&shared).face_mode(), enabled);
    }
    assert!(lock(&shared).selected_ids().is_empty());
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    let peer_spatial = peer.spatial();
    let peer_view = peer.view();
    for (name, field) in [
        ("show_grid", "grid_visible"),
        ("snap_to_grid", "snap"),
        ("tool_textures", "tool_textures"),
        ("selection_outlines", "selection_outlines"),
    ] {
        for enabled in [false, true] {
            let request = CallToolRequestParams::new("execute_editor_command")
                .with_arguments(json!({"command":name}).as_object().unwrap().clone());
            let context = RequestContext::new(NumberOrString::Number(3), service.peer().clone());
            let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
                .await
                .unwrap()
                .unwrap()
            {
                CallToolResponse::Complete(result) => value(Ok(result)),
                result => panic!("unexpected command response: {result:?}"),
            };
            assert_eq!(result["command"], name);
            assert_eq!(result[field], enabled);
            let session = lock(&shared);
            let actual = match field {
                "grid_visible" => session.spatial().grid_visible,
                "snap" => session.spatial().snap,
                "tool_textures" => session.view().tool_textures,
                "selection_outlines" => session.view().selection_outlines,
                _ => unreachable!(),
            };
            assert_eq!(actual, enabled);
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(document.active_operations(), 0);
        }
    }
    assert_eq!(peer.spatial(), peer_spatial);
    assert_eq!(peer.view(), peer_view);
    assert!(!peer.face_mode());
    assert_eq!(peer.camera(), peer_camera);
    assert_eq!(peer.document().vmf(), &peer_source);
    assert_eq!(peer.keyboard_shortcuts_state(), bindings);
    document.shutdown().unwrap();
    let camera = lock(&shared).camera();
    let spatial = lock(&shared).spatial();
    let view = lock(&shared).view();
    let log = lock(&shared).log_serial();
    let request = CallToolRequestParams::new("execute_editor_command").with_arguments(
        json!({"command":"tool_textures"})
            .as_object()
            .unwrap()
            .clone(),
    );
    let context = RequestContext::new(NumberOrString::Number(2), service.peer().clone());
    let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
        .await
        .unwrap()
        .unwrap()
    {
        CallToolResponse::Complete(result) => result,
        result => panic!("unexpected command response: {result:?}"),
    };
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        "the document is closed"
    );
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).spatial(), spatial);
    assert_eq!(lock(&shared).view(), view);
    assert!(!lock(&shared).face_mode());
    assert_eq!(lock(&shared).log_serial(), log);
    tokio::time::timeout(TIMEOUT, service.cancel())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn attached_shortcut_tools_share_activation_and_errors_but_execute_in_the_originating_session() {
    let directory = tempfile::tempdir().unwrap();
    let authority = crate::shortcuts::Authority::configured(Ok(directory.path().to_owned()));
    let mut first = Session::with_keyboard_shortcuts(Err("no graphics".into()), authority.clone());
    let mut second = Session::with_keyboard_shortcuts(Err("no graphics".into()), authority);
    first
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(32.), "first", true)
        .unwrap();
    second
        .create_block(
            Actor::Agent,
            DVec3::splat(1000.),
            DVec3::splat(1064.),
            "second",
            true,
        )
        .unwrap();
    let first_source = first.document().vmf().clone();
    let first_history = first.history_snapshot();
    let first_shared = Arc::new(Mutex::new(first));
    let second_shared = Arc::new(Mutex::new(second));
    let first = EditorServer::new(first_shared.clone(), None);
    let second = EditorServer::new(second_shared.clone(), None);
    let initial = value(first.get_keyboard_shortcuts());
    let mut bindings = Bindings::default();
    bindings.frame_selection[0].as_mut().unwrap().key = "F8".into();
    let saved = value(
        first.set_keyboard_shortcuts(Parameters(ShortcutsArgs::Save {
            snapshot: initial["snapshot"].as_str().unwrap().into(),
            bindings: Box::new(bindings.clone()),
        })),
    );
    let other = value(second.get_keyboard_shortcuts());
    assert_eq!(other["current"], saved["current"]);
    assert_eq!(other["snapshot"], saved["snapshot"]);
    value(second.execute_editor_command(Parameters(EditorCommandArgs {
        command: Command::DuplicateSelection,
        aspect: None,
    })));
    assert_eq!(lock(&first_shared).document().vmf(), &first_source);
    assert_eq!(lock(&first_shared).history_snapshot(), first_history);
    assert_eq!(lock(&second_shared).document().vmf().solids().count(), 2);
    std::fs::write(
        directory.path().join(crate::shortcuts::FILE_NAME),
        b"broken",
    )
    .unwrap();
    assert!(
        second
            .set_keyboard_shortcuts(Parameters(ShortcutsArgs::Reload {
                snapshot: other["snapshot"].as_str().unwrap().into()
            }))
            .unwrap()
            .is_error
            .unwrap()
    );
    let failed = value(first.get_keyboard_shortcuts());
    assert_eq!(failed["ready"], false);
    assert_eq!(failed["current"], json!(bindings));
    assert!(failed["error"].is_string());
    let reset = value(
        first.set_keyboard_shortcuts(Parameters(ShortcutsArgs::Reset {
            snapshot: failed["snapshot"].as_str().unwrap().into(),
        })),
    );
    assert_eq!(
        value(second.get_keyboard_shortcuts())["current"],
        reset["current"]
    );
    assert_eq!(reset["current"], json!(Bindings::default()));
    assert!(first.offscreen.lock().unwrap().renderer.is_none());
    assert!(second.offscreen.lock().unwrap().renderer.is_none());
}
