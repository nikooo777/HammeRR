use super::*;
use std::time::{Duration, Instant};

fn value(result: ToolResult, error: bool) -> Value {
    let result = result.unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

async fn finished(server: &EditorServer) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = value(server.get_compile_status(), false);
        if status["job"]["status"] != "running" {
            return status;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(1));
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn both_mcp_build_paths_publish_grouping_and_recover_after_ambiguous_claims() {
    let (directory, game) = crate::test_assets::game();
    let path = directory.path().join("viscluster.vmf");
    let mut document = map_core::Document::new(crate::test_assets::room());
    document
        .edit("doorway", |tx| {
            for (mins, maxs) in [
                ([-8.0, -256.0, 0.0], [8.0, -64.0, 256.0]),
                ([-8.0, 64.0, 0.0], [8.0, 256.0, 256.0]),
            ] {
                map_core::edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "test/checker",
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&path, document.vmf().write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &path, true).unwrap();
    let mut helpers = Vec::new();
    for _ in 0..2 {
        let brush = session
            .create_block(
                Actor::Agent,
                DVec3::new(-250.0, -250.0, 8.0),
                DVec3::new(250.0, 250.0, 248.0),
                "test/checker",
                false,
            )
            .unwrap();
        helpers.push(
            session
                .tie_to_entity(Actor::Agent, &[brush], "func_viscluster", BTreeMap::new())
                .unwrap(),
        );
    }
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let bsp = path.with_extension("bsp");
    let lin = path.with_extension("lin");
    std::fs::write(&bsp, b"previous BSP").unwrap();
    std::fs::write(&lin, b"previous pointfile").unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let failed = value(
        server
            .compile_map(Parameters(CompileArgs { path: None }))
            .await,
        true,
    );
    assert_eq!(failed["success"], false);
    assert!(
        failed["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["message"].as_str().unwrap().contains("both qualify"))
    );
    value(
        server.start_compile(Parameters(CompileArgs { path: None })),
        false,
    );
    let status = finished(&server).await;
    assert_eq!(status["job"]["status"], "failed");
    assert_eq!(
        status["job"]["result"]["diagnostics"],
        failed["diagnostics"]
    );
    assert_eq!(std::fs::read(&bsp).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    lock(&shared)
        .delete_objects(Actor::Agent, &helpers[1..])
        .unwrap();
    let complete = value(
        server
            .compile_map(Parameters(CompileArgs { path: None }))
            .await,
        false,
    );
    assert_eq!(complete["success"], true);
    assert_eq!(complete["stats"]["viscluster_groups"], 1);
    let bytes = std::fs::read(&bsp).unwrap();
    value(
        server.start_compile(Parameters(CompileArgs { path: None })),
        false,
    );
    let status = finished(&server).await;
    assert_eq!(status["job"]["status"], "completed");
    assert_eq!(status["job"]["result"]["stats"]["viscluster_groups"], 1);
    assert_eq!(std::fs::read(&bsp).unwrap(), bytes);
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
