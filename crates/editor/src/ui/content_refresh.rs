use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    ContentRefreshWindow,
    runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock},
    session::{Actor, Session},
};

pub(super) struct Controller {
    pub(super) window: ContentRefreshWindow,
    reviewed: Rc<RefCell<Value>>,
}

fn present(window: &ContentRefreshWindow, state: &Value) {
    window.set_current_path(
        state["current_config"]["gameinfo"]
            .as_str()
            .unwrap_or("No mounted game configuration")
            .into(),
    );
    window.set_status(
        state["summary"]
            .as_str()
            .or_else(|| state["status"].as_str())
            .unwrap_or_default()
            .into(),
    );
    window.set_blocker(state["blocker"].as_str().unwrap_or_default().into());
    window.set_error(state["error"].as_str().unwrap_or_default().into());
    window.set_watch_error(state["watch"]["error"].as_str().unwrap_or_default().into());
    window.set_watching(state["watch"]["enabled"] == true);
    window.set_busy(state["worker_active"] == true);
    window.set_can_reload(state["can_reload"] == true);
}

fn refresh(window: &ContentRefreshWindow, reviewed: &RefCell<Value>, session: &Session) {
    let state = session.content_refresh_report();
    present(window, &state);
    *reviewed.borrow_mut() = state;
}

fn admitted(
    window: &slint::Weak<ContentRefreshWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&ContentRefreshWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(&window, session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn close(window: &ContentRefreshWindow, session: &mut Session) {
    let state = session.content_refresh_report();
    if let Some(token) = state["pending"]["token"].as_str() {
        let result = token
            .parse::<u64>()
            .map_err(|_| "Content refresh cancellation token is unavailable".to_owned())
            .and_then(|token| session.cancel_content_refresh(Actor::User, token));
        if let Err(error) = result {
            window.set_message(error.into());
            present(window, &session.content_refresh_report());
            return;
        }
    }
    if let Err(error) = window.hide() {
        window.set_message(error.to_string().into());
    }
}

fn spawn_reload(
    window: &ContentRefreshWindow,
    access: &DocumentAccess,
    shared: &SharedSession,
    reviewed: &RefCell<Value>,
) {
    let intent = reviewed.borrow()["intent"].as_u64();
    let request = lock(shared).begin_content_refresh(intent);
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            window.set_message(error.into());
            refresh(window, reviewed, &lock(shared));
            return;
        }
    };
    let token = request.token();
    window.set_message("".into());
    refresh(window, reviewed, &lock(shared));
    let worker_access = access.clone();
    let worker_session = shared.clone();
    let worker_window = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("content-refresh".into())
        .spawn(move || {
            let result = worker_access.with_shared(|shared| {
                let completed =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request.run()))
                        .map_err(|_| "Content refresh worker panicked".to_owned())?;
                lock(shared).finish_content_refresh(Actor::User, completed)
            });
            if let Err(error) = &result {
                lock(&worker_session).fail_content_refresh(token, error.clone());
            }
            let _ = worker_window.upgrade_in_event_loop(|window| window.invoke_worker_finished());
        });
    if let Err(error) = spawned {
        let error = format!("Could not start content refresh: {error}");
        lock(shared).fail_content_refresh(token, error.clone());
        window.set_message(error.into());
        refresh(window, reviewed, &lock(shared));
    }
}

impl Controller {
    pub(super) fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = ContentRefreshWindow::new()?;
        let reviewed = Rc::new(RefCell::new(Value::Null));
        window.on_watch_changed({
            let (window, target, reviewed) =
                (window.as_weak(), access.downgrade(), reviewed.clone());
            move |enabled| {
                admitted(&window, &target, |window, session| {
                    window.set_message("".into());
                    if let Err(error) = session.set_content_watch(Actor::User, enabled) {
                        window.set_message(error.into());
                    }
                    refresh(window, &reviewed, session);
                });
            }
        });
        window.on_reload_content({
            let (window, target, reviewed) =
                (window.as_weak(), access.downgrade(), reviewed.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_shared(|shared| {
                        spawn_reload(&window, &access, shared, &reviewed);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_worker_finished({
            let (window, target, reviewed) =
                (window.as_weak(), access.downgrade(), reviewed.clone());
            move || {
                admitted(&window, &target, |window, session| {
                    refresh(window, &reviewed, session);
                });
            }
        });
        window.on_close_dialog({
            let (window, target) = (window.as_weak(), access.downgrade());
            move || admitted(&window, &target, close)
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_close_dialog();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self { window, reviewed })
    }

    pub(super) fn refresh(&self, session: &Session) {
        refresh(&self.window, &self.reviewed, session);
    }

    pub(super) fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        self.refresh(session);
        self.window.show()
    }
}
