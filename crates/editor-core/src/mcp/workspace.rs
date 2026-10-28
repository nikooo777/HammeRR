use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use rmcp::model::{CallToolResult, MetaObject};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{EditorServer, StdioOutput, TerminalClose, TerminalOwner, TerminalUi};
use crate::runtime::{DocumentCloseReview, DocumentHandle, DocumentId, lock};
use crate::session::Actor;
use crate::startup::{self, CloseDisposition, CloseReservation, DocumentVisibility};
use futures_util::FutureExt;
use render::GpuContext;
use rmcp::ServiceExt;

mod quit;
mod routing;
pub use routing::WorkspaceServer;

const COMMAND_LIMIT: usize = 4;
const UI_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const DOCUMENT_META: &str = "io.hammerr/document";
pub type WorkspaceUiFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type CreateHost =
    Arc<dyn Fn(DocumentHandle, String, TerminalClose, bool) -> WorkspaceUiFuture + Send + Sync>;
type HostAction = Arc<dyn Fn(DocumentId) -> WorkspaceUiFuture + Send + Sync>;
type ShowAction = Arc<dyn Fn(DocumentId, bool) -> WorkspaceUiFuture + Send + Sync>;

#[derive(Clone)]
pub struct WorkspaceUi {
    create: CreateHost,
    show: ShowAction,
    retire: HostAction,
    release: HostAction,
    quit: Arc<dyn Fn() -> WorkspaceUiFuture + Send + Sync>,
}

impl WorkspaceUi {
    pub fn new(
        create: impl Fn(DocumentHandle, String, TerminalClose, bool) -> WorkspaceUiFuture
        + Send
        + Sync
        + 'static,
        show: impl Fn(DocumentId, bool) -> WorkspaceUiFuture + Send + Sync + 'static,
        retire: impl Fn(DocumentId) -> WorkspaceUiFuture + Send + Sync + 'static,
        release: impl Fn(DocumentId) -> WorkspaceUiFuture + Send + Sync + 'static,
        quit: impl Fn() -> WorkspaceUiFuture + Send + Sync + 'static,
    ) -> Self {
        let create: CreateHost = Arc::new(create);
        let show: ShowAction = Arc::new(show);
        let retire: HostAction = Arc::new(retire);
        let release: HostAction = Arc::new(release);
        let quit = Arc::new(quit);
        Self {
            create: Arc::new(move |document, selector, terminal, present| {
                guarded_ui(|| create(document, selector, terminal, present))
            }),
            show: Arc::new(move |document, focus| guarded_ui(|| show(document, focus))),
            retire: Arc::new(move |document| guarded_ui(|| retire(document))),
            release: Arc::new(move |document| guarded_ui(|| release(document))),
            quit: Arc::new(move || guarded_ui(|| quit())),
        }
    }
}

fn guarded_ui(action: impl FnOnce() -> WorkspaceUiFuture) -> WorkspaceUiFuture {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        Ok(future) => Box::pin(async move {
            std::panic::AssertUnwindSafe(future)
                .catch_unwind()
                .await
                .map_err(|_| "The workspace UI acknowledgement panicked".to_owned())?
        }),
        Err(_) => Box::pin(async { Err("The workspace UI callback panicked".into()) }),
    }
}

struct Entry {
    server: EditorServer,
    owner: Mutex<Option<TerminalOwner>>,
}

type CommandReply = oneshot::Sender<Result<Value, String>>;
enum Command {
    New(Actor, bool, CommandReply),
    Open(Actor, PathBuf, bool, CommandReply),
    Show(String, bool, CommandReply),
    SaveAll(startup::SaveAllRequest, CommandReply),
}

struct CompletedClose {
    reservation: CloseReservation,
    result: Result<(), String>,
    response: Option<oneshot::Sender<CallToolResult>>,
    ui_state: &'static str,
}

#[derive(Default)]
struct Queue {
    commands: VecDeque<Command>,
    completed: VecDeque<CompletedClose>,
    stopping: bool,
    executing: bool,
    quitting: bool,
    quit: Option<quit::Work>,
}

#[derive(Default)]
struct TransportState {
    stop: Option<Arc<dyn Fn() + Send + Sync>>,
    terminal_finishing: bool,
}

struct Core {
    factory: startup::Workspace,
    ctx: Option<GpuContext>,
    ui: Option<WorkspaceUi>,
    entries: Mutex<BTreeMap<DocumentId, Arc<Entry>>>,
    queue: Mutex<Queue>,
    changed: Condvar,
    stdio: Mutex<Option<StdioOutput>>,
    stop: Mutex<TransportState>,
    errors: Mutex<Vec<String>>,
}

#[derive(Clone)]
pub struct Workspace(Arc<Core>);

#[must_use]
pub struct WorkspaceOwner {
    workspace: Workspace,
    thread: Option<JoinHandle<()>>,
    finished: Option<Result<(), String>>,
}

impl WorkspaceOwner {
    pub fn start(
        factory: startup::Workspace,
        ctx: Option<GpuContext>,
        ui: Option<WorkspaceUi>,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let workspace = Workspace(Arc::new(Core {
            factory,
            ctx,
            ui,
            entries: Mutex::default(),
            queue: Mutex::default(),
            changed: Condvar::new(),
            stdio: Mutex::default(),
            stop: Mutex::default(),
            errors: Mutex::default(),
        }));
        for document in workspace.0.factory.documents() {
            let entry = workspace.entry(document.document())?;
            workspace
                .0
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(document.document().id(), entry);
        }
        routing::tools()?;
        let worker = workspace.clone();
        let thread = std::thread::Builder::new()
            .name("workspace-commands".into())
            .spawn(move || run_worker(worker, runtime))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            workspace,
            thread: Some(thread),
            finished: None,
        })
    }

    pub fn workspace(&self) -> Workspace {
        self.workspace.clone()
    }
    pub fn server(&self) -> WorkspaceServer {
        WorkspaceServer(self.workspace())
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        self.workspace.0.factory.close_admission();
        {
            let mut queue = self
                .workspace
                .0
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            queue.stopping = true;
            queue.commands.clear();
        }
        self.workspace.0.changed.notify_one();
        let joined = self
            .thread
            .take()
            .expect("Workspace command worker missing")
            .join()
            .map_err(|_| "The workspace command worker panicked".to_owned());
        if let Err(error) = &joined {
            failed_worker_cleanup(&self.workspace, error);
        }
        let errors = self
            .workspace
            .0
            .errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let result = combine(
            joined,
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            },
        );
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
            (Err(primary), Err(error)) => {
                Err(format!("{primary}; workspace shutdown failed: {error}").into())
            }
        }
    }
}

impl Drop for WorkspaceOwner {
    fn drop(&mut self) {
        if self.finished.is_none()
            && let Err(error) = self.shutdown()
        {
            eprintln!("Workspace shutdown failed: {error}");
        }
    }
}

impl Workspace {
    fn entry(&self, document: DocumentHandle) -> Result<Arc<Entry>, String> {
        let ui = self.0.ui.as_ref().map(|ui| {
            let retire = ui.retire.clone();
            let quit = ui.quit.clone();
            TerminalUi::asynchronous(move |id| retire(id), move || quit())
        });
        let owner = TerminalOwner::start(document.clone(), ui)?;
        let weak = Arc::downgrade(&self.0);
        owner.handle().set_transport_stop(move || {
            if let Some(core) = weak.upgrade() {
                let stop = core
                    .stop
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stop
                    .clone();
                if let Some(stop) = stop {
                    stop();
                }
            }
        });
        let server = EditorServer::for_document(document, self.0.ctx.clone())
            .with_terminal_close(owner.handle())?;
        Ok(Arc::new(Entry {
            server,
            owner: Mutex::new(Some(owner)),
        }))
    }

    pub fn terminal_for(&self, document: DocumentId) -> Result<TerminalClose, String> {
        self.0
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&document)
            .and_then(|entry| entry.server.terminal.clone())
            .ok_or_else(|| "Unknown or closed document".into())
    }

    pub fn list_documents(&self) -> Result<Value, String> {
        let entries = self.0.factory.documents();
        let documents: Vec<Value> = entries.iter().map(|entry| {
            let document = entry.document();
            let session = lock(document.session());
            let admission = document.admission_inspection();
            json!({"document_id":entry.selector(),"runtime_document_id":document.id().value(),"title":session.title(),"path":session.path().map(|path|path.display().to_string()),"unsaved_changes":session.document().is_dirty(),"lifecycle":entry.state().label(),"visibility":entry.visibility().label(),"work":if session.compiling(){"Compiling"}else if session.launching(){"Launching"}else if session.game_running(){"Game running"}else if admission["active_operations"] != 0 {"Requests/workers"}else{"Idle"},"admission":admission,"error":entry.error()})
        }).collect();
        Ok(
            json!({"documents":documents,"limit":startup::DOCUMENT_LIMIT,"busy":self.0.factory.busy(),"can_create":self.0.factory.can_create()}),
        )
    }

    pub async fn new_document(&self, actor: Actor, present: bool) -> Result<Value, String> {
        let (send, receive) = oneshot::channel();
        self.enqueue(Command::New(actor, present, send))?;
        receive
            .await
            .map_err(|_| "The workspace command owner stopped".to_owned())?
    }

    pub async fn open_document(
        &self,
        actor: Actor,
        path: &Path,
        present: bool,
    ) -> Result<Value, String> {
        if !path.is_absolute() {
            return Err("open_document requires an absolute VMF path".into());
        }
        let (send, receive) = oneshot::channel();
        self.enqueue(Command::Open(actor, path.to_owned(), present, send))?;
        receive
            .await
            .map_err(|_| "The workspace command owner stopped".to_owned())?
    }

    pub async fn show_document(&self, selector: &str) -> Result<Value, String> {
        self.show_document_with_focus(selector, true).await
    }

    pub async fn show_document_with_focus(
        &self,
        selector: &str,
        focus: bool,
    ) -> Result<Value, String> {
        let (send, receive) = oneshot::channel();
        self.enqueue(Command::Show(selector.to_owned(), focus, send))?;
        receive
            .await
            .map_err(|_| "The workspace command owner stopped".to_owned())?
    }

    pub async fn save_all_named_maps(&self, actor: Actor) -> Result<Value, String> {
        let (send, receive) = oneshot::channel();
        {
            let mut queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if queue.stopping || queue.quitting {
                return Err("The workspace is closing".into());
            }
            if !queue.commands.is_empty() {
                return Err(
                    "The workspace command queue is busy; wait and retry Save all named maps"
                        .into(),
                );
            }
            let request = self.0.factory.prepare_save_all_named(actor)?;
            queue.commands.push_back(Command::SaveAll(request, send));
        }
        self.0.changed.notify_one();
        receive.await.map_err(|_| {
            "The Save all named maps owner stopped; inspect each document before retrying"
                .to_owned()
        })?
    }

    fn enqueue(&self, command: Command) -> Result<(), String> {
        let mut queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.stopping || queue.quitting {
            return Err("The workspace is closing".into());
        }
        if queue.commands.len() >= COMMAND_LIMIT {
            return Err("The workspace command queue is busy; wait and retry".into());
        }
        queue.commands.push_back(command);
        self.0.changed.notify_one();
        Ok(())
    }

    fn close_plan(
        &self,
        reservation: CloseReservation,
        metadata: Value,
    ) -> super::close::WorkspaceClose {
        super::close::WorkspaceClose {
            reservation,
            metadata,
            complete: self.completion(),
        }
    }

    fn completion(&self) -> super::close::CloseCompletion {
        let weak = Arc::downgrade(&self.0);
        Box::new(move |reservation, result, response, ui_state| {
            if let Some(core) = weak.upgrade() {
                let mut queue = core.queue.lock().unwrap_or_else(PoisonError::into_inner);
                queue.completed.push_back(CompletedClose {
                    reservation,
                    result,
                    response,
                    ui_state,
                });
                core.changed.notify_one();
            }
        })
    }

    pub fn close_native(
        &self,
        selector: &str,
        review: &DocumentCloseReview,
        discard_changes: bool,
    ) -> Result<(), String> {
        let entry = self.0.factory.resolve(Some(selector))?;
        let reservation = self.0.factory.reserve_close(selector)?;
        self.terminal_for(entry.document().id())?
            .commit_workspace_native(
                review,
                discard_changes,
                self.close_plan(reservation, metadata(&entry)),
            )
    }

    pub(super) fn fixed_server(
        &self,
        entry: &startup::WorkspaceDocument,
    ) -> Result<EditorServer, String> {
        let mut server = self
            .0
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&entry.document().id())
            .map(|entry| entry.server.clone())
            .ok_or("The document route is unavailable")?;
        server.stdio_output = self
            .0
            .stdio
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Ok(server)
    }
}

fn metadata(entry: &startup::WorkspaceDocument) -> Value {
    json!({"document_id":entry.selector(),"runtime_document_id":entry.document().id().value()})
}

pub(super) fn stamp_result(result: &mut CallToolResult, metadata: &Value) {
    result
        .meta
        .get_or_insert_with(MetaObject::new)
        .0
        .insert(DOCUMENT_META.into(), metadata.clone());
}

fn ui_wait(runtime: &tokio::runtime::Runtime, future: WorkspaceUiFuture) -> Result<(), String> {
    runtime.block_on(async {
        tokio::time::timeout(UI_TIMEOUT, future)
            .await
            .map_err(|_| "The workspace UI acknowledgement deadline expired".to_owned())?
    })
}

fn rollback_candidate(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    prepared: startup::PreparedDocument,
    entry: Arc<Entry>,
    error: String,
) -> Result<Value, String> {
    let document = prepared.document();
    let mut cleanup = document.shutdown();
    if let Some(mut owner) = entry
        .owner
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        cleanup = combine(cleanup, owner.shutdown());
    }
    if let Some(ui) = &workspace.0.ui {
        cleanup = combine(cleanup, ui_wait(runtime, (ui.retire)(document.id())));
        if cleanup.is_ok() {
            cleanup = ui_wait(runtime, (ui.release)(document.id()));
        }
    }
    match cleanup {
        Ok(()) => Err(error),
        Err(cleanup) => {
            let error = format!("{error}; candidate rollback failed: {cleanup}");
            let visibility = if workspace.0.ui.is_some() {
                DocumentVisibility::Hidden
            } else {
                DocumentVisibility::Headless
            };
            workspace
                .0
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(document.id(), entry);
            let failed = prepared.fail(error.clone(), visibility)?;
            Err(failed.error().unwrap_or(&error).to_owned())
        }
    }
}

fn create(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    actor: Actor,
    path: Option<&Path>,
    present: bool,
) -> Result<Value, String> {
    let mut prepared = if let Some(path) = path {
        workspace.0.factory.prepare_open(actor, path)?
    } else {
        workspace.0.factory.prepare_new(actor)?
    };
    let document = prepared.document();
    let selector = prepared.selector().to_owned();
    let message = prepared.open_message().map(str::to_owned);
    let entry = workspace.entry(document.clone())?;
    if let Some(ui) = &workspace.0.ui
        && let Err(error) = ui_wait(
            runtime,
            (ui.create)(
                document.clone(),
                selector.clone(),
                entry.server.terminal.clone().expect("Missing Close owner"),
                present,
            ),
        )
    {
        return rollback_candidate(workspace, runtime, prepared, entry, error);
    }
    let visibility = if workspace.0.ui.is_none() {
        DocumentVisibility::Headless
    } else if present {
        DocumentVisibility::Visible
    } else {
        DocumentVisibility::Hidden
    };
    workspace
        .0
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(document.id(), entry.clone());
    if let Err(error) = prepared.try_publish(visibility) {
        workspace
            .0
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&document.id());
        return rollback_candidate(workspace, runtime, prepared, entry, error);
    }
    Ok(
        json!({"document_id":selector,"runtime_document_id":document.id().value(),"visibility":visibility.label(),"message":message}),
    )
}

fn show(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    selector: &str,
    focus: bool,
) -> Result<Value, String> {
    let entry = workspace.0.factory.resolve(Some(selector))?;
    let ui = workspace
        .0
        .ui
        .as_ref()
        .ok_or("The headless workspace cannot show native windows")?;
    ui_wait(runtime, (ui.show)(entry.document().id(), focus))?;
    workspace
        .0
        .factory
        .set_visibility(entry.document().id(), DocumentVisibility::Visible)?;
    Ok(
        json!({"document_id":selector,"runtime_document_id":entry.document().id().value(),"visibility":"visible","focus_requested":focus}),
    )
}

fn finish_close(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    mut completed: CompletedClose,
) {
    let id = completed.reservation.document().id();
    let terminal = completed.reservation.disposition() == CloseDisposition::Terminal;
    let metadata =
        json!({"document_id":completed.reservation.selector(),"runtime_document_id":id.value()});
    let entry = workspace
        .0
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&id)
        .cloned();
    let joined = entry
        .and_then(|entry| {
            entry
                .owner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
        })
        .map_or(Ok(()), |mut owner| owner.shutdown());
    let mut result = combine(completed.result, joined);
    if result.is_ok()
        && let Some(ui) = &workspace.0.ui
    {
        result = ui_wait(runtime, (ui.release)(id));
    }
    result = completed.reservation.finish(result);
    if result.is_ok() {
        workspace
            .0
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
    if let Err(error) = &result {
        workspace
            .0
            .errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error.clone());
    }
    if terminal {
        finish_transport(workspace, runtime);
    }
    if let Some(response) = completed.response {
        let mut output = CallToolResult::structured(
            json!({"document_id":id.value(),"closed":result.is_ok(),"terminal":false,"ui":completed.ui_state,"error":result.as_ref().err()}),
        );
        output.is_error = Some(result.is_err());
        stamp_result(&mut output, &metadata);
        let _ = response.send(output);
    }
}

fn combine(left: Result<(), String>, right: Result<(), String>) -> Result<(), String> {
    match (left, right) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(left), Err(right)) => Err(format!("{left}; {right}")),
    }
}

fn finish_transport(workspace: &Workspace, runtime: &tokio::runtime::Runtime) {
    let stop = {
        let mut transport = workspace
            .0
            .stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        transport.terminal_finishing = true;
        transport.stop.clone()
    };
    if let Some(stop) = stop {
        stop();
    }
    if let Some(ui) = &workspace.0.ui
        && let Err(error) = ui_wait(runtime, (ui.quit)())
    {
        workspace
            .0
            .errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error);
    }
}

fn failed_worker_cleanup(workspace: &Workspace, error: &str) {
    let entries: Vec<_> = workspace
        .0
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for entry in entries {
        if let Some(mut owner) = entry
            .owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            && let Err(error) = owner.shutdown()
        {
            workspace
                .0
                .errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(error);
        }
    }
    loop {
        let completed = workspace
            .0
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .completed
            .pop_front();
        let Some(mut completed) = completed else {
            break;
        };
        let id = completed.reservation.document().id();
        let metadata = json!({"document_id":completed.reservation.selector(),"runtime_document_id":id.value()});
        let result = completed
            .reservation
            .finish(combine(Err(error.to_owned()), completed.result));
        if let Some(response) = completed.response {
            let mut output = CallToolResult::structured(
                json!({"document_id":id.value(),"closed":false,"terminal":completed.reservation.disposition()==CloseDisposition::Terminal,"ui":completed.ui_state,"error":result.err()}),
            );
            output.is_error = Some(true);
            stamp_result(&mut output, &metadata);
            let _ = response.send(output);
        }
    }
}

fn run_worker(workspace: Workspace, runtime: tokio::runtime::Runtime) {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_commands(&workspace, &runtime)
    }));
    if ran.is_err() {
        let error = "The workspace command worker panicked";
        workspace
            .0
            .errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error.into());
        workspace.0.factory.close_admission();
        {
            let mut queue = workspace
                .0
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            queue.stopping = true;
            queue.commands.clear();
        }
        failed_worker_cleanup(&workspace, error);
        finish_transport(&workspace, &runtime);
        return;
    }
    drain_workspace(&workspace, &runtime);
}

fn run_commands(workspace: &Workspace, runtime: &tokio::runtime::Runtime) {
    loop {
        let mut queue = workspace
            .0
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while queue.completed.is_empty()
            && queue.commands.is_empty()
            && queue.quit.is_none()
            && !queue.stopping
        {
            queue = workspace
                .0
                .changed
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if let Some(completed) = queue.completed.pop_front() {
            drop(queue);
            finish_close(workspace, runtime, completed);
            continue;
        }
        if let Some(work) = queue.quit.take() {
            drop(queue);
            quit::finish(workspace, runtime, work);
            break;
        }
        if queue.stopping {
            drop(queue);
            break;
        }
        let command = queue
            .commands
            .pop_front()
            .expect("Workspace command missing");
        queue.executing = true;
        drop(queue);

        let response = match command {
            Command::New(actor, present, response) => {
                if response.is_closed() {
                    None
                } else {
                    Some((response, create(workspace, runtime, actor, None, present)))
                }
            }
            Command::Open(actor, path, present, response) => {
                if response.is_closed() {
                    None
                } else {
                    Some((
                        response,
                        create(workspace, runtime, actor, Some(&path), present),
                    ))
                }
            }
            Command::SaveAll(request, response) => Some((response, Ok(request.run()))),
            Command::Show(selector, focus, response) => {
                if response.is_closed() {
                    None
                } else {
                    Some((response, show(workspace, runtime, &selector, focus)))
                }
            }
        };
        workspace
            .0
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .executing = false;
        if let Some((response, result)) = response {
            let _ = response.send(result);
        }
    }
}

fn drain_workspace(workspace: &Workspace, runtime: &tokio::runtime::Runtime) {
    let entries: Vec<_> = workspace
        .0
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for entry in entries {
        if let Some(mut owner) = entry
            .owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            && let Err(error) = owner.shutdown()
        {
            workspace
                .0
                .errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(error);
        }
    }
    loop {
        let completed = workspace
            .0
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .completed
            .pop_front();
        let Some(completed) = completed else {
            break;
        };
        finish_close(workspace, runtime, completed);
    }
}

pub fn serve_workspace_stdio(server: WorkspaceServer) -> Result<(), String> {
    let flags;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let (read, write, restore) = {
        let _entered = runtime.enter();
        super::stdio::process_transport()?
    };
    flags = restore;
    let result = runtime.block_on(run_workspace_stdio(server, read, write));
    drop(runtime);
    combine(result, flags.finish())
}

async fn run_workspace_stdio<R, W>(server: WorkspaceServer, read: R, write: W) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (transport, output) = super::stdio::transport(read, write);
    let workspace = server.0.clone();
    *workspace
        .0
        .stdio
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(output);
    let running = server
        .serve(transport)
        .await
        .map_err(|error| error.to_string())?;
    let cancellation = Mutex::new(Some(running.cancellation_token()));
    let stop: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        if let Some(cancellation) = cancellation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            cancellation.cancel();
        }
    });
    let finishing = {
        let mut transport = workspace
            .0
            .stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        transport.stop = Some(stop.clone());
        transport.terminal_finishing
    };
    if finishing {
        stop();
    }
    let result = match running.waiting().await.map_err(|error| error.to_string())? {
        rmcp::service::QuitReason::Closed | rmcp::service::QuitReason::Cancelled => Ok(()),
        rmcp::service::QuitReason::JoinError(error) => {
            Err(format!("MCP workspace stdio service failed: {error}"))
        }
        reason => Err(format!(
            "MCP workspace stdio service stopped unexpectedly: {reason:?}"
        )),
    };
    workspace
        .0
        .stdio
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    workspace
        .0
        .stop
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stop
        .take();
    result
}
