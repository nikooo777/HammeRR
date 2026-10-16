use std::{cell::RefCell, rc::Rc};

use glam::DVec3;
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    LinkedPathPoint, LinkedPathWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session, linked_path::Request},
};

#[derive(Default)]
struct Form {
    generation: u64,
    owned_token: Option<u64>,
    dirty: bool,
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
        classname: if window.get_class_index() == 1 {
            "path_track"
        } else {
            "path_corner"
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

fn load_preview(window: &LinkedPathWindow, preview: &Value) {
    window.set_class_index(i32::from(preview["classname"] == "path_track"));
    window.set_prefix(preview["prefix"].as_str().unwrap_or("path").into());
    window.set_closed(preview["closed"] == true);
    let points = preview["points"].as_array().cloned().unwrap_or_default();
    let rows = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let mut row = point_row(
                DVec3::new(
                    point[0].as_f64().unwrap_or_default(),
                    point[1].as_f64().unwrap_or_default(),
                    point[2].as_f64().unwrap_or_default(),
                ),
                index,
            );
            let node = &preview["nodes"][index];
            row.link = format!(
                "{} → {}",
                node["targetname"].as_str().unwrap_or(""),
                node["target"].as_str().unwrap_or("end")
            )
            .into();
            row
        })
        .collect::<Vec<_>>();
    window.set_points(ModelRc::new(VecModel::from(rows)));
}

fn refresh(window: &LinkedPathWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.linked_path();
    let preview = &state["preview"];
    let form = form.borrow();
    let same_map = form.generation == session.document_generation();
    let editable = same_map && state["editable"] != false;
    let token = preview["token"].as_u64();
    let user_owned = preview["owner"] == Actor::User.label();
    let current = !preview.is_null() && preview["valid"] == true;
    let reviewed = token.is_some() && token == form.owned_token && user_owned;
    window.set_editable(editable);
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
                "{} linked {} entities · {} · Apply creates the path in one Undo step.",
                preview["points"].as_array().map_or(0, Vec::len),
                preview["classname"].as_str().unwrap_or("path"),
                if preview["closed"] == true {
                    "closed loop"
                } else {
                    "open path"
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
        window.on_edited({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || edited(&window.unwrap(), &lock(&shared), &form)
        });
        window.on_add_point({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut rows = window.get_points().iter().collect::<Vec<_>>();
                if rows.len() >= 64 {
                    return;
                }
                let point = match request(&window) {
                    Ok(request) => request.points.last().copied().unwrap_or_default(),
                    Err(error) => {
                        window.set_message(error.into());
                        return;
                    }
                };
                let step = if point.x <= 65408.0 { 128.0 } else { -128.0 };
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
                if rows.len() > 2 && index >= 0 && (index as usize) < rows.len() {
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
                    let snapshot = session.linked_path_stamp();
                    session.preview_linked_path(Actor::User, request, &snapshot)
                })();
                match result {
                    Ok(state) => {
                        let preview = &state["preview"];
                        form.borrow_mut().owned_token = preview["token"].as_u64();
                        form.borrow_mut().dirty = false;
                        load_preview(&window, preview);
                        window.set_message(
                            "Review the ordered points and links before Apply.".into(),
                        );
                    }
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
                match result {
                    Ok(state) => {
                        let preview = &state["preview"];
                        form.borrow_mut().owned_token = preview["token"].as_u64();
                        form.borrow_mut().dirty = false;
                        load_preview(&window, preview);
                        window.set_message(
                            "Review the ordered points and links before Apply.".into(),
                        );
                    }
                    Err(error) => window.set_message(error.into()),
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
            if state["preview"].is_null() {
                let origin = session.placement_origin();
                let step = if origin.x <= 65408.0 { 128.0 } else { -128.0 };
                self.window.set_class_index(0);
                self.window.set_prefix("path".into());
                self.window.set_closed(false);
                set_rows(
                    &self.window,
                    vec![point_row(origin, 0), point_row(origin + DVec3::X * step, 1)],
                );
            } else {
                load_preview(&self.window, &state["preview"]);
            }
            self.window
                .set_message("Names use prefix_001, with unused names chosen at Preview.".into());
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
