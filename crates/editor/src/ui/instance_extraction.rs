use super::*;
use crate::InstanceExtractionWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use session::instance_extraction::Pivot;
use source_formats::vmf::{format_number, format_vec3, parse_vec3};
use std::cell::Cell;

pub(super) struct Controller {
    window: InstanceExtractionWindow,
    target: DocumentContinuation,
    token: Cell<Option<u64>>,
    timer: slint::Timer,
}

impl Controller {
    #[cfg(test)]
    pub(super) fn window(&self) -> &InstanceExtractionWindow {
        &self.window
    }

    #[cfg(test)]
    pub(super) fn timer_running(&self) -> bool {
        self.timer.running()
    }

    pub(super) fn stop_timer(&self) {
        self.timer.stop();
    }

    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        super::hide_window("Instance extraction", self.window.window(), errors);
    }

    fn with_shared<R>(
        &self,
        report_error: bool,
        body: impl FnOnce(&SharedSession) -> R,
    ) -> Option<R> {
        match self
            .target
            .upgrade()
            .and_then(|access| access.with_shared(|shared| Ok(body(shared))))
        {
            Ok(result) => Some(result),
            Err(error) => {
                if report_error {
                    self.window.set_message(error.into());
                }
                None
            }
        }
    }

    #[cfg(test)]
    fn refresh(&self) {
        self.with_shared(true, |shared| self.refresh_admitted(shared));
    }

    fn tick(&self) {
        if self.window.window().is_visible() {
            self.with_shared(false, |shared| self.refresh_admitted(shared));
        }
    }

    fn refresh_admitted(&self, shared: &SharedSession) {
        let report = lock(shared).get_instance_extraction();
        self.token.set(report["token"].as_u64());
        let active = self.token.get().is_some();
        let owned = report["owner"].as_str() == Some(Actor::User.label());
        self.window.set_active(active);
        self.window.set_owned(owned);
        self.window.set_ready(report["ready"] == true);
        self.window.set_busy(matches!(
            report["status"].as_str(),
            Some("preparing" | "publishing")
        ));
        if active {
            self.window
                .set_file(report["file"].as_str().unwrap_or_default().into());
            let mode = match report["pivot_mode"].as_str() {
                Some("world_origin") => Some(0),
                Some("selection_centre") => Some(1),
                Some("custom") => Some(2),
                _ => None,
            };
            if let Some(mode) = mode {
                self.window.set_pivot_mode(mode);
            }
            let pivot = serde_json::from_value::<[f64; 3]>(report["pivot"].clone())
                .ok()
                .map(DVec3::from_array);
            if let Some(pivot) = pivot {
                self.window.set_pivot_x(format_number(pivot.x).into());
                self.window.set_pivot_y(format_number(pivot.y).into());
                self.window.set_pivot_z(format_number(pivot.z).into());
            }
            let pivot_label = match mode {
                Some(0) => "World origin",
                Some(1) => "Selection centre",
                Some(2) => "Custom",
                _ => "Unavailable",
            };
            self.window.set_summary(format!("Parent: {}\nOwner: {} · {}\nPivot: {} · {}\nSelection: {}\nWorld brushes: {} · Detail brushes: {} · Faces: {}\nChild bytes: {} · SHA-256: {}\n{}",
                report["source"].as_str().unwrap_or("Unavailable"),report["owner"].as_str().unwrap_or("Unknown"),report["status"].as_str().unwrap_or("Unknown"),pivot_label,pivot.map(format_vec3).unwrap_or_else(||"Unavailable".into()),report["selected"].as_array().map(|ids|ids.iter().filter_map(|id|id.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                report["world_brushes"],report["detail_brushes"],report["sides"],report["bytes"],report["sha256"].as_str().unwrap_or("Not prepared"),
                report["error"].as_str().or_else(||report["cleanup_error"].as_str()).unwrap_or(if report["ready"]==true {"Serialized child and compiler-expanded geometry match. Extract publishes the new file and replaces this selection."}else{"Preparing uses a private file; the final child does not exist yet."})).into());
        } else if let Some(error) = report["cleanup_error"].as_str() {
            self.window
                .set_summary(format!("Private staging cleanup requires attention: {error}").into());
        } else if report["last"]["published"] == true {
            self.window.set_summary(format!("Created {} · {} bytes\nReference {} selected. The parent is unsaved; Undo retains the child file.",report["last"]["file"].as_str().unwrap_or_default(),report["last"]["bytes"],report["last"]["id"].as_str().unwrap_or_default()).into());
        } else {
            self.window.set_summary("Select visible world brushes and whole ordinary func_detail owners. World brushes must follow retained world brushes; detail owners must follow retained brush entities. Existing visible references, groups, layers, attached notes, partitions and inbound face bindings are rejected. The child directory must already exist.".into());
        }
    }

    fn pivot(&self) -> Result<Pivot, String> {
        match self.window.get_pivot_mode() {
            0 => Ok(Pivot::WorldOrigin),
            1 => Ok(Pivot::SelectionCentre),
            2 => parse_vec3(&format!(
                "{} {} {}",
                self.window.get_pivot_x(),
                self.window.get_pivot_y(),
                self.window.get_pivot_z()
            ))
            .filter(|point| point.is_finite())
            .map(Pivot::Custom)
            .ok_or_else(|| "Custom pivot requires finite X, Y and Z coordinates".into()),
            _ => Err("Choose World origin, Selection centre or Custom pivot".into()),
        }
    }

    fn request(
        &self,
        session: &mut Session,
        publishing: bool,
    ) -> Result<session::instance_extraction::Request, String> {
        let pivot = self.pivot()?;
        if !publishing {
            return session.begin_instance_extraction_with_pivot(
                Actor::User,
                self.window.get_file().as_str(),
                pivot,
            );
        }
        let review = session.get_instance_extraction();
        let matches = match pivot {
            Pivot::WorldOrigin => review["pivot_mode"] == "world_origin",
            Pivot::SelectionCentre => review["pivot_mode"] == "selection_centre",
            Pivot::Custom(position) => {
                review["pivot_mode"] == "custom"
                    && serde_json::from_value::<[f64; 3]>(review["pivot"].clone())
                        .is_ok_and(|reviewed| DVec3::from_array(reviewed) == position)
            }
        };
        if !matches {
            return Err("Pivot changed; cancel and prepare the extraction again".into());
        }
        session.publish_instance_extraction(
            Actor::User,
            self.token.get().ok_or("Prepare the extraction first")?,
        )
    }

    fn start(&self, publishing: bool) {
        self.with_shared(true, |shared| {
            self.window.set_message("".into());
            let request = self.request(&mut lock(shared), publishing);
            match request {
                Ok(request) => {
                    let token = request.token();
                    let worker_session = shared.clone();
                    if let Err(error) = std::thread::Builder::new()
                        .name("instance-extraction".into())
                        .spawn(move || {
                            let result = request.run();
                            let _ = lock(&worker_session)
                                .finish_instance_extraction(Actor::User, result);
                        })
                    {
                        let _ = lock(shared).cancel_instance_extraction(Actor::User, token);
                        self.window.set_message(
                            format!("Could not start extraction worker: {error}").into(),
                        );
                    }
                }
                Err(error) => self.window.set_message(error.into()),
            }
            self.refresh_admitted(shared);
        });
    }

    fn cancel(&self) {
        let accepted = self.with_shared(true, |shared| {
            if self.window.get_owned()
                && let Some(token) = self.token.get()
                && let Err(error) = lock(shared).cancel_instance_extraction(Actor::User, token)
            {
                self.window.set_message(error.into());
                return false;
            }
            self.refresh_admitted(shared);
            true
        });
        if accepted != Some(false) {
            let _ = self.window.hide();
        }
    }

    #[cfg(test)]
    fn new(shared: SharedSession) -> Result<Rc<Self>, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared))
    }

    fn new_with_access(access: &DocumentAccess) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: InstanceExtractionWindow::new()?,
            target: access.downgrade(),
            token: Cell::new(None),
            timer: slint::Timer::default(),
        });
        controller.window.on_prepare({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.start(false);
                }
            }
        });
        controller.window.on_extract({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.start(true);
                }
            }
        });
        controller.window.on_review({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.with_shared(true, |shared| {
                        if let Some(token) = controller.token.get() {
                            match lock(shared).review_instance_extraction(Actor::User, token) {
                                Ok(_) => controller.window.set_message("".into()),
                                Err(error) => controller.window.set_message(error.into()),
                            }
                            controller.refresh_admitted(shared);
                        }
                    });
                }
            }
        });
        controller.window.on_cancel({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.cancel();
                }
            }
        });
        controller.window.window().on_close_requested({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade()
                    && controller.window.get_owned()
                {
                    controller.cancel();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        controller
            .timer
            .start(slint::TimerMode::Repeated, Duration::from_millis(250), {
                let weak = Rc::downgrade(&controller);
                move || {
                    if let Some(controller) = weak.upgrade() {
                        controller.tick();
                    }
                }
            });
        Ok(controller)
    }
}

pub(super) fn bind_with_access(app: &AppWindow, access: &DocumentAccess) -> Option<Rc<Controller>> {
    let controller = match Controller::new_with_access(access) {
        Ok(controller) => controller,
        Err(error) => {
            app.set_message(format!("Instance extraction unavailable: {error}").into());
            return None;
        }
    };
    bind_controller(app, controller.clone());
    Some(controller)
}

fn bind_controller(app: &AppWindow, controller: Rc<Controller>) {
    app.on_extract_instance({
        let app = app.as_weak();
        let controller = Rc::downgrade(&controller);
        move || {
            let (Some(app), Some(controller)) = (app.upgrade(), controller.upgrade()) else {
                return;
            };
            if controller
                .with_shared(true, |shared| {
                    controller.refresh_admitted(shared);
                    controller.window.set_message("".into());
                    let _ = controller.window.show();
                })
                .is_none()
            {
                app.set_message(controller.window.get_message());
            }
        }
    });
}

#[cfg(test)]
mod tests;
