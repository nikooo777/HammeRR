use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::FogWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation, DocumentLifecycle};
use crate::session::fog::{Channel, Command, SelectionMode, Settings};
use crate::session::{Actor, Session};

const REPORT_INTERVAL: Duration = Duration::from_millis(125);

#[derive(Default)]
struct Form {
    snapshot: String,
    selection_snapshot: Option<String>,
    input_snapshot: Option<String>,
    input_controller: Option<u32>,
    time_snapshot: Option<String>,
    choices: Vec<Value>,
    channels: Vec<Value>,
    selections: Vec<Value>,
    inputs: Vec<Value>,
    controller_labels: Vec<slint::SharedString>,
    channel_labels: Vec<slint::SharedString>,
    selection_labels: Vec<slint::SharedString>,
    input_labels: Vec<slint::SharedString>,
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
    update_input(window, form);
}

fn update_input(window: &FogWindow, form: &Form) {
    let input = selected(&form.inputs, window.get_input_index());
    window.set_input_has_value(input.is_some_and(|input| input["has_value"] == true));
    let applied = entity_id(&form.report["controller"]);
    window.set_input_target(
        form.input_controller
            .or(applied)
            .map_or_else(
                || "No applied controller".to_owned(),
                |controller| format!("entity:{controller} · Preview only"),
            )
            .into(),
    );
    let changed = form
        .input_controller
        .is_some_and(|controller| Some(controller) != applied);
    let stale = form
        .input_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot != &form.snapshot);
    window.set_can_send_input(
        window.get_available()
            && !window.get_environment_busy()
            && applied.is_some()
            && input.is_some()
            && !window.get_selection_dirty()
            && !changed
            && !stale,
    );
    let help = if window.get_selection_dirty() {
        "Apply the controller selection before sending an input."
    } else if changed {
        "The applied controller changed. Reselect the input before sending it."
    } else if stale {
        "Fog preview changed. Reselect or edit the input before sending it."
    } else {
        input
            .and_then(|input| input["help"].as_str())
            .unwrap_or("Choose a registered controller input.")
    };
    window.set_input_help(help.into());
}

fn unavailable(window: &FogWindow, error: &str) {
    window.set_available(false);
    window.set_can_apply(false);
    window.set_can_play(false);
    window.set_can_seek(false);
    window.set_can_send_input(false);
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
        if let Some(controller) = report["controller"].as_str() {
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
        let mut choices = array(&report, "controllers")?;
        let mut channels = array(&report, "channels")?;
        let mut selections = array(&report, "selections")?;
        retain_choice(&mut choices, controller.as_ref());
        retain_choice(&mut channels, channel.as_ref());
        retain_choice(&mut selections, selection.as_ref());
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
        form.choices = choices;
        form.channels = channels;
        form.selections = selections;
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
            "0–{} s · {} / {} inputs",
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
            let controller = form
                .input_controller
                .or_else(|| entity_id(&form.report["controller"]))
                .ok_or("Apply a controller before sending an input")?;
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
            form.input_controller = None;
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
                    form.input_controller = entity_id(&form.report["controller"]);
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
                    form.input_controller = entity_id(&form.report["controller"]);
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
