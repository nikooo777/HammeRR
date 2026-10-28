use std::{cell::RefCell, rc::Rc};

use editor_core::session::baked_surfaces::{FrameTarget, MAX_ROWS, PreparationOptions, RecordKind};
use render::LightingChannel;
use render::compiled_faces::{LightingSettings, StyleGain};
use serde_json::Value;
use slint::ComponentHandle;

use super::{CompileWindow, DocumentAccess, Session, SharedSession, lock};
use crate::{BakedSurfacesWindow, session::Actor};

pub(super) struct Form {
    window: BakedSurfacesWindow,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    token: Option<u64>,
    offset: usize,
    next_offset: Option<usize>,
    observation: Option<Value>,
    lighting: LightingSettings,
    styles: Vec<u8>,
    selected_style: Option<u8>,
    faces: Vec<FaceTarget>,
    selected_face: Option<usize>,
    record_kind: RecordKind,
}

struct FaceTarget {
    face: usize,
    can_frame: bool,
    summary: String,
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
    ApplyLighting,
    Style(i32),
    Face(i32),
    RecordKind(i32),
    Frame,
}

fn read_lighting(value: &Value) -> Result<LightingSettings, String> {
    Ok(LightingSettings {
        channel: LightingChannel::parse(
            value["channel"]
                .as_str()
                .ok_or("Missing baked surface channel")?,
        )?,
        exposure_ev: value["exposure_ev"]
            .as_f64()
            .ok_or("Missing baked surface exposure")?,
        style_gains: serde_json::from_value(value["style_gains"].clone())
            .map_err(|error| format!("Invalid baked surface style settings: {error}"))?,
    })
}

fn show_lighting(window: &BakedSurfacesWindow, settings: &LightingSettings) {
    window.set_channel_index(i32::from(settings.channel == LightingChannel::Hdr));
    window.set_exposure_ev(settings.exposure_ev as f32);
}

fn show_preparation(window: &BakedSurfacesWindow, report: &Value) -> Result<(), String> {
    let options: PreparationOptions = serde_json::from_value(report["preparation_options"].clone())
        .map_err(|error| format!("Invalid baked surface preparation options: {error}"))?;
    window.set_compiled_static_props(options.compiled_static_props);
    window.set_draw_lod(i32::from(options.draw_lod));
    Ok(())
}

fn show_style(window: &BakedSurfacesWindow, state: &State) {
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

fn keep_style_gain(window: &BakedSurfacesWindow, state: &mut State) {
    if let Some(style) = state.selected_style {
        let gain = f64::from(window.get_style_gain());
        if gain == LightingSettings::default().gain(style) {
            state
                .lighting
                .style_gains
                .retain(|entry| entry.style != style);
            return;
        }
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
    window: &BakedSurfacesWindow,
    state: &mut State,
) -> Result<LightingSettings, String> {
    let channel = match window.get_channel_index() {
        0 => LightingChannel::Ldr,
        1 => LightingChannel::Hdr,
        _ => return Err("Choose LDR or HDR baked surfaces".into()),
    };
    keep_style_gain(window, state);
    Ok(LightingSettings {
        channel,
        exposure_ev: f64::from(window.get_exposure_ev()),
        style_gains: state.lighting.style_gains.clone(),
    })
}

fn show_state(window: &BakedSurfacesWindow, report: &Value) {
    let status = match report["status"].as_str() {
        _ if report["displayed"] == true => "Visible",
        Some("not_loaded") => "Not prepared",
        Some("loading") => "Preparing…",
        Some("cancelling") => "Cancelling…",
        Some("prepared") => "Prepared · hidden",
        Some("ready") => "Ready · hidden",
        Some("stale") => "Stale",
        Some("cancelled") => "Cancelled",
        Some("context_suppressed") => "Temporarily suppressed",
        _ => "Unavailable",
    };
    window.set_state(status.into());
    window.set_worker_active(report["worker_active"].as_bool().unwrap_or(false));
}

fn clear_faces(window: &BakedSurfacesWindow, state: &mut State) {
    state.faces.clear();
    state.selected_face = None;
    state.next_offset = None;
    window.set_face_rows(slint::ModelRc::default());
    window.set_current_row(-1);
    window.set_can_frame(false);
    window.set_previous(false);
    window.set_next(false);
    window.set_page("".into());
    window.set_face_summary(
        "Choose a compiled record to frame its mesh bounds. Regions include whole faces and props."
            .into(),
    );
}

fn present(
    window: &BakedSurfacesWindow,
    state: &mut State,
    report: Value,
    observation: Value,
) -> Result<(), String> {
    let lighting = read_lighting(&report["lighting_options"])?;
    let styles: Option<Vec<u8>> =
        serde_json::from_value(report["lighting"]["available_styles"].clone())
            .map_err(|error| format!("Invalid baked surface style list: {error}"))?;
    state.token = report["token"]
        .as_str()
        .and_then(|token| token.parse().ok());
    state.observation = Some(observation);
    state.lighting = lighting;
    state.styles = styles.unwrap_or_default();
    state.selected_style = state
        .selected_style
        .filter(|style| state.styles.contains(style))
        .or_else(|| state.styles.first().copied());
    window.set_has_token(state.token.is_some());
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
    show_preparation(window, &report)?;
    show_style(window, state);
    show_state(window, &report);
    window.set_summary(
        if let Some(total) = report["page"]["total"].as_u64() {
            format!(
                "{total} final {} · build {}",
                if state.record_kind == RecordKind::Faces {
                    "faces"
                } else {
                    "static props"
                },
                report["build"]["status"].as_str().unwrap_or("unavailable")
            )
        } else {
            "Prepare a successful lit build to view final surfaces and optional static props."
                .into()
        }
        .into(),
    );
    window.set_channels(
        report["lighting"]["channels"]
            .as_array()
            .map(|channels| {
                channels
                    .iter()
                    .map(|entry| {
                        format!(
                            "{} {}",
                            entry["channel"]
                                .as_str()
                                .unwrap_or("?")
                                .to_ascii_uppercase(),
                            if entry["available"] == true {
                                "available"
                            } else {
                                "unavailable"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .unwrap_or_else(|| "Inspect preparation to check channels".into())
            .into(),
    );
    clear_faces(window, state);
    let rows: Vec<_> = report["page"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            let key = if state.record_kind == RecordKind::Faces {
                "face"
            } else {
                "prop"
            };
            let face = row[key].as_u64().ok_or("Missing native record ordinal")? as usize;
            let receiver = row["receiver"].as_str().unwrap_or("unavailable");
            let material = row[if state.record_kind == RecordKind::Faces {
                "material"
            } else {
                "model"
            }]
            .as_str()
            .unwrap_or("unavailable");
            state.faces.push(FaceTarget {
                face,
                can_frame: row["frame_available"].as_bool().unwrap_or(false),
                summary: format!(
                    "{} {face} · {} · {material}",
                    if state.record_kind == RecordKind::Faces {
                        "Face"
                    } else {
                        "Static prop"
                    },
                    receiver.replace('_', " ")
                ),
            });
            Ok(slint::ModelRc::new(slint::VecModel::from(vec![
                slint::language::StandardListViewItem::from(face.to_string().as_str()),
                slint::language::StandardListViewItem::from(receiver.replace('_', " ").as_str()),
                slint::language::StandardListViewItem::from(material),
            ])))
        })
        .collect::<Result<_, String>>()?;
    window.set_face_rows(Rc::new(slint::VecModel::from(rows)).into());
    state.next_offset = report["page"]["next_offset"]
        .as_u64()
        .map(|value| value as usize);
    window.set_previous(state.offset > 0);
    window.set_next(state.next_offset.is_some());
    window.set_page(
        report["page"]["total"]
            .as_u64()
            .map(|total| {
                if state.faces.is_empty() {
                    format!("0 of {total}")
                } else {
                    format!(
                        "{}–{} of {total}",
                        state.offset + 1,
                        state.offset + state.faces.len()
                    )
                }
            })
            .unwrap_or_default()
            .into(),
    );
    let mut messages = Vec::new();
    for reason in [
        "prepare_unavailable_reason",
        "record_unavailable_reason",
        "suppression_reason",
        "error",
    ] {
        if let Some(reason) = report[reason].as_str() {
            messages.push(reason.to_owned());
        }
    }
    for diagnostics in [
        &report["build"]["diagnostics"],
        &report["preview_assets"]["diagnostics"],
    ] {
        messages.extend(
            diagnostics
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    window.set_message(messages.join("\n").into());
    Ok(())
}

fn perform(
    window: &BakedSurfacesWindow,
    shared: &SharedSession,
    state: &Rc<RefCell<State>>,
    action: Action,
) -> Result<(), String> {
    match action {
        Action::Style(index) => {
            let mut state = state.borrow_mut();
            let selected = usize::try_from(index)
                .ok()
                .and_then(|index| state.styles.get(index))
                .copied()
                .ok_or("Choose a stored baked surface style")?;
            keep_style_gain(window, &mut state);
            state.selected_style = Some(selected);
            show_style(window, &state);
            return Ok(());
        }
        Action::Face(index) => {
            let mut state = state.borrow_mut();
            state.selected_face = usize::try_from(index)
                .ok()
                .filter(|&index| index < state.faces.len());
            if let Some(face) = state.selected_face.and_then(|index| state.faces.get(index)) {
                window.set_face_summary(face.summary.clone().into());
                window.set_can_frame(face.can_frame);
            } else {
                window.set_can_frame(false);
            }
            return Ok(());
        }
        Action::Prepare => {
            let draw_lod =
                u8::try_from(window.get_draw_lod()).map_err(|_| "Choose Draw LOD 0–7")?;
            let request = lock(shared).begin_baked_surfaces_with_options(
                Actor::User,
                PreparationOptions {
                    compiled_static_props: window.get_compiled_static_props(),
                    draw_lod,
                },
            )?;
            let token = request.token();
            state.borrow_mut().token = Some(token);
            state.borrow_mut().offset = 0;
            let worker_session = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("baked-surfaces".into())
                .spawn(move || {
                    let prepared = request.run();
                    let _ = lock(&worker_session).finish_baked_surfaces(Actor::User, prepared);
                });
            if let Err(error) = spawned {
                let _ = lock(shared).fail_baked_surfaces(
                    Actor::User,
                    token,
                    format!("Could not start baked surface preparation: {error}"),
                );
            }
        }
        _ => {}
    }
    let mut session = lock(shared);
    let token = state.borrow().token;
    let mut offset = state.borrow().offset;
    match action {
        Action::Cancel => {
            session
                .cancel_baked_surfaces(Actor::User, token.ok_or("Prepare baked surfaces first")?)?;
        }
        Action::Clear => {
            session
                .clear_baked_surfaces(Actor::User, token.ok_or("Prepare baked surfaces first")?)?;
            offset = 0;
        }
        Action::Show | Action::Hide => {
            session.set_baked_surfaces_visible(
                Actor::User,
                token.ok_or("Prepare baked surfaces first")?,
                matches!(action, Action::Show),
            )?;
        }
        Action::ApplyLighting => {
            let settings = chosen_lighting(window, &mut state.borrow_mut())?;
            session.set_baked_surfaces_options(
                Actor::User,
                token.ok_or("Prepare baked surfaces first")?,
                settings,
            )?;
        }
        Action::Frame => {
            let state = state.borrow();
            let face = state
                .selected_face
                .and_then(|index| state.faces.get(index))
                .ok_or("Choose a compiled record to frame")?;
            let target = match state.record_kind {
                RecordKind::Faces => FrameTarget::Face(face.face),
                RecordKind::StaticProps => FrameTarget::StaticProp(
                    u32::try_from(face.face)
                        .map_err(|_| "Static-prop ordinal exceeds its native range")?,
                ),
            };
            session.frame_baked_surface_record(
                Actor::User,
                token.ok_or("Prepare baked surfaces first")?,
                target,
                window.get_aspect(),
            )?;
        }
        Action::Page(direction) => {
            offset = if direction < 0 {
                offset.saturating_sub(MAX_ROWS)
            } else {
                state.borrow().next_offset.ok_or("No next record page")?
            };
        }
        Action::Style(_) | Action::Face(_) => unreachable!(),
        Action::RecordKind(index) => {
            state.borrow_mut().record_kind = match index {
                0 => RecordKind::Faces,
                1 => RecordKind::StaticProps,
                _ => return Err("Choose Faces or Static props".into()),
            };
            offset = 0;
        }
        Action::Prepare | Action::Inspect => {}
    }
    let token =
        if matches!(action, Action::Clear | Action::Inspect | Action::Prepare) && offset == 0 {
            None
        } else {
            token
        };
    let report = session.inspect_baked_surface_records(
        token,
        state.borrow().record_kind,
        offset,
        MAX_ROWS,
    )?;
    state.borrow_mut().offset = offset;
    present(
        window,
        &mut state.borrow_mut(),
        report,
        session.baked_surfaces_observation(),
    )
}

fn handler(
    window: &BakedSurfacesWindow,
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
            if matches!(action(value), Action::ApplyLighting | Action::Style(_)) {
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
    let window = BakedSurfacesWindow::new()?;
    let state = Rc::new(RefCell::new(State::default()));
    parent.on_inspect_baked_surfaces({
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
    window.on_prepare_surfaces(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Cancel);
    window.on_cancel_preparation(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Clear);
    window.on_clear_surfaces(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Show);
    window.on_show_surfaces(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Hide);
    window.on_hide_surfaces(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::ApplyLighting);
    window.on_apply_lighting(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Inspect);
    window.on_refresh_inspection(move || callback(0));
    window.on_choose_style(handler(&window, access, &state, Action::Style));
    window.on_choose_face(handler(&window, access, &state, Action::Face));
    window.on_choose_record_kind(handler(&window, access, &state, Action::RecordKind));
    window.on_page_faces(handler(&window, access, &state, Action::Page));
    let callback = handler(&window, access, &state, |_| Action::Frame);
    window.on_frame_face(move || callback(0));
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
        let observation = session.baked_surfaces_observation();
        if self.state.borrow().observation.as_ref() == Some(&observation) {
            return;
        }
        let lighting = match read_lighting(&observation["lighting_options"]) {
            Ok(lighting) => lighting,
            Err(error) => {
                self.window.set_message(error.into());
                return;
            }
        };
        let mut state = self.state.borrow_mut();
        clear_faces(&self.window, &mut state);
        state.lighting = lighting;
        state.styles.clear();
        state.selected_style = None;
        state.token = observation["token"]
            .as_str()
            .and_then(|token| token.parse().ok());
        state.offset = 0;
        self.window.set_has_token(state.token.is_some());
        self.window.set_style_options(slint::ModelRc::default());
        show_lighting(&self.window, &state.lighting);
        if let Err(error) = show_preparation(&self.window, &observation) {
            self.window.set_message(error.into());
        }
        show_style(&self.window, &state);
        show_state(&self.window, &observation);
        self.window.set_summary(
            "State changed. Inspect to check the build, channels and final surfaces.".into(),
        );
        self.window
            .set_channels("Inspect to check channel availability".into());
        self.window
            .set_message(observation["error"].as_str().unwrap_or("").into());
        state.observation = Some(observation);
    }

    pub(super) fn set_aspect(&self, aspect: f32) {
        self.window.set_aspect(aspect);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Baked surfaces", self.window.window(), errors);
    }
}
