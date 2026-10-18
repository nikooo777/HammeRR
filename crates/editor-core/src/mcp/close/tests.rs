use super::*;
use crate::mcp::{ServerHandler, run_stdio, stdio};
use crate::runtime::{DocumentLifecycle, lock};
use crate::session::Session;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, JsonRpcMessage, RequestId, ServerResult,
};
use rmcp::service::{RequestContext, serve_directly};
use rmcp::transport::Transport;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const TIMEOUT: Duration = Duration::from_secs(10);

fn fixture() -> (tempfile::TempDir, DocumentHandle, EditorServer) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let document = DocumentHandle::start(Arc::new(Mutex::new(session))).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    (directory, document, server)
}

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("terminal Close fixture timed out")
}

fn close_request(token: String, discard: bool) -> CallToolRequestParams {
    CallToolRequestParams::new(TOOL_NAME).with_arguments(
        json!({"review_token":token,"discard_changes":discard})
            .as_object()
            .unwrap()
            .clone(),
    )
}

#[test]
fn native_owner_retains_cleanup_and_ui_failures_without_reopening_document() {
    let (_directory, document, _server) = fixture();
    let quit = Arc::new(AtomicBool::new(false));
    let quitting = quit.clone();
    let ui = TerminalUi::new(
        |_| Err("owned UI hide failed".into()),
        move || {
            quitting.store(true, Ordering::Release);
            Err("final quit failed".into())
        },
    );
    let mut owner = TerminalOwner::start(document.clone(), Some(ui)).unwrap();
    let review = document.document_close_review().unwrap();
    owner.handle().commit_native(&review, false).unwrap();
    let error = owner.shutdown().unwrap_err();
    assert!(error.contains("owned UI hide failed"));
    assert!(error.contains("final quit failed"));
    assert_eq!(owner.shutdown(), Err(error.clone()));
    assert_eq!(owner.handle().result(), Some(Err(error)));
    assert!(quit.load(Ordering::Acquire));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert!(document.admit().is_err());
}

#[tokio::test]
async fn lifecycle_dispatch_has_no_ordinary_lease_and_waits_for_exact_output() {
    let (_directory, document, mut server) = fixture();
    let mut owner = TerminalOwner::start(document.clone(), None).unwrap();
    server = server.with_terminal_close(owner.handle()).unwrap();
    let (mut output, observed) = stdio::transport(tokio::io::empty(), tokio::io::sink());
    server.stdio_output = Some(observed);
    let (service_io, _peer) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), service_io, None);
    let id = RequestId::Number(31);
    let context = RequestContext::new(id.clone(), running.peer().clone());
    let token = document.document_close_review().unwrap().token();
    let result = bounded(server.call_tool(close_request(token, false), context))
        .await
        .unwrap();
    let CallToolResponse::Complete(result) = result else {
        panic!("expected completed Close");
    };
    assert_eq!(result.is_error, Some(false));
    assert_eq!(document.active_operations(), 0);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert!(owner.handle().result().is_none());
    let wrong = JsonRpcMessage::response(
        ServerResult::CallToolResult(CallToolResult::success(vec![])),
        id.clone(),
    );
    bounded(output.send(wrong)).await.unwrap();
    assert!(owner.handle().result().is_none());
    bounded(output.send(JsonRpcMessage::response(
        ServerResult::CallToolResult(result),
        id,
    )))
    .await
    .unwrap();
    owner.shutdown().unwrap();
    assert_eq!(owner.handle().result(), Some(Ok(())));
    bounded(running.cancel()).await.unwrap();
}

#[tokio::test]
async fn cancellation_while_waiting_for_session_preserves_live_and_the_unused_fence() {
    let (_directory, document, mut server) = fixture();
    let mut owner = TerminalOwner::start(document.clone(), None).unwrap();
    let terminal = owner.handle();
    server = server.with_terminal_close(terminal.clone()).unwrap();
    let (_output, observed_output) = stdio::transport(tokio::io::empty(), tokio::io::sink());
    server.stdio_output = Some(observed_output.clone());
    let (service_io, _peer) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), service_io, None);
    let context = RequestContext::new(RequestId::Number(33), running.peer().clone());
    let cancel = context.ct.clone();
    let token = document.document_close_review().unwrap().token();
    let (entered, observed) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    *terminal.0.before_commit.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        wait.recv_timeout(TIMEOUT).unwrap();
    }));
    let session = lock(document.session());
    let worker = std::thread::spawn(move || terminal.commit_mcp(&server, &token, false, &context));
    observed.recv_timeout(TIMEOUT).unwrap();
    release.send(()).unwrap();
    cancel.cancel();
    drop(session);
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.contains("cancelled before acceptance"));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    assert!(
        !lock(document.session())
            .document_close_review()
            .inspection()["closing"]
            .as_bool()
            .unwrap()
    );
    let (_ticket, receipt) = observed_output
        .reserve(RequestId::Number(34), None, TerminalAttempt::new().unwrap())
        .unwrap();
    assert_eq!(receipt.outcome(), None);
    drop(receipt);
    owner.shutdown().unwrap();
    bounded(running.cancel()).await.unwrap();
}

#[tokio::test]
async fn abandoned_handler_and_output_still_complete_the_accepted_owned_cleanup() {
    let (_directory, document, mut server) = fixture();
    let (entered, observed) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let wait = Mutex::new(wait);
    let retired = Arc::new(AtomicBool::new(false));
    let retiring = retired.clone();
    let ui = TerminalUi::new(
        move |_| {
            entered.send(()).map_err(|error| error.to_string())?;
            wait.lock()
                .unwrap()
                .recv_timeout(TIMEOUT)
                .map_err(|error| error.to_string())?;
            retiring.store(true, Ordering::Release);
            Ok(())
        },
        || Ok(()),
    );
    let mut owner = TerminalOwner::start(document.clone(), Some(ui)).unwrap();
    server = server.with_terminal_close(owner.handle()).unwrap();
    let (output, observed_output) = stdio::transport(tokio::io::empty(), tokio::io::sink());
    server.stdio_output = Some(observed_output);
    let (service_io, _peer) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), service_io, None);
    let context = RequestContext::new(RequestId::Number(32), running.peer().clone());
    let token = document.document_close_review().unwrap().token();
    let request =
        tokio::spawn(async move { server.call_tool(close_request(token, false), context).await });
    bounded(tokio::task::spawn_blocking(move || {
        observed.recv_timeout(TIMEOUT)
    }))
    .await
    .unwrap()
    .unwrap();
    request.abort();
    let _ = bounded(request).await;
    drop(output);
    release.send(()).unwrap();
    let error = owner.shutdown().unwrap_err();
    assert!(error.contains("stdio") || error.contains("receiver"));
    assert!(retired.load(Ordering::Acquire));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert!(document.admit().is_err());
    bounded(running.cancel()).await.unwrap();
}

async fn write_message(writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bounded(writer.write_all(&bytes)).await.unwrap();
}

async fn read_message(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
) -> Value {
    let mut line = String::new();
    assert!(bounded(reader.read_line(&mut line)).await.unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}

async fn map_token(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
    id: u32,
) -> String {
    write_message(writer, json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"get_map_info","arguments":{}}})).await;
    let response = read_message(reader).await;
    let value: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    value["document_close"]["review_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn real_stdio_close_retries_busy_without_consuming_fence_and_exits_with_input_open() {
    let (_directory, document, server) = fixture();
    let mut owner = TerminalOwner::start(document.clone(), None).unwrap();
    let server = server.with_terminal_close(owner.handle()).unwrap();
    let (service, peer) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(service);
    let service = tokio::spawn(run_stdio(server, read, write));
    let (read, mut write) = tokio::io::split(peer);
    let mut read = BufReader::new(read);
    write_message(&mut write, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"terminal-close-test","version":"1"}}})).await;
    assert!(read_message(&mut read).await["result"].is_object());
    write_message(
        &mut write,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    let token = map_token(&mut read, &mut write, 2).await;
    let operation = document.admit().unwrap();
    write_message(&mut write, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":TOOL_NAME,"arguments":{"review_token":token}}})).await;
    let busy = read_message(&mut read).await;
    assert_eq!(busy["result"]["isError"], true);
    assert!(
        busy["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    drop(operation);
    let token = map_token(&mut read, &mut write, 4).await;
    write_message(&mut write, json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":TOOL_NAME,"arguments":{"review_token":token}}})).await;
    let closed = read_message(&mut read).await;
    assert_eq!(closed["id"], 5);
    assert_eq!(closed["result"]["isError"], false);
    assert_eq!(closed["result"]["structuredContent"]["closed"], true);
    assert_eq!(closed["result"]["structuredContent"]["ui"], "headless");
    assert!(closed["result"]["_meta"]["io.hammerr/terminal-attempt"].is_string());
    bounded(service).await.unwrap().unwrap();
    owner.shutdown().unwrap();
    assert_eq!(owner.handle().result(), Some(Ok(())));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    drop(write);
}
