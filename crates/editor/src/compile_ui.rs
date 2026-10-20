use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::ComponentHandle;

use crate::CompileWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session};

#[cfg(test)]
mod admission_tests;
mod baked_lightmaps;
mod capture;
#[cfg(test)]
mod instance_dependency_tests;
mod prop_vertex_lighting;
#[cfg(test)]
mod recipe_authority_tests;
#[cfg(test)]
mod recipe_reload_tests;
mod sky_packing;

pub struct Controller {
    pub window: CompileWindow,
    token: Rc<Cell<Option<u64>>>,
    recipes: Rc<RefCell<RecipeForm>>,
    capture: capture::Form,
    baked_lightmaps: baked_lightmaps::Form,
}

#[derive(Default)]
struct RecipeForm {
    revision: u64,
    registry_revision: u64,
    selected: Option<String>,
    rows: Vec<String>,
}

#[derive(Clone, Copy)]
enum RecipeAction {
    Apply,
    Save,
    Replace,
    Rename,
    Delete,
    Reload,
}

fn admitted(
    window: &slint::Weak<CompileWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(CompileWindow, &SharedSession),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|target| {
        target.with_shared(|shared| {
            body(window.clone_strong(), shared);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn hdr_lighting_details(stats: &serde_json::Value) -> String {
    let hdr = &stats["hdr"];
    if !hdr.is_object() {
        return String::new();
    }
    format!(
        "\nHDR baked lighting: {} faces · {} samples · {} light sources · {} bytes\nHDR light styles: {} · {} styled faces · {} lightmaps\nHDR brush entity lightmaps: {} models · {} faces\nHDR normal-map lighting: {} faces · {} directional maps\nHDR indirect light: {} · {} rays · {} samples lit · {} work units\nHDR brush entity bounce: {} faces · {} samples lit\nHDR model ambient: {} probes · {} bytes\nHDR native map data: {}\nLighting work: LDR {} · HDR {} · Shared total {}",
        hdr["lighting_faces"].as_u64().unwrap_or_default(),
        hdr["lightmap_samples"].as_u64().unwrap_or_default(),
        hdr["world_lights"].as_u64().unwrap_or_default(),
        hdr["lighting_bytes"].as_u64().unwrap_or_default(),
        hdr["light_styles"].as_u64().unwrap_or_default(),
        hdr["styled_faces"].as_u64().unwrap_or_default(),
        hdr["light_style_maps"].as_u64().unwrap_or_default(),
        hdr["lit_brush_models"].as_u64().unwrap_or_default(),
        hdr["model_lighting_faces"].as_u64().unwrap_or_default(),
        hdr["bump_faces"].as_u64().unwrap_or_default(),
        hdr["bump_lightmaps"].as_u64().unwrap_or_default(),
        compiler::bounce_label(hdr["bounces"].as_u64().unwrap_or_default() as u8),
        hdr["bounce_rays"].as_u64().unwrap_or_default(),
        hdr["bounce_samples"].as_u64().unwrap_or_default(),
        hdr["bounce_work"].as_u64().unwrap_or_default(),
        hdr["model_bounce_faces"].as_u64().unwrap_or_default(),
        hdr["model_bounce_samples"].as_u64().unwrap_or_default(),
        hdr["ambient_samples"].as_u64().unwrap_or_default(),
        hdr["ambient_bytes"].as_u64().unwrap_or_default(),
        if hdr["map_data_present"] == true {
            "HDR data ready"
        } else {
            "HDR unavailable: incomplete map data"
        },
        stats["lighting_work"].as_u64().unwrap_or_default(),
        hdr["lighting_work"].as_u64().unwrap_or_default(),
        stats["lighting_total_work"].as_u64().unwrap_or_default(),
    )
}

fn visibility_details(stats: &serde_json::Value) -> String {
    let Some(count) = stats["clusters"].as_u64() else {
        return String::new();
    };
    let solved = stats["visibility_solved_clusters"]
        .as_u64()
        .unwrap_or(count);
    let portals = stats["visibility_portals"].as_u64().unwrap_or_default();
    let removed = stats["visibility_removed_pairs"]
        .as_u64()
        .unwrap_or_default();
    let workers = stats["visibility_workers"].as_u64().unwrap_or(1);
    let mut text = format!(
        "\nVisibility: {count} emitted clusters · {solved} solved leaf clusters · {portals} leaf portals · Workers: {workers} · {removed} leaf-cluster pairs removed before authored grouping"
    );
    if stats["viscluster_helpers"].as_u64().unwrap_or_default() > 0 {
        text += &format!(
            "\nAuthored visclusters: {} helpers · {} effective groups · {} grouped leaves · Visibility may expand; solver cost is unchanged",
            stats["viscluster_helpers"].as_u64().unwrap_or_default(),
            stats["viscluster_groups"].as_u64().unwrap_or_default(),
            stats["viscluster_grouped_leaves"]
                .as_u64()
                .unwrap_or_default()
        );
    }
    text
}

fn refresh_current_settings(window: &CompileWindow, session: &Session) {
    let settings = session.compile_settings();
    sky_packing::refresh(window, session);
    prop_vertex_lighting::refresh(window, session);
    window.set_generate_reflections(settings["generate_reflections"] == true);
    window.set_reflection_description(
        settings["reflection_description"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    let choices = settings["choices"].as_array().cloned().unwrap_or_default();
    let choice = choices
        .iter()
        .position(|choice| choice["name"] == settings["lighting"])
        .unwrap_or_default() as i32;
    window.set_lighting_choice(choice);
    window.set_selected_lighting(
        choices[choice as usize]["label"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    window.set_lighting_enabled(settings["editable"].as_bool().unwrap_or(false));
    window.set_lighting_description(
        settings["choices"][choice as usize]["description"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    let physics_choices = settings["physics_choices"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let physics_choice = physics_choices
        .iter()
        .position(|choice| choice["name"] == settings["physics"])
        .unwrap_or_default();
    window.set_selected_physics(
        physics_choices[physics_choice]["label"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    window.set_physics_description(
        physics_choices[physics_choice]["description"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    let sky_choice = settings["sky_choices"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|choice| choice["name"] == settings["sky_quality"]);
    window.set_selected_sky_quality(
        sky_choice
            .and_then(|choice| choice["label"].as_str())
            .unwrap_or_default()
            .into(),
    );
    window.set_sky_enabled(
        settings["editable"].as_bool().unwrap_or(false)
            && compiler::LightingMode::parse(settings["lighting"].as_str().unwrap_or_default())
                .is_ok_and(compiler::LightingMode::performs_lighting),
    );
    window.set_smoothing_angle(settings["smoothing_angle"].as_u64().unwrap_or_default() as i32);
    let bounce_choice = settings["bounce_choices"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|choice| choice["bounces"] == settings["bounces"]);
    window.set_selected_bounces(
        bounce_choice
            .and_then(|choice| choice["label"].as_str())
            .unwrap_or_default()
            .into(),
    );
    let visibility_choice = settings["visibility_choices"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|choice| choice["name"] == settings["visibility"]);
    window.set_selected_visibility(
        visibility_choice
            .and_then(|choice| choice["label"].as_str())
            .unwrap_or_default()
            .into(),
    );
    window.set_visibility_description(
        visibility_choice
            .and_then(|choice| choice["description"].as_str())
            .unwrap_or_default()
            .into(),
    );
}

fn refresh_recipes(window: &CompileWindow, session: &Session, form: &RefCell<RecipeForm>) {
    refresh_current_settings(window, session);
    let (state, registry_revision) = session.build_recipes_observation();
    let mut form = form.borrow_mut();
    let revision = state["revision"].as_u64().unwrap_or_default();
    if form.registry_revision != registry_revision || form.revision == 0 {
        form.registry_revision = registry_revision;
        let entries = state["recipes"].as_array().cloned().unwrap_or_default();
        form.rows = entries
            .iter()
            .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
            .collect();
        window.set_recipe_names(
            Rc::new(slint::VecModel::from(
                entries
                    .iter()
                    .map(|entry| {
                        slint::SharedString::from(entry["name"].as_str().unwrap_or_default())
                    })
                    .collect::<Vec<_>>(),
            ))
            .into(),
        );
        let selected = entries
            .iter()
            .find(|entry| entry["id"].as_str() == form.selected.as_deref())
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry["id"] == state["active"]["id"])
            })
            .or_else(|| entries.first());
        if let Some(selected) = selected {
            form.selected = selected["id"].as_str().map(str::to_owned);
            let name = selected["name"].as_str().unwrap_or_default();
            window.set_selected_recipe(name.into());
            window.set_recipe_name(if selected["builtin"] == true {
                "".into()
            } else {
                name.into()
            });
            window.set_custom_recipe_selected(selected["builtin"] == false);
            window.set_recipe_description(
                selected["description"].as_str().unwrap_or_default().into(),
            );
        }
    }
    form.revision = revision;
    window.set_custom_recipes_enabled(state["custom_editable"].as_bool().unwrap_or(false));
    window.set_recipes_reload_enabled(state["editable"] == true && state["path"].is_string());
    let active = state["active"]["name"]
        .as_str()
        .unwrap_or("Custom settings");
    let storage = match state["persistence"].as_str() {
        Some("persistent") => "Custom recipes are saved on this computer.",
        Some("blocked") => "Custom recipe storage is blocked.",
        _ => "Custom recipes last for this session.",
    };
    let error = state["error"]
        .as_str()
        .map(|error| format!("\n{error}"))
        .unwrap_or_default();
    window.set_recipe_status(format!("Current recipe: {active}. {storage}{error}").into());
}

fn choose_recipe(
    window: &CompileWindow,
    session: &Session,
    form: &RefCell<RecipeForm>,
    index: i32,
) {
    let state = session.build_recipes();
    if state["revision"].as_u64() != Some(form.borrow().revision) {
        refresh_recipes(window, session, form);
        window
            .set_message("Build recipes changed. Choose a recipe from the refreshed list.".into());
        return;
    }
    let mut form = form.borrow_mut();
    let Some(id) = usize::try_from(index)
        .ok()
        .and_then(|index| form.rows.get(index))
    else {
        return;
    };
    let Some(entry) = state["recipes"].as_array().and_then(|entries| {
        entries
            .iter()
            .find(|entry| entry["id"].as_str() == Some(id))
    }) else {
        return;
    };
    form.selected = entry["id"].as_str().map(str::to_owned);
    form.revision = state["revision"].as_u64().unwrap_or_default();
    let name = entry["name"].as_str().unwrap_or_default();
    window.set_selected_recipe(name.into());
    window.set_recipe_name(if entry["builtin"] == true {
        "".into()
    } else {
        name.into()
    });
    window.set_custom_recipe_selected(entry["builtin"] == false);
    window.set_recipe_description(entry["description"].as_str().unwrap_or_default().into());
    window.set_message("Choose Apply recipe to use its settings, or save the current settings as a new named recipe.".into());
}

fn recipe_action(
    window: &CompileWindow,
    session: &mut Session,
    form: &RefCell<RecipeForm>,
    action: RecipeAction,
) {
    let (revision, selected) = {
        let form = form.borrow();
        (Some(form.revision), form.selected.clone())
    };
    let id = selected.as_deref().unwrap_or_default();
    let name = window.get_recipe_name();
    let result = match action {
        RecipeAction::Apply => session.apply_build_recipe(Actor::User, id, revision),
        RecipeAction::Save => session.save_build_recipe(Actor::User, &name, None, revision),
        RecipeAction::Replace => session.save_build_recipe(Actor::User, &name, Some(id), revision),
        RecipeAction::Rename => session.rename_build_recipe(Actor::User, id, &name, revision),
        RecipeAction::Delete => session.delete_build_recipe(Actor::User, id, revision),
        RecipeAction::Reload => session.reload_build_recipes(Actor::User, revision),
    };
    match result {
        Ok(state) => {
            let mut form = form.borrow_mut();
            form.revision = 0;
            if matches!(
                action,
                RecipeAction::Apply | RecipeAction::Save | RecipeAction::Replace
            ) {
                form.selected = state["active"]["id"].as_str().map(str::to_owned);
            }
            window.set_message(
                match action {
                    RecipeAction::Apply => {
                        "Recipe applied. Compile or Run Map will use these settings."
                    }
                    RecipeAction::Save | RecipeAction::Replace => {
                        "Recipe saved from the current settings."
                    }
                    RecipeAction::Rename => {
                        "Recipe renamed. Existing jobs retain their original name."
                    }
                    RecipeAction::Delete => "Recipe deleted. Current settings are unchanged.",
                    RecipeAction::Reload => {
                        "Recipes reloaded. Current build settings are unchanged."
                    }
                }
                .into(),
            );
        }
        Err(error) => window.set_message(error.into()),
    }
    refresh_recipes(window, session, form);
}

impl Controller {
    #[cfg(test)]
    pub(crate) fn baked_lightmaps_window(&self) -> &crate::BakedLightmapsWindow {
        self.baked_lightmaps.window()
    }

    pub(crate) fn hide_windows(&self, errors: &mut Vec<String>) {
        self.baked_lightmaps.hide_window(errors);
        crate::ui::hide_window("Compile", self.window.window(), errors);
    }

    #[cfg(test)]
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(shared, DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(
        shared: &SharedSession,
        access: DocumentAccess,
    ) -> Result<Self, slint::PlatformError> {
        let window = CompileWindow::new()?;
        let token = Rc::new(Cell::new(None));
        let recipes = Rc::new(RefCell::new(RecipeForm::default()));
        let capture = capture::bind(&window, &access, &recipes);
        sky_packing::bind(&window, &access, &recipes);
        prop_vertex_lighting::bind(&window, &access, &recipes);
        let baked_lightmaps = baked_lightmaps::bind(&window, &access)?;
        window.on_choose_recipe({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |index| {
                admitted(&window, &target, |window, shared| {
                    choose_recipe(&window, &lock(shared), &recipes, index)
                });
            }
        });
        window.on_apply_recipe({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    recipe_action(&window, &mut lock(shared), &recipes, RecipeAction::Apply)
                });
            }
        });
        window.on_save_recipe({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    recipe_action(&window, &mut lock(shared), &recipes, RecipeAction::Save)
                });
            }
        });
        window.on_replace_recipe({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    recipe_action(&window, &mut lock(shared), &recipes, RecipeAction::Replace)
                });
            }
        });
        window.on_rename_recipe({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    recipe_action(&window, &mut lock(shared), &recipes, RecipeAction::Rename)
                });
            }
        });
        window.on_delete_recipe({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    recipe_action(&window, &mut lock(shared), &recipes, RecipeAction::Delete)
                });
            }
        });
        window.on_cancel_recipe_edit({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    let session = lock(shared);
                    *recipes.borrow_mut() = RecipeForm::default();
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(
                        "Recipe edit cancelled. Current build settings are unchanged.".into(),
                    );
                });
            }
        });
        window.on_reload_recipes({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move || {
                admitted(&window, &target, |window, shared| {
                    recipe_action(&window, &mut lock(shared), &recipes, RecipeAction::Reload);
                });
            }
        });
        let settings = lock(shared).compile_settings();
        let labels = settings["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_lighting_modes(Rc::new(slint::VecModel::from(labels)).into());
        let physics_labels = settings["physics_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_physics_modes(Rc::new(slint::VecModel::from(physics_labels)).into());
        let sky_labels = settings["sky_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_sky_qualities(Rc::new(slint::VecModel::from(sky_labels)).into());
        let visibility_labels = settings["visibility_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_visibility_modes(Rc::new(slint::VecModel::from(visibility_labels)).into());
        let bounce_labels = settings["bounce_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_bounce_modes(Rc::new(slint::VecModel::from(bounce_labels)).into());
        window.on_change_smoothing_angle({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |angle| {
                admitted(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    let result = u8::try_from(angle)
                        .map_err(|_| "Choose an angle from 0 to 90 degrees".to_string())
                        .and_then(|angle| {
                            session.update_build_settings(
                                Actor::User,
                                crate::session::compilation::BuildSettingsUpdate {
                                    smoothing_angle: Some(angle),
                                    ..Default::default()
                                },
                            )
                        });
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(match result {
                        Ok(_) => {
                            "Brush smoothing angle updated. The next build will use this angle."
                                .into()
                        }
                        Err(error) => error.into(),
                    });
                });
            }
        });
        window.on_change_bounces({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |choice| {
                admitted(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    let settings = session.compile_settings();
                    let result = settings["bounce_choices"][choice as usize]["bounces"]
                        .as_u64()
                        .and_then(|count| u8::try_from(count).ok())
                        .ok_or_else(|| "Choose a supported indirect lighting mode".to_string())
                        .and_then(|count| {
                            session.update_compile_options(
                                Actor::User,
                                None,
                                None,
                                None,
                                None,
                                Some(count),
                            )
                        });
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(match result {
                        Ok(_) => {
                            "Indirect light updated. The next build will use this setting.".into()
                        }
                        Err(error) => error.into(),
                    });
                });
            }
        });
        window.on_change_visibility({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |choice| {
                admitted(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    let settings = session.compile_settings();
                    let result = settings["visibility_choices"][choice as usize]["name"]
                        .as_str()
                        .ok_or_else(|| "Choose a supported visibility quality".to_string())
                        .and_then(|mode| {
                            session.update_compile_options(
                                Actor::User,
                                None,
                                None,
                                None,
                                Some(mode),
                                None,
                            )
                        });
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(match result {
                        Ok(_) => {
                            "Visibility quality updated. The next build will use this quality."
                                .into()
                        }
                        Err(error) => error.into(),
                    });
                });
            }
        });
        window.on_change_sky_quality({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |choice| {
                admitted(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    let settings = session.compile_settings();
                    let result = settings["sky_choices"][choice as usize]["name"]
                        .as_str()
                        .ok_or_else(|| "Choose a supported sky quality".to_string())
                        .and_then(|quality| {
                            session.update_compile_settings_quality(
                                Actor::User,
                                None,
                                None,
                                Some(quality),
                            )
                        });
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(match result {
                        Ok(_) => {
                            "Sky quality updated. The next build will use this quality.".into()
                        }
                        Err(error) => error.into(),
                    });
                });
            }
        });
        window.on_change_physics({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |choice| {
                admitted(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    let settings = session.compile_settings();
                    let result = settings["physics_choices"][choice as usize]["name"]
                        .as_str()
                        .ok_or_else(|| "Choose a supported physics mode".to_string())
                        .and_then(|mode| {
                            session.update_compile_settings(Actor::User, None, Some(mode))
                        });
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(match result {
                        Ok(_) => "Physics mode updated. The next build will use this mode.".into(),
                        Err(error) => error.into(),
                    });
                });
            }
        });
        window.on_change_lighting({
            let (target, window, recipes) = (access.downgrade(), window.as_weak(), recipes.clone());
            move |choice| {
                admitted(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    let settings = session.compile_settings();
                    let result = settings["choices"][choice as usize]["name"]
                        .as_str()
                        .ok_or_else(|| "Choose a supported lighting mode".to_string())
                        .and_then(|mode| session.set_compile_settings(Actor::User, mode));
                    refresh_recipes(&window, &session, &recipes);
                    window.set_message(match result {
                        Ok(_) => {
                            "Build lighting updated. The next build will use this mode.".into()
                        }
                        Err(error) => error.into(),
                    });
                });
            }
        });
        window.on_play_build({
            let (target, window) = (access.downgrade(), window.as_weak());
            move || {
                admitted(&window, &target, |window, shared| {
                let result = crate::runtime::start_launch(shared, Actor::User);
                window.set_message(match result {
                    Ok(()) => "Launching the last successful build with -insecure. Open Game Log for engine output.".into(),
                    Err(error) => error.into(),
                });
                });
            }
        });
        window.on_cancel_build({
            let (continuation, window, token) = (access.downgrade(), window.as_weak(), token.clone());
            move || {
                let Some(window) = window.upgrade() else { return };
                if let Some(token) = token.get() {
                    let result = continuation.upgrade().and_then(|access| access.cancel_compile(Actor::User, token));
                    window.set_message(match result {
                        Ok(_) => {
                            "Cancellation requested. Queued jobs retire immediately; running jobs stop after cleanup.".into()
                        }
                        Err(error) => error.into(),
                    });
                }
            }
        });
        window.on_close_dialog({
            let (window, capture) = (window.as_weak(), capture.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                capture.discard(&window);
                let _ = window.hide();
            }
        });
        window.window().on_close_requested({
            let (window, capture) = (window.as_weak(), capture.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    capture.discard(&window);
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            token,
            recipes,
            capture,
            baked_lightmaps,
        })
    }

    pub fn refresh(&self, session: &Session) {
        refresh_recipes(&self.window, session, &self.recipes);
        self.capture.refresh(&self.window, session);
        self.baked_lightmaps.refresh(session);
        self.token.set(session.compile_token());
        self.window.set_cancellable(session.compile_cancellable());
        let status = session.compile_status();
        let launch = &status["launch"];
        self.window
            .set_launch_ready(launch["ready"].as_bool().unwrap_or(false));
        self.window.set_launch_details(
            match launch["bsp"].as_str() {
                Some(path) => format!("Ready to play without rebuilding: {path}"),
                None => format!(
                    "Play unavailable: {}",
                    launch["reason"].as_str().unwrap_or_default()
                ),
            }
            .into(),
        );
        let job = &status["job"];
        if job.is_null() {
            self.window
                .set_summary("No build has run in this session.".into());
            self.window.set_details(
                "Choose settings above, then Compile or Run Map from the toolbar.".into(),
            );
            self.window.set_diagnostics("".into());
            return;
        }
        self.window.set_summary(session.compile_label().into());
        let queue_details = match job["queue"]["position"].as_u64() {
            Some(position) => format!(
                "\nQueue position: {position} · {} / {} jobs admitted",
                job["queue"]["admitted"].as_u64().unwrap_or_default(),
                job["queue"]["limit"].as_u64().unwrap_or_default()
            ),
            None if job["queue"].is_object() => format!(
                "\nQueue wait: {:.2}s",
                job["queue"]["waiting_ms"].as_u64().unwrap_or_default() as f64 / 1000.0
            ),
            None => String::new(),
        };
        let result = &job["result"];
        let ldr_prefix = if result["stats"]["hdr"].is_object() {
            "LDR "
        } else {
            ""
        };
        let hdr = hdr_lighting_details(&result["stats"]);
        let packed_sky = if job["status"] == "completed" && result["published"] == true {
            sky_packing::details(&result["stats"])
        } else {
            String::new()
        };
        let prop_vertex_lighting = prop_vertex_lighting::details(job);
        let samples = result["stats"]["cubemap_samples"]
            .as_u64()
            .map(|count| format!("\nCubemap samples: {count}"))
            .unwrap_or_default();
        let instances = result["stats"]["instances"]
            .as_object()
            .map(|_| {
                let stats = &result["stats"]["instances"];
                let mut summary = format!(
                    "\nInstances: {} · Files: {} · World brushes: {} · Detail brushes: {}",
                    stats["instances"].as_u64().unwrap_or_default(),
                    stats["files"].as_u64().unwrap_or_default(),
                    stats["world_brushes"].as_u64().unwrap_or_default(),
                    stats["detail_brushes"].as_u64().unwrap_or_default()
                );
                for dependency in stats["dependencies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(32)
                {
                    summary.push_str(&format!(
                        "\nInstance file: {} · {} bytes · SHA256 {}",
                        dependency["path"].as_str().unwrap_or_default(),
                        dependency["bytes"].as_u64().unwrap_or_default(),
                        dependency["sha256"].as_str().unwrap_or_default()
                    ));
                }
                summary
            })
            .unwrap_or_default();
        let reflections = if job["settings"]["generate_reflections"] == true {
            format!(
                "\n{} reflection capture: {} / {} samples",
                if job["settings"]["lighting"] == "direct_ldr_hdr" {
                    "LDR + HDR"
                } else {
                    "LDR"
                },
                job["reflections"]["samples_complete"]
                    .as_u64()
                    .unwrap_or_default(),
                job["reflections"]["samples_total"]
                    .as_u64()
                    .unwrap_or_default()
            )
        } else {
            String::new()
        };
        let props = result["stats"]["static_props"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let models = result["stats"]["static_prop_models"]
                    .as_u64()
                    .unwrap_or_default();
                let leaves = result["stats"]["static_prop_leaf_refs"]
                    .as_u64()
                    .unwrap_or_default();
                format!(
                    "\nStatic props: {count} · Model assets: {models} · Leaf references: {leaves}\nProp mesh shadows: {} props · {} triangles · {} disabled · {} alpha-test triangles · {} alpha textures",
                    result["stats"]["prop_shadow_props"].as_u64().unwrap_or_default(),
                    result["stats"]["prop_shadow_triangles"].as_u64().unwrap_or_default(),
                    result["stats"]["prop_shadow_disabled"].as_u64().unwrap_or_default(),
                    result["stats"]["prop_shadow_alpha_triangles"].as_u64().unwrap_or_default(),
                    result["stats"]["prop_shadow_alpha_textures"].as_u64().unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let details = result["stats"]["detail_props"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let models = result["stats"]["detail_prop_models"]
                    .as_u64()
                    .unwrap_or_default();
                let sprites = result["stats"]["detail_prop_sprites"]
                    .as_u64()
                    .unwrap_or_default();
                let lighting = match result["stats"]["lighting_mode"].as_str() {
                    Some("direct_ldr") => "Baked LDR lighting",
                    Some("direct_ldr_hdr") => "Baked HDR shared lighting",
                    _ => "Unbaked white lighting",
                };
                format!("\nDetail props: {count} · Model assets: {models} · Sprite entries: {sprites} · {lighting} · No collision")
            })
            .unwrap_or_default();
        let visibility = visibility_details(&result["stats"]);
        let bumps = result["stats"]["bump_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\n{ldr_prefix}Normal-map lighting: {count} faces · {} directional maps",
                    result["stats"]["bump_lightmaps"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let model_maps = result["stats"]["lit_brush_models"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\n{ldr_prefix}Brush entity lightmaps: {count} models · {} faces",
                    result["stats"]["model_lighting_faces"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let styles = result["stats"]["light_styles"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\n{ldr_prefix}Light styles: {count} · {} styled faces · {} lightmaps · {} switched groups",
                    result["stats"]["styled_faces"].as_u64().unwrap_or_default(),
                    result["stats"]["light_style_maps"]
                        .as_u64()
                        .unwrap_or_default(),
                    result["stats"]["switchable_light_groups"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let smoothing = result["stats"]["smoothed_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nBrush smoothing: {count} faces · {} corners",
                    result["stats"]["smoothed_corners"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let model_bounce = result["stats"]["model_bounce_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\n{ldr_prefix}Brush entity bounce: {count} faces · {} samples lit",
                    result["stats"]["model_bounce_samples"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let bounce = result["stats"]["bounces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\n{ldr_prefix}Indirect light: {} · {} rays · {} samples lit{model_bounce}",
                    compiler::bounce_label(count as u8),
                    result["stats"]["bounce_rays"].as_u64().unwrap_or_default(),
                    result["stats"]["bounce_samples"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let hints = result["stats"]["hint_surfaces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let splits = result["stats"]["hint_splits"].as_u64().unwrap_or_default();
                format!(
                    "\nHint surfaces: {count} · Added splits: {splits} · Initial priority: {}",
                    result["stats"]["hint_priority_splits"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let terrain = result["stats"]["displacements"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let vertices = result["stats"]["displacement_vertices"]
                    .as_u64()
                    .unwrap_or_default();
                let triangles = result["stats"]["displacement_triangles"]
                    .as_u64()
                    .unwrap_or_default();
                let neighbors = result["stats"]["displacement_neighbor_pairs"].as_u64().unwrap_or_default();
                let corners=result["stats"]["displacement_corner_neighbor_pairs"].as_u64().unwrap_or_default();
                let mixed=result["stats"]["displacement_mixed_power_pairs"].as_u64().unwrap_or_default();
                let halves=result["stats"]["displacement_half_edge_pairs"].as_u64().unwrap_or_default();
                let disabled=result["stats"]["displacement_lod_disabled_vertices"].as_u64().unwrap_or_default();
                let snapped=result["stats"]["displacement_lod_snapped_vertices"].as_u64().unwrap_or_default();
                let smoothing=result["stats"]["displacement_smoothing_overrides"].as_u64().unwrap_or_default();
                format!(
                    "\nTerrain surfaces: {count} · Vertices: {vertices} · Triangles: {triangles} · Neighbor pairs: {neighbors} · Corner pairs: {corners}\nMixed-power pairs: {mixed} · Half-edge pairs: {halves} · LOD disabled vertices: {disabled} · Snapped vertices: {snapped}\nBorder smoothing: {smoothing} authored surfaces"
                )
            })
            .unwrap_or_default();
        let areas = result["stats"]["areaportals"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let areas = result["stats"]["areas"].as_u64().unwrap_or_default();
                let windows = result["stats"]["areaportal_windows"].as_u64().unwrap_or_default();
                let vertices = result["stats"]["areaportal_vertices"]
                    .as_u64()
                    .unwrap_or_default();
                format!(
                    "\nVisibility areas: {areas} · Areaportals: {count} · Window portals: {windows} · Clip vertices: {vertices}"
                )
            })
            .unwrap_or_default();
        let occluders = result["stats"]["occluders"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let polygons = result["stats"]["occluder_polygons"]
                    .as_u64()
                    .unwrap_or_default();
                let vertices = result["stats"]["occluder_vertex_refs"]
                    .as_u64()
                    .unwrap_or_default();
                format!(
                    "\nOccluders: {count} · Polygons: {polygons} · Vertex references: {vertices}"
                )
            })
            .unwrap_or_default();
        let overlays = result["stats"]["overlays"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let faces = result["stats"]["overlay_face_refs"]
                    .as_u64()
                    .unwrap_or_default();
                format!("\nOverlays: {count} · Surface references: {faces}")
            })
            .unwrap_or_default();
        let water = result["stats"]["water_regions"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let leaves = result["stats"]["water_leaves"].as_u64().unwrap_or_default();
                let region_label = if count == 1 { "region" } else { "regions" };
                let leaf_label = if leaves == 1 { "leaf" } else { "leaves" };
                format!("\nWater: {count} {region_label} · {leaves} submerged {leaf_label}")
            })
            .unwrap_or_default();
        let sky = result["stats"]["sky_clusters"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let label = if count == 1 { "cluster" } else { "clusters" };
                format!("\nSky visible from: {count} {label}")
            })
            .unwrap_or_default();
        let lighting = result["stats"]["lighting_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let samples = result["stats"]["lightmap_samples"]
                    .as_u64()
                    .unwrap_or_default();
                let lights = result["stats"]["world_lights"].as_u64().unwrap_or_default();
                let light_label = if lights == 1 {
                    "light source"
                } else {
                    "light sources"
                };
                format!(
                    "\n{ldr_prefix}Baked lighting: {count} faces · {samples} samples · {lights} {light_label} · {} bytes\n{ldr_prefix}Light culling: {} evaluated · {} hidden · {} beyond range\n{ldr_prefix}Model ambient: {} probes · {} bytes",
                    result["stats"]["lighting_bytes"].as_u64().unwrap_or_default(),
                    result["stats"]["light_evaluations"].as_u64().unwrap_or_default(),
                    result["stats"]["light_pvs_skipped"].as_u64().unwrap_or_default(),
                    result["stats"]["light_range_skipped"].as_u64().unwrap_or_default(),
                    result["stats"]["ambient_samples"].as_u64().unwrap_or_default(),
                    result["stats"]["ambient_bytes"].as_u64().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let physics = result["stats"]["physics_solids"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let convexes = result["stats"]["physics_convexes"]
                    .as_u64()
                    .unwrap_or_default();
                let label = if count == 1 { "body" } else { "bodies" };
                let fluids = result["stats"]["physics_fluids"]
                    .as_u64()
                    .unwrap_or_default();
                let mesh = result["stats"]["physics_mesh_triangles"]
                    .as_u64()
                    .filter(|&count| count > 0)
                    .map(|count| format!(" · Terrain triangles: {count}"))
                    .unwrap_or_default();
                let fluid_label = if fluids == 1 {
                    "fluid region"
                } else {
                    "fluid regions"
                };
                format!(
                    "\nPhysics: {count} {label} · {convexes} hulls · {fluids} {fluid_label}{mesh} · Surface properties: {}",
                    result["stats"]["physics_surface_properties"].as_u64().unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let region = &job["compile_region"];
        let region_label = if let Some(error) = region["error"].as_str() {
            format!("Invalid region: {error}")
        } else if region["enabled"] == true {
            format!("sealed region {} → {}", region["mins"], region["maxs"])
        } else {
            "full map".to_owned()
        };
        self.window.set_details(
            format!(
                "Output: {}\nRecipe at build start: {}\nCompile scope at build start: {region_label}\nBuild settings: {} · {}\nVisibility: {} · Sampling: {}\nIndirect light: {} · Brush smoothing angle: {}°\nStage: {} · {:.2}s elapsed{queue_details}{instances}{packed_sky}{samples}{reflections}{props}{prop_vertex_lighting}{details}{visibility}{hints}{bounce}{areas}{occluders}{terrain}{overlays}{water}{sky}{lighting}{bumps}{model_maps}{styles}{hdr}{smoothing}{physics}\n{}",
                job["path"].as_str().unwrap_or_default(),
                job["recipe"]["name"].as_str().unwrap_or("Custom settings"),
                compiler::LightingMode::parse(job["settings"]["lighting"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::PhysicsMode::parse(job["settings"]["physics"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::VisibilityMode::parse(job["settings"]["visibility"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::SkyQuality::parse(job["settings"]["sky_quality"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::bounce_label(job["settings"]["bounces"].as_u64().unwrap_or_default() as u8),
                job["settings"]["smoothing_angle"].as_u64().unwrap_or_default(),
                job["stage_label"].as_str().unwrap_or_default(),
                job["elapsed_ms"].as_u64().unwrap_or_default() as f64 / 1000.0,
                if job["stale"].as_bool().unwrap_or(false) {
                    if job["instance_dependencies"].is_object() {"The map, build settings or instance dependencies changed. Compile again before playing."} else {"The map or build settings changed. Compile again before playing."}
                } else {
                    "This build uses the current map snapshot. Unsaved VMF edits are included."
                }
            )
            .into(),
        );
        let mut messages = Vec::new();
        if let Some(error) = result["error"].as_str() {
            messages.push(error.to_owned());
        }
        if let Some(diagnostics) = result["diagnostics"].as_array() {
            for diagnostic in diagnostics {
                messages.push(format!(
                    "{}{}: {}",
                    diagnostic["level"].as_str().unwrap_or("info"),
                    diagnostic["object"]
                        .as_str()
                        .map(|object| format!(" · {object}"))
                        .unwrap_or_default(),
                    diagnostic["message"].as_str().unwrap_or_default()
                ));
            }
            let count = result["diagnostic_count"].as_u64().unwrap_or_default() as usize;
            if count > diagnostics.len() {
                messages.push(format!(
                    "Showing the first {} of {count} diagnostics.",
                    diagnostics.len()
                ));
            }
        }
        if messages.is_empty() {
            messages.push(
                if result.is_null() {
                    "Diagnostics appear when the build finishes."
                } else {
                    "No diagnostics."
                }
                .into(),
            );
        }
        self.window.set_diagnostics(messages.join("\n\n").into());
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("Closing this window leaves an active build running. Cancel build stops it and preserves previous output.".into());
        self.refresh(session);
        self.window.show()
    }

    pub fn is_visible(&self) -> bool {
        self.window.window().is_visible() || self.baked_lightmaps.is_visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn visibility_labels_distinguish_solved_work_from_authored_output() {
        let text = visibility_details(&serde_json::json!({
            "clusters":3,"visibility_solved_clusters":7,"visibility_portals":8,
            "visibility_removed_pairs":4,"viscluster_helpers":2,"viscluster_groups":1,
            "viscluster_grouped_leaves":5,"visibility_workers":2
        }));
        assert!(text.contains("3 emitted clusters"));
        assert!(text.contains("7 solved leaf clusters"));
        assert!(text.contains("4 leaf-cluster pairs removed before authored grouping"));
        assert!(text.contains("2 helpers · 1 effective groups · 5 grouped leaves"));
        assert!(text.contains("Visibility may expand"));
    }

    #[test]
    fn hdr_results_keep_channels_separate_and_explain_empty_native_data() {
        let mut stats = serde_json::json!({
            "lighting_faces": 3,
            "lighting_work": 101,
            "lighting_total_work": 303,
            "hdr": {
                "lighting_faces": 7,
                "lightmap_samples": 11,
                "world_lights": 2,
                "lighting_bytes": 44,
                "lighting_work": 202,
                "ambient_samples": 5,
                "ambient_bytes": 160,
                "map_data_present": false
            }
        });
        let details = hdr_lighting_details(&stats);
        assert!(
            details
                .contains("HDR baked lighting: 7 faces · 11 samples · 2 light sources · 44 bytes")
        );
        assert!(details.contains("HDR model ambient: 5 probes · 160 bytes"));
        assert!(details.contains("HDR native map data: HDR unavailable: incomplete map data"));
        assert!(details.contains("Lighting work: LDR 101 · HDR 202 · Shared total 303"));
        assert!(!details.contains("3 faces"));
        stats["hdr"]["map_data_present"] = true.into();
        assert!(hdr_lighting_details(&stats).contains("HDR data ready"));
        stats["hdr"] = serde_json::Value::Null;
        assert!(hdr_lighting_details(&stats).is_empty());
    }

    #[test]
    fn recipe_controls_share_crud_apply_cancel_and_job_guards() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (directory, game, _) = crate::test_assets::game_process(true);
        let mut session = Session::new(Ok(game));
        let path = directory.path().join("room.vmf");
        std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
        session.open(Actor::User, &path, false).unwrap();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_selected_recipe(), "Fast preview");
        assert!(
            controller
                .window
                .get_recipe_status()
                .contains("Current recipe: Fast preview")
        );
        assert!(!controller.window.get_custom_recipe_selected());
        assert!(!controller.window.get_sky_enabled());
        controller.window.invoke_change_lighting(2);
        controller.refresh(&lock(&shared));
        assert_eq!(
            controller.window.get_selected_lighting(),
            "Direct LDR + HDR"
        );
        assert!(controller.window.get_sky_enabled());
        controller.window.invoke_change_lighting(0);
        controller.window.invoke_choose_recipe(1);
        assert_eq!(lock(&shared).compile_settings()["lighting"], "fullbright");
        controller.window.invoke_apply_recipe();
        controller.refresh(&lock(&shared));
        assert_eq!(lock(&shared).compile_settings()["lighting"], "direct_ldr");
        assert!(
            controller
                .window
                .get_recipe_status()
                .contains("Current recipe: Final lighting")
        );
        controller.window.invoke_change_physics(0);
        controller.refresh(&lock(&shared));
        assert!(
            controller
                .window
                .get_recipe_status()
                .contains("Current recipe: Custom settings")
        );
        controller.window.invoke_change_lighting(2);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_sky_enabled());
        controller.window.set_recipe_name("Evening lighting".into());
        controller.window.invoke_save_recipe();
        let id = lock(&shared).build_recipes()["active"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(controller.window.get_selected_recipe(), "Evening lighting");
        assert!(controller.window.get_custom_recipe_selected());
        controller.window.set_recipe_name("Unfinished edit".into());
        controller.window.invoke_cancel_recipe_edit();
        assert_eq!(controller.window.get_recipe_name(), "Evening lighting");
        assert_eq!(
            lock(&shared).build_recipes()["recipes"][2]["name"],
            "Evening lighting"
        );
        controller.window.set_recipe_name("Night lighting".into());
        controller.window.invoke_rename_recipe();
        assert_eq!(
            lock(&shared).build_recipes()["recipes"][2]["name"],
            "Night lighting"
        );
        controller.window.invoke_choose_recipe(0);
        controller.window.invoke_apply_recipe();
        assert_eq!(lock(&shared).compile_settings()["lighting"], "fullbright");
        controller.window.invoke_choose_recipe(2);
        controller.window.invoke_apply_recipe();
        assert_eq!(
            lock(&shared).compile_settings()["lighting"],
            "direct_ldr_hdr"
        );
        assert_eq!(lock(&shared).compile_settings()["physics"], "none");
        controller.window.invoke_change_bounces(1);
        controller.window.invoke_replace_recipe();
        assert_eq!(
            lock(&shared).build_recipes()["recipes"][2]["settings"]["bounces"],
            1
        );
        lock(&shared)
            .rename_build_recipe(Actor::Agent, &id, "Evening preview", None)
            .unwrap();
        controller.window.set_recipe_name("Stale rename".into());
        controller.window.invoke_rename_recipe();
        assert!(controller.window.get_message().contains("changed"));
        assert_eq!(
            lock(&shared).build_recipes()["recipes"][2]["name"],
            "Evening preview"
        );
        controller.refresh(&lock(&shared));
        controller.window.show().unwrap();
        controller.window.invoke_choose_recipe(2);
        for label in [
            "Apply recipe",
            "Save current as new",
            "Replace with current",
            "Rename recipe",
            "Delete recipe",
            "Cancel recipe edit",
            "Reload recipes",
        ] {
            assert!(
                i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                    &controller.window,
                    label
                )
                .next()
                .is_some(),
                "Missing {label}"
            );
        }
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-recipes");
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-hdr-controls");
        let request = lock(&shared).begin_compile(Actor::Agent, None).unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_custom_recipes_enabled());
        assert!(!controller.window.get_lighting_enabled());
        assert!(!controller.window.get_sky_enabled());
        assert_eq!(
            lock(&shared).compile_status()["job"]["settings"]["lighting"],
            "direct_ldr_hdr"
        );
        assert!(
            controller
                .window
                .get_details()
                .contains("Build settings: Direct LDR + HDR")
        );
        assert!(
            controller
                .window
                .get_details()
                .contains("Recipe at build start: Evening preview")
        );
        controller.window.invoke_delete_recipe();
        assert!(controller.window.get_message().contains("active compile"));
        assert_eq!(
            lock(&shared).build_recipes()["recipes"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        controller.window.invoke_cancel_build();
        let result = request.run();
        lock(&shared).finish_compile(Actor::Agent, &result);
        assert!(result.is_err());
        controller.refresh(&lock(&shared));
        controller.window.invoke_delete_recipe();
        assert_eq!(
            lock(&shared).build_recipes()["recipes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(lock(&shared).document().undo_labels().count(), 0);
        controller.window.invoke_close_dialog();
        assert!(!controller.window.window().is_visible());
    }
}
