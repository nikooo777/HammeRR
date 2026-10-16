use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn change(server: &EditorServer, action: &str) -> Value {
    let state = value(server.get_grid_defaults());
    value(server.set_grid_defaults(Parameters(
        serde_json::from_value(json!({"action": action, "snapshot": state["snapshot"]})).unwrap(),
    )))
}

#[test]
fn mcp_and_human_actions_share_explicit_persistence_stale_guards_and_retry_errors() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    let source = session.document().vmf().clone();
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(12.5),
                snap: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(change(&server, "save")["saved"]["grid_size"], 12.5);
    let stale = value(server.get_grid_defaults())["snapshot"].clone();
    value(
        server.set_spatial_settings(Parameters(
            serde_json::from_value(
                json!({"grid_size":64,"snap":true,"elevation":19.25,"depth":10.5}),
            )
            .unwrap(),
        )),
    );
    assert!(
        server
            .set_grid_defaults(Parameters(
                serde_json::from_value(json!({"action":"save","snapshot":stale})).unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(
        value(server.get_grid_defaults())["saved"]["grid_size"],
        12.5
    );
    assert_eq!(change(&server, "apply")["current"]["grid_size"], 12.5);
    assert_eq!(lock(&shared).spatial().elevation, 19.25);
    assert_eq!(lock(&shared).spatial().depth, 10.5);
    std::fs::write(
        directory.path().join(crate::grid_preferences::FILE_NAME),
        b"malformed",
    )
    .unwrap();
    let snapshot = value(server.get_grid_defaults())["snapshot"].clone();
    assert!(
        server
            .set_grid_defaults(Parameters(
                serde_json::from_value(json!({"action":"reload","snapshot":snapshot})).unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(value(server.get_grid_defaults())["ready"], false);
    let snapshot = lock(&shared).grid_preferences_stamp();
    lock(&shared)
        .change_grid_preferences(Actor::User, "reset", &snapshot)
        .unwrap();
    assert_eq!(
        value(server.get_grid_defaults())["saved"],
        json!(crate::grid_preferences::Grid::default())
    );
    assert_eq!(
        value(server.get_grid_defaults())["current"]["grid_size"],
        12.5
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    for bad in [
        json!({"action":"save","snapshot":"", "elevation":32}),
        json!({"action":"unknown","snapshot":""}),
    ] {
        assert!(serde_json::from_value::<GridDefaultsArgs>(bad).is_err());
    }
}
