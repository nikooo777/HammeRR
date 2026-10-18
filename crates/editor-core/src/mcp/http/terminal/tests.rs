use super::*;
use axum::http::Request as HttpRequest;
use futures_util::{future::poll_fn, stream};
use rmcp::handler::server::router::tool::ToolRoute;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{ContentBlock, Tool};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};

use crate::mcp::EditorServer;
use crate::session::Session;

const TIMEOUT: Duration = Duration::from_secs(5);

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("HTTP witness fixture timed out")
}

fn result() -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text("retired exact document")]);
    result.structured_content =
        Some(json!({"completed":true,"values":[1.0000000000000002,1.2345678901234567,1.0e-99]}));
    result
}

fn fixture() -> (Witness, impl FnOnce(Result<(), String>)) {
    let (connection, finish) = ConnectionReceipt::test_pair();
    (
        Witness(Arc::new(Shared {
            connection,
            state: Mutex::new(State::default()),
        })),
        finish,
    )
}

fn reserve(witness: &Witness) -> (HttpTerminalResponse, TerminalResponseReceipt) {
    HttpTerminalResponse::reserve_for(
        witness.clone(),
        RequestId::Number(7),
        Some(ProtocolVersion::V_2026_07_28),
        TerminalAttempt::new().unwrap(),
    )
    .unwrap()
}

fn message(result: CallToolResult) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::response(ServerResult::CallToolResult(result), RequestId::Number(7))
}

fn sse(result: CallToolResult) -> Bytes {
    sse_stream::Sse::default()
        .data(serde_json::to_string(&message(result)).unwrap())
        .into()
}

async fn drain(body: &mut ObservedBody) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await {
        if let Ok(bytes) = frame.unwrap().into_data() {
            output.extend_from_slice(&bytes);
        }
    }
    output
}

#[tokio::test]
async fn exact_typed_sse_and_json_need_same_body_end_and_connection_success() {
    for json_response in [false, true] {
        let (witness, finish) = fixture();
        let (ticket, receipt) = reserve(&witness);
        let marked = ticket.bind(result()).unwrap();
        let bytes = if json_response {
            Bytes::from(serde_json::to_vec(&message(marked)).unwrap())
        } else {
            sse(marked)
        };
        let expected = bytes.clone();
        let chunks = bytes
            .chunks(3)
            .map(Bytes::copy_from_slice)
            .map(Ok::<_, std::io::Error>)
            .collect::<Vec<_>>();
        let mut body = ObservedBody::new(
            Body::from_stream(stream::iter(chunks)),
            witness,
            if json_response { Kind::Json } else { Kind::Sse },
        );
        assert_eq!(drain(&mut body).await, expected);
        drop(body);
        assert_eq!(receipt.result(), None);
        finish(Ok(()));
        assert_eq!(receipt.wait().await, Ok(()));
        assert_eq!(receipt.wait().await, Ok(()));
    }
}

#[test]
fn shared_marker_normalizes_protocol_and_distinguishes_numeric_from_string_ids() {
    let attempt = TerminalAttempt(9_007_199_254_740_993);
    let modern = attempt.mark_result(Some(&ProtocolVersion::V_2026_07_28), result());
    let legacy = attempt.mark_result(Some(&ProtocolVersion::V_2025_03_26), result());
    assert_eq!(
        modern.meta.as_ref().unwrap()[ATTEMPT_META],
        "9007199254740993"
    );
    assert!(modern.result_type.as_ref().unwrap().is_complete());
    assert_eq!(legacy.result_type, None);
    assert_eq!(modern.content, legacy.content);
    let string_id = RequestId::String("7".into());
    assert!(!TerminalAttempt::matches_result(
        &string_id,
        &modern,
        &message(modern.clone())
    ));
    let matching = ServerJsonRpcMessage::response(
        ServerResult::CallToolResult(modern.clone()),
        string_id.clone(),
    );
    assert!(TerminalAttempt::matches_result(
        &string_id, &modern, &matching
    ));
    assert!(!TerminalAttempt::matches_result(
        &RequestId::Number(7),
        &modern,
        &matching
    ));
}

#[tokio::test]
async fn late_reserve_skips_complete_priming_without_decoding_ordinary_output() {
    let (witness, finish) = fixture();
    let (send, receive) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let input = stream::unfold(receive, |mut receive| async {
        receive.recv().await.map(|bytes| (bytes, receive))
    });
    let mut body = ObservedBody::new(Body::from_stream(input), witness.clone(), Kind::Sse);
    let prime: Bytes = sse_stream::Sse::default()
        .data("")
        .id("old-prime")
        .retry(1000)
        .into();
    send.send(Ok(prime.clone())).await.unwrap();
    let returned = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert_eq!(returned, prime);
    assert!(body.decoder.is_none());
    assert_eq!((body.bytes, body.frames, body.events), (0, 0, 0));
    let (ticket, receipt) = reserve(&witness);
    let marked = ticket.bind(result()).unwrap();
    let expected = sse(marked);
    send.send(Ok(expected.clone())).await.unwrap();
    drop(send);
    assert_eq!(drain(&mut body).await, expected);
    finish(Ok(()));
    assert_eq!(receipt.wait().await, Ok(()));
}

#[tokio::test]
async fn missing_wrong_id_attempt_or_outcome_never_uses_eof_as_success() {
    for wrong in 0..4 {
        let (witness, finish) = fixture();
        let (ticket, receipt) = reserve(&witness);
        let mut marked = ticket.bind(result()).unwrap();
        let mut id = RequestId::Number(7);
        match wrong {
            0 => {
                id = RequestId::Number(8);
            }
            1 => {
                marked
                    .meta
                    .as_mut()
                    .unwrap()
                    .insert(ATTEMPT_META.into(), json!(0));
            }
            2 => {
                marked.content = vec![ContentBlock::text("different outcome")];
            }
            _ => {}
        }
        let bytes = if wrong == 3 {
            Bytes::from_static(b": no final result\n\n")
        } else {
            sse_stream::Sse::default()
                .data(
                    serde_json::to_string(&ServerJsonRpcMessage::response(
                        ServerResult::CallToolResult(marked),
                        id,
                    ))
                    .unwrap(),
                )
                .into()
        };
        let mut body = ObservedBody::new(Body::from(bytes), witness, Kind::Sse);
        drain(&mut body).await;
        finish(Ok(()));
        let error = receipt.wait().await.unwrap_err();
        assert_eq!(receipt.result(), Some(Err(error.clone())));
        assert_eq!(receipt.wait().await, Err(error));
    }
}

#[tokio::test]
async fn decoder_byte_event_empty_frame_body_and_drop_failures_remain_sticky() {
    for failure in 0..7 {
        let (witness, finish) = fixture();
        let (ticket, receipt) = reserve(&witness);
        let marked = ticket.bind(result()).unwrap();
        let chunks = match failure {
            0 => vec![Ok(Bytes::from_static(b"data: invalid json\n\n"))],
            1 => vec![Ok(Bytes::from(vec![b'x'; MAX_BYTES + 1]))],
            2 => (0..=MAX_EVENTS)
                .map(|_| Ok(Bytes::from_static(b"data:\n\n")))
                .collect(),
            3 => (0..=MAX_FRAMES).map(|_| Ok(Bytes::new())).collect(),
            4 => vec![Err(std::io::Error::other("injected body failure"))],
            _ => vec![Ok(sse(marked))],
        };
        let mut body =
            ObservedBody::new(Body::from_stream(stream::iter(chunks)), witness, Kind::Sse);
        if failure < 4 {
            drain(&mut body).await;
        }
        if failure == 4 {
            assert!(
                poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        if failure == 6 {
            let _ = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
        }
        drop(body);
        finish(Ok(()));
        let error = receipt.wait().await.unwrap_err();
        assert_eq!(receipt.wait().await, Err(error));
    }
}

#[tokio::test]
async fn abandoned_ticket_unpolled_wait_timeout_and_late_flush_error_cannot_succeed() {
    let (witness, finish) = fixture();
    let (ticket, receipt) = reserve(&witness);
    drop(ticket);
    let error = receipt.result().unwrap().unwrap_err();
    finish(Ok(()));
    assert_eq!(receipt.wait().await, Err(error));

    let (witness, finish) = fixture();
    let (ticket, receipt) = reserve(&witness);
    let _marked = ticket.bind(result()).unwrap();
    drop(receipt.wait());
    assert_eq!(receipt.result(), Some(Err(INTERRUPTED.into())));
    finish(Ok(()));
    assert_eq!(receipt.wait().await, Err(INTERRUPTED.into()));

    let (witness, finish) = fixture();
    let (ticket, receipt) = reserve(&witness);
    let marked = ticket.bind(result()).unwrap();
    let mut body = ObservedBody::new(Body::from(sse(marked)), witness, Kind::Sse);
    drain(&mut body).await;
    let error = receipt
        .wait_with_grace(Duration::from_millis(1))
        .await
        .unwrap_err();
    finish(Ok(()));
    assert_eq!(receipt.wait().await, Err(error));

    let (witness, finish) = fixture();
    let (ticket, receipt) = reserve(&witness);
    let marked = ticket.bind(result()).unwrap();
    let mut body = ObservedBody::new(Body::from(sse(marked)), witness, Kind::Sse);
    drain(&mut body).await;
    finish(Err("late flush failed".into()));
    assert_eq!(receipt.wait().await, Err("late flush failed".into()));
}

fn server(
    receipts: mpsc::UnboundedSender<TerminalResponseReceipt>,
    observed: Option<Arc<Mutex<Option<TerminalResponseReceipt>>>>,
) -> EditorServer {
    let mut server = EditorServer::new(
        Arc::new(Mutex::new(Session::new(Err("no assets".into())))),
        None,
    );
    server.tool_router.add_route(ToolRoute::new_dyn(
        Tool::new("terminal_witness", "test result witness", Arc::default()),
        move |context: ToolCallContext<'_, EditorServer>| {
            let receipts = receipts.clone();
            let observed = observed.clone();
            Box::pin(async move {
                let (ticket, receipt) = HttpTerminalResponse::reserve(
                    &context.request_context,
                    TerminalAttempt::new().unwrap(),
                )
                .unwrap();
                if let Some(observed) = observed {
                    *observed.lock().unwrap() = Some(receipt.clone());
                }
                receipts.send(receipt).unwrap();
                Ok(ticket.bind(result()).unwrap().into())
            })
        },
    ));
    server
}

fn request(mut value: Value, session: Option<&str>) -> HttpRequest<Body> {
    let modern = session.is_none() && value["method"] == "tools/call";
    if modern {
        value["params"]["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"terminal-witness","version":"1"}});
    }
    let mut request = HttpRequest::builder()
        .method("POST")
        .uri("http://localhost/mcp")
        .header("Host", "localhost")
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header(
            "MCP-Protocol-Version",
            if modern { "2026-07-28" } else { "2025-03-26" },
        );
    if modern {
        request = request
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "terminal_witness");
    }
    if let Some(session) = session {
        request = request.header("Mcp-Session-Id", session);
    }
    request.body(Body::from(value.to_string())).unwrap()
}

#[tokio::test]
async fn actual_rmcp_legacy_sse_stateless_sse_and_json_preserve_response_and_flush() {
    for (legacy, json_response) in [(true, false), (false, false), (false, true)] {
        let (sent, mut received) = mpsc::unbounded_channel();
        let config = StreamableHttpServerConfig::default().with_json_response(json_response);
        let cancellation = config.cancellation_token.clone();
        let sessions = Arc::new(LocalSessionManager::default());
        let server = server(sent, None);
        let service = StreamableHttpService::new(move || Ok(server.clone()), sessions, config);
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn(observe));
        let (io, client) = tokio::io::duplex(64 * 1024);
        let (_shutdown, shutdown) = watch::channel(());
        let driver = tokio::spawn(super::super::connection::serve(io, router, shutdown));
        let (mut client, connection) = bounded(hyper::client::conn::http1::handshake(
            hyper_util::rt::TokioIo::new(client),
        ))
        .await
        .unwrap();
        let client_driver = tokio::spawn(connection);
        let session = if legacy {
            let response = tokio::time::timeout(TIMEOUT, client.send_request(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"terminal-witness","version":"1"}}}), None))).await.unwrap().unwrap();
            let session = response.headers()["mcp-session-id"]
                .to_str()
                .unwrap()
                .to_owned();
            bounded(axum::body::to_bytes(
                Body::new(response.into_body()),
                MAX_BYTES,
            ))
            .await
            .unwrap();
            let response = bounded(client.send_request(request(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&session),
            )))
            .await
            .unwrap();
            bounded(axum::body::to_bytes(
                Body::new(response.into_body()),
                MAX_BYTES,
            ))
            .await
            .unwrap();
            Some(session)
        } else {
            None
        };
        let response = tokio::time::timeout(TIMEOUT, client.send_request(request(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"terminal_witness","arguments":{}}}), session.as_deref()))).await.unwrap().unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            if json_response {
                "application/json"
            } else {
                "text/event-stream"
            }
        );
        let receipt = tokio::time::timeout(TIMEOUT, received.recv())
            .await
            .unwrap()
            .unwrap();
        let output = tokio::time::timeout(
            TIMEOUT,
            axum::body::to_bytes(Body::new(response.into_body()), MAX_BYTES),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!output.is_empty());
        assert_eq!(receipt.result(), None);
        assert_eq!(
            tokio::time::timeout(TIMEOUT, receipt.wait()).await.unwrap(),
            Ok(())
        );
        assert_eq!(receipt.wait().await, Ok(()));
        cancellation.cancel();
        tokio::time::timeout(TIMEOUT, driver)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(TIMEOUT, client_driver)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

struct FailFlush<I> {
    io: I,
    release: oneshot::Receiver<()>,
    observed: Arc<Mutex<Option<TerminalResponseReceipt>>>,
}

impl<I: AsyncRead + Unpin> AsyncRead for FailFlush<I> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buffer)
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for FailFlush<I> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let ended = self
            .observed
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|receipt| matches!(receipt.body.borrow().as_ref(), Some(Ok(()))));
        if !ended {
            return Pin::new(&mut self.io).poll_flush(cx);
        }
        match Pin::new(&mut self.release).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => Poll::Ready(Err(std::io::Error::other(
                "injected terminal flush failure",
            ))),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn actual_terminal_body_end_does_not_acknowledge_a_late_socket_flush_error() {
    let (sent, mut received) = mpsc::unbounded_channel();
    let observed = Arc::new(Mutex::new(None));
    let server = server(sent, Some(observed.clone()));
    let config = StreamableHttpServerConfig::default();
    let cancellation = config.cancellation_token.clone();
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(observe));
    let (io, client) = tokio::io::duplex(64 * 1024);
    let (release, released) = oneshot::channel();
    let io = FailFlush {
        io,
        release: released,
        observed,
    };
    let (_shutdown, shutdown) = watch::channel(());
    let driver = tokio::spawn(super::super::connection::serve(io, router, shutdown));
    let (mut client, connection) = bounded(hyper::client::conn::http1::handshake(
        hyper_util::rt::TokioIo::new(client),
    ))
    .await
    .unwrap();
    let client_driver = tokio::spawn(connection);
    let response = tokio::time::timeout(TIMEOUT, client.send_request(request(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"terminal_witness","arguments":{}}}), None))).await.unwrap().unwrap();
    let receipt = tokio::time::timeout(TIMEOUT, received.recv())
        .await
        .unwrap()
        .unwrap();
    let mut body = receipt.body.clone();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            if body.borrow().is_some() {
                break;
            }
            body.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(*body.borrow(), Some(Ok(())));
    assert_eq!(receipt.result(), None);
    let waiting = tokio::spawn(receipt.wait());
    release.send(()).unwrap();
    let error = tokio::time::timeout(TIMEOUT, waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("injected terminal flush failure"), "{error}");
    assert_eq!(receipt.result(), Some(Err(error.clone())));
    assert_eq!(receipt.wait().await, Err(error));
    cancellation.cancel();
    tokio::time::timeout(TIMEOUT, driver)
        .await
        .unwrap()
        .unwrap();
    drop(response);
    drop(client);
    client_driver.abort();
    let _ = bounded(client_driver).await;
}

#[test]
fn runtime_drop_of_an_unpolled_receipt_wait_is_an_interrupted_result() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (witness, finish) = fixture();
    let (ticket, receipt) = reserve(&witness);
    let _marked = ticket.bind(result()).unwrap();
    let task = runtime.spawn(receipt.wait());
    drop(runtime);
    assert_eq!(receipt.result(), Some(Err(INTERRUPTED.into())));
    drop(task);
    finish(Ok(()));
}
