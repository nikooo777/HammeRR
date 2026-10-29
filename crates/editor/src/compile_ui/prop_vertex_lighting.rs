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
    let mut description = "Bake direct light and up to four diffuse bounces into admitted opaque or default-alpha-test static props. LDR and HDR are baked separately when both are enabled. VHV stores a static style-0 base; supported point/spot styles remain runtime direct lighting in the game. The editor preview shows the stored base. Switched indirect ambient remains unsupported. Choose Direct only through Four diffuse bounces under Indirect light. Reflected light comes from the preceding world, terrain or eligible stationary Shadows Yes root/assembled-child func_brush lighting pass, with eligible static-prop sources when Reflect light from static props is enabled, and needs texture-header or authored material reflectivity from every lit source. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention. Only trimmed %notooltexture 0 or 1 selects that convention; unavailable required providers stop the build. Unsupported props stop the build and identify the affected object.".to_owned();
    if let Some(reason) = lighting["unavailable_reason"].as_str() {
        description.push(' ');
        description.push_str(reason);
    }
    window.set_prop_vertex_lighting_description(description.into());
    let reflection = &settings["static_prop_reflection"];
    window.set_reflect_static_props(settings["reflect_static_props"] == true);
    window.set_reflect_static_props_enabled(
        settings["editable"] == true && reflection["available"] == true,
    );
    let mut description = reflection["description"].as_str().unwrap_or("").to_owned();
    if let Some(reason) = reflection["unavailable_reason"].as_str() {
        description.push(' ');
        description.push_str(reason);
    }
    window.set_reflect_static_props_description(description.into());
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
    window.on_change_reflect_static_props({
        let (window, target, recipes) = (window.as_weak(), access.downgrade(), recipes.clone());
        move |enabled| {
            super::admitted(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = session.update_build_settings(
                    Actor::User,
                    BuildSettingsUpdate {
                        reflect_static_props: Some(enabled),
                        ..Default::default()
                    },
                );
                super::refresh_recipes(&window, &session, &recipes);
                window.set_message(match result {
                    Ok(_) if enabled => {
                        "Reflected light from eligible static props enabled.".into()
                    }
                    Ok(_) => "Reflected light from static props disabled.".into(),
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
    let mut description = format!(
        "\nStatic-prop vertex lighting: {props} props · {models} models · {meshes} hardware meshes · {vertices} vertices\nProp vertex lighting bytes: LDR {ldr_bytes} · HDR {hdr_bytes}\nProp vertex lighting work: {work} · Asset bytes: {asset_bytes}"
    );
    if let (Some(props), Some(triangles), Some(vertices)) = (
        stats["reflected_source_props"].as_u64(),
        stats["reflected_source_triangles"].as_u64(),
        stats["reflected_source_vertices"].as_u64(),
    ) && (props != 0 || job["settings"]["reflect_static_props"] == true)
    {
        description.push_str(&format!(
            "\nReflected static-prop sources: {props} props · {triangles} triangles · {vertices} vertices"
        ));
    }
    description
}
