use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use futures_util::Stream;
use hyper::body::{Frame, SizeHint};
use rmcp::model::{CallToolResult, ProtocolVersion, RequestId, ServerJsonRpcMessage, ServerResult};
use rmcp::service::RequestContext;
use tokio::sync::watch;

use super::ConnectionReceipt;

const ATTEMPT_META: &str = "io.hammerr/terminal-attempt";
const MAX_BYTES: usize = 256 * 1024;
const MAX_EVENTS: usize = 128;
const MAX_FRAMES: usize = 512;
const MAX_REQUEST_ID_BYTES: usize = 1024;
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
const INTERRUPTED: &str = "MCP terminal response observation was interrupted";
static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalAttempt(u64);

impl TerminalAttempt {
    pub fn value(&self) -> u64 {
        self.0
    }

    pub(crate) fn mark_result(
        &self,
        protocol: Option<&ProtocolVersion>,
        mut result: CallToolResult,
    ) -> CallToolResult {
        result
            .meta
            .get_or_insert_default()
            .insert(ATTEMPT_META.into(), serde_json::json!(self.0.to_string()));
        if protocol.is_none_or(|version| version.as_str() < ProtocolVersion::V_2026_07_28.as_str())
            && result
                .result_type
                .as_ref()
                .is_some_and(|value| value.is_complete())
        {
            result.result_type = None;
        }
        result
    }

    pub(crate) fn matches_result(
        request: &RequestId,
        expected: &CallToolResult,
        message: &ServerJsonRpcMessage,
    ) -> bool {
        expected
            .meta
            .as_ref()
            .and_then(|meta| meta.get(ATTEMPT_META))
            .is_some()
            && matches!(message, ServerJsonRpcMessage::Response(response) if response.id == *request && matches!(&response.result, ServerResult::CallToolResult(result) if result == expected))
    }

    #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
    pub fn new() -> Result<Self, String> {
        NEXT_ATTEMPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map(Self)
            .map_err(|_| "MCP terminal attempt identifiers exhausted".into())
    }
}

#[derive(Clone)]
struct Witness(Arc<Shared>);

struct Shared {
    connection: ConnectionReceipt,
    state: Mutex<State>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let state = self.state.get_mut().expect("HTTP output lock poisoned");
        if !state.ended
            && let Some(expected) = &state.reserved
        {
            let error = expected.completion.finish(Err(INTERRUPTED.into()));
            expected.body.send_replace(Some(error));
        }
    }
}

#[derive(Default)]
struct State {
    ended: bool,
    reserved: Option<Expected>,
}

struct Expected {
    attempt: TerminalAttempt,
    request: RequestId,
    result: Option<CallToolResult>,
    matched: bool,
    body: watch::Sender<Option<Result<(), String>>>,
    completion: Arc<Completion>,
}

#[must_use]
pub struct HttpTerminalResponse {
    witness: Witness,
    protocol: Option<ProtocolVersion>,
    bound: bool,
}

impl HttpTerminalResponse {
    pub fn reserve(
        context: &RequestContext<rmcp::RoleServer>,
        attempt: TerminalAttempt,
    ) -> Result<(Self, TerminalResponseReceipt), String> {
        if matches!(&context.id, RequestId::String(id) if id.len() > MAX_REQUEST_ID_BYTES) {
            return Err("MCP terminal request ID exceeds the receipt limit".into());
        }
        let witness = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Witness>())
            .cloned()
            .ok_or("This request has no owned HTTP response witness")?;
        Self::reserve_for(
            witness,
            context.id.clone(),
            context.protocol_version(),
            attempt,
        )
    }

    fn reserve_for(
        witness: Witness,
        request: RequestId,
        protocol: Option<ProtocolVersion>,
        attempt: TerminalAttempt,
    ) -> Result<(Self, TerminalResponseReceipt), String> {
        if let Some(result) = witness.0.connection.result() {
            result?;
            return Err("This HTTP connection has already completed".into());
        }
        let (body, observed) = watch::channel(None);
        let completion = Arc::new(Completion::new());
        {
            let mut state = witness.0.state.lock().expect("HTTP output lock poisoned");
            if state.ended || state.reserved.is_some() {
                return Err("This HTTP response is already finished or reserved".into());
            }
            state.reserved = Some(Expected {
                attempt,
                request,
                result: None,
                matched: false,
                body,
                completion: completion.clone(),
            });
        }
        let receipt = TerminalResponseReceipt {
            body: observed,
            connection: witness.0.connection.clone(),
            completion,
        };
        Ok((
            Self {
                witness,
                protocol,
                bound: false,
            },
            receipt,
        ))
    }

    pub fn bind(mut self, result: CallToolResult) -> Result<CallToolResult, String> {
        let result = self.bind_result(result);
        if let Err(error) = &result {
            self.witness.fail(error);
        }
        self.bound = result.is_ok();
        result
    }

    fn bind_result(&self, result: CallToolResult) -> Result<CallToolResult, String> {
        let mut state = self
            .witness
            .0
            .state
            .lock()
            .expect("HTTP output lock poisoned");
        let expected = state.reserved.as_mut().expect("HTTP reservation missing");
        if let Some(result) = expected.completion.result() {
            result?;
            return Err("HTTP response already completed".into());
        }
        let result = expected.attempt.mark_result(self.protocol.as_ref(), result);
        let mut encoded = ByteBudget {
            remaining: MAX_BYTES / 2,
            bytes: Vec::new(),
        };
        serde_json::to_writer(&mut encoded, &result).map_err(|error| error.to_string())?;
        expected.result =
            Some(serde_json::from_slice(&encoded.bytes).map_err(|error| error.to_string())?);
        Ok(result)
    }
}

impl Drop for HttpTerminalResponse {
    fn drop(&mut self) {
        if !self.bound {
            self.witness.fail("MCP terminal result was not bound");
        }
    }
}

struct Completion {
    state: Mutex<(bool, Option<Result<(), String>>)>,
    changed: watch::Sender<Option<Result<(), String>>>,
}

impl Completion {
    fn new() -> Self {
        Self {
            state: Mutex::new((false, None)),
            changed: watch::channel(None).0,
        }
    }
    fn result(&self) -> Option<Result<(), String>> {
        self.state
            .lock()
            .expect("HTTP receipt lock poisoned")
            .1
            .clone()
    }
    fn finish(&self, result: Result<(), String>) -> Result<(), String> {
        let mut state = self.state.lock().expect("HTTP receipt lock poisoned");
        if state.1.is_none() {
            state.1 = Some(result);
            self.changed.send_replace(state.1.clone());
        }
        state.1.clone().unwrap()
    }
}

struct WaitGuard(Arc<Completion>);
impl Drop for WaitGuard {
    fn drop(&mut self) {
        let _ = self.0.finish(Err(INTERRUPTED.into()));
    }
}

struct ByteBudget {
    remaining: usize,
    bytes: Vec<u8>,
}
impl std::io::Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.remaining = self.remaining.checked_sub(bytes.len()).ok_or_else(|| {
            std::io::Error::other("MCP terminal result exceeds the output observation budget")
        })?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
#[must_use]
pub struct TerminalResponseReceipt {
    body: watch::Receiver<Option<Result<(), String>>>,
    connection: ConnectionReceipt,
    completion: Arc<Completion>,
}

impl TerminalResponseReceipt {
    pub fn result(&self) -> Option<Result<(), String>> {
        if let Some(result) = self.completion.result() {
            return Some(result);
        }
        if let Some(result) = self.connection.result() {
            if let Err(error) = result {
                return Some(self.completion.finish(Err(error)));
            }
            if self.body.borrow().as_ref() == Some(&Ok(())) {
                return Some(self.completion.finish(Ok(())));
            }
        }
        if self.body.borrow().is_none() && self.body.has_changed().is_err() {
            return Some(self.completion.finish(Err(INTERRUPTED.into())));
        }
        None
    }

    /// Wait in the retained host coordinator, never in the producing handler.
    pub fn wait(&self) -> impl Future<Output = Result<(), String>> + Send + 'static {
        self.wait_with_grace(OUTPUT_GRACE)
    }

    fn wait_with_grace(
        &self,
        grace: Duration,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let owner = {
            let mut state = self
                .completion
                .state
                .lock()
                .expect("HTTP receipt lock poisoned");
            if state.0 {
                None
            } else {
                state.0 = true;
                Some(WaitGuard(self.completion.clone()))
            }
        };
        let receipt = self.clone();
        let mut changed = self.completion.changed.subscribe();
        async move {
            if let Some(owner) = owner {
                let result = tokio::time::timeout(grace, receipt.finish_output())
                    .await
                    .unwrap_or_else(|_| Err("MCP terminal response output timed out".into()));
                return owner.0.finish(result);
            }
            loop {
                if let Some(result) = receipt.result() {
                    return result;
                }
                changed
                    .changed()
                    .await
                    .map_err(|_| INTERRUPTED.to_string())?;
            }
        }
    }

    async fn finish_output(&self) -> Result<(), String> {
        let mut body = self.body.clone();
        loop {
            if let Some(result) = self.result() {
                return result;
            }
            let result = body.borrow().clone();
            if let Some(result) = result {
                result?;
                break;
            }
            body.changed().await.map_err(|_| INTERRUPTED.to_string())?;
        }
        self.connection.request_graceful();
        self.connection.wait().await
    }
}

impl Witness {
    fn fail(&self, error: &str) {
        let state = self.0.state.lock().expect("HTTP output lock poisoned");
        if let Some(expected) = &state.reserved {
            let error = expected.completion.finish(Err(error.into()));
            expected.body.send_replace(Some(error));
        }
    }
    fn armed(&self) -> bool {
        self.0
            .state
            .lock()
            .expect("HTTP output lock poisoned")
            .reserved
            .as_ref()
            .is_some_and(|expected| expected.completion.result().is_none())
    }
    fn message(&self, message: ServerJsonRpcMessage) {
        let mut state = self.0.state.lock().expect("HTTP output lock poisoned");
        let Some(expected) = &mut state.reserved else {
            return;
        };
        if expected.result.as_ref().is_some_and(|result| {
            TerminalAttempt::matches_result(&expected.request, result, &message)
        }) {
            expected.matched = true;
            return;
        }
        if let ServerJsonRpcMessage::Response(response) = message
            && response.id == expected.request
            && let ServerResult::CallToolResult(result) = response.result
            && result.meta.as_ref().and_then(|meta| meta.get(ATTEMPT_META))
                == Some(&serde_json::json!(expected.attempt.0.to_string()))
        {
            let error = expected.completion.finish(Err(
                "MCP terminal outcome did not match the bound result".into(),
            ));
            expected.body.send_replace(Some(error));
        }
    }
    fn end(&self) {
        let mut state = self.0.state.lock().expect("HTTP output lock poisoned");
        state.ended = true;
        if let Some(expected) = &state.reserved {
            let result = if let Some(Err(error)) = expected.completion.result() {
                Err(error)
            } else if expected.matched {
                Ok(())
            } else {
                expected.completion.finish(Err(
                    "HTTP body ended without its bound terminal result".into()
                ))
            };
            expected.body.send_replace(Some(result));
        }
    }
}

pub(super) async fn observe(mut request: Request, next: Next) -> Response {
    let Some(connection) = request.extensions().get::<ConnectionReceipt>().cloned() else {
        return next.run(request).await;
    };
    let witness = Witness(Arc::new(Shared {
        connection,
        state: Mutex::new(State::default()),
    }));
    request.extensions_mut().insert(witness.clone());
    let response = next.run(request).await;
    let content = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let kind = match content.split(';').next() {
        Some("text/event-stream") => Kind::Sse,
        Some("application/json") => Kind::Json,
        _ => Kind::Unsupported,
    };
    response.map(|inner| Body::new(ObservedBody::new(inner, witness, kind)))
}

enum Kind {
    Sse,
    Json,
    Unsupported,
}
enum Decoder {
    Sse {
        stream: Pin<Box<sse_stream::SseStream<FeedBody>>>,
        feed: Arc<Mutex<Feed>>,
    },
    Json(Vec<u8>),
}
#[derive(Default)]
struct Feed {
    chunks: VecDeque<Bytes>,
    ended: bool,
}
struct FeedBody(Arc<Mutex<Feed>>);

impl HttpBody for FeedBody {
    type Data = Bytes;
    type Error = std::io::Error;
    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let mut feed = self.0.lock().expect("SSE feed lock poisoned");
        if let Some(bytes) = feed.chunks.pop_front() {
            Poll::Ready(Some(Ok(Frame::data(bytes))))
        } else if feed.ended {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

struct ObservedBody {
    inner: Pin<Box<Body>>,
    witness: Witness,
    kind: Kind,
    decoder: Option<Decoder>,
    bytes: usize,
    events: usize,
    frames: usize,
    ended: bool,
    failed: bool,
}

impl ObservedBody {
    fn new(inner: Body, witness: Witness, kind: Kind) -> Self {
        Self {
            inner: Box::pin(inner),
            witness,
            kind,
            decoder: None,
            bytes: 0,
            events: 0,
            frames: 0,
            ended: false,
            failed: false,
        }
    }
    fn observe(&mut self, bytes: Option<&Bytes>, ended: bool, cx: &mut Context<'_>) {
        if self.failed || !self.witness.armed() {
            return;
        }
        if self.decoder.is_none() {
            // rmcp's SseBody emits whole events, including any priming frames skipped before reserve.
            self.decoder = Some(match self.kind {
                Kind::Sse => {
                    let feed = Arc::new(Mutex::new(Feed::default()));
                    Decoder::Sse {
                        stream: Box::pin(sse_stream::SseStream::new(FeedBody(feed.clone()))),
                        feed,
                    }
                }
                Kind::Json => Decoder::Json(Vec::new()),
                Kind::Unsupported => {
                    self.failed = true;
                    self.witness
                        .fail("Unsupported terminal response content type");
                    return;
                }
            });
        }
        self.frames += 1;
        self.bytes = self.bytes.saturating_add(bytes.map_or(0, Bytes::len));
        if self.bytes > MAX_BYTES || self.frames > MAX_FRAMES {
            self.failed = true;
            self.witness
                .fail("MCP terminal response exceeded the byte/frame budget");
            self.decoder = None;
            return;
        }
        let result = match self.decoder.as_mut().unwrap() {
            Decoder::Json(data) => {
                if let Some(bytes) = bytes {
                    data.extend_from_slice(bytes);
                }
                if ended {
                    serde_json::from_slice::<ServerJsonRpcMessage>(data)
                        .map(|message| self.witness.message(message))
                        .map_err(|error| error.to_string())
                } else {
                    Ok(())
                }
            }
            Decoder::Sse { stream, feed } => {
                {
                    let mut feed = feed.lock().expect("SSE feed lock poisoned");
                    if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
                        feed.chunks.push_back(bytes.clone());
                    }
                    feed.ended = ended;
                }
                loop {
                    match stream.as_mut().poll_next(cx) {
                        Poll::Pending | Poll::Ready(None) => break Ok(()),
                        Poll::Ready(Some(Err(error))) => break Err(error.to_string()),
                        Poll::Ready(Some(Ok(event))) => {
                            self.events += 1;
                            if self.events > MAX_EVENTS {
                                break Err("MCP terminal response exceeded the event budget".into());
                            }
                            if let Some(data) = event.data.filter(|data| !data.is_empty()) {
                                match serde_json::from_str::<ServerJsonRpcMessage>(&data) {
                                    Ok(message) => self.witness.message(message),
                                    Err(error) => break Err(error.to_string()),
                                }
                            }
                        }
                    }
                }
            }
        };
        if let Err(error) = result {
            self.failed = true;
            self.witness
                .fail(&format!("MCP terminal response decoding failed: {error}"));
            self.decoder = None;
        }
    }
}

impl HttpBody for ObservedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let frame = match self.inner.as_mut().poll_frame(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(frame) => frame,
        };
        let ended = frame.is_none() || self.inner.is_end_stream();
        match &frame {
            Some(Err(error)) => {
                self.failed = true;
                self.witness
                    .fail(&format!("MCP terminal response body failed: {error}"));
            }
            Some(Ok(frame)) => self.observe(frame.data_ref(), ended, cx),
            None => self.observe(None, true, cx),
        }
        if ended {
            self.ended = true;
            self.witness.end();
        }
        Poll::Ready(frame)
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for ObservedBody {
    fn drop(&mut self) {
        if !self.ended {
            self.witness.fail(INTERRUPTED);
        }
        self.witness
            .0
            .state
            .lock()
            .expect("HTTP output lock poisoned")
            .ended = true;
    }
}

#[cfg(test)]
mod tests;
