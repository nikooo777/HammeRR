use serde_json::Value;
use slint::ComponentHandle;

use super::{Actor, CompileWindow, DocumentAccess, Session, lock};
use crate::session::compilation::BuildSettingsUpdate;

pub(super) fn refresh(window: &CompileWindow, session: &Session) {
    let settings = session.compile_settings();
    window.set_pack_static_prop_assets(session.compile_options().pack_static_prop_assets);
    window.set_static_prop_packing_description(
        settings["static_prop_packing_description"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
}

fn change(window: &CompileWindow, session: &mut Session, enabled: bool) {
    let result = session.update_build_settings(
        Actor::User,
        BuildSettingsUpdate {
            pack_static_prop_assets: Some(enabled),
            ..Default::default()
        },
    );
    refresh(window, session);
    window.set_message(match result {
        Ok(_) if enabled => "Static-prop asset packing enabled. The next Compile or Run Map packs original supported static-prop assets; missing or unsupported dependencies stop the build.".into(),
        Ok(_) => "Static-prop asset packing disabled.".into(),
        Err(error) => error.into(),
    });
}

pub(super) fn bind(
    window: &CompileWindow,
    access: &DocumentAccess,
    recipes: &std::rc::Rc<std::cell::RefCell<super::RecipeForm>>,
) {
    window.on_change_pack_static_prop_assets({
        let (window, target, recipes) = (window.as_weak(), access.downgrade(), recipes.clone());
        move |enabled| {
            super::admitted(&window, &target, |window, shared| {
                let mut session = lock(shared);
                change(&window, &mut session, enabled);
                super::refresh_recipes(&window, &session, &recipes);
            });
        }
    });
}

pub(super) fn details(job: &Value) -> String {
    if job["status"] != "completed" || job["result"]["published"] != true || job["stale"] != false {
        return String::new();
    }
    let stats = &job["result"]["stats"]["static_prop_assets"];
    let (Some(models), Some(files), Some(bytes)) = (
        stats["models"].as_u64(),
        stats["files"].as_u64(),
        stats["asset_bytes"].as_u64(),
    ) else {
        return String::new();
    };
    format!("\nStatic-prop assets: {models} models · {files} files · {bytes} bytes")
}
