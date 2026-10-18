use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use tokio::sync::oneshot;

use super::{EditorServer, HttpTerminalResponse, TerminalAttempt};
use crate::runtime::{DocumentCloseReview, DocumentHandle, DocumentId};

pub(crate) const TOOL_NAME: &str = "close_document";
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BYTES: usize = 2048;

type ResultBinder = Box<dyn FnOnce(CallToolResult) -> Result<CallToolResult, String> + Send>;
type OutputWait = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type UiAction = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
type StopAction = Arc<dyn Fn() + Send + Sync>;

pub struct TerminalUi {
    retire: Arc<dyn Fn(DocumentId) -> Result<(), String> + Send + Sync>,
    quit: UiAction,
}

impl TerminalUi {
    pub fn new(
        retire: impl Fn(DocumentId) -> Result<(), String> + Send + Sync + 'static,
        quit: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            retire: Arc::new(retire),
            quit: Arc::new(quit),
        }
    }
}

struct Output {
    bind: ResultBinder,
    wait: OutputWait,
}

impl Output {
    fn reserve(
        server: &EditorServer,
        context: &RequestContext<rmcp::RoleServer>,
        attempt: TerminalAttempt,
    ) -> Result<Self, String> {
        if let Some(output) = &server.stdio_output {
            let (ticket, receipt) = output
                .reserve(context.id.clone(), context.protocol_version(), attempt)
                .map_err(|error| error.to_string())?;
            Ok(Self {
                bind: Box::new(move |result| {
                    ticket.bind(result).map_err(|error| error.to_string())
                }),
                wait: Box::pin(async move {
                    receipt
                        .wait()
                        .await
                        .map(drop)
                        .map_err(|error| error.to_string())
                }),
            })
        } else {
            let (ticket, receipt) = HttpTerminalResponse::reserve(context, attempt)?;
            Ok(Self {
                bind: Box::new(move |result| ticket.bind(result)),
                wait: Box::pin(async move { receipt.wait().await }),
            })
        }
    }
}

struct Work {
    attempt: TerminalAttempt,
    output: Option<Output>,
    response: Option<oneshot::Sender<CallToolResult>>,
}

#[derive(Default)]
struct State {
    accepted: bool,
    stopping: bool,
    finishing: bool,
    work: Option<Work>,
    result: Option<Result<(), String>>,
    stop: Option<StopAction>,
}

struct Core {
    document: DocumentHandle,
    ui: Option<TerminalUi>,
    state: Mutex<State>,
    changed: Condvar,
    #[cfg(test)]
    before_commit: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[derive(Clone)]
pub struct TerminalClose(Arc<Core>);

#[must_use]
pub struct TerminalOwner {
    handle: TerminalClose,
    thread: Option<JoinHandle<()>>,
    finished: Option<Result<(), String>>,
}

impl TerminalOwner {
    pub fn start(document: DocumentHandle, ui: Option<TerminalUi>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let core = Arc::new(Core {
            document,
            ui,
            state: Mutex::default(),
            changed: Condvar::new(),
            #[cfg(test)]
            before_commit: Mutex::default(),
        });
        let worker = core.clone();
        let thread = std::thread::Builder::new()
            .name("document-close".into())
            .spawn(move || run_worker(worker, runtime))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            handle: TerminalClose(core),
            thread: Some(thread),
            finished: None,
        })
    }

    pub fn handle(&self) -> TerminalClose {
        self.handle.clone()
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        {
            let mut state = self
                .handle
                .0
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.stopping = true;
            self.handle.0.changed.notify_all();
        }
        let joined = self
            .thread
            .take()
            .expect("Close worker missing")
            .join()
            .map_err(|_| "The terminal Close worker panicked".to_owned());
        let result = joined.and_then(|()| self.handle.result().unwrap_or(Ok(())));
        self.finished = Some(result.clone());
        result
    }

    pub fn finish(
        mut self,
        primary: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match (primary, self.shutdown()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Err(primary), Err(close)) => {
                Err(format!("{primary}; terminal Close failed: {close}").into())
            }
        }
    }
}

impl Drop for TerminalOwner {
    fn drop(&mut self) {
        if self.finished.is_none()
            && let Err(error) = self.shutdown()
        {
            eprintln!("Terminal Close failed: {error}");
        }
    }
}

impl TerminalClose {
    pub fn document_id(&self) -> DocumentId {
        self.0.document.id()
    }

    pub fn result(&self) -> Option<Result<(), String>> {
        self.0
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .result
            .clone()
    }

    pub fn set_transport_stop(&self, stop: impl Fn() + Send + Sync + 'static) {
        let stop: StopAction = Arc::new(stop);
        let finishing = {
            let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.stop = Some(stop.clone());
            state.finishing
        };
        if finishing {
            stop();
        }
    }

    pub fn commit_native(
        &self,
        review: &DocumentCloseReview,
        discard_changes: bool,
    ) -> Result<(), String> {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        ensure_available(&state)?;
        let attempt = TerminalAttempt::new()?;
        self.0
            .document
            .try_commit_reviewed_close(review, discard_changes)?;
        state.accepted = true;
        state.work = Some(Work {
            attempt,
            output: None,
            response: None,
        });
        self.0.changed.notify_one();
        Ok(())
    }

    pub(super) fn commit_mcp(
        &self,
        server: &EditorServer,
        token: &str,
        discard_changes: bool,
        context: &RequestContext<rmcp::RoleServer>,
    ) -> Result<oneshot::Receiver<CallToolResult>, String> {
        if context.ct.is_cancelled() {
            return Err("The Close request was cancelled before acceptance".into());
        }
        if server._document.as_ref().map(DocumentHandle::id) != Some(self.document_id()) {
            return Err("The terminal Close owner belongs to another document".into());
        }
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        ensure_available(&state)?;
        #[cfg(test)]
        if let Some(before_commit) = self.0.before_commit.lock().unwrap().take() {
            before_commit();
        }
        let attempt = TerminalAttempt::new()?;
        let (response, receiver) = oneshot::channel();
        let output = self
            .0
            .document
            .commit_close_token(token, discard_changes, || {
                if context.ct.is_cancelled() {
                    return Err("The Close request was cancelled before acceptance".into());
                }
                Output::reserve(server, context, attempt.clone())
            })?;
        state.accepted = true;
        state.work = Some(Work {
            attempt,
            output: Some(output),
            response: Some(response),
        });
        self.0.changed.notify_one();
        Ok(receiver)
    }
}

fn ensure_available(state: &State) -> Result<(), String> {
    if state.accepted || state.stopping {
        Err("The document is already closing or closed".into())
    } else {
        Ok(())
    }
}

fn combine(left: Result<(), String>, right: Result<(), String>) -> Result<(), String> {
    match (left, right) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(left), Err(right)) => Err(format!("{left}; {right}")),
    }
}

fn bounded_error(error: &str) -> &str {
    let end = error
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= MAX_ERROR_BYTES)
        .last()
        .unwrap_or(0);
    if error.len() <= MAX_ERROR_BYTES {
        error
    } else {
        &error[..end]
    }
}

fn finish_work(core: &Core, work: Work, runtime: &tokio::runtime::Runtime) -> Result<(), String> {
    let drained = core.document.shutdown();
    let retired = core
        .ui
        .as_ref()
        .map_or(Ok(()), |ui| (ui.retire)(core.document.id()));
    let ui_state = if core.ui.is_none() {
        "headless"
    } else if retired.is_ok() {
        "retired"
    } else {
        "failed"
    };
    let cleaned = combine(drained, retired);
    let Some(output) = work.output else {
        return cleaned;
    };
    let value = serde_json::json!({
        "document_id": core.document.id().value(),
        "attempt": work.attempt.value().to_string(),
        "closed": cleaned.is_ok(),
        "terminal": true,
        "ui": ui_state,
        "error": cleaned.as_ref().err().map(|error| bounded_error(error)),
    });
    let mut result = CallToolResult::structured(value);
    result.is_error = Some(cleaned.is_err());
    let result = match (output.bind)(result) {
        Ok(result) => result,
        Err(error) => {
            if let Some(response) = work.response {
                let _ = response.send(CallToolResult::error(vec![
                    rmcp::model::ContentBlock::text(bounded_error(&error).to_owned()),
                ]));
            }
            return combine(
                cleaned,
                Err(format!("Could not bind terminal response: {error}")),
            );
        }
    };
    let handed_off = work
        .response
        .ok_or_else(|| "The terminal response receiver is missing".to_owned())
        .and_then(|response| {
            response
                .send(result)
                .map_err(|_| "The terminal response receiver was dropped".to_owned())
        });
    let sent = runtime.block_on(async {
        tokio::time::timeout(OUTPUT_TIMEOUT, output.wait)
            .await
            .map_err(|_| "The terminal response output deadline expired".to_owned())?
    });
    combine(combine(cleaned, handed_off), sent)
}

fn run_worker(core: Arc<Core>, runtime: tokio::runtime::Runtime) {
    let work = {
        let mut state = core.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.work.is_none() && !state.stopping {
            state = core
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.work.take()
    };
    let Some(work) = work else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_work(&core, work, &runtime)
    }));
    let mut result = match result {
        Ok(result) => result,
        Err(_) => {
            let cleanup = core.document.shutdown();
            let retired = core
                .ui
                .as_ref()
                .map_or(Ok(()), |ui| (ui.retire)(core.document.id()));
            combine(
                Err("The terminal Close worker panicked".into()),
                combine(cleanup, retired),
            )
        }
    };
    let stop = {
        let mut state = core.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.finishing = true;
        state.stop.clone()
    };
    if let Some(stop) = stop {
        stop();
    }
    if let Some(ui) = &core.ui {
        result = combine(result, (ui.quit)());
    }
    core.state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .result = Some(result);
}

#[cfg(test)]
mod tests;

#[cfg(feature = "test-support")]
pub fn validate_terminal_close_output(
    mut server: EditorServer,
    token: String,
    before_send: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    use super::ServerHandler;
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, JsonRpcMessage, RequestId, ServerResult,
    };
    use rmcp::service::serve_directly;
    use rmcp::transport::Transport;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let (mut transport, output) =
            super::stdio::transport(tokio::io::empty(), tokio::io::sink());
        server.stdio_output = Some(output);
        let (service_io, _peer) = tokio::io::duplex(4096);
        let running =
            serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), service_io, None);
        let id = RequestId::String("owned-host-output-fixture".into());
        let context = RequestContext::new(id.clone(), running.peer().clone());
        let request = CallToolRequestParams::new(TOOL_NAME).with_arguments(
            serde_json::json!({"review_token":token,"discard_changes":true})
                .as_object()
                .unwrap()
                .clone(),
        );
        let result = tokio::time::timeout(OUTPUT_TIMEOUT * 2, server.call_tool(request, context))
            .await
            .map_err(|_| "The owned-host Close dispatcher timed out".to_owned())?
            .map_err(|error| error.to_string())?;
        let CallToolResponse::Complete(result) = result else {
            return Err("The owned-host Close did not return a completed result".into());
        };
        if result.is_error == Some(true) || result.meta.is_none() {
            return Err(format!("The owned-host Close failed: {result:?}"));
        }
        before_send()?;
        tokio::time::timeout(
            OUTPUT_TIMEOUT,
            transport.send(JsonRpcMessage::response(
                ServerResult::CallToolResult(result),
                id,
            )),
        )
        .await
        .map_err(|_| "The owned-host terminal send timed out".to_owned())?
        .map_err(|error| error.to_string())?;
        tokio::time::timeout(OUTPUT_TIMEOUT, running.cancel())
            .await
            .map_err(|_| "The owned-host fixture service did not stop".to_owned())?
            .map_err(|error| error.to_string())?;
        Ok(())
    })
}
