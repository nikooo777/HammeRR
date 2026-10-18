use std::sync::{Arc, Mutex, mpsc};

use super::document_window::UiRetired;
use super::*;

type RetirementResult = Result<UiRetired, String>;
pub(super) type RetirementReceipt = mpsc::Receiver<RetirementResult>;
pub(super) type SharedHost = Rc<RefCell<DocumentHost>>;
pub(super) type WeakHost = std::rc::Weak<RefCell<DocumentHost>>;

#[derive(Default)]
struct RetirementState {
    requested: bool,
    result: Option<RetirementResult>,
    handoff_error: Option<String>,
    waiters: Vec<mpsc::SyncSender<RetirementResult>>,
}

impl RetirementState {
    fn complete(&mut self, result: RetirementResult) {
        if self.result.is_some() {
            return;
        }
        let result = match (&self.handoff_error, result) {
            (None, result) => result,
            (Some(error), Ok(_)) => Err(error.clone()),
            (Some(handoff), Err(retire)) => Err(format!("{handoff}; {retire}")),
        };
        self.result = Some(result.clone());
        for waiter in self.waiters.drain(..) {
            let _ = waiter.try_send(result.clone());
        }
    }
}

#[derive(Clone)]
pub(super) struct RetirementHandle {
    document: runtime::DocumentId,
    app: slint::Weak<AppWindow>,
    state: Arc<Mutex<RetirementState>>,
}

impl RetirementHandle {
    pub(super) fn request(
        &self,
        document: runtime::DocumentId,
    ) -> Result<RetirementReceipt, String> {
        if document != self.document {
            return Err("UI retirement was requested for another document".into());
        }
        let mut state = self.state.lock().expect("UI retirement lock poisoned");
        if state.result.is_none()
            && let Some(error) = &state.handoff_error
        {
            return Err(error.clone());
        }
        let (sender, receipt) = mpsc::sync_channel(1);
        if let Some(result) = &state.result {
            let _ = sender.try_send(result.clone());
            return Ok(receipt);
        }
        state.waiters.push(sender);
        if !std::mem::replace(&mut state.requested, true)
            && let Err(error) = self
                .app
                .upgrade_in_event_loop(|app| app.invoke_retire_document())
        {
            let error = format!("Could not schedule UI retirement: {error}");
            state.handoff_error = Some(error.clone());
            return Err(error);
        }
        Ok(receipt)
    }
}

pub(super) struct DocumentHost {
    pub(super) window: DocumentWindow,
    document: DocumentHandle,
    retirement: RetirementHandle,
    pub(super) terminal_close: Option<mcp::TerminalClose>,
    pub(super) workspace_close: Option<(mcp::Workspace, String)>,
    terminal: bool,
    terminal_finished: bool,
    terminal_error: Option<String>,
    finished: bool,
}

impl DocumentHost {
    pub(super) fn new(window: DocumentWindow, document: DocumentHandle) -> SharedHost {
        assert_eq!(
            window.document,
            document.id(),
            "Document host identity mismatch"
        );
        let retirement = RetirementHandle {
            document: document.id(),
            app: window.app().as_weak(),
            state: Arc::default(),
        };
        let host = Rc::new(RefCell::new(Self {
            window,
            document,
            retirement,
            terminal_close: None,
            workspace_close: None,
            terminal: false,
            terminal_finished: false,
            terminal_error: None,
            finished: false,
        }));
        let app = host.borrow().window.app().clone_strong();
        app.on_retire_document({
            let host = Rc::downgrade(&host);
            move || {
                if let Some(host) = host.upgrade() {
                    host.borrow_mut().retire_requested();
                }
            }
        });
        app.window().on_close_requested({
            let host = Rc::downgrade(&host);
            move || {
                let Some(owner) = host.upgrade() else {
                    return slint::CloseRequestResponse::HideWindow;
                };
                let (app, ui) = {
                    let owner = owner.borrow();
                    let Some(app) = owner.window.app.as_ref() else {
                        return slint::CloseRequestResponse::HideWindow;
                    };
                    (app.clone_strong(), owner.window.weak_ui())
                };
                request_terminal_close(&app, &ui, &host);
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        host
    }

    pub(super) fn retirement(&self) -> RetirementHandle {
        self.retirement.clone()
    }

    pub(super) fn document_id(&self) -> runtime::DocumentId {
        self.document.id()
    }

    pub(super) fn accept_terminal_close(host: &WeakHost) {
        let Some(host) = host.upgrade() else {
            return;
        };
        let (retirement, document) = {
            let mut host = host.borrow_mut();
            host.terminal = true;
            (host.retirement(), host.document.id())
        };
        if retirement.request(document).is_err() {
            host.borrow_mut().retire_requested();
        }
    }

    fn retire_requested(&mut self) {
        let result = self.window.retire();
        self.retirement
            .state
            .lock()
            .expect("UI retirement lock poisoned")
            .complete(result);
        if self.terminal && !self.terminal_finished {
            self.terminal_finished = true;
            if let Err(error) = slint::quit_event_loop() {
                let mut error = format!("Could not quit after accepted Close: {error}");
                if let Err(drain) = self.document.shutdown() {
                    error.push_str(&format!("; terminal cleanup failed: {drain}"));
                }
                self.terminal_error = Some(error);
            }
        }
    }

    pub(super) fn finish(
        &mut self,
        primary: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.finished = true;
        let primary = match (primary, &self.terminal_error) {
            (result, None) => result,
            (Ok(()), Some(error)) => Err(error.clone().into()),
            (Err(primary), Some(error)) => Err(format!("{primary}; {error}").into()),
        };
        let result = self.window.finish(primary);
        self.retirement
            .state
            .lock()
            .expect("UI retirement lock poisoned")
            .complete(self.window.retired.as_ref().unwrap().clone());
        let handoff = self
            .retirement
            .state
            .lock()
            .expect("UI retirement lock poisoned")
            .handoff_error
            .clone();
        match (result, handoff) {
            (result, None) => result,
            (Ok(()), Some(error)) => Err(error.into()),
            (Err(primary), Some(error)) => Err(format!("{primary}; {error}").into()),
        }
    }
}

impl Drop for DocumentHost {
    fn drop(&mut self) {
        let result = self.window.retire();
        if !self.finished
            && let Err(error) = &result
        {
            eprintln!("UI retirement failed: {error}");
        }
        self.retirement
            .state
            .lock()
            .expect("UI retirement lock poisoned")
            .complete(result);
    }
}
