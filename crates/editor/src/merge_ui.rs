use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{MergeRow, MergeWindow};

fn parameters(window: &MergeWindow) -> Result<Vec<u32>, String> {
    let mut sources = window
        .get_sources()
        .iter()
        .filter(|row| row.selected)
        .map(|row| {
            row.solid
                .parse::<u32>()
                .map_err(|_| "Invalid source brush".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    sources.sort_unstable();
    Ok(sources)
}

fn can_preview(window: &MergeWindow) -> bool {
    parameters(window)
        .is_ok_and(|sources| (2..=map_core::merge::MAX_BRUSHES).contains(&sources.len()))
}

pub fn refresh(window: &MergeWindow, session: &Session) {
    let report = session.merge_report();
    let preview = &report["preview"];
    let token = preview["token"].as_str().unwrap_or_default();
    let source = format!(
        "{}:{}:{:?}",
        session.document_generation(),
        session.document().revision(),
        session.selected_ids()
    );
    let incoming = !token.is_empty() && window.get_token() != token;
    let chosen = if incoming {
        preview["result"]["sources"].as_array().map(|ids| {
            ids.iter()
                .map(|id| id.as_u64().unwrap() as u32)
                .collect::<Vec<_>>()
        })
    } else if window.get_source() == source {
        parameters(window).ok()
    } else {
        None
    };
    match session.merge_sources() {
        Ok(sources) => {
            let rows = sources
                .iter()
                .map(|source| {
                    let size = source.maxs - source.mins;
                    let owner = match source.owner {
                        map_core::ObjectId::Solid(_) => "world".to_string(),
                        map_core::ObjectId::Entity(id) => format!("entity {id}"),
                    };
                    MergeRow {
                        solid: source.solid.to_string().into(),
                        label: format!(
                            "Brush {} · {} · {:.1} × {:.1} × {:.1} · {}",
                            source.solid, owner, size.x, size.y, size.z, source.material
                        )
                        .into(),
                        selected: chosen
                            .as_ref()
                            .is_none_or(|ids| ids.contains(&source.solid)),
                    }
                })
                .collect::<Vec<_>>();
            window.set_sources(Rc::new(VecModel::from(rows)).into());
        }
        Err(error) => {
            window.set_sources(Rc::new(VecModel::from(Vec::<MergeRow>::new())).into());
            window.set_message(error.into());
        }
    }
    window.set_source(source.into());
    window.set_stamp(session.merge_stamp().into());
    window.set_token(token.into());
    window.set_can_preview(can_preview(window));
    window.set_ready(
        !preview.is_null()
            && parameters(window)
                .is_ok_and(|sources| preview["result"]["sources"] == serde_json::json!(sources)),
    );
    window.set_summary(if preview.is_null() {
        "Select at least two brushes, then preview the result.".into()
    } else {
        let result = &preview["result"];
        format!(
            "{} brushes → Brush {} · {} outer faces\nThe map is unchanged until Apply.",
            result["sources"].as_array().unwrap().len(),
            result["solid"],
            result["faces"].as_array().unwrap().len()
        )
        .into()
    });
}

pub fn refresh_changed(window: &MergeWindow, session: &Session) {
    if window.get_stamp() != session.merge_stamp() {
        refresh(window, session);
    }
}

fn with_window<R>(
    window: &slint::Weak<MergeWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&MergeWindow, &SharedSession) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = target.upgrade().and_then(|access| {
        access.with_shared(|shared| {
            #[cfg(test)]
            {
                let hook = BEFORE_ACTION.with(|slot| slot.borrow_mut().take());
                if let Some(hook) = hook {
                    hook();
                }
            }
            Ok(body(&window, shared))
        })
    });
    match result {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

#[cfg(test)]
pub fn bind(window: &MergeWindow, shared: &SharedSession) {
    bind_with_access(window, &DocumentAccess::unmanaged(shared.clone()));
}

pub fn bind_with_access(window: &MergeWindow, access: &DocumentAccess) {
    window.on_choose_source({
        let window = window.as_weak();
        let target = access.downgrade();
        move |id| {
            with_window(&window, &target, |window, _| {
                let rows = window
                    .get_sources()
                    .iter()
                    .map(|mut row| {
                        if row.solid == id {
                            row.selected = !row.selected;
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                window.set_sources(Rc::new(VecModel::from(rows)).into());
                window.set_ready(false);
                window.set_can_preview(can_preview(window));
                window.set_message("Sources changed. Preview the result before applying.".into());
            });
        }
    });
    window.on_preview_edit({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = parameters(window).and_then(|sources| {
                    session.preview_merge(&sources, window.get_stamp().as_str())
                });
                refresh(window, &session);
                if let Err(error) = result {
                    window.set_ready(false);
                    window.set_message(error.into());
                } else {
                    window.set_message("".into());
                }
            });
        }
    });
    window.on_discard({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = window
                    .get_token()
                    .parse::<u64>()
                    .map_err(|_| "Preview a merge first".to_string())
                    .and_then(|token| session.cancel_merge(token));
                refresh(window, &session);
                window.set_message(result.err().unwrap_or_default().into());
            });
        }
    });
    window.on_apply({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, shared| {
                if !window.get_ready() {
                    return;
                }
                let mut session = lock(shared);
                let result = window
                    .get_token()
                    .parse::<u64>()
                    .map_err(|_| "Preview a merge first".to_string())
                    .and_then(|token| session.commit_merge(Actor::User, token));
                refresh(window, &session);
                window.set_message(
                    result
                        .map(|_| "Merge applied. Undo restores the original brushes.".to_string())
                        .unwrap_or_else(|error| error)
                        .into(),
                );
            });
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            with_window(&window.as_weak(), &target, |window, shared| {
                if let Ok(token) = window.get_token().parse::<u64>() {
                    let _ = lock(shared).cancel_merge(token);
                }
            });
            let _ = window.hide();
        }
    });
    window.window().on_close_requested({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                window.invoke_cancel();
            }
            slint::CloseRequestResponse::HideWindow
        }
    });
}

#[cfg(test)]
std::thread_local! {
    static BEFORE_ACTION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod admission_tests;
