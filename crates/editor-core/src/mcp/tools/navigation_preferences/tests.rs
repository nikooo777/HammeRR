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

fn attached_defaults_state(server: &EditorServer, kind: usize) -> Value {
    value(match kind {
        0 => server.get_grid_defaults(),
        1 => server.get_preview_defaults(),
        _ => server.get_navigation_settings(),
    })
}

fn attached_defaults_change(
    server: &EditorServer,
    kind: usize,
    action: &str,
    snapshot: Value,
) -> ToolResult {
    let args = json!({"action":action,"snapshot":snapshot});
    match kind {
        0 => server.set_grid_defaults(Parameters(serde_json::from_value(args).unwrap())),
        1 => server.set_preview_defaults(Parameters(serde_json::from_value(args).unwrap())),
        _ => server.set_navigation_defaults(Parameters(serde_json::from_value(args).unwrap())),
    }
}

#[test]
fn attached_mcp_sessions_report_shared_saved_defaults_and_reject_old_forms_without_applying_to_peers()
 {
    let directory = tempfile::tempdir().unwrap();
    let defaults = crate::saved_defaults::SavedDefaults::configured(Ok(directory.path().into()));
    let first = Arc::new(Mutex::new(Session::with_saved_defaults(
        Err("no assets needed".into()),
        defaults.clone(),
    )));
    let second = Arc::new(Mutex::new(Session::with_saved_defaults(
        Err("no assets needed".into()),
        defaults,
    )));
    let left = EditorServer::new(first.clone(), None);
    let right = EditorServer::new(second.clone(), None);
    value(left.set_spatial_settings(Parameters(
        serde_json::from_value(json!({"grid_size":17.375})).unwrap(),
    )));
    value(left.set_view(Parameters(
        serde_json::from_value(json!({"selection_outlines":false})).unwrap(),
    )));
    value(left.set_navigation_settings(Parameters(
        serde_json::from_value(json!({"fly_speed":987.625})).unwrap(),
    )));
    let current = (0..3)
        .map(|kind| attached_defaults_state(&right, kind)["current"].clone())
        .collect::<Vec<_>>();
    let source = lock(&second).document().vmf().clone();
    let camera = lock(&second).camera();
    for (kind, peer_current) in current.iter().enumerate() {
        let old = attached_defaults_state(&right, kind)["snapshot"].clone();
        let stamp = attached_defaults_state(&left, kind)["snapshot"].clone();
        let saved = value(attached_defaults_change(&left, kind, "save", stamp));
        assert_eq!(saved["write_committed"], true);
        assert_eq!(
            attached_defaults_state(&right, kind)["saved"],
            saved["saved"]
        );
        assert_eq!(
            &attached_defaults_state(&right, kind)["current"],
            peer_current
        );
        assert_eq!(
            attached_defaults_change(&right, kind, "reset", old)
                .unwrap()
                .is_error,
            Some(true)
        );
        let stamp = attached_defaults_state(&right, kind)["snapshot"].clone();
        let reloaded = value(attached_defaults_change(&right, kind, "reload", stamp));
        assert_eq!(&reloaded["current"], peer_current);
        let mut session = lock(&second);
        let reset = match kind {
            0 => {
                let stamp = session.grid_preferences_stamp();
                session.change_grid_preferences(Actor::User, "reset", &stamp)
            }
            1 => {
                let stamp = session.preview_preferences_stamp();
                session.change_preview_preferences(Actor::User, "reset", &stamp)
            }
            _ => {
                let stamp = session.navigation_preferences_stamp();
                session.change_navigation_preferences(Actor::User, "reset", &stamp)
            }
        }
        .unwrap();
        drop(session);
        assert_eq!(
            attached_defaults_state(&left, kind)["saved"],
            reset["saved"]
        );
    }
    assert_eq!(lock(&second).document().vmf(), &source);
    assert_eq!(lock(&second).camera(), camera);
    assert_eq!(lock(&second).document().undo_labels().count(), 0);
    assert_eq!(
        value(right.get_map_info())["rendering"]["initialized"],
        false
    );
}
