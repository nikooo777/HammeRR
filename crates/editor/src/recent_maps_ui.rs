use std::path::Path;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{RecentMapRow, RecentMapsWindow};

fn map_guard(session: &Session) -> String {
    format!(
        "{}:{}:{:?}",
        session.document_generation(),
        session.document().revision(),
        session.partition_preview_token()
    )
}

pub fn refresh(window: &RecentMapsWindow, session: &Session) {
    let listing = session.recent_maps();
    let guard = map_guard(session);
    if window.get_map_guard() != guard {
        window.set_map_guard(guard.into());
        window.set_discard_changes(false);
    }
    window.set_dirty_map(
        session.document().is_dirty() || session.partition_preview_token().is_some(),
    );
    window.set_writable(listing["persistence"] != "blocked");
    window.set_warning(listing["error"].as_str().unwrap_or("").into());
    window.set_summary(
        format!(
            "{} / {} recent maps",
            listing["entries"].as_array().map_or(0, Vec::len),
            crate::recent_maps::MAX_RECENT_MAPS
        )
        .into(),
    );
    let selected = window.get_selected_path();
    let entries = listing["entries"]
        .as_array()
        .expect("recent maps entries are an array");
    let mut rows = Vec::new();
    let mut selected_available = false;
    let mut selected_detail = None;
    for entry in entries {
        let path = entry["path"].as_str().unwrap_or_default();
        let available = entry["available"] == true;
        let compatible = entry["context_compatible"] == true;
        rows.push(RecentMapRow {
            path: path.into(),
            title: entry["title"].as_str().unwrap_or(path).into(),
            detail: format!(
                "{} · {}",
                if available {
                    "Available"
                } else {
                    "Missing or unavailable"
                },
                if compatible {
                    "Build settings retained"
                } else {
                    "Different game content"
                }
            )
            .into(),
        });
        if selected.as_str() == path {
            selected_available = available;
            let settings = &entry["build_settings"];
            let game = entry["game_directory"]
                .as_str()
                .unwrap_or("No mounted game");
            selected_detail = Some(format!(
                "{path}\nGame: {game}\nBuild: {} lighting · {} physics · {} sky · {} visibility · {} bounces · {}° smoothing · LDR reflections {}\n{}",
                settings["lighting"].as_str().unwrap_or_default(),
                settings["physics"].as_str().unwrap_or_default(),
                settings["sky_quality"].as_str().unwrap_or_default(),
                settings["visibility"].as_str().unwrap_or_default(),
                settings["bounces"],
                settings["smoothing_angle"],
                if settings["generate_reflections"] == true {
                    "on"
                } else {
                    "off"
                },
                if compatible {
                    "Opening restores these build settings."
                } else {
                    "Opening keeps the current game and build settings. Restart with the listed game to restore this context."
                }
            ));
        }
    }
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    window.set_selected_available(selected_available);
    if let Some(detail) = selected_detail {
        window.set_selected_detail(detail.into());
    } else {
        window.set_selected_path("".into());
        window.set_selected_detail(
            "Select a map to inspect its path and retained build settings.".into(),
        );
    }
}

pub fn bind(window: &RecentMapsWindow, shared: &SharedSession) {
    window.on_cancel({
        let window = window.as_weak();
        move || {
            let _ = window.unwrap().hide();
        }
    });
    window.on_refresh({
        let (window, session) = (window.as_weak(), shared.clone());
        move || refresh(&window.unwrap(), &lock(&session))
    });
    window.on_select_map({
        let (window, session) = (window.as_weak(), shared.clone());
        move |path| {
            let window = window.unwrap();
            window.set_selected_path(path);
            window.set_message("".into());
            refresh(&window, &lock(&session));
        }
    });
    window.on_open_selected({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            if window.get_map_guard() != map_guard(&session) {
                window.set_message(
                    "The current map changed; review it before opening another map.".into(),
                );
                refresh(&window, &session);
                return;
            }
            let result = session.open_recent_map(
                Actor::User,
                Path::new(window.get_selected_path().as_str()),
                window.get_discard_changes(),
            );
            match result {
                Ok(message) => {
                    window.set_message(message.into());
                    let _ = window.hide();
                }
                Err(error) => window.set_message(error.into()),
            }
            refresh(&window, &session);
        }
    });
    window.on_remove_selected({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = session.remove_recent_map(Path::new(window.get_selected_path().as_str()));
            window.set_message(match result {
                Ok(()) => "Removed from recent maps; the file is unchanged.".into(),
                Err(error) => error.into(),
            });
            refresh(&window, &session);
        }
    });
    window.on_clear_list({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if !window.get_confirm_clear() {
                window.set_message("Select Confirm clear list first.".into());
                return;
            }
            let mut session = lock(&shared);
            let result = session.clear_recent_maps();
            window.set_message(match result {
                Ok(()) => "Recent maps cleared; map files are unchanged.".into(),
                Err(error) => error.into(),
            });
            window.set_confirm_clear(false);
            refresh(&window, &session);
        }
    });
}
