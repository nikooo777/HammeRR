use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::FogWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation, DocumentLifecycle};
use crate::session::fog::{Channel, Command, InitialRadialContext, SelectionMode, Settings};
use crate::session::{Actor, Session};

const REPORT_INTERVAL: Duration = Duration::from_millis(125);

#[derive(Default)]
struct Form {
    snapshot: String,
    selection_snapshot: Option<String>,
    input_snapshot: Option<String>,
    player_snapshot: Option<String>,
    time_snapshot: Option<String>,
    choices: Vec<Value>,
    channels: Vec<Value>,
    selections: Vec<Value>,
    initial_rules: Vec<Value>,
    inputs: Vec<Value>,
    input_targets: Vec<Value>,
    player_targets: Vec<Value>,
    controller_labels: Vec<slint::SharedString>,
    channel_labels: Vec<slint::SharedString>,
    selection_labels: Vec<slint::SharedString>,
    initial_rule_labels: Vec<slint::SharedString>,
    input_labels: Vec<slint::SharedString>,
    input_target_labels: Vec<slint::SharedString>,
    player_labels: Vec<slint::SharedString>,
    report: Value,
    action_error: Option<String>,
    seen: Option<(String, u64, u64, u64)>,
    refreshed: Option<Instant>,
}

pub struct Controller {
    pub window: FogWindow,
    form: Rc<RefCell<Form>>,
}

#[derive(Clone, Copy)]
enum Action {
    Apply,
    Playback,
    Reset,
    Seek,
    Scrub(f64),
    Input,
    PrepareEnvironment,
    Cancel,
    Clear,
    SwitchPlayer,
}

fn selected(choices: &[Value], index: i32) -> Option<&Value> {
    usize::try_from(index)
        .ok()
        .and_then(|index| choices.get(index))
}

fn entity_id(value: &Value) -> Option<u32> {
    value.as_str()?.strip_prefix("entity:")?.parse().ok()
}

fn selected_controller(form: &Form, index: i32) -> Option<u32> {
    selected(&form.choices, index - 1).and_then(|choice| entity_id(&choice["id"]))
}

fn catalog_id<'a>(choices: &'a [Value], index: i32, label: &str) -> Result<&'a str, String> {
    selected(choices, index)
        .and_then(|choice| choice["id"].as_str())
        .ok_or_else(|| format!("Choose a {label}"))
}

fn set_catalog(
    choices: &[Value],
    placeholder: Option<&str>,
    previous: &mut Vec<slint::SharedString>,
    setter: impl FnOnce(slint::ModelRc<slint::SharedString>),
) {
    let mut labels = Vec::new();
    if let Some(placeholder) = placeholder {
        labels.push(placeholder.into());
    }
    labels.extend(choices.iter().map(|choice| {
        let label = choice["label"].as_str().unwrap_or("Unavailable choice");
        if choice["ready"] == false {
            format!("{label} · unavailable").into()
        } else {
            label.into()
        }
    }));
    if *previous != labels {
        *previous = labels.clone();
        setter(Rc::new(VecModel::from(labels)).into());
    }
}

fn index_of(choices: &[Value], id: &Value) -> i32 {
    choices
        .iter()
        .position(|choice| choice["id"] == *id)
        .map_or(-1, |index| index as i32)
}

fn update_choice(window: &FogWindow, form: &Form) {
    let controller = selected(&form.choices, window.get_controller_index() - 1);
    let channel = selected(&form.channels, window.get_channel_index());
    let selection = selected(&form.selections, window.get_environment_index());
    let initial_rule = selected(&form.initial_rules, window.get_initial_rule_index());
    let explicit = selection.is_some_and(|selection| selection["id"] == "explicit");
    window.set_environment_automatic(
        selection.is_some_and(|selection| selection["id"] == "environment"),
    );
    window.set_selection_status(
        if form
            .selection_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot != &form.snapshot)
        {
            "Context changed · refresh choices"
        } else {
            "Unapplied selection"
        }
        .into(),
    );
    window.set_can_apply(
        window.get_available()
            && !window.get_environment_busy()
            && window.get_selection_dirty()
            && form
                .selection_snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot == &form.snapshot)
            && channel.is_some_and(|channel| channel["ready"] == true)
            && selection.is_some_and(|selection| selection["ready"] == true)
            && initial_rule.is_some_and(|initial_rule| initial_rule["ready"] == true)
            && (!window.get_preview_enabled()
                || !explicit
                || controller.is_some_and(|controller| controller["ready"] == true)),
    );
    window.set_channel_detail(
        channel
            .and_then(|channel| channel["detail"].as_str())
            .unwrap_or_default()
            .into(),
    );
    window.set_environment_detail(
        if window.get_selection_dirty() && window.get_environment_automatic() {
            "Apply preview before preparing environment."
        } else {
            selection
                .and_then(|selection| selection["detail"].as_str())
                .unwrap_or_default()
        }
        .into(),
    );
    window.set_can_prepare_environment(
        window.get_available()
            && !window.get_environment_busy()
            && !window.get_selection_dirty()
            && form.report["can_prepare_environment"] == true,
    );
    window.set_initial_rule_detail(
        initial_rule
            .and_then(|initial_rule| initial_rule["detail"].as_str())
            .unwrap_or_default()
            .into(),
    );
    update_input(window, form);
    update_player(window, form);
}

fn update_player(window: &FogWindow, form: &Form) {
    let target = selected(&form.player_targets, window.get_player_index() - 1);
    let stale = form
        .player_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot != &form.snapshot);
    window.set_can_switch_player(
        window.get_available()
            && !window.get_selection_dirty()
            && !stale
            && form.report["can_switch_player_controller"] == true
            && target.is_some_and(|target| target["ready"] == true),
    );
    let issue = if window.get_selection_dirty() {
        Some("Apply the current selection before switching the player controller.")
    } else if stale {
        Some("Fog preview changed. Reselect the player target before switching.")
    } else {
        target
            .and_then(|target| {
                target["issue"]
                    .as_str()
                    .or_else(|| target["detail"].as_str())
            })
            .or_else(|| form.report["player_switch_issue"].as_str())
    };
    window.set_player_issue(issue.is_some());
    window.set_player_detail(issue.unwrap_or("Choose a named target. Its foglerptime sets the duration; zero switches immediately.").into());
    let player = &form.report["player"];
    let status = if player["issue"].is_string() {
        "Player state unavailable".to_owned()
    } else if let Some(controller) = player["current_controller"].as_str() {
        let phase = match player["phase"].as_str() {
            Some("blending") => player["progress"].as_f64().map_or_else(
                || "Blending".to_owned(),
                |progress| format!("Blending {:.0}%", progress * 100.0),
            ),
            Some("settled") => "Settled".to_owned(),
            Some("initial") => "Initial".to_owned(),
            _ => "State unavailable".to_owned(),
        };
        format!("Active {controller} · {phase}")
    } else if let Some(controller) = form.report["controller"].as_str() {
        format!("Active {controller} · No player switches")
    } else {
        "No player switches".to_owned()
    };
    window.set_player_status(status.into());
    let mut state = Vec::new();
    if let Some(raw) = raw_summary(&player["current_raw"]) {
        state.push(format!("Copied raw: {raw}"));
    }
    if let Some(sample_time) = player["sample_time_seconds"].as_f64() {
        let mut sample = format!("Player sample {sample_time:.3} s");
        if let Some(render_time) = player["render_time_seconds"].as_f64() {
            sample.push_str(&format!(" · render evaluation {render_time:.3} s"));
        }
        state.push(sample);
    }
    window.set_player_state(state.join("\n").into());
}

fn update_input(window: &FogWindow, form: &Form) {
    let input = selected(&form.inputs, window.get_input_index());
    let target = selected(&form.input_targets, window.get_input_controller_index() - 1);
    window.set_input_has_value(input.is_some_and(|input| input["has_value"] == true));
    window.set_input_target(
        target
            .and_then(|target| target["id"].as_str())
            .map_or_else(
                || "Choose an input target".to_owned(),
                |controller| format!("{controller} · Preview only"),
            )
            .into(),
    );
    let stale = form
        .input_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot != &form.snapshot);
    window.set_can_send_input(
        window.get_available()
            && !window.get_environment_busy()
            && input.is_some()
            && target.is_some_and(|target| target["ready"] == true)
            && !window.get_selection_dirty()
            && !stale
            && form.report["can_send_controller_input"] == true,
    );
    let help = if window.get_selection_dirty() {
        "Apply the controller selection before sending an input."
    } else if stale {
        "Fog preview changed. Reselect the target or edit the input before sending it."
    } else if target.is_none() {
        "Choose a controller input target. This does not switch the player controller."
    } else if let Some(issue) = target.and_then(|target| {
        target["issue"]
            .as_str()
            .or_else(|| target["detail"].as_str())
    }) {
        issue
    } else if let Some(issue) = form.report["controller_input_issue"].as_str() {
        issue
    } else {
        input
            .and_then(|input| input["help"].as_str())
            .unwrap_or("Choose a registered controller input.")
    };
    window.set_input_help(help.into());
    let state = target.and_then(|target| {
        form.report["controller_states"]
            .as_array()?
            .iter()
            .find(|state| state["controller"] == target["id"])
    });
    let mut details = Vec::new();
    if let Some(state) = state {
        if let Some(raw) = raw_summary(&state["server"]) {
            details.push(format!("Server raw: {raw}"));
        }
        if let Some(raw) = raw_summary(&state["received"]) {
            details.push(format!("Delivered raw: {raw}"));
        }
        if let Some(time) = state["pending_think_time_seconds"].as_f64() {
            details.push(format!("Controller think due at {time:.3} s"));
        }
    }
    window.set_input_state(details.join("\n").into());
}

fn unavailable(window: &FogWindow, error: &str) {
    window.set_available(false);
    window.set_can_apply(false);
    window.set_can_play(false);
    window.set_can_seek(false);
    window.set_can_send_input(false);
    window.set_can_switch_player(false);
    window.set_player_status("Player state unavailable".into());
    window.set_player_state("".into());
    window.set_player_detail(error.into());
    window.set_player_issue(true);
    window.set_can_prepare_environment(false);
    window.set_environment_busy(false);
    window.set_playing(false);
    window.set_transitioning(false);
    window.set_has_issue(true);
    window.set_current("Fog preview is unavailable".into());
    window.set_detail("No current fog settings are available.".into());
    window.set_playback_status("Unavailable".into());
    window.set_time_label("Time unavailable".into());
    window.set_input_target("No current controller".into());
    window.set_input_state("".into());
    window.set_effective_controller("Unavailable".into());
    window.set_message(error.into());
}

fn array(report: &Value, key: &str) -> Result<Vec<Value>, String> {
    report[key]
        .as_array()
        .cloned()
        .ok_or_else(|| format!("Fog preview has no {key} catalog"))
}

fn retain_choice(choices: &mut Vec<Value>, previous: Option<&Value>) {
    if let Some(previous) = previous
        && !choices.iter().any(|choice| choice["id"] == previous["id"])
    {
        let mut missing = previous.clone();
        missing["ready"] = false.into();
        missing["detail"] = "This choice is no longer available. Refresh choices.".into();
        choices.push(missing);
    }
}

fn vector_text(value: &Value) -> Option<String> {
    let values = value.as_array()?;
    Some(format!(
        "{}, {}, {}",
        values.first()?.as_f64()? as f32,
        values.get(1)?.as_f64()? as f32,
        values.get(2)?.as_f64()? as f32
    ))
}

fn scalar_text(value: &Value) -> String {
    value.as_f64().map_or_else(
        || "Not reported".to_owned(),
        |value| (value as f32).to_string(),
    )
}

fn raw_summary(raw: &Value) -> Option<String> {
    let profile = &raw["profile"];
    let color = vector_text(&profile["color"])?;
    Some(format!(
        "RGB {color} · start {} · end {}",
        scalar_text(&profile["start"]),
        scalar_text(&profile["end"])
    ))
}

fn settings_detail(report: &Value) -> String {
    let current = &report["current"];
    let profile = &current["profile"];
    let mut lines = Vec::new();
    if let Some(controller) = current["controller"].as_str() {
        lines.push(format!("Controller: {controller}"));
    }
    if let Some(reason) = current["selection_reason"].as_str() {
        lines.push(format!("Selection: {}", reason.replace('_', " ")));
    }
    if profile.is_object() {
        if let Some(color) = vector_text(&profile["color"]) {
            lines.push(format!("Primary RGB: {color}"));
        }
        if let Some(color) = vector_text(&profile["color2"]) {
            lines.push(format!("Secondary RGB: {color}"));
        }
        lines.push(format!(
            "Start: {} units · End: {} units",
            scalar_text(&profile["start"]),
            scalar_text(&profile["end"])
        ));
        lines.push(format!(
            "Density parameter: {} · renderer clamps to 0–1",
            scalar_text(&profile["max_density"])
        ));
        lines.push(format!(
            "Authored fog: {} · distance: {}",
            if profile["enabled"] == true {
                "enabled"
            } else {
                "disabled"
            },
            if profile["radial"] == true {
                "radial"
            } else {
                "view depth"
            }
        ));
        lines.push(format!(
            "Directional color blend: {}",
            if profile["blend"] == true {
                "enabled"
            } else {
                "disabled"
            }
        ));
        if let Some(direction) = vector_text(&profile["direction"]) {
            lines.push(format!("Direction: {direction}"));
        }
        lines.push(format!(
            "Far-Z parameter: {} · transition duration: {} s",
            scalar_text(&profile["far_z"]),
            scalar_text(&profile["lerp_time"])
        ));
    }
    let water = &current["water"];
    if water.is_object() {
        lines.push(format!(
            "Water: {} · {}",
            water["solid"].as_str().unwrap_or("source unavailable"),
            water["material"].as_str().unwrap_or("material unavailable")
        ));
        if let Some(color) = vector_text(&water["color_normalized"]) {
            lines.push(format!("Water RGB (normalized): {color}"));
        }
        lines.push(format!(
            "Water start: {} · end: {} · {}",
            scalar_text(&water["start"]),
            scalar_text(&water["end"]),
            if water["enabled"] == true {
                "enabled"
            } else {
                "disabled"
            }
        ));
    }
    if report["enabled"] != true {
        lines.push("Fog preview is off.".into());
    } else if current.is_object() && current["applied"] != true {
        lines.push("Color fog is inactive in the current view.".into());
    }
    if lines.is_empty() {
        lines.push(
            if report["issue"].is_string() {
                "No current fog profile. Inspect the reported issue."
            } else {
                "Choose an authored env_fog_controller, then Apply preview."
            }
            .into(),
        );
    }
    if let Some(rule) = report["initial_radial_contexts"]
        .as_array()
        .and_then(|rules| {
            rules
                .iter()
                .find(|rule| rule["id"] == report["initial_radial_context"])
        })
        && let Some(label) = rule["label"].as_str()
    {
        lines.insert(0, format!("Initial rule: {label}"));
    }
    lines.join("\n")
}

fn playback_status(report: &Value) -> String {
    let mut status = match report["status"].as_str().unwrap_or("unavailable") {
        "preparing_environment" => "Preparing environment",
        "disabled" => "Preview off",
        "paused" => "Paused",
        "playing" => "Playing",
        "stale" => "Source changed",
        "failed" => "Failed",
        _ => "Unavailable",
    }
    .to_owned();
    if report["busy"] == true
        && let Some(actor) = report["worker_actor"].as_str()
    {
        status.push_str(match actor {
            "user" => " · User",
            "agent" => " · Agent",
            "system" => " · System",
            _ => " · actor unavailable",
        });
    }
    status
}

fn present(window: &FogWindow, form: &RefCell<Form>, session: &Session) -> Result<(), String> {
    let report = session.fog_preview_state();
    let clock = session.fog_preview_clock();
    let snapshot = report["snapshot"]
        .as_str()
        .ok_or("Fog preview has no source snapshot")?
        .to_owned();
    let summary = report["summary"]
        .as_str()
        .ok_or("Fog preview has no summary")?;
    let mut form = form.borrow_mut();
    window.set_document_label(session.title().into());
    window.set_current(summary.into());
    window.set_effective_controller(
        if let Some(initial) = report["player"]["initial_controller"].as_str() {
            format!("{initial} · replay baseline")
        } else if let Some(controller) = report["controller"].as_str() {
            if let Some(reason) = report["current"]["selection_reason"].as_str() {
                format!("{controller} · {}", reason.replace('_', " "))
            } else {
                controller.to_owned()
            }
        } else {
            "No applied controller".to_owned()
        }
        .into(),
    );
    window.set_has_issue(
        report["issue"].is_string()
            || report["error"].is_string()
            || report["catalog_issue"].is_string(),
    );
    window.set_available(true);
    {
        let dirty = window.get_selection_dirty();
        let controller = dirty
            .then(|| selected(&form.choices, window.get_controller_index() - 1))
            .flatten()
            .cloned();
        let channel = dirty
            .then(|| selected(&form.channels, window.get_channel_index()))
            .flatten()
            .cloned();
        let selection = dirty
            .then(|| selected(&form.selections, window.get_environment_index()))
            .flatten()
            .cloned();
        let initial_rule = dirty
            .then(|| selected(&form.initial_rules, window.get_initial_rule_index()))
            .flatten()
            .cloned();
        let mut choices = array(&report, "controllers")?;
        let mut channels = array(&report, "channels")?;
        let mut selections = array(&report, "selections")?;
        let mut initial_rules = array(&report, "initial_radial_contexts")?;
        retain_choice(&mut choices, controller.as_ref());
        retain_choice(&mut channels, channel.as_ref());
        retain_choice(&mut selections, selection.as_ref());
        retain_choice(&mut initial_rules, initial_rule.as_ref());
        set_catalog(
            &choices,
            Some("Choose a controller"),
            &mut form.controller_labels,
            |model| window.set_controllers(model),
        );
        set_catalog(&channels, None, &mut form.channel_labels, |model| {
            window.set_channels(model)
        });
        set_catalog(&selections, None, &mut form.selection_labels, |model| {
            window.set_environments(model)
        });
        set_catalog(
            &initial_rules,
            None,
            &mut form.initial_rule_labels,
            |model| window.set_initial_rules(model),
        );
        form.choices = choices;
        form.channels = channels;
        form.selections = selections;
        form.initial_rules = initial_rules;
        window.set_controller_index(
            index_of(
                &form.choices,
                if dirty {
                    controller
                        .as_ref()
                        .map_or(&Value::Null, |choice| &choice["id"])
                } else {
                    &report["configured_controller"]
                },
            ) + 1,
        );
        window.set_channel_index(index_of(
            &form.channels,
            channel
                .as_ref()
                .map_or(&report["channel"], |choice| &choice["id"]),
        ));
        window.set_environment_index(index_of(
            &form.selections,
            selection
                .as_ref()
                .map_or(&report["selection"], |choice| &choice["id"]),
        ));
        window.set_initial_rule_index(index_of(
            &form.initial_rules,
            initial_rule
                .as_ref()
                .map_or(&report["initial_radial_context"], |choice| &choice["id"]),
        ));
        if !dirty {
            window.set_preview_enabled(report["enabled"] == true);
        }
    }
    if !window.get_input_dirty() {
        let previous =
            selected(&form.inputs, window.get_input_index()).map(|input| input["id"].clone());
        let inputs = array(&report, "inputs")?;
        set_catalog(&inputs, None, &mut form.input_labels, |model| {
            window.set_inputs(model)
        });
        form.inputs = inputs;
        let index = previous
            .as_ref()
            .map_or(-1, |id| index_of(&form.inputs, id));
        window.set_input_index(if index >= 0 {
            index
        } else if form.inputs.is_empty() {
            -1
        } else {
            0
        });
    }
    {
        let previous =
            selected(&form.input_targets, window.get_input_controller_index() - 1).cloned();
        let mut targets = array(&report, "input_targets")?;
        if window.get_input_dirty() {
            retain_choice(&mut targets, previous.as_ref());
        }
        set_catalog(
            &targets,
            Some("Choose an input target"),
            &mut form.input_target_labels,
            |model| window.set_input_controllers(model),
        );
        form.input_targets = targets;
        window.set_input_controller_index(
            previous
                .as_ref()
                .map_or(0, |target| index_of(&form.input_targets, &target["id"]) + 1),
        );
    }
    {
        let previous = selected(&form.player_targets, window.get_player_index() - 1).cloned();
        let mut targets = array(&report, "switch_targets")?;
        if window.get_player_dirty() {
            retain_choice(&mut targets, previous.as_ref());
        }
        set_catalog(
            &targets,
            Some("Choose a player fog target"),
            &mut form.player_labels,
            |model| window.set_player_controllers(model),
        );
        form.player_targets = targets;
        window.set_player_index(previous.as_ref().map_or(0, |target| {
            index_of(&form.player_targets, &target["id"]) + 1
        }));
    }
    window.set_playing(clock.playing);
    window.set_transitioning(report["transitioning"] == true);
    window.set_can_play(report["can_play"] == true);
    window.set_can_seek(report["can_seek"] == true);
    window.set_environment_busy(clock.busy);
    window.set_playback_status(playback_status(&report).into());
    window.set_max_time_seconds(
        report["max_time_seconds"]
            .as_f64()
            .ok_or("Fog preview has no time bound")? as f32,
    );
    window.set_time_label(format!("Current time {:.2} s", clock.time_seconds).into());
    window.set_time_limits(
        format!(
            "0–{} s · {} / {} events",
            report["max_time_seconds"],
            report["events"].as_array().map_or(0, Vec::len),
            report["limits"]["events"]
        )
        .into(),
    );
    if !window.get_scrub_dirty() {
        window.set_time_value(clock.time_seconds as f32);
    }
    if !window.get_time_dirty() {
        window.set_seek_time(clock.requested_time_seconds.to_string().into());
    }
    window.set_detail(settings_detail(&report).into());
    window.set_scope(
        ["selection_policy", "persistence"]
            .into_iter()
            .filter_map(|key| report[key].as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
            .into(),
    );
    window.set_message(
        form.action_error
            .as_deref()
            .or_else(|| report["error"].as_str())
            .or_else(|| report["issue"].as_str())
            .or_else(|| report["catalog_issue"].as_str())
            .unwrap_or_default()
            .into(),
    );
    form.snapshot = snapshot;
    form.seen = Some((
        clock.snapshot,
        clock.serial,
        session.camera_serial(),
        session.view_serial(),
    ));
    form.refreshed = Some(Instant::now());
    form.report = report;
    update_choice(window, &form);
    Ok(())
}

fn refresh(
    window: &FogWindow,
    form: &RefCell<Form>,
    target: &DocumentContinuation,
) -> Result<(), String> {
    target
        .upgrade()?
        .with_session(|session| present(window, form, session))
}

fn number(text: &str) -> Result<f64, String> {
    text.trim()
        .parse()
        .map_err(|_| "Seek time must be a number".into())
}

fn command(window: &FogWindow, form: &Form, action: Action) -> Result<(String, Command), String> {
    let snapshot = match action {
        Action::Apply => form.selection_snapshot.as_ref(),
        Action::Input => form.input_snapshot.as_ref(),
        Action::SwitchPlayer => form.player_snapshot.as_ref(),
        Action::Seek | Action::Scrub(_) => form.time_snapshot.as_ref(),
        _ => None,
    }
    .unwrap_or(&form.snapshot)
    .clone();
    let command = match action {
        Action::Apply => Command::Settings(Settings {
            enabled: window.get_preview_enabled(),
            controller: selected_controller(form, window.get_controller_index()),
            selection: SelectionMode::parse(catalog_id(
                &form.selections,
                window.get_environment_index(),
                "fog environment",
            )?)?,
            channel: Channel::parse(catalog_id(
                &form.channels,
                window.get_channel_index(),
                "fog channel",
            )?)?,
            initial_radial_context: InitialRadialContext::parse(catalog_id(
                &form.initial_rules,
                window.get_initial_rule_index(),
                "initial fog rule",
            )?)?,
        }),
        Action::Playback if window.get_playing() => Command::Pause,
        Action::Playback => Command::Play,
        Action::Reset => Command::Reset,
        Action::Seek => Command::Seek {
            time_seconds: number(&window.get_seek_time())?,
        },
        Action::Scrub(time_seconds) => Command::Seek { time_seconds },
        Action::Input => {
            let input = catalog_id(
                &form.inputs,
                window.get_input_index(),
                "registered controller input",
            )?;
            let controller = selected(&form.input_targets, window.get_input_controller_index() - 1)
                .and_then(|target| entity_id(&target["id"]))
                .ok_or("Choose a controller input target")?;
            let value = window.get_input_value();
            Command::Input {
                controller,
                input: map_core::fog::Input::parse(
                    input,
                    window.get_input_has_value().then_some(value.as_str()),
                )?,
            }
        }
        Action::PrepareEnvironment => Command::PrepareEnvironment,
        Action::Cancel => Command::Cancel,
        Action::Clear => Command::Clear,
        Action::SwitchPlayer => Command::SwitchPlayerController {
            controller: selected(&form.player_targets, window.get_player_index() - 1)
                .and_then(|target| entity_id(&target["id"]))
                .ok_or("Choose a player fog controller target")?,
        },
    };
    Ok((snapshot, command))
}

fn run(window: &FogWindow, form: &RefCell<Form>, target: &DocumentContinuation, action: Action) {
    let result = command(window, &form.borrow(), action).and_then(|(snapshot, command)| {
        let background = matches!(
            command,
            Command::PrepareEnvironment
                | Command::Input { .. }
                | Command::Seek { .. }
                | Command::Advance { .. }
                | Command::Reset
                | Command::SwitchPlayerController { .. }
        );
        target
            .upgrade()?
            .fog_preview(Actor::User, &snapshot, command, background)
    });
    form.borrow_mut().action_error = result.as_ref().err().cloned();
    if result.is_ok() {
        window.set_message("".into());
        let mut form = form.borrow_mut();
        if matches!(action, Action::Apply | Action::Clear) {
            window.set_selection_dirty(false);
            form.selection_snapshot = None;
        }
        if matches!(
            action,
            Action::Seek | Action::Scrub(_) | Action::Reset | Action::Clear
        ) {
            window.set_time_dirty(false);
            window.set_scrub_dirty(false);
            form.time_snapshot = None;
        }
        if matches!(action, Action::Input | Action::Clear) {
            window.set_input_dirty(false);
            form.input_snapshot = None;
            if matches!(action, Action::Clear) {
                window.set_input_controller_index(0);
            }
        }
        if matches!(action, Action::SwitchPlayer | Action::Clear) {
            window.set_player_dirty(false);
            form.player_snapshot = None;
            if matches!(action, Action::Clear) {
                window.set_player_index(0);
            }
        }
    }
    if let Err(error) = refresh(window, form, target) {
        unavailable(window, &error);
    } else if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn retired(access: &DocumentAccess) -> bool {
    access
        .document_lifecycle()
        .is_some_and(|lifecycle| lifecycle != DocumentLifecycle::Live)
}

fn hide(window: &FogWindow, form: &RefCell<Form>) {
    if let Err(error) = window.hide() {
        let error = format!("Could not hide Fog preview: {error}");
        form.borrow_mut().action_error = Some(error.clone());
        window.set_message(error.into());
    }
}

fn close(window: &FogWindow, form: &RefCell<Form>, target: &DocumentContinuation) {
    let Ok(access) = target.upgrade() else {
        hide(window, form);
        return;
    };
    if retired(&access) {
        hide(window, form);
        return;
    }
    let result = access
        .with_session(|session| Ok(session.fog_preview_clock().snapshot))
        .and_then(|snapshot| access.fog_preview(Actor::User, &snapshot, Command::Pause, false));
    match result {
        Ok(_) => hide(window, form),
        Err(_) if retired(&access) => hide(window, form),
        Err(error) => {
            form.borrow_mut().action_error = Some(error.clone());
            if let Err(refresh_error) = refresh(window, form, target) {
                unavailable(window, &refresh_error);
            }
            window.set_message(error.into());
        }
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = FogWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let callback = |action| {
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move || {
                if let Some(window) = window.upgrade() {
                    run(&window, &form, &target, action);
                }
            }
        };
        window.on_apply(callback(Action::Apply));
        window.on_play_pause(callback(Action::Playback));
        window.on_reset(callback(Action::Reset));
        window.on_seek(callback(Action::Seek));
        window.on_send_input(callback(Action::Input));
        window.on_prepare_environment(callback(Action::PrepareEnvironment));
        window.on_cancel_work(callback(Action::Cancel));
        window.on_clear_preview(callback(Action::Clear));
        window.on_switch_player_controller(callback(Action::SwitchPlayer));
        window.on_player_changed({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    let mut form = form.borrow_mut();
                    form.player_snapshot = Some(form.snapshot.clone());
                    update_player(&window, &form);
                }
            }
        });
        window.on_scrub({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move |seconds| {
                if let Some(window) = window.upgrade() {
                    run(&window, &form, &target, Action::Scrub(f64::from(seconds)));
                }
            }
        });
        window.on_choice_changed({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    let mut form = form.borrow_mut();
                    form.selection_snapshot = Some(form.snapshot.clone());
                    update_choice(&window, &form);
                }
            }
        });
        window.on_input_changed({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    let mut form = form.borrow_mut();
                    form.input_snapshot = Some(form.snapshot.clone());
                    update_input(&window, &form);
                }
            }
        });
        window.on_input_edited({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    let mut form = form.borrow_mut();
                    form.input_snapshot = Some(form.snapshot.clone());
                    update_input(&window, &form);
                }
            }
        });
        window.on_time_edited({
            let form = form.clone();
            move || {
                let mut form = form.borrow_mut();
                form.time_snapshot = Some(form.snapshot.clone());
            }
        });
        window.on_refresh({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move || {
                if let Some(window) = window.upgrade() {
                    window.set_message("".into());
                    window.set_selection_dirty(false);
                    let mut state = form.borrow_mut();
                    state.selection_snapshot = None;
                    state.action_error = None;
                    drop(state);
                    if let Err(error) = refresh(&window, &form, &target) {
                        unavailable(&window, &error);
                    }
                }
            }
        });
        window.on_cancel({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move || {
                if let Some(window) = window.upgrade() {
                    close(&window, &form, &target);
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        self.form.borrow_mut().action_error = None;
        if let Err(error) = present(&self.window, &self.form, session) {
            unavailable(&self.window, &error);
        }
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        if !self.window.window().is_visible() {
            return;
        }
        let clock = session.fog_preview_clock();
        {
            let form = self.form.borrow();
            if form.seen.as_ref()
                == Some(&(
                    clock.snapshot.clone(),
                    clock.serial,
                    session.camera_serial(),
                    session.view_serial(),
                ))
                || (!matches!(clock.status, "failed" | "stale")
                    && form
                        .refreshed
                        .is_some_and(|at| at.elapsed() < REPORT_INTERVAL))
            {
                return;
            }
        }
        if let Err(error) = present(&self.window, &self.form, session) {
            unavailable(&self.window, &error);
        }
    }

    pub fn set_active(&self, active: bool) {
        self.window.set_document_active(active);
        self.form.borrow_mut().refreshed = None;
    }
}
