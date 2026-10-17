use super::*;
use std::time::{Duration, Instant};

fn value(result: ToolResult, error: bool) -> Value {
    let result = result.unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn instance_rejection_reports_blocking_run_and_background_mcp_build_failures() {
    let (directory, game) = crate::test_assets::game();
    let source_path = directory.path().join("instances.vmf");
    let bsp = source_path.with_extension("bsp");
    let lin = source_path.with_extension("lin");
    let mut vmf = crate::test_assets::room();
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
    let mut entity = Entity::new(id, "FuNc_InStAnCe_PaRmS");
    entity.properties.set("origin", "0 0 64");
    vmf.entities.push(entity);
    let source_bytes = vmf.write(&Default::default());
    std::fs::write(&source_path, &source_bytes).unwrap();
    std::fs::write(&bsp, b"previous BSP").unwrap();
    std::fs::write(&lin, b"previous pointfile").unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source_path, true).unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let failed = value(
        server
            .compile_map(Parameters(CompileArgs { path: None }))
            .await,
        true,
    );
    assert_eq!(failed["success"], false);
    assert!(failed["bsp"].is_null());
    assert!(failed["game"].is_null());
    assert_eq!(failed["diagnostics"][0]["level"], "error");
    assert_eq!(failed["diagnostics"][0]["object"], format!("entity:{id}"));
    let run = value(
        server.run_map(Parameters(CompileArgs { path: None })).await,
        true,
    );
    assert_eq!(run["success"], false);
    assert!(run["game"].is_null());
    assert_eq!(run["diagnostics"], failed["diagnostics"]);
    let started = value(
        server.start_compile(Parameters(CompileArgs { path: None })),
        false,
    );
    let token = started["job"]["token"].clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        let status = value(server.get_compile_status(), false);
        if status["job"]["status"] == "failed" {
            break status;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(status["job"]["token"], token);
    assert_eq!(status["job"]["result"]["success"], false);
    assert_eq!(status["job"]["result"]["published"], false);
    assert_eq!(
        status["job"]["result"]["diagnostics"],
        failed["diagnostics"]
    );
    assert_eq!(status["launch"]["ready"], false);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert!(!session.launching());
    assert!(!session.game_running());
    assert_eq!(std::fs::read_to_string(&source_path).unwrap(), source_bytes);
    assert_eq!(std::fs::read(&bsp).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
