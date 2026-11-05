use std::{cell::RefCell, rc::Rc};

use editor_core::session::baked_surfaces::{FrameTarget, MAX_SAMPLE_ROWS};
use render::compiled_faces::props::VertexSample;
use serde_json::Value;
use slint::ComponentHandle;

use super::{DocumentAccess, Session};
use crate::{BakedPropSampleRow, BakedPropSamplesWindow, session::Actor};

pub(super) struct Form {
    window: BakedPropSamplesWindow,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    token: Option<u64>,
    prop: Option<u32>,
    offset: usize,
    next_offset: Option<usize>,
    samples: Vec<Sample>,
    selected: Option<usize>,
    observation: Option<Value>,
}

struct Sample {
    data: VertexSample,
    frame_available: bool,
    guide_available: bool,
}

#[derive(Clone, Copy)]
enum Action {
    Refresh,
    Select(i32),
    Page(i32),
    Frame,
    ShowGuide,
    ClearGuide,
}

fn vector<T: ToString, const N: usize>(values: [T; N]) -> String {
    values.map(|value| value.to_string()).join(", ")
}

fn coordinates([x, y, z]: [f32; 3]) -> String {
    format!("X {x} · Y {y} · Z {z}")
}

fn clear_rows(window: &BakedPropSamplesWindow, state: &mut State) {
    state.samples.clear();
    state.selected = None;
    state.next_offset = None;
    window.set_rows(slint::ModelRc::default());
    window.set_selected(-1);
    window.set_can_frame(false);
    window.set_can_show_guide(false);
    window.set_previous(false);
    window.set_next(false);
    window.set_page("".into());
    window.set_availability("".into());
    window.set_details("Choose a sample to inspect its exact prepared position and color.".into());
    window.set_technical_details("".into());
}

fn show_selection(window: &BakedPropSamplesWindow, state: &State) {
    let sample = state.selected.and_then(|index| state.samples.get(index));
    window.set_selected(state.selected.map_or(-1, |index| index as i32));
    window.set_can_frame(sample.is_some_and(|sample| sample.frame_available));
    window.set_can_show_guide(sample.is_some_and(|sample| sample.guide_available));
    if let Some(sample) = sample {
        let sample = &sample.data;
        window.set_details(
            format!(
                "World position: {}\nWorld normal: {}\nStored RGB: R {} · G {} · B {}\nChannel: {} · {}",
                coordinates(sample.position),
                coordinates(sample.normal),
                sample.bgra[2],
                sample.bgra[1],
                sample.bgra[0],
                sample.channel.name().to_ascii_uppercase(),
                if sample.used_by_triangles { "Used by triangles" } else { "Unused hardware occurrence" },
            )
            .into(),
        );
        window.set_technical_details(
            format!(
                "Prop {} · section {} · local vertex {} · hardware {} · LOD {}\nRequested exposure: {} EV\nEffective exposure: {} EV · {}\nStored BGRA: {}\nDecoded linear RGB: {}\nBefore exposure: {}\nExposed RGB: {}",
                sample.prop,
                sample.section,
                sample.vertex,
                sample.hardware_index,
                sample.lod,
                sample.exposure_ev,
                sample.effective_exposure_ev,
                sample.exposure_owner.replace('_', " "),
                vector(sample.bgra),
                vector(sample.decoded_linear),
                vector(sample.pre_ev),
                vector(sample.exposed),
            )
            .into(),
        );
    } else {
        window.set_details(
            "Choose a sample to inspect its exact prepared position and color.".into(),
        );
        window.set_technical_details("".into());
    }
}

fn guide_status(window: &BakedPropSamplesWindow, guide: &Value, token: Option<u64>) {
    let guide_token = guide["token"]
        .as_str()
        .and_then(|token| token.parse::<u64>().ok());
    window
        .set_can_clear_guide(guide["requested"] == true && token.is_some() && guide_token == token);
    window.set_guide_status(
        guide["summary"]
            .as_str()
            .or_else(|| guide["status"].as_str())
            .unwrap_or_default()
            .into(),
    );
}

fn section_text(section: &Value) -> Result<String, String> {
    let ordinal = section["section"]
        .as_u64()
        .ok_or("Prepared section identity is unavailable")?;
    let lod = section["lod"]
        .as_u64()
        .ok_or("Prepared section LOD is unavailable")?;
    let vertices = section["vertices"]
        .as_u64()
        .ok_or("Prepared section vertex count is unavailable")?;
    let triangles = section["triangles"]
        .as_u64()
        .ok_or("Prepared section triangle count is unavailable")?;
    let material = section["material"]
        .as_u64()
        .ok_or("Prepared section material is unavailable")?;
    let hardware = section["hardware_start"]
        .as_u64()
        .ok_or("Prepared section hardware identity is unavailable")?;
    let mins: [f32; 3] = serde_json::from_value(section["mins"].clone())
        .map_err(|error| format!("Prepared section bounds are unavailable: {error}"))?;
    let maxs: [f32; 3] = serde_json::from_value(section["maxs"].clone())
        .map_err(|error| format!("Prepared section bounds are unavailable: {error}"))?;
    Ok(format!(
        "Section {ordinal} · LOD {lod} · {vertices} vertices · {triangles} triangles · material {material} · hardware start {hardware}\nBounds: {} → {}",
        coordinates(mins),
        coordinates(maxs)
    ))
}

fn inspect(
    window: &BakedPropSamplesWindow,
    state: &mut State,
    session: &mut Session,
) -> Result<(), String> {
    let token = state.token.ok_or("Select a prepared static prop first")?;
    let prop = state.prop.ok_or("Select a prepared static prop first")?;
    let report = session.inspect_baked_prop_samples(token, prop, state.offset, MAX_SAMPLE_ROWS)?;
    let samples = report["page"]["rows"]
        .as_array()
        .ok_or("Prepared sample rows are unavailable")?
        .iter()
        .map(|row| {
            Ok(Sample {
                data: serde_json::from_value(row.clone())
                    .map_err(|error| format!("Prepared sample page is unavailable: {error}"))?,
                frame_available: row["frame_available"]
                    .as_bool()
                    .ok_or("Prepared sample frame readiness is unavailable")?,
                guide_available: row["guide_available"]
                    .as_bool()
                    .ok_or("Prepared sample guide readiness is unavailable")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let sections = report["sections"]
        .as_array()
        .ok_or("Prepared section metadata is unavailable")?;
    let sections = sections
        .iter()
        .map(section_text)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n\n");
    let chosen = state
        .selected
        .and_then(|index| state.samples.get(index))
        .map(|sample| (sample.data.prop, sample.data.section, sample.data.vertex));
    state.samples = samples;
    state.selected = chosen.and_then(|chosen| {
        state.samples.iter().position(|sample| {
            (sample.data.prop, sample.data.section, sample.data.vertex) == chosen
        })
    });
    state.next_offset = report["page"]["next_offset"]
        .as_u64()
        .map(|offset| offset as usize);
    window.set_rows(
        Rc::new(slint::VecModel::from(
            state
                .samples
                .iter()
                .map(|sample| &sample.data)
                .map(|sample| BakedPropSampleRow {
                    title: format!(
                        "Vertex {} · section {}{}",
                        sample.vertex,
                        sample.section,
                        if sample.used_by_triangles {
                            ""
                        } else {
                            " · unused"
                        },
                    )
                    .into(),
                    position: format!("Position: {}", coordinates(sample.position)).into(),
                })
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    window.set_summary(report["summary"].as_str().unwrap_or_default().into());
    let mut unavailable = Vec::new();
    for key in ["frame_unavailable_reason", "guide_unavailable_reason"] {
        if let Some(reason) = report[key].as_str()
            && !unavailable.contains(&reason)
        {
            unavailable.push(reason);
        }
    }
    window.set_availability(unavailable.join("\n").into());
    window.set_sections(sections.into());
    window.set_previous(state.offset > 0);
    window.set_next(state.next_offset.is_some());
    let total = report["page"]["total"]
        .as_u64()
        .ok_or("Prepared sample count is unavailable")?;
    window.set_page(
        if state.samples.is_empty() {
            format!("0 of {total}")
        } else {
            format!(
                "{}–{} of {total}",
                state.offset + 1,
                state.offset + state.samples.len()
            )
        }
        .into(),
    );
    guide_status(window, &report["guide"], state.token);
    show_selection(window, state);
    window.set_message("".into());
    state.observation = Some(session.baked_surfaces_observation());
    Ok(())
}

fn perform(
    window: &BakedPropSamplesWindow,
    state: &RefCell<State>,
    session: &mut Session,
    action: Action,
) -> Result<(), String> {
    let mut state = state.borrow_mut();
    match action {
        Action::Select(index) => {
            state.selected = usize::try_from(index)
                .ok()
                .filter(|index| *index < state.samples.len());
            show_selection(window, &state);
            return Ok(());
        }
        Action::Page(direction) => {
            state.offset = if direction < 0 {
                state.offset.saturating_sub(MAX_SAMPLE_ROWS)
            } else {
                state.next_offset.ok_or("No next sample page")?
            };
            state.selected = None;
        }
        Action::Frame | Action::ShowGuide => {
            let sample = state
                .selected
                .and_then(|index| state.samples.get(index))
                .ok_or("Choose a prepared sample first")?;
            let sample = &sample.data;
            let token = state.token.ok_or("Select a prepared static prop first")?;
            if matches!(action, Action::Frame) {
                session.frame_baked_surface_record(
                    Actor::User,
                    token,
                    FrameTarget::StaticPropSample {
                        prop: sample.prop,
                        section: sample.section,
                        vertex: sample.vertex,
                    },
                    window.get_aspect(),
                )?;
            } else {
                session.set_baked_prop_sample_guide(
                    Actor::User,
                    token,
                    sample.prop,
                    sample.section,
                    sample.vertex,
                )?;
            }
        }
        Action::ClearGuide => {
            let guide = session.clear_baked_prop_sample_guide(
                Actor::User,
                state.token.ok_or("Select a prepared static prop first")?,
            )?;
            guide_status(window, &guide, state.token);
            state.observation = Some(session.baked_surfaces_observation());
            return Ok(());
        }
        Action::Refresh => {}
    }
    inspect(window, &mut state, session)
}

fn handler(
    window: &BakedPropSamplesWindow,
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
            target.with_session(|session| {
                let result = perform(&window, &state, session, action(value));
                if result.is_err() {
                    let observation = session.baked_surfaces_observation();
                    guide_status(
                        &window,
                        &observation["prop_sample_guide"],
                        state.borrow().token,
                    );
                    state.borrow_mut().observation = Some(observation);
                }
                result
            })
        });
        if let Err(error) = result {
            clear_rows(&window, &mut state.borrow_mut());
            window.set_summary("".into());
            window.set_sections("".into());
            window.set_message(error.into());
        }
    }
}

impl Form {
    pub(super) fn new(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = BakedPropSamplesWindow::new()?;
        let state = Rc::new(RefCell::new(State::default()));
        window.on_choose_sample(handler(&window, access, &state, Action::Select));
        window.on_page_samples(handler(&window, access, &state, Action::Page));
        let callback = handler(&window, access, &state, |_| Action::Refresh);
        window.on_refresh_inspection(move || callback(0));
        let callback = handler(&window, access, &state, |_| Action::Frame);
        window.on_frame_sample(move || callback(0));
        let callback = handler(&window, access, &state, |_| Action::ShowGuide);
        window.on_show_guide(move || callback(0));
        let callback = handler(&window, access, &state, |_| Action::ClearGuide);
        window.on_clear_guide(move || callback(0));
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade()
                    && let Err(error) = window.hide()
                {
                    window.set_message(error.to_string().into());
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_close_dialog();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self { window, state })
    }

    pub(super) fn show(&self, session: &mut Session, token: u64, prop: u32) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        *state = State {
            token: Some(token),
            prop: Some(prop),
            ..State::default()
        };
        clear_rows(&self.window, &mut state);
        self.window.set_summary("".into());
        self.window.set_sections("".into());
        let result = inspect(&self.window, &mut state, session);
        if let Err(error) = &result {
            let observation = session.baked_surfaces_observation();
            guide_status(&self.window, &observation["prop_sample_guide"], state.token);
            state.observation = Some(observation);
            self.window.set_message(error.clone().into());
        }
        self.window.show().map_err(|error| error.to_string())?;
        result
    }

    pub(super) fn refresh(&self, session: &Session) {
        if !self.is_visible() {
            return;
        }
        let observation = session.baked_surfaces_observation();
        let mut state = self.state.borrow_mut();
        if state.observation.as_ref() != Some(&observation) {
            clear_rows(&self.window, &mut state);
            self.window.set_summary(
                "Prepared state changed. Refresh inspection to check these exact samples.".into(),
            );
            self.window.set_sections("".into());
            self.window.set_message(
                observation["error"]
                    .as_str()
                    .or_else(|| observation["suppression_reason"].as_str())
                    .unwrap_or_default()
                    .into(),
            );
            guide_status(&self.window, &observation["prop_sample_guide"], state.token);
            state.observation = Some(observation);
        }
    }

    pub(super) fn set_aspect(&self, aspect: f32) {
        self.window.set_aspect(aspect);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Baked prop samples", self.window.window(), errors);
    }
}
