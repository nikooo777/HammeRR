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

#[test]
fn managed_mcp_background_awaited_and_play_share_queue_without_runtime_or_waiter_ownership() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    let mut queue = crate::runtime::CompileQueue::isolated().unwrap();
    let mut documents = Vec::new();
    let mut directories = Vec::new();
    let mut previous_outputs = Vec::new();
    for _ in 0..4 {
        let (directory, game, _) = crate::test_assets::game_process(false);
        let source = directory.path().join("source.vmf");
        std::fs::write(
            &source,
            crate::test_assets::room().write(&Default::default()),
        )
        .unwrap();
        let mut session = Session::new(Ok(game));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        session.open(Actor::User, &source, true).unwrap();
        documents.push(
            crate::runtime::DocumentHandle::start_managed(Arc::new(Mutex::new(session)), &queue)
                .unwrap(),
        );
        previous_outputs.push(std::fs::read(directory.path().join("source.bsp")).unwrap());
        directories.push(directory);
    }
    let (entered, started) = std::sync::mpsc::sync_channel(1);
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    queue.before_next_execute(move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(Duration::from_secs(30)).unwrap();
    });
    crate::runtime::DocumentAccess::for_document(documents[0].clone())
        .start_compile(Actor::User, None, false, None)
        .unwrap();
    started.recv_timeout(Duration::from_secs(30)).unwrap();
    let background = EditorServer::for_document(documents[1].clone(), None);
    let awaited = EditorServer::for_document(documents[2].clone(), None);
    let play = EditorServer::for_document(documents[3].clone(), None);
    assert_eq!(
        value(background.start_compile(Parameters(CompileArgs {
            path: None,
            expected_generation: None
        })))["job"]["status"],
        "queued"
    );
    let mut compile = Box::pin(awaited.compile_map(Parameters(CompileArgs {
        path: None,
        expected_generation: None,
    })));
    let mut run = Box::pin(play.run_map(Parameters(CompileArgs {
        path: None,
        expected_generation: None,
    })));
    let mut context = Context::from_waker(Waker::noop());
    assert!(compile.as_mut().poll(&mut context).is_pending());
    assert!(run.as_mut().poll(&mut context).is_pending());
    let status = value(play.get_compile_status());
    assert_eq!(status["job"]["status"], "queued");
    assert_eq!(status["job"]["queue"]["position"], 3);
    assert_eq!(status["job"]["queue"]["admitted"], 4);
    drop(compile);
    assert!(lock(documents[2].session()).compiling());
    assert_eq!(documents[2].active_operations(), 1);
    for server in [&background, &awaited, &play] {
        let token = value(server.get_compile_status())["job"]["token"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            value(server.cancel_compile(Parameters(CompileTokenArgs { token })))["job"]["status"],
            "cancelled"
        );
    }
    let Poll::Ready(result) = run.as_mut().poll(&mut context) else {
        panic!("cancelled managed run_map waiter stayed pending");
    };
    assert!(rejected(result).contains("cancelled"));
    drop(run);
    for document in &documents[1..] {
        assert_eq!(document.active_operations(), 0);
        assert!(!lock(document.session()).game_running());
        assert!(!lock(document.session()).launching());
    }
    for (directory, previous) in directories[1..].iter().zip(&previous_outputs[1..]) {
        assert_eq!(
            std::fs::read(directory.path().join("source.bsp")).unwrap(),
            *previous
        );
        assert!(!directory.path().join("source.lin").exists());
    }
    release.send(()).unwrap();
    queue.shutdown().unwrap();
    for document in &documents {
        document.shutdown().unwrap();
    }
}
