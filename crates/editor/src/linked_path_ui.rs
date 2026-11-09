use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::DVec3;
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    LinkedPathPoint, LinkedPathWindow,
    runtime::{DocumentAccess, DocumentContinuation},
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
    source_class: String,
    source_closed: bool,
    source_ids: Vec<String>,
}

pub struct Controller {
    pub window: LinkedPathWindow,
    form: Rc<RefCell<Form>>,
    viewport: Rc<Cell<(u32, u32)>>,
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

fn train_description(train: &Value) -> String {
    let name = train["targetname"].as_str().unwrap_or_default();
    let target = train["target"].as_str().unwrap_or_default();
    format!(
        "{} · {} · {} → {}",
        train["classname"].as_str().unwrap_or("train"),
        train["id"].as_str().unwrap_or_default(),
        if name.is_empty() { "unnamed" } else { name },
        if target.is_empty() {
            "no path target"
        } else {
            target
        },
    )
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
    window.set_existing(
        existing
            || matches!(
                preview["operation"].as_str(),
                Some("train" | "duplicate" | "topology")
            ),
    );
    window.set_prefix(prefix.into());
    window.set_closed(closed);
    window.set_points(ModelRc::new(VecModel::from(rows)));
    window.set_removed_nodes(ModelRc::new(VecModel::from(removed)));
    window.set_train_summary(if preview["train"].is_object() {
        train_description(&preview["train"]).into()
    } else {
        preview["trains"]
            .as_array()
            .map_or_else(String::new, |trains| {
                trains
                    .iter()
                    .map(train_description)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .into()
    });
    if preview.get("operation").is_none() {
        let associated = preview["trains"].as_array().and_then(|trains| {
            if let [train] = trains.as_slice() {
                train["id"].as_str()
            } else {
                None
            }
        });
        window.set_train_id(associated.unwrap_or_default().into());
    }
    if let Some(operation) = preview["operation"].as_str() {
        window.set_operation_index(match operation {
            "train" => 1,
            "duplicate" => 2,
            _ => 0,
        });
        let options = &preview["operation_request"];
        if let Some(train) = options["train"].as_str() {
            entity_id(train)?;
            window.set_train_id(train.into());
        }
        if operation == "train" {
            let speed = options["speed"]
                .as_f64()
                .filter(|speed| speed.is_finite())
                .ok_or(invalid)?;
            window.set_train_speed(source_formats::vmf::format_number(speed).into());
            window.set_train_start_on(options["start_on"].as_bool().ok_or(invalid)?);
        } else if operation == "duplicate" {
            window.set_duplicate_train(!options["train"].is_null());
            window
                .set_duplicate_suffix(options["naming"]["suffix"].as_str().ok_or(invalid)?.into());
            window.set_external_index(match options["external"].as_str() {
                Some("reject") => 0,
                Some("keep") => 1,
                _ => return Err(invalid.into()),
            });
            let offset = options["offset"]
                .as_array()
                .filter(|values| values.len() == 3)
                .ok_or(invalid)?;
            let offset = offset
                .iter()
                .map(|value| {
                    value
                        .as_f64()
                        .filter(|value| value.is_finite())
                        .ok_or(invalid)
                })
                .collect::<Result<Vec<_>, _>>()?;
            window.set_offset_x(source_formats::vmf::format_number(offset[0]).into());
            window.set_offset_y(source_formats::vmf::format_number(offset[1]).into());
            window.set_offset_z(source_formats::vmf::format_number(offset[2]).into());
        }
    }
    Ok(())
}

fn entity_id(value: &str) -> Result<u32, String> {
    match crate::identity::parse_id(value.trim())? {
        map_core::ObjectId::Entity(id) => Ok(id),
        _ => Err("Enter an entity ID such as entity:123".into()),
    }
}

fn source_context(form: &RefCell<Form>) -> Result<(u32, String), String> {
    let form = form.borrow();
    Ok((
        form.existing_seed.ok_or("Load an existing path first")?,
        form.existing_snapshot
            .clone()
            .ok_or("Load the existing path again")?,
    ))
}

fn preview_seed(preview: &Value) -> Result<Option<u32>, String> {
    let Some(seed) = preview["seed"].as_str() else {
        return if preview["existing"] == true {
            Err("Existing path preview has no source seed".into())
        } else {
            Ok(None)
        };
    };
    let map_core::ObjectId::Entity(id) = crate::identity::parse_id(seed)? else {
        return Err("Existing path seed requires an entity ID".into());
    };
    Ok(Some(id))
}

fn load_source(form: &RefCell<Form>, source: &Value) {
    let mut form = form.borrow_mut();
    form.source_class = source["classname"].as_str().unwrap_or_default().into();
    form.source_closed = source["closed"] == true;
    form.source_ids = source["nodes"].as_array().map_or_else(Vec::new, |nodes| {
        nodes
            .iter()
            .filter_map(|node| node["id"].as_str().map(str::to_owned))
            .collect()
    });
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
    if let Some(seed) = preview_seed(preview)? {
        form.borrow_mut().existing_seed = Some(seed);
    }
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
    let picking = !preview["picking"].is_null();
    let handling = !preview["handles"].is_null();
    let busy = picking || handling;
    window.set_picking(picking);
    window.set_handling(handling);
    window.set_editable(editable && !busy);
    window.set_can_load(
        editable && !busy && session.selected_ids().len() == 1 && (preview.is_null() || user_owned),
    );
    window.set_can_preview(editable && !busy && (preview.is_null() || user_owned));
    window.set_can_review(editable && current && (!reviewed || form.dirty));
    window.set_ready(editable && current && reviewed && !form.dirty && !busy);
    window.set_preview_status(
        if !same_map {
            "Map changed. Reopen Linked paths… to start in this map.".into()
        } else if preview.is_null() {
            "Preview the path to check its names, links and position in the viewport.".into()
        } else if !current {
            "The preview is stale. Preview again to check the current map.".into()
        } else if !reviewed {
            format!(
                "Preview by {}. Review preview to load its points and take ownership.",
                preview["owner"].as_str().unwrap_or("another editor")
            )
        } else if handling {
            "Drag a move handle to reposition this preview node. Escape returns to the path. Apply is still required.".into()
        } else if form.dirty {
            "Settings changed. Preview again before applying.".into()
        } else {
            format!(
                "{} linked {} entities · {} · {} · Apply commits one Undo step.",
                preview["points"].as_array().map_or(0, Vec::len),
                preview["classname"].as_str().unwrap_or("path"),
                if preview["closed"] == true {
                    "closed loop"
                } else {
                    "open path"
                },
                match preview["operation"].as_str() {
                    Some("train") => "train setup",
                    Some("duplicate") => "duplicate path",
                    Some("topology") => "convert path",
                    _ if preview["existing"] == true => "edit path",
                    _ => "create path",
                },
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

fn cancel_position_pick(
    window: &LinkedPathWindow,
    session: &mut Session,
    form: &RefCell<Form>,
) -> Result<(), String> {
    let token = form
        .borrow()
        .owned_token
        .ok_or("Review the path before cancelling position picking")?;
    let state = session.cancel_linked_path_pick(Actor::User, token)?;
    form.borrow_mut().owned_token = state["preview"]["token"].as_u64();
    load_preview(window, &state["preview"])?;
    refresh(window, session, form);
    window.set_message("Position picking cancelled; the preview points are unchanged.".into());
    let _ = window.show();
    Ok(())
}

fn finish_handles(
    window: &LinkedPathWindow,
    session: &mut Session,
    form: &RefCell<Form>,
) -> Result<(), String> {
    let state = session.linked_path();
    let token = state["preview"]["token"]
        .as_u64()
        .ok_or("Review the linked path first")?;
    let state = session.cancel_linked_path_handles(Actor::User, token)?;
    let adopted = adopt_preview(window, &state, form);
    refresh(window, session, form);
    let _ = window.show();
    adopted?;
    window.set_message(
        "Moved positions remain in the preview. Review the coordinates, then Apply or Cancel."
            .into(),
    );
    Ok(())
}

fn with_session<R>(
    window: &slint::Weak<LinkedPathWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&LinkedPathWindow, &mut Session) -> R,
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

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = LinkedPathWindow::new()?;
        let access = access.downgrade();
        let viewport = Rc::new(Cell::new((1, 1)));
        window
            .window()
            .set_size(slint::LogicalSize::new(980.0, 820.0));
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_use_selected_train({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let selected = session.selected_ids();
                    let [map_core::ObjectId::Entity(id)] = selected.as_slice() else {
                        window.set_message(
                            "Select one func_train or func_tracktrain, then Use selected train."
                                .into(),
                        );
                        return;
                    };
                    window.set_train_id(
                        crate::identity::id_string(map_core::ObjectId::Entity(*id)).into(),
                    );
                    if window.get_operation_index() != 0 {
                        edited(window, session, &form);
                    }
                });
            }
        });
        window.on_move_handles({
            let (window, access, form, viewport) = (
                window.as_weak(),
                access.clone(),
                form.clone(),
                viewport.clone(),
            );
            move |row| {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        refresh(window, session, &form);
                        if !window.get_ready() || row < 0 {
                            return Err(
                                "Preview and review the current values before moving a node".into(),
                            );
                        }
                        let token = form.borrow().owned_token.ok_or("Review the path first")?;
                        let stamp = session.linked_path_pick_stamp();
                        let state = session.begin_linked_path_handles(
                            Actor::User,
                            token,
                            row as usize,
                            session.camera(),
                            viewport.get(),
                            &stamp,
                        )?;
                        adopt_preview(window, &state, &form)?;
                        refresh(window, session, &form);
                        let _ = window.hide();
                        Ok::<_, String>(())
                    })();
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_finish_handles({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = finish_handles(window, session, &form);
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_pick_position({
            let (window, access, form, viewport) = (
                window.as_weak(),
                access.clone(),
                form.clone(),
                viewport.clone(),
            );
            move |row| {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        refresh(window, session, &form);
                        if !window.get_ready() || row < 0 {
                            return Err("Preview and review the current values before picking a position".into());
                        }
                        let token = form.borrow().owned_token.ok_or("Review the path first")?;
                        let camera = session.camera();
                        let stamp = session.linked_path_pick_stamp();
                        let state = session.begin_linked_path_pick(
                            Actor::User,
                            token,
                            row as usize,
                            camera,
                            viewport.get(),
                            &stamp,
                        )?;
                        adopt_preview(window, &state, &form)?;
                        refresh(window, session, &form);
                        window.set_message("Click a visible surface to update this preview row. Escape cancels picking. Apply is still required.".into());
                        let _ = window.hide();
                        Ok::<_, String>(())
                    })();
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_cancel_pick({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    if let Err(error) = cancel_position_pick(window, session, &form) {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_load_selected({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        if form.borrow().generation != session.document_generation() {
                            return Err("Map changed; reopen Linked paths…".into());
                        }
                        let selected = session.selected_ids();
                        let [map_core::ObjectId::Entity(id)] = selected.as_slice() else {
                            return Err("Select one existing path_corner or path_track node".into());
                        };
                        let state = session.inspect_existing_linked_path(*id)?;
                        load_preview(window, &state)?;
                        form.borrow_mut().existing_seed = preview_seed(&state)?;
                        form.borrow_mut().existing_snapshot = state["snapshot"].as_str().map(str::to_owned);
                        load_source(&form, &state);
                        window.set_operation_index(0);
                        form.borrow_mut().dirty = true;
                        window.set_message("Preview changes to review their links. Class or loop conversion keeps the original nodes; finish membership edits in a separate Apply.".into());
                        Ok::<_, String>(())
                    })();
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_new_path({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    window.set_existing(false);
                    window.set_operation_index(0);
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
                    set_rows(window, rows);
                    form.borrow_mut().existing_snapshot = None;
                    form.borrow_mut().existing_seed = None;
                    window.set_removed_nodes(ModelRc::new(VecModel::default()));
                    edited(window, session, &form);
                });
            }
        });
        window.on_edited({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    edited(window, session, &form);
                });
            }
        });
        window.on_add_point({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut rows = window.get_points().iter().collect::<Vec<_>>();
                    if rows.len() >= MAX_POINTS {
                        return;
                    }
                    let point =
                        match request(window).and_then(|request| {
                            request.points.last().copied().ok_or_else(|| {
                                "No path point to extend; reopen Linked paths…".into()
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
                    set_rows(window, rows);
                    edited(window, session, &form);
                });
            }
        });
        window.on_remove_point({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move |index| {
                with_session(&window, &access, |window, session| {
                    let mut rows = window.get_points().iter().collect::<Vec<_>>();
                    if rows.len() > MIN_POINTS && index >= 0 && (index as usize) < rows.len() {
                        rows.remove(index as usize);
                        set_rows(window, rows);
                        edited(window, session, &form);
                    }
                });
            }
        });
        window.on_move_point({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move |index, direction| {
                with_session(&window, &access, |window, session| {
                    let mut rows = window.get_points().iter().collect::<Vec<_>>();
                    let target = index + direction;
                    if direction.abs() == 1
                        && index >= 0
                        && target >= 0
                        && (index as usize) < rows.len()
                        && (target as usize) < rows.len()
                    {
                        rows.swap(index as usize, target as usize);
                        set_rows(window, rows);
                        edited(window, session, &form);
                    }
                });
            }
        });
        window.on_preview({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        if form.borrow().generation != session.document_generation() {
                            return Err("Map changed; reopen Linked paths…".into());
                        }
                        let request = request(window)?;
                        if window.get_existing() {
                            let (seed, snapshot) = source_context(&form)?;
                            match window.get_operation_index() {
                                1 => return session.preview_linked_path_train(Actor::User, seed, entity_id(&window.get_train_id())?, coordinate(&window.get_train_speed())?, window.get_train_start_on(), &snapshot),
                                2 => {
                                    let train = window.get_duplicate_train().then(|| entity_id(&window.get_train_id())).transpose()?;
                                    let policies = map_core::repeat::Policies {
                                        naming: map_core::repeat::Naming { suffix: window.get_duplicate_suffix().into(), ..Default::default() },
                                        internal: map_core::repeat::InternalReferences::Remap,
                                        external: match window.get_external_index() { 0 => map_core::repeat::ExternalReferences::Reject, 1 => map_core::repeat::ExternalReferences::Keep, _ => return Err("Choose an external reference policy".into()) },
                                    };
                                    return session.preview_linked_path_duplicate(Actor::User, seed, train, DVec3::new(coordinate(&window.get_offset_x())?, coordinate(&window.get_offset_y())?, coordinate(&window.get_offset_z())?), policies, &snapshot);
                                }
                                0 => {}
                                _ => return Err("Choose a linked path operation".into()),
                            }
                            let convert = {
                                let form = form.borrow();
                                request.classname != form.source_class || request.closed != form.source_closed
                            };
                            if convert {
                                let ids = window.get_points().iter().map(|row| row.id.to_string()).collect::<Vec<_>>();
                                if ids != form.borrow().source_ids { return Err("Apply node additions, removals or reordering before converting class or loop topology".into()); }
                                return session.preview_linked_path_topology(Actor::User, seed, request.classname, request.closed, Some(request.points), &snapshot);
                            }
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
                    if let Err(error) =
                        result.and_then(|state| adopt_preview(window, &state, &form))
                    {
                        window.set_message(error.into());
                    }
                    refresh(window, session, &form);
                });
            }
        });
        window.on_review({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        if form.borrow().generation != session.document_generation() {
                            return Err("Map changed; reopen Linked paths…".into());
                        }
                        let state = session.linked_path();
                        let token = state["preview"]["token"]
                            .as_u64()
                            .ok_or("No linked path preview to review")?;
                        session.review_linked_path(Actor::User, token)
                    })();
                    if let Err(error) =
                        result.and_then(|state| adopt_preview(window, &state, &form))
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
                    refresh(window, session, &form);
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
                    refresh(window, session, &form);
                });
            }
        });
        window.on_cancel({
            let (window, access, form) = (window.as_weak(), access.clone(), form.clone());
            move || {
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
            .ok_or("Review the linked path before picking a position")?;
        let state = session.pick_linked_path_pixel(
            Actor::User,
            token,
            crate::session::Pointer {
                position: position.floor() + glam::DVec2::splat(0.5),
                size,
            },
        )?;
        adopt_preview(&self.window, &state, &self.form)?;
        refresh(&self.window, session, &self.form);
        self.window.set_message(
            "Position picked into the preview. Review the coordinates; Apply is still required."
                .into(),
        );
        let _ = self.window.show();
        Ok(())
    }

    pub fn cancel_pick(&self, session: &mut Session) -> Result<(), String> {
        cancel_position_pick(&self.window, session, &self.form)
    }

    pub fn handle_pointer(
        &self,
        session: &mut Session,
        pointer: crate::session::Pointer,
        down: bool,
    ) -> Result<(), String> {
        let state = session.linked_path();
        let token = state["preview"]["token"]
            .as_u64()
            .ok_or("Review the linked path first")?;
        let state = if down {
            let handle = session
                .pick_linked_path_handle(pointer)
                .ok_or("Drag a visible move handle")?;
            session.begin_linked_path_drag(Actor::User, token, handle, Some(pointer))?
        } else if state["preview"]["handles"]["dragging"] == true {
            session.update_linked_path_drag(Actor::User, token, Some(pointer), None)?;
            let token = session.linked_path()["preview"]["token"]
                .as_u64()
                .ok_or("Linked path preview was replaced")?;
            session.finish_linked_path_drag(Actor::User, token)?
        } else {
            return Ok(());
        };
        adopt_preview(&self.window, &state, &self.form)?;
        refresh(&self.window, session, &self.form);
        if !down {
            self.window.set_message(
                "Position moved into the preview. Review the coordinates, then Apply or Cancel."
                    .into(),
            );
            let _ = self.window.show();
        }
        Ok(())
    }

    pub fn move_handle(
        &self,
        session: &mut Session,
        pointer: crate::session::Pointer,
    ) -> Result<(), String> {
        let state = session.linked_path();
        if state["preview"]["handles"]["dragging"] != true {
            return Ok(());
        }
        let token = state["preview"]["token"]
            .as_u64()
            .ok_or("Review the linked path first")?;
        let state = session.update_linked_path_drag(Actor::User, token, Some(pointer), None)?;
        adopt_preview(&self.window, &state, &self.form)?;
        refresh(&self.window, session, &self.form);
        Ok(())
    }

    pub fn cancel_handles(&self, session: &mut Session) -> Result<(), String> {
        let state = session.linked_path();
        if state["preview"]["handles"]["dragging"] == true {
            let token = state["preview"]["token"]
                .as_u64()
                .ok_or("Review the linked path first")?;
            session.cancel_linked_path_drag(Actor::User, token)?;
        }
        finish_handles(&self.window, session, &self.form)
    }

    pub fn discard_handles(&self, session: &mut Session) -> Result<(), String> {
        let state = session.linked_path();
        let token = state["preview"]["token"]
            .as_u64()
            .ok_or("Review the linked path first")?;
        let state = if state["preview"]["handles"]["dragging"] == true {
            session.cancel_linked_path_drag(Actor::User, token)?
        } else {
            state
        };
        let token = state["preview"]["token"]
            .as_u64()
            .ok_or("Linked path preview was replaced")?;
        let state = session.cancel_linked_path_handles(Actor::User, token)?;
        adopt_preview(&self.window, &state, &self.form)?;
        refresh(&self.window, session, &self.form);
        Ok(())
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
                self.window.set_operation_index(0);
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
            } else if let Ok(Some(seed)) = preview_seed(&state["preview"])
                && let Ok(source) = session.inspect_existing_linked_path(seed)
            {
                load_source(&self.form, &source);
            }
        }
        self.refresh(session);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }
}
