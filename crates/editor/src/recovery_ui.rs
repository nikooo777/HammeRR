use std::time::{SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::recovery::{Kind, Settings};
use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{RecoveryRow, RecoveryWindow};

const PAGE_SIZE: usize = 20;

#[cfg(test)]
mod tests;

fn age(created_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let minutes = now.saturating_sub(created_ms) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} days ago", minutes / 1440),
    }
}

fn append_warning(window: &RecoveryWindow, warning: &str) {
    let current = window.get_warning();
    window.set_warning(if current.is_empty() {
        warning.into()
    } else {
        format!("{current}\n{warning}").into()
    });
}

pub fn refresh(window: &RecoveryWindow, session: &Session) {
    let (settings, status) = session.recovery_status_snapshot();
    window.set_busy(session.recovery_busy());
    window.set_available(status["available"].as_bool().unwrap_or(false));
    window.set_dirty_map(
        session.document().is_dirty() || session.partition_preview_token().is_some(),
    );
    let review = session.buffer_replacement_review().token();
    if window.get_map_review() != review {
        window.set_discard_changes(false);
        window.set_map_review(review.into());
    }
    if !window.get_settings_dirty() {
        window.set_autosave_enabled(settings.settings.enabled);
        window.set_interval_seconds(settings.settings.interval_seconds as i32);
        window.set_keep_count(settings.settings.keep_per_map as i32);
        window.set_settings_revision(settings.revision.to_string().into());
    }
    let state = if session.recovery_busy() {
        "Writing a recovery snapshot…"
    } else if status["error"].is_string() {
        "Recovery needs attention"
    } else if !settings.settings.enabled {
        "Automatic snapshots are off"
    } else if status["pending_changes"] == true {
        "Unsaved changes · autosave is scheduled"
    } else {
        "No changes waiting for autosave"
    };
    window.set_status(state.into());
    window.set_storage(
        status["directory"]
            .as_str()
            .unwrap_or("Storage unavailable")
            .into(),
    );
    window.set_warning(status["error"].as_str().unwrap_or("").into());
    let listing = match session.recovery_entries() {
        Ok(listing) => listing,
        Err(error) => {
            append_warning(window, &error);
            window.set_rows(ModelRc::default());
            window.set_selected_id("".into());
            return;
        }
    };
    let entries: Vec<_> = listing
        .entries
        .into_iter()
        .filter(|entry| {
            !window.get_current_map_only()
                || Some(entry.document_key.as_str()) == status["document_key"].as_str()
        })
        .collect();
    let page = (window.get_page().max(0) as usize).min(entries.len().saturating_sub(1) / PAGE_SIZE);
    window.set_page(page as i32);
    window.set_has_next(entries.len() > (page + 1) * PAGE_SIZE);
    window.set_summary(
        format!(
            "{} recovery snapshot{}",
            entries.len(),
            if entries.len() == 1 { "" } else { "s" }
        )
        .into(),
    );
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.id == window.get_selected_id().as_str())
    {
        window.set_selected_detail(
            format!(
                "{} · {}\n{}",
                entry.title,
                age(entry.created_ms),
                entry
                    .source_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Originally an unnamed map".into())
            )
            .into(),
        );
    } else {
        window.set_selected_id("".into());
        window.set_selected_detail("Select a snapshot to recover or delete.".into());
        window.set_confirm_delete(false);
    }
    window.set_rows(ModelRc::new(VecModel::from(
        entries
            .iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .map(|entry| RecoveryRow {
                id: entry.id.clone().into(),
                title: entry.title.clone().into(),
                detail: format!(
                    "{} · {} · {} KiB",
                    match entry.kind {
                        Kind::Autosave => "Autosave",
                        Kind::Manual => "Manual snapshot",
                        Kind::BeforeSave => "Previous saved file",
                    },
                    age(entry.created_ms),
                    entry.bytes.div_ceil(1024)
                )
                .into(),
            })
            .collect::<Vec<_>>(),
    )));
    if !listing.warnings.is_empty() {
        append_warning(
            window,
            &format!(
                "{} incomplete or unreadable snapshots were skipped. {}",
                listing.warnings.len(),
                listing.warnings[0]
            ),
        );
    }
}

#[cfg(test)]
pub fn bind(window: &RecoveryWindow, shared: &SharedSession) {
    bind_with_access(
        window,
        shared,
        crate::runtime::DocumentAccess::unmanaged(shared.clone()),
    );
}

pub fn bind_with_access(
    window: &RecoveryWindow,
    shared: &SharedSession,
    access: crate::runtime::DocumentAccess,
) {
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
    window.on_reload_settings({
        let (window, session) = (window.as_weak(), shared.clone());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let mut session = lock(&session);
            match session.reload_recovery_settings(Actor::User, None) {
                Ok(_) => {
                    window.set_settings_dirty(false);
                    window.set_message("Saved recovery settings reloaded.".into());
                }
                Err(error) => window.set_message(error.into()),
            }
            refresh(&window, &session);
        }
    });
    window.on_apply_settings({
        let (window, session) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let version = window.get_settings_revision().parse::<u64>();
            let result = version
                .map_err(|_| "Reload recovery settings first".to_owned())
                .and_then(|version| {
                    lock(&session).set_recovery_settings(
                        Actor::User,
                        Settings {
                            enabled: window.get_autosave_enabled(),
                            interval_seconds: window.get_interval_seconds().max(0) as u64,
                            keep_per_map: window.get_keep_count().max(0) as usize,
                        },
                        Some(version),
                    )
                });
            match result {
                Ok(()) => {
                    window.set_settings_dirty(false);
                    window.set_message("Recovery settings saved.".into());
                }
                Err(error) => window.set_message(error.into()),
            }
            refresh(&window, &lock(&session));
        }
    });
    window.on_select_snapshot({
        let (window, session) = (window.as_weak(), shared.clone());
        move |id| {
            let window = window.unwrap();
            window.set_selected_id(id);
            window.set_confirm_delete(false);
            window.set_discard_changes(false);
            window.set_message("".into());
            refresh(&window, &lock(&session));
        }
    });
    window.on_delete_snapshot({
        let (window, session) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if !window.get_confirm_delete() {
                window.set_message("Confirm deletion first.".into());
                return;
            }
            let result =
                lock(&session).delete_recovery_snapshot(Actor::User, &window.get_selected_id());
            window.set_message(match result {
                Ok(()) => "Snapshot deleted.".into(),
                Err(error) => error.into(),
            });
            refresh(&window, &lock(&session));
        }
    });
    window.on_snapshot_now({
        let (window, session) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if let Err(error) = start_snapshot(&session) {
                window.set_message(error.into());
            }
            refresh(&window, &lock(&session));
        }
    });
    window.on_restore_snapshot({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
            let admitted = access.with_session(|session| {
                let result = session
                    .buffer_replacement_review_from_token(&window.get_map_review())
                    .and_then(|review| {
                        session.restore_recovery_snapshot_reviewed(
                            Actor::User,
                            &window.get_selected_id(),
                            window.get_discard_changes(),
                            None,
                            &review,
                        )
                    });
                match result {
                    Ok(()) => {
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, session);
                Ok(())
            });
            if let Err(error) = admitted {
                window.set_message(error.into());
            }
        }
    });
}

fn start_snapshot(session: &SharedSession) -> Result<(), String> {
    crate::runtime::start_recovery_snapshot(session, Actor::User)
}
