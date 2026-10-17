use super::*;
use std::cell::Cell;

use crate::InstanceContentsWindow;

struct Controller {
    window: InstanceContentsWindow,
    shared: SharedSession,
    offset: Cell<usize>,
    token: Cell<Option<u64>>,
    timer: slint::Timer,
}

fn coordinates(value: &serde_json::Value) -> String {
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_f64()
                        .map(|value| format!("{value:.2}"))
                        .unwrap_or_else(|| "?".into())
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "unavailable".into())
}

impl Controller {
    fn refresh(&self) {
        let mut session = lock(&self.shared);
        let state = match session.inspect_instance_contents(self.offset.get(), 64) {
            Ok(state) => state,
            Err(error) => {
                self.window.set_message(error.into());
                return;
            }
        };
        self.token.set(state["token"].as_u64());
        self.window.set_summary(
            state["summary"]
                .as_str()
                .unwrap_or("Instance contents have not been refreshed")
                .into(),
        );
        self.window
            .set_loading(state["loading"].as_bool().unwrap_or(false));
        self.window
            .set_cancellable(state["cancellable"].as_bool().unwrap_or(false));
        self.window
            .set_can_clear(self.token.get().is_some() && state["loading"] != true);
        self.window.set_can_frame(
            state["available"] == true
                && state["suppression_reason"].is_null()
                && state["total"].as_u64().unwrap_or(0) > 0,
        );
        self.window.set_has_previous(self.offset.get() > 0);
        self.window.set_has_next(state["has_more"] == true);
        self.window.set_page_summary(
            format!(
                "Owners {}–{} of {}",
                self.offset
                    .get()
                    .saturating_add(1)
                    .min(state["total"].as_u64().unwrap_or(0) as usize),
                (self.offset.get() + 64).min(state["total"].as_u64().unwrap_or(0) as usize),
                state["total"].as_u64().unwrap_or(0)
            )
            .into(),
        );
        let source = &state["source"];
        self.window.set_source(
            format!(
                "Parent: {}\nSource revision: {} · preview {}",
                source["path"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| session.path().map(|path| path.display().to_string()))
                    .unwrap_or_else(|| "Unsaved parent".into()),
                source["revision"],
                state["token"]
            )
            .into(),
        );
        let mut owners = String::new();
        if let Some(rows) = state["owners"].as_array() {
            for row in rows {
                owners.push_str(&format!(
                    "{} · {}\nOrigin ({}) · Angles ({})\nWorld brushes: {} · Detail brushes: {}\n",
                    row["owner"].as_str().unwrap_or("Instance"),
                    row["file"].as_str().unwrap_or("unavailable"),
                    coordinates(&row["origin"]),
                    coordinates(&row["angles"]),
                    row["world_brushes"],
                    row["detail_brushes"]
                ));
                if let Some(bounds) = row["bounds"].as_array()
                    && bounds.len() == 2
                {
                    owners.push_str(&format!(
                        "Content bounds ({}) to ({})\n",
                        coordinates(&bounds[0]),
                        coordinates(&bounds[1])
                    ));
                }
                owners.push('\n');
            }
        }
        self.window.set_owners(if owners.is_empty() {
            "No resolved instance owners".into()
        } else {
            owners.into()
        });
        let mut diagnostics = String::new();
        if let Some(error) = state["error"].as_str() {
            diagnostics.push_str(error);
            diagnostics.push('\n');
        }
        if let Some(reason) = state["suppression_reason"].as_str() {
            diagnostics.push_str(reason);
            diagnostics.push('\n');
        }
        if let Some(rows) = state["diagnostics"].as_array() {
            for row in rows {
                if let Some(message) = row.as_str().or_else(|| row["message"].as_str()) {
                    diagnostics.push_str(message);
                    diagnostics.push('\n');
                }
            }
        }
        if let Some(rows) = state["stats"]["dependencies"].as_array() {
            for row in rows {
                diagnostics.push_str(&format!(
                    "{} · {} bytes\nSHA-256 {}\n",
                    row["path"].as_str().unwrap_or("Dependency"),
                    row["bytes"],
                    row["sha256"].as_str().unwrap_or("unavailable")
                ));
            }
        }
        self.window.set_diagnostics(diagnostics.into());
    }

    fn action(&self, action: &str) {
        self.window.set_message("".into());
        let Some(token) = self.token.get() else {
            self.window
                .set_message("Inspect the current instance contents first".into());
            return;
        };
        let mut session = lock(&self.shared);
        let result = match action {
            "cancel" => session.cancel_instance_contents_refresh(Actor::User, token),
            "clear" => session.clear_instance_contents(Actor::User, token),
            "frame" => {
                let ids = session.selected_ids();
                session.frame_instance_contents(Actor::User, token, &ids, self.window.get_aspect())
            }
            _ => unreachable!(),
        };
        if let Err(error) = result {
            self.window.set_message(error.into());
        }
        drop(session);
        self.refresh();
    }

    fn start(&self) {
        self.window.set_message("".into());
        let started = { lock(&self.shared).begin_instance_contents_refresh(Actor::User) };
        let request = match started {
            Ok(request) => request,
            Err(error) => {
                self.window.set_message(error.into());
                self.refresh();
                return;
            }
        };
        let token = request.token();
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("instance-contents".into())
            .spawn(move || {
                let result = request.run();
                let _ = lock(&shared).finish_instance_contents_refresh(Actor::User, result);
            });
        if let Err(error) = spawned {
            let _ = lock(&self.shared).fail_instance_contents_refresh(
                Actor::User,
                token,
                format!("Could not start instance refresh: {error}"),
            );
        }
        self.refresh();
    }

    fn new(shared: SharedSession) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: InstanceContentsWindow::new()?,
            shared,
            offset: Cell::new(0),
            token: Cell::new(None),
            timer: slint::Timer::default(),
        });
        controller.window.on_refresh_contents({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.start();
                }
            }
        });
        controller.window.on_cancel_refresh({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.action("cancel");
                }
            }
        });
        controller.window.on_clear_contents({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.action("clear");
                }
            }
        });
        controller.window.on_frame_contents({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.action("frame");
                }
            }
        });
        controller.window.on_previous({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller
                        .offset
                        .set(controller.offset.get().saturating_sub(64));
                    controller.refresh();
                }
            }
        });
        controller.window.on_next({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller
                        .offset
                        .set(controller.offset.get().saturating_add(64));
                    controller.refresh();
                }
            }
        });
        controller.window.on_close_dialog({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    let _ = controller.window.hide();
                }
            }
        });
        controller.timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            {
                let weak = Rc::downgrade(&controller);
                move || {
                    if let Some(controller) = weak.upgrade()
                        && controller.window.window().is_visible()
                    {
                        controller.refresh();
                    }
                }
            },
        );
        controller.refresh();
        Ok(controller)
    }
}

pub(super) fn bind(app: &AppWindow, shared: SharedSession) {
    let controller = match Controller::new(shared) {
        Ok(controller) => controller,
        Err(error) => {
            app.set_message(format!("Instance controls unavailable: {error}").into());
            return;
        }
    };
    let app = app.as_weak();
    app.unwrap().on_inspect_instance_contents({
        let app = app.clone();
        move || {
            if let Some(app) = app.upgrade() {
                controller
                    .window
                    .set_aspect(app.get_viewport_width() / app.get_viewport_height().max(1.0));
            }
            controller.refresh();
            let _ = controller.window.show();
        }
    });
}

#[cfg(test)]
mod tests;
