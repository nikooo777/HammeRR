use map_core::ObjectId;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::identity::{id_string, parse_id};
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, ConnectionFields, ConnectionRecord, Session};
use crate::{ConnectionRow, ConnectionWindow};

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
