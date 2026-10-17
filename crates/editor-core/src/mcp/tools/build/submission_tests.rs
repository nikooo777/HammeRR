use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn rejected(result: ToolResult) -> String {
    let result = result.unwrap();
    assert!(result.is_error.unwrap_or(false), "{result:?}");
    result.content[0].as_text().unwrap().text.clone()
}

#[tokio::test]
async fn mcp_and_background_play_share_busy_admission_and_preserve_the_completed_job() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let mut state = Session::new(Ok(game));
    let path = directory.path().join("room.vmf");
    std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
    state.open(Actor::User, &path, false).unwrap();
    let generation = state.document_generation();
    let shared = Arc::new(Mutex::new(state));
    let server = EditorServer::new(shared.clone(), None);
    let built = value(
        server
            .run_map(Parameters(CompileArgs {
                path: None,
                expected_generation: Some(generation),
            }))
            .await,
    );
    assert_eq!(built["success"], true);
    assert!(built["game"]["pid"].is_number());
    assert!(lock(&shared).game_running());
    let completed = value(server.get_compile_status());
    let log_serial = lock(&shared).log_serial();
    let mcp_error = rejected(
        server
            .run_map(Parameters(CompileArgs {
                path: None,
                expected_generation: Some(generation),
            }))
            .await,
    );
    let background_error = crate::runtime::start_compile_for_generation(
        &shared,
        Actor::User,
        None,
        true,
        Some(generation),
    )
    .unwrap_err();
    assert!(mcp_error.contains(&background_error));
    assert!(background_error.contains("already launching or running"));
    assert_eq!(value(server.get_compile_status()), completed);
    assert_eq!(lock(&shared).log_serial(), log_serial);
    lock(&shared).stop_game(Actor::User).unwrap();
    assert!(!lock(&shared).game_running());
}
