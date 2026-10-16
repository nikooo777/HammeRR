use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{Actor, CompileWindow, Session, SharedSession, lock};

#[derive(Default)]
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

impl Form {
    pub(super) fn refresh(&self, window: &CompileWindow, session: &Session) {
        let state = session.native_capture_runtime();
        let engine = state["engine_root"].as_str().unwrap_or("Unavailable");
        let steam = state["steam_root"]
            .as_str()
            .unwrap_or("Automatic from running Steam client at capture");
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
        window.set_capture_summary(format!("{readiness}\nCS:S: {engine}\nSteam: {steam}").into());
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

pub(super) fn bind(window: &CompileWindow, shared: &SharedSession) -> Form {
    let form = Form::default();
    window.on_change_reflections({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |enabled| {
            let mut session = lock(&shared);
            let result = session.update_build_settings(Actor::User, crate::session::compilation::BuildSettingsUpdate {
                generate_reflections: Some(enabled),
                ..Default::default()
            });
            let window = window.unwrap();
            window.set_generate_reflections(session.compile_options().generate_reflections);
            window.set_message(match result {
                Ok(_) if enabled => "LDR reflection generation enabled. The next Compile or Run Map captures and verifies reflections before publishing.".into(),
                Ok(_) => "LDR reflection generation disabled.".into(),
                Err(error) => error.into(),
            });
        }
    });
    window.on_begin_capture_edit({
        let (window, shared, baseline, serial) = (window.as_weak(), shared.clone(), form.baseline.clone(), form.serial.clone());
        move || {
            let session = lock(&shared);
            let window = window.unwrap();
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
        }
    });
    window.on_choose_capture_root({
        let (window, serial) = (window.as_weak(), form.serial.clone());
        move |kind| {
            let current = window.unwrap();
            if !current.get_capture_editing()
                || !current.get_capture_editable()
                || !(0..=1).contains(&kind)
            {
                return;
            }
            let pending = serial.get();
            let (window, serial) = (window.clone(), serial.clone());
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
                let Some(window) = window.upgrade() else {
                    return;
                };
                if serial.get() != pending
                    || !window.get_capture_editing()
                    || !window.get_capture_editable()
                {
                    return;
                }
                let path = folder.path().display().to_string().into();
                if kind == 0 {
                    window.set_capture_engine_root(path);
                } else {
                    window.set_capture_steam_root(path);
                }
            })
            .expect("event loop is running");
        }
    });
    window.on_apply_capture_runtime({
        let (window, shared, baseline, serial) = (window.as_weak(), shared.clone(), form.baseline.clone(), form.serial.clone());
        move || {
            let mut session = lock(&shared);
            let window = window.unwrap();
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
        }
    });
    window.on_cancel_capture_edit({
        let (window, baseline, serial) =
            (window.as_weak(), form.baseline.clone(), form.serial.clone());
        move || {
            *baseline.borrow_mut() = None;
            serial.set(serial.get().wrapping_add(1));
            let window = window.unwrap();
            window.set_capture_editing(false);
            window.set_message("Runtime edit cancelled. Capture settings are unchanged.".into());
        }
    });
    form
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_ui::Controller;
    use std::sync::{Arc, Mutex};

    #[test]
    fn reflection_controls_toggle_review_cancel_and_freeze_with_the_shared_job() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (directory, game, _) = crate::test_assets::game_process(true);
        let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
        let original = lock(&shared).document().vmf().clone();
        let controller = Controller::new(&shared).unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_generate_reflections());
        controller.window.invoke_change_reflections(true);
        assert!(!controller.window.get_generate_reflections());
        assert!(controller.window.get_message().contains("Direct LDR"));
        controller.window.invoke_change_lighting(1);
        controller.window.invoke_change_reflections(true);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_generate_reflections());
        controller.window.invoke_change_lighting(2);
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
        assert!(controller.window.get_generate_reflections());
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-reflection-controls");
        let runtime = lock(&shared).native_capture_runtime();
        controller.window.invoke_begin_capture_edit();
        assert!(controller.window.get_capture_editing());
        controller
            .window
            .window()
            .set_size(slint::LogicalSize::new(720.0, 1400.0));
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-reflection-runtime");
        controller
            .window
            .window()
            .set_size(slint::LogicalSize::new(720.0, 920.0));
        controller
            .window
            .set_capture_engine_root("/missing/hammerr-capture-install".into());
        controller
            .window
            .set_capture_steam_root("/missing/hammerr-steam-client".into());
        assert_eq!(lock(&shared).native_capture_runtime(), runtime);
        controller.window.invoke_cancel_capture_edit();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_capture_editing());
        assert_eq!(lock(&shared).native_capture_runtime(), runtime);
        controller.window.invoke_begin_capture_edit();
        controller
            .window
            .set_capture_engine_root("/missing/hammerr-capture-install".into());
        controller.window.invoke_apply_capture_runtime();
        assert!(controller.window.get_capture_editing());
        assert_eq!(
            controller.window.get_capture_engine_root(),
            "/missing/hammerr-capture-install"
        );
        assert_eq!(lock(&shared).native_capture_runtime(), runtime);
        controller.window.invoke_cancel_capture_edit();
        controller.window.invoke_change_reflections(false);
        controller.window.invoke_begin_capture_edit();
        let request = lock(&shared)
            .begin_compile(Actor::User, Some(&directory.path().join("room.bsp")))
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_lighting_enabled());
        assert!(!controller.window.get_capture_editable());
        assert_eq!(
            lock(&shared).compile_status()["job"]["settings"]["generate_reflections"],
            false
        );
        controller.window.invoke_change_reflections(true);
        controller.window.invoke_apply_capture_runtime();
        assert!(!lock(&shared).compile_options().generate_reflections);
        assert_eq!(lock(&shared).native_capture_runtime(), runtime);
        assert!(controller.window.get_capture_editing());
        controller.window.invoke_cancel_capture_edit();
        controller.window.invoke_cancel_build();
        let result = request.run();
        lock(&shared).finish_compile(Actor::User, &result);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_lighting_enabled());
        assert!(!controller.window.get_capture_editing());
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    }
}
