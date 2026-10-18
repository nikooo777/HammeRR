use std::io;
use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};

use super::*;

const TIMEOUT: Duration = Duration::from_secs(5);
const BODY: &str = "observed-complete-response-body";

async fn response(
    axum::Extension(receipt): axum::Extension<ConnectionReceipt>,
    State(receipts): State<mpsc::Sender<ConnectionReceipt>>,
) -> &'static str {
    receipts.send(receipt).unwrap();
    BODY
}

fn router(receipts: mpsc::Sender<ConnectionReceipt>) -> axum::Router {
    axum::Router::new()
        .route("/", get(response))
        .with_state(receipts)
}

async fn read_body(client: &mut tokio::io::DuplexStream) {
    let mut response = Vec::new();
    let mut bytes = [0; 1024];
    while !response.ends_with(BODY.as_bytes()) {
        let read = client.read(&mut bytes).await.unwrap();
        assert_ne!(read, 0, "response ended before its body");
        response.extend_from_slice(&bytes[..read]);
        assert!(response.len() < 4096);
    }
}

struct FailFlush<I> {
    io: I,
    written: Vec<u8>,
    release: oneshot::Receiver<()>,
}

impl<I: AsyncRead + Unpin> AsyncRead for FailFlush<I> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buffer)
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for FailFlush<I> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.io).poll_write(cx, bytes) {
            Poll::Ready(Ok(written)) => {
                self.written.extend_from_slice(&bytes[..written]);
                assert!(self.written.len() < 4096);
                Poll::Ready(Ok(written))
            }
            result => result,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.written.ends_with(BODY.as_bytes()) {
            return Pin::new(&mut self.io).poll_flush(cx);
        }
        match Pin::new(&mut self.release).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected late response flush failure",
            ))),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn receipt_waits_for_actual_flush_and_memoizes_late_connection_error() {
    let (server, mut client) = tokio::io::duplex(4096);
    let (release, released) = oneshot::channel();
    let io = FailFlush {
        io: server,
        written: Vec::new(),
        release: released,
    };
    let (_shutdown, shutdown_requested) = watch::channel(());
    let (receipts, receive) = mpsc::channel();
    let driver = tokio::spawn(serve(io, router(receipts), shutdown_requested));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(TIMEOUT, read_body(&mut client))
        .await
        .unwrap();
    let receipt = receive.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(receipt.result(), None);
    receipt.request_graceful();
    release.send(()).unwrap();
    tokio::time::timeout(TIMEOUT, driver)
        .await
        .unwrap()
        .unwrap();
    let result = receipt.wait().await;
    assert!(
        result
            .as_ref()
            .unwrap_err()
            .contains("injected late response flush failure"),
        "{result:?}"
    );
    assert_eq!(receipt.result(), Some(result.clone()));
    assert_eq!(receipt.wait().await, result);
    receipt.request_graceful();
    assert_eq!(receipt.result(), Some(result));
}

#[test]
fn cancellation_before_first_poll_and_runtime_drop_resolve_interrupted_receipts() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (receipt, completion, requested) = ConnectionReceipt::new();
    let (_shutdown, shutdown_requested) = watch::channel(());
    let (server, _client) = tokio::io::duplex(4096);
    let (receipts, _receive) = mpsc::channel();
    let driver = runtime.spawn(drive(
        server,
        router(receipts),
        shutdown_requested,
        requested,
        completion,
    ));
    driver.abort();
    assert!(runtime.block_on(driver).unwrap_err().is_cancelled());
    assert_eq!(receipt.result(), Some(Err(INTERRUPTED.into())));
    assert_eq!(runtime.block_on(receipt.wait()), Err(INTERRUPTED.into()));
    drop(runtime);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (server, mut client) = tokio::io::duplex(4096);
    let (_shutdown, shutdown_requested) = watch::channel(());
    let (receipts, receive) = mpsc::channel();
    let driver = runtime.spawn(serve(server, router(receipts), shutdown_requested));
    runtime.block_on(async {
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        tokio::time::timeout(TIMEOUT, read_body(&mut client))
            .await
            .unwrap();
    });
    let receipt = receive.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(receipt.result(), None);
    assert!(!driver.is_finished());
    drop(runtime);
    assert_eq!(receipt.result(), Some(Err(INTERRUPTED.into())));
    receipt.request_graceful();
    assert_eq!(receipt.result(), Some(Err(INTERRUPTED.into())));
    drop(client);
}
