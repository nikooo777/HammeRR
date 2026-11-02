use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

use editor_core::{build_settings::BuildSettings, content_profiles::Config};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session, recent::Request};
use crate::{RecentMapRow, RecentMapsWindow};

struct WorkResult {
    result: Result<Value, String>,
}

type Completion = Arc<Mutex<Option<WorkResult>>>;

fn map_guard(session: &Session) -> String {
    session.buffer_replacement_review().token()
}

fn build_description(value: &Value) -> String {
    serde_json::from_value::<BuildSettings>(value.clone())
        .map_err(|error| error.to_string())
        .and_then(|settings| settings.description())
        .unwrap_or_else(|error| format!("Build settings unavailable: {error}"))
}

fn optional_path(path: Option<&PathBuf>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "Discovery".into())
}

fn entry_detail(entry: &Value) -> String {
    let path = entry["path"].as_str().unwrap_or_default();
    let reopen = entry["reopen_path"].as_str().unwrap_or(path);
    let mut detail = if path == reopen {
        path.to_owned()
    } else {
        format!("Open: {reopen}\nFile: {path}")
    };
    if let Some(error) = entry["availability_error"].as_str() {
        detail.push_str(&format!("\nUnavailable: {error}"));
    }
    if entry["context_mode"] == "full_content" {
        match serde_json::from_value::<Config>(entry["content"].clone()) {
            Ok(config) => detail.push_str(&format!(
                "\n\nCaptured content\ngameinfo.txt: {}\nFGD: {}\nCS:S root: {}\nSteam root: {}\n\nBuild: {}\n\nOpening restores this captured context. Later profile changes do not alter it.",
                config.gameinfo.display(),
                optional_path(config.fgd.as_ref()),
                optional_path(config.engine_root.as_ref()),
                optional_path(config.steam_root.as_ref()),
                build_description(&entry["build_settings"]),
            )),
            Err(error) => detail.push_str(&format!("\nRemembered content is unavailable: {error}")),
        }
    } else {
        detail.push_str(&format!(
            "\n\nLegacy context · no captured game, definitions or launch paths\nGame directory: {}\n\nBuild: {}\n\n{}",
            entry["game_directory"].as_str().unwrap_or("Unavailable"),
            build_description(&entry["build_settings"]),
            if entry["context_compatible"] == true {
                "Opening keeps current content and definitions, and restores the remembered build settings."
            } else {
                "Opening keeps current content, definitions and build settings because the remembered game differs."
            },
        ));
    }
    detail
}

fn serial(listing: &Value) -> String {
    listing["serial"]
        .as_u64()
        .map(|serial| serial.to_string())
        .unwrap_or_default()
}

pub fn refresh(window: &RecentMapsWindow, session: &Session) {
    let listing = session.recent_maps();
    let guard = map_guard(session);
    if window.get_map_guard() != guard {
        window.set_map_guard(guard.into());
        window.set_discard_changes(false);
    }
    window.set_dirty_map(session.document_close_review().requires_discard());
    window.set_writable(listing["persistence"] != "blocked");
    let warning = [
        listing["error"].as_str(),
        listing["validation_error"].as_str(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    window.set_warning(warning.into());
    window.set_summary(
        format!(
            "{} / {} recent maps",
            listing["entries"].as_array().map_or(0, Vec::len),
            crate::recent_maps::MAX_RECENT_MAPS
        )
        .into(),
    );
    let selected = window.get_selected_path();
    let current_serial = serial(&listing);
    let reviewed_serial = window.get_list_serial();
    let stale =
        !selected.is_empty() && !reviewed_serial.is_empty() && reviewed_serial != current_serial;
    let mut rows = Vec::new();
    let mut selected_entry = None;
    for entry in listing["entries"].as_array().into_iter().flatten() {
        let path = entry["path"].as_str().unwrap_or_default();
        rows.push(RecentMapRow {
            path: path.into(),
            title: entry["title"].as_str().unwrap_or(path).into(),
            detail: format!(
                "{} · {}",
                if entry["available"] == true {
                    "Available"
                } else {
                    "Missing or unavailable"
                },
                if entry["context_mode"] == "full_content" {
                    "Captured content and build settings"
                } else {
                    "Legacy build context"
                }
            )
            .into(),
        });
        if selected.as_str() == path {
            selected_entry = Some(entry);
        }
    }
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    window.set_selection_stale(stale && selected_entry.is_some());
    window.set_selected_available(selected_entry.is_some_and(|entry| entry["available"] == true));
    window.set_selected_relocatable(
        selected_entry.is_some_and(|entry| entry["relocation_available"] == true),
    );
    match selected_entry {
        Some(entry) if !stale => {
            window.set_list_serial(current_serial.into());
            window.set_selected_detail(entry_detail(entry).into());
        }
        Some(_) => {}
        None => {
            window.set_selected_path("".into());
            window.set_list_serial(current_serial.into());
            window.set_selected_detail("Select a map to inspect its remembered context.".into());
        }
    }
    let own_work = !window.get_worker_token().is_empty();
    let active = listing["worker_active"] == true;
    let picking = window.get_picking();
    window.set_busy(own_work || active || picking);
    window.set_can_cancel_work(own_work);
    window.set_progress(if picking {
        "Choose the existing replacement VMF. Cancel leaves the list unchanged.".into()
    } else if listing["worker_status"] == "cancelling" {
        "Cancelling operation…".into()
    } else if own_work {
        window.get_worker_action()
    } else if active {
        "Validating a recent map operation…".into()
    } else {
        "".into()
    });
}

fn with_session<R>(
    window: &slint::Weak<RecentMapsWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&RecentMapsWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

fn finish(window: &RecentMapsWindow, session: &Session, result: Result<Value, String>) {
    window.set_worker_token("".into());
    window.set_worker_action("".into());
    let opened = match result {
        Ok(result) if result["relocated"].is_object() => {
            window.set_selected_path(
                result["relocated"]["path"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            );
            window.set_list_serial(serial(&result["recent_maps"]).into());
            window.set_message(
                "Map location updated. Open selected map restores its remembered context.".into(),
            );
            false
        }
        Ok(result) => {
            window.set_message(
                result["message"]
                    .as_str()
                    .unwrap_or("Map and remembered context opened.")
                    .into(),
            );
            window.set_selected_path("".into());
            true
        }
        Err(error) => {
            window.set_message(error.into());
            false
        }
    };
    refresh(window, session);
    if opened && let Err(error) = window.hide() {
        window.set_message(error.to_string().into());
    }
}

fn spawn(
    window: &RecentMapsWindow,
    shared: &SharedSession,
    completion: &Completion,
    request: Request,
    action: &str,
) {
    let token = request.token();
    window.set_worker_token(token.to_string().into());
    window.set_worker_action(action.into());
    window.set_message("".into());
    refresh(window, &lock(shared));
    let worker_session = shared.clone();
    let worker_window = window.as_weak();
    let completion = completion.clone();
    let spawned = std::thread::Builder::new()
        .name("recent-map-context".into())
        .spawn(move || {
            let completed = request.run();
            let result = lock(&worker_session).finish_recent_map(Actor::User, completed);
            *completion
                .lock()
                .expect("Recent map completion lock poisoned") = Some(WorkResult { result });
            let _ = worker_window.upgrade_in_event_loop(|window| window.invoke_worker_finished());
        });
    if let Err(error) = spawned {
        let error = format!("Could not start recent map validation: {error}");
        lock(shared).fail_recent_map(token, error.clone());
        finish(window, &lock(shared), Err(error));
    }
}

fn cancel_owned(window: &RecentMapsWindow, session: &mut Session) {
    if let Ok(token) = window.get_worker_token().parse() {
        let result = session.cancel_recent_map(Actor::User, token);
        window.set_message(match result {
            Ok(_) => "Cancellation requested. The operation is draining.".into(),
            Err(error) => error.into(),
        });
        refresh(window, session);
    }
}

fn open(
    window: &slint::Weak<RecentMapsWindow>,
    access: &DocumentContinuation,
    completion: &Completion,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = access.upgrade().and_then(|access| {
        access.with_shared(|shared| {
            let request = {
                let mut session = lock(shared);
                let review =
                    session.buffer_replacement_review_from_token(&window.get_map_guard())?;
                let serial = window
                    .get_list_serial()
                    .parse()
                    .map_err(|_| "Select a recent map again before opening")?;
                session.begin_open_recent_map(
                    Path::new(window.get_selected_path().as_str()),
                    window.get_discard_changes(),
                    &review,
                    Some(serial),
                )?
            };
            spawn(
                &window,
                shared,
                completion,
                request,
                "Opening map and validating remembered context…",
            );
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn locate(
    window: &RecentMapsWindow,
    access: &DocumentContinuation,
    completion: &Completion,
    picker: &Rc<Cell<u64>>,
) {
    let old_path = PathBuf::from(window.get_selected_path().as_str());
    let serial = match window.get_list_serial().parse::<u64>() {
        Ok(serial) => serial,
        Err(_) => {
            window.set_message("Select a recent map again before locating it.".into());
            return;
        }
    };
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Locate moved map")
        .add_filter("Valve Map File", &["vmf"]);
    if let Some(parent) = old_path.parent().filter(|parent| parent.is_dir()) {
        dialog = dialog.set_directory(parent);
    }
    let generation = picker.get().wrapping_add(1);
    picker.set(generation);
    window.set_picking(true);
    window.set_busy(true);
    window.set_message("".into());
    window.set_progress(
        "Choose the existing replacement VMF. Cancel leaves the list unchanged.".into(),
    );
    let (window, access, completion, picker) = (
        window.as_weak(),
        access.clone(),
        completion.clone(),
        picker.clone(),
    );
    slint::spawn_local(async move {
        let chosen = dialog.pick_file().await;
        let Some(window) = window.upgrade() else { return; };
        if picker.get() != generation { return; }
        window.set_picking(false);
        let result = access.upgrade().and_then(|access| access.with_shared(|shared| {
            refresh(&window, &lock(shared));
            let Some(chosen) = chosen else { return Ok(()); };
            if window.get_selected_path().as_str() != old_path.to_string_lossy() || window.get_list_serial().as_str() != serial.to_string() {
                return Err("The selected recent entry changed while choosing a replacement. Select it again.".into());
            }
            let request = lock(shared).begin_relocate_recent_map(&old_path, chosen.path(), Some(serial))?;
            spawn(&window, shared, &completion, request, "Validating replacement map and updating its saved location…");
            Ok(())
        }));
        if let Err(error) = result { window.set_message(error.into()); }
    }).expect("event loop is running");
}

pub fn bind_with_access(
    window: &RecentMapsWindow,
    _shared: &SharedSession,
    access: DocumentAccess,
) {
    let access = access.downgrade();
    let completion: Completion = Arc::new(Mutex::new(None));
    let picker = Rc::new(Cell::new(0_u64));
    window.on_cancel({
        let (window, access, picker) = (window.as_weak(), access.clone(), picker.clone());
        move || {
            picker.set(picker.get().wrapping_add(1));
            with_session(&window, &access, |window, session| {
                window.set_picking(false);
                cancel_owned(window, session);
            });
            if let Some(window) = window.upgrade()
                && let Err(error) = window.hide()
            {
                window.set_message(error.to_string().into());
            }
        }
    });
    window.window().on_close_requested({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                window.invoke_cancel();
            }
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
    window.on_cancel_work({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_session(&window, &access, cancel_owned);
        }
    });
    window.on_worker_finished({
        let (window, access, completion) = (window.as_weak(), access.clone(), completion.clone());
        move || {
            let completed = completion
                .lock()
                .expect("Recent map completion lock poisoned")
                .take();
            if let Some(WorkResult { result }) = completed {
                if let Some(window) = window.upgrade() {
                    window.set_worker_token("".into());
                }
                with_session(&window, &access, |window, session| {
                    finish(window, session, result)
                });
            }
        }
    });
    window.on_refresh({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_session(&window, &access, |window, session| {
                let result = session.reload_recent_maps();
                window.set_confirm_clear(false);
                window.set_message(match result {
                    Ok(listing) => {
                        window.set_list_serial(serial(&listing).into());
                        "Saved list reloaded; the current map is unchanged.".into()
                    }
                    Err(error) => error.into(),
                });
                refresh(window, session);
            });
        }
    });
    window.on_select_map({
        let (window, access) = (window.as_weak(), access.clone());
        move |path| {
            with_session(&window, &access, |window, session| {
                window.set_selected_path(path);
                window.set_list_serial(serial(&session.recent_maps()).into());
                window.set_message("".into());
                refresh(window, session);
            });
        }
    });
    window.on_open_selected({
        let (window, access, completion) = (window.as_weak(), access.clone(), completion.clone());
        move || open(&window, &access, &completion)
    });
    window.on_locate_selected({
        let (window, access, completion) = (window.as_weak(), access.clone(), completion.clone());
        move || {
            with_session(&window, &access, |window, _session| {
                locate(window, &access, &completion, &picker);
            });
        }
    });
    window.on_remove_selected({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_session(&window, &access, |window, session| {
                let result =
                    session.remove_recent_map(Path::new(window.get_selected_path().as_str()));
                window.set_message(match result {
                    Ok(()) => "Removed from recent maps; the file is unchanged.".into(),
                    Err(error) => error.into(),
                });
                refresh(window, session);
            });
        }
    });
    window.on_clear_list({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_session(&window, &access, |window, session| {
                if !window.get_confirm_clear() {
                    window.set_message("Select Confirm clear list first.".into());
                    return;
                }
                let result = session.clear_recent_maps();
                window.set_message(match result {
                    Ok(()) => "Recent maps cleared; map files are unchanged.".into(),
                    Err(error) => error.into(),
                });
                window.set_confirm_clear(false);
                refresh(window, session);
            });
        }
    });
}
