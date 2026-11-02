use std::{cell::RefCell, rc::Rc};

use editor_core::workspace_preferences::Layout;
use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::{
    AppWindow, WorkspaceDefaultRow, WorkspaceDefaultsWindow,
    runtime::DocumentAccess,
    session::{Actor, Session},
};

#[derive(Default)]
struct State {
    snapshot: String,
    layout_snapshot: String,
    loaded: Option<Layout>,
    preserve_fields: bool,
}

pub struct Controller {
    pub window: WorkspaceDefaultsWindow,
    state: Rc<RefCell<State>>,
}

pub fn adopt_layout(app: &AppWindow, layout: Layout) {
    app.set_workspace_dock_visible(layout.dock_visible);
    app.set_workspace_dock_width(i32::from(layout.dock_width));
    app.set_workspace_agent_visible(layout.agent_visible);
    app.set_workspace_agent_height(i32::from(layout.agent_height));
    app.set_workspace_tool_rail_visible(layout.tool_rail_visible);
}

fn display_value(value: &Value) -> slint::SharedString {
    match value {
        Value::Bool(true) => "On".into(),
        Value::Bool(false) => "Off".into(),
        Value::Number(value) => format!("{value} px").into(),
        _ => "Unavailable".into(),
    }
}

fn refresh(
    window: &WorkspaceDefaultsWindow,
    app: &AppWindow,
    session: &Session,
    state: &RefCell<State>,
    reset_fields: bool,
) {
    let report = session.workspace_preferences_state();
    let layout_snapshot = report["layout_snapshot"].as_str().unwrap().to_owned();
    let pristine = state
        .borrow()
        .loaded
        .is_none_or(|loaded| layout(window).is_ok_and(|value| value == loaded));
    if reset_fields
        || (pristine
            && !window.get_editing_size()
            && !state.borrow().preserve_fields
            && state.borrow().layout_snapshot != layout_snapshot)
    {
        let layout = session.workspace_layout();
        window.set_dock_visible(layout.dock_visible);
        window.set_dock_width(i32::from(layout.dock_width));
        window.set_agent_visible(layout.agent_visible);
        window.set_agent_height(i32::from(layout.agent_height));
        window.set_tool_rail_visible(layout.tool_rail_visible);
        let mut state = state.borrow_mut();
        state.loaded = Some(layout);
        state.layout_snapshot = layout_snapshot.clone();
        state.preserve_fields = false;
    }
    {
        let mut state = state.borrow_mut();
        state.snapshot = report["snapshot"].as_str().unwrap().into();
    }
    let rows = [
        ("dock_visible", "Dock"),
        ("dock_width", "Preferred dock width"),
        ("agent_visible", "Agent feed"),
        ("agent_height", "Preferred Agent height"),
        ("tool_rail_visible", "Tool rail"),
    ]
    .into_iter()
    .map(|(key, label)| WorkspaceDefaultRow {
        label: label.into(),
        current: display_value(&report["current"][key]),
        defaults: display_value(&report["defaults"][key]),
    })
    .collect::<Vec<_>>();
    window.set_rows(Rc::new(VecModel::from(rows)).into());
    window.set_fit(
        format!(
            "Rendered fit · Dock {:.0} px wide · Agent feed {:.0} px high",
            app.get_workspace_dock_actual_width(),
            app.get_workspace_agent_actual_height(),
        )
        .into(),
    );
    window.set_defaults(
        if report["ready"] != true {
            "Saved defaults could not load. Live layout controls remain available."
        } else if report["stored"] == true {
            "Saved startup defaults"
        } else {
            "Factory defaults · no saved file"
        }
        .into(),
    );
    window.set_storage(
        format!(
            "Storage: {}",
            report["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    let mut status = report["error"].as_str().or(report["storage_error"].as_str())
        .unwrap_or("Only Save current as defaults and Reset saved defaults write preferences. Reload defaults keeps the current layout.").to_owned();
    if state.borrow().layout_snapshot != layout_snapshot {
        status.push_str(
            " Applied layout changed. Reload current layout to replace the staged fields.",
        );
    }
    window.set_status(status.into());
    window.set_ready(report["ready"] == true);
}

fn layout(window: &WorkspaceDefaultsWindow) -> Result<Layout, String> {
    Ok(Layout {
        dock_visible: window.get_dock_visible(),
        dock_width: u16::try_from(window.get_dock_width())
            .map_err(|_| "Preferred dock width must be between 240 and 640 logical pixels")?,
        agent_visible: window.get_agent_visible(),
        agent_height: u16::try_from(window.get_agent_height())
            .map_err(|_| "Preferred Agent height must be between 96 and 480 logical pixels")?,
        tool_rail_visible: window.get_tool_rail_visible(),
    })
}

fn act(
    window: &WorkspaceDefaultsWindow,
    app: &AppWindow,
    session: &mut Session,
    state: &RefCell<State>,
    action: &str,
) {
    let result = if action == "layout" {
        layout(window).and_then(|layout| {
            session.set_workspace_layout(Actor::User, layout, &state.borrow().layout_snapshot)
        })
    } else {
        session.change_workspace_preferences(Actor::User, action, &state.borrow().snapshot)
    };
    let reset_fields = result.is_ok() && matches!(action, "layout" | "apply");
    match result {
        Ok(report) if report["write_committed"] == true && report["ready"] != true => {
            window.set_message("Workspace defaults were written but could not reload. Review the storage error before continuing.".into());
        }
        Ok(_) => window.set_message(
            match action {
                "layout" => "Layout applied. Saved defaults are unchanged.",
                "save" => "Applied layout saved for future document windows.",
                "apply" => "Saved workspace layout applied to this window.",
                "reset" => "Factory defaults saved. Apply saved defaults to use them now.",
                _ => "Workspace defaults reloaded. The current layout is unchanged.",
            }
            .into(),
        ),
        Err(error) => {
            state.borrow_mut().preserve_fields = true;
            window.set_message(error.into());
        }
    }
    refresh(window, app, session, state, reset_fields);
}

impl Controller {
    pub fn new_with_access(
        app: &AppWindow,
        access: &DocumentAccess,
    ) -> Result<Self, slint::PlatformError> {
        let window = WorkspaceDefaultsWindow::new()?;
        let state = Rc::new(RefCell::new(State::default()));
        let callback = |action: &'static str| {
            let (window, app, target, state) = (
                window.as_weak(),
                app.as_weak(),
                access.downgrade(),
                state.clone(),
            );
            move || {
                let (Some(window), Some(app)) = (window.upgrade(), app.upgrade()) else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        if action == "reload_layout" {
                            refresh(&window, &app, session, &state, true);
                            window.set_message("Current layout loaded into the fields.".into());
                        } else {
                            act(&window, &app, session, &state, action);
                        }
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        };
        window.on_apply_layout(callback("layout"));
        window.on_reload_layout(callback("reload_layout"));
        window.on_save(callback("save"));
        window.on_apply(callback("apply"));
        window.on_reload(callback("reload"));
        window.on_reset(callback("reset"));
        window.on_cancel({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        Ok(Self { window, state })
    }

    pub fn refresh(&self, app: &AppWindow, session: &Session) {
        refresh(&self.window, app, session, &self.state, false);
    }

    pub fn show(&self, app: &AppWindow, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        refresh(&self.window, app, session, &self.state, true);
        self.window.show()
    }
}
