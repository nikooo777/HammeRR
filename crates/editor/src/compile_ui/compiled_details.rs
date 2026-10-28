use std::{cell::RefCell, rc::Rc};

use render::LightingChannel;
use render::compiled_details::{Lighting, LightingSettings, StyleGain};
use serde_json::Value;
use slint::ComponentHandle;

use super::{CompileWindow, DocumentAccess, Session, SharedSession, lock};
use crate::{CompiledDetailsWindow, session::Actor};

pub(super) struct Form {
    window: CompiledDetailsWindow,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    token: Option<u64>,
    offset: usize,
    observation: Option<Value>,
    lighting: LightingSettings,
    styles: Vec<u8>,
    selected_style: Option<u8>,
}

#[derive(Clone, Copy)]
enum Action {
    Prepare,
    Cancel,
    Clear,
    Show,
    Hide,
    Inspect,
    Page(i32),
    Lighting(i32),
    ApplyLighting,
    Style(i32),
}

fn mode_index(mode: Lighting) -> i32 {
    match mode {
        Lighting::EditorShading => 0,
        Lighting::BakedLdr => 1,
        Lighting::BakedRadiance => 2,
    }
}

fn read_lighting(value: &Value) -> Result<LightingSettings, String> {
    Ok(LightingSettings {
        mode: Lighting::parse(
            value["mode"]
                .as_str()
                .ok_or("Missing detail lighting mode")?,
        )?,
        channel: LightingChannel::parse(
            value["channel"]
                .as_str()
                .ok_or("Missing detail lighting channel")?,
        )?,
        exposure_ev: value["exposure_ev"]
            .as_f64()
            .ok_or("Missing detail exposure")?,
        style_gains: serde_json::from_value(value["style_gains"].clone())
            .map_err(|error| format!("Invalid detail style settings: {error}"))?,
    })
}

fn show_lighting(window: &CompiledDetailsWindow, settings: &LightingSettings) {
    window.set_lighting_index(mode_index(settings.mode));
    window.set_lighting_channel_index(i32::from(settings.channel == LightingChannel::Hdr));
    window.set_exposure_ev(settings.exposure_ev as f32);
}

fn show_style(window: &CompiledDetailsWindow, state: &State) {
    window.set_style_index(
        state
            .selected_style
            .and_then(|style| {
                state
                    .styles
                    .iter()
                    .position(|&candidate| candidate == style)
            })
            .map_or(-1, |index| index as i32),
    );
    window.set_style_gain(
        state
            .selected_style
            .map_or(0.0, |style| state.lighting.gain(style)) as f32,
    );
}

fn keep_style_gain(window: &CompiledDetailsWindow, state: &mut State) {
    if let Some(style) = state.selected_style {
        let gain = f64::from(window.get_style_gain());
        if let Some(entry) = state
            .lighting
            .style_gains
            .iter_mut()
            .find(|entry| entry.style == style)
        {
            entry.gain = gain;
        } else {
            state.lighting.style_gains.push(StyleGain { style, gain });
        }
    }
}

fn chosen_lighting(
    window: &CompiledDetailsWindow,
    state: &mut State,
    index: i32,
) -> Result<LightingSettings, String> {
    let mode = match index {
        0 => Lighting::EditorShading,
        1 => Lighting::BakedLdr,
        2 => Lighting::BakedRadiance,
        _ => return Err("Unknown compiled detail lighting mode".into()),
    };
    if mode != Lighting::BakedRadiance {
        return Ok(LightingSettings {
            mode,
            ..Default::default()
        });
    }
    let channel = match window.get_lighting_channel_index() {
        0 => LightingChannel::Ldr,
        1 => LightingChannel::Hdr,
        _ => return Err("Unknown compiled detail lighting channel".into()),
    };
    keep_style_gain(window, state);
    Ok(LightingSettings {
        mode,
        channel,
        exposure_ev: f64::from(window.get_exposure_ev()),
        style_gains: state.lighting.style_gains.clone(),
    })
}

fn static_source_label(source: &str) -> &'static str {
    match source {
        "shared_native_base" => "shared native base",
        "selected_channel_style_zero" => "selected channel style 0",
        "shared_native_base_and_selected_channel_style_zero" => {
            "shared native base + selected channel style 0"
        }
        _ => "unavailable static contribution",
    }
}

fn lighting_text(lighting: &Value) -> String {
    let mut text = format!(
        "shared RGBExp32 {} exponent {}",
        lighting["rgb"], lighting["exponent"]
    );
    if let Some(reason) = lighting["selected_table_unavailable_reason"].as_str() {
        text.push_str(&format!(" · {reason}"));
        return text;
    }
    let contribution = &lighting["static_contribution"];
    text.push_str(&format!(
        " · static {} ({}) linear {}",
        contribution["selected_channel"]
            .as_str()
            .unwrap_or("unknown")
            .to_ascii_uppercase(),
        static_source_label(contribution["source"].as_str().unwrap_or("")),
        contribution["linear"]
    ));
    if contribution["style_zero"].is_object() {
        text.push_str(&format!(
            "\n    stored style 0 RGBExp32 {} exponent {} · editor weight 1",
            contribution["style_zero"]["rgb"], contribution["style_zero"]["exponent"]
        ));
    }
    if !lighting["exposed_linear"].is_null() {
        text.push_str(&format!(
            "\n    exposed linear {}",
            lighting["exposed_linear"]
        ));
    } else if !lighting["vertex_color"].is_null() {
        text.push_str(&format!(
            "\n    linear vertex colour {}",
            lighting["vertex_color"]
        ));
    }
    if let Some(extras) = lighting["style_extras"].as_array() {
        for entry in extras.iter().filter(|entry| entry["style"] != 0) {
            text.push_str(&format!(
                "\n    style {} RGBExp32 {} exponent {} · effective weight {}",
                entry["style"], entry["rgb"], entry["exponent"], entry["effective_weight"]
            ));
        }
    }
    text
}

fn present(
    window: &CompiledDetailsWindow,
    state: &mut State,
    report: Value,
    observation: Value,
) -> Result<(), String> {
    state.token = report["token"]
        .as_str()
        .and_then(|token| token.parse().ok());
    state.observation = Some(observation);
    window.set_has_token(state.token.is_some());
    window.set_worker_active(report["worker_active"].as_bool().unwrap_or(false));
    state.lighting = read_lighting(&report["lighting"])?;
    let styles: Option<Vec<u8>> =
        serde_json::from_value(report["lighting"]["available_styles"].clone())
            .map_err(|error| format!("Invalid detail style list: {error}"))?;
    state.styles = styles.unwrap_or_default();
    state.selected_style = state
        .selected_style
        .filter(|style| state.styles.contains(style))
        .or_else(|| state.styles.first().copied());
    window.set_style_options(
        Rc::new(slint::VecModel::from(
            state
                .styles
                .iter()
                .map(|style| slint::SharedString::from(format!("Style {style}")))
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    show_lighting(window, &state.lighting);
    show_style(window, state);
    let lighting_state = if report["lighting"]["baked_ldr_available"].is_null() {
        "Prepare details to inspect lighting".to_owned()
    } else {
        match state.lighting.mode {
            Lighting::EditorShading => "Editor shading".to_owned(),
            Lighting::BakedLdr => report["lighting"]["unavailable_reason"]
                .as_str()
                .unwrap_or("LDR static colours · stored style 0 at editor unity")
                .to_owned(),
            Lighting::BakedRadiance => report["lighting"]["radiance_unavailable_reason"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!(
                        "{} static colours + manual styles · stored style 0 at editor unity",
                        state.lighting.channel.name().to_ascii_uppercase()
                    )
                }),
        }
    };
    window.set_lighting_state(lighting_state.into());
    window.set_state(
        format!(
            "{} · requested {} · displayed {} · complete geometry {}",
            report["status"].as_str().unwrap_or("unavailable"),
            report["requested"],
            report["displayed"],
            report["complete"]
        )
        .into(),
    );
    let counts = if let Some(total) = report["page"]["total"].as_u64() {
        format!(
            "{total} final records · models {} · cards {} · cross {} · tri {}",
            report["counts"]["models"],
            report["counts"]["cards"],
            report["counts"]["cross"],
            report["counts"]["tri"]
        )
    } else {
        "Final records have not been prepared or are unavailable".into()
    };
    window.set_build_state(
        format!(
            "At inspection: build {} · build token {} · {counts}",
            report["build"]["status"].as_str().unwrap_or("unavailable"),
            report["build"]["token"].as_str().unwrap_or("none")
        )
        .into(),
    );
    window.set_previous(state.offset > 0);
    window.set_next(report["page"]["next_offset"].is_number());
    window.set_page(
        report["page"]["total"]
            .as_u64()
            .map_or_else(String::new, |total| {
                format!(
                    "Native records {}–{} of {total}",
                    state.offset,
                    state.offset + report["page"]["returned"].as_u64().unwrap_or(0) as usize
                )
            })
            .into(),
    );
    let rows=report["page"]["rows"].as_array().map(|rows|rows.iter().map(|row|format!(
        "#{}  {}  leaf {}  dictionary {}{}\n    origin {}  angles {}  scale {}  orientation {}  sway {}  shape angle/size {}/{}\n    native lighting {}  styles offset/count {}/{}",
        row["ordinal"],row["kind_name"].as_str().unwrap_or("unknown"),row["leaf"],row["dictionary_index"],
        row["model"].as_str().map(|name|format!("  {name}")).unwrap_or_default(),row["origin"],row["angles"],row["scale"],
        row["orientation"],row["sway"],row["shape_angle"],row["shape_size"],lighting_text(&row["lighting"]),row["light_styles"],row["light_style_count"]
    )).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
    window.set_rows(rows.into());
    let mut messages = Vec::new();
    if let Some(reason) = report["suppression_reason"].as_str() {
        messages.push(reason.to_owned());
    }
    if let Some(error) = report["error"].as_str() {
        messages.push(error.to_owned());
    }
    if let Some(diagnostics) = report["build"]["diagnostics"].as_array() {
        messages.extend(
            diagnostics
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    if let Some(diagnostics) = report["preview_assets"]["diagnostics"].as_array() {
        messages.extend(
            diagnostics
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    window.set_message(messages.join("\n").into());
    Ok(())
}

fn perform(
    window: &CompiledDetailsWindow,
    shared: &SharedSession,
    state: &Rc<RefCell<State>>,
    action: Action,
) -> Result<(), String> {
    if let Action::Style(index) = action {
        let mut state = state.borrow_mut();
        let selected = usize::try_from(index)
            .ok()
            .and_then(|index| state.styles.get(index))
            .copied()
            .ok_or("Choose an available detail style")?;
        keep_style_gain(window, &mut state);
        state.selected_style = Some(selected);
        show_style(window, &state);
        return Ok(());
    }
    if matches!(action, Action::Prepare) {
        let request = lock(shared).begin_compiled_details(Actor::User)?;
        let token = request.token();
        state.borrow_mut().token = Some(token);
        state.borrow_mut().offset = 0;
        let worker_session = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("compiled-details".into())
            .spawn(move || {
                let prepared = request.run();
                let _ = lock(&worker_session).finish_compiled_details(Actor::User, prepared);
            });
        if let Err(error) = spawned {
            let _ = lock(shared).fail_compiled_details(
                Actor::User,
                token,
                format!("Could not start detail preparation: {error}"),
            );
        }
    }
    let mut session = lock(shared);
    let token = state.borrow().token;
    match action {
        Action::Cancel => {
            session.cancel_compiled_details(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
            )?;
        }
        Action::Clear => {
            session.clear_compiled_details(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
            )?;
            state.borrow_mut().offset = 0;
        }
        Action::Show | Action::Hide => {
            session.set_compiled_details_visible(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
                matches!(action, Action::Show),
            )?;
        }
        Action::Page(direction) => {
            let offset = state.borrow().offset;
            state.borrow_mut().offset = if direction < 0 {
                offset.saturating_sub(64)
            } else {
                offset.saturating_add(64)
            };
        }
        lighting_action @ (Action::Lighting(_) | Action::ApplyLighting) => {
            let index = match lighting_action {
                Action::Lighting(index) => index,
                _ => window.get_lighting_index(),
            };
            let settings = chosen_lighting(window, &mut state.borrow_mut(), index)?;
            session.set_compiled_details_lighting_options(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
                settings,
            )?;
        }
        Action::Style(_) => unreachable!(),
        Action::Prepare | Action::Inspect => {}
    }
    let offset = state.borrow().offset;
    let token =
        if matches!(action, Action::Clear | Action::Inspect | Action::Prepare) && offset == 0 {
            None
        } else {
            state.borrow().token
        };
    let report = session.inspect_compiled_details(token, offset, 64)?;
    present(
        window,
        &mut state.borrow_mut(),
        report,
        session.compiled_details_observation(),
    )?;
    Ok(())
}

fn handler(
    window: &CompiledDetailsWindow,
    access: &DocumentAccess,
    state: &Rc<RefCell<State>>,
    action: impl Fn(i32) -> Action + 'static,
) -> impl Fn(i32) + 'static {
    let (window, target, state) = (window.as_weak(), access.downgrade(), state.clone());
    move |value| {
        let Some(window) = window.upgrade() else {
            return;
        };
        let result = target.upgrade().and_then(|target| {
            target.with_shared(|shared| perform(&window, shared, &state, action(value)))
        });
        if let Err(error) = result {
            if matches!(
                action(value),
                Action::Lighting(_) | Action::ApplyLighting | Action::Style(_)
            ) {
                let _ = target.upgrade().and_then(|target| {
                    target.with_shared(|shared| perform(&window, shared, &state, Action::Inspect))
                });
            }
            window.set_message(error.into());
        }
    }
}

pub(super) fn bind(
    parent: &CompileWindow,
    access: &DocumentAccess,
) -> Result<Form, slint::PlatformError> {
    let window = CompiledDetailsWindow::new()?;
    let state = Rc::new(RefCell::new(State::default()));
    parent.on_inspect_compiled_details({
        let (parent, window, target, state) = (
            parent.as_weak(),
            window.as_weak(),
            access.downgrade(),
            state.clone(),
        );
        move || {
            super::admitted(&parent, &target, |_, shared| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                state.borrow_mut().offset = 0;
                if let Err(error) = perform(&window, shared, &state, Action::Inspect) {
                    window.set_message(error.into());
                }
                let _ = window.show();
            })
        }
    });
    let callback = handler(&window, access, &state, |_| Action::Prepare);
    window.on_prepare_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Cancel);
    window.on_cancel_preparation(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Clear);
    window.on_clear_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Show);
    window.on_show_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Hide);
    window.on_hide_details(move || callback(0));
    window.on_change_lighting(handler(&window, access, &state, Action::Lighting));
    window.on_choose_style(handler(&window, access, &state, Action::Style));
    let callback = handler(&window, access, &state, |_| Action::ApplyLighting);
    window.on_apply_lighting(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Inspect);
    window.on_refresh_inspection(move || callback(0));
    window.on_page_records(handler(&window, access, &state, Action::Page));
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
    pub(super) fn refresh(&self, session: &Session) {
        if !self.is_visible() {
            return;
        }
        let observation = session.compiled_details_observation();
        if self.state.borrow().observation.as_ref() == Some(&observation) {
            return;
        }
        self.window.set_rows("".into());
        self.window.set_previous(false);
        self.window.set_next(false);
        self.window.set_page("".into());
        self.window
            .set_worker_active(observation["worker_active"].as_bool().unwrap_or(false));
        let settings = match read_lighting(&observation["lighting_options"]) {
            Ok(settings) => settings,
            Err(error) => {
                self.window.set_message(error.into());
                return;
            }
        };
        show_lighting(&self.window, &settings);
        {
            let mut state = self.state.borrow_mut();
            state.lighting = settings;
            state.styles.clear();
            state.selected_style = None;
            self.window.set_style_options(slint::ModelRc::default());
            show_style(&self.window, &state);
        }
        self.window
            .set_lighting_state("Refresh inspection to check channels and styles".into());
        self.state.borrow_mut().token = observation["token"]
            .as_str()
            .and_then(|token| token.parse::<u64>().ok())
            .filter(|token| *token != 0);
        self.window
            .set_has_token(self.state.borrow().token.is_some());
        self.window.set_state(
            format!(
                "{} · requested {} · displayed {} · complete geometry {}",
                observation["status"].as_str().unwrap_or("not_loaded"),
                observation["requested"],
                observation["displayed"],
                observation["complete"]
            )
            .into(),
        );
        self.window.set_build_state("State changed. Refresh inspection to check source/output freshness and show native records.".into());
        self.window
            .set_message(observation["error"].as_str().unwrap_or("").into());
        self.state.borrow_mut().offset = 0;
        self.state.borrow_mut().observation = Some(observation);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }
    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Compiled details", self.window.window(), errors);
    }
}

#[cfg(test)]
mod tests;
