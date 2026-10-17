use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn change(server: &EditorServer, action: &str) -> Value {
    let snapshot = value(server.get_navigation_settings())["snapshot"].clone();
    value(server.set_navigation_defaults(Parameters(
        serde_json::from_value(json!({"action":action,"snapshot":snapshot})).unwrap(),
    )))
}

#[test]
fn mcp_and_desktop_operations_share_temporary_speed_and_all_explicit_defaults_actions() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    let source = session.document().vmf().clone();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(
        value(server.get_navigation_settings())["current"]["fly_speed"],
        600.0
    );
    let args = serde_json::from_value(json!({"fly_speed":987.625})).unwrap();
    value(server.set_navigation_settings(Parameters(args)));
    assert_eq!(lock(&shared).fly_speed(), 987.625);
    let path = directory
        .path()
        .join(crate::navigation_preferences::FILE_NAME);
    assert!(!path.exists());
    assert_eq!(change(&server, "save")["saved"]["fly_speed"], 987.625);
    let stale = value(server.get_navigation_settings())["snapshot"].clone();
    lock(&shared).adjust_fly_speed(240.0).unwrap();
    assert_eq!(
        value(server.get_navigation_settings())["current"]["fly_speed"],
        1975.25
    );
    assert_eq!(
        server
            .set_navigation_defaults(Parameters(
                serde_json::from_value(json!({"action":"apply","snapshot":stale})).unwrap()
            ))
            .unwrap()
            .is_error,
        Some(true)
    );
    assert_eq!(change(&server, "apply")["current"]["fly_speed"], 987.625);
    assert_eq!(change(&server, "reset")["current"]["fly_speed"], 987.625);
    assert_eq!(change(&server, "apply")["current"]["fly_speed"], 600.0);
    std::fs::write(&path, b"outside malformed edit").unwrap();
    let snapshot = value(server.get_navigation_settings())["snapshot"].clone();
    assert_eq!(
        server
            .set_navigation_defaults(Parameters(
                serde_json::from_value(json!({"action":"reload","snapshot":snapshot})).unwrap()
            ))
            .unwrap()
            .is_error,
        Some(true)
    );
    assert_eq!(value(server.get_navigation_settings())["ready"], false);
    assert_eq!(std::fs::read(&path).unwrap(), b"outside malformed edit");
    assert_eq!(change(&server, "reset")["ready"], true);
    for speed in [24.0, 20001.0] {
        assert_eq!(
            server
                .set_navigation_settings(Parameters(NavigationArgs { fly_speed: speed }))
                .unwrap()
                .is_error,
            Some(true)
        );
        assert_eq!(lock(&shared).fly_speed(), 600.0);
    }
    for invalid in [
        json!({}),
        json!({"fly_speed":600,"position":[0,0,0]}),
        json!({"fly_speed":"600"}),
    ] {
        assert!(serde_json::from_value::<NavigationArgs>(invalid).is_err());
    }
    for invalid in [
        json!({"action":"save","snapshot":"","speed":600}),
        json!({"action":"unknown","snapshot":""}),
    ] {
        assert!(serde_json::from_value::<NavigationDefaultsArgs>(invalid).is_err());
    }
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), 0);
    assert_eq!(session.camera(), camera);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
