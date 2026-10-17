use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    AppWindow, GridDefaultsWindow,
    runtime::DocumentAccess,
    session::{Actor, Session},
};

pub fn refresh_status(app: &AppWindow, session: &Session) {
    app.set_grid_defaults_error(!session.grid_preferences_state()["error"].is_null());
}

pub struct Controller {
    pub window: GridDefaultsWindow,
    snapshot: Rc<RefCell<String>>,
}

fn grid(value: &Value) -> String {
    format!(
        "{} units · snapping {} · grid {}",
        value["grid_size"],
        if value["snap"] == true { "on" } else { "off" },
        if value["grid_visible"] == true {
            "visible"
        } else {
            "hidden"
        }
    )
}

fn refresh(window: &GridDefaultsWindow, session: &Session, snapshot: &RefCell<String>) {
    let state = session.grid_preferences_state();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    window.set_current(format!("Current session: {}", grid(&state["current"])).into());
    window.set_defaults(if state["ready"] != true {
        "Saved defaults could not load.".into()
    } else {
        format!(
            "{}: {}",
            if state["stored"] == true {
                "Saved defaults"
            } else {
                "Factory defaults · no saved file"
            },
            grid(&state["defaults"])
        )
        .into()
    });
    window.set_storage(
        format!(
            "Storage: {}",
            state["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    window.set_status(state["error"].as_str().or(state["storage_error"].as_str()).unwrap_or("Startup uses these defaults. Apply saved defaults changes only the current session.").into());
    window.set_ready(state["ready"] == true);
}

fn act(
    window: &GridDefaultsWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    action: &str,
) {
    let result = session.change_grid_preferences(Actor::User, action, &snapshot.borrow());
    match result {
        Ok(state) if state["write_committed"]==true && state["ready"]!=true=>window.set_message("Grid defaults were written but could not reload. Review the storage error before continuing.".into()),
        Ok(_) => window.set_message(
            match action {
                "save" => "Current grid saved for future startups.",
                "apply" => "Saved grid applied. Workplane and brush depth are unchanged.",
                "reset" => "Factory defaults saved. Apply saved defaults to use them now.",
                _ => "Grid defaults reloaded. Current grid is unchanged.",
            }
            .into(),
        ),
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, snapshot);
}

impl Controller {
    #[cfg(test)]
    pub fn new(shared: &crate::runtime::SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = GridDefaultsWindow::new()?;
        let snapshot = Rc::new(RefCell::new(String::new()));
        let callback = |action: &'static str| {
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        act(&window, session, &snapshot, action);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        };
        window.on_save(callback("save"));
        window.on_apply(callback("apply"));
        window.on_reload(callback("reload"));
        window.on_reset(callback("reset"));
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, snapshot })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.snapshot);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        self.refresh(session);
        self.window.show()
    }
}

#[cfg(test)]
mod tests;
