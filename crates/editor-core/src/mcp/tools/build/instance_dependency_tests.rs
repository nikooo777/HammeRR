use super::*;
use std::time::{Duration, Instant};

fn value(result: ToolResult, error: bool) -> Value {
    let result = result.unwrap();
    assert_eq!(
        result.is_error.unwrap_or(false),
        error,
        "{:?}",
        result.content
    );
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn instance_dependency_mcp_builds_report_counts_background_freshness_and_failed_run_without_launch()
 {
    let (_directory, game, source, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, true).unwrap();
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let selection = session.selected_ids();
    let camera = session.camera();
    let bytes = std::fs::read(&source).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let built = value(
        server
            .compile_map(Parameters(CompileArgs { path: None }))
            .await,
        false,
    );
    assert_eq!(built["success"], true);
    assert!(built["game"].is_null());
    assert_eq!(built["stats"]["instances"]["instances"], 1);
    assert_eq!(built["stats"]["instances"]["files"], 1);
    assert_eq!(
        built["stats"]["instances"]["dependencies"][0]["path"],
        "child.vmf"
    );
    let started = value(
        server.start_compile(Parameters(CompileArgs { path: None })),
        false,
    );
    let token = started["job"]["token"].clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = value(server.get_compile_status(), false);
        assert_eq!(status["job"]["token"], token);
        if status["job"]["status"] != "running" && status["job"]["status"] != "cancelling" {
            assert_eq!(status["job"]["status"], "completed", "{status}");
            assert_eq!(status["job"]["result"]["published"], true);
            assert_eq!(status["job"]["stale"], false);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let bsp = std::fs::read(source.with_extension("bsp")).unwrap();
    std::fs::remove_file(&child).unwrap();
    let stale = value(server.get_compile_status(), false);
    assert_eq!(stale["job"]["stale"], true);
    assert_eq!(stale["launch"]["ready"], false);
    assert_eq!(
        lock(&shared).baked_lightmap_build_state()["status"],
        "stale"
    );
    let result = server.launch_game().await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let failed = value(
        server.run_map(Parameters(CompileArgs { path: None })).await,
        true,
    );
    assert_eq!(failed["success"], false);
    assert!(failed["game"].is_null());
    assert!(
        failed["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["level"] == "error")
    );
    assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), bsp);
    assert_eq!(
        std::fs::read(source.with_extension("lin")).unwrap(),
        b"previous pointfile"
    );
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.camera(), camera);
    assert!(!session.launching() && !session.game_running());
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
