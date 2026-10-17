use super::*;
use std::time::{Duration, Instant};

#[test]
fn mcp_queued_launch_is_admitted_before_worker_and_cancelled_waiter_retires_it() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
    let path = directory.path().join("room.vmf");
    std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
    lock(&shared).open(Actor::User, &path, false).unwrap();
    crate::runtime::CompileSubmission::begin(&shared, Actor::Agent, None, false, None)
        .unwrap()
        .run()
        .unwrap();
    let server = EditorServer::new(shared.clone(), None);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (release, wait) = std::sync::mpsc::channel();
    runtime.block_on(async {
        let blocker =
            tokio::task::spawn_blocking(move || wait.recv_timeout(Duration::from_secs(5)).unwrap());
        let caller = tokio::spawn(async move { server.launch_game().await });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !lock(&shared).launching() {
            assert!(
                Instant::now() < deadline,
                "MCP launch was not admitted while queued"
            );
            tokio::task::yield_now().await;
        }
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(lock(&shared).launching());
        release.send(()).unwrap();
        blocker.await.unwrap();
        while lock(&shared).launching() {
            assert!(
                Instant::now() < deadline,
                "cancelled queued launch did not retire"
            );
            tokio::task::yield_now().await;
        }
    });
    assert!(!lock(&shared).game_running());
    assert!(!directory.path().join("cstrike/downloadlists").exists());
}
