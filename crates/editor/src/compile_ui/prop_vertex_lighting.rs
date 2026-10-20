use serde_json::Value;
use slint::ComponentHandle;

use super::{Actor, CompileWindow, DocumentAccess, Session, lock};
use crate::session::compilation::BuildSettingsUpdate;

pub(super) fn refresh(window: &CompileWindow, session: &Session) {
    let settings = session.compile_settings();
    let lighting = &settings["prop_vertex_lighting"];
    window.set_bake_static_prop_vertices(settings["bake_static_prop_vertices"] == true);
    window.set_prop_vertex_lighting_enabled(
        settings["editable"] == true && lighting["available"] == true,
    );
    let mut description = "Bake direct light into static props. LDR and HDR are baked separately when both are enabled. Requires Direct only under Indirect light. Unsupported props stop the build and identify the affected object.".to_owned();
    if let Some(reason) = lighting["unavailable_reason"].as_str() {
        description.push(' ');
        description.push_str(reason);
    }
    window.set_prop_vertex_lighting_description(description.into());
}

pub(super) fn bind(
    window: &CompileWindow,
    access: &DocumentAccess,
    recipes: &std::rc::Rc<std::cell::RefCell<super::RecipeForm>>,
) {
    window.on_change_bake_static_prop_vertices({
        let (window, target, recipes) = (window.as_weak(), access.downgrade(), recipes.clone());
        move |enabled| {
            super::admitted(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = session.update_build_settings(
                    Actor::User,
                    BuildSettingsUpdate {
                        bake_static_prop_vertices: Some(enabled),
                        ..Default::default()
                    },
                );
                super::refresh_recipes(&window, &session, &recipes);
                window.set_message(match result {
                    Ok(_) if enabled => "Static-prop vertex lighting enabled. Unsupported requested receivers stop the build; disablevertexlighting=1 explicitly opts out one prop.".into(),
                    Ok(_) => "Static-prop vertex lighting disabled.".into(),
                    Err(error) => error.into(),
                });
            });
        }
    });
}

pub(super) fn details(job: &Value) -> String {
    if job["status"] != "completed" || job["result"]["published"] != true || job["stale"] != false {
        return String::new();
    }
    let stats = &job["result"]["stats"]["prop_vertex_lighting"];
    let values = [
        "props",
        "models",
        "meshes",
        "vertices",
        "ldr_bytes",
        "hdr_bytes",
        "work",
        "asset_bytes",
    ]
    .map(|key| stats[key].as_u64());
    let [
        Some(props),
        Some(models),
        Some(meshes),
        Some(vertices),
        Some(ldr_bytes),
        Some(hdr_bytes),
        Some(work),
        Some(asset_bytes),
    ] = values
    else {
        return String::new();
    };
    format!(
        "\nStatic-prop vertex lighting: {props} props · {models} models · {meshes} hardware meshes · {vertices} vertices\nProp vertex lighting bytes: LDR {ldr_bytes} · HDR {hdr_bytes}\nProp vertex lighting work: {work} · Asset bytes: {asset_bytes}"
    )
}

#[cfg(test)]
mod tests;
