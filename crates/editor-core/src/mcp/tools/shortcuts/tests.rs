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
