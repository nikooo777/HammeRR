use std::net::Shutdown;

use rmcp::handler::server::tool::ToolCallContext;

use super::*;

const COMPOSED_CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
const COMPOSED_CLOSE_MAX_BYTES: usize = 256 * 1024;

async fn close_bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(COMPOSED_CLOSE_TIMEOUT, future)
        .await
        .expect("Composed HTTP Close fixture timed out")
}

fn close_request(
    address: SocketAddr,
    value: Value,
    session: Option<&str>,
    close: bool,
) -> axum::http::Request<axum::body::Body> {
    let mut request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/mcp")
        .header(axum::http::header::HOST, address.to_string())
        .header(
            axum::http::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("MCP-Protocol-Version", "2025-03-26");
    if let Some(session) = session {
        request = request.header("Mcp-Session-Id", session);
    }
    if close {
        request = request.header(axum::http::header::CONNECTION, "close");
    }
    request
        .body(axum::body::Body::from(value.to_string()))
        .unwrap()
}

async fn close_response(response: hyper::Response<hyper::body::Incoming>) -> Value {
    use futures_util::StreamExt;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let json = response.headers()[axum::http::header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("application/json");
    let body = close_bounded(axum::body::to_bytes(
        axum::body::Body::new(response.into_body()),
        COMPOSED_CLOSE_MAX_BYTES,
    ))
    .await
    .unwrap();
    if json {
        return serde_json::from_slice(&body).unwrap();
    }
    let mut events = Box::pin(sse_stream::SseStream::new(axum::body::Body::from(body)));
    while let Some(event) = close_bounded(events.next()).await {
        if let Some(data) = event.unwrap().data.filter(|data| !data.is_empty()) {
            let message: Value = serde_json::from_str(&data).unwrap();
            if message.get("result").is_some() || message.get("error").is_some() {
                return message;
            }
        }
    }
    panic!("The production HTTP response omitted its JSON-RPC result")
}

#[test]
fn production_http_close_uses_fresh_review_and_finishes_before_client_sender_drop() {
    let (_directory, document, server) = fixture();
    let mut owner = crate::mcp::TerminalOwner::start(document.clone(), None).unwrap();
    let terminal = owner.handle();
    let server = server.with_terminal_close(terminal.clone()).unwrap();
    let mut http = spawn_http(server, address()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
    let address = http.local_addr();
    let stream = close_bounded(tokio::net::TcpStream::connect(address))
        .await
        .unwrap();
    let (mut client, connection) = close_bounded(hyper::client::conn::http1::handshake(
        hyper_util::rt::TokioIo::new(stream),
    ))
    .await
    .unwrap();
    let driver = tokio::spawn(connection);
    close_bounded(client.ready()).await.unwrap();
    let response = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"production-http-close","version":"1"}}}),
        None, false,
    ))).await.unwrap();
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        close_response(response).await["result"]["serverInfo"]["name"],
        "hammerr"
    );
    close_bounded(client.ready()).await.unwrap();
    let initialized = close_bounded(client.send_request(close_request(
        address,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        Some(&session),
        false,
    )))
    .await
    .unwrap();
    assert_eq!(initialized.status(), axum::http::StatusCode::ACCEPTED);
    assert!(
        close_bounded(axum::body::to_bytes(
            axum::body::Body::new(initialized.into_body()),
            COMPOSED_CLOSE_MAX_BYTES
        ))
        .await
        .unwrap()
        .is_empty()
    );
    close_bounded(client.ready()).await.unwrap();
    let info = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_map_info","arguments":{}}}),
        Some(&session), false,
    ))).await.unwrap();
    let info = close_response(info).await;
    let info: Value =
        serde_json::from_str(info["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        info["document_close"],
        document.document_close_review().unwrap().inspection()
    );
    let token = info["document_close"]["review_token"].as_str().unwrap();
    close_bounded(client.ready()).await.unwrap();
    let response = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"close_document","arguments":{"review_token":token}}}),
        Some(&session), true,
    ))).await.unwrap();
    let closed = close_response(response).await;
    assert_eq!(closed["id"], 3);
    assert_eq!(closed["result"]["isError"], false);
    assert_eq!(closed["result"]["structuredContent"]["closed"], true);
    assert_eq!(closed["result"]["structuredContent"]["ui"], "headless");
    assert_eq!(
        closed["result"]["structuredContent"]["document_id"],
        info["document_close"]["document_id"]
    );
    assert!(closed["result"]["_meta"]["io.hammerr/terminal-attempt"].is_string());
    assert_eq!(
        closed["result"]["_meta"]["io.hammerr/terminal-attempt"],
        closed["result"]["structuredContent"]["attempt"]
    );
    owner.shutdown().unwrap();
    assert_eq!(terminal.result(), Some(Ok(())));
    assert_eq!(
        document.lifecycle(),
        crate::runtime::DocumentLifecycle::Finished
    );
    assert!(document.admit().is_err());
    close_bounded(driver).await.unwrap().unwrap();
    assert!(std::net::TcpStream::connect_timeout(&address, COMPOSED_CLOSE_TIMEOUT).is_ok());
    http.shutdown().unwrap();
    assert!(std::net::TcpStream::connect_timeout(&address, COMPOSED_CLOSE_TIMEOUT).is_err());
    assert!(client.is_closed());
    drop(client);
    });
}

#[test]
fn production_workspace_http_closes_peer_then_last_document_on_same_connection() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No assets required".into()),
        directory.path().join("recovery"),
    );
    let mut owner = crate::mcp::WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let workspace = owner.workspace();
    let first = application.workspace().documents()[0].clone();
    let document = first.document();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let created = runtime
        .block_on(close_bounded(
            workspace.new_document(crate::session::Actor::Agent, false),
        ))
        .unwrap();
    let second = created["document_id"].as_str().unwrap().to_owned();
    let mut http = super::super::spawn_workspace_http(owner.server(), address()).unwrap();
    runtime.block_on(async {
    let address = http.local_addr();
    let stream = close_bounded(tokio::net::TcpStream::connect(address))
        .await
        .unwrap();
    let (mut client, connection) = close_bounded(hyper::client::conn::http1::handshake(
        hyper_util::rt::TokioIo::new(stream),
    ))
    .await
    .unwrap();
    let driver = tokio::spawn(connection);
    close_bounded(client.ready()).await.unwrap();
    let response = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"production-http-close","version":"1"}}}),
        None, false,
    ))).await.unwrap();
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        close_response(response).await["result"]["serverInfo"]["name"],
        "hammerr"
    );
    close_bounded(client.ready()).await.unwrap();
    let initialized = close_bounded(client.send_request(close_request(
        address,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        Some(&session),
        false,
    )))
    .await
    .unwrap();
    assert_eq!(initialized.status(), axum::http::StatusCode::ACCEPTED);
    assert!(
        close_bounded(axum::body::to_bytes(
            axum::body::Body::new(initialized.into_body()),
            COMPOSED_CLOSE_MAX_BYTES
        ))
        .await
        .unwrap()
        .is_empty()
    );
    close_bounded(client.ready()).await.unwrap();
    let info = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_map_info","arguments":{"document_id":first.selector()}}}),
        Some(&session), false,
    ))).await.unwrap();
    let info = close_response(info).await;
    let info: Value =
        serde_json::from_str(info["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        info["document_close"],
        document.document_close_review().unwrap().inspection()
    );
    let token = info["document_close"]["review_token"].as_str().unwrap();
    close_bounded(client.ready()).await.unwrap();
    let response = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"close_document","arguments":{"document_id":first.selector(),"review_token":token}}}),
        Some(&session), false,
    ))).await.unwrap();
    let closed = close_response(response).await;
    assert_eq!(closed["id"], 3);
    assert_eq!(closed["result"]["isError"], false);
    assert_eq!(closed["result"]["structuredContent"]["closed"], true);
    assert_eq!(closed["result"]["structuredContent"]["ui"], "headless");
    assert_eq!(
        closed["result"]["structuredContent"]["document_id"],
        info["document_close"]["document_id"]
    );
    assert_eq!(closed["result"]["structuredContent"]["terminal"], false);
    assert!(
        closed["result"]["_meta"]
            .get("io.hammerr/terminal-attempt")
            .is_none()
    );
    assert_eq!(
        closed["result"]["_meta"]["io.hammerr/document"]["document_id"],
        first.selector()
    );
    assert_eq!(application.workspace().documents().len(), 1);
    close_bounded(client.ready()).await.unwrap();
    let remaining = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_map_info","arguments":{}}}), Some(&session), false,
    ))).await.unwrap();
    let remaining = close_response(remaining).await;
    assert_eq!(
        remaining["result"]["_meta"]["io.hammerr/document"]["document_id"],
        second
    );
    let remaining: Value =
        serde_json::from_str(remaining["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let token = remaining["document_close"]["review_token"]
        .as_str()
        .unwrap();
    close_bounded(client.ready()).await.unwrap();
    let response = close_bounded(client.send_request(close_request(address,
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"close_document","arguments":{"document_id":second,"review_token":token}}}), Some(&session), true,
    ))).await.unwrap();
    let closed = close_response(response).await;
    assert_eq!(closed["result"]["structuredContent"]["terminal"], true);
    assert_eq!(
        closed["result"]["_meta"]["io.hammerr/document"]["document_id"],
        second
    );
    assert!(closed["result"]["_meta"]["io.hammerr/terminal-attempt"].is_string());
    owner.shutdown().unwrap();
    assert!(application.workspace().documents().is_empty());
    assert_eq!(
        document.lifecycle(),
        crate::runtime::DocumentLifecycle::Finished
    );
    assert!(document.admit().is_err());
    close_bounded(driver).await.unwrap().unwrap();
    assert!(std::net::TcpStream::connect_timeout(&address, COMPOSED_CLOSE_TIMEOUT).is_ok());
    http.shutdown().unwrap();
    assert!(std::net::TcpStream::connect_timeout(&address, COMPOSED_CLOSE_TIMEOUT).is_err());
    assert!(client.is_closed());
    drop(client);
    application.finish(Ok(())).unwrap();
    });
}

#[test]
fn per_connection_receipt_reaches_mcp_parts_and_observes_graceful_completion() {
    let (_directory, _document, mut server) = fixture();
    let (receipts, receive) = mpsc::channel();
    server.tool_router.add_route(ToolRoute::new_dyn(
        rmcp::model::Tool::new("connection_receipt", "observe transport", Arc::default()),
        move |context: ToolCallContext<'_, EditorServer>| {
            let parts = context
                .request_context
                .extensions
                .get::<axum::http::request::Parts>()
                .unwrap();
            let receipt = parts.extensions.get::<ConnectionReceipt>().unwrap().clone();
            receipts.send(receipt).unwrap();
            Box::pin(async { super::super::super::ok(json!({"observed":true})).map(Into::into) })
        },
    ));
    let mut http = spawn_http(server, address()).unwrap();
    let address = http.local_addr();
    let session = initialize(address);
    let mut client = connection(address);
    for id in [2, 3] {
        let body = json!({
            "jsonrpc":"2.0", "id":id, "method":"tools/call",
            "params":{"name":"connection_receipt","arguments":{}}
        })
        .to_string();
        write!(
            client.get_mut(),
            "POST /mcp HTTP/1.1\r\nHost: {address}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nMCP-Protocol-Version: 2025-03-26\r\nMcp-Session-Id: {session}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let (_, returned) = response(&mut client);
        assert_eq!(message(&returned)["id"], id);
    }
    let first = receive.recv_timeout(CLIENT_TIMEOUT).unwrap();
    let second = receive.recv_timeout(CLIENT_TIMEOUT).unwrap();
    assert_eq!(first.result(), None);
    assert_eq!(second.result(), None);
    first.request_graceful();
    first.request_graceful();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(CLIENT_TIMEOUT, second.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.wait().await, Ok(()));
    });
    assert_eq!(first.result(), Some(Ok(())));
    assert_eq!(second.result(), Some(Ok(())));
    let mut ended = Vec::new();
    client.read_to_end(&mut ended).unwrap();
    assert!(ended.is_empty());
    http.shutdown().unwrap();
}

#[test]
fn incomplete_client_disconnect_stays_local_and_leaves_healthy_mcp_serving() {
    let (_directory, _document, server) = fixture();
    let mut http = spawn_http(server, address()).unwrap();
    let address = http.local_addr();
    let mut interrupted = connection(address);
    write!(
        interrupted.get_mut(),
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nExpect: 100-continue\r\n\r\n"
    )
    .unwrap();
    let (status, _) = headers(&mut interrupted);
    assert_eq!(status, 100);
    interrupted.get_mut().write_all(b"{").unwrap();
    interrupted.get_mut().shutdown(Shutdown::Both).unwrap();
    drop(interrupted);
    let session = initialize(address);
    let mut client = call(address, &session, "get_map_info");
    let (_, returned) = response(&mut client);
    let returned = message(&returned);
    assert!(returned["error"].is_null(), "{returned}");
    assert!(returned["result"]["content"][0]["text"].is_string());
    http.shutdown().unwrap();
    http.shutdown().unwrap();
}
