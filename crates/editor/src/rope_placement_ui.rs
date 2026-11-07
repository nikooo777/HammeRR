use std::time::Duration;
use std::{cell::Cell, cell::RefCell, rc::Rc};

use glam::DVec2;
use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    RopePlacementWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{
        Actor, Pointer, Session,
        rope_placement::{Endpoint, Request},
    },
};

#[derive(Default)]
struct Form {
    snapshot: String,
    token: Option<u64>,
    dirty: bool,
    error: String,
}

pub struct Controller {
    pub window: RopePlacementWindow,
    form: Rc<RefCell<Form>>,
    viewport: Rc<Cell<(u32, u32)>>,
    settings_timer: Rc<slint::Timer>,
}

#[derive(Clone, Copy)]
enum Action {
    Settings,
    Edited,
    Pick(Endpoint),
    Review,
    Apply,
    Cancel,
}

fn coordinates(value: &Value) -> String {
    let Some(point) = value["point"].as_array() else {
        return "Not picked".into();
    };
    let xyz = point.iter().filter_map(Value::as_f64).collect::<Vec<_>>();
    if let [x, y, z] = xyz.as_slice() {
        format!("{x:.2}, {y:.2}, {z:.2}")
    } else {
        "Not picked".into()
    }
}

fn present(window: &RopePlacementWindow, report: &Value, form: &Form) {
    let draft = &report["draft"];
    let owned = draft.is_null() || draft["owner"] == Actor::User.label();
    window.set_owned(owned);
    window.set_shared_draft(!draft.is_null() && !owned);
    window.set_ready(
        form.error.is_empty()
            && draft["valid"] == true
            && draft["ready"] == true
            && draft["picking_endpoint"].is_null(),
    );
    window.set_start_position(coordinates(&draft["start"]).into());
    window.set_end_position(coordinates(&draft["end"]).into());
    window.set_message_error(
        owned
            && (!form.error.is_empty()
                || draft["error"].is_string()
                || draft["picking_error"].is_string()),
    );
    let summary = if draft["ready"] == true {
        draft["plan"]["segment_count"]
            .as_u64()
            .map(|segments| {
                format!(
                    "{segments} preview segments · Slack {} units",
                    draft["slack"]
                )
            })
            .unwrap_or_else(|| "Review the pending rope in the viewport.".into())
    } else {
        String::new()
    };
    window.set_summary(summary.into());
    let message = if !owned {
        "Review shared placement to edit it. Close leaves the agent's draft unchanged.".to_owned()
    } else if !form.error.is_empty() {
        form.error.clone()
    } else if let Some(error) = draft["error"].as_str() {
        error.to_owned()
    } else if let Some(error) = draft["picking_error"].as_str() {
        error.to_owned()
    } else if draft["picking"] == true {
        format!(
            "Click a visible surface for the {}. Escape stops picking.",
            draft["picking_endpoint"].as_str().unwrap_or("endpoint")
        )
    } else if form.dirty {
        "Settings changed. Press Enter to preview or Apply rope to use them.".into()
    } else if draft["ready"] == true {
        "Apply rope adds the endpoints and span as one Undo step.".into()
    } else {
        "Pick start and end to preview a rope.".into()
    };
    window.set_message(message.into());
}

fn accept(window: &RopePlacementWindow, form: &mut Form, report: &Value, settings: bool) {
    let draft = &report["draft"];
    form.snapshot = report["snapshot"].as_str().unwrap_or_default().into();
    form.token = draft["token"].as_u64();
    if settings && !draft.is_null() {
        window.set_material(draft["material"].as_str().unwrap_or_default().into());
        window.set_slack(draft["slack"].to_string().into());
        window.set_rope_width(draft["width"].to_string().into());
    }
    present(window, report, form);
}

fn settings(
    window: &RopePlacementWindow,
    form: &mut Form,
    session: &mut Session,
) -> Result<(), String> {
    form.dirty = true;
    let request = Request {
        material: window.get_material().to_string(),
        slack: window
            .get_slack()
            .trim()
            .parse()
            .map_err(|_| "Slack must be a whole number of units")?,
        width: window
            .get_rope_width()
            .trim()
            .parse()
            .map_err(|_| "Width must be a number of units")?,
    };
    let current = session.get_rope_placement();
    let draft = &current["draft"];
    let unchanged = form.token.is_some()
        && form.token == draft["token"].as_u64()
        && draft["owner"] == Actor::User.label()
        && draft["valid"] == true
        && draft["material"].as_str() == Some(request.material.as_str())
        && draft["slack"].as_i64() == Some(i64::from(request.slack))
        && draft["width"].as_f64() == Some(request.width);
    let report = if unchanged {
        current
    } else {
        match form.token {
            Some(token) => session.update_rope_placement(Actor::User, token, request)?,
            None => session.begin_rope_placement(Actor::User, request, &form.snapshot)?,
        }
    };
    form.dirty = false;
    form.error.clear();
    accept(window, form, &report, true);
    Ok(())
}

fn action(
    window: &RopePlacementWindow,
    form: &mut Form,
    viewport: (u32, u32),
    session: &mut Session,
    action: Action,
) -> Result<(), String> {
    match action {
        Action::Settings => settings(window, form, session),
        Action::Edited => {
            form.dirty = true;
            form.error.clear();
            present(window, &session.get_rope_placement(), form);
            Ok(())
        }
        Action::Pick(endpoint) => {
            settings(window, form, session)?;
            let token = form.token.ok_or("Open Place rope again")?;
            let report = session.begin_rope_placement_pick(
                Actor::User,
                token,
                endpoint,
                session.camera(),
                viewport,
                &session.rope_placement_pick_stamp(),
            )?;
            accept(window, form, &report, false);
            window.hide().map_err(|error| error.to_string())
        }
        Action::Review => {
            let token = form.token.ok_or("No shared rope placement to review")?;
            let report = session.review_rope_placement(Actor::User, token)?;
            form.dirty = false;
            form.error.clear();
            accept(window, form, &report, true);
            Ok(())
        }
        Action::Apply => {
            if form.dirty {
                settings(window, form, session)?;
            }
            session.apply_rope_placement(
                Actor::User,
                form.token.ok_or("Pick both endpoints first")?,
            )?;
            form.token = None;
            window.invoke_applied();
            window.hide().map_err(|error| error.to_string())
        }
        Action::Cancel => {
            let report = session.get_rope_placement();
            if report["draft"]["owner"] == Actor::User.label() {
                if report["draft"]["token"].as_u64() != form.token {
                    return Err("Rope placement changed. Reopen Place rope to review it.".into());
                }
                session.cancel_rope_placement(
                    Actor::User,
                    form.token.ok_or("Rope placement changed")?,
                )?;
            }
            form.token = None;
            form.dirty = false;
            form.error.clear();
            window.hide().map_err(|error| error.to_string())
        }
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = RopePlacementWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let viewport = Rc::new(Cell::new((1, 1)));
        let settings_timer = Rc::new(slint::Timer::default());
        let access = access.downgrade();
        for action_kind in [
            Action::Settings,
            Action::Edited,
            Action::Pick(Endpoint::Start),
            Action::Pick(Endpoint::End),
            Action::Review,
            Action::Apply,
            Action::Cancel,
        ] {
            let (target, form, access, viewport) = (
                window.as_weak(),
                form.clone(),
                access.clone(),
                viewport.clone(),
            );
            let timer = settings_timer.clone();
            let handler = move || {
                timer.stop();
                if matches!(action_kind, Action::Settings) {
                    let admitted = callback(&target, &access, |window, session| {
                        let _ = action(
                            window,
                            &mut form.borrow_mut(),
                            viewport.get(),
                            session,
                            Action::Edited,
                        );
                    });
                    if admitted.is_none() {
                        return;
                    }
                    let (target, form, access) = (target.clone(), form.clone(), access.clone());
                    timer.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(250),
                        move || {
                            callback(&target, &access, |window, session| {
                                let mut form = form.borrow_mut();
                                if let Err(error) = settings(window, &mut form, session) {
                                    form.error = error;
                                    present(window, &session.get_rope_placement(), &form);
                                }
                            });
                        },
                    );
                    return;
                }
                callback(&target, &access, |window, session| {
                    let mut form = form.borrow_mut();
                    if let Err(error) =
                        action(window, &mut form, viewport.get(), session, action_kind)
                    {
                        form.error = error;
                        present(window, &session.get_rope_placement(), &form);
                    }
                });
            };
            match action_kind {
                Action::Settings => window.on_settings_changed(handler),
                Action::Edited => window.on_settings_edited(handler),
                Action::Pick(Endpoint::Start) => window.on_pick_start(handler),
                Action::Pick(Endpoint::End) => window.on_pick_end(handler),
                Action::Review => window.on_review(handler),
                Action::Apply => window.on_apply(handler),
                Action::Cancel => window.on_cancel(handler),
            }
        }
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self {
            window,
            form,
            viewport,
            settings_timer,
        })
    }

    pub fn set_viewport(&self, viewport: (u32, u32)) {
        self.viewport.set(viewport);
    }

    pub fn suspend(&self) {
        self.settings_timer.stop();
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.refresh(session);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        let report = session.get_rope_placement();
        let mut form = self.form.borrow_mut();
        let changed = form.token != report["draft"]["token"].as_u64();
        if changed {
            form.dirty = false;
            form.error.clear();
        }
        let settings = changed || !form.dirty;
        accept(&self.window, &mut form, &report, settings);
    }

    pub fn pick_at_pixel(
        &self,
        session: &mut Session,
        position: DVec2,
        size: (u32, u32),
    ) -> Result<(), String> {
        self.settings_timer.stop();
        self.refresh(session);
        let token = self.form.borrow().token.ok_or("Open Place rope again")?;
        let report =
            session.pick_rope_placement_pixel(Actor::User, token, Pointer { position, size })?;
        let mut form = self.form.borrow_mut();
        form.error.clear();
        accept(&self.window, &mut form, &report, false);
        self.window.show().map_err(|error| error.to_string())
    }

    pub fn cancel_pick(&self, session: &mut Session) -> Result<(), String> {
        self.settings_timer.stop();
        let token = self.form.borrow().token.ok_or("Open Place rope again")?;
        let report = session.cancel_rope_placement_pick(Actor::User, token)?;
        let mut form = self.form.borrow_mut();
        form.error.clear();
        accept(&self.window, &mut form, &report, false);
        self.window.show().map_err(|error| error.to_string())
    }

    pub fn report_error(&self, session: &Session, error: &str) {
        let mut form = self.form.borrow_mut();
        form.error = error.into();
        present(&self.window, &session.get_rope_placement(), &form);
    }

    pub fn material_target(&self, session: &Session) -> String {
        self.settings_timer.stop();
        format!(
            "{}:{:?}:{}:{}:{}",
            session.rope_placement_stamp(),
            self.form.borrow().token,
            self.window.get_material(),
            self.window.get_slack(),
            self.window.get_rope_width()
        )
    }

    pub fn choose_material(
        &self,
        session: &mut Session,
        target: &str,
        material: &str,
    ) -> Result<(), String> {
        if self.material_target(session) != target {
            return Err("Rope settings or map changed. Open the material picker again.".into());
        }
        self.window.set_material(material.into());
        let mut form = self.form.borrow_mut();
        let result = settings(&self.window, &mut form, session);
        if let Err(error) = &result {
            form.error.clone_from(error);
            present(&self.window, &session.get_rope_placement(), &form);
        }
        result
    }
}

fn callback<R>(
    window: &slint::Weak<RopePlacementWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&RopePlacementWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_ready(false);
            window.set_message_error(true);
            window.set_message(error.into());
            None
        }
    }
}
