use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn change(server: &EditorServer, action: &str) -> Value {
    let state = value(server.get_preview_defaults());
    value(server.set_preview_defaults(Parameters(
        serde_json::from_value(json!({"action": action, "snapshot": state["snapshot"]})).unwrap(),
    )))
}

#[test]
fn mcp_and_ui_operations_share_explicit_defaults_without_starting_graphics() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let custom = crate::preview_preferences::Preview {
        selection_outlines: false,
        entity_images: false,
        entity_model_guides: false,
        rope_preview: true,
        sky_preview: true,
    };
    value(server.set_view(Parameters(serde_json::from_value(json!(custom)).unwrap())));
    let path = directory.path().join(crate::preview_preferences::FILE_NAME);
    assert!(!path.exists());
    let saved = change(&server, "save");
    assert_eq!(saved["saved"], json!(custom));
    assert_eq!(saved["write_committed"], true);
    let stale = saved["snapshot"].clone();
    value(server.set_view(Parameters(
        serde_json::from_value(json!({"rope_preview": false})).unwrap(),
    )));
    assert_eq!(
        server
            .set_preview_defaults(Parameters(
                serde_json::from_value(json!({"action":"save","snapshot":stale})).unwrap()
            ))
            .unwrap()
            .is_error,
        Some(true)
    );
    assert_eq!(change(&server, "apply")["current"], json!(custom));
    let snapshot = lock(&shared).preview_preferences_stamp();
    lock(&shared)
        .change_preview_preferences(Actor::User, "reset", &snapshot)
        .unwrap();
    assert_eq!(
        value(server.get_preview_defaults())["current"],
        json!(custom)
    );
    assert_eq!(
        change(&server, "apply")["current"],
        json!(crate::preview_preferences::Preview::default())
    );
    std::fs::write(&path, b"malformed").unwrap();
    let snapshot = value(server.get_preview_defaults())["snapshot"].clone();
    assert_eq!(
        server
            .set_preview_defaults(Parameters(
                serde_json::from_value(json!({"action":"reload","snapshot":snapshot})).unwrap()
            ))
            .unwrap()
            .is_error,
        Some(true)
    );
    let state = value(server.get_preview_defaults());
    assert_eq!(state["ready"], false);
    assert!(state["error"].is_string());
    assert_eq!(std::fs::read(&path).unwrap(), b"malformed");
    assert_eq!(change(&server, "reset")["ready"], true);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    for invalid in [
        json!({"action":"save","snapshot":"","sky_exposure_ev":2}),
        json!({"action":"save","snapshot":"","selection_outlines":false}),
        json!({"action":"unknown","snapshot":""}),
    ] {
        assert!(serde_json::from_value::<PreviewDefaultsArgs>(invalid).is_err());
    }
}
