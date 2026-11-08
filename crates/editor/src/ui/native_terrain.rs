use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use editor_core::session::compiled_terrain_snapshot::Query;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    NativeTerrainRow, NativeTerrainWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Session},
};

const PAGE_ROWS: usize = 32;
const SOURCE_ROWS: usize = 1;
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
struct State {
    token: Option<u64>,
    snapshot: String,
    displacement: Option<u16>,
    vertex: Option<usize>,
    displacement_offset: usize,
    vertex_offset: usize,
    source_offset: usize,
    next_displacements: Option<usize>,
    next_vertices: Option<usize>,
    next_sources: Option<usize>,
    displacements: Vec<Value>,
    vertices: Vec<Value>,
    admitting: bool,
    message: Option<String>,
}

pub(super) struct Controller {
    pub(super) window: NativeTerrainWindow,
    target: DocumentContinuation,
    state: RefCell<State>,
    aspect: Cell<f32>,
    timer: slint::Timer,
}

#[derive(Clone, Copy)]
enum Action {
    Cancel,
    Clear,
    Frame,
    ShowGuide,
    ToggleGuide,
    ClearGuide,
}

#[derive(Clone, Copy)]
enum Page {
    Displacements,
    Vertices,
    Sources,
}

fn number(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|value| usize::try_from(value).ok())
}

fn displacement(value: &Value) -> Option<u16> {
    value["displacement"]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
}

fn token(value: &Value) -> Option<u64> {
    value.as_str().and_then(|value| value.parse().ok())
}

fn page_text(page: &Value) -> String {
    let offset = number(&page["offset"]).unwrap_or_default();
    let total = number(&page["total"]).unwrap_or_default();
    let shown = page["rows"].as_array().map_or(0, Vec::len);
    if shown == 0 {
        format!("0 of {total}")
    } else {
        format!("{}–{} of {total}", offset + 1, offset + shown)
    }
}

fn vector_state(value: &Value) -> &'static str {
    match value["state"].as_str() {
        Some("finite_nonzero") => "finite, nonzero",
        Some("zero") => "zero; no guide segment",
        Some("nonfinite") => "nonfinite; no guide segment",
        _ => "unavailable",
    }
}

fn component_values(value: &Value) -> String {
    let Some(bits) = value["bits"].as_array().filter(|bits| bits.len() == 3) else {
        return "unavailable".into();
    };
    bits.iter()
        .map(|bits| {
            bits.as_str()
                .and_then(|bits| bits.strip_prefix("0x"))
                .and_then(|bits| u32::from_str_radix(bits, 16).ok())
                .map(f32::from_bits)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unavailable".into())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn raw_vector(label: &str, value: &Value) -> String {
    let bits = value["bits"]
        .as_array()
        .map(|bits| {
            bits.iter()
                .map(|bits| bits.as_str().unwrap_or("unavailable"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "unavailable".into());
    let state = if label == "P" {
        value["state"].as_str().unwrap_or("unavailable")
    } else {
        vector_state(value)
    };
    format!(
        "{label}: {}\n{state}\nBits: {bits}",
        component_values(value)
    )
}

fn source_number(value: &Value, key: &str) -> String {
    value[key]
        .as_u64()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".into())
}

fn source_text(value: &Value) -> String {
    let solid = source_number(value, "solid");
    let side = source_number(value, "side");
    match value["kind"].as_str() {
        Some("root") => format!(
            "Root entity {} · solid {solid} · side {side}",
            source_number(value, "entity")
        ),
        Some("generated_region") => format!("Generated region · solid {solid} · side {side}"),
        Some("unresolved") => format!("Unresolved side {side}"),
        Some("instance") => {
            let route = value["occurrence"]
                .as_array()
                .map(|steps| {
                    steps
                        .iter()
                        .map(|step| {
                            format!(
                                "Helper {} → {}",
                                source_number(step, "helper"),
                                step["file"].as_str().unwrap_or("unavailable")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| "Occurrence route unavailable".into());
            format!(
                "Outer helper {} · solid {solid} · side {side}\nFile: {}\n{route}",
                source_number(value, "instance"),
                value["file"].as_str().unwrap_or("unavailable")
            )
        }
        _ => "Source contributor unavailable".into(),
    }
}

fn status_text(report: &Value, admitting: bool) -> &'static str {
    if admitting {
        return "Starting native observation…";
    }
    if report["busy"] == true && report["status"] != "preparing" {
        return "Finishing native cleanup…";
    }
    match report["status"].as_str() {
        Some("preparing") if report["cancel_requested"] == true => {
            "Cancelling native observation; waiting for confirmed cleanup."
        }
        Some("preparing") => "Preparing a complete native observation…",
        Some("ready") => "Native observation ready.",
        Some("stale") => "Historical observation; the build or runtime changed.",
        Some("failed") => "Native observation failed.",
        Some("cancelled") => "Native observation cancelled.",
        Some("cleared") => "Native observation cleared.",
        _ => "No native observation prepared.",
    }
}

impl Controller {
    fn message(&self, message: Option<String>) {
        self.window
            .set_message(message.as_deref().unwrap_or_default().into());
        self.state.borrow_mut().message = message;
    }

    fn with_session<R>(&self, body: impl FnOnce(&mut Session) -> Result<R, String>) -> Option<R> {
        match self
            .target
            .upgrade()
            .and_then(|access| access.with_session(body))
        {
            Ok(result) => Some(result),
            Err(error) => {
                self.message(Some(error));
                self.window.set_can_prepare(false);
                self.window.set_can_cancel(false);
                self.window.set_can_clear(false);
                self.window.set_can_frame(false);
                self.window.set_can_show_guide(false);
                self.window.set_can_toggle_guide(false);
                self.window.set_can_clear_guide(false);
                None
            }
        }
    }

    fn refresh(&self) {
        self.with_session(|session| self.present(session));
    }

    fn clear_rows(&self, state: &mut State) {
        state.displacements.clear();
        state.vertices.clear();
        state.next_displacements = None;
        state.next_vertices = None;
        state.next_sources = None;
        self.window.set_displacements(ModelRc::default());
        self.window.set_vertices(ModelRc::default());
        self.window.set_selected_displacement(-1);
        self.window.set_selected_vertex(-1);
        self.window.set_displacement_page("".into());
        self.window.set_vertex_page("".into());
        self.window.set_source_page("".into());
        self.window
            .set_selected_sample("Choose a native grid vertex.".into());
        self.window.set_raw_values("".into());
        self.window
            .set_sources("No source contributor page.".into());
        self.window.set_previous_displacements(false);
        self.window.set_next_displacements(false);
        self.window.set_previous_vertices(false);
        self.window.set_next_vertices(false);
        self.window.set_previous_sources(false);
        self.window.set_next_sources(false);
        self.window.set_can_frame(false);
        self.window.set_can_show_guide(false);
    }

    fn present(&self, session: &Session) -> Result<(), String> {
        let report = session.compiled_terrain_snapshot();
        let mut state = self.state.borrow_mut();
        let next_token = token(&report["token"]);
        if next_token != state.token {
            state.token = next_token;
            state.displacement = None;
            state.vertex = None;
            state.displacement_offset = 0;
            state.vertex_offset = 0;
            state.source_offset = 0;
            self.clear_rows(&mut state);
        }
        state.snapshot = report["snapshot"].as_str().unwrap_or_default().into();
        let busy = state.admitting || report["busy"] == true;
        self.window.set_busy(busy);
        self.window
            .set_can_prepare(report["prepare_available"] == true && !busy);
        self.window.set_can_cancel(
            report["status"] == "preparing"
                && report["cancel_requested"] != true
                && state.token.is_some(),
        );
        self.window.set_can_clear(state.token.is_some() && !busy);
        self.window
            .set_status(status_text(&report, state.admitting).into());
        self.window.set_summary(
            if let (Some(displacements), Some(samples)) =
                (number(&report["displacements"]), number(&report["samples"]))
            {
                format!(
                    "{displacements} displacements · {samples} native vertices · {} · build {}",
                    report["channel"]
                        .as_str()
                        .unwrap_or("unavailable")
                        .to_uppercase(),
                    report["build_token"].as_str().unwrap_or("unavailable")
                )
            } else {
                String::new()
            }
            .into(),
        );
        let mut availability = Vec::new();
        for reason in [
            report["current_error"].as_str(),
            report["cleanup_error"].as_str(),
            report["guide"]["suppression_reason"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            if !availability.contains(&reason) {
                availability.push(reason);
            }
        }
        if report["prepare_available"] != true
            && report["displacements"].is_null()
            && !busy
            && availability.is_empty()
        {
            availability
                .push("Prepare requires a saved current build and an available native client.");
        }
        self.window.set_availability(availability.join("\n").into());
        self.window.set_message(
            state
                .message
                .as_deref()
                .or_else(|| report["error"].as_str())
                .unwrap_or_default()
                .into(),
        );
        self.window.set_observation(
            format!(
                "Profile: {}\nBuild: {} · snapshot: {}\nChannel: {}\nElapsed: {} ms\nArtifacts: {}\n\nObserved final CPU fields. Shader pixels, VRAD lighting and runtime LOD frames are outside this observation.",
                report["profile"].as_str().unwrap_or("unavailable"),
                report["build_token"].as_str().unwrap_or("unavailable"),
                report["token"].as_str().unwrap_or("unavailable"),
                report["channel"].as_str().unwrap_or("unavailable"),
                report["elapsed_ms"].as_u64().map(|value| value.to_string()).unwrap_or_else(|| "unavailable".into()),
                report["artifacts"].as_str().unwrap_or("unavailable")
            )
            .into(),
        );
        self.present_guide(&report);
        let Some(token) = state.token.filter(|_| report["displacements"].is_number()) else {
            self.clear_rows(&mut state);
            return Ok(());
        };
        let catalog = session.inspect_compiled_terrain_snapshot(Query {
            token,
            displacement: None,
            offset: state.displacement_offset,
            limit: PAGE_ROWS,
            source_offset: 0,
            source_limit: SOURCE_ROWS,
        })?;
        let rows = catalog["page"]["rows"]
            .as_array()
            .ok_or("Native displacement page is unavailable")?;
        if state.displacements != *rows {
            state.displacements.clone_from(rows);
            self.window.set_displacements(ModelRc::new(VecModel::from(
                rows.iter()
                    .map(|row| NativeTerrainRow {
                        title: format!(
                            "Displacement {} · FACE {} · power {}",
                            row["displacement"], row["face"], row["power"]
                        )
                        .into(),
                        detail: format!(
                            "{} × {} · {} samples · {}",
                            row["grid_size"],
                            row["grid_size"],
                            row["samples"],
                            row["source_label"].as_str().unwrap_or("Source unavailable")
                        )
                        .into(),
                        state: "".into(),
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        state.next_displacements = number(&catalog["page"]["next_offset"]);
        self.window
            .set_displacement_page(page_text(&catalog["page"]).into());
        self.window
            .set_previous_displacements(state.displacement_offset > 0);
        self.window
            .set_next_displacements(state.next_displacements.is_some());
        if state.displacement.is_none() {
            state.displacement = rows.first().and_then(displacement);
        }
        self.window.set_selected_displacement(
            rows.iter()
                .position(|row| displacement(row) == state.displacement)
                .map_or(-1, |index| index as i32),
        );
        let Some(ordinal) = state.displacement else {
            return Ok(());
        };
        let samples = session.inspect_compiled_terrain_snapshot(Query {
            token,
            displacement: Some(ordinal),
            offset: state.vertex_offset,
            limit: PAGE_ROWS,
            source_offset: state.source_offset,
            source_limit: SOURCE_ROWS,
        })?;
        let rows = samples["page"]["rows"]
            .as_array()
            .ok_or("Native vertex page is unavailable")?;
        if state.vertices != *rows {
            state.vertices.clone_from(rows);
            self.window.set_vertices(ModelRc::new(VecModel::from(
                rows.iter()
                    .map(|row| NativeTerrainRow {
                        title: format!(
                            "Vertex {} · row {}, column {}",
                            row["vertex"], row["row"], row["column"]
                        )
                        .into(),
                        detail: format!("P: {}", component_values(&row["position"])).into(),
                        state: format!(
                            "N: {} · S: {} · T: {}",
                            row["normal"]["state"].as_str().unwrap_or("unavailable"),
                            row["tangent_s"]["state"].as_str().unwrap_or("unavailable"),
                            row["tangent_t"]["state"].as_str().unwrap_or("unavailable")
                        )
                        .into(),
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        if !rows
            .iter()
            .any(|row| number(&row["vertex"]) == state.vertex)
        {
            state.vertex = rows.first().and_then(|row| number(&row["vertex"]));
        }
        state.next_vertices = number(&samples["page"]["next_offset"]);
        self.window
            .set_vertex_page(page_text(&samples["page"]).into());
        self.window.set_previous_vertices(state.vertex_offset > 0);
        self.window.set_next_vertices(state.next_vertices.is_some());
        let sources = &samples["provenance"];
        let contributors = sources["contributors"]
            .as_array()
            .ok_or("Native source contributor page is unavailable")?;
        self.window.set_sources(
            contributors
                .iter()
                .map(source_text)
                .collect::<Vec<_>>()
                .join("\n\n")
                .into(),
        );
        let source_total = number(&sources["page"]["total"]).unwrap_or_default();
        self.window.set_source_page(
            if contributors.is_empty() {
                format!("0 of {source_total}")
            } else {
                format!("{} of {source_total}", state.source_offset + 1)
            }
            .into(),
        );
        state.next_sources = number(&sources["page"]["next_offset"]);
        self.window.set_previous_sources(state.source_offset > 0);
        self.window.set_next_sources(state.next_sources.is_some());
        self.present_sample(&state);
        Ok(())
    }

    fn present_guide(&self, report: &Value) {
        let guide = &report["guide"];
        let requested = guide["requested"] == true;
        let visible = guide["visible"] == true;
        self.window.set_guide_visible(visible);
        self.window.set_can_clear_guide(requested);
        self.window
            .set_can_toggle_guide(requested && (visible || guide["suppression_reason"].is_null()));
        self.window.set_guide_status(
            if requested {
                format!(
                    "Guide: displacement {} · vertex {} · {}. Available directions are normalized for display.",
                    guide["displacement"], guide["vertex"],
                    if guide["displayed"] == true { "shown" } else if visible { "suppressed" } else { "hidden" }
                )
            } else {
                "No vector guide. N is cyan, S is red and T is green.".into()
            }
            .into(),
        );
    }

    fn present_sample(&self, state: &State) {
        let chosen = state
            .vertices
            .iter()
            .position(|row| number(&row["vertex"]) == state.vertex);
        self.window
            .set_selected_vertex(chosen.map_or(-1, |index| index as i32));
        let Some(row) = chosen.and_then(|index| state.vertices.get(index)) else {
            self.window.set_can_frame(false);
            self.window.set_can_show_guide(false);
            self.window
                .set_selected_sample("Choose a native grid vertex.".into());
            self.window.set_raw_values("".into());
            return;
        };
        self.window.set_can_frame(row["frame_available"] == true);
        self.window
            .set_can_show_guide(row["guide_available"] == true);
        self.window.set_selected_sample(
            format!(
                "Displacement {} · FACE {} · vertex {} · row {}, column {}",
                row["displacement"], row["face"], row["vertex"], row["row"], row["column"]
            )
            .into(),
        );
        self.window.set_raw_values(
            [
                raw_vector("P", &row["position"]),
                raw_vector("N", &row["normal"]),
                raw_vector("S", &row["tangent_s"]),
                raw_vector("T", &row["tangent_t"]),
            ]
            .join("\n\n")
            .into(),
        );
    }

    fn choose(&self, page: Page, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        {
            let mut state = self.state.borrow_mut();
            match page {
                Page::Displacements => {
                    let Some(ordinal) = state.displacements.get(index).and_then(displacement)
                    else {
                        return;
                    };
                    state.displacement = Some(ordinal);
                    state.vertex = None;
                    state.vertex_offset = 0;
                    state.source_offset = 0;
                }
                Page::Vertices => {
                    state.vertex = state
                        .vertices
                        .get(index)
                        .and_then(|row| number(&row["vertex"]));
                }
                Page::Sources => return,
            }
            state.message = None;
        }
        self.refresh();
    }

    fn page(&self, page: Page, direction: i32) {
        {
            let mut state = self.state.borrow_mut();
            let (offset, next, step) = match page {
                Page::Displacements => (
                    state.displacement_offset,
                    state.next_displacements,
                    PAGE_ROWS,
                ),
                Page::Vertices => (state.vertex_offset, state.next_vertices, PAGE_ROWS),
                Page::Sources => (state.source_offset, state.next_sources, SOURCE_ROWS),
            };
            let offset = if direction > 0 {
                let Some(next) = next else {
                    return;
                };
                next
            } else {
                offset.saturating_sub(step)
            };
            match page {
                Page::Displacements => state.displacement_offset = offset,
                Page::Vertices => {
                    state.vertex_offset = offset;
                    state.vertex = None;
                }
                Page::Sources => state.source_offset = offset,
            }
            state.message = None;
        }
        self.refresh();
    }

    fn action(&self, action: Action) {
        let (token, ordinal, vertex) = {
            let state = self.state.borrow();
            (state.token, state.displacement, state.vertex)
        };
        let Some(token) = token else {
            return;
        };
        self.message(None);
        self.with_session(|session| {
            match action {
                Action::Cancel => session.cancel_compiled_terrain_snapshot(Actor::User, token),
                Action::Clear => session.clear_compiled_terrain_snapshot(Actor::User, token),
                Action::ToggleGuide => session.set_compiled_terrain_snapshot_visible(
                    Actor::User,
                    token,
                    !self.window.get_guide_visible(),
                ),
                Action::ClearGuide => {
                    session.clear_compiled_terrain_sample_guide(Actor::User, token)
                }
                Action::Frame | Action::ShowGuide => {
                    let ordinal = ordinal.ok_or("Choose a native displacement first")?;
                    let vertex = vertex.ok_or("Choose a native grid vertex first")?;
                    match action {
                        Action::Frame => session.frame_compiled_terrain_sample(
                            Actor::User,
                            token,
                            ordinal,
                            vertex,
                            self.aspect.get(),
                        ),
                        _ => session.set_compiled_terrain_sample_guide(
                            Actor::User,
                            token,
                            ordinal,
                            vertex,
                        ),
                    }
                }
            }
            .map(|_| ())
        });
        self.refresh();
    }

    fn prepare(&self) {
        let access = match self.target.upgrade() {
            Ok(access) => access,
            Err(error) => {
                self.message(Some(error));
                return;
            }
        };
        let snapshot = {
            let mut state = self.state.borrow_mut();
            if state.admitting || !self.window.get_can_prepare() {
                return;
            }
            state.admitting = true;
            state.message = None;
            state.snapshot.clone()
        };
        self.refresh();
        let window = self.window.as_weak();
        let spawned = std::thread::Builder::new()
            .name("native-terrain-admission".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    access.prepare_compiled_terrain_snapshot(Actor::User, snapshot)
                }))
                .unwrap_or_else(|_| Err("Native terrain observation admission panicked".into()));
                let error = result.err().unwrap_or_default();
                let _ = window.upgrade_in_event_loop(move |window| {
                    window.invoke_prepare_finished(error.into());
                });
            });
        if let Err(error) = spawned {
            self.state.borrow_mut().admitting = false;
            self.message(Some(format!(
                "Could not start native terrain observation: {error}"
            )));
            self.refresh();
        }
    }

    pub(super) fn new_with_access(
        access: &DocumentAccess,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: NativeTerrainWindow::new()?,
            target: access.downgrade(),
            state: RefCell::new(State::default()),
            aspect: Cell::new(1.0),
            timer: slint::Timer::default(),
        });
        controller.window.on_prepare_snapshot({
            let controller = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = controller.upgrade() {
                    controller.prepare();
                }
            }
        });
        controller.window.on_prepare_finished({
            let controller = Rc::downgrade(&controller);
            move |error| {
                if let Some(controller) = controller.upgrade() {
                    controller.state.borrow_mut().admitting = false;
                    controller.message((!error.is_empty()).then(|| error.to_string()));
                    controller.refresh();
                }
            }
        });
        for action in [
            Action::Cancel,
            Action::Clear,
            Action::Frame,
            Action::ShowGuide,
            Action::ToggleGuide,
            Action::ClearGuide,
        ] {
            let weak = Rc::downgrade(&controller);
            let callback = move || {
                if let Some(controller) = weak.upgrade() {
                    controller.action(action);
                }
            };
            match action {
                Action::Cancel => controller.window.on_cancel_snapshot(callback),
                Action::Clear => controller.window.on_clear_snapshot(callback),
                Action::Frame => controller.window.on_frame_sample(callback),
                Action::ShowGuide => controller.window.on_show_guide(callback),
                Action::ToggleGuide => controller.window.on_toggle_guide(callback),
                Action::ClearGuide => controller.window.on_clear_guide(callback),
            }
        }
        controller.window.on_choose_displacement({
            let weak = Rc::downgrade(&controller);
            move |index| {
                if let Some(controller) = weak.upgrade() {
                    controller.choose(Page::Displacements, index);
                }
            }
        });
        controller.window.on_choose_vertex({
            let weak = Rc::downgrade(&controller);
            move |index| {
                if let Some(controller) = weak.upgrade() {
                    controller.choose(Page::Vertices, index);
                }
            }
        });
        for page in [Page::Displacements, Page::Vertices, Page::Sources] {
            let weak = Rc::downgrade(&controller);
            let callback = move |direction| {
                if let Some(controller) = weak.upgrade() {
                    controller.page(page, direction);
                }
            };
            match page {
                Page::Displacements => controller.window.on_page_displacements(callback),
                Page::Vertices => controller.window.on_page_vertices(callback),
                Page::Sources => controller.window.on_page_sources(callback),
            }
        }
        controller.window.on_close_dialog({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade()
                    && let Err(error) = controller.window.hide()
                {
                    controller.message(Some(error.to_string()));
                }
            }
        });
        controller.window.window().on_close_requested({
            let window = controller.window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_close_dialog();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        controller
            .timer
            .start(slint::TimerMode::Repeated, REFRESH_INTERVAL, {
                let weak = Rc::downgrade(&controller);
                move || {
                    if let Some(controller) = weak.upgrade()
                        && controller.window.window().is_visible()
                    {
                        controller.refresh();
                    }
                }
            });
        Ok(controller)
    }

    pub(super) fn set_aspect(&self, aspect: f32) {
        if aspect.is_finite() && aspect > 0.0 {
            self.aspect.set(aspect);
        }
    }

    pub(super) fn show(&self, aspect: f32) -> Result<(), slint::PlatformError> {
        self.set_aspect(aspect);
        self.message(None);
        self.refresh();
        self.window.show()
    }

    pub(super) fn stop_timer(&self) {
        self.timer.stop();
    }

    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        super::hide_window("Native terrain", self.window.window(), errors);
    }
}
