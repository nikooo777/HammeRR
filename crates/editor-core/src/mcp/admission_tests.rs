use super::*;
use crate::runtime::DocumentLifecycle;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, RunningService, serve_directly};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture() -> (tempfile::TempDir, DocumentHandle, EditorServer) {
    let directory = tempfile::tempdir().unwrap();
    let mut state = Session::new(Err("no assets required".into()));
    state.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = Arc::new(Mutex::new(state));
    let document = DocumentHandle::start(shared).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    (directory, document, server)
}

fn service(
    server: EditorServer,
) -> (
    RunningService<rmcp::RoleServer, EditorServer>,
    tokio::io::DuplexStream,
) {
    let (transport, client) = tokio::io::duplex(4096);
    (
        serve_directly::<rmcp::RoleServer, _, _, _, _>(server, transport, None),
        client,
    )
}

fn context(
    service: &RunningService<rmcp::RoleServer, EditorServer>,
) -> RequestContext<rmcp::RoleServer> {
    RequestContext::new(NumberOrString::Number(1), service.peer().clone())
}

fn complete(result: Result<CallToolResponse, ErrorData>) -> CallToolResult {
    match result.unwrap() {
        CallToolResponse::Complete(result) => result,
        result => panic!("unexpected tool response: {result:?}"),
    }
}

#[tokio::test]
async fn map_info_exposes_the_same_fixed_document_close_observation_without_a_close_tool() {
    let (_directory, document, server) = fixture();
    let (running, _client) = service(server.clone());
    let result = complete(
        server
            .call_tool(
                CallToolRequestParams::new("get_map_info"),
                context(&running),
            )
            .await,
    );
    let value: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(
        value["document_close"],
        document.document_close_review().unwrap().inspection()
    );
    assert_eq!(value["document_close"]["discard_required"], false);
    assert!(server.tool_router.get("close_document").is_none());
    running.cancel().await.unwrap();
    document.shutdown().unwrap();
}

#[tokio::test]
async fn common_dispatch_rejects_retained_edit_read_and_storage_calls_after_terminal_drain() {
    let (_directory, document, server) = fixture();
    let (service, _client) = service(server.clone());
    let request = CallToolRequestParams::new("create_entity").with_arguments(
        json!({"classname":"info_target","origin":[0,0,0]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let result = complete(server.call_tool(request, context(&service)).await);
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    assert_eq!(document.active_operations(), 0);
    document.shutdown().unwrap();
    let before = lock(document.session()).history_snapshot();
    let vmf = lock(document.session()).document().vmf().clone();
    let recovery = lock(document.session()).recovery_status();
    for (name, arguments) in [
        ("create_entity", json!({"classname":"info_target"})),
        ("get_map_info", json!({})),
        ("load_pointfile", json!({"path":"/not-read.lin"})),
        ("render_sound_preview", json!({"value":"not-decoded.wav"})),
        ("set_recovery_settings", json!({"enabled":false})),
    ] {
        let request =
            CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
        let result = complete(server.call_tool(request, context(&service)).await);
        assert_eq!(result.is_error, Some(true), "{name}: {result:?}");
        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "the document is closed"
        );
    }
    assert!(server.get_tool("new_map").is_some());
    assert_eq!(lock(document.session()).history_snapshot(), before);
    assert_eq!(lock(document.session()).document().vmf(), &vmf);
    assert_eq!(lock(document.session()).recovery_status(), recovery);
    assert_eq!(server.rendering_status()["initialized"], false);
    service.cancel().await.unwrap();
}

#[test]
fn cancelled_dispatched_stop_keeps_queued_worker_admission_until_actual_retirement() {
    let (_directory, document, server) = fixture();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (service, _client) = service(server.clone());
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv_timeout(TIMEOUT).unwrap();
        });
        entered.await.unwrap();
        let request_context = context(&service);
        let caller = tokio::spawn(async move {
            server
                .call_tool(CallToolRequestParams::new("stop_game"), request_context)
                .await
        });
        let deadline = Instant::now() + TIMEOUT;
        while document.active_operations() == 0 {
            assert!(Instant::now() < deadline, "stop was not admitted");
            tokio::task::yield_now().await;
        }
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(document.active_operations(), 1);
        let draining = document.clone();
        let (finished, done) = mpsc::channel();
        let shutdown = std::thread::spawn(move || finished.send(draining.shutdown()).unwrap());
        while document.lifecycle() == DocumentLifecycle::Live {
            assert!(Instant::now() < deadline, "shutdown did not start");
            tokio::task::yield_now().await;
        }
        assert!(done.try_recv().is_err());
        release.send(()).unwrap();
        blocker.await.unwrap();
        done.recv_timeout(TIMEOUT).unwrap().unwrap();
        shutdown.join().unwrap();
        assert_eq!(document.active_operations(), 0);
        assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
        service.cancel().await.unwrap();
    });
}

#[test]
fn never_run_worker_drops_recovery_request_and_token_before_last_runtime_owner_drain() {
    let (directory, document, server) = fixture();
    let shared = document.session().clone();
    lock(&shared)
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let request = server.admitted().unwrap();
    let submission = crate::runtime::RecoverySubmission::begin(&shared, Actor::Agent).unwrap();
    let worker = request.worker(move || submission.run());
    assert_eq!(document.active_operations(), 1);
    assert!(lock(&shared).recovery_busy());
    drop(request);
    drop(server);
    drop(document);
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        drop(worker);
        send.send(()).unwrap();
    });
    receive.recv_timeout(TIMEOUT).unwrap();
    thread.join().unwrap();
    assert!(!lock(&shared).recovery_busy());
    assert_eq!(lock(&shared).recovery_status()["pending_changes"], false);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    assert_eq!(store.list().unwrap().entries.len(), 1);
}

#[test]
fn never_run_compile_drops_request_and_operation_before_last_runtime_owner_drain() {
    let (directory, game) = crate::test_assets::game();
    let mut state = Session::new(Ok(game));
    state.configure_recovery(Ok(directory.path().join("recovery")));
    state
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let shared = Arc::new(Mutex::new(state));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    let request = server.admitted().unwrap();
    let path = directory.path().join("abandoned.bsp");
    let submission =
        CompileSubmission::begin(&shared, Actor::Agent, Some(&path), false, None).unwrap();
    let worker = request.worker(move || submission.run());
    assert!(lock(&shared).compiling());
    assert_eq!(document.active_operations(), 1);
    drop(request);
    drop(server);
    drop(document);
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        drop(worker);
        send.send(()).unwrap();
    });
    receive
        .recv_timeout(TIMEOUT)
        .expect("abandoned compile blocked final runtime-owner drop");
    thread.join().unwrap();
    let state = lock(&shared);
    assert!(!state.compiling());
    assert_eq!(state.compile_status()["job"]["status"], "failed");
    assert_eq!(state.recovery_entries().unwrap().entries.len(), 1);
    assert!(!path.exists() && !path.with_extension("lin").exists());
}

#[tokio::test]
async fn failed_terminal_dispatch_retains_exact_shutdown_error_without_retry() {
    let (directory, document, server) = fixture();
    lock(document.session())
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let recovery = directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    let (service, _client) = service(server.clone());
    let result = complete(
        server
            .call_tool(
                CallToolRequestParams::new("get_map_info"),
                context(&service),
            )
            .await,
    );
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        format!("document shutdown failed: {error}")
    );
    assert_eq!(result.is_error, Some(true));
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
    service.cancel().await.unwrap();
}
