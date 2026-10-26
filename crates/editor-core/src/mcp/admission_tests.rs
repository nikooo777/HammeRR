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

async fn snap_request(
    server: &EditorServer,
    running: &RunningService<rmcp::RoleServer, EditorServer>,
    name: &'static str,
    arguments: Value,
) -> CallToolResult {
    let request =
        CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
    complete(server.call_tool(request, context(running)).await)
}

fn snap_value(result: CallToolResult) -> Value {
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn independent_grid_snap_protocol_shares_selection_deltas_preview_cancel_apply_and_undo() {
    let (_directory, document, server) = fixture();
    let (ids, source, history, camera, rigid, independent) = {
        let mut session = lock(document.session());
        let mut ids = Vec::new();
        for minimum in [
            DVec3::new(-3.125, 5.25, 7.5),
            DVec3::new(34.25, -19.5, 9.75),
        ] {
            ids.push(
                session
                    .create_block(
                        Actor::User,
                        minimum,
                        minimum + DVec3::splat(16.0),
                        "brick/test",
                        false,
                    )
                    .unwrap(),
            );
        }
        session.select(&ids, SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let rigid =
            serde_json::to_value(session.preview_snap_to_grid(Actor::User, &ids).unwrap()).unwrap();
        session.cancel_transform_preview();
        let independent = serde_json::to_value(
            session
                .preview_snap_to_grid_mode(
                    Actor::User,
                    &ids,
                    crate::session::GridSnapMode::EachObject,
                )
                .unwrap(),
        )
        .unwrap();
        session.cancel_transform_preview();
        (ids, source, history, camera, rigid, independent)
    };
    let (running, _client) = service(server.clone());
    let default =
        snap_value(snap_request(&server, &running, "preview_snap_to_grid", json!({})).await);
    assert_eq!(default["grid_snap"], rigid);
    snap_value(snap_request(&server, &running, "cancel_transform_preview", json!({})).await);
    let preview = snap_value(
        snap_request(
            &server,
            &running,
            "preview_snap_to_grid",
            json!({"mode":"each_object"}),
        )
        .await,
    );
    assert_eq!(preview["grid_snap"], independent);
    assert_eq!(preview["grid_snap"]["objects"][0]["id"], id_string(ids[0]));
    assert_ne!(
        preview["grid_snap"]["objects"][0]["delta"],
        preview["grid_snap"]["objects"][1]["delta"]
    );
    let inspected =
        snap_value(snap_request(&server, &running, "get_transform_handles", json!({})).await);
    assert_eq!(inspected["state"]["preview"]["grid_snap"], independent);
    for arguments in [json!({"mode":"per_object"}), json!({"independent":true})] {
        let result = server
            .call_tool(
                CallToolRequestParams::new("preview_snap_to_grid")
                    .with_arguments(arguments.as_object().unwrap().clone()),
                context(&running),
            )
            .await;
        assert!(result.is_err() || result.as_ref().is_ok_and(|response| matches!(response, CallToolResponse::Complete(result) if result.is_error == Some(true))));
    }
    let failed = snap_request(
        &server,
        &running,
        "preview_snap_to_grid",
        json!({"mode":"each_object","ids":[id_string(ids[0]),"entity:4294967295"]}),
    )
    .await;
    assert_eq!(failed.is_error, Some(true));
    assert_eq!(lock(document.session()).document().vmf(), &source);
    assert_eq!(lock(document.session()).history_snapshot(), history);
    assert_eq!(lock(document.session()).camera(), camera);
    assert_eq!(lock(document.session()).selected_ids(), ids);
    assert_eq!(
        serde_json::to_value(
            &lock(document.session())
                .transform_preview()
                .unwrap()
                .grid_snap
        )
        .unwrap(),
        independent
    );
    snap_value(snap_request(&server, &running, "cancel_transform_preview", json!({})).await);
    assert_eq!(lock(document.session()).render_document().vmf(), &source);
    assert_eq!(lock(document.session()).history_snapshot(), history);
    let explicit =
        json!({"mode":"each_object","ids":[id_string(ids[1]),id_string(ids[0]),id_string(ids[1])]});
    let repeated =
        snap_value(snap_request(&server, &running, "preview_snap_to_grid", explicit).await);
    assert_eq!(repeated["grid_snap"], independent);
    let displayed = lock(document.session()).render_document().vmf().clone();
    let count = lock(document.session()).document().undo_labels().count();
    snap_value(snap_request(&server, &running, "commit_transform_preview", json!({})).await);
    assert_eq!(lock(document.session()).document().vmf(), &displayed);
    assert_eq!(
        lock(document.session()).document().undo_labels().count(),
        count + 1
    );
    snap_value(snap_request(&server, &running, "undo", json!({})).await);
    assert_eq!(lock(document.session()).document().vmf(), &source);
    assert_eq!(lock(document.session()).camera(), camera);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    running.cancel().await.unwrap();
    document.shutdown().unwrap();
}

#[tokio::test]
async fn map_info_exposes_the_same_fixed_document_close_observation() {
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
    assert!(server.tool_router.get("close_document").is_some());
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
