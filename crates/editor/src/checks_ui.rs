use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, Session};
use crate::{CheckIssueRow, ChecksWindow};

fn title(code: &str) -> &str {
    match code {
        "zero_id" => "Invalid ID",
        "duplicate_id" => "Duplicate ID",
        "duplicate_property" => "Repeated property",
        "invalid_brush" => "Invalid brush geometry",
        "open_brush" => "Brush is not closed",
        "world_bounds" => "Outside reference world bounds",
        "redundant_faces" => "Unused brush faces",
        "empty_material" => "No material assigned",
        "invalid_projection" => "Invalid texture projection",
        "zero_lightmap_scale" => "Invalid lightmap scale",
        "invalid_displacement" => "Invalid terrain data",
        "displacement_start" => "Terrain corner mismatch",
        "displacement_blend_range" => "Unusual terrain blend weights",
        "id_space_exhausted" => "No IDs left for new objects",
        "world_class" => "Invalid world class",
        "missing_class" => "Entity has no class",
        "invalid_origin" => "Invalid entity position",
        "assets_unchecked" => "Game content unavailable",
        "definitions_unchecked" => "Entity definitions unavailable",
        "entity_geometry" => "Entity geometry mismatch",
        "property_value" => "Unexpected property value",
        "unknown_class" => "Unknown entity class",
        "compiler_entity" => "Entity not compiled yet",
        "compiler_displacement" => "Terrain not compiled yet",
        "missing_model" => "Model not found",
        "missing_material" => "Material not found",
        "invalid_connection" => "Invalid connection",
        "connection" => "Check entity connection",
        "missing_reference" => "Referenced entity not found",
        "missing_team_spawn" => "Missing team spawn",
        "spawn_origin" => "Spawn position unchecked",
        "spawn_clearance" => "Blocked standing spawn hull",
        "spawn_clearance_unchecked" => "Spawn collision unchecked",
        "spawn_clearance_scope" => "Spawn clearance limits",
        "sdk_reference_limit" => "Reference resource limit",
        "geometry_check_limit" => "Brush too complex to check",
        _ => code,
    }
}

fn severity(window: &ChecksWindow) -> Option<&'static str> {
    match window.get_severity() {
        1 => Some("error"),
        2 => Some("warning"),
        3 => Some("info"),
        _ => None,
    }
}

pub fn refresh(window: &ChecksWindow, session: &Session) {
    let report = session
        .map_checks_json(
            severity(window),
            &window.get_query(),
            window.get_page().max(0) as usize * 30,
            30,
        )
        .unwrap();
    let token = report["report"].as_str().unwrap_or_default();
    if window.get_report_token().as_str() != token {
        window.set_selected_id("".into());
        window.set_report_token(token.into());
    }
    let stale = session.map_checks_stale();
    window.set_stale(stale);
    window.set_summary(if report["report"].is_null() {
        "Run checks to inspect the applied map.".into()
    } else {
        format!(
            "{} errors · {} warnings · {} notes",
            report["counts"]["error"], report["counts"]["warning"], report["counts"]["info"]
        )
        .into()
    });
    window.set_has_next(
        report["matching"].as_u64().unwrap_or_default() as usize
            > (window.get_page().max(0) as usize + 1) * 30,
    );
    let rows = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| {
            let mut location = issue["object"]
                .as_str()
                .unwrap_or("Map")
                .replace("solid:", "Brush ")
                .replace("entity:", "Entity ");
            if !issue["face"].is_null() {
                location = format!("{} · face {}", location, issue["face"]["side"]);
            }
            if let Some(key) = issue["property"].as_str() {
                location.push_str(&format!(" · {key}"));
            }
            if issue["hidden"] == true {
                location.push_str(" · hidden");
            }
            CheckIssueRow {
                id: issue["id"].as_str().unwrap().into(),
                title: title(issue["code"].as_str().unwrap()).into(),
                location: location.into(),
                severity: issue["severity"].as_str().unwrap().into(),
            }
        })
        .collect::<Vec<_>>();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    window.set_page_summary(if report["report"].is_null() {
        "No report".into()
    } else {
        format!(
            "{} matching issues · page {}",
            report["matching"],
            window.get_page() + 1
        )
        .into()
    });
    if report["omitted"].as_u64().unwrap_or_default() > 0 {
        window.set_message(format!("{} additional issues were omitted. Repair the reported issues and run checks again.",report["omitted"]).into());
    }
    let selected = window
        .get_selected_id()
        .parse::<usize>()
        .ok()
        .and_then(|index| session.map_check_report()?.result.issues.get(index));
    window.set_can_select(
        !stale
            && selected
                .is_some_and(|issue| issue.location.object.is_some() && !issue.location.ambiguous),
    );
    window.set_can_repair(!stale && selected.is_some_and(|issue| issue.fix.is_some()));
    window.set_detail_title(
        selected
            .map_or("Choose an issue", |issue| title(issue.code))
            .into(),
    );
    window.set_detail(
        selected
            .map_or("Select a result to see what needs attention.", |issue| {
                issue.message.as_str()
            })
            .into(),
    );
    window.set_location(
        selected
            .map_or_else(String::new, |issue| {
                format!(
                    "{}{}{}",
                    issue.location.path,
                    if issue.location.hidden {
                        "\nHidden content is excluded from compilation."
                    } else {
                        ""
                    },
                    if issue.location.ambiguous {
                        "\nIDs are ambiguous; selection and automatic repairs are unavailable."
                    } else {
                        ""
                    }
                )
            })
            .into(),
    );
    if let Some(draft) = session.map_fix_draft() {
        window.set_preview_token(draft.token.to_string().into());
        let text = draft
            .changes
            .iter()
            .map(describe_repair)
            .collect::<Vec<_>>()
            .join("\n\n");
        window.set_preview_text(text.into());
    } else {
        window.set_preview_token("".into());
        window.set_preview_text("".into());
    }
}

fn describe_repair(change: &serde_json::Value) -> String {
    let before = &change["before"];
    let after = &change["after"];
    let mut lines = vec![format!(
        "Brush {} · face {}",
        change["face"]["solid"], change["face"]["side"]
    )];
    let fields: &[(&str, &str)] = if !before["lightmap_scale"].is_null() {
        &[("Lightmap sample size", "lightmap_scale")]
    } else {
        &[
            ("Texture scale", "scale"),
            ("Shift", "shift"),
            ("Rotation", "rotation"),
            ("U axis", "u"),
            ("V axis", "v"),
        ]
    };
    for (label, key) in fields {
        lines.push(format!("{label}: {} → {}", before[key], after[key]));
    }
    lines.join("\n")
}

fn with_session<R>(
    window: &slint::Weak<ChecksWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&ChecksWindow, &mut Session) -> R,
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

pub fn bind_with_access(window: &ChecksWindow, access: &DocumentAccess) {
    window.on_run_checks({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                session.run_map_checks(Actor::User);
                window.set_page(0);
                window.set_message("".into());
                refresh(window, session);
            });
        }
    });
    window.on_filter({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| refresh(window, session));
        }
    });
    window.on_inspect({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |id| {
            with_session(&window, &access, |window, session| {
                window.set_selected_id(id);
                window.set_message("".into());
                refresh(window, session);
            });
        }
    });
    window.on_navigate({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |frame| {
            with_session(&window, &access, |window, session| {
                let result = window
                    .get_report_token()
                    .parse()
                    .map_err(|_| "Run map checks first".to_string())
                    .and_then(|report| {
                        window
                            .get_selected_id()
                            .parse()
                            .map_err(|_| "Choose an issue".to_string())
                            .and_then(|issue| {
                                session.select_map_issue(
                                    Actor::User,
                                    report,
                                    issue,
                                    frame,
                                    window.get_aspect(),
                                )
                            })
                    });
                refresh(window, session);
                window.set_message(
                    result
                        .map_or_else(
                            |error| error,
                            |()| {
                                if frame {
                                    "Issue selected and framed".into()
                                } else {
                                    "Issue selected".into()
                                }
                            },
                        )
                        .into(),
                );
            });
        }
    });
    window.on_review_repair({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let result = window
                    .get_report_token()
                    .parse()
                    .map_err(|_| "Run map checks first".to_string())
                    .and_then(|report| {
                        window
                            .get_selected_id()
                            .parse()
                            .map_err(|_| "Choose an issue".to_string())
                            .and_then(|issue| session.preview_map_fixes(report, &[issue]))
                    });
                refresh(window, session);
                window.set_message(result.err().unwrap_or_default().into());
            });
        }
    });
    window.on_apply_repair({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let result = window
                    .get_preview_token()
                    .parse()
                    .map_err(|_| "Review a repair first".to_string())
                    .and_then(|token| session.apply_map_fixes(Actor::User, token));
                refresh(window, session);
                window.set_message(
                    result
                        .map_or_else(
                            |error| error,
                            |count| {
                                format!(
                                    "Repaired {count} issue(s). Undo restores the previous values."
                                )
                            },
                        )
                        .into(),
                );
            });
        }
    });
    window.on_cancel_repair({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                let result = window
                    .get_preview_token()
                    .parse()
                    .map_err(|_| "Repair preview already closed".to_string())
                    .and_then(|token| session.cancel_map_fixes(token));
                refresh(window, session);
                window.set_message(result.err().unwrap_or_default().into());
            });
        }
    });
    window.on_cancel({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &access, |window, session| {
                if let Ok(token) = window.get_preview_token().parse() {
                    let _ = session.cancel_map_fixes(token);
                }
            });
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
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
