use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn recovery_mcp_preserves_independent_failures_and_graphics_laziness() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = Session::new(Err("no assets".into()));
    state.configure_recovery(Ok(directory.path().to_owned()));
    let request = state
        .begin_recovery_snapshot(crate::recovery::Kind::Manual)
        .unwrap();
    state
        .finish_recovery_snapshot(
            Actor::Agent,
            &request,
            Err("snapshot storage failure".into()),
        )
        .unwrap_err();
    let source = state.document().vmf().clone();
    let history = state.history_snapshot();
    let camera = state.camera();
    let shared = Arc::new(Mutex::new(state));
    let server = EditorServer::new(shared.clone(), None);
    value(server.set_recovery_settings(Parameters(
        serde_json::from_value(json!({"interval_seconds":60})).unwrap(),
    )));
    assert_eq!(
        lock(&shared).recovery_status()["snapshot_error"],
        "snapshot storage failure"
    );
    assert!(
        server
            .set_recovery_settings(Parameters(
                serde_json::from_value(json!({"interval_seconds":0})).unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap()
    );
    let failed = value(server.get_recovery(Parameters(serde_json::from_value(json!({})).unwrap())));
    assert!(failed["status"]["settings_error"].is_string());
    assert_eq!(
        failed["status"]["snapshot_error"],
        "snapshot storage failure"
    );
    assert!(
        failed["status"]["error"]
            .as_str()
            .unwrap()
            .contains("snapshot storage failure")
    );
    let warning = failed["status"]["settings_error"].clone();
    value(server.create_recovery_snapshot().await);
    let captured =
        value(server.get_recovery(Parameters(serde_json::from_value(json!({})).unwrap())));
    assert_eq!(captured["status"]["settings_error"], warning);
    assert_eq!(captured["status"]["error"], warning);
    assert!(captured["status"]["snapshot_error"].is_null());
    value(server.set_recovery_settings(Parameters(
        serde_json::from_value(json!({"interval_seconds":90})).unwrap(),
    )));
    assert!(lock(&shared).recovery_status()["error"].is_null());
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(state.camera(), camera);
    drop(state);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
