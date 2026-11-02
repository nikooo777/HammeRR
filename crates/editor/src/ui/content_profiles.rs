use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use editor_core::{
    build_settings::BuildSettings,
    content_profiles::{Config, MAX_PROFILES},
};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    AppWindow, ContentProfileRow, ContentProfilesWindow,
    runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock},
    session::{Actor, Session},
};

#[derive(Default)]
struct Form {
    reviewed: Value,
    selected: Option<String>,
    build: Option<BuildSettings>,
    dirty: bool,
    busy: bool,
}

struct CompletedAction {
    action: Action,
    result: Result<Value, String>,
}

type Completion = Arc<Mutex<Option<CompletedAction>>>;

pub(super) struct Controller {
    pub(super) window: ContentProfilesWindow,
    form: Rc<RefCell<Form>>,
}

#[derive(Clone, Copy)]
enum Action {
    Save,
    Replace,
    Apply,
    Remove,
    Reload,
}

fn admitted(
    window: &slint::Weak<ContentProfilesWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&ContentProfilesWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(&window, session);
            Ok(())
        })
    });
    if let Err(error) = result {
        message(&window, &error, true);
    }
}

fn message(window: &ContentProfilesWindow, text: &str, error: bool) {
    window.set_message(text.into());
    window.set_message_error(error);
}

fn config(value: &Value) -> Result<Config, String> {
    if value.is_null() {
        return Err("Current content is unavailable. Select a saved profile or load a valid game configuration before saving current content.".into());
    }
    serde_json::from_value(value.clone())
        .map_err(|error| format!("Content configuration is unavailable: {error}"))
}

fn build_summary(settings: &BuildSettings) -> String {
    settings
        .description()
        .unwrap_or_else(|error| format!("Build settings unavailable: {error}"))
}

fn fill(
    window: &ContentProfilesWindow,
    form: &mut Form,
    config: Result<Config, String>,
    name: &str,
) {
    match config {
        Ok(config) => {
            window.set_profile_name(name.into());
            window.set_gameinfo(config.gameinfo.to_string_lossy().into_owned().into());
            window.set_fgd(path_text(config.fgd.as_ref()));
            window.set_engine_root(path_text(config.engine_root.as_ref()));
            window.set_steam_root(path_text(config.steam_root.as_ref()));
            window.set_build_summary(build_summary(&config.build).into());
            form.build = Some(config.build);
            form.dirty = false;
        }
        Err(error) => {
            form.build = None;
            form.dirty = false;
            window.set_profile_name(name.into());
            window.set_gameinfo("".into());
            window.set_fgd("".into());
            window.set_engine_root("".into());
            window.set_steam_root("".into());
            window.set_build_summary("Build settings unavailable".into());
            message(window, &error, true);
        }
    }
}

fn path_text(path: Option<&PathBuf>) -> slint::SharedString {
    path.map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
        .into()
}

fn choose(window: &ContentProfilesWindow, session: &Session, form: &mut Form, id: Option<&str>) {
    let profile = id.and_then(|id| {
        form.reviewed["profiles"]
            .as_array()?
            .iter()
            .find(|profile| profile["id"].as_str() == Some(id))
            .cloned()
    });
    match profile {
        Some(profile) => {
            form.selected = profile["id"].as_str().map(str::to_owned);
            let name = profile["name"].as_str().unwrap_or_default();
            fill(window, form, config(&profile["config"]), name);
            window.set_selection(format!("Selected saved profile: {name}").into());
        }
        None => {
            form.selected = None;
            let current = if form.reviewed["current_config"].is_null() {
                Ok(Config {
                    gameinfo: PathBuf::new(),
                    fgd: None,
                    engine_root: None,
                    steam_root: None,
                    build: BuildSettings::from_options(session.compile_options()),
                })
            } else {
                config(&form.reviewed["current_config"])
            };
            fill(window, form, current, "");
            window.set_selection("New profile from current content".into());
        }
    }
    window.set_selected_id(form.selected.as_deref().unwrap_or_default().into());
}

fn present(window: &ContentProfilesWindow, form: &Form, state: &Value) {
    let active = &state["active_profile"];
    let name = active["name"].as_str();
    let matches = state["active_matches_current"] == true;
    let available = state["active_profile_available"] == true;
    let current = match name {
        Some(name) if matches && available => format!("Current content: {name}"),
        Some(name) if matches => {
            format!("Current content: {name} · saved profile changed or removed")
        }
        Some(name) => format!("Current content: modified from {name}"),
        None if state["current_config"].is_null() => "Current content: unavailable".into(),
        None => "Current content: custom configuration".into(),
    };
    window.set_current(current.into());
    window.set_current_path(
        state["current_config"]["gameinfo"]
            .as_str()
            .unwrap_or("No mounted game configuration")
            .into(),
    );
    let rows = form.reviewed["profiles"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_PROFILES)
        .map(|profile| ContentProfileRow {
            id: profile["id"].as_str().unwrap_or_default().into(),
            name: profile["name"].as_str().unwrap_or("Unnamed profile").into(),
            game: profile["config"]["gameinfo"]
                .as_str()
                .unwrap_or_default()
                .into(),
            current: matches && available && profile["id"] == active["id"],
        })
        .collect::<Vec<_>>();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    let revision_current = form.reviewed["revision"].as_u64().is_some()
        && form.reviewed["revision"] == state["revision"];
    let intent_current =
        form.reviewed["intent"].as_u64().is_some() && form.reviewed["intent"] == state["intent"];
    let ready = state["blocked"] != true && revision_current;
    let busy = form.busy || state["worker_active"] == true;
    window.set_busy(busy);
    window.set_ready(ready && !busy);
    window.set_can_save(ready && !busy && form.build.is_some());
    window.set_can_apply(
        ready && !busy && intent_current && !form.dirty && state["can_apply"] == true,
    );
    let mut status = Vec::new();
    if let Some(error) = state["error"].as_str() {
        status.push(error.to_owned());
    }
    if let Some(error) = state["validation_error"].as_str() {
        status.push(error.to_owned());
    }
    if !revision_current {
        status.push("Saved profiles changed. Reload before saving, removing or applying.".into());
    } else if !intent_current {
        status.push("Current content or Build settings changed. Reload before applying.".into());
    }
    if form.dirty && form.selected.is_some() {
        status.push("Save new or Replace selected before applying edited fields.".into());
    }
    if let Some(blocker) = state["apply_blocker"].as_str() {
        status.push(blocker.to_owned());
    }
    if busy {
        status.push("Validating profile… Closing this dialog leaves validation running.".into());
    }
    window.set_status(status.join("\n").into());
    let persistence = match state["persistence"].as_str() {
        Some("persistent") => "Saved on disk",
        Some("session") => "This session only",
        _ => "Profile storage unavailable",
    };
    window.set_storage(
        match state["path"].as_str() {
            Some(path) => format!("{persistence} · {path}"),
            None => persistence.to_owned(),
        }
        .into(),
    );
}

fn optional_path(value: slint::SharedString) -> Option<PathBuf> {
    (!value.trim().is_empty()).then(|| PathBuf::from(value.as_str()))
}

fn draft_config(window: &ContentProfilesWindow, form: &Form) -> Result<Config, String> {
    let build = form
        .build
        .clone()
        .ok_or("Capture current Build settings before saving a profile")?;
    Ok(Config {
        gameinfo: PathBuf::from(window.get_gameinfo().as_str()),
        fgd: optional_path(window.get_fgd()),
        engine_root: optional_path(window.get_engine_root()),
        steam_root: optional_path(window.get_steam_root()),
        build,
    })
}

fn finish_action(
    window: &ContentProfilesWindow,
    session: &mut Session,
    form: &RefCell<Form>,
    action: Action,
    result: Result<Value, String>,
) {
    let mut form = form.borrow_mut();
    form.busy = false;
    match result {
        Ok(state) => {
            let selected = match action {
                Action::Save | Action::Replace => state["affected_profile"]["id"]
                    .as_str()
                    .or_else(|| state["affected_profile"].as_str())
                    .map(str::to_owned),
                Action::Remove => None,
                _ => form.selected.clone(),
            };
            form.reviewed = state;
            message(
                window,
                match action {
                    Action::Save => "Profile saved. Select Apply to use it in this document.",
                    Action::Replace => {
                        "Selected profile replaced. Select Apply to use it in this document."
                    }
                    Action::Apply => "Selected profile applied to this document.",
                    Action::Remove => "Profile removed. Current content is unchanged.",
                    Action::Reload => "Profiles reloaded. Current content is unchanged.",
                },
                false,
            );
            choose(window, session, &mut form, selected.as_deref());
            if form.reviewed["blocked"] == true {
                message(
                    window,
                    "Profile storage could not reload. Review the storage error before continuing.",
                    true,
                );
            }
        }
        Err(error) => message(window, &error, true),
    }
    present(window, &form, &session.content_profiles_snapshot());
}

fn begin(
    window: &ContentProfilesWindow,
    session: &mut Session,
    form: &Form,
    action: Action,
) -> Result<editor_core::session::content_profiles::Request, String> {
    if form.busy {
        return Err("Profile validation is already running".into());
    }
    let revision = form.reviewed["revision"]
        .as_u64()
        .ok_or("Reload profiles before making changes")?;
    match action {
        Action::Save | Action::Replace => {
            let config = draft_config(window, form)?;
            let replace = if matches!(action, Action::Replace) {
                Some(
                    form.selected
                        .as_deref()
                        .ok_or("Select a profile to replace")?,
                )
            } else {
                None
            };
            session.begin_content_profile_save(
                window.get_profile_name().as_str(),
                Some(config),
                replace,
                Some(revision),
            )
        }
        Action::Apply => {
            let intent = form.reviewed["intent"]
                .as_u64()
                .ok_or("Reload profiles before applying")?;
            let id = form
                .selected
                .as_deref()
                .ok_or("Select a profile to apply")?;
            session.begin_content_profile_apply(id, Some(revision), Some(intent))
        }
        Action::Remove | Action::Reload => unreachable!(),
    }
}

fn spawn_validation(
    window: &ContentProfilesWindow,
    shared: &SharedSession,
    form: &RefCell<Form>,
    completion: &Completion,
    action: Action,
) {
    let request = begin(window, &mut lock(shared), &form.borrow(), action);
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            message(window, &error, true);
            present(
                window,
                &form.borrow(),
                &lock(shared).content_profiles_snapshot(),
            );
            return;
        }
    };
    let token = request.token();
    form.borrow_mut().busy = true;
    message(window, "", false);
    present(
        window,
        &form.borrow(),
        &lock(shared).content_profiles_snapshot(),
    );
    let worker_session = shared.clone();
    let worker_window = window.as_weak();
    let completion = completion.clone();
    let spawned = std::thread::Builder::new()
        .name("content-profile-validation".into())
        .spawn(move || {
            let completed = request.run();
            let result = lock(&worker_session).finish_content_profile(Actor::User, completed);
            *completion.lock().expect("Profile completion lock poisoned") =
                Some(CompletedAction { action, result });
            let _ = worker_window.upgrade_in_event_loop(|window| window.invoke_worker_finished());
        });
    if let Err(error) = spawned {
        let error = format!("Could not start profile validation: {error}");
        lock(shared).fail_content_profile(token, error.clone());
        finish_action(window, &mut lock(shared), form, action, Err(error));
    }
}

fn act(
    window: &ContentProfilesWindow,
    session: &mut Session,
    form: &RefCell<Form>,
    action: Action,
) {
    let result = match action {
        Action::Remove => {
            let form = form.borrow();
            let revision = form.reviewed["revision"].as_u64();
            match (form.selected.as_deref(), revision) {
                (Some(id), Some(revision)) => {
                    session.remove_content_profile(Actor::User, id, Some(revision))
                }
                _ => Err("Select a profile to remove after reloading".into()),
            }
        }
        Action::Reload => session.reload_content_profiles(Actor::User, None),
        _ => unreachable!(),
    };
    finish_action(window, session, form, action, result);
}

impl Controller {
    pub(super) fn new_with_access(
        app: &AppWindow,
        access: &DocumentAccess,
    ) -> Result<Self, slint::PlatformError> {
        let window = ContentProfilesWindow::new()?;
        window.set_max_profiles(MAX_PROFILES as i32);
        let form = Rc::new(RefCell::new(Form::default()));
        let completion: Completion = Arc::new(Mutex::new(None));
        let callback = |action| {
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                admitted(&window, &target, |window, session| {
                    act(window, session, &form, action)
                })
            }
        };
        window.on_remove(callback(Action::Remove));
        window.on_reload(callback(Action::Reload));
        let worker_callback = |action| {
            let (window, target, form, completion) = (
                window.as_weak(),
                access.downgrade(),
                form.clone(),
                completion.clone(),
            );
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_shared(|shared| {
                        spawn_validation(&window, shared, &form, &completion, action);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    message(&window, &error, true);
                }
            }
        };
        window.on_save_new(worker_callback(Action::Save));
        window.on_replace(worker_callback(Action::Replace));
        window.on_apply(worker_callback(Action::Apply));
        window.on_worker_finished({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                let completed = completion
                    .lock()
                    .expect("Profile completion lock poisoned")
                    .take();
                let Some(CompletedAction { action, result }) = completed else {
                    return;
                };
                form.borrow_mut().busy = false;
                admitted(&window, &target, |window, session| {
                    finish_action(window, session, &form, action, result);
                });
            }
        });
        window.on_choose({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |id| {
                admitted(&window, &target, |window, session| {
                    message(window, "", false);
                    choose(window, session, &mut form.borrow_mut(), Some(id.as_str()));
                    present(window, &form.borrow(), &session.content_profiles_snapshot());
                })
            }
        });
        window.on_use_current({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                admitted(&window, &target, |window, session| {
                    message(window, "", false);
                    let mut form = form.borrow_mut();
                    let state = session.content_profiles_snapshot();
                    form.reviewed["current_config"] = state["current_config"].clone();
                    choose(window, session, &mut form, None);
                    present(window, &form, &state);
                })
            }
        });
        window.on_capture_build({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                admitted(&window, &target, |window, session| {
                    let state = session.content_profiles_snapshot();
                    let mut form = form.borrow_mut();
                    let build = BuildSettings::from_options(session.compile_options());
                    window.set_build_summary(build_summary(&build).into());
                    form.build = Some(build);
                    form.dirty = true;
                    message(
                        window,
                        "Current shared Build settings captured in this form.",
                        false,
                    );
                    present(window, &form, &state);
                })
            }
        });
        window.on_draft_changed({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                admitted(&window, &target, |window, session| {
                    form.borrow_mut().dirty = true;
                    present(window, &form.borrow(), &session.content_profiles_snapshot());
                })
            }
        });
        window.on_build_settings({
            let app = app.as_weak();
            move || {
                if let Some(app) = app.upgrade() {
                    app.invoke_build_status();
                }
            }
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade()
                    && let Err(error) = window.hide()
                {
                    message(&window, &error.to_string(), true);
                }
            }
        });
        Ok(Self { window, form })
    }

    pub(super) fn refresh(&self, session: &Session) {
        present(
            &self.window,
            &self.form.borrow(),
            &session.content_profiles_snapshot(),
        );
    }

    pub(super) fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        message(&self.window, "", false);
        let mut form = self.form.borrow_mut();
        let state = session.content_profiles_snapshot();
        if !form.busy {
            form.reviewed = state.clone();
            choose(&self.window, session, &mut form, None);
        }
        present(&self.window, &form, &state);
        self.window.show()
    }
}
