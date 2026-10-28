use slint::ComponentHandle;

use super::{Actor, CompileWindow, DocumentAccess, Session, lock};
use crate::session::compilation::BuildSettingsUpdate;

pub(super) fn refresh(window: &CompileWindow, session: &Session) {
    let settings = session.compile_settings();
    let shadowing = &settings["brush_texture_shadowing"];
    window.set_brush_texture_shadows(settings["brush_texture_shadows"] == true);
    window.set_brush_texture_shadows_enabled(
        settings["editable"] == true && shadowing["available"] == true,
    );
    let mut description = shadowing["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if let Some(reason) = shadowing["unavailable_reason"].as_str() {
        description.push(' ');
        description.push_str(reason);
    }
    window.set_brush_texture_shadows_description(description.into());
}

pub(super) fn bind(
    window: &CompileWindow,
    access: &DocumentAccess,
    recipes: &std::rc::Rc<std::cell::RefCell<super::RecipeForm>>,
) {
    window.on_change_brush_texture_shadows({
        let (window, target, recipes) = (window.as_weak(), access.downgrade(), recipes.clone());
        move |enabled| {
            super::admitted(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = session.update_build_settings(
                    Actor::User,
                    BuildSettingsUpdate {
                        brush_texture_shadows: Some(enabled),
                        ..Default::default()
                    },
                );
                super::refresh_recipes(&window, &session, &recipes);
                window.set_message(match result {
                    Ok(_) if enabled => "Brush texture shadows enabled. Unsupported requested materials stop the build.".into(),
                    Ok(_) => "Brush texture shadows disabled.".into(),
                    Err(error) => error.into(),
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::compile_ui::Controller;
    use crate::ui::tests::{assert_dialog_button_visible, assert_ui_renders};

    #[test]
    fn brush_texture_shadows_use_real_controls_recipes_and_frozen_jobs() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (directory, game) = crate::test_assets::game();
        let path = directory.path().join("brush-shadows.vmf");
        std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
        let mut session = Session::new(Ok(game));
        session.configure_build_recipes(Ok(directory.path().join("preferences")));
        session.open(Actor::User, &path, false).unwrap();
        let source = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        assert!(!controller.window.get_brush_texture_shadows());
        assert!(!controller.window.get_brush_texture_shadows_enabled());
        controller.window.invoke_change_brush_texture_shadows(true);
        assert!(!lock(&shared).compile_options().brush_texture_shadows);
        assert!(
            controller
                .window
                .get_message()
                .contains("requires Direct LDR")
        );
        controller.window.invoke_change_lighting(1);
        assert_dialog_button_visible(&controller.window, "Brush texture shadows");
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &controller.window,
            "Brush texture shadows",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(lock(&shared).compile_options().brush_texture_shadows);
        controller.window.set_recipe_name("Brush cutouts".into());
        controller.window.invoke_save_recipe();
        controller.window.invoke_change_brush_texture_shadows(false);
        controller.window.invoke_apply_recipe();
        assert!(controller.window.get_brush_texture_shadows());
        assert_ui_renders(controller.window.window(), "build-brush-texture-shadows");
        let request = lock(&shared).begin_compile(Actor::User, None).unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_brush_texture_shadows_enabled());
        controller.window.invoke_change_brush_texture_shadows(false);
        assert!(controller.window.get_brush_texture_shadows());
        assert_eq!(
            lock(&shared).compile_status()["job"]["settings"]["brush_texture_shadows"],
            true
        );
        controller.window.invoke_cancel_build();
        let result = request.run();
        assert!(result.is_err());
        lock(&shared).finish_compile(Actor::User, &result);
        assert_eq!(lock(&shared).document().vmf(), &source);
    }
}
