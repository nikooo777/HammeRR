use std::{cell::RefCell, rc::Rc};

use editor_core::session::compiled_visibility::{
    GuideMode, Inspection, Kind, PositionSource, Query, Row, Target,
};
use editor_core::session::compiled_world_lights::Channel;
use serde_json::Value;
use slint::{ComponentHandle, Model};

use super::{CompileWindow, DocumentAccess, Session, lock};
use crate::{CompiledVisibilityWindow, session::Actor};

pub(super) struct Form {
    window: CompiledVisibilityWindow,
    state: Rc<RefCell<State>>,
}

impl Clone for Form {
    fn clone(&self) -> Self {
        Self {
            window: self.window.clone_strong(),
            state: self.state.clone(),
        }
    }
}
#[derive(Default)]
struct State {
    query: Query,
    report: Option<Inspection>,
    observation: Option<Value>,
}

#[derive(Clone, Copy)]
enum Action {
    Refresh,
    Inspect,
    Position,
    Camera,
    Rows(i32),
    Destinations(i32),
    Select(i32),
    Destination(i32),
    Frame,
    Show,
    ShowDestinations(Kind),
    Clear,
}

fn row_text(row: &Row) -> String {
    match row {
        Row::Leaf(row) => format!(
            "Leaf {} · cluster {} · contents {} · native bounds {:?} → {:?}",
            row.ordinal, row.cluster, row.contents, row.bounds[0], row.bounds[1]
        ),
        Row::Portal(row) => format!(
            "Portal {} · leaves {:?} · final clusters {:?}{} · {} vertices",
            row.ordinal,
            row.leaves,
            row.clusters,
            if row.internal_cluster {
                " · internal cluster"
            } else {
                ""
            },
            row.vertices
        ),
        Row::Cluster { ordinal, leaves } => format!("Cluster {ordinal} · {leaves} world leaves"),
    }
}

fn rows(
    values: impl Iterator<Item = String>,
) -> slint::ModelRc<slint::ModelRc<slint::language::StandardListViewItem>> {
    slint::ModelRc::new(slint::VecModel::from(
        values
            .map(|text| {
                slint::ModelRc::new(slint::VecModel::from(vec![
                    slint::language::StandardListViewItem::from(text.as_str()),
                ]))
            })
            .collect::<Vec<_>>(),
    ))
}

fn present(window: &CompiledVisibilityWindow, report: &Inspection) {
    window.set_current_row(-1);
    window.set_current_destination(-1);
    window.set_build_state(
        format!(
            "At last inspection: build {} · token {} · {} visibility{}",
            report.build["status"].as_str().unwrap_or("unavailable"),
            report.build["token"].as_str().unwrap_or("none"),
            report.build["visibility"].as_str().unwrap_or("unavailable"),
            if report.build["render_context"]["pending_preview"] == true {
                " · displayed drafts were not compiled"
            } else {
                ""
            }
        )
        .into(),
    );
    window.set_summary(report.summary.as_ref().map_or_else(|| "Compiler visibility metadata unavailable".into(), |summary| format!("{} world leaves · {} final clusters · {} retained portals · {} excluded exterior/solid portals", summary.world_leaves, summary.clusters, summary.portals, summary.excluded_portals)).into());
    window.set_world_light_result(
        report
            .world_light
            .as_ref()
            .map_or_else(String::new, |light| {
                format!(
                    "{} native light {} → stored source cluster {}",
                    match light.channel {
                        Channel::Ldr => "LDR",
                        Channel::Hdr => "HDR",
                    },
                    light.ordinal,
                    light.cluster
                )
            })
            .into(),
    );
    window.set_position_result(
        report
            .position
            .as_ref()
            .map_or_else(String::new, |position| {
                format!(
                    "{} {:?} → native f32 {:?}: {} · leaf {} · cluster {}{}",
                    if matches!(position.source, PositionSource::Camera {}) {
                        "Current camera position at inspection"
                    } else {
                        "World position"
                    },
                    position.requested_position,
                    position.effective_position,
                    if position.cluster < 0 {
                        "Solid/exterior; no PVS/PAS source row"
                    } else {
                        "Open clustered space"
                    },
                    position.native_leaf,
                    position.cluster,
                    if position.retained_world_leaf {
                        ""
                    } else {
                        " · reserved native leaf; no retained bounds or guide"
                    }
                )
            })
            .into(),
    );
    window.set_rows(rows(report.page.rows.iter().map(row_text)));
    window.set_visibility_rows(rows(
        report
            .visibility
            .rows
            .iter()
            .map(|ordinal| format!("Cluster {ordinal}")),
    ));
    window.set_page_label(
        format!(
            "Rows {}–{} of {} · select a row to inspect its leaf or portal",
            report.page.offset,
            report.page.offset + report.page.rows.len(),
            report
                .page
                .total
                .map_or_else(|| "unavailable".into(), |count| count.to_string())
        )
        .into(),
    );
    window.set_visibility_label(
        format!(
            "{} destinations {}–{} of {}",
            if report.kind == Kind::Pvs {
                "PVS"
            } else {
                "PAS"
            },
            report.visibility.offset,
            report.visibility.offset + report.visibility.rows.len(),
            report
                .visibility
                .total
                .map_or_else(|| "unavailable".into(), |count| count.to_string())
        )
        .into(),
    );
    window.set_previous_rows(report.page.total.is_some() && report.page.offset != 0);
    window.set_next_rows(report.page.next_offset.is_some());
    window.set_previous_destinations(
        report.visibility.total.is_some() && report.visibility.offset != 0,
    );
    window.set_next_destinations(report.visibility.next_offset.is_some());
    window.set_can_frame(report.current_bounds().is_ok());
    window.set_can_show_destinations(
        report.build["current"] == true
            && report.build["status"] == "current"
            && report.visibility.total.is_some()
            && matches!(
                report.target,
                Target::Leaf { .. } | Target::Cluster { .. } | Target::WorldLight { .. }
            ),
    );
    window.set_details(serde_json::to_string_pretty(&serde_json::json!({"target":report.target,"world_light":report.world_light,"selection":report.selection,"portal_vertices":report.selected_vertices,"position":report.position,"notes":report.notes})).unwrap_or_else(|error| error.to_string()).into());
    present_diagnostics(window, report);
    present_guide(window, &report.overlay);
    let (name, ordinal) = match report.target {
        Target::Summary {} => ("Summary", None),
        Target::Leaf { ordinal } => ("Leaf", Some(ordinal)),
        Target::Cluster { ordinal } => ("Cluster", Some(ordinal)),
        Target::Portal { ordinal } => ("Portal", Some(ordinal)),
        Target::Position { .. } => ("Position", None),
        Target::WorldLight { channel, ordinal } => {
            window.set_world_light_channel(
                match channel {
                    Channel::Ldr => "LDR",
                    Channel::Hdr => "HDR",
                }
                .into(),
            );
            ("World light", Some(ordinal))
        }
    };
    window.set_target_kind(name.into());
    if let Some(ordinal) = ordinal {
        window.set_ordinal(ordinal.to_string().into());
    }
    window.set_visibility_kind(
        match report.kind {
            Kind::Pvs => "PVS",
            Kind::Pas => "PAS",
        }
        .into(),
    );
}

fn present_diagnostics(window: &CompiledVisibilityWindow, report: &Inspection) {
    let diagnostics = report.build["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(report.diagnostics.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    window.set_diagnostics(diagnostics.into());
}

fn present_guide(window: &CompiledVisibilityWindow, overlay: &Value) {
    window.set_guide_state(
        if overlay["displayed"] == true {
            if overlay["mode"]["kind"] == "destinations" {
                let light = &overlay["world_light"];
                let source = light["channel"].as_str().map_or_else(String::new, |channel| {
                    format!(" · {} native light {}", channel.to_ascii_uppercase(), light["ordinal"])
                });
                format!("{} potential destination bounds: {} native world leaves across {} destination clusters from source cluster {}{} (cap {} leaves). Native bounds are not exact cells or current game occlusion.",
                    if overlay["mode"]["visibility"] == "pas" { "PAS" } else { "PVS" },
                    overlay["leaf_count"], overlay["destination_clusters"], overlay["source_cluster"], source, overlay["guide_leaf_cap"])
            } else {
                "Selected compiled visibility guide is displayed".into()
            }
        } else if overlay["requested"] == true {
            "Guide is suppressed because its owned build is stale or unavailable".into()
        } else {
            "No compiled visibility guide. Leaf boxes show native bounds, not exact convex cells.".into()
        }.into(),
    );
}

fn token(state: &State) -> Result<String, String> {
    state
        .report
        .as_ref()
        .and_then(|report| report.build["token"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| "Refresh inspection before choosing a compiled target".into())
}

fn from_controls(window: &CompiledVisibilityWindow) -> Result<Target, String> {
    if window.get_target_kind() == "Summary" {
        return Ok(Target::Summary {});
    }
    if window.get_target_kind() == "Position" {
        return Ok(Target::Position {
            source: input_position(window)?,
        });
    }
    let ordinal = window
        .get_ordinal()
        .trim()
        .parse::<usize>()
        .map_err(|_| "Ordinal must be a nonnegative integer")?;
    match window.get_target_kind().as_str() {
        "Leaf" => Ok(Target::Leaf { ordinal }),
        "Cluster" => Ok(Target::Cluster { ordinal }),
        "Portal" => Ok(Target::Portal { ordinal }),
        "World light" => Ok(Target::WorldLight {
            channel: match window.get_world_light_channel().as_str() {
                "LDR" => Channel::Ldr,
                "HDR" => Channel::Hdr,
                _ => return Err("Choose LDR or HDR for the native light".into()),
            },
            ordinal,
        }),
        _ => Err("Choose Summary, Leaf, Cluster, Portal, Position or World light".into()),
    }
}

fn input_position(window: &CompiledVisibilityWindow) -> Result<PositionSource, String> {
    let fields = [
        window.get_position_x(),
        window.get_position_y(),
        window.get_position_z(),
    ];
    let mut position = [0.0; 3];
    for (axis, text) in fields.iter().enumerate() {
        position[axis] = text
            .trim()
            .parse()
            .map_err(|_| format!("World position {} must be numeric", ["X", "Y", "Z"][axis]))?;
    }
    Ok(PositionSource::Position { position })
}

fn perform(
    window: &CompiledVisibilityWindow,
    session: &mut Session,
    state: &RefCell<State>,
    action: Action,
) -> Result<(), String> {
    if matches!(action, Action::Select(index) | Action::Destination(index) if index < 0) {
        return Ok(());
    }
    if matches!(action, Action::Clear) {
        session.set_compiled_visibility_overlay(Actor::User, None, None, GuideMode::default())?;
        if let Some(report) = &mut state.borrow_mut().report {
            report.overlay = session.compiled_visibility_overlay();
            present(window, report);
        }
        state.borrow_mut().observation = Some(session.compiled_visibility_observation());
        return Ok(());
    }
    let mut query = state.borrow().query.clone();
    match action {
        Action::Refresh => query = Query::default(),
        Action::Position | Action::Camera => {
            let source = if matches!(action, Action::Camera) {
                PositionSource::Camera {}
            } else {
                input_position(window)?
            };
            query = Query {
                build_token: Some(token(&state.borrow())?),
                target: Target::Position { source },
                kind: if window.get_visibility_kind() == "PAS" {
                    Kind::Pas
                } else {
                    Kind::Pvs
                },
                ..Query::default()
            };
        }
        Action::Inspect => {
            query.target = from_controls(window)?;
            query.build_token = Some(token(&state.borrow())?);
            query.offset = 0;
            query.visibility_offset = 0;
            query.kind = if window.get_visibility_kind() == "PAS" {
                Kind::Pas
            } else {
                Kind::Pvs
            };
        }
        Action::Rows(direction) | Action::Destinations(direction) => {
            let stored = state.borrow();
            let report = stored
                .report
                .as_ref()
                .ok_or("Refresh inspection before changing pages")?;
            query.build_token = Some(token(&stored)?);
            if matches!(action, Action::Rows(_)) {
                query.offset = if direction < 0 {
                    query.offset.saturating_sub(query.limit)
                } else {
                    report.page.next_offset.ok_or("No next visibility page")?
                };
            } else {
                query.visibility_offset = if direction < 0 {
                    query.visibility_offset.saturating_sub(query.limit)
                } else {
                    report
                        .visibility
                        .next_offset
                        .ok_or("No next destination page")?
                };
            }
        }
        Action::Select(index) | Action::Destination(index) => {
            if index < 0 {
                return Ok(());
            }
            let stored = state.borrow();
            let report = stored
                .report
                .as_ref()
                .ok_or("Refresh inspection before choosing a row")?;
            let target = if matches!(action, Action::Select(_)) {
                report.page.rows.get(index as usize).map(Row::target)
            } else {
                report
                    .visibility
                    .rows
                    .get(index as usize)
                    .map(|&ordinal| Target::Cluster { ordinal })
            }
            .ok_or("The selected visibility row is no longer retained")?;
            query = Query {
                build_token: Some(token(&stored)?),
                target,
                kind: query.kind,
                ..Query::default()
            };
        }
        Action::ShowDestinations(visibility) => {
            let stored = state.borrow();
            let target = stored
                .report
                .as_ref()
                .ok_or("Inspect a compiled leaf, cluster or native light first")?
                .target;
            let build_token = token(&stored)?;
            drop(stored);
            let result = session.set_compiled_visibility_overlay(
                Actor::User,
                Some(build_token),
                Some(target),
                GuideMode::Destinations { visibility },
            );
            let (overlay, error) = match result {
                Ok(overlay) => (overlay, None),
                Err(error) => (session.compiled_visibility_overlay(), Some(error)),
            };
            present_guide(window, &overlay);
            if let Some(report) = &mut state.borrow_mut().report {
                report.overlay = overlay;
                present_diagnostics(window, report);
            }
            if let Some(error) = error {
                window.set_diagnostics(error.into());
            } else {
                state.borrow_mut().observation = Some(session.compiled_visibility_observation());
            }
            return Ok(());
        }
        Action::Frame | Action::Show => {
            let stored = state.borrow();
            let target = stored
                .report
                .as_ref()
                .ok_or("Select a leaf or portal")?
                .target;
            let token = token(&stored)?;
            drop(stored);
            if matches!(action, Action::Frame) {
                session.frame_compiled_visibility(
                    Actor::User,
                    token,
                    target,
                    window.get_aspect(),
                )?;
            } else {
                session.set_compiled_visibility_overlay(
                    Actor::User,
                    Some(token),
                    Some(target),
                    GuideMode::default(),
                )?;
            }
        }
        Action::Clear => unreachable!(),
    }
    let report = session.inspect_compiled_visibility(&query)?;
    query.target = report.target;
    present(window, &report);
    *state.borrow_mut() = State {
        query,
        report: Some(report),
        observation: Some(session.compiled_visibility_observation()),
    };
    Ok(())
}

fn failure(window: &CompiledVisibilityWindow, state: &RefCell<State>, error: String) {
    *state.borrow_mut() = State::default();
    window.set_rows(slint::ModelRc::default());
    window.set_visibility_rows(slint::ModelRc::default());
    window.set_can_frame(false);
    window.set_can_show_destinations(false);
    window.set_previous_rows(false);
    window.set_next_rows(false);
    window.set_previous_destinations(false);
    window.set_next_destinations(false);
    window.set_build_state("Visibility inspection unavailable; refresh inspection".into());
    window.set_summary("".into());
    window.set_details("".into());
    window.set_position_result("".into());
    window.set_world_light_result("".into());
    window.set_page_label("".into());
    window.set_visibility_label("".into());
    window.set_diagnostics(error.into());
    window.set_guide_state("Guide state unavailable; refresh inspection or clear the guide".into());
}

fn handler(
    window: &CompiledVisibilityWindow,
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
            target.with_session(|session| perform(&window, session, &state, action(index)))
        });
        if let Err(error) = result {
            failure(&window, &state, error);
        }
    }
}

pub(super) fn bind(
    parent: &CompileWindow,
    access: &DocumentAccess,
) -> Result<Form, slint::PlatformError> {
    let window = CompiledVisibilityWindow::new()?;
    for visibility in [false, true] {
        let mut column = slint::language::TableColumn::default();
        column.title = if visibility {
            "Destination cluster"
        } else {
            "Native build record"
        }
        .into();
        column.width = 900.0;
        column.min_width = 720.0;
        let columns = slint::ModelRc::new(slint::VecModel::from(vec![column]));
        if visibility {
            window.set_visibility_columns(columns);
        } else {
            window.set_columns(columns);
        }
    }
    let state = Rc::new(RefCell::new(State::default()));
    parent.on_inspect_visibility({
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
                let result = perform(&window, &mut lock(shared), &state, Action::Refresh);
                if let Err(error) = result {
                    failure(&window, &state, error);
                }
                let _ = window.show();
            });
        }
    });
    let callback = handler(&window, access, &state, |_| Action::Refresh);
    window.on_refresh_inspection(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Inspect);
    window.on_inspect_target(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Position);
    window.on_inspect_position(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Camera);
    window.on_inspect_camera_position(move || callback(0));
    window.on_page_rows(handler(&window, access, &state, Action::Rows));
    window.on_page_destinations(handler(&window, access, &state, Action::Destinations));
    window.on_inspect_row(handler(&window, access, &state, Action::Select));
    window.on_inspect_destination(handler(&window, access, &state, Action::Destination));
    let callback = handler(&window, access, &state, |_| Action::Frame);
    window.on_frame_selection(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Show);
    window.on_show_guide(move || callback(0));
    let callback = handler(&window, access, &state, |_| {
        Action::ShowDestinations(Kind::Pvs)
    });
    window.on_show_pvs_bounds(move || callback(0));
    let callback = handler(&window, access, &state, |_| {
        Action::ShowDestinations(Kind::Pas)
    });
    window.on_show_pas_bounds(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Clear);
    window.on_clear_guide(move || callback(0));
    window.on_reset_sort({
        let window = window.as_weak();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            for columns in [window.get_columns(), window.get_visibility_columns()] {
                for (index, mut column) in columns.iter().enumerate() {
                    column.sort_order = slint::language::SortOrder::Unsorted;
                    columns.set_row_data(index, column);
                }
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
    pub(super) fn open_world_light(
        &self,
        session: &Session,
        build_token: String,
        channel: Channel,
        ordinal: usize,
    ) -> Result<(), String> {
        let query = Query {
            build_token: Some(build_token),
            target: Target::WorldLight { channel, ordinal },
            ..Query::default()
        };
        let report = session.inspect_compiled_visibility(&query)?;
        present(&self.window, &report);
        *self.state.borrow_mut() = State {
            query,
            report: Some(report),
            observation: Some(session.compiled_visibility_observation()),
        };
        self.window.show().map_err(|error| error.to_string())
    }

    pub(super) fn set_aspect(&self, aspect: f32) {
        self.window.set_aspect(aspect);
    }
    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }
    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Visibility inspection", self.window.window(), errors);
    }
    pub(super) fn refresh(&self, session: &Session) {
        if self.is_visible()
            && self.state.borrow().observation.as_ref()
                != Some(&session.compiled_visibility_observation())
        {
            failure(&self.window, &self.state, "Build/source/render context changed; refresh inspection. Historical output remains inspectable when intact.".into());
            self.state.borrow_mut().observation = Some(session.compiled_visibility_observation());
        }
    }
}
