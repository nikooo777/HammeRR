use map_core::ObjectId;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::identity::{id_string, parse_id};
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{
    Actor, ConnectionBatchAction, ConnectionFields, ConnectionPatch, ConnectionRecord,
    ConnectionRowTarget, Session,
};
use crate::{BatchConnectionRow, ConnectionBatchWindow, ConnectionRow, ConnectionWindow};

fn source(window: &ConnectionWindow) -> Result<u32, String> {
    match parse_id(window.get_source_id().as_str())? {
        ObjectId::Entity(id) => Ok(id),
        _ => Err("Choose an entity".into()),
    }
}

fn guard(window: &ConnectionWindow, session: &Session) -> Result<(), String> {
    session.check_connection_snapshot(
        window
            .get_generation()
            .parse()
            .map_err(|_| "Reopen connections")?,
        window
            .get_revision()
            .parse()
            .map_err(|_| "Reopen connections")?,
    )
}

fn draft_guard(window: &ConnectionWindow, session: &Session) -> Result<(), String> {
    session.check_connection_draft(
        window
            .get_draft_token()
            .parse()
            .map_err(|_| "Choose a connection first")?,
    )
}

fn rows(records: Vec<ConnectionRecord>, incoming: bool) -> ModelRc<ConnectionRow> {
    ModelRc::new(VecModel::from(
        records
            .into_iter()
            .map(|record| {
                let (label, detail) = record
                    .spec
                    .map(|spec| {
                        let count = match spec.times {
                            -1 => "unlimited".into(),
                            1 => "once".into(),
                            times => format!("{times} times"),
                        };
                        (
                            format!("{} → {} · {}", spec.output, spec.target, spec.input),
                            format!(
                                "After {} s · {count}{}",
                                spec.delay,
                                if spec.parameter.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · parameter: {}", spec.parameter)
                                }
                            ),
                        )
                    })
                    .unwrap_or_else(|| {
                        (
                            format!("{} · imported connection", record.raw.output),
                            record.raw.value.replace('\u{1b}', " | "),
                        )
                    });
                ConnectionRow {
                    source: id_string(ObjectId::Entity(record.source)).into(),
                    index: record.index as i32,
                    label: if incoming {
                        format!("entity:{} · {label}", record.source).into()
                    } else {
                        label.into()
                    },
                    detail: detail.into(),
                    warning: record.warnings.join("\n").into(),
                }
            })
            .collect::<Vec<_>>(),
    ))
}

fn strings(values: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        std::iter::once("Suggestions".to_owned())
            .chain(values)
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

pub fn refresh(window: &ConnectionWindow, session: &Session) {
    if session
        .connection_draft()
        .is_some_and(|draft| draft.batch.is_some())
    {
        window.set_editing(false);
        window.set_can_apply(false);
        window.set_outgoing(ModelRc::default());
        window.set_incoming(ModelRc::default());
        window.set_source_label("Batch connections".into());
        window.set_message(
            "Use Batch connections to review this multi-row draft, or cancel it.".into(),
        );
        return;
    }
    if let Some(draft) = session.connection_draft() {
        window.set_source_id(id_string(ObjectId::Entity(draft.source)).into());
    }
    let Ok(source) = source(window) else {
        return;
    };
    window.set_generation(session.document_generation().to_string().into());
    window.set_revision(session.document().revision().to_string().into());
    let Some(entity) = session.document().entity(source) else {
        window.set_editing(false);
        window.set_can_apply(false);
        window.set_source_label("Entity no longer exists".into());
        window.set_outgoing(ModelRc::default());
        window.set_incoming(ModelRc::default());
        window.set_message("Cancel and select another entity.".into());
        return;
    };
    window.set_source_label(
        format!(
            "{} · {} · entity:{source}",
            entity
                .properties
                .get("targetname")
                .unwrap_or("Unnamed entity"),
            entity.classname().unwrap_or("unknown class")
        )
        .into(),
    );
    window.set_outgoing(rows(
        session.connection_records(source).unwrap_or_default(),
        false,
    ));
    window.set_incoming(rows(
        session.incoming_connections(source).unwrap_or_default(),
        true,
    ));
    let Some(draft) = session.connection_draft() else {
        window.set_editing(false);
        window.set_can_apply(false);
        window.set_message("Each applied connection is one undo step.".into());
        window.set_warnings("".into());
        return;
    };
    window.set_editing(true);
    window.set_draft_token(draft.token.to_string().into());
    let fields = &draft.fields;
    window.set_output(fields.output.clone().into());
    window.set_target(fields.target.clone().into());
    window.set_input(fields.input.clone().into());
    window.set_parameter(fields.parameter.clone().into());
    window.set_delay(fields.delay.clone().into());
    window.set_times(fields.times.clone().into());
    window.set_firing_mode(match fields.times.trim() {
        "-1" => 0,
        "1" => 1,
        _ => 2,
    });
    let outputs = session.connection_outputs(source);
    let inputs = session.connection_inputs(source, &fields.target);
    window.set_output_help(
        outputs
            .iter()
            .find(|io| io.name.eq_ignore_ascii_case(&fields.output))
            .map(|io| io.description.clone())
            .unwrap_or_default()
            .into(),
    );
    window.set_input_help(
        inputs
            .iter()
            .find(|io| io.name.eq_ignore_ascii_case(&fields.input))
            .map(|io| format!("{} · {}", io.kind, io.description))
            .unwrap_or_default()
            .into(),
    );
    window.set_outputs(strings(outputs.into_iter().map(|io| io.name)));
    window.set_inputs(strings(inputs.into_iter().map(|io| io.name)));
    let targets = session.connection_targets("");
    window.set_target_help(
        targets
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&fields.target))
            .map(|(_, description)| description.clone())
            .unwrap_or_default()
            .into(),
    );
    window.set_targets(strings(
        session
            .connection_targets(window.get_target_filter().as_str())
            .into_iter()
            .map(|(name, _)| name),
    ));
    let (error, warnings) = session.connection_draft_feedback();
    window.set_can_apply(error.is_none());
    window.set_message(error.unwrap_or_else(|| "Ready to apply".into()).into());
    window.set_warnings(warnings.join("\n\n").into());
}

fn batch_source_ids(window: &ConnectionBatchWindow) -> Result<Vec<u32>, String> {
    window
        .get_sources()
        .iter()
        .map(|id| match parse_id(id.as_str())? {
            ObjectId::Entity(id) => Ok(id),
            _ => Err("Captured connection sources must be whole entities".into()),
        })
        .collect()
}

fn clear_batch_capture(window: &ConnectionBatchWindow) {
    window.set_sources(ModelRc::default());
    window.set_snapshot("".into());
    window.set_rows(ModelRc::default());
    window.set_current_capture(false);
    window.set_can_apply(false);
    window.set_sources_label(
        "Select whole source entities, then choose Use selected sources.".into(),
    );
}

fn report_source_ids(report: &serde_json::Value) -> Result<Vec<u32>, String> {
    report["sources"]
        .as_array()
        .ok_or("The connection batch has no source list")?
        .iter()
        .map(|id| {
            match parse_id(
                id.as_str()
                    .ok_or("A connection batch source identity is malformed")?,
            )? {
                ObjectId::Entity(id) => Ok(id),
                _ => Err("Connection batch sources must be whole entities".to_owned()),
            }
        })
        .collect()
}

fn capture_report(window: &ConnectionBatchWindow, report: &serde_json::Value) {
    let sources = report["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(SharedString::from)
        .collect::<Vec<_>>();
    window.set_sources_label(
        format!(
            "Selected {} source entities: {}",
            sources.len(),
            sources
                .iter()
                .map(SharedString::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into(),
    );
    window.set_sources(ModelRc::new(VecModel::from(sources)));
    window.set_snapshot(report["snapshot"].as_str().unwrap_or_default().into());
    let selected = report["draft"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["identity"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    window.set_rows(ModelRc::new(VecModel::from(
        report["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                let source = row["source"].as_str().unwrap_or_default();
                let identity = row["identity"].as_str().unwrap_or_default();
                BatchConnectionRow {
                    source: source.into(),
                    identity: identity.into(),
                    selected: selected.contains(identity),
                    label: format!(
                        "{source} · row {} · {} → {} · {}",
                        row["index"],
                        row["fields"]["output"].as_str().unwrap_or_default(),
                        row["fields"]["target"].as_str().unwrap_or_default(),
                        row["fields"]["input"].as_str().unwrap_or_default()
                    )
                    .into(),
                    detail: format!(
                        "Raw: {}",
                        row["raw"]
                            .as_str()
                            .unwrap_or_default()
                            .replace('\u{1b}', " | ")
                    )
                    .into(),
                }
            })
            .collect::<Vec<_>>(),
    )));
}

pub fn load_batch_selection(window: &ConnectionBatchWindow, session: &Session) {
    if !session.connection_batch_report().is_null() {
        refresh_batch(window, session);
        return;
    }
    match session.connection_batch_state(None) {
        Ok(report) => {
            capture_report(window, &report);
            refresh_batch(window, session);
            window.set_message("Source entities selected. Add one connection per source, or highlight exact rows to edit or remove.".into());
        }
        Err(error) => {
            clear_batch_capture(window);
            refresh_batch(window, session);
            window.set_message(error.into());
        }
    }
}

pub fn refresh_batch(window: &ConnectionBatchWindow, session: &Session) {
    let report = session.connection_batch_report();
    window.set_has_plan(!report.is_null());
    window.set_current_capture(window.get_snapshot().as_str() == session.connection_batch_stamp());
    window.set_can_apply(report["can_apply"] == true);
    if report.is_null() {
        if !window.get_current_capture() && !window.get_snapshot().is_empty() {
            clear_batch_capture(window);
            window.set_message(
                "Map or definitions changed; select the source entities again.".into(),
            );
        }
        window.set_draft_token("".into());
        window.set_warnings("".into());
        return;
    }
    let current = report["generation"].as_u64() == Some(session.document_generation())
        && report["revision"].as_u64() == Some(session.document().revision())
        && report["definitions"].as_u64() == Some(session.definitions_serial());
    let rows = if current {
        report_source_ids(&report)
            .and_then(|sources| session.connection_batch_state(Some(&sources)))
    } else {
        Err(report["error"]
            .as_str()
            .unwrap_or(
                "The connection batch is stale; cancel it and select the source entities again",
            )
            .to_owned())
    };
    let row_error = match rows {
        Ok(state) => {
            capture_report(window, &state);
            None
        }
        Err(error) => {
            clear_batch_capture(window);
            Some(error)
        }
    };
    window.set_draft_token(report["token"].to_string().into());
    let add = report["action"] == "add";
    window.set_add_plan(add);
    window.set_remove_plan(report["action"] == "remove");
    window.set_summary(
        format!(
            "{} · {} exact affected rows across {} selected source entities",
            report["action"].as_str().unwrap_or("Connections"),
            report["affected_rows"],
            report["sources"].as_array().map_or(0, Vec::len)
        )
        .into(),
    );
    let text = |field: &str| {
        if add {
            &report["fields"][field]
        } else if report["patch"][field].is_string() {
            &report["patch"][field]
        } else {
            &report["common"][field]
        }
        .as_str()
        .unwrap_or_default()
        .to_owned()
    };
    window.set_output(text("output").into());
    window.set_target(text("target").into());
    window.set_input(text("input").into());
    window.set_parameter(text("parameter").into());
    window.set_delay(text("delay").into());
    window.set_times(text("times").into());
    window.set_replace_output(report["patch"]["output"].is_string());
    window.set_replace_target(report["patch"]["target"].is_string());
    window.set_replace_input(report["patch"]["input"].is_string());
    window.set_replace_parameter(report["patch"]["parameter"].is_string());
    window.set_replace_delay(report["patch"]["delay"].is_string());
    window.set_replace_times(report["patch"]["times"].is_string());
    window.set_mixed_output(!add && report["common"]["output"].is_null());
    window.set_mixed_target(!add && report["common"]["target"].is_null());
    window.set_mixed_input(!add && report["common"]["input"].is_null());
    window.set_mixed_parameter(!add && report["common"]["parameter"].is_null());
    window.set_mixed_delay(!add && report["common"]["delay"].is_null());
    window.set_mixed_times(!add && report["common"]["times"].is_null());
    window.set_warnings(
        report["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|warning| warning.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
    );
    window.set_message(
        row_error
            .as_deref()
            .or_else(|| report["error"].as_str())
            .unwrap_or(
                "Review every affected row and the per-source warnings, then Apply or Cancel plan.",
            )
            .into(),
    );
    if row_error.is_some() {
        window.set_can_apply(false);
    }
}

fn with_batch(
    window: &slint::Weak<ConnectionBatchWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&ConnectionBatchWindow, &mut Session) -> Result<(), String>,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = access.upgrade().and_then(|access| {
        access.with_session(|session| {
            let result = body(&window, session);
            refresh_batch(&window, session);
            result
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn begin_batch(
    window: &ConnectionBatchWindow,
    session: &mut Session,
    action: ConnectionBatchAction,
) -> Result<(), String> {
    let rows = window
        .get_rows()
        .iter()
        .filter(|row| row.selected)
        .map(|row| match parse_id(row.source.as_str())? {
            ObjectId::Entity(source) => Ok(ConnectionRowTarget {
                source,
                identity: row.identity.to_string(),
            }),
            _ => Err("Choose entity connection rows".to_owned()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let sources = batch_source_ids(window)?;
    session.begin_connection_batch(
        action,
        (action == ConnectionBatchAction::Add).then_some(sources.as_slice()),
        if action == ConnectionBatchAction::Add {
            &[]
        } else {
            &rows
        },
        window.get_snapshot().as_str(),
    )
}

pub fn bind_batch_with_access(window: &ConnectionBatchWindow, access: &DocumentAccess) {
    let access = access.downgrade();
    window.on_capture({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                load_batch_selection(window, session);
                Ok(())
            })
        }
    });
    window.on_toggle_row({
        let window = window.as_weak();
        move |identity| {
            if let Some(window) = window.upgrade() {
                let rows = window
                    .get_rows()
                    .iter()
                    .map(|mut row| {
                        if row.identity == identity {
                            row.selected = !row.selected;
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                window.set_rows(ModelRc::new(VecModel::from(rows)));
            }
        }
    });
    window.on_begin_add({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                begin_batch(window, session, ConnectionBatchAction::Add)
            })
        }
    });
    window.on_begin_edit({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                begin_batch(window, session, ConnectionBatchAction::Edit)
            })
        }
    });
    window.on_begin_remove({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                begin_batch(window, session, ConnectionBatchAction::Remove)
            })
        }
    });
    window.on_fields_changed({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                let token = window
                    .get_draft_token()
                    .parse()
                    .map_err(|_| "Capture a connection batch first")?;
                let add = window.get_add_plan();
                let patch = ConnectionPatch {
                    output: (add || window.get_replace_output())
                        .then(|| window.get_output().to_string()),
                    target: (add || window.get_replace_target())
                        .then(|| window.get_target().to_string()),
                    input: (add || window.get_replace_input())
                        .then(|| window.get_input().to_string()),
                    parameter: (add || window.get_replace_parameter())
                        .then(|| window.get_parameter().to_string()),
                    delay: (add || window.get_replace_delay())
                        .then(|| window.get_delay().to_string()),
                    times: (add || window.get_replace_times())
                        .then(|| window.get_times().to_string()),
                };
                session.update_connection_batch(token, patch)
            })
        }
    });
    window.on_apply({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                let token = window
                    .get_draft_token()
                    .parse()
                    .map_err(|_| "Capture a connection batch first")?;
                let report = session.commit_connection_batch(Actor::User, token)?;
                let rows = report_source_ids(&report).and_then(|sources|session.connection_batch_state(Some(&sources)));
                let row_error = match rows {
                    Ok(state) => { capture_report(window,&state); None },
                    Err(error) => { clear_batch_capture(window); Some(error) },
                };
                window.set_message(
                    format!(
                        "Applied {} connection rows. {}{}",
                        report["affected_rows"],
                        if report["changed"] == true {
                            "One Undo step."
                        } else {
                            "Values were unchanged; no history added."
                        },
                        row_error.map(|error|format!(" Row list refresh failed: {error}. Select the source entities again before another operation.")).unwrap_or_default()
                    )
                    .into(),
                );
                Ok(())
            })
        }
    });
    window.on_cancel_plan({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_batch(&window, &access, |window, session| {
                let token = window
                    .get_draft_token()
                    .parse()
                    .map_err(|_| "Capture a connection batch first")?;
                session.check_connection_draft(token)?;
                session.cancel_connection_edit();
                window.set_message("Connection plan cancelled; the map is unchanged.".into());
                Ok(())
            })
        }
    });
    window.on_close_window({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            let Some(retained) = window.upgrade() else {
                return;
            };
            let closed = access.upgrade().and_then(|access|access.with_session(|session| {
                let token = retained.get_draft_token();
                if !token.is_empty() {
                    let token = token.parse().map_err(|_|"The connection batch identity is invalid; refresh before closing")?;
                    session.check_connection_draft(token)?;
                    session.cancel_connection_edit();
                }
                refresh_batch(&retained,session);
                Ok(())
            }));
            match closed {
                Ok(()) => { if let Err(error)=retained.hide() {retained.set_message(format!("The connection plan is closed, but the window could not be hidden: {error}").into());} }
                Err(error) => retained.set_message(error.into()),
            }
        }
    });
    window.window().on_close_requested({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                window.invoke_close_window();
            }
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
}

fn result(window: &ConnectionWindow, session: &Session, result: Result<(), String>) {
    refresh(window, session);
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn with_connection<R>(
    window: &slint::Weak<ConnectionWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&ConnectionWindow, &mut Session) -> R,
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

pub fn bind_with_access(window: &ConnectionWindow, access: &DocumentAccess) {
    let access = access.downgrade();
    window.on_add({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_connection(&window, &access, |window, session| {
                let edit = guard(window, session)
                    .and_then(|()| session.begin_connection_edit(source(window)?, None));
                result(window, session, edit);
            });
        }
    });
    window.on_choose({
        let (window, access) = (window.as_weak(), access.clone());
        move |id, index| {
            with_connection(&window, &access, |window, session| {
                let edit = guard(window, session).and_then(|()| match parse_id(id.as_str())? {
                    ObjectId::Entity(source) => session.begin_connection_edit(
                        source,
                        Some(index.try_into().map_err(|_| "Invalid connection index")?),
                    ),
                    _ => Err("Choose an entity".into()),
                });
                result(window, session, edit);
            });
        }
    });
    window.on_delete_connection({
        let (window, access) = (window.as_weak(), access.clone());
        move |index| {
            with_connection(&window, &access, |window, session| {
                let edit = guard(window, session).and_then(|()| {
                    session.delete_entity_connection(
                        Actor::User,
                        source(window)?,
                        index.try_into().map_err(|_| "Invalid connection index")?,
                    )
                });
                result(window, session, edit);
            });
        }
    });
    window.on_fields_changed({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_connection(&window, &access, |window, session| {
                let edit = draft_guard(window, session).and_then(|()| {
                    session.update_connection_draft(ConnectionFields {
                        output: window.get_output().into(),
                        target: window.get_target().into(),
                        input: window.get_input().into(),
                        parameter: window.get_parameter().into(),
                        delay: window.get_delay().into(),
                        times: window.get_times().into(),
                    })
                });
                result(window, session, edit);
            });
        }
    });
    window.on_filter_changed({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_connection(&window, &access, |window, session| refresh(window, session));
        }
    });
    window.on_apply({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_connection(&window, &access, |window, session| {
                let edit = draft_guard(window, session)
                    .and_then(|()| session.commit_connection_edit(Actor::User).map(|_| ()));
                result(window, session, edit);
            });
        }
    });
    window.on_cancel({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            let Some(origin) = window.upgrade() else {
                return;
            };
            with_connection(&window, &access, |window, session| {
                if draft_guard(window, session).is_ok() {
                    session.cancel_connection_edit();
                }
            });
            let _ = origin.hide();
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
