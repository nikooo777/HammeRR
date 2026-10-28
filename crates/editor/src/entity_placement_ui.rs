use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use map_core::query::Ray;
use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    EntityPlacementWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Session, entity_placement::Request},
};

#[derive(Default)]
struct Form {
    token: Option<u64>,
    snapshot: String,
    properties: BTreeMap<String, String>,
    dirty: bool,
}

pub struct Controller {
    pub window: EntityPlacementWindow,
    form: Rc<RefCell<Form>>,
}

fn angle_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(|component| {
            let Ok(number) = component.parse::<f64>() else {
                return component.to_string();
            };
            if number.abs() < 0.0005 {
                return "0".into();
            }
            format!("{number:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn present(window: &EntityPlacementWindow, report: &Value, dirty: bool) {
    let draft = &report["draft"];
    let own = draft.is_null() || draft["owner"] == Actor::User.label();
    window.set_owned(own);
    window.set_shared_draft(!draft.is_null() && !own);
    window.set_ready(!dirty && draft["valid"] == true && !draft["plan"].is_null());
    window.set_grid(
        report["grid_size"]
            .as_f64()
            .unwrap_or(16.0)
            .to_string()
            .into(),
    );
    let plan = &draft["plan"];
    window.set_summary(if plan.is_null() { "".into() } else {
        let origin = &plan["origin"];
        let low = &plan["bounds"]["mins"];
        let high = &plan["bounds"]["maxs"];
        let angles = plan["angles"].as_str().or(plan["angle"].as_str()).map(angle_text).unwrap_or_else(||"default".into());
        let pitch = plan["pitch"].as_str().map(|pitch|format!(" · light pitch {}", angle_text(pitch))).unwrap_or_default();
        format!("Origin · {:.3}, {:.3}, {:.3} · {} bounds\nBounds minimum · {:.3}, {:.3}, {:.3}\nBounds maximum · {:.3}, {:.3}, {:.3}\nTarget · {} · normal {}\nSource angles · {angles}{pitch}\nContact offset · {:.3} units",
            origin[0].as_f64().unwrap(), origin[1].as_f64().unwrap(), origin[2].as_f64().unwrap(),
            if plan["bounds_source"] == "model_mesh" { "model mesh" } else { "editor helper" },
            low[0].as_f64().unwrap(),low[1].as_f64().unwrap(),low[2].as_f64().unwrap(),
            high[0].as_f64().unwrap(),high[1].as_f64().unwrap(),high[2].as_f64().unwrap(),
            plan["target"].as_str().unwrap(),plan["normal"],plan["contact_offset"].as_f64().unwrap()).into()
    });
    window.set_message(if let Some(error) = draft["error"].as_str() {
        error.into()
    } else if !own {
        "Review shared placement to take ownership before picking, applying or cancelling it."
            .into()
    } else if dirty {
        "Pick a surface again after changing settings.".into()
    } else if draft["picking"] == true {
        "Click a visible surface. Escape or Cancel picking abandons this draft.".into()
    } else if let Some(error) = plan["model_error"].as_str() {
        format!("Model unavailable; using editor helper bounds. {error}").into()
    } else if !plan.is_null() {
        "Review the preview bounds in the viewport. Apply entity adds one Undo step.".into()
    } else {
        "Choose options, then Pick surface.".into()
    });
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = EntityPlacementWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let access = access.downgrade();
        window.on_invalidate({
            let (window, form, access) = (window.as_weak(), form.clone(), access.clone());
            move || {
                callback(&window, &access, |window, _| {
                    form.borrow_mut().dirty = true;
                    window.set_ready(false);
                    window.set_message("Pick a surface again after changing settings.".into());
                });
            }
        });
        window.on_pick({
            let (window, form, access) = (window.as_weak(), form.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    let result = (|| {
                        let request = Request {
                            classname: window.get_classname().to_string(),
                            properties: form.borrow().properties.clone(),
                            offset: window
                                .get_offset()
                                .parse()
                                .map_err(|_| "Enter a finite surface offset")?,
                            align: window.get_align(),
                            snap: window.get_snap(),
                        };
                        let snapshot = form.borrow().snapshot.clone();
                        session.begin_entity_placement(Actor::User, request, &snapshot)
                    })();
                    match result {
                        Ok(report) => {
                            let mut form = form.borrow_mut();
                            form.token = report["draft"]["token"].as_u64();
                            form.snapshot = report["snapshot"].as_str().unwrap().into();
                            form.dirty = false;
                            present(window, &report, false);
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_review({
            let (window, form, access) = (window.as_weak(), form.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    let result = form
                        .borrow()
                        .token
                        .ok_or("No shared entity placement to review".into())
                        .and_then(|token| session.review_entity_placement(Actor::User, token));
                    match result {
                        Ok(report) => {
                            let mut form = form.borrow_mut();
                            form.token = report["draft"]["token"].as_u64();
                            form.snapshot = report["snapshot"].as_str().unwrap().into();
                            present(window, &report, form.dirty);
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_apply({
            let (window, form, access) = (window.as_weak(), form.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    let result = (|| {
                        let form = form.borrow();
                        if form.dirty {
                            return Err("Pick a surface again after changing settings".into());
                        }
                        session.apply_entity_placement(
                            Actor::User,
                            form.token.ok_or("Pick a surface first")?,
                        )
                    })();
                    match result {
                        Ok(_) => {
                            form.borrow_mut().token = None;
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_cancel({
            let (window, form, access) = (window.as_weak(), form.clone(), access.clone());
            move || {
                callback(&window, &access, |_, session| {
                    if let Some(token) = form.borrow_mut().token.take() {
                        let _ = session.cancel_entity_placement(Actor::User, token);
                    }
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, form })
    }

    pub fn initialize(&self, session: &Session, classname: &str) {
        let report = session.get_entity_placement();
        let draft = &report["draft"];
        let mut form = self.form.borrow_mut();
        form.snapshot = report["snapshot"].as_str().unwrap().into();
        form.token = draft["token"].as_u64();
        form.properties = serde_json::from_value(draft["properties"].clone()).unwrap_or_default();
        form.dirty = false;
        self.window
            .set_classname(draft["classname"].as_str().unwrap_or(classname).into());
        self.window.set_offset(
            draft["offset"]
                .as_f64()
                .unwrap_or_default()
                .to_string()
                .into(),
        );
        self.window.set_align(draft["align"] == true);
        self.window
            .set_snap(draft["snap"].as_bool().unwrap_or(session.spatial().snap));
        present(&self.window, &report, false);
    }

    pub fn pick_at_ray(&self, session: &mut Session, ray: Ray) -> Result<(), String> {
        let token = self.form.borrow().token.ok_or("Reopen entity placement")?;
        let report = session.pick_entity_placement(Actor::User, token, ray)?;
        let mut form = self.form.borrow_mut();
        form.snapshot = report["snapshot"].as_str().unwrap().into();
        form.dirty = false;
        present(&self.window, &report, false);
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        let report = session.get_entity_placement();
        if self.form.borrow().token == report["draft"]["token"].as_u64() {
            self.form.borrow_mut().snapshot = report["snapshot"].as_str().unwrap().into();
            present(&self.window, &report, self.form.borrow().dirty);
        } else {
            self.window.set_ready(false);
            self.window.set_owned(false);
            self.window.set_message(
                "Entity placement changed. Reopen Entity… → Pick surface… to review it.".into(),
            );
        }
    }
}

fn callback<R>(
    window: &slint::Weak<EntityPlacementWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&EntityPlacementWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}
