use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    AppWindow, PivotDefaultsWindow,
    runtime::DocumentAccess,
    session::{Actor, Session, transform_pivot::AttachmentInfo},
};

pub fn refresh_status(app: &AppWindow, session: &Session) {
    app.set_pivot_defaults_error(session.pivot_preferences_error().is_some());
}

pub struct Controller {
    pub window: PivotDefaultsWindow,
    snapshot: Rc<RefCell<String>>,
}

fn pivot(value: &Value) -> String {
    match value["mode"].as_str() {
        Some("selection_center") => "Selection center".into(),
        Some("world_origin") => "World origin".into(),
        Some("custom") => format!("Custom world point: {}", value["point"]),
        _ => "Unavailable".into(),
    }
}

pub(crate) fn attachment_summary(attachment: &AttachmentInfo) -> String {
    match attachment.kind {
        "origin" => format!("{} · Authored origin", attachment.object),
        "local" => format!("{} · Rigid local point", attachment.object),
        "vertex" => {
            let Some(reference) = &attachment.source else {
                return format!("{} · Vertex source unavailable", attachment.object);
            };
            let source = reference.report();
            match source["kind"].as_str() {
                Some("brush_corner") => {
                    let sides = source["sides"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "{} · Brush corner · brush {} · faces {sides}",
                        attachment.object, source["solid"]
                    )
                }
                Some("terrain_vertex") => format!(
                    "{} · Terrain vertex · brush {} · face {} · row {}, column {}",
                    attachment.object,
                    source["solid"],
                    source["side"],
                    source["row"],
                    source["column"]
                ),
                Some("model_vertex") => format!(
                    "{} · Model vertex · part {} · triangle {} · vertex {}\n{} · body {} · skin {}",
                    attachment.object,
                    source["part"],
                    source["triangle"],
                    source["vertex"],
                    source["model"]
                        .as_str()
                        .unwrap_or("Model source unavailable"),
                    source["body"],
                    source["skin"]
                ),
                _ => format!("{} · Vertex source unavailable", attachment.object),
            }
        }
        _ => format!("{} · Unavailable attachment", attachment.object),
    }
}

fn refresh(window: &PivotDefaultsWindow, session: &Session, snapshot: &RefCell<String>) {
    let state = session.pivot_preferences_state();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    let attachment = session.transform_pivot_attachment_state();
    window.set_current(if let Some(binding) = &attachment.attachment {
        let mut current = format!("Current document: {}", attachment_summary(binding));
        if let Some(point) = attachment.effective_pivot {
            current.push_str(&format!(
                "\nResolved · X {:.3} · Y {:.3} · Z {:.3}",
                point[0], point[1], point[2]
            ));
        }
        if let Some(error) = state["current_error"]
            .as_str()
            .or(attachment.error.as_deref())
        {
            current.push_str(&format!("\nUnavailable: {error}"));
            if binding.kind == "vertex" {
                current
                    .push_str("\nUse Attach vertex… in the Inspector to choose the source again.");
            }
        }
        current.into()
    } else {
        format!("Current document: {}", pivot(&state["current"])).into()
    });
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
            pivot(&state["defaults"])
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
    window.set_status(state["error"].as_str().or(state["storage_error"].as_str()).unwrap_or("New document windows use these defaults. Apply changes only this document’s pivot.").into());
    window.set_ready(state["ready"] == true);
    window.set_can_save(state["ready"] == true && state["save_error"].is_null());
    window.set_save_status(state["save_error"].as_str().unwrap_or("").into());
    window.set_can_apply(state["ready"] == true && state["apply_error"].is_null());
    window.set_apply_status(state["apply_error"].as_str().unwrap_or("").into());
}

fn act(
    window: &PivotDefaultsWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    action: &str,
) {
    let result = session.change_pivot_preferences(Actor::User, action, &snapshot.borrow());
    match result {
        Ok(state) if state["write_committed"]==true && state["ready"]!=true=>window.set_message("Transform pivot defaults were written but could not reload. Review the storage error before continuing.".into()),
        Ok(_) => window.set_message(
            match action {
                "save" => "Current pivot saved for future document startups.",
                "apply" => "Saved pivot applied to this document.",
                "reset" => "Factory defaults saved. Apply saved defaults to use them now.",
                _ => "Transform pivot defaults reloaded. Current pivot is unchanged.",
            }
            .into(),
        ),
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, snapshot);
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = PivotDefaultsWindow::new()?;
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
