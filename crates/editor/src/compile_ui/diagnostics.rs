use serde_json::Value;
use slint::{ComponentHandle, Model};

use super::{Actor, CompileWindow, DocumentAccess};
use crate::BuildDiagnosticRow;

pub(super) fn bind(window: &CompileWindow, access: &DocumentAccess) {
    let (callback_window, target) = (window.as_weak(), access.downgrade());
    window.on_frame_diagnostic(move |token, index| {
        let Some(window) = callback_window.upgrade() else {
            return;
        };
        let result = usize::try_from(index)
            .map_err(|_| "Diagnostic index is unavailable".to_owned())
            .and_then(|index| {
                target.upgrade()?.with_session(|session| {
                    session.frame_build_diagnostic(
                        Actor::User,
                        token.as_str(),
                        index,
                        window.get_aspect(),
                    )
                })
            });
        let message = match result {
            Ok(_) => format!("Framed recorded world position from build {token}."),
            Err(error) => format!("Frame unavailable: {error}"),
        };
        let rows = window.get_diagnostic_rows();
        if let Some((slot, mut row)) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.job_token.as_str() == token.as_str() && row.index == index)
        {
            row.feedback = message.into();
            rows.set_row_data(slot, row);
        } else {
            window.set_message(message.into());
        }
    });
}

pub(super) fn refresh(window: &CompileWindow, job: &Value) {
    let result = &job["result"];
    let token = job["token"].as_str().unwrap_or_default();
    let previous = window.get_diagnostic_rows();
    let diagnostics = result["diagnostics"].as_array();
    let mut messages = Vec::new();
    let error = result["error"].as_str().unwrap_or_default();
    if !error.is_empty() {
        messages.push(error.to_owned());
    }
    let rows = diagnostics
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(slot, diagnostic)| {
            let index = diagnostic["index"]
                .as_u64()
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(-1);
            let heading = format!(
                "{}{}",
                diagnostic["level"].as_str().unwrap_or("info"),
                diagnostic["object"]
                    .as_str()
                    .map(|object| format!(" · {object}"))
                    .unwrap_or_default()
            );
            let message = diagnostic["message"].as_str().unwrap_or_default();
            messages.push(format!("{heading}: {message}"));
            let world = diagnostic["position_space"] == "world";
            let available =
                diagnostic["frame_available"] == true && world && index >= 0 && !token.is_empty();
            let reason = if available {
                ""
            } else {
                diagnostic["frame_unavailable_reason"].as_str().unwrap_or(
                    if token.is_empty() || index < 0 {
                        "No retained diagnostic identity is available"
                    } else {
                        "No certified captured world position is available"
                    },
                )
            };
            BuildDiagnosticRow {
                job_token: token.into(),
                index,
                heading: heading.into(),
                message: message.into(),
                position: if diagnostic["position"].is_null() {
                    "".into()
                } else {
                    format!(
                        "Recorded {}position: {}",
                        if world { "world " } else { "unclassified " },
                        diagnostic["position"]
                    )
                    .into()
                },
                frame_available: available,
                unavailable_reason: reason.into(),
                feedback: previous
                    .row_data(slot)
                    .filter(|row| row.job_token.as_str() == token && row.index == index)
                    .map(|row| row.feedback)
                    .unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();
    let count = result["diagnostic_count"].as_u64().unwrap_or_default();
    let summary = if result.is_null() {
        "Diagnostics appear when the build finishes.".to_owned()
    } else if count > rows.len() as u64 {
        format!("Showing the first {} of {count} diagnostics.", rows.len())
    } else if count == 0 {
        "No diagnostics.".to_owned()
    } else {
        format!("{count} diagnostics.")
    };
    if count > rows.len() as u64 || messages.is_empty() {
        messages.push(summary.clone());
    }
    window.set_diagnostic_summary(summary.into());
    window.set_diagnostic_error(error.into());
    if !previous.iter().eq(rows.iter().cloned()) {
        window.set_diagnostic_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    }
    window.set_diagnostics(messages.join("\n\n").into());
}
