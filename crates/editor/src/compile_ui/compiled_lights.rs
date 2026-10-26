use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::compiled_world_lights::{Channel, Inspection, Query, WorldLightRow};
use serde_json::Value;
use slint::{ComponentHandle, Model};

use super::{CompileWindow, DocumentAccess, Session, lock};
use crate::{CompiledLightDetails, CompiledLightsWindow};

pub(super) struct Form {
    window: CompiledLightsWindow,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    observation: Option<Value>,
    report: Option<Inspection>,
    query: Query,
}

#[derive(Clone, Copy)]
enum Action {
    Refresh,
    Channel,
    Page(i32),
    Select(i32),
}

fn scalar(value: f32) -> String {
    let magnitude = value.abs();
    if magnitude != 0.0 && !(0.0001..1_000_000.0).contains(&magnitude) {
        format!("{value:e}")
    } else {
        value.to_string()
    }
}

fn vector(value: [f32; 3]) -> String {
    value.map(scalar).join(", ")
}

fn build_text(build: &Value) -> String {
    let lighting = build["lighting"]
        .as_str()
        .map(|raw| {
            compiler::LightingMode::parse(raw)
                .map(compiler::LightingMode::label)
                .unwrap_or(raw)
        })
        .unwrap_or("no captured lighting");
    format!(
        "At last inspection: build {} · token {} · {}{}",
        build["status"].as_str().unwrap_or("unavailable"),
        build["token"].as_str().unwrap_or("none"),
        lighting,
        if build["render_context"]["pending_preview"] == true {
            "\nThe displayed geometry draft was not baked."
        } else {
            ""
        }
    )
}

fn clear(window: &CompiledLightsWindow, state: &RefCell<State>) {
    *state.borrow_mut() = State::default();
    window.set_light_rows(slint::ModelRc::default());
    window.set_current_row(-1);
    window.set_has_selection(false);
    window.set_can_frame(false);
    window.set_details(CompiledLightDetails::default());
    window.set_light_page("No retained light page".into());
    window.set_channels("".into());
    window.set_previous_lights(false);
    window.set_next_lights(false);
}

fn association_text(row: &WorldLightRow, historical: bool, compact: bool) -> String {
    use compiler::world_light_origins::Origin;
    use editor_core::session::compiled_world_lights::SourceAssociation;
    let prefix = if historical {
        "Historical source: "
    } else {
        "Captured source: "
    };
    match &row.source_association {
        SourceAssociation::Available {
            origin: Origin::Root { entity },
        } => {
            if compact {
                format!("entity:{entity}")
            } else {
                format!("{prefix}root entity:{entity}")
            }
        }
        SourceAssociation::Available {
            origin:
                Origin::Instance {
                    instance,
                    file,
                    entity,
                    expanded_entity,
                    occurrence,
                },
        } => {
            if compact {
                format!("instance:{instance} / entity:{entity}")
            } else {
                let route = occurrence.label();
                format!(
                    "{prefix}outer helper entity:{instance}\nOccurrence: {route}\nLeaf file: {file}\nOriginal entity:{entity} · expanded entity:{expanded_entity} (this build)"
                )
            }
        }
        SourceAssociation::Unavailable { reason } => {
            if compact {
                "Unavailable".into()
            } else {
                format!("Unavailable: {reason}")
            }
        }
    }
}

fn details(window: &CompiledLightsWindow, row: &WorldLightRow, report: &Inspection) {
    window.set_details(CompiledLightDetails {
        record: row.ordinal.to_string().into(),
        source_association: association_text(row, report.historical, false).into(),
        kind: format!("{} ({})", row.kind_name, row.kind).into(),
        style: row.style.to_string().into(),
        origin: vector(row.origin).into(),
        intensity: vector(row.intensity).into(),
        normal: vector(row.normal).into(),
        cluster: row.cluster.to_string().into(),
        stop_dot: scalar(row.stop_dot).into(),
        stop_dot2: scalar(row.stop_dot2).into(),
        exponent: scalar(row.exponent).into(),
        radius: scalar(row.radius).into(),
        constant_attenuation: scalar(row.constant_attenuation).into(),
        linear_attenuation: scalar(row.linear_attenuation).into(),
        quadratic_attenuation: scalar(row.quadratic_attenuation).into(),
        flags: format!("{} / 0x{:08x}", row.flags, row.flags as u32).into(),
        texinfo: row.texinfo.to_string().into(),
        owner: row.owner.to_string().into(),
    });
    window.set_has_selection(true);
    window.set_can_frame(report.frame_origin(row.ordinal).is_ok());
}

fn present(window: &CompiledLightsWindow, report: &Inspection) {
    window.set_build_state(build_text(&report.build).into());
    window.set_channels(
        report
            .channels
            .iter()
            .map(|channel| {
                format!(
                    "{}: {}{}",
                    channel.channel.to_ascii_uppercase(),
                    channel.status.replace('_', " "),
                    match (channel.count, channel.bytes) {
                        (Some(count), Some(bytes)) => format!(" · {count} records · {bytes} bytes"),
                        _ => String::new(),
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("    ")
            .into(),
    );
    let rows = report
        .page
        .rows
        .iter()
        .map(|row| {
            let cells = [
                row.ordinal.to_string(),
                format!("{} ({})", row.kind_name, row.kind),
                row.style.to_string(),
                vector(row.origin),
                vector(row.intensity),
                row.flags.to_string(),
                association_text(row, report.historical, true),
            ];
            let cells = cells
                .iter()
                .map(|text| slint::language::StandardListViewItem::from(text.as_str()))
                .collect::<Vec<_>>();
            slint::ModelRc::new(slint::VecModel::from(cells))
        })
        .collect::<Vec<_>>();
    window.set_light_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    window.set_current_row(-1);
    window.set_has_selection(false);
    window.set_can_frame(false);
    window.set_details(CompiledLightDetails::default());
    window.set_light_page(
        match report.page.total {
            Some(total) => format!(
                "{} records from {} · {total} total",
                report.page.returned, report.page.offset
            ),
            None => "No available light table".into(),
        }
        .into(),
    );
    window.set_previous_lights(report.page.total.is_some() && report.page.offset > 0);
    window.set_next_lights(report.page.next_offset.is_some());
    let mut diagnostics = report
        .diagnostics
        .iter()
        .map(String::as_str)
        .chain(
            [
                &report.build["diagnostics"],
                &report.build["render_context"]["diagnostics"],
            ]
            .into_iter()
            .flat_map(|value| value.as_array().into_iter().flatten())
            .filter_map(Value::as_str),
        )
        .collect::<Vec<_>>()
        .join("\n");
    if report.diagnostics_truncated || report.build["diagnostics_truncated"] == true {
        diagnostics.push_str("\nAdditional diagnostics were truncated by the inspection limits.");
    }
    window.set_diagnostics(diagnostics.into());
}

fn current(window: &CompiledLightsWindow, session: &Session, state: &RefCell<State>) -> bool {
    let observation = session.compiled_world_light_observation();
    if state.borrow().report.is_some() && state.borrow().observation.as_ref() == Some(&observation)
    {
        return true;
    }
    clear(window, state);
    window.set_build_state("Inspection needs refresh".into());
    window.set_diagnostics(
        "Build, document or settings changed. Refresh inspection before using retained records."
            .into(),
    );
    state.borrow_mut().observation = Some(observation);
    false
}

fn inspect(
    window: &CompiledLightsWindow,
    session: &Session,
    state: &RefCell<State>,
    action: Action,
) {
    if !matches!(action, Action::Refresh) && !current(window, session, state) {
        return;
    }
    let selected = if let Action::Select(index) = action {
        let state = state.borrow();
        let row = usize::try_from(index)
            .ok()
            .and_then(|index| Some((index, state.report.as_ref()?.page.rows.get(index)?.ordinal)));
        if row.is_none() {
            window.set_current_row(-1);
            window.set_has_selection(false);
            window.set_can_frame(false);
            window.set_details(CompiledLightDetails::default());
            return;
        }
        row
    } else {
        None
    };
    let mut query = state.borrow().query.clone();
    match action {
        Action::Refresh | Action::Channel => {
            let channel = match Channel::parse(&window.get_channel().to_ascii_lowercase()) {
                Ok(channel) => channel,
                Err(error) => {
                    clear(window, state);
                    window.set_diagnostics(error.into());
                    return;
                }
            };
            query.channel = channel;
            query.offset = 0;
            if matches!(action, Action::Refresh) {
                query.build_token = None;
            }
        }
        Action::Page(direction) => {
            let next = state.borrow().report.as_ref().and_then(|report| {
                if direction > 0 {
                    report.page.next_offset
                } else {
                    report
                        .page
                        .total
                        .map(|_| report.page.offset.saturating_sub(query.limit))
                }
            });
            let Some(offset) = next else {
                return;
            };
            query.offset = offset;
        }
        Action::Select(_) => {}
    }
    match session.inspect_compiled_world_lights(&query) {
        Ok(report) => {
            query.build_token = report.build["token"].as_str().map(str::to_owned);
            present(window, &report);
            if let Some((index, ordinal)) = selected
                && let Some(row) = report.page.rows.get(index)
                && row.ordinal == ordinal
            {
                window.set_current_row(index as i32);
                details(window, row, &report);
            }
            *state.borrow_mut() = State {
                observation: Some(session.compiled_world_light_observation()),
                report: Some(report),
                query,
            };
        }
        Err(error) => {
            clear(window, state);
            let observation = session.compiled_world_light_observation();
            window.set_build_state("Inspection failed; refresh inspection".into());
            window.set_diagnostics(error.into());
            state.borrow_mut().observation = Some(observation);
        }
    }
}

fn frame_light(
    window: &CompiledLightsWindow,
    session: &mut Session,
    state: &RefCell<State>,
) -> Result<(), String> {
    if !current(window, session, state) {
        return Err("Refresh inspection before framing a compiled light".into());
    }
    let (token, channel, ordinal) = {
        let state = state.borrow();
        let report = state
            .report
            .as_ref()
            .ok_or("Inspect compiled lights first")?;
        let index = usize::try_from(window.get_current_row())
            .map_err(|_| "Select a compiled point or spot light")?;
        let row = report
            .page
            .rows
            .get(index)
            .ok_or("Select a compiled point or spot light")?;
        let token = state
            .query
            .build_token
            .clone()
            .ok_or("Refresh the owned build inspection first")?;
        (token, state.query.channel, row.ordinal)
    };
    let framed = session.frame_compiled_world_light(
        crate::session::Actor::User,
        token,
        channel,
        ordinal,
        window.get_aspect(),
    )?;
    window.set_diagnostics(
        format!(
            "Framed {} record {} at its stored native origin. Map selection is unchanged.",
            framed.inspection.channel.to_ascii_uppercase(),
            ordinal
        )
        .into(),
    );
    Ok(())
}

fn handler(
    window: &CompiledLightsWindow,
    access: &DocumentAccess,
    state: &Rc<RefCell<State>>,
    action: impl Fn(i32) -> Action + 'static,
) -> impl Fn(i32) + 'static {
    let (window, target, state) = (window.as_weak(), access.downgrade(), state.clone());
    move |index| {
        let Some(window) = window.upgrade() else {
            return;
        };
        let result = target.upgrade().and_then(|target| {
            target.with_shared(|shared| {
                inspect(&window, &lock(shared), &state, action(index));
                Ok(())
            })
        });
        if let Err(error) = result {
            clear(&window, &state);
            window.set_build_state("Build unavailable".into());
            window.set_diagnostics(error.into());
        }
    }
}

pub(super) fn bind(
    parent: &CompileWindow,
    access: &DocumentAccess,
) -> Result<Form, slint::PlatformError> {
    let window = CompiledLightsWindow::new()?;
    let columns = [
        ("Record", 100.0),
        ("Kind", 125.0),
        ("Style", 65.0),
        ("Origin", 195.0),
        ("Intensity", 195.0),
        ("Flags", 80.0),
        ("Source", 220.0),
    ]
    .map(|(title, width)| {
        let mut column = slint::language::TableColumn::default();
        column.title = title.into();
        column.width = width;
        column.min_width = width;
        column
    });
    window.set_light_columns(slint::ModelRc::new(slint::VecModel::from(columns.to_vec())));
    let state = Rc::new(RefCell::new(State::default()));
    parent.on_inspect_compiled_lights({
        let (window, target, state, parent) = (
            window.as_weak(),
            access.downgrade(),
            state.clone(),
            parent.as_weak(),
        );
        move || {
            super::admitted(&parent, &target, |_, shared| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                inspect(&window, &lock(shared), &state, Action::Refresh);
                let _ = window.show();
            });
        }
    });
    let refresh = handler(&window, access, &state, |_| Action::Refresh);
    window.on_refresh_inspection(move || refresh(0));
    let channel = handler(&window, access, &state, |_| Action::Channel);
    window.on_change_channel(move || channel(0));
    window.on_page_lights(handler(&window, access, &state, Action::Page));
    window.on_inspect_light(handler(&window, access, &state, Action::Select));
    window.on_frame_light({
        let (window, target, state) = (window.as_weak(), access.downgrade(), state.clone());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let result = target.upgrade().and_then(|target| {
                target.with_session(|session| frame_light(&window, session, &state))
            });
            if let Err(error) = result {
                clear(&window, &state);
                window.set_build_state("Framing unavailable; refresh inspection".into());
                window.set_diagnostics(error.into());
            }
        }
    });
    window.on_reset_sort({
        let window = window.as_weak();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let columns = window.get_light_columns();
            for (index, mut column) in columns.iter().enumerate() {
                column.sort_order = slint::language::SortOrder::Unsorted;
                columns.set_row_data(index, column);
            }
        }
    });
    window.on_close_dialog({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
        }
    });
    Ok(Form { window, state })
}

impl Form {
    pub(super) fn set_aspect(&self, aspect: f32) {
        self.window.set_aspect(aspect);
    }

    #[cfg(test)]
    pub(super) fn window(&self) -> &CompiledLightsWindow {
        &self.window
    }

    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Compiled lights", self.window.window(), errors);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    pub(super) fn refresh(&self, session: &Session) {
        if self.is_visible()
            && self.state.borrow().observation.as_ref()
                != Some(&session.compiled_world_light_observation())
        {
            current(&self.window, session, &self.state);
        }
    }
}

#[cfg(test)]
mod tests;
