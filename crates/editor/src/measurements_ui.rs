use std::{cell::RefCell, rc::Rc};

use glam::DVec3;
use map_core::measurements::Measurement;
use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::measurements::Request;
use crate::session::{Actor, Session};
use crate::{MeasurementRow, MeasurementsWindow};

#[derive(Default)]
struct Form {
    generation: u64,
    revision: u64,
    snapshot: String,
    selected: Option<u32>,
    owned_token: Option<u64>,
    displayed_token: Option<u64>,
    dirty: bool,
}

pub struct Controller {
    pub window: MeasurementsWindow,
    form: Rc<RefCell<Form>>,
}

fn coordinate(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .map_err(|_| "Enter numeric X, Y and Z coordinates".into())
}

fn request(window: &MeasurementsWindow, selected: Option<u32>) -> Result<Request, String> {
    Ok(Request {
        id: selected,
        name: window.get_measurement_name().into(),
        start: DVec3::new(
            coordinate(&window.get_start_x())?,
            coordinate(&window.get_start_y())?,
            coordinate(&window.get_start_z())?,
        ),
        end: DVec3::new(
            coordinate(&window.get_end_x())?,
            coordinate(&window.get_end_y())?,
            coordinate(&window.get_end_z())?,
        ),
    })
}

fn load(window: &MeasurementsWindow, measurement: &Measurement) {
    window.set_selected_id(measurement.id.to_string().into());
    window.set_measurement_name(measurement.name.clone().into());
    window.set_start_x(measurement.start.x.to_string().into());
    window.set_start_y(measurement.start.y.to_string().into());
    window.set_start_z(measurement.start.z.to_string().into());
    window.set_end_x(measurement.end.x.to_string().into());
    window.set_end_y(measurement.end.y.to_string().into());
    window.set_end_z(measurement.end.z.to_string().into());
}

fn result(measurement: &Measurement) -> String {
    let delta = measurement.components();
    format!(
        "Distance {:.6} units\nΔX {:+.6} · ΔY {:+.6} · ΔZ {:+.6}",
        measurement.distance(),
        delta.x,
        delta.y,
        delta.z
    )
}

fn refresh(window: &MeasurementsWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.measurement_state();
    let draft = &state["draft"];
    let mut form = form.borrow_mut();
    form.displayed_token = draft["token"].as_u64();
    let current = form.generation == session.document_generation()
        && form.revision == session.document().revision();
    let valid = draft["valid"] == true;
    let reviewed = valid
        && form.owned_token.is_some()
        && form.owned_token == form.displayed_token
        && draft["owner"] == Actor::User.label();
    let editable = current && state["metadata_error"].is_null() && state["edit_error"].is_null();
    window.set_editable(editable);
    window.set_can_preview(editable && (draft.is_null() || draft["owner"] == Actor::User.label()));
    window.set_can_review(editable && valid && (!reviewed || form.dirty));
    window.set_ready(editable && reviewed && !form.dirty);
    window.set_can_delete(editable && draft.is_null() && form.selected.is_some());
    window.set_can_frame(current && ((reviewed && !form.dirty) || form.selected.is_some()));
    window.set_show_guides(state["show_guides"] == true);
    let entries = session
        .measurements()
        .map(|metadata| {
            metadata
                .measurements
                .into_iter()
                .map(|measurement| {
                    let delta = measurement.components();
                    MeasurementRow {
                        id: measurement.id.to_string().into(),
                        name: measurement.name.clone().into(),
                        detail: format!(
                            "{:.3} units · ΔX {:+.3} · ΔY {:+.3} · ΔZ {:+.3}",
                            measurement.distance(),
                            delta.x,
                            delta.y,
                            delta.z
                        )
                        .into(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    window.set_measurements(Rc::new(VecModel::from(entries)).into());
    let status = if let Some(error) = state["metadata_error"].as_str() {
        format!(
            "Imported measurement metadata error: {error}. Metadata is preserved; edits are disabled."
        )
    } else if !current {
        "Map changed. Reopen Measurements… or select a saved measurement to continue.".into()
    } else if let Some(error) = state["edit_error"].as_str() {
        error.into()
    } else if draft.is_null() {
        "Preview checks the endpoints without changing the map or Undo history.".into()
    } else if !valid {
        "The measurement preview is stale. Preview again, or Cancel your draft.".into()
    } else if !reviewed {
        format!(
            "Preview by {}. Review preview loads its endpoints and takes ownership.",
            draft["owner"].as_str().unwrap_or("another editor")
        )
    } else if form.dirty {
        "Endpoints or name changed. Preview again before Apply.".into()
    } else {
        "Preview is current. Apply stores the measurement in one Undo step.".into()
    };
    window.set_preview_status(status.into());
    let readout = request(window, form.selected)
        .and_then(|request| {
            let measurement = Measurement {
                id: request.id.unwrap_or(1),
                name: request.name,
                start: request.start,
                end: request.end,
                extra: Vec::new(),
            };
            if measurement.start.is_finite() && measurement.end.is_finite() {
                Ok(result(&measurement))
            } else {
                Err("Enter finite endpoint coordinates".into())
            }
        })
        .unwrap_or_else(|error| error);
    window.set_result(readout.into());
}

fn remember_snapshot(form: &mut Form, session: &Session) {
    form.generation = session.document_generation();
    form.revision = session.document().revision();
    form.snapshot = session.measurement_stamp();
}

fn adopt(
    window: &MeasurementsWindow,
    session: &Session,
    form: &RefCell<Form>,
    state: &Value,
) -> Result<(), String> {
    let draft = &state["draft"];
    let data = &draft["measurement"];
    let point = |name: &str| -> Result<DVec3, String> {
        let values = serde_json::from_value::<[f64; 3]>(data[name].clone())
            .map_err(|_| "Invalid measurement preview endpoints")?;
        Ok(DVec3::from_array(values))
    };
    let measurement = Measurement {
        id: data["id"]
            .as_u64()
            .ok_or("Invalid measurement preview ID")? as u32,
        name: data["name"]
            .as_str()
            .ok_or("Invalid measurement preview name")?
            .into(),
        start: point("start")?,
        end: point("end")?,
        extra: Vec::new(),
    };
    measurement.validate()?;
    load(window, &measurement);
    let mut form = form.borrow_mut();
    form.selected = (draft["replace"] == true).then_some(measurement.id);
    form.owned_token = draft["token"].as_u64();
    form.dirty = false;
    remember_snapshot(&mut form, session);
    Ok(())
}

fn cancel_owned(session: &mut Session, form: &RefCell<Form>) {
    if let Some(token) = form.borrow_mut().owned_token.take() {
        let _ = session.cancel_measurement(Actor::User, token);
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = MeasurementsWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_edited({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().dirty = true;
                refresh(&window.unwrap(), &lock(&shared), &form);
            }
        });
        window.on_choose({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |id| {
                let window = window.unwrap();
                let session = lock(&shared);
                let result = session.measurements().and_then(|metadata| {
                    if !session.measurement_state()["draft"].is_null() {
                        return Err(
                            "Apply or Cancel the preview before choosing a measurement".into()
                        );
                    }
                    let id = id.parse::<u32>().map_err(|_| "Invalid measurement ID")?;
                    metadata
                        .measurements
                        .into_iter()
                        .find(|measurement| measurement.id == id)
                        .ok_or_else(|| "Measurement no longer exists".into())
                });
                match result {
                    Ok(measurement) => {
                        load(&window, &measurement);
                        let mut form = form.borrow_mut();
                        form.selected = Some(measurement.id);
                        form.dirty = false;
                        form.owned_token = None;
                        remember_snapshot(&mut form, &session);
                        window
                            .set_message("Edit endpoints or name, then Preview and Apply.".into());
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_new_measurement({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let session = lock(&shared);
                if !session.measurement_state()["draft"].is_null() {
                    window.set_message(
                        "Apply or Cancel the preview before starting another measurement.".into(),
                    );
                    return;
                }
                *form.borrow_mut() = Form::default();
                remember_snapshot(&mut form.borrow_mut(), &session);
                load(
                    &window,
                    &Measurement {
                        id: 1,
                        name: "".into(),
                        start: DVec3::ZERO,
                        end: DVec3::X * 64.0,
                        extra: Vec::new(),
                    },
                );
                window.set_selected_id("".into());
                refresh(&window, &session, &form);
            }
        });
        window.on_preview({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = {
                    let form = form.borrow();
                    request(&window, form.selected).and_then(|request| {
                        session.preview_measurement(Actor::User, request, &form.snapshot)
                    })
                };
                match result.and_then(|state| adopt(&window, &session, &form, &state)) {
                    Ok(()) => window.set_message(
                        "Check the gold ruler and colored XYZ components in the viewport.".into(),
                    ),
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_review({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = form
                    .borrow()
                    .displayed_token
                    .ok_or_else(|| "No measurement preview to review".to_string())
                    .and_then(|token| session.review_measurement(Actor::User, token));
                if let Err(error) = result.and_then(|state| adopt(&window, &session, &form, &state))
                {
                    window.set_message(error.into());
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_apply({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = {
                    let form = form.borrow();
                    if form.dirty {
                        Err("Preview the changed endpoints or name before Apply".to_string())
                    } else {
                        form.owned_token
                            .ok_or_else(|| "Review or Preview the measurement before Apply".to_string())
                            .and_then(|token| session.apply_measurement(Actor::User, token))
                    }
                };
                match result {
                    Ok(state) => {
                        let mut form = form.borrow_mut();
                        form.owned_token = None;
                        form.selected = state["id"].as_u64().map(|id| id as u32);
                        remember_snapshot(&mut form, &session);
                        window.set_message("Measurement saved. Save the VMF to keep it. Undo restores the previous state.".into());
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_delete_measurement({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = {
                    let form = form.borrow();
                    form.selected
                        .ok_or_else(|| "Select a saved measurement first".to_string())
                        .and_then(|id| session.delete_measurement(Actor::User, id, &form.snapshot))
                };
                match result {
                    Ok(_) => {
                        let mut form = form.borrow_mut();
                        form.selected = None;
                        remember_snapshot(&mut form, &session);
                        window.set_selected_id("".into());
                        window.set_message("Measurement deleted. Undo restores it.".into());
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_frame({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let form_ref = form.borrow();
                let (id, token) = if form_ref.owned_token.is_some() && !form_ref.dirty {
                    (None, form_ref.owned_token)
                } else {
                    (form_ref.selected, None)
                };
                if let Err(error) =
                    session.frame_measurement(id, token, 16.0 / 9.0, &form_ref.snapshot)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_view_changed({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let snapshot = session.measurement_stamp();
                match session.set_measurement_guides(window.get_show_guides(), &snapshot) {
                    Ok(_) => {
                        if form.borrow().generation == session.document_generation()
                            && form.borrow().revision == session.document().revision()
                        {
                            form.borrow_mut().snapshot = session.measurement_stamp();
                        }
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_cancel({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                cancel_owned(&mut lock(&shared), &form);
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let (shared, form) = (shared.clone(), form.clone());
            move || {
                cancel_owned(&mut lock(&shared), &form);
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, form })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        if !self.window.window().is_visible() {
            *self.form.borrow_mut() = Form::default();
            remember_snapshot(&mut self.form.borrow_mut(), session);
            load(
                &self.window,
                &Measurement {
                    id: 1,
                    name: "".into(),
                    start: DVec3::ZERO,
                    end: DVec3::X * 64.0,
                    extra: Vec::new(),
                },
            );
            self.window.set_selected_id("".into());
            self.window.set_message(
                "Enter exact world coordinates. Endpoints are not snapped or attached to objects."
                    .into(),
            );
        }
        self.refresh(session);
        self.window.show()
    }
}

#[cfg(test)]
mod tests;
