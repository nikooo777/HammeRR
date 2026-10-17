use super::*;
use crate::InstanceExtractionWindow;
use std::cell::Cell;

struct Controller {
    window: InstanceExtractionWindow,
    shared: SharedSession,
    token: Cell<Option<u64>>,
    timer: slint::Timer,
}

impl Controller {
    fn refresh(&self) {
        let report = lock(&self.shared).get_instance_extraction();
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
            self.window.set_summary(format!("Parent: {}\nOwner: {} · {}\nSelection: {}\n{} world brushes · {} detail brushes · {} faces\nChild bytes: {} · SHA-256: {}\n{}",
                report["source"].as_str().unwrap_or("Unavailable"),report["owner"].as_str().unwrap_or("Unknown"),report["status"].as_str().unwrap_or("Unknown"),report["selected"].as_array().map(|ids|ids.iter().filter_map(|id|id.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
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

    fn start(&self, publishing: bool) {
        self.window.set_message("".into());
        let request = {
            let mut session = lock(&self.shared);
            if publishing {
                self.token
                    .get()
                    .ok_or_else(|| "Prepare the extraction first".to_owned())
                    .and_then(|token| session.publish_instance_extraction(Actor::User, token))
            } else {
                session.begin_instance_extraction(Actor::User, self.window.get_file().as_str())
            }
        };
        match request {
            Ok(request) => {
                let token = request.token();
                let shared = self.shared.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("instance-extraction".into())
                    .spawn(move || {
                        let result = request.run();
                        let _ = lock(&shared).finish_instance_extraction(Actor::User, result);
                    })
                {
                    let _ = lock(&self.shared).cancel_instance_extraction(Actor::User, token);
                    self.window
                        .set_message(format!("Could not start extraction worker: {error}").into());
                }
            }
            Err(error) => self.window.set_message(error.into()),
        }
        self.refresh();
    }

    fn cancel(&self) {
        if self.window.get_owned()
            && let Some(token) = self.token.get()
            && let Err(error) = lock(&self.shared).cancel_instance_extraction(Actor::User, token)
        {
            self.window.set_message(error.into());
            return;
        }
        self.refresh();
        let _ = self.window.hide();
    }

    fn new(shared: SharedSession) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: InstanceExtractionWindow::new()?,
            shared,
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
                if let Some(controller) = weak.upgrade()
                    && let Some(token) = controller.token.get()
                {
                    match lock(&controller.shared).review_instance_extraction(Actor::User, token) {
                        Ok(_) => controller.window.set_message("".into()),
                        Err(error) => controller.window.set_message(error.into()),
                    }
                    controller.refresh();
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
    match Controller::new(shared) {
        Ok(controller) => app.on_extract_instance({
            move || {
                controller.refresh();
                controller.window.set_message("".into());
                let _ = controller.window.show();
            }
        }),
        Err(error) => app.set_message(format!("Instance extraction unavailable: {error}").into()),
    }
}

#[cfg(test)]
mod tests;
