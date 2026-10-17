use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{Actor, CompileWindow, Session, SharedSession, lock};

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
        window.set_capture_summary(
            if state["ready"] == true {
                "Capture runtime ready. Configure capture… shows its folders."
            } else {
                "Capture runtime unavailable. Open Configure capture… for details."
            }
            .into(),
        );
        window.set_capture_details(format!("{readiness}\nCS:S: {engine}\nSteam: {steam}").into());
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
    shared: &SharedSession,
    recipes: &Rc<RefCell<super::RecipeForm>>,
) -> Form {
    let form = Form::default();
    window.on_change_reflections({
        let (window, shared, recipes) = (window.as_weak(), shared.clone(), recipes.clone());
        move |enabled| {
            let mut session = lock(&shared);
            let result = session.update_build_settings(Actor::User, crate::session::compilation::BuildSettingsUpdate {
                generate_reflections: Some(enabled),
                ..Default::default()
            });
            let window = window.unwrap();
            super::refresh_recipes(&window, &session, &recipes);
            window.set_message(match result {
                Ok(_) if enabled => "Reflection generation enabled. The next Compile or Run Map captures and verifies the selected lighting channels before publishing.".into(),
                Ok(_) => "Reflection generation disabled.".into(),
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
                accept_folder(&window, &serial, pending, kind, folder.path());
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
        let (window, form) = (window.as_weak(), form.clone());
        move || {
            let window = window.unwrap();
            form.discard(&window);
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
        assert_eq!(
            controller.window.get_selected_lighting(),
            "Direct LDR + HDR"
        );
        assert!(controller.window.get_generate_reflections());
        controller.window.invoke_change_lighting(0);
        controller.refresh(&lock(&shared));
        assert_eq!(
            controller.window.get_selected_lighting(),
            "Direct LDR + HDR"
        );
        assert!(controller.window.get_generate_reflections());
        assert!(lock(&shared).compile_options().generate_reflections);
        if let Some(engine_root) = std::env::var_os("HAMMERR_UI_CAPTURE_ENGINE_ROOT") {
            let state = lock(&shared)
                .configure_native_capture_runtime(Actor::User, Some(Path::new(&engine_root)), None)
                .unwrap();
            assert_eq!(state["ready"], true);
            controller.refresh(&lock(&shared));
        }
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(
            controller.window.window(),
            "build-hdr-reflection-controls",
        );
        controller.window.invoke_change_lighting(1);
        controller.refresh(&lock(&shared));
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-reflection-controls");
        let runtime = lock(&shared).native_capture_runtime();
        controller.window.invoke_begin_capture_edit();
        assert!(controller.window.get_capture_editing());
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-reflection-runtime");
        controller
            .window
            .set_capture_engine_root("/missing/hammerr-capture-install".into());
        controller
            .window
            .set_capture_steam_root("/missing/hammerr-steam-client".into());
        assert_eq!(lock(&shared).native_capture_runtime(), runtime);
        controller.window.invoke_cancel_capture_edit();
        for close in 0..3 {
            controller.window.show().unwrap();
            controller.refresh(&lock(&shared));
            controller.window.invoke_begin_capture_edit();
            let pending = controller.capture.serial.get();
            controller
                .window
                .set_capture_engine_root("/unapplied/closed-install".into());
            match close {
                0 => controller.window.invoke_close_dialog(),
                1 => controller.window.window().dispatch_event(
                    slint::platform::WindowEvent::KeyPressed {
                        text: slint::platform::Key::Escape.into(),
                    },
                ),
                _ => controller
                    .window
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested),
            }
            assert!(!controller.window.window().is_visible());
            assert!(!controller.window.get_capture_editing());
            assert!(controller.capture.baseline.borrow().is_none());
            assert_ne!(controller.capture.serial.get(), pending);
            assert!(controller.window.get_capture_engine_root().is_empty());
            controller.window.show().unwrap();
            controller.refresh(&lock(&shared));
            controller.window.invoke_begin_capture_edit();
            let reviewed = controller.window.get_capture_engine_root();
            assert!(!accept_folder(
                &controller.window,
                &controller.capture.serial,
                pending,
                0,
                Path::new("/stale/chooser-result")
            ));
            assert_eq!(controller.window.get_capture_engine_root(), reviewed);
            assert_eq!(lock(&shared).native_capture_runtime(), runtime);
            controller.window.invoke_cancel_capture_edit();
        }
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
        let active_runtime = lock(&shared).native_capture_runtime();
        assert_eq!(active_runtime["editable"], false);
        let mut expected_runtime = runtime.clone();
        expected_runtime["editable"] = false.into();
        assert_eq!(active_runtime, expected_runtime);
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
