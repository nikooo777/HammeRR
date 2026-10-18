use std::net::Shutdown;

use rmcp::handler::server::tool::ToolCallContext;

use super::*;

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
