use glam::DVec3;
use map_core::compile_region::{Bounds, Definition};
use serde_json::Value;
use slint::ComponentHandle;
use source_formats::vmf::format_number;
use std::{cell::RefCell, rc::Rc};

use crate::{
    CompileRegionWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Session},
};

#[cfg(test)]
use crate::runtime::{SharedSession, lock};

#[derive(Default)]
struct Form {
    token: Option<u64>,
    seen: Option<String>,
    reviewed: Option<Option<Definition>>,
}

pub struct Controller {
    pub window: CompileRegionWindow,
    form: Rc<RefCell<Form>>,
}

fn fields(window: &CompileRegionWindow) -> Result<Definition, String> {
    let number = |text: slint::SharedString| {
        text.trim()
            .parse::<f64>()
            .map_err(|_| "Enter numeric region corners".to_string())
    };
    Ok(Definition {
        enabled: window.get_region_enabled(),
        bounds: Bounds {
            mins: DVec3::new(
                number(window.get_min_x())?,
                number(window.get_min_y())?,
                number(window.get_min_z())?,
            ),
            maxs: DVec3::new(
                number(window.get_max_x())?,
                number(window.get_max_y())?,
                number(window.get_max_z())?,
            ),
        }
        .validate()
        .map_err(|error| error.to_string())?,
    })
}

fn load(window: &CompileRegionWindow, region: &Value) {
    let values = |name: &str| {
        region[name].as_array().map(|values| {
            DVec3::new(
                values[0].as_f64().unwrap(),
                values[1].as_f64().unwrap(),
                values[2].as_f64().unwrap(),
            )
        })
    };
    if let (Some(mins), Some(maxs)) = (values("mins"), values("maxs")) {
        window.set_min_x(format_number(mins.x).into());
        window.set_min_y(format_number(mins.y).into());
        window.set_min_z(format_number(mins.z).into());
        window.set_max_x(format_number(maxs.x).into());
        window.set_max_y(format_number(maxs.y).into());
        window.set_max_z(format_number(maxs.z).into());
    }
    if let Some(enabled) = region["enabled"].as_bool() {
        window.set_region_enabled(enabled);
    }
}

fn refresh(window: &CompileRegionWindow, session: &Session, form: &RefCell<Form>) {
    form.borrow_mut().seen = Some(session.compile_region_stamp());
    let state = session.compile_region_state();
    window.set_scope_help(state["help"].as_str().unwrap().into());
    let committed = &state["committed"];
    let scope = &state["build_scope"];
    let label = if let Some(error) = scope["error"].as_str() {
        format!("Builds blocked: {error}")
    } else if let Some(error) = committed["error"].as_str() {
        format!(
            "Current builds: {}. Saved metadata needs explicit Clear before editing: {error}",
            if scope["enabled"] == true {
                "sealed region"
            } else {
                "full map"
            },
        )
    } else if committed.is_null() {
        "Current builds: full map · no region saved".into()
    } else {
        format!(
            "Current builds: {} · ({}) to ({})",
            if committed["enabled"] == true {
                "sealed region"
            } else {
                "full map · region disabled"
            },
            corner(&committed["mins"]),
            corner(&committed["maxs"])
        )
    };
    window.set_committed(label.into());
    let draft = &state["draft"];
    window.set_can_review(draft["valid"] == true);
    let form = form.borrow();
    if form.token.is_some() && draft["token"].as_u64() != form.token {
        window.set_message("The shared draft was closed or replaced. Preview your fields again, or Review current draft.".into());
    } else if form.token.is_some() && draft["valid"] != true {
        window.set_message("The map changed. Preview the region again.".into());
    }
    window.set_ready(
        draft["valid"] == true && draft["token"].as_u64() == form.token && form.reviewed.is_some(),
    );
}

fn corner(value: &Value) -> String {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|value| format_number(value.as_f64().unwrap()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn stage(
    window: &CompileRegionWindow,
    session: &mut Session,
    form: &RefCell<Form>,
    definition: Option<Definition>,
) {
    match session.preview_compile_region(Actor::User, definition, &session.compile_region_stamp()) {
        Ok(state) => {
            let mut form = form.borrow_mut();
            form.token = state["draft"]["token"].as_u64();
            form.reviewed = Some(definition);
            window.set_message(
                if definition.is_some() {
                    "Review the orange bounds, then Apply region."
                } else {
                    "Apply region will remove all compile-region metadata. Cancel keeps it."
                }
                .into(),
            );
        }
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, form);
}

fn cancel_owned(session: &mut Session, form: &RefCell<Form>) {
    let mut form = form.borrow_mut();
    if let Some(token) = form.token.take()
        && session.compile_region_state()["draft"]["token"].as_u64() == Some(token)
    {
        session
            .cancel_compile_region(token)
            .expect("matching compile-region draft token");
    }
    form.reviewed = None;
}

fn callback(
    window: &slint::Weak<CompileRegionWindow>,
    document: &DocumentContinuation,
    body: impl FnOnce(CompileRegionWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = document.upgrade().and_then(|access| {
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
    #[cfg(test)]
    pub fn new(
        shared: &SharedSession,
        aspect: impl Fn() -> f32 + 'static,
    ) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()), aspect)
    }

    pub fn new_with_access(
        access: &DocumentAccess,
        aspect: impl Fn() -> f32 + 'static,
    ) -> Result<Self, slint::PlatformError> {
        let window = CompileRegionWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_edited({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, _session| {
                    form.borrow_mut().reviewed = None;
                    window.set_ready(false);
                });
            }
        });
        window.on_use_bounds({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    let state = session.compile_region_state();
                    if let Some(error) = state["suggested_bounds"]["error"].as_str() {
                        window.set_message(error.into());
                        return;
                    }
                    load(&window, &state["suggested_bounds"]);
                    form.borrow_mut().reviewed = None;
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_preview({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    match fields(&window) {
                        Ok(value) => stage(&window, session, &form, Some(value)),
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_preview_clear({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    stage(&window, session, &form, None);
                });
            }
        });
        window.on_review({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    let state = session.compile_region_state();
                    let draft = &state["draft"];
                    if draft["valid"] != true {
                        window.set_message("No current draft to review.".into());
                        return;
                    }
                    let definition = if draft["clear"] == true {
                        None
                    } else {
                        load(&window, &draft["region"]);
                        match fields(&window) {
                            Ok(value) => Some(value),
                            Err(error) => {
                                window.set_message(error.into());
                                return;
                            }
                        }
                    };
                    {
                        let mut form = form.borrow_mut();
                        form.token = draft["token"].as_u64();
                        form.reviewed = Some(definition);
                    }
                    window.set_message(
                        if definition.is_none() {
                            "Review: remove all region metadata. Apply region confirms."
                        } else {
                            "Current draft loaded for review. Apply region confirms it."
                        }
                        .into(),
                    );
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_frame({
            let (window, document) = (window.as_weak(), access.downgrade());
            move || {
                callback(&window, &document, |window, session| {
                    if let Err(error) = session.frame_compile_region(Actor::User, aspect()) {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_apply({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    let (token, reviewed) = {
                        let form = form.borrow();
                        (form.token, form.reviewed)
                    };
                    let Some(token) = token.filter(|_| reviewed.is_some()) else {
                        window.set_message("Preview or review the region before applying.".into());
                        return;
                    };
                    if reviewed
                        .flatten()
                        .is_some_and(|reviewed| fields(&window).ok() != Some(reviewed))
                    {
                        window.set_message("Fields changed; preview the region again.".into());
                        window.set_ready(false);
                        return;
                    }
                    match session.apply_compile_region(Actor::User, token) {
                        Ok(_) => {
                            *form.borrow_mut() = Form::default();
                            window.set_message(
                                "Region applied. Compile uses the saved enabled state.".into(),
                            );
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_cancel({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel_owned(session, &form);
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel_owned(session, &form);
                });
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        let state = session.compile_region_state();
        if state["committed"]["mins"].is_array() {
            load(&self.window, &state["committed"]);
        } else {
            load(&self.window, &state["suggested_bounds"]);
            self.window.set_region_enabled(false);
        }
        self.form.borrow_mut().reviewed = None;
        self.window.set_message(
            state["suggested_bounds"]["error"]
                .as_str()
                .unwrap_or("Preview changes or explicitly review the current shared draft.")
                .into(),
        );
        refresh(&self.window, session, &self.form);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        if self.form.borrow().seen.as_ref() == Some(&session.compile_region_stamp()) {
            return;
        }
        refresh(&self.window, session, &self.form);
    }
}

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn region_controls_review_shared_drafts_and_cancel_only_their_own_preview() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("No assets needed".into()));
        session
            .create_block(
                Actor::Agent,
                DVec3::splat(-256.0),
                DVec3::splat(256.0),
                "test",
                false,
            )
            .unwrap();
        let original = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared, || 16.0 / 9.0).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.set_min_x("-128".into());
        controller.window.set_min_y("-128".into());
        controller.window.set_min_z("0".into());
        controller.window.set_max_x("128".into());
        controller.window.set_max_y("128".into());
        controller.window.set_max_z("256".into());
        controller.window.set_region_enabled(true);
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        assert_eq!(lock(&shared).document().vmf(), &original);
        let foreign = {
            let mut session = lock(&shared);
            let snapshot = session.compile_region_stamp();
            session
                .preview_compile_region(
                    Actor::Agent,
                    Some(Definition {
                        enabled: false,
                        bounds: Bounds {
                            mins: DVec3::splat(-64.0),
                            maxs: DVec3::splat(64.0),
                        },
                    }),
                    &snapshot,
                )
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap()
        };
        controller.window.invoke_cancel();
        assert_eq!(
            lock(&shared).compile_region_state()["draft"]["token"],
            foreign
        );
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_review();
        assert!(controller.window.get_ready());
        controller.window.invoke_apply();
        assert_eq!(
            lock(&shared).compile_region_state()["committed"]["enabled"],
            false
        );
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared).redo(Actor::Agent).unwrap();
        let applied = lock(&shared).document().vmf().clone();
        controller.window.invoke_preview_clear();
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &applied);
        controller.show(&lock(&shared)).unwrap();
        controller.window.set_region_enabled(true);
        controller.window.invoke_edited();
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "compile-region");
        controller.window.invoke_apply();
        assert_eq!(
            lock(&shared).compile_region_state()["committed"]["enabled"],
            true
        );
    }
}
