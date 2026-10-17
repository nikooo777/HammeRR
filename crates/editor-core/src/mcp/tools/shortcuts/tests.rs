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
    assert_eq!(shared.lock().unwrap().shortcut_bindings(), &bindings);
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
