use super::*;
use std::cell::Cell;

use crate::InstanceReferenceWindow;
use session::instance_reference::Changes;

struct Controller {
    window: InstanceReferenceWindow,
    shared: SharedSession,
    token: Cell<Option<u64>>,
    generation: Cell<u64>,
    edits: Cell<u64>,
    picker: Cell<u64>,
    editing: Cell<bool>,
    values: RefCell<[String; 3]>,
    timer: slint::Timer,
}

impl Controller {
    fn present(&self, report: &serde_json::Value) {
        self.token.set(report["token"].as_u64());
        let owned = report["owner"].as_str() == Some(Actor::User.label());
        self.window.set_owned(owned);
        self.window
            .set_shared_form(self.token.get().is_some() && !owned);
        self.window.set_edit_mode(report["mode"] == "edit");
        self.window.set_ready(report["valid"] == true);
        self.window.set_source(
            format!(
                "Parent: {}\nRelative to: {}",
                report["source"]
                    .as_str()
                    .unwrap_or("Unsaved parent — save it first"),
                report["directory"].as_str().unwrap_or("Unavailable")
            )
            .into(),
        );
        let next: [String; 3] = std::array::from_fn(|index| {
            report[["file", "origin", "angles"][index]]
                .as_str()
                .unwrap_or("")
                .into()
        });
        let previous = self.values.borrow();
        if next[0] != previous[0] || self.window.get_file().as_str() != next[0] {
            self.window.set_file(next[0].clone().into());
        }
        if next[1] != previous[1] && !self.editing.get() {
            let values = next[1].split_whitespace().collect::<Vec<_>>();
            self.window
                .set_origin_x(if values.len() == 3 { values[0] } else { "" }.into());
            self.window
                .set_origin_y(if values.len() == 3 { values[1] } else { "" }.into());
            self.window
                .set_origin_z(if values.len() == 3 { values[2] } else { "" }.into());
        }
        if next[2] != previous[2] && !self.editing.get() {
            let values = next[2].split_whitespace().collect::<Vec<_>>();
            self.window
                .set_pitch(if values.len() == 3 { values[0] } else { "" }.into());
            self.window
                .set_yaw(if values.len() == 3 { values[1] } else { "" }.into());
            self.window
                .set_roll(if values.len() == 3 { values[2] } else { "" }.into());
        }
        drop(previous);
        self.values.replace(next);
        self.window
            .set_raw_origin(if report["origin_components"].is_null() {
                report["origin"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("(blank)")
                    .into()
            } else {
                "".into()
            });
        self.window
            .set_raw_angles(if report["angle_components"].is_null() {
                report["angles"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("(blank)")
                    .into()
            } else {
                "".into()
            });
        let status = report["error"]
            .as_str()
            .or_else(|| report["presence_error"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if report["presence_checked"] == true {
                    format!(
                        "File present; contents unchecked · {} bytes",
                        report["bytes"]
                    )
                } else {
                    "File presence unchecked; Create/Apply checks it before editing the map".into()
                }
            });
        self.window.set_status(
            if owned {
                status
            } else {
                format!(
                    "Shared form owned by {} · {status}",
                    report["owner"].as_str().unwrap_or("another client")
                )
            }
            .into(),
        );
    }

    fn refresh(&self) {
        let mut session = lock(&self.shared);
        let report = session.get_instance_reference();
        self.present(&report);
        self.window.set_contents(
            session
                .inspect_instance_contents(0, 1)
                .ok()
                .and_then(|report| report["summary"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "Instance contents unavailable".into())
                .into(),
        );
    }

    fn result(&self, result: Result<serde_json::Value, String>) {
        match result {
            Ok(report) => {
                self.window.set_message("".into());
                self.present(&report);
            }
            Err(error) => self.window.set_message(error.into()),
        }
    }

    fn open(&self, entity: Option<u32>) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.edits.set(self.edits.get().wrapping_add(1));
        self.reset_fields();
        let result = lock(&self.shared).begin_instance_reference(Actor::User, entity);
        self.result(result);
        self.refresh();
        let _ = self.window.show();
    }

    fn reset_fields(&self) {
        self.values.replace(std::array::from_fn(|_| String::new()));
        self.window.set_origin_x("".into());
        self.window.set_origin_y("".into());
        self.window.set_origin_z("".into());
        self.window.set_pitch("".into());
        self.window.set_yaw("".into());
        self.window.set_roll("".into());
    }

    fn edited(&self, field: i32) {
        self.edits.set(self.edits.get().wrapping_add(1));
        let Some(token) = self.token.get() else {
            return;
        };
        let mut changes = Changes::default();
        match field {
            0 => changes.file = Some(self.window.get_file().into()),
            1 => {
                changes.origin = Some(format!(
                    "{} {} {}",
                    self.window.get_origin_x(),
                    self.window.get_origin_y(),
                    self.window.get_origin_z()
                ))
            }
            2 => {
                changes.angles = Some(format!(
                    "{} {} {}",
                    self.window.get_pitch(),
                    self.window.get_yaw(),
                    self.window.get_roll()
                ))
            }
            _ => return,
        }
        let result = lock(&self.shared).update_instance_reference(Actor::User, token, changes);
        match result {
            Ok(report) => {
                self.editing.set(true);
                self.result(Ok(report));
                self.editing.set(false);
            }
            Err(error) => {
                self.reset_fields();
                let report = lock(&self.shared).get_instance_reference();
                self.present(&report);
                self.window.set_message(
                    format!("{error}; rejected inputs were restored to the shared form").into(),
                );
            }
        }
    }

    fn complete_file(&self, token: u64, generation: u64, edits: u64, picker: u64, path: &Path) {
        if self.token.get() != Some(token)
            || self.generation.get() != generation
            || self.edits.get() != edits
            || self.picker.get() != picker
            || !self.window.window().is_visible()
        {
            self.window
                .set_message("File selection expired; Browse again to use a new selection".into());
            return;
        }
        let result = lock(&self.shared).instance_reference_file(Actor::User, token, path);
        self.edits.set(self.edits.get().wrapping_add(1));
        self.result(result);
    }

    fn browse(self: &Rc<Self>) {
        let Some(token) = self.token.get() else {
            return;
        };
        let generation = self.generation.get();
        let edits = self.edits.get();
        self.picker.set(self.picker.get().wrapping_add(1));
        let picker = self.picker.get();
        let directory = lock(&self.shared)
            .path()
            .and_then(|path| path.parent())
            .map(Path::to_owned);
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Choose instance VMF")
            .add_filter("Valve Map File", &["vmf"]);
        if let Some(directory) = directory {
            dialog = dialog.set_directory(directory);
        }
        let weak = Rc::downgrade(self);
        slint::spawn_local(async move {
            let Some(file) = dialog.pick_file().await else {
                return;
            };
            if let Some(controller) = weak.upgrade() {
                controller.complete_file(token, generation, edits, picker, file.path());
            }
        })
        .expect("event loop is running");
    }

    fn refresh_contents(&self) {
        let request = lock(&self.shared).begin_instance_contents_refresh(Actor::User);
        match request {
            Ok(request) => {
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
                self.window.set_message(
                    "Refreshing committed references; form edits remain unapplied".into(),
                );
            }
            Err(error) => self.window.set_message(error.into()),
        }
        self.refresh();
    }

    fn new(shared: SharedSession) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: InstanceReferenceWindow::new()?,
            shared,
            token: Cell::new(None),
            generation: Cell::new(0),
            edits: Cell::new(0),
            picker: Cell::new(0),
            editing: Cell::new(false),
            values: RefCell::new(std::array::from_fn(|_| String::new())),
            timer: slint::Timer::default(),
        });
        controller.window.on_edited({
            let weak = Rc::downgrade(&controller);
            move |field| {
                if let Some(controller) = weak.upgrade() {
                    controller.edited(field);
                }
            }
        });
        controller.window.on_browse({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.browse();
                }
            }
        });
        controller.window.on_check_file({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade()
                    && let Some(token) = controller.token.get()
                {
                    let result =
                        lock(&controller.shared).check_instance_reference(Actor::User, token);
                    controller.result(result);
                }
            }
        });
        controller.window.on_review({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade()
                    && let Some(token) = controller.token.get()
                {
                    let result =
                        lock(&controller.shared).review_instance_reference(Actor::User, token);
                    controller
                        .generation
                        .set(controller.generation.get().wrapping_add(1));
                    controller.result(result);
                }
            }
        });
        controller.window.on_refresh_contents({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.refresh_contents();
                }
            }
        });
        controller.window.on_apply({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade()
                    && let Some(token) = controller.token.get()
                {
                    let result =
                        lock(&controller.shared).apply_instance_reference(Actor::User, token);
                    match result {
                        Ok(_) => {
                            controller
                                .generation
                                .set(controller.generation.get().wrapping_add(1));
                            controller.token.set(None);
                            let _ = controller.window.hide();
                        }
                        Err(error) => controller.window.set_message(error.into()),
                    }
                }
            }
        });
        controller.window.on_cancel({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    if controller.window.get_owned()
                        && let Some(token) = controller.token.get()
                        && let Err(error) =
                            lock(&controller.shared).cancel_instance_reference(Actor::User, token)
                    {
                        controller.window.set_message(error.into());
                        return;
                    }
                    controller
                        .generation
                        .set(controller.generation.get().wrapping_add(1));
                    controller.token.set(None);
                    let _ = controller.window.hide();
                }
            }
        });
        controller.window.window().on_close_requested({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.window.invoke_cancel();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        controller
            .timer
            .start(slint::TimerMode::Repeated, Duration::from_millis(250), {
                let weak = Rc::downgrade(&controller);
                move || {
                    if let Some(controller) = weak.upgrade()
                        && controller.window.window().is_visible()
                    {
                        controller.refresh();
                    }
                }
            });
        Ok(controller)
    }
}

pub(super) fn bind(app: &AppWindow, shared: SharedSession) {
    let controller = match Controller::new(shared.clone()) {
        Ok(controller) => controller,
        Err(error) => {
            app.set_message(format!("Instance reference controls unavailable: {error}").into());
            return;
        }
    };
    app.on_create_instance_reference({
        let controller = controller.clone();
        move || controller.open(None)
    });
    app.on_edit_instance_reference({
        let controller = controller.clone();
        move || {
            let id = single_selected_entity(&lock(&shared));
            if let Some(id) = id {
                controller.open(Some(id));
            }
        }
    });
}

#[cfg(test)]
mod tests;
