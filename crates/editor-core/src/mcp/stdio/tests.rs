use super::*;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Notify;

const TIMEOUT: Duration = Duration::from_secs(5);
const IO_CAPACITY: usize = 4096;

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("stdio fixture timed out")
}

#[derive(Default)]
struct FlushState {
    outcome: Option<Result<(), io::ErrorKind>>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct FlushGate {
    state: Mutex<FlushState>,
    entered: Notify,
}

impl FlushGate {
    fn release(&self, outcome: Result<(), io::ErrorKind>) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            state.outcome = Some(outcome);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    async fn entered(&self) {
        bounded(self.entered.notified()).await;
    }
}

struct GatedWriter {
    inner: DuplexStream,
    gate: Arc<FlushGate>,
}

impl AsyncWrite for GatedWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(context, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.gate.entered.notify_one();
        let outcome = {
            let mut state = this.gate.state.lock().unwrap();
            match state.outcome {
                Some(outcome) => outcome,
                None => {
                    state.waker = Some(context.waker().clone());
                    return Poll::Pending;
                }
            }
        };
        match outcome {
            Ok(()) => Pin::new(&mut this.inner).poll_flush(context),
            Err(kind) => Poll::Ready(Err(io::Error::from(kind))),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}

fn gated<R: AsyncRead + Send + Unpin>(
    read: R,
) -> (
    ObservedStdio<R, GatedWriter>,
    StdioOutput,
    Arc<FlushGate>,
    BufReader<DuplexStream>,
) {
    let (writer, peer) = tokio::io::duplex(IO_CAPACITY);
    let gate = Arc::new(FlushGate::default());
    let (transport, output) = transport(
        read,
        GatedWriter {
            inner: writer,
            gate: gate.clone(),
        },
    );
    (transport, output, gate, BufReader::new(peer))
}

fn response(id: RequestId) -> ServerJsonRpcMessage {
    JsonRpcMessage::response(
        rmcp::model::ServerResult::CallToolResult(rmcp::model::CallToolResult::success(vec![])),
        id,
    )
}

fn message(value: Value) -> ServerJsonRpcMessage {
    serde_json::from_value(value).unwrap()
}

async fn read_message(peer: &mut BufReader<DuplexStream>) -> Value {
    let mut line = String::new();
    let bytes = bounded(peer.read_line(&mut line)).await.unwrap();
    assert!(bytes > 0);
    assert!(line.ends_with('\n'));
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn receipt_waits_for_async_rw_flush_after_the_complete_line_is_written() {
    let (mut transport, output, gate, mut peer) = gated(tokio::io::empty());
    let receipt = output.arm(RequestId::Number(7)).unwrap();
    let identity = receipt.identity().clone();
    let item = response(RequestId::Number(7));
    let expected = serde_json::to_value(&item).unwrap();
    let send = tokio::spawn(transport.send(item));
    gate.entered().await;
    assert_eq!(read_message(&mut peer).await, expected);
    assert_eq!(receipt.outcome(), None);
    assert!(!send.is_finished());
    gate.release(Ok(()));
    bounded(send).await.unwrap().unwrap();
    assert_eq!(bounded(receipt.wait()).await, Ok(identity));
    assert_eq!(
        output.arm(RequestId::Number(8)).unwrap_err(),
        StdioFenceError::AlreadyUsed
    );
}

#[tokio::test]
async fn flush_failure_is_retained_even_when_the_response_bytes_were_written() {
    let (mut transport, output, gate, mut peer) = gated(tokio::io::empty());
    let receipt = output.arm(RequestId::String("close".into())).unwrap();
    let send = tokio::spawn(transport.send(response(RequestId::String("close".into()))));
    gate.entered().await;
    assert_eq!(read_message(&mut peer).await["id"], "close");
    assert_eq!(receipt.outcome(), None);
    gate.release(Err(io::ErrorKind::BrokenPipe));
    assert_eq!(
        bounded(send).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    assert_eq!(
        bounded(receipt.wait()).await,
        Err(StdioFenceError::SendFailed(io::ErrorKind::BrokenPipe))
    );
}

#[tokio::test]
async fn typed_id_matching_preserves_ordinary_requests_notifications_and_error_frames() {
    let (writer, peer) = tokio::io::duplex(IO_CAPACITY);
    let (mut transport, output) = transport(tokio::io::empty(), writer);
    let mut peer = BufReader::new(peer);
    let long_id = "x".repeat(MAX_RECEIPT_ID_BYTES + 1);
    assert_eq!(
        output
            .arm(RequestId::String(long_id.clone().into()))
            .unwrap_err(),
        StdioFenceError::RequestIdTooLong
    );
    let receipt = output.arm(RequestId::Number(7)).unwrap();
    let identity = receipt.identity().clone();
    for item in [
        message(json!({"jsonrpc":"2.0","method":"ping","id":7})),
        message(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})),
        message(json!({"jsonrpc":"2.0","error":{"code":-32603,"message":"idless"}})),
        response(RequestId::String("7".into())),
        response(RequestId::String(long_id.into())),
    ] {
        let expected = serde_json::to_value(&item).unwrap();
        bounded(transport.send(item)).await.unwrap();
        assert_eq!(read_message(&mut peer).await, expected);
        assert_eq!(receipt.outcome(), None);
    }
    let item = JsonRpcMessage::error(
        rmcp::model::ErrorData::internal_error("terminal failure", None),
        Some(RequestId::Number(7)),
    );
    let expected = serde_json::to_value(&item).unwrap();
    bounded(transport.send(item)).await.unwrap();
    assert_eq!(read_message(&mut peer).await, expected);
    assert_eq!(bounded(receipt.wait()).await, Ok(identity));
}

#[tokio::test]
async fn never_polled_and_aborted_send_futures_fail_only_their_exact_attempts() {
    let (mut first, first_output, _first_gate, _first_peer) = gated(tokio::io::empty());
    let (mut second, second_output, second_gate, _second_peer) = gated(tokio::io::empty());
    let first_receipt = first_output.arm(RequestId::Number(7)).unwrap();
    let second_receipt = second_output.arm(RequestId::Number(7)).unwrap();
    assert_ne!(
        first_receipt.identity().attempt,
        second_receipt.identity().attempt
    );
    let unpolled = first.send(response(RequestId::Number(7)));
    drop(unpolled);
    assert_eq!(
        bounded(first_receipt.wait()).await,
        Err(StdioFenceError::SendAborted)
    );
    assert_eq!(second_receipt.outcome(), None);
    let running = tokio::spawn(second.send(response(RequestId::Number(7))));
    second_gate.entered().await;
    running.abort();
    assert!(bounded(running).await.unwrap_err().is_cancelled());
    assert_eq!(
        bounded(second_receipt.wait()).await,
        Err(StdioFenceError::SendAborted)
    );
}

#[tokio::test]
async fn matching_client_cancellation_cannot_be_overwritten_by_later_successful_flush() {
    let (read, mut client) = tokio::io::duplex(IO_CAPACITY);
    let (mut transport, output, gate, mut peer) = gated(read);
    let receipt = output.arm(RequestId::Number(7)).unwrap();
    let send = tokio::spawn(transport.send(response(RequestId::Number(7))));
    gate.entered().await;
    read_message(&mut peer).await;
    bounded(client.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":\"7\"}}\n")).await.unwrap();
    assert!(bounded(transport.receive()).await.is_some());
    assert_eq!(receipt.outcome(), None);
    bounded(client.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n")).await.unwrap();
    assert!(bounded(transport.receive()).await.is_some());
    assert_eq!(
        receipt.outcome(),
        Some(Err(StdioFenceError::RequestCancelled))
    );
    gate.release(Ok(()));
    bounded(send).await.unwrap().unwrap();
    assert_eq!(
        bounded(receipt.wait()).await,
        Err(StdioFenceError::RequestCancelled)
    );
}

#[tokio::test]
async fn cancelled_receipt_waiter_remains_failed_after_the_io_send_finishes() {
    let (mut transport, output, gate, _peer) = gated(tokio::io::empty());
    let receipt = output.arm(RequestId::Number(7)).unwrap();
    let attempt = receipt.attempt.clone();
    let send = tokio::spawn(transport.send(response(RequestId::Number(7))));
    gate.entered().await;
    let waiting = tokio::spawn(receipt.wait());
    waiting.abort();
    assert!(bounded(waiting).await.unwrap_err().is_cancelled());
    assert_eq!(
        attempt.outcome(),
        Some(Err(StdioFenceError::ReceiptCancelled))
    );
    gate.release(Ok(()));
    bounded(send).await.unwrap().unwrap();
    assert_eq!(
        attempt.outcome(),
        Some(Err(StdioFenceError::ReceiptCancelled))
    );
}

#[tokio::test]
async fn premature_close_and_drop_fail_before_an_existing_send_can_later_succeed() {
    for drop_transport in [false, true] {
        let (mut transport, output, gate, _peer) = gated(tokio::io::empty());
        let receipt = output.arm(RequestId::Number(7)).unwrap();
        let send = tokio::spawn(transport.send(response(RequestId::Number(7))));
        gate.entered().await;
        let expected = if drop_transport {
            drop(transport);
            StdioFenceError::TransportDropped
        } else {
            let close = transport.close();
            drop(close);
            StdioFenceError::TransportClosed
        };
        assert_eq!(receipt.outcome(), Some(Err(expected)));
        gate.release(Ok(()));
        bounded(send).await.unwrap().unwrap();
        assert_eq!(bounded(receipt.wait()).await, Err(expected));
        assert_eq!(output.arm(RequestId::Number(8)).unwrap_err(), expected);
    }
}

#[tokio::test]
async fn actual_async_write_failure_preserves_the_ordinary_send_error() {
    let (writer, peer) = tokio::io::duplex(IO_CAPACITY);
    let (mut transport, output) = transport(tokio::io::empty(), writer);
    let receipt = output.arm(RequestId::Number(7)).unwrap();
    drop(peer);
    let error = bounded(transport.send(response(RequestId::Number(7))))
        .await
        .unwrap_err();
    assert_eq!(
        bounded(receipt.wait()).await,
        Err(StdioFenceError::SendFailed(error.kind()))
    );
}
