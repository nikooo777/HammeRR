use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use editor_core::session::player_support_observation::{self as support, Input, Options, Query};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    PlayerSupportRow, PlayerSupportWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Session},
};

const PAGE_ROWS: usize = 32;
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
struct State {
    token: Option<u64>,
    snapshot: String,
    offset: usize,
    next: Option<usize>,
    selected: Option<usize>,
    rows: Vec<Value>,
    admitting: bool,
    current: bool,
    message: Option<String>,
}

pub(super) struct Controller {
    pub(super) window: PlayerSupportWindow,
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

fn number(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|value| usize::try_from(value).ok())
}

fn token(value: &Value) -> Option<u64> {
    value.as_str().and_then(|value| value.parse().ok())
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Number(value) if value.is_f64() => {
            let Some(value) = value.as_f64().filter(|value| value.is_finite()) else {
                return "unavailable".into();
            };
            if value == 0.0 {
                return "0".into();
            }
            let magnitude = value.abs();
            if !(0.0001..1_000_000.0).contains(&magnitude) {
                return format!("{value:.5e}");
            }
            let precision = (5.0 - magnitude.log10().floor()) as usize;
            let text = format!("{value:.precision$}");
            if text.contains('.') {
                text.trim_end_matches('0').trim_end_matches('.').into()
            } else {
                text
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        _ => "unavailable".into(),
    }
}

fn boolean(value: &Value) -> &'static str {
    match value.as_bool() {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unavailable",
    }
}

fn flag(value: &Value, mask: u64) -> &'static str {
    match value.as_u64() {
        Some(flags) if flags & mask != 0 => "set",
        Some(_) => "unset",
        None => "unavailable",
    }
}

fn hexadecimal(value: &Value) -> String {
    value
        .as_u64()
        .map(|value| format!("0x{value:08x}"))
        .unwrap_or_else(|| "unavailable".into())
}

fn input_label(value: &Value) -> String {
    value
        .as_str()
        .and_then(|value| value.parse::<Input>().ok())
        .map(|input| input.label().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

fn vector(value: &Value) -> String {
    let Some(values) = value.as_array().filter(|values| values.len() == 3) else {
        return "unavailable".into();
    };
    format!(
        "[{}]",
        values.iter().map(scalar).collect::<Vec<_>>().join(", ")
    )
}

fn status_text(report: &Value, admitting: bool) -> &'static str {
    if admitting {
        return "Starting player observation…";
    }
    if report["busy"] == true && report["status"] != "observing" {
        return "Finishing native cleanup…";
    }
    match report["status"].as_str() {
        Some("observing") if report["cancel_requested"] == true => {
            "Cancelling observation; waiting for confirmed cleanup."
        }
        Some("observing") => "Waiting for acknowledged input and native player samples…",
        Some("ready") => "Player observation ready.",
        Some("stale") => "Historical observation; the map, build or runtime changed.",
        Some("cancelled") => "Player observation cancelled.",
        Some("failed") => "Player observation failed.",
        Some("cleared") => "Player observation cleared.",
        _ => "No player observation retained.",
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
                self.window.set_can_start(false);
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

    fn present(&self, session: &Session) -> Result<(), String> {
        let report = session.player_support_observation();
        let mut state = self.state.borrow_mut();
        let next_token = token(&report["token"]);
        if next_token != state.token {
            state.token = next_token;
            state.offset = 0;
            state.selected = None;
        }
        state.snapshot = report["snapshot"].as_str().unwrap_or_default().into();
        state.current = report["current"] == true;
        let busy = state.admitting || report["busy"] == true;
        self.window.set_busy(busy);
        self.window
            .set_can_start(report["start_available"] == true && !busy);
        self.window.set_can_cancel(
            report["status"] == "observing"
                && report["cancel_requested"] != true
                && state.token.is_some(),
        );
        self.window.set_can_clear(state.token.is_some() && !busy);
        self.window
            .set_status(status_text(&report, state.admitting).into());
        self.window.set_summary(
            number(&report["samples"])
                .map(|count| {
                    format!(
                        "{count} retained samples · {}",
                        if state.current {
                            "current compiled map"
                        } else {
                            "historical compiled map"
                        }
                    )
                })
                .unwrap_or_default()
                .into(),
        );
        let mut reasons = Vec::new();
        for reason in [
            report["current_error"].as_str(),
            report["cleanup_error"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
        if report["start_available"] != true && state.token.is_none() && !busy && reasons.is_empty()
        {
            reasons.push("Start requires a saved current build and an available native client.");
        }
        self.window.set_availability(reasons.join("\n").into());
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
                "Profile: {}\nObservation: {}\nElapsed wall time: {} ms\nArtifacts: {}\n\nOne fresh private process observes the exact compiled BSP. Retained rows describe sampled native state; they do not attest a finished rendered frame.",
                report["observation"]["profile"].as_str().unwrap_or("unavailable"),
                report["token"].as_str().unwrap_or("unavailable"),
                scalar(&report["elapsed_ms"]),
                report["artifacts"].as_str().unwrap_or("unavailable"),
            )
            .into(),
        );
        self.present_guide(&report["guide"]);
        let rows = if let Some(token) = state.token.filter(|_| report["samples"].is_number()) {
            let page = session.inspect_player_support_observation(Query {
                token,
                offset: state.offset,
                limit: PAGE_ROWS,
            })?;
            let rows = page["page"]["rows"]
                .as_array()
                .ok_or("Player sample page is unavailable")?
                .clone();
            state.next = number(&page["page"]["next_offset"]);
            let total = number(&page["page"]["total"]).unwrap_or_default();
            self.window.set_page_label(
                if rows.is_empty() {
                    format!("0 of {total}")
                } else {
                    format!(
                        "{}–{} of {total}",
                        state.offset + 1,
                        state.offset + rows.len()
                    )
                }
                .into(),
            );
            rows
        } else {
            state.next = None;
            self.window.set_page_label("".into());
            Vec::new()
        };
        if state.rows != rows {
            if state.rows.is_empty() && !rows.is_empty() {
                self.window.set_settings_expanded(false);
            }
            self.window.set_samples(ModelRc::new(VecModel::from(
                rows.iter()
                    .map(|row| PlayerSupportRow {
                        title: format!(
                            "Sample {} · {}",
                            number(&row["ordinal"]).map_or_else(
                                || "unavailable".into(),
                                |ordinal| (ordinal + 1).to_string()
                            ),
                            if row["ground"].is_object() {
                                "grounded"
                            } else {
                                "airborne"
                            }
                        )
                        .into(),
                        detail: format!(
                            "Native {} s · origin {}",
                            scalar(&row["server_time"]),
                            vector(&row["origin"])
                        )
                        .into(),
                    })
                    .collect::<Vec<_>>(),
            )));
            state.rows = rows;
        }
        if !state
            .rows
            .iter()
            .any(|row| number(&row["ordinal"]) == state.selected)
        {
            state.selected = state.rows.first().and_then(|row| number(&row["ordinal"]));
        }
        self.window.set_previous_page(state.offset > 0);
        self.window.set_next_page(state.next.is_some());
        self.present_sample(&state);
        Ok(())
    }

    fn present_sample(&self, state: &State) {
        let selected = state
            .rows
            .iter()
            .position(|row| number(&row["ordinal"]) == state.selected);
        self.window
            .set_selected_row(selected.map_or(-1, |index| index as i32));
        let Some(row) = selected.and_then(|index| state.rows.get(index)) else {
            self.window.set_can_frame(false);
            self.window
                .set_sample_identity("Choose a retained sample.".into());
            self.window.set_motion("".into());
            self.window.set_hull("".into());
            self.window.set_support("".into());
            self.window.set_input_status("".into());
            self.window.set_probe_status("".into());
            self.window.set_probe_details("".into());
            self.window.set_can_show_guide(false);
            return;
        };
        self.window
            .set_can_frame(state.current && row["frame_available"] == true);
        self.window
            .set_can_show_guide(state.current && row["guide_available"] == true);
        self.window.set_input_status(
            format!(
                "Input: {} · acknowledged: {}\nButtons: {} · expected: {}",
                input_label(&row["input"]),
                boolean(&row["input_acknowledged"]),
                hexadecimal(&row["buttons"]),
                hexadecimal(&row["expected_buttons"])
            )
            .into(),
        );
        self.window.set_sample_identity(
            format!(
                "Sample {} · {}",
                state.selected.map_or(0, |ordinal| ordinal + 1),
                if state.current {
                    "current compiled map"
                } else {
                    "historical compiled map"
                }
            )
            .into(),
        );
        self.window.set_motion(
            format!(
                "Native time: {} s · server tick: {} · command tickbase: {}\nOrigin: {}\nLocal velocity: {}\nAbsolute velocity: {} · base velocity: {}",
                scalar(&row["server_time"]), scalar(&row["server_tick"]),
                scalar(&row["player_tickbase"]), vector(&row["origin"]),
                vector(&row["velocity"]), vector(&row["absolute_velocity"]),
                vector(&row["base_velocity"])
            )
            .into(),
        );
        self.window.set_hull(
            format!(
                "Native bounds: {} to {}\nCollision bounds: {} to {}\nDucked: {} · raw duck flag: {}\nModel scale: {} · raw flags: {} · health: {} · team: {}",
                vector(&row["hull_mins"]), vector(&row["hull_maxs"]),
                vector(&row["collision_mins"]), vector(&row["collision_maxs"]),
                boolean(&row["ducked"]), flag(&row["flags"], 2),
                scalar(&row["model_scale"]), hexadecimal(&row["flags"]),
                scalar(&row["health"]), scalar(&row["team"])
            )
            .into(),
        );
        let ground = &row["ground"];
        let surface = &row["surface"];
        let ground_text = if ground.is_object() {
            format!(
                "Ground: {} · entity {} · serial {}",
                ground["classname"].as_str().unwrap_or("unavailable"),
                scalar(&ground["index"]),
                scalar(&ground["serial"])
            )
        } else {
            "No current ground owner".into()
        };
        let surface_text = if surface.is_object() {
            format!(
                "{}: {} · index {}\nMovement friction: {} · material: {}",
                if ground.is_object() && row["surface_is_cached"] == false {
                    "Supporting surface"
                } else {
                    "Cached surface (previous support)"
                },
                surface["name"].as_str().unwrap_or("unavailable"),
                scalar(&surface["index"]),
                scalar(&surface["friction"]),
                scalar(&surface["material"])
            )
        } else {
            "No retained surface data.".into()
        };
        let ground_flag = flag(&row["flags"], 1);
        self.window.set_support(
            format!("{ground_text}\nOn-ground flag: {ground_flag}\n{surface_text}").into(),
        );
        self.present_probe(&row["probe"]);
    }

    fn present_guide(&self, guide: &Value) {
        let requested = guide["requested"] == true;
        let visible = guide["visible"] == true;
        self.window.set_guide_visible(visible);
        self.window.set_can_clear_guide(requested);
        self.window
            .set_can_toggle_guide(requested && (visible || guide["suppression_reason"].is_null()));
        self.window.set_guide_status(
            if requested {
                format!(
                    "Observed hull and trajectory · sample {} · {}{}",
                    number(&guide["index"])
                        .map_or_else(|| "unavailable".into(), |index| (index + 1).to_string()),
                    if guide["displayed"] == true {
                        "shown"
                    } else if visible {
                        "suppressed"
                    } else {
                        "hidden"
                    },
                    guide["suppression_reason"]
                        .as_str()
                        .map(|reason| format!("\n{reason}"))
                        .unwrap_or_default()
                )
            } else {
                "No sample guide. Guides show observed hulls, trajectory and any diagnostic sweep."
                    .into()
            }
            .into(),
        );
    }

    fn present_probe(&self, probe: &Value) {
        if !probe.is_object() {
            self.window.set_probe_status("".into());
            self.window.set_probe_details("".into());
            return;
        }
        let status = match probe["status"].as_str() {
            Some("miss") => "No hit",
            Some("impact") => "Impact",
            Some("start_solid") => "Starts solid",
            Some("all_solid") => "All solid",
            _ => "Unavailable",
        };
        self.window.set_probe_status(format!(
            "{status} · fraction: {}\nStart solid: {} · all solid: {} · left-solid fraction: {}",
            scalar(&probe["fraction"]), boolean(&probe["start_solid"]),
            boolean(&probe["all_solid"]), scalar(&probe["fraction_left_solid"])
        ).into());
        let plane = &probe["plane"];
        let plane = if plane["available"] == true {
            format!(
                "Plane normal: {} · distance: {}",
                vector(&plane["normal"]),
                scalar(&plane["distance"])
            )
        } else {
            "No valid impact plane.".into()
        };
        let entity = &probe["hit_entity"];
        let hit = if entity.is_object() {
            format!(
                "Hit owner: {} · entity {} · serial {}\nHit box: {} · hit group: {} · physics bone: {}",
                entity["classname"].as_str().unwrap_or("unavailable"),
                scalar(&entity["index"]),
                scalar(&entity["serial"]),
                scalar(&probe["hitbox"]),
                scalar(&probe["hit_group"]),
                scalar(&probe["physics_bone"])
            )
        } else {
            "No retained hit owner.".into()
        };
        let surface = &probe["surface"];
        let surface = if surface.is_object() {
            format!(
                "Surface property: {} · index {} · material {}\nPhysics friction coefficient: {}",
                surface["name"].as_str().unwrap_or("unavailable"),
                scalar(&surface["index"]),
                scalar(&surface["material"]),
                scalar(&surface["friction"])
            )
        } else {
            "No measured surface property.".into()
        };
        self.window.set_probe_details(format!(
            "Start: {}\nRequested end: {}\nNative start: {}\nHull reference endpoint: {}\nDistance: {} · mask: {} · collision group: {}\n{plane}\n{hit}\n{surface}\nCollision surface name unavailable.",
            vector(&probe["start"]), vector(&probe["requested_end"]), vector(&probe["native_start"]), vector(&probe["native_end"]),
            scalar(&probe["distance"]), hexadecimal(&probe["mask"]), scalar(&probe["collision_group"])
        ).into());
    }

    fn action(&self, action: Action) {
        let (token, selected) = {
            let state = self.state.borrow();
            (state.token, state.selected)
        };
        let Some(token) = token else { return };
        self.message(None);
        self.with_session(|session| {
            match action {
                Action::Cancel => session.cancel_player_support_observation(Actor::User, token),
                Action::Clear => session.clear_player_support_observation(Actor::User, token),
                Action::ShowGuide => session.set_player_support_sample_guide(
                    Actor::User,
                    token,
                    selected.ok_or("Choose a retained sample first")?,
                ),
                Action::ToggleGuide => session.set_player_support_sample_guide_visible(
                    Actor::User,
                    token,
                    !self.window.get_guide_visible(),
                ),
                Action::ClearGuide => session.clear_player_support_sample_guide(Actor::User, token),
                Action::Frame => session.frame_player_support_sample(
                    Actor::User,
                    token,
                    selected.ok_or("Choose a retained sample first")?,
                    self.aspect.get(),
                ),
            }
            .map(|_| ())
        });
        self.refresh();
    }

    fn start(&self) {
        let input = usize::try_from(self.window.get_input_index())
            .ok()
            .and_then(|index| Input::ALL.get(index).copied());
        let Some(input) = input else {
            self.message(Some("Choose a supported player input".into()));
            return;
        };
        let access = match self.target.upgrade() {
            Ok(access) => access,
            Err(error) => {
                self.message(Some(error));
                return;
            }
        };
        let snapshot = {
            let mut state = self.state.borrow_mut();
            if state.admitting || !self.window.get_can_start() {
                return;
            }
            state.admitting = true;
            state.message = None;
            state.snapshot.clone()
        };
        let options = Options {
            duration_ms: self.window.get_duration_ms() as u64,
            interval_ms: self.window.get_interval_ms() as u64,
            max_samples: self.window.get_sample_limit() as usize,
            team: if self.window.get_team_index() == 0 {
                3
            } else {
                2
            },
            input,
            probe_distance: self
                .window
                .get_probe_enabled()
                .then_some(self.window.get_probe_distance() as u16),
        };
        self.refresh();
        let window = self.window.as_weak();
        let spawned = std::thread::Builder::new()
            .name("player-support-admission".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    access.start_player_support_observation(Actor::User, snapshot, options)
                }))
                .unwrap_or_else(|_| Err("Player observation admission panicked".into()));
                let error = result.err().unwrap_or_default();
                let _ = window.upgrade_in_event_loop(move |window| {
                    window.invoke_start_finished(error.into());
                });
            });
        if let Err(error) = spawned {
            self.state.borrow_mut().admitting = false;
            self.message(Some(format!("Could not start player observation: {error}")));
            self.refresh();
        }
    }

    pub(super) fn new_with_access(
        access: &DocumentAccess,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            window: PlayerSupportWindow::new()?,
            target: access.downgrade(),
            state: RefCell::new(State::default()),
            aspect: Cell::new(1.0),
            timer: slint::Timer::default(),
        });
        let defaults = Options::default();
        controller.window.set_inputs(ModelRc::new(VecModel::from(
            Input::ALL
                .iter()
                .map(|input| input.label().into())
                .collect::<Vec<_>>(),
        )));
        controller.window.set_input_index(
            Input::ALL
                .iter()
                .position(|input| *input == defaults.input)
                .map_or(0, |index| index as i32),
        );
        controller
            .window
            .set_probe_enabled(defaults.probe_distance.is_some());
        controller
            .window
            .set_probe_distance(i32::from(defaults.probe_distance.unwrap_or(2)));
        controller
            .window
            .set_minimum_probe_distance(i32::from(support::MIN_PLAYER_SUPPORT_PROBE_DISTANCE));
        controller
            .window
            .set_maximum_probe_distance(i32::from(support::MAX_PLAYER_SUPPORT_PROBE_DISTANCE));
        controller
            .window
            .set_duration_ms(defaults.duration_ms as i32);
        controller
            .window
            .set_interval_ms(defaults.interval_ms as i32);
        controller
            .window
            .set_sample_limit(defaults.max_samples as i32);
        controller
            .window
            .set_team_index(if defaults.team == 3 { 0 } else { 1 });
        controller
            .window
            .set_minimum_duration(support::MIN_PLAYER_SUPPORT_DURATION_MS as i32);
        controller
            .window
            .set_maximum_duration(support::MAX_PLAYER_SUPPORT_DURATION_MS as i32);
        controller
            .window
            .set_minimum_interval(support::MIN_PLAYER_SUPPORT_INTERVAL_MS as i32);
        controller
            .window
            .set_maximum_interval(support::MAX_PLAYER_SUPPORT_INTERVAL_MS as i32);
        controller
            .window
            .set_minimum_samples(support::MIN_PLAYER_SUPPORT_SAMPLES as i32);
        controller
            .window
            .set_maximum_samples(support::MAX_PLAYER_SUPPORT_SAMPLES as i32);
        controller.window.on_start_observation({
            let weak = Rc::downgrade(&controller);
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.start();
                }
            }
        });
        controller.window.on_start_finished({
            let weak = Rc::downgrade(&controller);
            move |error| {
                if let Some(controller) = weak.upgrade() {
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
                Action::Cancel => controller.window.on_cancel_observation(callback),
                Action::Clear => controller.window.on_clear_observation(callback),
                Action::Frame => controller.window.on_frame_sample(callback),
                Action::ShowGuide => controller.window.on_show_guide(callback),
                Action::ToggleGuide => controller.window.on_toggle_guide(callback),
                Action::ClearGuide => controller.window.on_clear_guide(callback),
            }
        }
        controller.window.on_choose_sample({
            let weak = Rc::downgrade(&controller);
            move |index| {
                if let Some(controller) = weak.upgrade() {
                    {
                        let mut state = controller.state.borrow_mut();
                        state.selected = usize::try_from(index).ok().and_then(|index| {
                            state
                                .rows
                                .get(index)
                                .and_then(|row| number(&row["ordinal"]))
                        });
                    }
                    controller.refresh();
                }
            }
        });
        controller.window.on_page_samples({
            let weak = Rc::downgrade(&controller);
            move |direction| {
                if let Some(controller) = weak.upgrade() {
                    {
                        let mut state = controller.state.borrow_mut();
                        state.offset = if direction > 0 {
                            let Some(next) = state.next else { return };
                            next
                        } else {
                            state.offset.saturating_sub(PAGE_ROWS)
                        };
                        state.selected = None;
                    }
                    controller.refresh();
                }
            }
        });
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
        super::hide_window("Player support", self.window.window(), errors);
    }
}
