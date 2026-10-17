use glam::DVec3;
use map_core::context::{self, Scope};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

#[cfg(test)]
use crate::runtime::SharedSession;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::context::{Fields, Query};
use crate::session::{Actor, Session};
use crate::{ContextRow, ContextWindow};

const PAGE_SIZE: usize = 16;

fn scope_label(scope: &str) -> String {
    if scope == "map" {
        "Map brief".into()
    } else {
        scope
            .replace("solid:", "Brush ")
            .replace("entity:", "Entity ")
            .replace("region:", "Region ")
    }
}

fn coordinate(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "Search coordinates must be finite numbers".into())
}

fn relevant_query(window: &ContextWindow, session: &Session) -> Result<Query, String> {
    let mut query = Query {
        include_map: window.get_include_map(),
        ..Query::default()
    };
    match window.get_filter_mode() {
        4 => {
            let Some(Scope::Object(id)) = session.selected_context_scope() else {
                return Err("Select one object to find its relevant context".into());
            };
            query.object = Some(id);
        }
        5 | 6 => {
            let mins = DVec3::new(
                coordinate(&window.get_query_min_x())?,
                coordinate(&window.get_query_min_y())?,
                coordinate(&window.get_query_min_z())?,
            );
            if window.get_filter_mode() == 5 {
                query.point = Some(mins);
            } else {
                query.bounds = Some([
                    mins,
                    DVec3::new(
                        coordinate(&window.get_query_max_x())?,
                        coordinate(&window.get_query_max_y())?,
                        coordinate(&window.get_query_max_z())?,
                    ),
                ]);
            }
        }
        _ => {}
    }
    Ok(query)
}

fn listing(window: &ContextWindow, session: &Session) -> Result<(Vec<Value>, usize), String> {
    let offset = window.get_page().max(0) as usize * PAGE_SIZE;
    if window.get_filter_mode() < 4 {
        let kind = match window.get_filter_mode() {
            1 => Some("map"),
            2 => Some("object"),
            3 => Some("region"),
            _ => None,
        };
        let page = session.list_context_notes(&window.get_query(), kind, offset, PAGE_SIZE)?;
        return Ok((
            page["notes"].as_array().unwrap().clone(),
            page["matching"].as_u64().unwrap() as usize,
        ));
    }
    let query = relevant_query(window, session)?;
    let search = window.get_query().to_lowercase();
    let mut notes = Vec::new();
    let mut cursor = None;
    loop {
        let page =
            session.retrieve_context(&query, context::DEFAULT_CONTEXT_BYTES, cursor.as_deref())?;
        for note in page["notes"].as_array().unwrap() {
            let full = if note["body_omitted"] == true && !search.is_empty() {
                session.get_context_note(Scope::parse(note["scope"].as_str().unwrap())?)?["note"]
                    .clone()
            } else {
                note.clone()
            };
            if format!(
                "{} {} {}",
                full["scope"].as_str().unwrap(),
                full["summary"].as_str().unwrap(),
                full["body"].as_str().unwrap_or_default()
            )
            .to_lowercase()
            .contains(&search)
            {
                notes.push(note.clone());
            }
        }
        cursor = page["next_cursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    let count = notes.len();
    Ok((
        notes.into_iter().skip(offset).take(PAGE_SIZE).collect(),
        count,
    ))
}

pub fn refresh(window: &ContextWindow, session: &Session) {
    let state = session.context_state();
    window.set_budget(
        format!(
            "{} notes · {} / 1,024 KiB",
            state["notes"],
            state["text_bytes"]
                .as_u64()
                .unwrap_or_default()
                .div_ceil(1024)
        )
        .into(),
    );
    window.set_can_attach(session.selected_context_scope().is_some());
    window.set_show_regions(state["show_regions"].as_bool().unwrap_or_default());
    match listing(window, session) {
        Ok((notes, total)) => {
            window.set_has_next(total > (window.get_page().max(0) as usize + 1) * PAGE_SIZE);
            window.set_page_summary(
                format!("{total} matching notes · page {}", window.get_page() + 1).into(),
            );
            window.set_rows(ModelRc::new(VecModel::from(
                notes
                    .iter()
                    .map(|note| {
                        let status = if note["orphaned"] == true {
                            " · attachment missing"
                        } else if note["ambiguous"] == true {
                            " · ambiguous ID"
                        } else if note["stale"] == true {
                            " · source changed"
                        } else {
                            ""
                        };
                        ContextRow {
                            scope: note["scope"].as_str().unwrap().into(),
                            summary: note["summary"].as_str().unwrap().into(),
                            detail: format!(
                                "{} · {}{status}",
                                scope_label(note["scope"].as_str().unwrap()),
                                note["author"].as_str().unwrap()
                            )
                            .into(),
                        }
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        Err(error) => {
            window.set_rows(ModelRc::default());
            window.set_has_next(false);
            window.set_page_summary(error.into());
        }
    }
    let Some(draft) = session.context_draft() else {
        window.set_token("".into());
        window.set_scope("".into());
        window.set_scope_label("Choose a note".into());
        window.set_source_status("".into());
        window.set_can_apply(false);
        window.set_can_delete(false);
        window.set_can_focus(false);
        return;
    };
    let token = draft.token.to_string();
    if window.get_token() != token {
        window.set_confirm_delete(false);
    }
    window.set_token(token.into());
    window.set_scope(draft.scope.to_string().into());
    window.set_scope_label(scope_label(&draft.scope.to_string()).into());
    window.set_region(matches!(draft.scope, Scope::Region(_)));
    window.set_summary(draft.fields.summary.clone().into());
    window.set_body(draft.fields.body.clone().into());
    window.set_min_x(draft.fields.mins[0].clone().into());
    window.set_min_y(draft.fields.mins[1].clone().into());
    window.set_min_z(draft.fields.mins[2].clone().into());
    window.set_max_x(draft.fields.maxs[0].clone().into());
    window.set_max_y(draft.fields.maxs[1].clone().into());
    window.set_max_z(draft.fields.maxs[2].clone().into());
    window.set_red(draft.fields.color[0].into());
    window.set_green(draft.fields.color[1].into());
    window.set_blue(draft.fields.color[2].into());
    window.set_note_budget(
        format!(
            "{} / 200 summary characters · {} / 4,096 text bytes",
            draft.fields.summary.chars().count(),
            draft.fields.summary.len() + draft.fields.body.len()
        )
        .into(),
    );
    let feedback = session.context_draft_feedback();
    window.set_can_apply(feedback.is_ok());
    window.set_feedback(feedback.err().unwrap_or_default().into());
    let current = draft.generation == session.document_generation()
        && draft.revision == session.document().revision();
    window.set_can_delete(draft.existing && current);
    let note = session.get_context_note(draft.scope).ok();
    let missing = note
        .as_ref()
        .is_some_and(|note| note["note"]["orphaned"] == true || note["note"]["ambiguous"] == true);
    window.set_can_focus(!missing && (!matches!(draft.scope, Scope::Region(_)) || draft.existing));
    window.set_source_status(
        if let Some(note) = note {
            let note = &note["note"];
            format!(
                "Written by {} · {}",
                note["author"].as_str().unwrap(),
                if missing {
                    "attachment missing or ambiguous"
                } else if note["stale"] == true {
                    "source changed since this note was saved"
                } else {
                    "source unchanged since this note was saved"
                }
            )
        } else {
            "New note · saved with this map".into()
        }
        .into(),
    );
}

fn token(window: &ContextWindow) -> Result<u64, String> {
    window
        .get_token()
        .parse()
        .map_err(|_| "Open a context note first".into())
}

fn fields(window: &ContextWindow) -> Fields {
    Fields {
        summary: window.get_summary().into(),
        body: window.get_body().into(),
        mins: [
            window.get_min_x().into(),
            window.get_min_y().into(),
            window.get_min_z().into(),
        ],
        maxs: [
            window.get_max_x().into(),
            window.get_max_y().into(),
            window.get_max_z().into(),
        ],
        color: [
            window.get_red() as u8,
            window.get_green() as u8,
            window.get_blue() as u8,
        ],
    }
}

fn result(window: &ContextWindow, session: &Session, result: Result<(), String>) {
    window.set_message(result.err().unwrap_or_default().into());
    refresh(window, session);
}

fn cancel(window: &ContextWindow, session: &mut Session) -> bool {
    if window.get_token().is_empty() {
        return true;
    }
    let result = token(window).and_then(|token| session.cancel_context_edit(token));
    let okay = result.is_ok();
    self::result(window, session, result);
    okay
}

fn with_session<R>(
    window: &slint::Weak<ContextWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&ContextWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access.upgrade().and_then(|access| {
        access.with_session(|session| {
            #[cfg(test)]
            admission_tests::before_action();
            Ok(body(&window, session))
        })
    }) {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

#[cfg(test)]
pub fn bind(window: &ContextWindow, shared: &SharedSession) {
    bind_with_access(window, &DocumentAccess::unmanaged(shared.clone()));
}

pub fn bind_with_access(window: &ContextWindow, access: &DocumentAccess) {
    window.on_filter({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| refresh(window, session));
        }
    });
    window.on_begin({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |scope| {
            with_session(&window, &access, |window, session| {
                let scope = match scope.as_str() {
                    "new_region" => Ok(None),
                    "selection" => session
                        .selected_context_scope()
                        .map(Some)
                        .ok_or_else(|| "Select one object first".to_owned()),
                    _ => Scope::parse(&scope).map(Some),
                };
                let operation = scope.and_then(|scope| session.begin_context_edit(scope));
                result(window, session, operation);
            });
        }
    });
    window.on_edit({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let operation = token(window)
                    .and_then(|token| session.update_context_edit(token, fields(window)));
                result(window, session, operation);
            });
        }
    });
    window.on_apply({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let operation = token(window)
                    .and_then(|token| session.apply_context_edit(Actor::User, token))
                    .map(|_| ());
                result(window, session, operation);
            });
        }
    });
    window.on_cancel_edit({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, cancel);
        }
    });
    window.on_delete_note({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let operation = token(window).and_then(|token| {
                    let draft = session
                        .context_draft()
                        .filter(|draft| draft.token == token)
                        .ok_or("Context edit was closed or replaced")?
                        .clone();
                    if !window.get_confirm_delete() {
                        return Err("Confirm note removal first".into());
                    }
                    session.delete_context_note(
                        Actor::User,
                        draft.scope,
                        draft.generation,
                        draft.revision,
                    )
                });
                result(window, session, operation);
            });
        }
    });
    window.on_focus_note({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let operation = Scope::parse(&window.get_scope()).and_then(|scope| {
                    session.focus_context(Actor::User, scope, window.get_aspect())
                });
                result(window, session, operation);
            });
        }
    });
    window.on_toggle_regions({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                session.set_context_view(window.get_show_regions());
                refresh(window, session);
            });
        }
    });
    window.on_cancel({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            if with_session(&window, &access, cancel).unwrap_or(true)
                && let Some(window) = window.upgrade()
            {
                let _ = window.hide();
            }
        }
    });
    window.window().on_close_requested({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            if with_session(&window, &access, cancel).unwrap_or(true) {
                slint::CloseRequestResponse::HideWindow
            } else {
                slint::CloseRequestResponse::KeepWindowShown
            }
        }
    });
}

#[cfg(test)]
mod admission_tests;
