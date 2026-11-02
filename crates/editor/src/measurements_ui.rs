use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::DVec3;
use map_core::measurements::{Endpoint as StoredEndpoint, Measurement};
use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation};

use crate::session::measurements::{Endpoint, PickMode, Request};
use crate::session::workplane::Update as PlaneUpdate;
use crate::session::{Actor, Pointer, Session};
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

#[derive(Default)]
struct PlaneForm {
    snapshot: String,
    dirty: bool,
}

pub struct Controller {
    pub window: MeasurementsWindow,
    form: Rc<RefCell<Form>>,
    viewport: Rc<Cell<(u32, u32)>>,
    plane: Rc<RefCell<PlaneForm>>,
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
    window.set_start_origin_object(
        measurement
            .origin_reference(StoredEndpoint::Start)
            .ok()
            .flatten()
            .map(|id| format!("entity:{id}"))
            .unwrap_or_default()
            .into(),
    );
    window.set_end_origin_object(
        measurement
            .origin_reference(StoredEndpoint::End)
            .ok()
            .flatten()
            .map(|id| format!("entity:{id}"))
            .unwrap_or_default()
            .into(),
    );
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
    let picking = !draft["picking"].is_null();
    window.set_editable(editable);
    window.set_can_preview(editable && (draft.is_null() || draft["owner"] == Actor::User.label()));
    window.set_can_review(editable && valid && (!reviewed || form.dirty));
    window.set_ready(
        editable
            && reviewed
            && !form.dirty
            && !picking
            && draft["measurement"]["available"] == true,
    );
    window.set_can_attach(editable && reviewed && !form.dirty && !picking);
    window.set_can_pick(editable && reviewed && !form.dirty);
    window.set_picking(picking && reviewed);
    window.set_can_delete(editable && draft.is_null() && form.selected.is_some());
    window.set_can_frame(
        current && !picking && ((reviewed && !form.dirty) || form.selected.is_some()),
    );
    window.set_show_guides(state["show_guides"] == true);
    let displayed = if form.owned_token.is_some() && form.owned_token == form.displayed_token {
        Some(&draft["measurement"])
    } else {
        state["measurements"].as_array().and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["id"].as_u64() == form.selected.map(u64::from))
        })
    };
    for endpoint in [Endpoint::Start, Endpoint::End] {
        let name = match endpoint {
            Endpoint::Start => "start",
            Endpoint::End => "end",
        };
        let attachment = displayed.map(|data| &data["attachments"][name]);
        let attached = attachment.is_some_and(|data| !data.is_null());
        let status = attachment.filter(|data|!data.is_null()).map(|data| {
            let object = data["object"].as_str().unwrap_or("entity");
            let target = if data["kind"] == "brush_surface" {
                format!("surface point on solid:{} / side:{}", data["solid"], data["side"])
            } else if data["kind"] == "terrain_surface" {
                format!("terrain surface point on solid:{} / side:{} · triangle {} · grid {}", data["solid"], data["side"], data["triangle"], data["grid_size"])
            } else if data["kind"] == "entity_local" {
                match serde_json::from_value::<[f64; 3]>(data["local_offset"].clone()) {
                    Ok(offset) => {
                        let [x, y, z] = offset.map(|value| {
                            if value.abs() < 0.0005 { 0.0 } else { value }
                        });
                        format!("local point ({x:.3}, {y:.3}, {z:.3})")
                    }
                    Err(_) => "local point with invalid coordinates".into(),
                }
            } else {
                "origin".into()
            };
            match data["error"].as_str() {
                Some(error) if !current => format!("{name}: {object} {target} unavailable — {error}. Select the ruler again to review stored coordinates."),
                Some(error) => format!("{name}: {object} {target} unavailable — {error}. Stored coordinates shown; Use {name} world explicitly replaces this reference."),
                None => format!("{name}: follows {object} {target}. Detach freezes its current world point."),
            }
        }).unwrap_or_else(||format!("{name}: fixed world point. Entity attachments use an ID or the selected point entity; surface attachments use the endpoint's last surface pick."));
        match endpoint {
            Endpoint::Start => {
                window.set_start_attached(attached);
                window.set_start_origin_status(status.into());
            }
            Endpoint::End => {
                window.set_end_attached(attached);
                window.set_end_origin_status(status.into());
            }
        }
    }
    let entries = state["measurements"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|measurement| {
            let detail = if let Some(error) = measurement["error"].as_str() {
                format!("Unavailable: {error}")
            } else {
                match (
                    measurement["distance"].as_f64(),
                    serde_json::from_value::<[f64; 3]>(measurement["components"].clone()),
                ) {
                    (Some(distance), Ok([x, y, z])) => {
                        format!("{distance:.3} units · ΔX {x:+.3} · ΔY {y:+.3} · ΔZ {z:+.3}")
                    }
                    _ => "Unavailable: measurement coordinates could not be resolved".into(),
                }
            };
            MeasurementRow {
                id: measurement["id"]
                    .as_u64()
                    .unwrap_or_default()
                    .to_string()
                    .into(),
                name: measurement["name"].as_str().unwrap_or_default().into(),
                detail: detail.into(),
            }
        })
        .collect::<Vec<_>>();
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
    } else if let Some(error) = draft["measurement"]["error"].as_str() {
        format!(
            "Ruler unavailable: {error}. Attach a valid point or explicitly use a world endpoint before Apply."
        )
    } else if form.dirty {
        "Endpoints or name changed. Preview again before Apply.".into()
    } else if picking {
        draft["picking"]["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "Pick the {} endpoint {}. Escape cancels picking and keeps the endpoints.",
                    draft["picking"]["endpoint"].as_str().unwrap_or("selected"),
                    if draft["picking"]["mode"] == "plane" {
                        "on the enabled reference plane"
                    } else {
                        "on a visible surface"
                    }
                )
            })
    } else {
        "Preview is current. Apply stores the measurement in one Undo step.".into()
    };
    window.set_preview_status(status.into());
    let readout = if let Some(error) = displayed.and_then(|data| data["error"].as_str()) {
        format!("Unavailable: {error}")
    } else {
        request(window, form.selected)
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
            .unwrap_or_else(|error| error)
    };
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
        let value = if data[name].is_null() {
            &data[format!("stored_{name}")]
        } else {
            &data[name]
        };
        let values = serde_json::from_value::<[f64; 3]>(value.clone())
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
    load(window, &measurement);
    let origin_object = |endpoint: &str| {
        let attachment = &data["attachments"][endpoint];
        if attachment["kind"] == "entity_origin" || attachment["kind"] == "entity_local" {
            attachment["object"].as_str().unwrap_or("")
        } else {
            ""
        }
    };
    window.set_start_origin_object(origin_object("start").into());
    window.set_end_origin_object(origin_object("end").into());
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

#[derive(Clone, Copy)]
enum OriginAction {
    Attach,
    AttachLocal,
    AttachSurface,
    Detach,
    World,
}

fn change_origin(
    window: &MeasurementsWindow,
    session: &mut Session,
    form: &RefCell<Form>,
    endpoint: Endpoint,
    action: OriginAction,
) -> Result<Value, String> {
    let form = form.borrow();
    if form.dirty {
        return Err(
            "Preview the changed name or world coordinates before changing attachments".into(),
        );
    }
    let token = form
        .owned_token
        .ok_or("Preview or Review the measurement before changing attachments")?;
    match action {
        OriginAction::Attach | OriginAction::AttachLocal => {
            let text = match endpoint {
                Endpoint::Start => window.get_start_origin_object(),
                Endpoint::End => window.get_end_origin_object(),
            };
            let object = if text.trim().is_empty() {
                if session.selection().len() != 1 {
                    return Err("Enter entity:<id> or select exactly one point entity".into());
                }
                *session.selection().first().unwrap()
            } else {
                crate::identity::parse_id(&text)?
            };
            match action {
                OriginAction::Attach => {
                    session.attach_measurement_origin(Actor::User, token, endpoint, object)
                }
                _ => session.attach_measurement_local(Actor::User, token, endpoint, object),
            }
        }
        OriginAction::AttachSurface => {
            session.attach_measurement_surface(Actor::User, token, endpoint)
        }
        OriginAction::Detach => session.detach_measurement_origin(Actor::User, token, endpoint),
        OriginAction::World => {
            let request = request(window, form.selected)?;
            let point = match endpoint {
                Endpoint::Start => request.start,
                Endpoint::End => request.end,
            };
            session.set_measurement_world_endpoint(Actor::User, token, endpoint, point)
        }
    }
}

fn with_session<R>(
    window: &slint::Weak<MeasurementsWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&MeasurementsWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

fn load_plane(
    window: &MeasurementsWindow,
    form: &RefCell<PlaneForm>,
    state: &Value,
) -> Result<(), String> {
    let vector = |key: &str| {
        serde_json::from_value::<[f64; 3]>(state[key].clone())
            .map_err(|_| format!("Reference plane {key} is unavailable"))
    };
    let [ox, oy, oz] = vector("origin")?.map(|value| value.to_string());
    let [nx, ny, nz] = vector("normal")?.map(|value| value.to_string());
    let [tx, ty, tz] = vector("tangent")?.map(|value| value.to_string());
    let snapshot = state["snapshot"]
        .as_str()
        .ok_or("Reference plane snapshot is unavailable")?;
    let grid = state["grid_size"]
        .as_f64()
        .ok_or("Reference plane grid spacing is unavailable")?;
    let extent = state["half_extent"]
        .as_f64()
        .ok_or("Reference plane extent is unavailable")?;
    window.set_plane_origin_x(ox.into());
    window.set_plane_origin_y(oy.into());
    window.set_plane_origin_z(oz.into());
    window.set_plane_normal_x(nx.into());
    window.set_plane_normal_y(ny.into());
    window.set_plane_normal_z(nz.into());
    window.set_plane_tangent_x(tx.into());
    window.set_plane_tangent_y(ty.into());
    window.set_plane_tangent_z(tz.into());
    window.set_plane_grid(grid.to_string().into());
    window.set_plane_extent(extent.to_string().into());
    window.set_plane_enabled(state["enabled"] == true);
    window.set_plane_show(state["show"] == true);
    window.set_plane_snap(state["snap"] == true);
    let mut form = form.borrow_mut();
    form.snapshot = snapshot.into();
    form.dirty = false;
    Ok(())
}

fn refresh_plane(window: &MeasurementsWindow, session: &Session, form: &RefCell<PlaneForm>) {
    let state = session.workplane_state();
    let form = form.borrow();
    let stale = state["snapshot"].as_str() != Some(form.snapshot.as_str());
    window.set_plane_stale(stale);
    window.set_plane_can_pick(
        !stale && !form.dirty && state["enabled"] == true && state["ready"] == true,
    );
    window.set_plane_status(format!(
        "Current plane: {} · {} · {}\nOrigin {} · normal {}\nGrid {} units · displayed spacing {} units{}",
        if state["enabled"] == true {"Enabled"} else {"Disabled"},
        if state["shown"] == true {"Grid shown"} else {"Grid hidden"},
        if state["snap"] == true {"Plane snapping on"} else {"Plane snapping off"},
        state["origin"], state["normal"], state["grid_size"], state["display_spacing"],
        if form.dirty {"\nChanged fields need Apply plane."} else {""},
    ).into());
}

fn plane_update(window: &MeasurementsWindow) -> Result<PlaneUpdate, String> {
    Ok(PlaneUpdate {
        origin: Some([
            coordinate(&window.get_plane_origin_x())?,
            coordinate(&window.get_plane_origin_y())?,
            coordinate(&window.get_plane_origin_z())?,
        ]),
        normal: Some([
            coordinate(&window.get_plane_normal_x())?,
            coordinate(&window.get_plane_normal_y())?,
            coordinate(&window.get_plane_normal_z())?,
        ]),
        tangent: Some([
            coordinate(&window.get_plane_tangent_x())?,
            coordinate(&window.get_plane_tangent_y())?,
            coordinate(&window.get_plane_tangent_z())?,
        ]),
        enabled: Some(window.get_plane_enabled()),
        show: Some(window.get_plane_show()),
        snap: Some(window.get_plane_snap()),
        grid_size: Some(coordinate(&window.get_plane_grid())?),
        half_extent: Some(coordinate(&window.get_plane_extent())?),
    })
}

#[derive(Clone, Copy)]
enum PlaneAction {
    Apply,
    Reset,
    Reload,
}

fn bind_plane(
    window: &MeasurementsWindow,
    access: &DocumentContinuation,
) -> Rc<RefCell<PlaneForm>> {
    let form = Rc::new(RefCell::new(PlaneForm::default()));
    let callback = |action| {
        let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
        move || {
            with_session(&window, &access, |window, session| {
                let snapshot = form.borrow().snapshot.clone();
                let result = match action {
                    PlaneAction::Apply => plane_update(window).and_then(|update| {
                        session.set_workplane(Actor::User, update, Some(&snapshot))
                    }),
                    PlaneAction::Reset => session.reset_workplane(Actor::User, Some(&snapshot)),
                    PlaneAction::Reload => Ok(session.workplane_state()),
                };
                match result.and_then(|state| load_plane(window, &form, &state)) {
                    Ok(()) => window.set_plane_message(
                        match action {
                            PlaneAction::Apply => {
                                "Reference plane applied. Plane picks use these guide settings."
                            }
                            PlaneAction::Reset => "Reference plane reset and disabled.",
                            PlaneAction::Reload => {
                                "Current reference plane loaded. Unapplied fields discarded."
                            }
                        }
                        .into(),
                    ),
                    Err(error) => window.set_plane_message(error.into()),
                }
                refresh_plane(window, session, &form);
            });
        }
    };
    window.on_plane_apply(callback(PlaneAction::Apply));
    window.on_plane_reset(callback(PlaneAction::Reset));
    window.on_plane_reload(callback(PlaneAction::Reload));
    window.on_plane_edited({
        let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
        move || {
            with_session(&window, &access, |window, session| {
                form.borrow_mut().dirty = true;
                refresh_plane(window, session, &form);
            });
        }
    });
    window.on_plane_preset({
        let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
        move |preset| {
            with_session(&window, &access, |window, session| {
                let (normal, tangent) = match preset {
                    0 => (DVec3::Z, DVec3::X),
                    1 => (-DVec3::Y, DVec3::X),
                    2 => (DVec3::X, DVec3::Y),
                    _ => {
                        window.set_plane_message("Choose XY, XZ or YZ orientation.".into());
                        return;
                    }
                };
                window.set_plane_normal_x(normal.x.to_string().into());
                window.set_plane_normal_y(normal.y.to_string().into());
                window.set_plane_normal_z(normal.z.to_string().into());
                window.set_plane_tangent_x(tangent.x.to_string().into());
                window.set_plane_tangent_y(tangent.y.to_string().into());
                window.set_plane_tangent_z(tangent.z.to_string().into());
                window.set_plane_enabled(true);
                form.borrow_mut().dirty = true;
                window.set_plane_message(
                    "Orientation prepared at the entered origin. Apply plane to use it.".into(),
                );
                refresh_plane(window, session, &form);
            });
        }
    });
    form
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let access = access.downgrade();
        let window = MeasurementsWindow::new()?;
        let plane = bind_plane(&window, &access);
        let form = Rc::new(RefCell::new(Form::default()));
        let viewport = Rc::new(Cell::new((1024, 576)));
        for endpoint in [Endpoint::Start, Endpoint::End] {
            for action in [
                OriginAction::Attach,
                OriginAction::AttachLocal,
                OriginAction::AttachSurface,
                OriginAction::Detach,
                OriginAction::World,
            ] {
                let (window_ref, access, form) = (window.as_weak(), access.clone(), form.clone());
                let callback = move || {
                    with_session(&window_ref, &access, |window, session| {
                        let other_object = match endpoint {
                            Endpoint::Start => window.get_end_origin_object(),
                            Endpoint::End => window.get_start_origin_object(),
                        };
                        match change_origin(window, session, &form, endpoint, action)
                        .and_then(|state| {
                            adopt(window, session, &form, &state)?;
                            match endpoint {
                                Endpoint::Start => window.set_end_origin_object(other_object),
                                Endpoint::End => window.set_start_origin_object(other_object),
                            }
                            Ok(())
                        }) {
                        Ok(()) => window.set_message("Endpoint preview updated. Check the ruler, then Apply to save in one Undo step.".into()),
                        Err(error) => window.set_message(error.into()),
                    }
                        refresh(window, session, &form);
                    });
                };
                match (endpoint, action) {
                    (Endpoint::Start, OriginAction::Attach) => {
                        window.on_attach_start_origin(callback)
                    }
                    (Endpoint::End, OriginAction::Attach) => window.on_attach_end_origin(callback),
                    (Endpoint::Start, OriginAction::AttachLocal) => {
                        window.on_attach_start_local(callback)
                    }
                    (Endpoint::End, OriginAction::AttachLocal) => {
                        window.on_attach_end_local(callback)
                    }
                    (Endpoint::Start, OriginAction::AttachSurface) => {
                        window.on_attach_start_surface(callback)
                    }
                    (Endpoint::End, OriginAction::AttachSurface) => {
                        window.on_attach_end_surface(callback)
                    }
                    (Endpoint::Start, OriginAction::Detach) => {
                        window.on_detach_start_origin(callback)
                    }
                    (Endpoint::End, OriginAction::Detach) => window.on_detach_end_origin(callback),
                    (Endpoint::Start, OriginAction::World) => window.on_world_start(callback),
                    (Endpoint::End, OriginAction::World) => window.on_world_end(callback),
                }
            }
        }
        for endpoint in [Endpoint::Start, Endpoint::End] {
            for mode in [PickMode::Surface, PickMode::Plane] {
                let (pick_window, access, form, viewport) = (
                    window.as_weak(),
                    access.clone(),
                    form.clone(),
                    viewport.clone(),
                );
                let pick = move || {
                    with_session(&pick_window, &access, |window, session| {
                        let camera = session.camera();
                        let snapshot = session.measurement_pick_stamp();
                        let result = {
                            let form = form.borrow();
                            if form.dirty {
                                Err("Preview the changed endpoints or name before picking"
                                    .to_string())
                            } else {
                                form.owned_token
                                    .ok_or_else(|| {
                                        "Preview or Review the measurement before picking"
                                            .to_string()
                                    })
                                    .and_then(|token| {
                                        session.begin_measurement_pick_mode(
                                            Actor::User,
                                            token,
                                            endpoint,
                                            mode,
                                            (camera, viewport.get()),
                                            &snapshot,
                                        )
                                    })
                            }
                        };
                        match result.and_then(|state| adopt(window, session, &form, &state)) {
                            Ok(()) => {
                                let _ = window.hide();
                            }
                            Err(error) => window.set_message(error.into()),
                        }
                        refresh(window, session, &form);
                    });
                };
                match (endpoint, mode) {
                    (Endpoint::Start, PickMode::Surface) => window.on_pick_start(pick),
                    (Endpoint::End, PickMode::Surface) => window.on_pick_end(pick),
                    (Endpoint::Start, PickMode::Plane) => window.on_pick_start_plane(pick),
                    (Endpoint::End, PickMode::Plane) => window.on_pick_end_plane(pick),
                }
            }
        }
        window.on_cancel_pick({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = form
                        .borrow()
                        .owned_token
                        .ok_or_else(|| {
                            "Review the measurement before cancelling endpoint picking".to_string()
                        })
                        .and_then(|token| session.cancel_measurement_pick(Actor::User, token));
                    if let Err(error) =
                        result.and_then(|state| adopt(window, session, &form, &state))
                    {
                        window.set_message(error.into());
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_edited({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    form.borrow_mut().dirty = true;
                    refresh(window, session, &form);
                });
            }
        });
        window.on_choose({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move |id| {
                with_session(&window, &access, |window, session| {
                    let result = session.measurements().and_then(|metadata| {
                        if !session.measurement_state()["draft"].is_null() {
                            return Err(
                                "Apply or Cancel the preview before choosing a measurement".into(),
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
                            let mut displayed = measurement.clone();
                            for endpoint in [StoredEndpoint::Start, StoredEndpoint::End] {
                                if let Ok(point) =
                                    measurement.resolve_endpoint(session.document().vmf(), endpoint)
                                {
                                    displayed.set_stored_endpoint(endpoint, point);
                                }
                            }
                            load(window, &displayed);
                            let mut form = form.borrow_mut();
                            form.selected = Some(measurement.id);
                            form.dirty = false;
                            form.owned_token = None;
                            remember_snapshot(&mut form, session);
                            window.set_message(
                                "Edit endpoints or name, then Preview and Apply.".into(),
                            );
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_new_measurement({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    if !session.measurement_state()["draft"].is_null() {
                        window.set_message(
                            "Apply or Cancel the preview before starting another measurement."
                                .into(),
                        );
                        return;
                    }
                    *form.borrow_mut() = Form::default();
                    remember_snapshot(&mut form.borrow_mut(), session);
                    load(
                        window,
                        &Measurement {
                            id: 1,
                            name: "".into(),
                            start: DVec3::ZERO,
                            end: DVec3::X * 64.0,
                            extra: Vec::new(),
                        },
                    );
                    window.set_selected_id("".into());
                    refresh(window, session, &form);
                });
            }
        });
        window.on_preview({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = {
                        let form = form.borrow();
                        request(window, form.selected).and_then(|request| {
                            session.preview_measurement(Actor::User, request, &form.snapshot)
                        })
                    };
                    match result.and_then(|state| adopt(window, session, &form, &state)) {
                        Ok(()) => window.set_message(
                            "Check the gold ruler and colored XYZ components in the viewport."
                                .into(),
                        ),
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_review({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = form
                        .borrow()
                        .displayed_token
                        .ok_or_else(|| "No measurement preview to review".to_string())
                        .and_then(|token| session.review_measurement(Actor::User, token));
                    if let Err(error) =
                        result.and_then(|state| adopt(window, session, &form, &state))
                    {
                        window.set_message(error.into());
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_apply({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {

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
                        remember_snapshot(&mut form, session);
                        window.set_message("Measurement saved. Save the VMF to keep it. Undo restores the previous state.".into());
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(window, session, &form);

                });
            }
        });
        window.on_delete_measurement({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = {
                        let form = form.borrow();
                        form.selected
                            .ok_or_else(|| "Select a saved measurement first".to_string())
                            .and_then(|id| {
                                session.delete_measurement(Actor::User, id, &form.snapshot)
                            })
                    };
                    match result {
                        Ok(_) => {
                            let mut form = form.borrow_mut();
                            form.selected = None;
                            remember_snapshot(&mut form, session);
                            window.set_selected_id("".into());
                            window.set_message("Measurement deleted. Undo restores it.".into());
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_frame({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
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
                });
            }
        });
        window.on_view_changed({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
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
                    refresh(window, session, &form);
                });
            }
        });
        window.on_cancel({
            let (window, access, form, plane) = (
                window.as_weak(),
                access.clone(),
                form.clone(),
                plane.clone(),
            );
            move || {
                *plane.borrow_mut() = PlaneForm::default();
                with_session(&window, &access, |_window, session| {
                    cancel_owned(session, &form);
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            form,
            viewport,
            plane,
        })
    }

    pub fn set_viewport(&self, size: (u32, u32)) {
        self.viewport.set(size);
    }

    pub fn pick_at_pixel(
        &self,
        session: &mut Session,
        position: glam::DVec2,
        size: (u32, u32),
    ) -> Result<(), String> {
        let token = self
            .form
            .borrow()
            .owned_token
            .ok_or("Review the measurement before picking an endpoint")?;
        let state = session.pick_measurement_pixel(
            Actor::User,
            token,
            Pointer {
                position: position.floor() + glam::DVec2::splat(0.5),
                size,
            },
        )?;
        adopt(&self.window, session, &self.form, &state)?;
        self.window.set_message(
            "Endpoint picked. Check the exact coordinates, then pick the other endpoint or Apply."
                .into(),
        );
        self.refresh(session);
        let _ = self.window.show();
        Ok(())
    }

    pub fn cancel_pick(&self, session: &mut Session) -> Result<(), String> {
        let token = self
            .form
            .borrow()
            .owned_token
            .ok_or("Review the measurement before cancelling endpoint picking")?;
        let state = session.cancel_measurement_pick(Actor::User, token)?;
        adopt(&self.window, session, &self.form, &state)?;
        self.window
            .set_message("Endpoint picking cancelled; the preview endpoints are unchanged.".into());
        self.refresh(session);
        let _ = self.window.show();
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
        refresh_plane(&self.window, session, &self.plane);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        if self.plane.borrow().snapshot.is_empty() {
            self.window.set_plane_message("".into());
            if let Err(error) = load_plane(&self.window, &self.plane, &session.workplane_state()) {
                self.window.set_plane_message(error.into());
            }
        }
        if !self.window.window().is_visible()
            && (self.form.borrow().owned_token.is_none()
                || self.form.borrow().owned_token
                    != session.measurement_state()["draft"]["token"].as_u64())
        {
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
                "Enter exact world coordinates and Preview, then use Pick start or Pick end for visible surfaces."
                    .into(),
            );
        }
        self.refresh(session);
        self.window.show()
    }
}
