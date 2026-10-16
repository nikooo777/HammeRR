use std::{cell::RefCell, rc::Rc};

use glam::DVec3;
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    LinkedPathPoint, LinkedPathWindow,
    runtime::{SharedSession, lock},
    session::{
        Actor, Session,
        linked_path::{ChangeRequest, EditNode, MAX_POINTS, MIN_POINTS, Request},
    },
};

const POINT_STEP: f64 = 128.0;

#[derive(Default)]
struct Form {
    generation: u64,
    owned_token: Option<u64>,
    dirty: bool,
    existing_snapshot: Option<String>,
    existing_seed: Option<u32>,
}

pub struct Controller {
    pub window: LinkedPathWindow,
    form: Rc<RefCell<Form>>,
}

fn coordinate(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "Enter finite X, Y and Z coordinates for every point".into())
}

fn point_row(point: DVec3, index: usize) -> LinkedPathPoint {
    LinkedPathPoint {
        label: (index + 1).to_string().into(),
        x: source_formats::vmf::format_number(point.x).into(),
        y: source_formats::vmf::format_number(point.y).into(),
        z: source_formats::vmf::format_number(point.z).into(),
        link: Default::default(),
        id: Default::default(),
        name: Default::default(),
    }
}

fn set_rows(window: &LinkedPathWindow, mut rows: Vec<LinkedPathPoint>) {
    for (index, row) in rows.iter_mut().enumerate() {
        row.label = (index + 1).to_string().into();
        row.link = Default::default();
    }
    window.set_points(ModelRc::new(VecModel::from(rows)));
}

fn request(window: &LinkedPathWindow) -> Result<Request, String> {
    Ok(Request {
        classname: match window.get_class_index() {
            0 => "path_corner",
            1 => "path_track",
            _ => return Err("Choose path_corner or path_track".into()),
        }
        .into(),
        prefix: window.get_prefix().into(),
        points: window
            .get_points()
            .iter()
            .map(|row| {
                Ok(DVec3::new(
                    coordinate(&row.x)?,
                    coordinate(&row.y)?,
                    coordinate(&row.z)?,
                ))
            })
            .collect::<Result<_, String>>()?,
        closed: window.get_closed(),
    })
}

fn load_preview(window: &LinkedPathWindow, preview: &Value) -> Result<(), String> {
    let invalid = "Linked path preview data is invalid; preview it again";
    let class_index = match preview["classname"].as_str() {
        Some("path_corner") => 0,
        Some("path_track") => 1,
        _ => return Err(invalid.into()),
    };
    let prefix = preview["prefix"].as_str().ok_or(invalid)?;
    let closed = preview["closed"].as_bool().ok_or(invalid)?;
    let points = preview["points"].as_array().ok_or(invalid)?;
    let nodes = preview["nodes"].as_array().ok_or(invalid)?;
    let existing = preview["existing"] == true;
    preview_seed(preview)?;
    let removed = preview["removed"]
        .as_array()
        .ok_or(invalid)?
        .iter()
        .map(|node| {
            Ok(format!(
                "{} · {}",
                node["targetname"].as_str().ok_or(invalid)?,
                node["id"].as_str().ok_or(invalid)?
            )
            .into())
        })
        .collect::<Result<Vec<slint::SharedString>, String>>()?;
    if !(MIN_POINTS..=MAX_POINTS).contains(&points.len()) || nodes.len() != points.len() {
        return Err(invalid.into());
    }
    let rows = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let values = point
                .as_array()
                .filter(|values| values.len() == 3)
                .ok_or(invalid)?;
            let mut coordinates = [0.0; 3];
            for (coordinate, value) in coordinates.iter_mut().zip(values) {
                *coordinate = value
                    .as_f64()
                    .filter(|value| value.is_finite() && value.abs() <= crate::spatial::MAX_EXTENT)
                    .ok_or_else(|| {
                        format!(
                            "Linked path preview point {} has invalid coordinates",
                            index + 1
                        )
                    })?;
            }
            let node = &nodes[index];
            let name = node["targetname"]
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or(invalid)?;
            let target = match node.get("target") {
                Some(Value::Null) => "end",
                Some(Value::String(target)) => {
                    if target.is_empty() {
                        "end"
                    } else {
                        target
                    }
                }
                _ => return Err(invalid.into()),
            };
            let mut row = point_row(DVec3::from_array(coordinates), index);
            let id = node["id"].as_str().ok_or(invalid)?;
            if !matches!(
                crate::identity::parse_id(id)?,
                map_core::ObjectId::Entity(_)
            ) {
                return Err(invalid.into());
            }
            row.id = if existing {
                match node.get("source_id") {
                    Some(Value::Null) => "",
                    Some(Value::String(source)) if source == id => source,
                    _ => return Err(invalid.into()),
                }
            } else {
                id
            }
            .into();
            row.name = name.into();
            row.link = format!("{name} → {target}").into();
            Ok(row)
        })
        .collect::<Result<Vec<_>, String>>()?;
    window.set_class_index(class_index);
    window.set_existing(preview["existing"] == true);
    window.set_prefix(prefix.into());
    window.set_closed(closed);
    window.set_points(ModelRc::new(VecModel::from(rows)));
    window.set_removed_nodes(ModelRc::new(VecModel::from(removed)));
    Ok(())
}

fn preview_seed(preview: &Value) -> Result<Option<u32>, String> {
    if preview["existing"] != true {
        return Ok(None);
    }
    let map_core::ObjectId::Entity(id) = crate::identity::parse_id(
        preview["seed"]
            .as_str()
            .ok_or("Existing path preview has no source seed")?,
    )?
    else {
        return Err("Existing path seed requires an entity ID".into());
    };
    Ok(Some(id))
}

fn adopt_preview(
    window: &LinkedPathWindow,
    state: &Value,
    form: &RefCell<Form>,
) -> Result<(), String> {
    form.borrow_mut().dirty = true;
    let preview = &state["preview"];
    if preview["owner"] != Actor::User.label() || preview["valid"] != true {
        return Err("Linked path preview is not current and owned by you".into());
    }
    let token = preview["token"]
        .as_u64()
        .ok_or("Linked path preview has no token")?;
    form.borrow_mut().owned_token = Some(token);
    load_preview(window, preview)?;
    form.borrow_mut().existing_seed = preview_seed(preview)?;
    form.borrow_mut().existing_snapshot = state["snapshot"].as_str().map(str::to_owned);
    form.borrow_mut().dirty = false;
    window.set_message("Review the ordered points and links before Apply.".into());
    Ok(())
}

fn refresh(window: &LinkedPathWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.linked_path();
    let preview = &state["preview"];
    let form = form.borrow();
    let same_map = form.generation == session.document_generation();
    let editable = same_map && state["editable"] == true;
    let token = preview["token"].as_u64();
    let user_owned = preview["owner"] == Actor::User.label();
    let current = !preview.is_null() && preview["valid"] == true;
    let reviewed = token.is_some() && token == form.owned_token && user_owned;
    window.set_editable(editable);
    window.set_can_load(
        editable && session.selected_ids().len() == 1 && (preview.is_null() || user_owned),
    );
    window.set_can_preview(editable && (preview.is_null() || user_owned));
    window.set_can_review(editable && current && (!reviewed || form.dirty));
    window.set_ready(editable && current && reviewed && !form.dirty);
    window.set_preview_status(
        if !same_map {
            "Map changed. Reopen Create linked path… to start in this map.".into()
        } else if preview.is_null() {
            "Preview the path to check its names, links and position in the viewport.".into()
        } else if !current {
            "The preview is stale. Preview again to check the current map.".into()
        } else if !reviewed {
            format!(
                "Preview by {}. Review preview to load its points and take ownership.",
                preview["owner"].as_str().unwrap_or("another editor")
            )
        } else if form.dirty {
            "Settings changed. Preview again before applying.".into()
        } else {
            format!(
                "{} linked {} entities · {} · Apply {} the path in one Undo step.",
                preview["points"].as_array().map_or(0, Vec::len),
                preview["classname"].as_str().unwrap_or("path"),
                if preview["closed"] == true {
                    "closed loop"
                } else {
                    "open path"
                },
                if preview["existing"] == true {
                    "updates"
                } else {
                    "creates"
                }
            )
        }
        .into(),
    );
}

fn edited(window: &LinkedPathWindow, session: &Session, form: &RefCell<Form>) {
    form.borrow_mut().dirty = true;
    window.set_message("".into());
    refresh(window, session, form);
}

fn cancel_owned(session: &mut Session, form: &RefCell<Form>) {
    if let Some(token) = form.borrow_mut().owned_token.take() {
        let _ = session.cancel_linked_path(Actor::User, token);
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = LinkedPathWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(840.0, 700.0));
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_load_selected({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let session = lock(&shared);
                let result = (|| {
                    if form.borrow().generation != session.document_generation() {
                        return Err("Map changed; reopen Create linked path…".into());
                    }
                    let selected = session.selected_ids();
                    let [map_core::ObjectId::Entity(id)] = selected.as_slice() else {
                        return Err("Select one existing path_corner or path_track node".into());
                    };
                    let state = session.inspect_existing_linked_path(*id)?;
                    load_preview(&window, &state)?;
                    form.borrow_mut().existing_seed = preview_seed(&state)?;
                    form.borrow_mut().existing_snapshot = state["snapshot"].as_str().map(str::to_owned);
                    form.borrow_mut().dirty = true;
                    window.set_message("Add, remove or move rows, then Preview. Retained names, class and open/closed topology stay fixed. Referenced nodes and connections cannot be removed.".into());
                    Ok::<_, String>(())
                })();
                if let Err(error) = result { window.set_message(error.into()); }
                refresh(&window, &session, &form);
            }
        });
        window.on_new_path({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                window.set_existing(false);
                window.set_prefix("path".into());
                window.set_closed(false);
                let rows = window
                    .get_points()
                    .iter()
                    .enumerate()
                    .map(|(index, mut row)| {
                        row.id = Default::default();
                        row.name = Default::default();
                        row.label = (index + 1).to_string().into();
                        row
                    })
                    .collect();
                set_rows(&window, rows);
                form.borrow_mut().existing_snapshot = None;
                form.borrow_mut().existing_seed = None;
                window.set_removed_nodes(ModelRc::new(VecModel::default()));
                edited(&window, &lock(&shared), &form);
            }
        });
        window.on_edited({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || edited(&window.unwrap(), &lock(&shared), &form)
        });
        window.on_add_point({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut rows = window.get_points().iter().collect::<Vec<_>>();
                if rows.len() >= MAX_POINTS {
                    return;
                }
                let point =
                    match request(&window).and_then(|request| {
                        request.points.last().copied().ok_or_else(|| {
                            "No path point to extend; reopen Create linked path…".into()
                        })
                    }) {
                        Ok(point) => point,
                        Err(error) => {
                            window.set_message(error.into());
                            return;
                        }
                    };
                let step = if point.x <= crate::spatial::MAX_EXTENT - POINT_STEP {
                    POINT_STEP
                } else {
                    -POINT_STEP
                };
                rows.push(point_row(point + DVec3::X * step, rows.len()));
                set_rows(&window, rows);
                edited(&window, &lock(&shared), &form);
            }
        });
        window.on_remove_point({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |index| {
                let window = window.unwrap();
                let mut rows = window.get_points().iter().collect::<Vec<_>>();
                if rows.len() > MIN_POINTS && index >= 0 && (index as usize) < rows.len() {
                    rows.remove(index as usize);
                    set_rows(&window, rows);
                    edited(&window, &lock(&shared), &form);
                }
            }
        });
        window.on_move_point({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |index, direction| {
                let window = window.unwrap();
                let mut rows = window.get_points().iter().collect::<Vec<_>>();
                let target = index + direction;
                if direction.abs() == 1
                    && index >= 0
                    && target >= 0
                    && (index as usize) < rows.len()
                    && (target as usize) < rows.len()
                {
                    rows.swap(index as usize, target as usize);
                    set_rows(&window, rows);
                    edited(&window, &lock(&shared), &form);
                }
            }
        });
        window.on_preview({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = (|| {
                    if form.borrow().generation != session.document_generation() {
                        return Err("Map changed; reopen Create linked path…".into());
                    }
                    let request = request(&window)?;
                    if window.get_existing() {
                        let nodes = window
                            .get_points()
                            .iter()
                            .zip(request.points)
                            .map(|(row, origin)| {
                                let id = if row.id.is_empty() {
                                    None
                                } else {
                                    let map_core::ObjectId::Entity(id) =
                                        crate::identity::parse_id(&row.id)?
                                    else {
                                        return Err(
                                            "Existing path row has no entity ID; load it again"
                                                .into(),
                                        );
                                    };
                                    Some(id)
                                };
                                Ok(EditNode { id, origin })
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        let snapshot = form
                            .borrow()
                            .existing_snapshot
                            .clone()
                            .ok_or("Load the existing path again")?;
                        let seed = form
                            .borrow()
                            .existing_seed
                            .ok_or("Load the existing path again")?;
                        session.preview_linked_path_changes(
                            Actor::User,
                            ChangeRequest {
                                seed,
                                nodes,
                                prefix: request.prefix,
                            },
                            &snapshot,
                        )
                    } else {
                        let snapshot = session.linked_path_stamp();
                        session.preview_linked_path(Actor::User, request, &snapshot)
                    }
                })();
                if let Err(error) = result.and_then(|state| adopt_preview(&window, &state, &form)) {
                    window.set_message(error.into());
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_review({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = (|| {
                    if form.borrow().generation != session.document_generation() {
                        return Err("Map changed; reopen Create linked path…".into());
                    }
                    let state = session.linked_path();
                    let token = state["preview"]["token"]
                        .as_u64()
                        .ok_or("No linked path preview to review")?;
                    session.review_linked_path(Actor::User, token)
                })();
                if let Err(error) = result.and_then(|state| adopt_preview(&window, &state, &form)) {
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
                refresh(&window, &session, &form);
                if !window.get_ready() {
                    window.set_message(
                        "Preview and review the current settings before Apply.".into(),
                    );
                    return;
                }
                let token = form.borrow().owned_token.unwrap();
                match session.apply_linked_path(Actor::User, token) {
                    Ok(_) => {
                        form.borrow_mut().owned_token = None;
                        form.borrow_mut().dirty = true;
                        let _ = window.hide();
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

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        if !self.window.window().is_visible() {
            *self.form.borrow_mut() = Form {
                generation: session.document_generation(),
                dirty: true,
                ..Default::default()
            };
            let state = session.linked_path();
            self.window
                .set_message("Names use prefix_001, with unused names chosen at Preview.".into());
            if state["preview"].is_null() {
                let origin = session.placement_origin();
                let step = if origin.x <= crate::spatial::MAX_EXTENT - POINT_STEP {
                    POINT_STEP
                } else {
                    -POINT_STEP
                };
                self.window.set_class_index(0);
                self.window.set_existing(false);
                self.window.set_prefix("path".into());
                self.window.set_closed(false);
                self.window
                    .set_removed_nodes(ModelRc::new(VecModel::default()));
                set_rows(
                    &self.window,
                    vec![point_row(origin, 0), point_row(origin + DVec3::X * step, 1)],
                );
            } else if let Err(error) = load_preview(&self.window, &state["preview"]) {
                self.window.set_message(error.into());
            }
        }
        self.refresh(session);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }
}

#[cfg(test)]
mod tests;
