use super::*;
use std::cell::Cell;

use editor_core::session::external_comparison::{Action, Filter, Query};
use map_core::comparison::Status;

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::{ComparisonRow, ExternalComparisonWindow};

pub(super) struct Controller {
    window: ExternalComparisonWindow,
    target: DocumentContinuation,
    timer: slint::Timer,
    token: Cell<Option<u64>>,
    offset: Cell<usize>,
    aspect: Cell<f32>,
    picker: Cell<u64>,
    retired: Cell<bool>,
    dirty: Cell<bool>,
    file_pending: Cell<bool>,
    cancellation: RefCell<Option<compiler::Control>>,
    local_message: RefCell<Option<String>>,
}

impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(control) = self.cancellation.get_mut().take() {
            let _ = control.cancel();
        }
    }
}

impl Controller {
    fn set_local_message(&self, message: String) {
        self.window.set_message(message.clone().into());
        *self.local_message.borrow_mut() = Some(message);
    }
    fn clear_local_message(&self) {
        self.local_message.borrow_mut().take();
        self.window.set_message("".into());
    }
    fn access<R>(&self, body: impl FnOnce(&SharedSession) -> R) -> Option<R> {
        if self.retired.get() {
            return None;
        }
        match self
            .target
            .upgrade()
            .and_then(|access| access.with_shared(|shared| Ok(body(shared))))
        {
            Ok(result) => Some(result),
            Err(error) => {
                self.set_local_message(error);
                self.clear_rows();
                None
            }
        }
    }
    fn clear_rows(&self) {
        self.window.set_rows(slint::ModelRc::default());
        self.window.set_can_act(false);
        self.window.set_previous(false);
        self.window.set_next(false);
        self.window
            .set_page_summary("No verified page displayed".into());
    }
    fn refresh(&self) {
        self.access(|shared| self.present(&mut lock(shared)));
    }
    fn present(&self, session: &mut Session) {
        let state = session.external_comparison_state();
        let token = state["token"].as_u64();
        if token != self.token.get() {
            self.dirty.set(false);
            self.offset.set(0);
        }
        self.token.set(token);
        let owned = state["owner"].as_str() == Some(Actor::User.label());
        let busy = state["busy"] == true;
        self.window.set_active(token.is_some());
        self.window.set_owned(owned);
        self.window.set_busy(busy);
        if !self.file_pending.get()
            && let Some(file) = state["file"].as_str()
        {
            self.window.set_file(file.into());
        }
        self.window.set_summary(
            format!(
                "{} · Owner: {}",
                state["status"].as_str().unwrap_or("Closed"),
                state["owner"].as_str().unwrap_or("None")
            )
            .into(),
        );
        if let Some(message) = self.local_message.borrow().as_deref() {
            self.window.set_message(message.into());
        } else if let Some(error) = state["error"].as_str() {
            self.window.set_message(error.into());
        } else {
            self.window.set_message("".into());
        }
        let page = &state["page"];
        if busy || page.is_null() || self.dirty.get() || self.file_pending.get() {
            self.clear_rows();
            return;
        }
        self.window.set_status(
            match page["status_filter"].as_str() {
                Some("identical") => "Identical",
                Some("changed") => "Changed",
                Some("current_only") => "Current only",
                Some("external_only") => "External only",
                Some("ambiguous") => "Ambiguous",
                _ => "All statuses",
            }
            .into(),
        );
        self.window
            .set_search(page["search"].as_str().unwrap_or_default().into());
        let offset = page["offset"].as_u64().unwrap_or(0) as usize;
        self.offset.set(offset);
        self.window
            .set_rows(slint::ModelRc::new(slint::VecModel::from(
                page["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|row| {
                        let status = match row["status"].as_str().unwrap_or("") {
                            "current_only" => "Current only",
                            "external_only" => "External only",
                            "identical" => "Identical",
                            "changed" => "Changed",
                            _ => "Ambiguous",
                        };
                        ComparisonRow {
                        title: format!(
                            "{} · {}",
                            row["label"].as_str().unwrap_or("Object"),
                            status
                        )
                        .into(),
                        detail: format!(
                            "{} · Current occurrences: {} · External occurrences: {}\nExternal: {}",
                            row["id"].as_str().unwrap_or(""),
                            row["current_count"],
                            row["external_count"],
                            row["external_label"].as_str().unwrap_or("Absent")
                        )
                        .into(),
                    }
                    })
                    .collect::<Vec<_>>(),
            )));
        let shown = page["rows"].as_array().map_or(0, Vec::len);
        self.window.set_page_summary(
            format!(
                "Rows {}–{} of {} filtered · {} total · {} current matches across all pages",
                if shown == 0 { 0 } else { offset + 1 },
                offset + shown,
                page["filtered"],
                page["total"],
                page["current_matches"]
            )
            .into(),
        );
        self.window.set_previous(offset > 0);
        self.window.set_next(page["next_offset"].is_u64());
        self.window
            .set_can_act(owned && page["current_matches"].as_u64().unwrap_or(0) > 0);
    }
    fn filter(&self) -> Filter {
        let status = match self.window.get_status().as_str() {
            "Identical" => Some(Status::Identical),
            "Changed" => Some(Status::Changed),
            "Current only" => Some(Status::CurrentOnly),
            "External only" => Some(Status::ExternalOnly),
            "Ambiguous" => Some(Status::Ambiguous),
            _ => None,
        };
        Filter {
            status,
            search: self.window.get_search().to_string(),
        }
    }
    fn launch(&self, compare: bool, action: Action, offset: usize) {
        if self.retired.get() {
            return;
        }
        self.clear_local_message();
        if !compare && self.file_pending.get() {
            self.set_local_message(
                "Choose Compare to capture the newly chosen external VMF.".into(),
            );
            return;
        }
        self.access(|shared| {
            let request = {
                let mut session = lock(shared);
                if compare {
                    session.begin_external_comparison(
                        Actor::User,
                        Path::new(self.window.get_file().as_str()),
                    )
                } else {
                    self.token
                        .get()
                        .ok_or_else(|| "Compare with an external VMF first".to_owned())
                        .and_then(|token| {
                            session.query_external_comparison(
                                Actor::User,
                                token,
                                Query {
                                    filter: self.filter(),
                                    offset,
                                    limit: 64,
                                    action,
                                },
                            )
                        })
                }
            };
            match request {
                Ok(request) => {
                    *self.cancellation.borrow_mut() = Some(request.cancellation());
                    if compare {
                        self.file_pending.set(false);
                        self.window.set_status("All statuses".into());
                        self.window.set_search("".into());
                    }
                    self.dirty.set(false);
                    let token = request.token();
                    let worker_session = shared.clone();
                    if let Err(error) = std::thread::Builder::new()
                        .name("external-map-comparison".into())
                        .spawn(move || {
                            let completed = request.run();
                            let _ = lock(&worker_session)
                                .finish_external_comparison(Actor::User, completed);
                        })
                    {
                        let _ = lock(shared).cancel_external_comparison(Actor::User, token);
                        self.set_local_message(format!(
                            "Could not start comparison worker: {error}"
                        ));
                    }
                }
                Err(error) => self.set_local_message(error),
            }
            self.present(&mut lock(shared));
        });
    }
    fn cancel(&self, hide: bool) {
        if self.retired.get() {
            return;
        }
        self.clear_local_message();
        self.picker.set(self.picker.get().wrapping_add(1));
        self.access(|shared| {
            if self.window.get_owned()
                && let Some(token) = self.token.get()
                && let Err(error) = lock(shared).cancel_external_comparison(Actor::User, token)
            {
                self.set_local_message(error);
            }
            self.present(&mut lock(shared));
        });
        self.clear_rows();
        if hide {
            let _ = self.window.hide();
        }
    }
    fn choose(self: &Rc<Self>) {
        self.clear_local_message();
        self.access(|shared| {
            let generation = lock(shared).document_generation();
            let token = self.token.get();
            self.picker.set(self.picker.get().wrapping_add(1));
            let picker = self.picker.get();
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title("Choose comparison VMF")
                .add_filter("Valve Map File", &["vmf"]);
            if let Some(directory) = lock(shared)
                .path()
                .and_then(|path| path.parent())
                .map(Path::to_owned)
            {
                dialog = dialog.set_directory(directory);
            }
            let weak = Rc::downgrade(self);
            slint::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    return;
                };
                if let Some(controller) = weak.upgrade() {
                    controller.access(|shared| {
                        if controller.picker.get() != picker
                            || controller.token.get() != token
                            || lock(shared).document_generation() != generation
                        {
                            return;
                        }
                        match file.path().to_str() {
                            Some(path) => {
                                controller.window.set_file(path.into());
                                controller.dirty.set(true);
                                controller.file_pending.set(true);
                                controller.clear_rows();
                                controller.set_local_message(
                                    "Choose Compare to capture this external file.".into(),
                                );
                            }
                            None => controller
                                .set_local_message("Choose a VMF through a UTF-8 path.".into()),
                        }
                    });
                }
            })
            .expect("event loop is running");
        });
    }
    pub(super) fn new(access: &DocumentAccess) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: ExternalComparisonWindow::new()?,
            target: access.downgrade(),
            timer: slint::Timer::default(),
            token: Cell::new(None),
            offset: Cell::new(0),
            aspect: Cell::new(1.0),
            picker: Cell::new(0),
            retired: Cell::new(false),
            dirty: Cell::new(false),
            file_pending: Cell::new(false),
            cancellation: RefCell::new(None),
            local_message: RefCell::new(None),
        });
        macro_rules! bind {
            ($callback:ident,$body:expr) => {{
                let weak = Rc::downgrade(&controller);
                controller.window.$callback(move || {
                    if let Some(controller) = weak.upgrade()
                        && !controller.retired.get()
                    {
                        ($body)(&controller);
                    }
                });
            }};
        }
        bind!(on_choose_file, |controller: &Rc<Self>| controller.choose());
        bind!(on_compare, |controller: &Rc<Self>| controller.launch(
            true,
            Action::Page,
            0
        ));
        bind!(on_refresh, |controller: &Rc<Self>| controller.launch(
            false,
            Action::Page,
            0
        ));
        bind!(on_filter_changed, |controller: &Rc<Self>| {
            controller.dirty.set(true);
            controller.offset.set(0);
            controller.clear_rows();
            controller.set_local_message(
                "Filters changed. Refresh page to inspect all matching rows.".into(),
            );
        });
        bind!(on_select_current, |controller: &Rc<Self>| {
            let mode = match controller.window.get_mode().as_str() {
                "Add" => SelectMode::Add,
                "Remove" => SelectMode::Remove,
                "Toggle" => SelectMode::Toggle,
                _ => SelectMode::Replace,
            };
            controller.launch(false, Action::Select(mode), 0);
        });
        bind!(on_frame_current, |controller: &Rc<Self>| controller.launch(
            false,
            Action::Frame(controller.aspect.get()),
            0
        ));
        bind!(on_review, |controller: &Rc<Self>| {
            controller.clear_local_message();
            let accepted = controller.access(|shared| {
                let result = controller
                    .token
                    .get()
                    .ok_or_else(|| "No shared comparison is open".to_owned())
                    .and_then(|token| lock(shared).review_external_comparison(Actor::User, token));
                let accepted = result.is_ok();
                if let Err(error) = result {
                    controller.set_local_message(error);
                }
                controller.present(&mut lock(shared));
                accepted
            });
            if accepted == Some(true) {
                controller.launch(false, Action::Page, 0);
            }
        });
        bind!(on_cancel, |controller: &Rc<Self>| controller.cancel(false));
        bind!(on_close_dialog, |controller: &Rc<Self>| controller
            .cancel(true));
        controller.window.on_page({
            let weak = Rc::downgrade(&controller);
            move |direction| {
                if let Some(controller) = weak.upgrade()
                    && !controller.retired.get()
                {
                    let offset = if direction < 0 {
                        controller.offset.get().saturating_sub(64)
                    } else {
                        controller.offset.get().saturating_add(64)
                    };
                    controller.launch(false, Action::Page, offset);
                }
            }
        });
        controller.window.window().on_close_requested({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.cancel(true);
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
    pub(super) fn show(&self, aspect: f32) -> Result<(), slint::PlatformError> {
        if self.retired.get() {
            return Err(slint::PlatformError::Other(
                "Comparison window retired".into(),
            ));
        }
        self.aspect.set(aspect);
        self.refresh();
        self.window.show()
    }
    pub(super) fn retire(&self, errors: &mut Vec<String>) {
        self.retired.set(true);
        if let Some(control) = self.cancellation.borrow_mut().take() {
            let _ = control.cancel();
        }
        self.picker.set(self.picker.get().wrapping_add(1));
        self.timer.stop();
        self.clear_rows();
        super::hide_window("External map comparison", self.window.window(), errors);
    }
    #[cfg(test)]
    pub(super) fn window(&self) -> &ExternalComparisonWindow {
        &self.window
    }
    #[cfg(test)]
    pub(super) fn timer_running(&self) -> bool {
        self.timer.running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::sync::{Arc, Mutex};

    fn wait(controller: &Controller, shared: &SharedSession) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while lock(shared).external_comparison_state()["busy"] == true {
            assert!(Instant::now() < deadline, "comparison worker did not drain");
            std::thread::sleep(Duration::from_millis(5));
        }
        controller.refresh();
    }

    #[test]
    fn software_comparison_uses_shared_report_actions_and_retires_retained_callbacks() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("No assets required".into()));
        session
            .edit(Actor::User, "comparison points", |tx| {
                for (id, name) in [(10, "Entry relay"), (11, "Exit relay")] {
                    let mut entity = source_formats::vmf::Entity::new(id, "logic_relay");
                    entity.properties.set("targetname", name);
                    entity.properties.set("origin", format!("{} 0 64", id * 8));
                    tx.add_entity(entity)?;
                }
                Ok(())
            })
            .unwrap();
        let mut external = session.document().vmf().clone();
        external.entities[1]
            .properties
            .set("targetname", "Moved exit relay");
        let path = directory.path().join("external.vmf");
        std::fs::write(&path, external.write(&Default::default())).unwrap();
        let shared = Arc::new(Mutex::new(session));
        let access = DocumentAccess::unmanaged(shared.clone());
        let controller = Controller::new(&access).unwrap();
        controller.show(1.0).unwrap();
        controller.window.invoke_compare();
        let message = controller.window.get_message();
        assert!(message.contains(".vmf"), "{message}");
        controller.refresh();
        assert_eq!(controller.window.get_message(), message);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300));
        assert_eq!(controller.window.get_message(), message);
        assert!(lock(&shared).external_comparison_state()["error"].is_null());
        controller.window.set_file(path.to_str().unwrap().into());
        controller.window.invoke_compare();
        wait(&controller, &shared);
        assert!(controller.window.get_message().is_empty());
        assert_eq!(controller.window.get_rows().row_count(), 2);
        assert!(controller.window.get_can_act());
        controller.window.set_status("Changed".into());
        controller.window.invoke_filter_changed();
        assert_eq!(controller.window.get_rows().row_count(), 0);
        controller.window.invoke_refresh();
        wait(&controller, &shared);
        assert_eq!(controller.window.get_rows().row_count(), 1);
        controller.window.invoke_select_current();
        wait(&controller, &shared);
        assert_eq!(lock(&shared).selected_ids(), [ObjectId::Entity(11)]);
        controller
            .window
            .window()
            .set_size(slint::PhysicalSize::new(940, 820));
        crate::ui::tests::assert_ui_renders(controller.window.window(), "external-map-comparison");
        let before = lock(&shared).document().vmf().clone();
        lock(&shared).shutdown();
        let mut errors = Vec::new();
        controller.retire(&mut errors);
        assert!(errors.is_empty());
        assert!(!controller.timer.running());
        controller.window.invoke_compare();
        controller.window.invoke_refresh();
        controller.window.invoke_select_current();
        assert!(lock(&shared).external_comparison_state()["token"].is_null());
        assert_eq!(lock(&shared).document().vmf(), &before);
    }
}
