use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use super::document_host::SharedHost;
use super::*;
use editor_core::startup::{DocumentVisibility, Workspace as Factory};

mod controls;
mod presentation;

const MAX_UI_COMMANDS: usize = 8;
const DISPATCH_INTERVAL: Duration = Duration::from_millis(16);
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

type UiFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type SharedOwner = Rc<RefCell<Owner>>;
type WeakOwner = std::rc::Weak<RefCell<Owner>>;

enum Action {
    Create(DocumentHandle, String, mcp::TerminalClose, bool),
    Show(runtime::DocumentId, bool),
    Retire(runtime::DocumentId),
    Release(runtime::DocumentId),
    Quit,
}

struct Command {
    action: Action,
    reply: oneshot::Sender<Result<(), String>>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Open,
    Closing,
    Closed,
}

#[derive(Default)]
struct Queue {
    phase: Phase,
    commands: VecDeque<Command>,
    terminal_quit: Option<Command>,
    final_retirement: Vec<(runtime::DocumentId, Result<(), String>)>,
    quit: Option<Result<(), String>>,
}

impl Queue {
    fn closed_result(&self, action: &Action) -> Result<(), String> {
        match action {
            Action::Retire(id) | Action::Release(id) => self
                .final_retirement
                .iter()
                .find(|(document, _)| document == id)
                .map_or(Ok(()), |(_, result)| result.clone()),
            Action::Quit => self.quit.clone().unwrap_or_else(|| {
                Err("The UI event loop stopped before the Close quit handoff".into())
            }),
            _ => Err("The UI event loop is no longer accepting document commands".into()),
        }
    }
}

#[derive(Clone, Default)]
struct Proxy(Arc<Mutex<Queue>>);

struct Pending {
    cancelled: Arc<AtomicBool>,
    cancellable: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.cancellable {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

impl Proxy {
    fn request(&self, action: Action) -> UiFuture {
        let (reply, receipt) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let pending = Pending {
            cancelled: cancelled.clone(),
            cancellable: matches!(action, Action::Create(..) | Action::Show(..)),
        };
        let immediate = {
            let mut queue = self.0.lock().expect("Workspace UI queue poisoned");
            queue
                .commands
                .retain(|command| !command.cancelled.load(Ordering::Acquire));
            match queue.phase {
                Phase::Closed => Some(queue.closed_result(&action)),
                Phase::Closing if matches!(action, Action::Create(..) | Action::Show(..)) => {
                    Some(Err("The UI event loop is closing".into()))
                }
                _ if matches!(action, Action::Quit) && queue.terminal_quit.is_some() => {
                    Some(Err("A terminal UI quit handoff is already queued".into()))
                }
                _ if matches!(action, Action::Quit) => {
                    queue.terminal_quit = Some(Command {
                        action,
                        reply,
                        cancelled,
                    });
                    None
                }
                _ if queue.commands.len() == MAX_UI_COMMANDS => {
                    Some(Err("Workspace UI is busy; try again".into()))
                }
                _ => {
                    queue.commands.push_back(Command {
                        action,
                        reply,
                        cancelled,
                    });
                    None
                }
            }
        };
        if let Some(result) = immediate {
            return Box::pin(async move { result });
        }
        let wake = slint::invoke_from_event_loop(|| {})
            .map_err(|error| format!("Could not wake the document UI owner: {error}"));
        Box::pin(async move {
            let _pending = pending;
            wake?;
            receipt
                .await
                .map_err(|_| "The document UI owner lost its acknowledgement".to_owned())?
        })
    }

    fn callbacks(&self) -> mcp::WorkspaceUi {
        let (create, show, retire, release, quit) = (
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
        );
        mcp::WorkspaceUi::new(
            move |document, selector, terminal, present| {
                create.request(Action::Create(document, selector, terminal, present))
            },
            move |document, focus| {
                let show = show.clone();
                Box::pin(async move {
                    if !focus {
                        presentation::prepare_silent_show().await?;
                    }
                    show.request(Action::Show(document, focus)).await
                })
            },
            move |document| retire.request(Action::Retire(document)),
            move |document| release.request(Action::Release(document)),
            move || quit.request(Action::Quit),
        )
    }
}

pub(super) struct Owner {
    ctx: GpuContext,
    hosts: Vec<(String, SharedHost)>,
    workspace: Option<mcp::Workspace>,
    status: String,
    proxy: Proxy,
    timer: slint::Timer,
    weak: WeakOwner,
    dialog: Option<Rc<controls::Controller>>,
    refreshed: Instant,
    finished: bool,
}

impl Owner {
    fn new(ctx: GpuContext) -> SharedOwner {
        let owner = Rc::new_cyclic(|weak| {
            RefCell::new(Self {
                ctx,
                hosts: Vec::new(),
                workspace: None,
                status: "MCP starting".into(),
                proxy: Proxy::default(),
                timer: slint::Timer::default(),
                weak: weak.clone(),
                dialog: None,
                refreshed: Instant::now(),
                finished: false,
            })
        });
        let weak = Rc::downgrade(&owner);
        owner
            .borrow()
            .timer
            .start(slint::TimerMode::Repeated, DISPATCH_INTERVAL, move || {
                if let Some(owner) = weak.upgrade() {
                    dispatch(&owner);
                }
            });
        owner
    }

    fn bind_host(
        &self,
        selector: &str,
        host: &SharedHost,
        terminal: mcp::TerminalClose,
    ) -> Result<(), String> {
        let workspace = self
            .workspace
            .clone()
            .ok_or("The workspace command owner is not ready")?;
        let mut host = host.borrow_mut();
        host.terminal_close = Some(terminal);
        host.workspace_close = Some((workspace, selector.to_owned()));
        let weak = self.weak.clone();
        let selector = selector.to_owned();
        host.window.app().on_open_documents(move || {
            let Some(owner) = weak.upgrade() else { return };
            let origin = owner.borrow().hosts.iter().find_map(|(id, host)| {
                let host = host.borrow();
                (id == &selector && host.window.app.is_some()).then(|| host.document_id())
            });
            if let Some(origin) = origin {
                controls::open(&owner, origin);
            }
        });
        let weak = self.weak.clone();
        let selector = host.workspace_close.as_ref().unwrap().1.clone();
        host.window.app().window().on_close_requested(move || {
            if let Some(owner) = weak.upgrade() {
                controls::request_close(&owner, &selector);
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(())
    }

    fn create(
        &mut self,
        document: DocumentHandle,
        selector: String,
        terminal: mcp::TerminalClose,
        present: bool,
    ) -> Result<(), String> {
        if terminal.document_id() != document.id() {
            return Err("The Close owner belongs to another document".into());
        }
        if self
            .hosts
            .iter()
            .any(|(id, host)| id == &selector || host.borrow().document_id() == document.id())
        {
            return Err("A UI host already owns this document".into());
        }
        if self.hosts.len() == editor_core::startup::DOCUMENT_LIMIT {
            return Err("The two-document UI capacity is full".into());
        }
        let window = create_document_window(
            document.clone(),
            self.ctx.clone(),
            self.status.clone(),
            None,
        )
        .map_err(|error| error.to_string())?;
        let host = DocumentHost::new(window, document);
        if let Err(error) = self.bind_host(&selector, &host, terminal) {
            return self.failed_candidate(selector, host, error);
        }
        if present {
            let shown = host.borrow().window.app().show();
            if let Err(error) = shown {
                return self.failed_candidate(selector, host, error.to_string());
            }
        }
        self.hosts.push((selector, host));
        Ok(())
    }

    fn failed_candidate(
        &mut self,
        selector: String,
        host: SharedHost,
        error: String,
    ) -> Result<(), String> {
        let error = host
            .borrow_mut()
            .finish(Err(error.into()))
            .unwrap_err()
            .to_string();
        if host
            .borrow()
            .window
            .retired
            .as_ref()
            .is_some_and(Result::is_err)
        {
            self.hosts.push((selector, host));
        }
        Err(error)
    }

    fn show(&mut self, document: runtime::DocumentId, focus: bool) -> Result<(), String> {
        let host = self
            .hosts
            .iter()
            .find(|(_, host)| host.borrow().document_id() == document)
            .ok_or("The requested document has no UI host")?;
        let host = host.1.borrow();
        let app = host
            .window
            .app
            .as_ref()
            .ok_or("The document UI has retired")?;
        app.set_background_display(!focus);
        app.show().map_err(|error| error.to_string())
    }

    fn close_dialog_for(&mut self, document: runtime::DocumentId) -> Result<(), String> {
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.origin == document)
        {
            let dialog = self.dialog.take().unwrap();
            return dialog.window.hide().map_err(|error| error.to_string());
        }
        Ok(())
    }

    fn retire(&mut self, document: runtime::DocumentId) -> Result<(), String> {
        let dialog = self.close_dialog_for(document);
        let retired = self
            .hosts
            .iter()
            .find(|(_, host)| host.borrow().document_id() == document)
            .map_or(Ok(()), |(_, host)| {
                host.borrow_mut().window.retire().map(|_| ())
            });
        combine(dialog, retired)
    }

    fn release(&mut self, document: runtime::DocumentId) -> Result<(), String> {
        let Some(index) = self
            .hosts
            .iter()
            .position(|(_, host)| host.borrow().document_id() == document)
        else {
            return Ok(());
        };
        let result = self.hosts[index].1.borrow_mut().finish(Ok(()));
        result.map_err(|error| error.to_string())?;
        self.hosts.remove(index);
        Ok(())
    }

    fn process(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Create(document, selector, terminal, present) => {
                self.create(document, selector, terminal, present)
            }
            Action::Show(document, focus) => self.show(document, focus),
            Action::Retire(document) => self.retire(document),
            Action::Release(document) => self.release(document),
            Action::Quit => {
                let result = slint::quit_event_loop().map_err(|error| error.to_string());
                self.proxy
                    .0
                    .lock()
                    .expect("Workspace UI queue poisoned")
                    .quit = Some(result.clone());
                result
            }
        }
    }

    fn finish(
        &mut self,
        primary: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.finished = true;
        self.timer.stop();
        {
            let mut queue = self.proxy.0.lock().expect("Workspace UI queue poisoned");
            queue.phase = Phase::Closing;
            if let Some(command) = queue.terminal_quit.take() {
                let _ = command.reply.send(Err(
                    "The UI event loop ended before the quit handoff ran".into(),
                ));
            }
            for command in queue.commands.drain(..) {
                let _ = command.reply.send(Err(
                    "The UI event loop ended before this document command ran".into(),
                ));
            }
        }
        let mut errors = primary
            .err()
            .map(|error| error.to_string())
            .into_iter()
            .collect::<Vec<_>>();
        let dialog_result = self.dialog.take().map(|dialog| {
            (
                dialog.origin,
                dialog.window.hide().map_err(|error| error.to_string()),
            )
        });
        let mut results = Vec::new();
        for (_, host) in self.hosts.drain(..) {
            let mut host = host.borrow_mut();
            let result = host.finish(Ok(())).map_err(|error| error.to_string());
            let dialog = dialog_result
                .as_ref()
                .filter(|(id, _)| *id == host.document_id())
                .map_or(Ok(()), |(_, result)| result.clone());
            let result = combine(dialog, result);
            if let Err(error) = &result {
                errors.push(error.clone());
            }
            results.push((host.document_id(), result));
        }
        {
            let mut queue = self.proxy.0.lock().expect("Workspace UI queue poisoned");
            queue.final_retirement = results;
            queue.phase = Phase::Closed;
            if let Some(command) = queue.terminal_quit.take() {
                let result = queue.closed_result(&command.action);
                let _ = command.reply.send(result);
            }
            while let Some(command) = queue.commands.pop_front() {
                let result = queue.closed_result(&command.action);
                let _ = command.reply.send(result);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; ").into())
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = self.finish(Ok(()))
        {
            eprintln!("Workspace UI cleanup failed: {error}");
        }
    }
}

fn combine(left: Result<(), String>, right: Result<(), String>) -> Result<(), String> {
    match (left, right) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(left), Err(right)) => Err(format!("{left}; {right}")),
    }
}

fn dispatch(owner: &SharedOwner) {
    let quit = owner
        .borrow()
        .proxy
        .0
        .lock()
        .expect("Workspace UI queue poisoned")
        .terminal_quit
        .take();
    if let Some(command) = quit {
        let result = owner.borrow_mut().process(command.action);
        let _ = command.reply.send(result);
    }
    for _ in 0..MAX_UI_COMMANDS {
        let command = owner
            .borrow()
            .proxy
            .0
            .lock()
            .expect("Workspace UI queue poisoned")
            .commands
            .pop_front();
        let Some(command) = command else { break };
        let result = if command.cancelled.load(Ordering::Acquire) {
            Err("The document UI request was cancelled before execution".into())
        } else {
            owner.borrow_mut().process(command.action)
        };
        let _ = command.reply.send(result);
    }
    if owner.borrow().refreshed.elapsed() >= REFRESH_INTERVAL {
        owner.borrow_mut().refreshed = Instant::now();
        controls::refresh(owner);
    }
}

pub(super) fn run(
    factory: Factory,
    document: DocumentHandle,
    ctx: GpuContext,
    open_message: Option<Result<String, String>>,
    configure: impl FnOnce(
        mcp::WorkspaceUi,
    ) -> Result<(mcp::Workspace, String), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize WGPU's debug lock TLS before Slint retains the queue.
    ctx.queue.submit([]);
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        })
        .select()?;
    let owner = Owner::new(ctx.clone());
    let result = (|| {
        let initial = factory
            .documents()
            .into_iter()
            .find(|entry| entry.document().id() == document.id())
            .ok_or("The startup document is missing from its workspace")?;
        let window =
            create_document_window(document.clone(), ctx, "MCP starting".into(), open_message)?;
        let host = DocumentHost::new(window, document.clone());
        owner
            .borrow_mut()
            .hosts
            .push((initial.selector().to_owned(), host.clone()));
        host.borrow().window.app().show()?;
        factory.set_visibility(document.id(), DocumentVisibility::Visible)?;
        let callbacks = owner.borrow().proxy.callbacks();
        let (workspace, status) = configure(callbacks)?;
        {
            let mut state = owner.borrow_mut();
            state.workspace = Some(workspace.clone());
            state.status = status.clone();
            host.borrow().window.app().set_mcp_status(status.into());
            state.bind_host(
                initial.selector(),
                &host,
                workspace.terminal_for(document.id())?,
            )?;
        }
        slint::run_event_loop_until_quit()?;
        Ok(())
    })();
    owner.borrow_mut().finish(result)
}
