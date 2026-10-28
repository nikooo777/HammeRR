use std::{cell::RefCell, rc::Rc};

use glam::DVec3;
use serde_json::Value;
use slint::ComponentHandle;
use source_formats::vmf::format_number;

use crate::{
    AppWindow, WorkingRegionWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Session, working_region::Action},
};

pub struct Controller {
    pub window: WorkingRegionWindow,
    snapshot: Rc<RefCell<String>>,
}

fn load(window: &WorkingRegionWindow, bounds: &Value) {
    let text =
        |axis: usize, corner: usize| format_number(bounds[corner][axis].as_f64().unwrap()).into();
    window.set_min_x(text(0, 0));
    window.set_min_y(text(1, 0));
    window.set_min_z(text(2, 0));
    window.set_max_x(text(0, 1));
    window.set_max_y(text(1, 1));
    window.set_max_z(text(2, 1));
}

fn fields(window: &WorkingRegionWindow) -> Result<[DVec3; 2], String> {
    let number = |text: slint::SharedString| {
        text.trim()
            .parse::<f64>()
            .map_err(|_| "Enter finite numeric region corners".to_string())
    };
    Ok([
        DVec3::new(
            number(window.get_min_x())?,
            number(window.get_min_y())?,
            number(window.get_min_z())?,
        ),
        DVec3::new(
            number(window.get_max_x())?,
            number(window.get_max_y())?,
            number(window.get_max_z())?,
        ),
    ])
}

fn refresh(
    window: &WorkingRegionWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    reset: bool,
) {
    let state = session.working_region_summary();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    window.set_summary(
        if state["enabled"] == true {
            format!(
                "Working region active · Excluded entries: {} · Excluded selection items: {}",
                state["excluded"], state["selected_excluded"]
            )
        } else {
            "Working region off · saved visibility and compilation are unchanged".into()
        }
        .into(),
    );
    if reset {
        window.set_region_enabled(state["enabled"] == true);
        if !state["mins"].is_null() {
            load(window, &serde_json::json!([state["mins"], state["maxs"]]));
        }
    }
}

pub fn bind_clear_with_access(app: &AppWindow, access: &DocumentAccess) {
    app.on_clear_working_region({
        let (app, target) = (app.as_weak(), access.downgrade());
        move || {
            let Some(app) = app.upgrade() else {
                return;
            };
            let result = target.upgrade().and_then(|access| {
                access.with_session(|session| {
                    let snapshot = session.working_region_stamp();
                    match session.set_working_region(Actor::User, Action::Clear, &snapshot) {
                        Ok(_) => app.set_working_region_active(false),
                        Err(error) => app.set_message(error.into()),
                    }
                    Ok(())
                })
            });
            if let Err(error) = result {
                app.set_message(error.into());
            }
        }
    });
}

fn callback(
    window: &slint::Weak<WorkingRegionWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(WorkingRegionWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(window.clone_strong(), session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = WorkingRegionWindow::new()?;
        let snapshot = Rc::new(RefCell::new(String::new()));
        window.on_apply({
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                callback(&window, &target, |window, session| {
                    let result = fields(&window).and_then(|bounds| {
                        session.set_working_region(
                            Actor::User,
                            Action::Set {
                                bounds,
                                enabled: window.get_region_enabled(),
                            },
                            &snapshot.borrow(),
                        )
                    });
                    match result {
                        Ok(_) => {
                            window.set_message(
                                "View updated. Source and compilation are unchanged.".into(),
                            );
                            refresh(&window, session, &snapshot, false);
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_use_bounds({
            let (window,target,snapshot)=(window.as_weak(),access.downgrade(),snapshot.clone());
            move || {
                callback(&window, &target, |window, session| {
                if *snapshot.borrow()!=session.working_region_stamp() { window.set_message("Map, selection or view changed. Reopen Working region to inspect it again.".into()); return; }
                match session.working_region_selection_bounds() { Ok(bounds)=> { load(&window,&serde_json::json!(bounds.map(|point|point.to_array()))); window.set_region_enabled(true);window.set_message("Selection bounds loaded. Apply updates the view.".into()); }, Err(error)=>window.set_message(error.into()) }
                });
            }
        });
        window.on_clear({
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                callback(&window, &target, |window, session| {
                    let stamp = snapshot.borrow().clone();
                    match session.set_working_region(Actor::User, Action::Clear, &stamp) {
                        Ok(_) => {
                            window.set_message("Working region cleared.".into());
                            refresh(&window, session, &snapshot, true);
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_cancel({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        Ok(Self { window, snapshot })
    }

    pub fn show(&self, session: &mut Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, session, &self.snapshot, true);
        self.window.set_message("Use selection bounds or enter corners, then Apply. Clear working region restores otherwise-visible objects.".into());
        self.window.show()
    }

    pub fn refresh(&self, session: &mut Session) {
        if *self.snapshot.borrow() != session.working_region_stamp() {
            refresh(&self.window, session, &self.snapshot, true);
            self.window
                .set_message("Map, selection or view changed. Current region reloaded.".into());
        }
    }
}
