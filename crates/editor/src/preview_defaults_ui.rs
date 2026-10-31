use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::{
    PreviewDefaultRow, PreviewDefaultsWindow,
    runtime::DocumentAccess,
    session::{Actor, Session},
};

pub struct Controller {
    pub window: PreviewDefaultsWindow,
    snapshot: Rc<RefCell<String>>,
}

fn enabled(value: &Value) -> slint::SharedString {
    match value.as_bool() {
        Some(true) => "On".into(),
        Some(false) => "Off".into(),
        None => "Unavailable".into(),
    }
}

fn refresh(window: &PreviewDefaultsWindow, session: &Session, snapshot: &RefCell<String>) {
    let state = session.preview_preferences_state();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    let rows: Vec<_> = [
        ("selection_outlines", "Selection outlines"),
        ("selection_tint", "Selection tint"),
        ("entity_images", "Entity images"),
        ("authored_models", "Authored models"),
        ("sprite_resources", "Sprite resources"),
        ("entity_helpers", "Entity helpers"),
        ("selection_bounds", "Selection bounds"),
        ("entity_model_guides", "Entity model guides"),
        ("rope_preview", "Rope preview"),
        ("sky_preview", "2D sky preview"),
    ]
    .into_iter()
    .map(|(key, label)| PreviewDefaultRow {
        label: label.into(),
        current: enabled(&state["current"][key]),
        defaults: enabled(&state["defaults"][key]),
    })
    .collect();
    window.set_rows(Rc::new(VecModel::from(rows)).into());
    window.set_defaults(
        if state["ready"] != true {
            "Saved defaults could not load."
        } else if state["stored"] == true {
            "Saved startup defaults"
        } else {
            "Factory defaults · no saved file"
        }
        .into(),
    );
    window.set_storage(
        format!(
            "Storage: {}",
            state["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    window.set_status(state["error"].as_str().or(state["storage_error"].as_str()).unwrap_or("Startup uses these defaults. Apply saved defaults changes the current session after active previews and viewport tools are finished.").into());
    window.set_ready(state["ready"] == true);
}

fn act(
    window: &PreviewDefaultsWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    action: &str,
) {
    let result = session.change_preview_preferences(Actor::User, action, &snapshot.borrow());
    match result {
        Ok(state) if state["write_committed"] == true && state["ready"] != true => window.set_message("Preview defaults were written but could not reload. Review the storage error before continuing.".into()),
        Ok(_) => window.set_message(
            match action {
                "save" => "Current preview settings saved for future startups.",
                "apply" => "Saved preview settings applied to this session.",
                "reset" => "Factory defaults saved. Apply saved defaults to use them now.",
                _ => "Preview defaults reloaded. Current preview settings are unchanged.",
            }
            .into(),
        ),
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, snapshot);
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = PreviewDefaultsWindow::new()?;
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
