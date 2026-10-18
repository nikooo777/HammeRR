use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::sync::{Mutex, mpsc};

use rmcp::handler::server::router::tool::ToolRoute;
use serde_json::{Value, json};

use super::*;
use crate::mcp::ToolResult;
use crate::runtime::{DocumentHandle, lock};
use crate::session::Session;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_NETWORK_GRACE: Duration = Duration::from_millis(50);

fn address() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

fn fixture() -> (tempfile::TempDir, DocumentHandle, EditorServer) {
    let directory = tempfile::tempdir().unwrap();
    let mut state = Session::new(Err("no assets required".into()));
    state.configure_recovery(Ok(directory.path().join("recovery")));
    let document = DocumentHandle::start(Arc::new(Mutex::new(state))).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    (directory, document, server)
}

fn connection(address: SocketAddr) -> BufReader<TcpStream> {
    let stream = TcpStream::connect_timeout(&address, CLIENT_TIMEOUT).unwrap();
    stream.set_read_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    BufReader::new(stream)
}

fn request(
    address: SocketAddr,
    method: &str,
    session: Option<&str>,
    body: Option<Value>,
) -> BufReader<TcpStream> {
    let mut client = connection(address);
    let body = body.map_or(String::new(), |body| body.to_string());
    let session = session.map_or(String::new(), |session| {
        format!("Mcp-Session-Id: {session}\r\n")
    });
    write!(
        client.get_mut(),
        "{method} /mcp HTTP/1.1\r\nHost: {address}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nMCP-Protocol-Version: 2025-03-26\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    client
}

fn headers(client: &mut BufReader<TcpStream>) -> (u16, BTreeMap<String, String>) {
    let mut line = String::new();
    client.read_line(&mut line).unwrap();
    let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = BTreeMap::new();
    loop {
        line.clear();
        client.read_line(&mut line).unwrap();
        if line == "\r\n" {
            return (status, headers);
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
}

fn body(client: &mut BufReader<TcpStream>, headers: &BTreeMap<String, String>) -> Vec<u8> {
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        let mut body = Vec::new();
        loop {
            let mut size = String::new();
            client.read_line(&mut size).unwrap();
            let size = usize::from_str_radix(size.trim().split(';').next().unwrap(), 16).unwrap();
            if size == 0 {
                let mut trailer = String::new();
                loop {
                    trailer.clear();
                    client.read_line(&mut trailer).unwrap();
                    if trailer == "\r\n" {
                        return body;
                    }
                }
            }
            let offset = body.len();
            body.resize(offset + size, 0);
            client.read_exact(&mut body[offset..]).unwrap();
            let mut end = [0; 2];
            client.read_exact(&mut end).unwrap();
            assert_eq!(end, *b"\r\n");
        }
    }
    let mut body = Vec::new();
    client.read_to_end(&mut body).unwrap();
    if let Some(length) = headers.get("content-length") {
        assert_eq!(body.len(), length.parse::<usize>().unwrap());
    }
    body
}

fn response(client: &mut BufReader<TcpStream>) -> (BTreeMap<String, String>, Vec<u8>) {
    let (status, headers) = headers(client);
    assert_eq!(status, 200);
    let body = body(client, &headers);
    (headers, body)
}

fn message(body: &[u8]) -> Value {
    let body = std::str::from_utf8(body).unwrap();
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value.get("result").is_some() || value.get("error").is_some())
        .unwrap_or_else(|| panic!("MCP response missing from {body}"))
}

fn initialize(address: SocketAddr) -> String {
    let mut client = request(
        address,
        "POST",
        None,
        Some(json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{
                "protocolVersion":"2025-03-26", "capabilities":{},
                "clientInfo":{"name":"http-ownership-test", "version":"1"}
            }
        })),
    );
    let (initialize_headers, initialize_body) = response(&mut client);
    assert_eq!(
        message(&initialize_body)["result"]["serverInfo"]["name"],
        "hammerr"
    );
    let session = initialize_headers["mcp-session-id"].clone();
    let mut initialized = request(
        address,
        "POST",
        Some(&session),
        Some(json!({"jsonrpc":"2.0", "method":"notifications/initialized"})),
    );
    let (status, headers) = headers(&mut initialized);
    assert_eq!(status, 202);
    assert!(body(&mut initialized, &headers).is_empty());
    session
}

fn call(address: SocketAddr, session: &str, name: &str) -> BufReader<TcpStream> {
    request(
        address,
        "POST",
        Some(session),
        Some(json!({
            "jsonrpc":"2.0", "id":2, "method":"tools/call",
            "params":{"name":name,"arguments":{}}
        })),
    )
}

#[test]
fn shutdown_releases_idle_sse_sessions_and_transport_caches_then_rebinds() {
    let (_directory, document, server) = fixture();
    let before = lock(document.session()).history_snapshot();
    let weak_session = Arc::downgrade(document.session());
    let weak_offscreen = Arc::downgrade(&server.offscreen);
    let weak_models = Arc::downgrade(&server.model_renderer);
    let mut http = spawn_http(server.clone(), address()).unwrap();
    let address = http.local_addr();
    let session = initialize(address);
    let mut info = call(address, &session, "get_map_info");
    let (_, returned) = response(&mut info);
    let returned = message(&returned);
    assert!(returned["error"].is_null(), "{returned}");
    assert!(returned["result"]["content"][0]["text"].is_string());
    assert_ne!(returned["result"]["isError"], true);
    assert_eq!(document.active_operations(), 0);
    let mut events = request(address, "GET", Some(&session), None);
    let (status, headers) = headers(&mut events);
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "text/event-stream");
    http.stop();
    http.stop();
    http.shutdown().unwrap();
    http.shutdown().unwrap();
    let _ = body(&mut events, &headers);
    assert_eq!(Arc::strong_count(&server.offscreen), 1);
    assert_eq!(Arc::strong_count(&server.model_renderer), 1);
    assert_eq!(lock(document.session()).history_snapshot(), before);
    let rebound = spawn_http(server.clone(), address).unwrap();
    drop(rebound);
    assert_eq!(Arc::strong_count(&server.offscreen), 1);
    drop(server);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    assert!(weak_offscreen.upgrade().is_none());
    assert!(weak_models.upgrade().is_none());
}

#[test]
fn shutdown_waits_for_admitted_blocking_work_before_last_document_owner_release() {
    let (_directory, document, mut server) = fixture();
    let weak_session = Arc::downgrade(document.session());
    let weak_offscreen = Arc::downgrade(&server.offscreen);
    let weak_models = Arc::downgrade(&server.model_renderer);
    let (started, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    server.tool_router.add_route(ToolRoute::new_dyn(
        rmcp::model::Tool::new("held_worker", "hold actual admitted work", Arc::default()),
        move |context: rmcp::handler::server::tool::ToolCallContext<'_, EditorServer>| {
            let started = started.clone();
            let released = released.clone();
            let worker = context.service.worker(move || {
                started.send(()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(CLIENT_TIMEOUT)
                    .unwrap();
            });
            Box::pin(async move {
                tokio::task::spawn_blocking(worker).await.unwrap();
                let result: ToolResult = super::super::ok(json!({"finished":true}));
                result.map(Into::into)
            })
        },
    ));
    let mut http = spawn_http(server, address()).unwrap();
    let session = initialize(http.local_addr());
    let mut client = call(http.local_addr(), &session, "held_worker");
    entered.recv_timeout(CLIENT_TIMEOUT).unwrap();
    assert_eq!(document.active_operations(), 1);
    drop(document);
    let (finished, done) = mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        let first = http.shutdown();
        let second = http.shutdown();
        finished.send((first, second)).unwrap();
    });
    let _ = response(&mut client);
    assert!(done.try_recv().is_err());
    assert!(weak_session.upgrade().is_some());
    assert!(weak_offscreen.upgrade().is_some());
    assert!(weak_models.upgrade().is_some());
    release.send(()).unwrap();
    let (first, second) = done.recv_timeout(CLIENT_TIMEOUT).unwrap();
    assert_eq!(first, Ok(()));
    assert_eq!(second, first);
    shutdown.join().unwrap();
    assert!(weak_session.upgrade().is_none());
    assert!(weak_offscreen.upgrade().is_none());
    assert!(weak_models.upgrade().is_none());
}

#[test]
fn stalled_request_body_reports_memoized_timeout_and_releases_last_transport_owner() {
    let (_directory, document, server) = fixture();
    let weak_session = Arc::downgrade(document.session());
    let weak_offscreen = Arc::downgrade(&server.offscreen);
    let mut http = spawn(server, address(), TEST_NETWORK_GRACE).unwrap();
    let address = http.local_addr();
    let mut client = connection(address);
    write!(
        client.get_mut(),
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nExpect: 100-continue\r\n\r\n"
    )
    .unwrap();
    let (status, _) = headers(&mut client);
    assert_eq!(status, 100);
    client.get_mut().write_all(b"{").unwrap();
    drop(document);
    let error = http.shutdown().unwrap_err();
    assert_eq!(error, "MCP HTTP network shutdown exceeded 50 ms");
    assert_eq!(http.shutdown(), Err(error.clone()));
    assert!(weak_session.upgrade().is_none());
    assert!(weak_offscreen.upgrade().is_none());
    let error = http
        .finish(Err("primary and application drain failure".into()))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "primary and application drain failure; MCP HTTP shutdown failed: MCP HTTP network shutdown exceeded 50 ms"
    );
    let (_directory, document, server) = fixture();
    let mut rebound = spawn_http(server, address).unwrap();
    rebound.shutdown().unwrap();
    drop(document);
}

#[test]
fn operation_bearing_transport_anchor_is_rejected_without_pinning_admission() {
    let (_directory, document, server) = fixture();
    let admitted = server.admitted().unwrap();
    assert_eq!(document.active_operations(), 1);
    let error = spawn_http(admitted, address()).err().unwrap();
    assert_eq!(
        error,
        "MCP HTTP requires an idle server without document admission"
    );
    assert_eq!(document.active_operations(), 0);
    document.shutdown().unwrap();
}

mod driver;
