use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{Actor, CompileWindow, DocumentAccess, DocumentContinuation, Session, lock};

#[derive(Clone, Default)]
pub(super) struct Form {
    baseline: Rc<RefCell<Option<Value>>>,
    serial: Rc<Cell<u64>>,
}

fn configuration(state: &Value) -> Value {
    json!({
        "engine_root":state["configured_engine_root"],
        "steam_root":state["configured_steam_root"],
    })
}

fn accept_folder(
    window: &CompileWindow,
    serial: &Cell<u64>,
    pending: u64,
    kind: i32,
    path: &Path,
) -> bool {
    if serial.get() != pending || !window.get_capture_editing() || !window.get_capture_editable() {
        return false;
    }
    let path = path.display().to_string().into();
    match kind {
        0 => window.set_capture_engine_root(path),
        1 => window.set_capture_steam_root(path),
        _ => return false,
    }
    true
}

fn complete_folder(
    window: &slint::Weak<CompileWindow>,
    target: &DocumentContinuation,
    serial: &Cell<u64>,
    pending: u64,
    kind: i32,
    path: &Path,
) {
    let Some(current) = window.upgrade() else {
        return;
    };
    if serial.get() != pending || !current.get_capture_editing() || !current.get_capture_editable()
    {
        return;
    }
    super::admitted(window, target, |window, _| {
        accept_folder(&window, serial, pending, kind, path);
    });
}

impl Form {
    pub(super) fn discard(&self, window: &CompileWindow) {
        *self.baseline.borrow_mut() = None;
        self.serial.set(self.serial.get().wrapping_add(1));
        window.set_capture_editing(false);
        window.set_capture_current(true);
        window.set_capture_engine_root("".into());
        window.set_capture_steam_root("".into());
    }

    pub(super) fn refresh(&self, window: &CompileWindow, session: &Session) {
        let state = session.native_capture_runtime();
        let engine = state["engine_root"].as_str().unwrap_or("Unavailable");
        let steam = state["steam_root"]
            .as_str()
            .unwrap_or("Automatic from checked Steam client configuration");
        let readiness = if state["ready"] == true {
            "Capture runtime ready".to_owned()
        } else {
            format!(
                "Capture unavailable: {}",
                state["reason"]
                    .as_str()
                    .unwrap_or("Configure a supported CS:S installation")
            )
        };
        let activity = state["native_activity"]["message"]
            .as_str()
            .unwrap_or("Native activity availability is unknown.");
        window.set_capture_summary(
            if state["ready"] == true && state["native_activity"]["managed"] == true {
                format!("Capture runtime ready. {activity}")
            } else if state["ready"] == true {
                "Capture runtime ready. Configure capture… shows its folders.".to_owned()
            } else {
                "Capture runtime unavailable. Open Configure capture… for details.".to_owned()
            }
            .into(),
        );
        window.set_capture_details(
            format!("{readiness}\n{activity}\nCS:S: {engine}\nSteam: {steam}").into(),
        );
        window.set_capture_editable(session.compile_settings()["editable"] == true);
        let baseline = self.baseline.borrow();
        window.set_capture_editing(baseline.is_some());
        window.set_capture_current(
            baseline
                .as_ref()
                .is_none_or(|value| *value == configuration(&state)),
        );
        if baseline.is_none() {
            window.set_capture_engine_root(
                state["configured_engine_root"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            );
            window.set_capture_steam_root(
                state["configured_steam_root"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            );
        }
    }
}

pub(super) fn bind(
    window: &CompileWindow,
    access: &DocumentAccess,
    recipes: &Rc<RefCell<super::RecipeForm>>,
) -> Form {
    let form = Form::default();
    window.on_change_reflections({
        let (window, target, recipes) = (window.as_weak(), access.downgrade(), recipes.clone());
        move |enabled| {
            super::admitted(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = session.update_build_settings(Actor::User, crate::session::compilation::BuildSettingsUpdate {
                    generate_reflections: Some(enabled),
                    ..Default::default()
                });
                super::refresh_recipes(&window, &session, &recipes);
                window.set_message(match result {
                    Ok(_) if enabled => "Reflection generation enabled. The next Compile or Run Map captures and verifies the selected lighting channels before publishing.".into(),
                    Ok(_) => "Reflection generation disabled.".into(),
                    Err(error) => error.into(),
                });
            });
        }
    });
    window.on_begin_capture_edit({
        let (window, target, baseline, serial) = (window.as_weak(), access.downgrade(), form.baseline.clone(), form.serial.clone());
        move || {
            super::admitted(&window, &target, |window, shared| {
                let session = lock(shared);
                if session.compile_settings()["editable"] != true {
                    window.set_message("Wait for the active build before editing capture runtime settings.".into());
                    return;
                }
                let state = session.native_capture_runtime();
                window.set_capture_engine_root(state["configured_engine_root"].as_str().unwrap_or_default().into());
                window.set_capture_steam_root(state["configured_steam_root"].as_str().unwrap_or_default().into());
                *baseline.borrow_mut() = Some(configuration(&state));
                serial.set(serial.get().wrapping_add(1));
                window.set_capture_editing(true);
                window.set_capture_current(true);
                window.set_message("Choose the CS:S installation and Steam client overrides, then Apply capture runtime. Empty overrides use automatic values.".into());
            });
        }
    });
    window.on_choose_capture_root({
        let (window, target, serial) = (window.as_weak(), access.downgrade(), form.serial.clone());
        move |kind| {
            super::admitted(&window, &target, |current, _| {
                if !current.get_capture_editing()
                    || !current.get_capture_editable()
                    || !(0..=1).contains(&kind)
                {
                    return;
                }
                let pending = serial.get();
                let (window, target, serial) = (window.clone(), target.clone(), serial.clone());
                let title = if kind == 0 {
                    "Choose CS:S installation"
                } else {
                    "Choose Steam client"
                };
                slint::spawn_local(async move {
                    let Some(folder) = rfd::AsyncFileDialog::new()
                        .set_title(title)
                        .pick_folder()
                        .await
                    else {
                        return;
                    };
                    complete_folder(&window, &target, &serial, pending, kind, folder.path());
                })
                .expect("event loop is running");
            });
        }
    });
    window.on_apply_capture_runtime({
        let (window, target, baseline, serial) = (window.as_weak(), access.downgrade(), form.baseline.clone(), form.serial.clone());
        move || {
            super::admitted(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let Some(reviewed) = baseline.borrow().clone() else { return; };
                if reviewed != configuration(&session.native_capture_runtime()) {
                    window.set_capture_current(false);
                    window.set_message("Capture configuration changed. Cancel this edit and review the current configuration.".into());
                    return;
                }
                let engine = window.get_capture_engine_root();
                let steam = window.get_capture_steam_root();
                let result = session.configure_native_capture_runtime(Actor::User,
                    (!engine.trim().is_empty()).then(|| Path::new(engine.trim())),
                    (!steam.trim().is_empty()).then(|| Path::new(steam.trim())));
                window.set_message(match result {
                    Ok(_) => {
                        *baseline.borrow_mut() = None;
                        serial.set(serial.get().wrapping_add(1));
                        window.set_capture_editing(false);
                        "Capture runtime applied. No game was launched.".into()
                    },
                    Err(error) => error.into(),
                });
            });
        }
    });
    window.on_cancel_capture_edit({
        let (window, form) = (window.as_weak(), form.clone());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            form.discard(&window);
            window.set_message("Runtime edit cancelled. Capture settings are unchanged.".into());
        }
    });
    form
}
