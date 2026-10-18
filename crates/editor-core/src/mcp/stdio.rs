use std::fmt;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rmcp::RoleServer;
use rmcp::model::{ClientNotification, JsonRpcMessage, RequestId, ServerJsonRpcMessage};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::transport::async_rw::AsyncRwTransport;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;

const MAX_RECEIPT_ID_BYTES: usize = 1024;
static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdioAttempt(u64);

impl StdioAttempt {
    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdioResponseFlushed {
    pub request_id: RequestId,
    pub attempt: StdioAttempt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdioFenceError {
    AlreadyUsed,
    RequestIdTooLong,
    AttemptCapacityExhausted,
    SendFailed(io::ErrorKind),
    SendAborted,
    RequestCancelled,
    ReceiptCancelled,
    TransportClosed,
    TransportDropped,
    ObserverLost,
}

impl fmt::Display for StdioFenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyUsed => {
                formatter.write_str("The terminal stdio response fence was already used")
            }
            Self::RequestIdTooLong => {
                formatter.write_str("The stdio response request ID exceeds the receipt limit")
            }
            Self::AttemptCapacityExhausted => {
                formatter.write_str("Stdio response attempt capacity exhausted")
            }
            Self::SendFailed(kind) => {
                write!(formatter, "Stdio response send/flush failed: {kind:?}")
            }
            Self::SendAborted => {
                formatter.write_str("The stdio response send was abandoned before flush completed")
            }
            Self::RequestCancelled => {
                formatter.write_str("The client cancelled the observed stdio request")
            }
            Self::ReceiptCancelled => {
                formatter.write_str("The stdio response receipt was cancelled")
            }
            Self::TransportClosed => formatter
                .write_str("The stdio transport closed before the observed response completed"),
            Self::TransportDropped => formatter.write_str(
                "The stdio transport was dropped before the observed response completed",
            ),
            Self::ObserverLost => {
                formatter.write_str("The stdio response completion observer was lost")
            }
        }
    }
}

impl std::error::Error for StdioFenceError {}

type Outcome = Result<StdioResponseFlushed, StdioFenceError>;

#[derive(Debug)]
struct Completion {
    outcome: Option<Outcome>,
    sender: Option<oneshot::Sender<Outcome>>,
}

#[derive(Debug)]
struct Attempt {
    identity: StdioResponseFlushed,
    completion: Mutex<Completion>,
}

impl Attempt {
    fn resolve(&self, outcome: Outcome) {
        let sender = {
            let mut completion = self
                .completion
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if completion.outcome.is_some() {
                return;
            }
            completion.outcome = Some(outcome.clone());
            completion.sender.take()
        };
        if let Some(sender) = sender {
            let _ = sender.send(outcome);
        }
    }

    fn outcome(&self) -> Option<Outcome> {
        self.completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .outcome
            .clone()
    }
}

#[derive(Debug, Default)]
struct State {
    used: bool,
    terminal: Option<StdioFenceError>,
    attempt: Option<Arc<Attempt>>,
}

#[derive(Clone, Debug)]
pub struct StdioOutput {
    state: Arc<Mutex<State>>,
}

impl StdioOutput {
    fn new() -> Self {
        Self {
            state: Arc::default(),
        }
    }

    /// Arms one terminal response while its request is live, before its handler
    /// returns. Earlier cancellation is not retained. This transport cannot arm
    /// again or disambiguate a request ID reused by the peer.
    pub fn arm(&self, request_id: RequestId) -> Result<StdioReceipt, StdioFenceError> {
        if matches!(&request_id, RequestId::String(id) if id.len() > MAX_RECEIPT_ID_BYTES) {
            return Err(StdioFenceError::RequestIdTooLong);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = state.terminal {
            return Err(error);
        }
        if state.used {
            return Err(StdioFenceError::AlreadyUsed);
        }
        let attempt = NEXT_ATTEMPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map(StdioAttempt)
            .map_err(|_| StdioFenceError::AttemptCapacityExhausted)?;
        let (sender, receiver) = oneshot::channel();
        let attempt = Arc::new(Attempt {
            identity: StdioResponseFlushed {
                request_id,
                attempt,
            },
            completion: Mutex::new(Completion {
                outcome: None,
                sender: Some(sender),
            }),
        });
        state.used = true;
        state.attempt = Some(attempt.clone());
        Ok(StdioReceipt { attempt, receiver })
    }

    fn matching(&self, request_id: &RequestId) -> Option<Arc<Attempt>> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .attempt
            .as_ref()
            .filter(|attempt| &attempt.identity.request_id == request_id)
            .cloned()
    }

    fn finish(&self, error: StdioFenceError) {
        let attempt = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.terminal.is_none() {
                state.terminal = Some(error);
            }
            state.attempt.clone()
        };
        if let Some(attempt) = attempt {
            attempt.resolve(Err(error));
        }
    }
}

#[derive(Debug)]
pub struct StdioReceipt {
    attempt: Arc<Attempt>,
    receiver: oneshot::Receiver<Outcome>,
}

impl StdioReceipt {
    pub fn identity(&self) -> &StdioResponseFlushed {
        &self.attempt.identity
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.attempt.outcome()
    }

    /// Success covers the local send and writer flush, not peer consumption.
    pub async fn wait(mut self) -> Outcome {
        match (&mut self.receiver).await {
            Ok(outcome) => outcome,
            Err(_) => {
                self.attempt.resolve(Err(StdioFenceError::ObserverLost));
                self.attempt
                    .outcome()
                    .unwrap_or(Err(StdioFenceError::ObserverLost))
            }
        }
    }
}

impl Drop for StdioReceipt {
    fn drop(&mut self) {
        self.attempt.resolve(Err(StdioFenceError::ReceiptCancelled));
    }
}

struct SendGuard(Arc<Attempt>);

impl SendGuard {
    fn finish(self, result: &io::Result<()>) {
        self.0.resolve(match result {
            Ok(()) => Ok(self.0.identity.clone()),
            Err(error) => Err(StdioFenceError::SendFailed(error.kind())),
        });
    }
}

impl Drop for SendGuard {
    fn drop(&mut self) {
        self.0.resolve(Err(StdioFenceError::SendAborted));
    }
}

pub(super) struct ObservedStdio<R: AsyncRead, W: AsyncWrite> {
    inner: AsyncRwTransport<RoleServer, R, W>,
    output: StdioOutput,
}

pub(super) fn transport<R, W>(read: R, write: W) -> (ObservedStdio<R, W>, StdioOutput)
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let output = StdioOutput::new();
    let transport = ObservedStdio {
        inner: AsyncRwTransport::new_server(read, write),
        output: output.clone(),
    };
    (transport, output)
}

fn response_id(message: &ServerJsonRpcMessage) -> Option<&RequestId> {
    match message {
        JsonRpcMessage::Response(response) => Some(&response.id),
        JsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    }
}

impl<R, W> Transport<RoleServer> for ObservedStdio<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let guard = response_id(&item)
            .and_then(|id| self.output.matching(id))
            .map(SendGuard);
        let send = self.inner.send(item);
        async move {
            let result = send.await;
            if let Some(guard) = guard {
                guard.finish(&result);
            }
            result
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let item = self.inner.receive().await;
        if let Some(JsonRpcMessage::Notification(notification)) = &item
            && let ClientNotification::CancelledNotification(cancelled) = &notification.notification
            && let Some(request_id) = &cancelled.params.request_id
            && let Some(attempt) = self.output.matching(request_id)
        {
            attempt.resolve(Err(StdioFenceError::RequestCancelled));
        }
        item
    }

    fn close(&mut self) -> impl Future<Output = io::Result<()>> + Send {
        self.output.finish(StdioFenceError::TransportClosed);
        self.inner.close()
    }
}

impl<R: AsyncRead, W: AsyncWrite> Drop for ObservedStdio<R, W> {
    fn drop(&mut self) {
        self.output.finish(StdioFenceError::TransportDropped);
    }
}

#[cfg(test)]
mod tests;
