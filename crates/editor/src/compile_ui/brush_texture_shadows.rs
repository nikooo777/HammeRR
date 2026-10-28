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
