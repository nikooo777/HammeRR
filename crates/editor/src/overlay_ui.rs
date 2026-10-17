use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::{DVec2, DVec3};
use map_core::{overlay::Options, surface::FaceId};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    OverlayTargetRow, OverlayWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Pointer, Session, overlay::Request},
    ui::preview_image,
};

#[derive(Default)]
struct Form {
    source: Option<FaceId>,
    token: Option<u64>,
    reviewed: Option<Options>,
    refit: bool,
    rebind: bool,
    corners: bool,
    reviewed_corners: Option<[[f64; 2]; 4]>,
    target: Option<u32>,
    choices: Vec<Value>,
    choices_snapshot: String,
    choices_offset: usize,
    snapshot: String,
    seen: String,
    serial: u64,
    material_generation: u64,
}

#[derive(Clone)]
pub struct MaterialTarget {
    snapshot: String,
    serial: u64,
}

pub struct Controller {
    pub window: OverlayWindow,
    form: Rc<RefCell<Form>>,
    viewport: Rc<Cell<(u32, u32)>>,
}

fn number(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or("Enter finite texture coordinates and fade distances".into())
}

fn fields(window: &OverlayWindow) -> Result<Options, String> {
    let targetname = window.get_targetname();
    Ok(Options {
        material: window.get_material().to_string(),
        targetname: (!targetname.trim().is_empty()).then(|| targetname.to_string()),
        render_order: window
            .get_render_order()
            .try_into()
            .map_err(|_| "Render order must be between 0 and 3")?,
        u: [number(&window.get_u_start())?, number(&window.get_u_end())?],
        v: [number(&window.get_v_start())?, number(&window.get_v_end())?],
        fade_min: number(&window.get_fade_min())?,
        fade_max: number(&window.get_fade_max())?,
    })
}

fn corner_fields(window: &OverlayWindow) -> Result<[[f64; 2]; 4], String> {
    let fields = [
        [window.get_corner_0_u(), window.get_corner_0_v()],
        [window.get_corner_1_u(), window.get_corner_1_v()],
        [window.get_corner_2_u(), window.get_corner_2_v()],
        [window.get_corner_3_u(), window.get_corner_3_v()],
    ];
    let mut corners = [[0.0; 2]; 4];
    for (corner, fields) in corners.iter_mut().zip(fields) {
        for (value, field) in corner.iter_mut().zip(fields) {
            *value = field
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or("Enter finite local U/V positions for all four corners")?;
        }
    }
    Ok(corners)
}

fn load_corners(window: &OverlayWindow, value: &Value) {
    let text = |index: usize, axis: usize| {
        value["local_corners"][index][axis]
            .as_f64()
            .unwrap_or_default()
            .to_string()
            .into()
    };
    window.set_corner_0_u(text(0, 0));
    window.set_corner_0_v(text(0, 1));
    window.set_corner_1_u(text(1, 0));
    window.set_corner_1_v(text(1, 1));
    window.set_corner_2_u(text(2, 0));
    window.set_corner_2_v(text(2, 1));
    window.set_corner_3_u(text(3, 0));
    window.set_corner_3_v(text(3, 1));
    let vector = |key: &str| {
        format!(
            "({:.3}, {:.3}, {:.3})",
            value[key][0].as_f64().unwrap_or_default(),
            value[key][1].as_f64().unwrap_or_default(),
            value[key][2].as_f64().unwrap_or_default()
        )
    };
    window.set_corner_frame(
        format!(
            "World origin {} · U {} · V {}",
            vector("basis_origin"),
            vector("basis_u"),
            vector("basis_v")
        )
        .into(),
    );
}

fn corner_form_matches(window: &OverlayWindow, form: &Form) -> bool {
    !form.corners
        || (form.reviewed_corners.is_some() && corner_fields(window).ok() == form.reviewed_corners)
}

fn load(window: &OverlayWindow, options: &Value) {
    let text = |value: &Value, fallback: f64| {
        (value.as_f64().unwrap_or(fallback) as f32)
            .to_string()
            .into()
    };
    window.set_material(options["material"].as_str().unwrap_or_default().into());
    window.set_targetname(options["targetname"].as_str().unwrap_or_default().into());
    window.set_render_order(options["render_order"].as_i64().unwrap_or_default() as i32);
    window.set_u_start(text(&options["u"][0], 0.0));
    window.set_u_end(text(&options["u"][1], 1.0));
    window.set_v_start(text(&options["v"][0], 0.0));
    window.set_v_end(text(&options["v"][1], 1.0));
    window.set_fade_min(text(&options["fade_min"], -1.0));
    window.set_fade_max(text(&options["fade_max"], 0.0));
}

fn source(value: &Value) -> Option<FaceId> {
    Some(FaceId {
        solid: value["solid"].as_u64()?.try_into().ok()?,
        side: value["side"].as_u64()?.try_into().ok()?,
    })
}

fn outline(value: &Value) -> Option<String> {
    let points = value
        .as_array()?
        .iter()
        .map(|point| {
            Some(DVec3::new(
                point[0].as_f64()?,
                point[1].as_f64()?,
                point[2].as_f64()?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    if points.len() != 4 || points.iter().any(|point| !point.is_finite()) {
        return None;
    }
    let right = (points[1] - points[0]).try_normalize()?;
    let normal = right.cross(points[3] - points[0]).try_normalize()?;
    let up = normal.cross(right);
    let projected = points
        .iter()
        .map(|point| DVec2::new(point.dot(right), point.dot(up)))
        .collect::<Vec<_>>();
    let (mins, maxs) = projected
        .iter()
        .fold((DVec2::MAX, DVec2::MIN), |(mins, maxs), &point| {
            (mins.min(point), maxs.max(point))
        });
    let extent = maxs - mins;
    if extent.min_element() <= 0.0 {
        return None;
    }
    let scale = (288.0 / extent.x).min(148.0 / extent.y);
    let center = (mins + maxs) * 0.5;
    let mut path = String::new();
    for (index, point) in projected.iter().enumerate() {
        let point = (*point - center) * scale;
        path.push_str(&format!(
            "{} {} {} ",
            if index == 0 { "M" } else { "L" },
            160.0 + point.x,
            90.0 - point.y
        ));
    }
    path.push('Z');
    Some(path)
}

fn refresh_material(window: &OverlayWindow, session: &Session) {
    if window.get_material().trim().is_empty() {
        window.set_material_image(slint::Image::default());
        window.set_material_detail("Choose an overlay material".into());
        return;
    }
    match session.material_preview(&window.get_material()) {
        Ok(preview) => {
            window.set_material_image(preview_image(&preview));
            window.set_material_detail(if preview.missing || preview.image.is_none() {
                "Texture preview unavailable".into()
            } else {
                format!("{} × {}", preview.width, preview.height).into()
            });
        }
        Err(error) => {
            window.set_material_image(slint::Image::default());
            window.set_material_detail(error.into());
        }
    }
}

const OVERLAY_PAGE_SIZE: usize = 8;

fn refresh_choices(window: &OverlayWindow, session: &Session, form: &RefCell<Form>) {
    let offset = form.borrow().choices_offset;
    match session.browse_overlay_targets(&window.get_overlay_search(), offset, OVERLAY_PAGE_SIZE) {
        Ok(report) => {
            let entries = report["entries"].as_array().unwrap().clone();
            let total = report["total"].as_u64().unwrap() as usize;
            if offset > 0 && offset >= total {
                form.borrow_mut().choices_offset =
                    total.saturating_sub(1) / OVERLAY_PAGE_SIZE * OVERLAY_PAGE_SIZE;
                refresh_choices(window, session, form);
                return;
            }
            window.set_overlay_targets(ModelRc::new(VecModel::from(
                entries
                    .iter()
                    .map(|entry| {
                        let name = entry["name"]
                            .as_str()
                            .filter(|name| !name.trim().is_empty())
                            .unwrap_or("Unnamed overlay");
                        let source = &entry["source"];
                        OverlayTargetRow {
                            entity: entry["entity"].as_str().unwrap().into(),
                            label: format!("{} · {}", name, entry["entity"].as_str().unwrap())
                                .into(),
                            detail: if entry["eligible"] == true {
                                format!(
                                    "{} · brush {} / face {}",
                                    entry["material"].as_str().unwrap(),
                                    source["solid"],
                                    source["side"]
                                )
                            } else {
                                format!(
                                    "Unavailable: {}",
                                    entry["error"].as_str().unwrap_or("Unsupported binding")
                                )
                            }
                            .into(),
                            eligible: entry["eligible"] == true,
                        }
                    })
                    .collect::<Vec<_>>(),
            )));
            window.set_overlay_previous(offset > 0);
            window.set_overlay_next(offset.saturating_add(OVERLAY_PAGE_SIZE) < total);
            window.set_overlay_page(if total == 0 {
                "No matching overlays".into()
            } else {
                format!(
                    "{}–{} of {}",
                    offset.saturating_add(1),
                    offset.saturating_add(entries.len()),
                    total
                )
                .into()
            });
            let mut form = form.borrow_mut();
            form.choices = entries;
            form.choices_snapshot = report["snapshot"].as_str().unwrap().into();
        }
        Err(error) => {
            window.set_overlay_targets(ModelRc::default());
            window.set_overlay_previous(false);
            window.set_overlay_next(false);
            window.set_overlay_page(error.into());
            let mut form = form.borrow_mut();
            form.choices.clear();
            form.choices_snapshot.clear();
        }
    }
}

fn choose_overlay(window: &OverlayWindow, session: &Session, form: &RefCell<Form>, entity: &str) {
    if form.borrow().choices_snapshot != session.overlay_stamp() {
        refresh_choices(window, session, form);
        window.set_message("Overlay choices changed. Choose from the refreshed list.".into());
        return;
    }
    let choice = form
        .borrow()
        .choices
        .iter()
        .find(|entry| entry["entity"] == entity && entry["eligible"] == true)
        .cloned();
    let Some(choice) = choice else {
        return;
    };
    load(window, &choice["options"]);
    let mut state = form.borrow_mut();
    state.target = choice["id"].as_u64().and_then(|id| u32::try_from(id).ok());
    state.refit = true;
    state.rebind = true;
    state.corners = false;
    state.reviewed_corners = None;
    state.reviewed = None;
    state.serial += 1;
    drop(state);
    window.set_rebind_target(
        format!(
            "Rebind {} · {} to the destination face above",
            choice["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Unnamed overlay"),
            entity
        )
        .into(),
    );
    window.set_message(
        "Preview rebind checks the destination and preserves existing settings.".into(),
    );
    refresh_material(window, session);
    refresh(window, session, form);
}

fn preview_rebind(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let request = {
        let form = form.borrow();
        form.target
            .zip(form.source)
            .map(|(id, source)| (id, source, form.snapshot.clone()))
    };
    let result = request
        .ok_or("Select a destination face and choose an existing overlay".into())
        .and_then(|(id, source, snapshot)| {
            session.preview_overlay_rebind(Actor::User, id, source, &snapshot)
        });
    match result {
        Ok(state) => {
            adopt(window, &state, form);
            window.set_message("Review the destination outline, then Apply rebind. The existing overlay keeps its ID and settings.".into());
        }
        Err(error) => window.set_message(error.into()),
    }
    refresh_material(window, session);
    refresh(window, session, form);
}

fn refresh(window: &OverlayWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.get_overlay_state();
    let draft = &state["draft"];
    let snapshot = state["snapshot"].as_str().unwrap_or_default();
    if form.borrow().choices_snapshot != snapshot {
        refresh_choices(window, session, form);
    }
    let mut form = form.borrow_mut();
    let context_refreshed = form.seen != snapshot;
    form.seen = snapshot.into();
    window.set_can_review(draft["valid"] == true);
    window.set_refit_mode(form.refit);
    window.set_rebind_mode(form.rebind);
    window.set_corners_mode(form.corners);
    window.set_can_edit_corners(!state["selected_corners"].is_null() && form.snapshot == snapshot);
    window.set_can_preview_corners(
        form.corners && form.target.is_some() && form.snapshot == snapshot,
    );
    window.set_can_rebind(
        form.rebind && form.target.is_some() && form.source.is_some() && form.snapshot == snapshot,
    );
    window.set_can_preview(!form.refit && form.source.is_some() && form.snapshot == snapshot);
    window.set_can_refit(!state["selected_overlay"].is_null() && form.snapshot == snapshot);
    let matches = draft["token"].as_u64() == form.token && draft["owner"] == "you";
    window.set_ready(
        matches
            && draft["valid"] == true
            && form.reviewed.is_some()
            && fields(window).ok().as_ref() == form.reviewed.as_ref()
            && corner_form_matches(window, &form)
            && draft["handles"].is_null(),
    );
    window.set_can_handle_corners(form.corners && window.get_ready());
    window.set_draft_label(if draft.is_null() {
        "No overlay draft · Fit / Preview leaves the map unchanged".into()
    } else if draft["valid"] != true {
        "The shared draft is stale · reopen Overlay to preview the current source".into()
    } else if matches && draft["operation"] == "corners" {
        format!(
            "Corner edit {} · Apply corners changes this existing projection",
            draft["entity"].as_str().unwrap_or("overlay")
        )
        .into()
    } else if matches && draft["operation"] == "rebind" {
        format!(
            "Rebind {} · Apply rebind moves this existing overlay to the destination",
            draft["entity"].as_str().unwrap_or("overlay")
        )
        .into()
    } else if matches && draft["operation"] == "refit" {
        format!(
            "Refit {} · Apply refit updates this existing entity",
            draft["entity"].as_str().unwrap_or("overlay")
        )
        .into()
    } else if matches {
        "Your fitted overlay draft · Apply overlay creates one entity".into()
    } else {
        "A shared draft is available · Review current draft to use it".into()
    });
    let display_source = if matches && draft["valid"] == true && form.reviewed.is_some() {
        &draft["source"]
    } else if form.rebind {
        &state["selected_source"]
    } else if !state["selected_overlay"].is_null() {
        &state["selected_overlay"]["source"]
    } else {
        &state["selected_source"]
    };
    window.set_source_label(if let Some(source) = source(display_source) {
        format!(
            "Source · brush {} · face {} · world / detail quad",
            source.solid, source.side
        )
        .into()
    } else {
        state[if session.face_mode() {
            "selection_error"
        } else {
            "refit_error"
        }]
        .as_str()
        .unwrap_or("Select exactly one visible world or detail quad face")
        .into()
    });
    let corners = if matches && draft["valid"] == true && form.reviewed.is_some() {
        &draft["corners"]
    } else if form.corners {
        &state["selected_corners"]["corners"]
    } else if form.rebind {
        &state["selected_source"]["corners"]
    } else if !state["selected_overlay"].is_null() {
        &state["selected_overlay"]["corners"]
    } else {
        &state["selected_source"]["corners"]
    };
    window.set_outline(outline(corners).unwrap_or_default().into());
    if form.snapshot != snapshot {
        window.set_message("Map, selection or shared draft changed. Review current draft, or reopen Overlay to use the selected face or overlay.".into());
    } else if context_refreshed && window.get_ready() {
        window.set_message("".into());
    }
}

fn edited(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let mut state = form.borrow_mut();
    state.reviewed = None;
    state.serial += 1;
    drop(state);
    window.set_ready(false);
    window.set_message("Settings changed. Preview settings before applying.".into());
    refresh_material(window, session);
}

fn adopt(window: &OverlayWindow, state: &Value, form: &RefCell<Form>) {
    let draft = &state["draft"];
    load(window, &draft["options"]);
    let mut form = form.borrow_mut();
    form.source = source(&draft["source"]);
    form.token = draft["token"].as_u64();
    form.reviewed = fields(window).ok();
    form.refit = matches!(
        draft["operation"].as_str(),
        Some("refit" | "rebind" | "corners")
    );
    form.corners = draft["operation"] == "corners";
    form.reviewed_corners = if form.corners {
        load_corners(window, draft);
        corner_fields(window).ok()
    } else {
        None
    };
    form.rebind = draft["operation"] == "rebind";
    form.target = draft["entity"]
        .as_str()
        .and_then(|value| value.strip_prefix("entity:"))
        .and_then(|value| value.parse().ok());
    form.snapshot = state["snapshot"].as_str().unwrap_or_default().into();
    form.serial += 1;
    if form.rebind {
        window.set_rebind_target(
            format!(
                "Existing {} · previous brush {} / face {} → destination brush {} / face {}",
                draft["entity"].as_str().unwrap_or("overlay"),
                draft["previous_source"]["solid"],
                draft["previous_source"]["side"],
                draft["source"]["solid"],
                draft["source"]["side"]
            )
            .into(),
        );
    }
}

fn preview(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let result = (|| {
        let (source, snapshot) = {
            let form = form.borrow();
            if form.refit {
                return Err("Existing-overlay updates preserve settings. Use Preview rebind, Apply or Cancel.".into());
            }
            (
                form.source
                    .ok_or("Select one world or detail quad face and reopen Overlay")?,
                form.snapshot.clone(),
            )
        };
        session.preview_overlay(
            Actor::User,
            Request {
                source,
                options: fields(window)?,
            },
            &snapshot,
        )
    })();
    match result {
        Ok(state) => {
            adopt(window, &state, form);
            window.set_message(
                "Review the cyan outline and material thumbnail, then Apply overlay.".into(),
            );
        }
        Err(error) => {
            form.borrow_mut().reviewed = None;
            window.set_message(error.into());
        }
    }
    refresh_material(window, session);
    refresh(window, session, form);
}

fn preview_refit(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let result = (|| {
        let state = session.get_overlay_state();
        let id = state["selected_overlay"]["id"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or("Select one eligible existing overlay and reopen Overlay")?;
        let snapshot = form.borrow().snapshot.clone();
        session.preview_overlay_refit(Actor::User, id, &snapshot)
    })();
    match result {
        Ok(state) => {
            adopt(window, &state, form);
            window.set_message("Review the cyan outline, then Apply refit. Existing settings and metadata are preserved.".into());
        }
        Err(error) => window.set_message(error.into()),
    }
    refresh_material(window, session);
    refresh(window, session, form);
}

fn edit_selected_corners(window: &OverlayWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.get_overlay_state();
    if form.borrow().snapshot != state["snapshot"].as_str().unwrap_or_default() {
        window.set_message(
            "Map, selection or draft changed. Reopen Overlay or review the current draft.".into(),
        );
        return;
    }
    let selected = &state["selected_corners"];
    let Some(id) = selected["id"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
    else {
        window.set_message(
            state["corners_error"]
                .as_str()
                .unwrap_or("Select one eligible overlay")
                .into(),
        );
        return;
    };
    load(window, &selected["options"]);
    load_corners(window, selected);
    let mut current = form.borrow_mut();
    current.corners = true;
    current.refit = true;
    current.rebind = false;
    current.target = Some(id);
    current.source = source(&selected["source"]);
    current.reviewed = None;
    current.reviewed_corners = None;
    current.serial += 1;
    drop(current);
    window.set_message("Edit local U/V world-unit positions in perimeter order, then Preview corners. The source binding and projection frame stay fixed.".into());
    refresh_material(window, session);
    refresh(window, session, form);
}

fn preview_corners(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let result = (|| {
        let current = form.borrow();
        if !current.corners {
            return Err("Choose Edit selected corners first".into());
        }
        let id = current
            .target
            .ok_or("Select an eligible existing overlay")?;
        session.preview_overlay_corners(Actor::User, id, corner_fields(window)?, &current.snapshot)
    })();
    match result {
        Ok(state) => {
            adopt(window, &state, form);
            window.set_message("Review the cyan projection outline, then Apply corners. The same overlay keeps its binding, frame and settings.".into());
        }
        Err(error) => {
            form.borrow_mut().reviewed_corners = None;
            window.set_message(error.into());
        }
    }
    refresh(window, session, form);
}

fn cancel_owned(session: &mut Session, form: &RefCell<Form>) {
    let token = form.borrow_mut().token.take();
    let state = session.get_overlay_state();
    if let Some(token) = token
        && state["draft"]["token"].as_u64() == Some(token)
        && state["draft"]["owner"] == "you"
    {
        let _ = session.cancel_overlay(Actor::User, token);
    }
    form.borrow_mut().reviewed = None;
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = OverlayWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let viewport = Rc::new(Cell::new((1024, 576)));
        window.on_edited({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || edited(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_preview({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || preview(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_search_overlays({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().choices_offset = 0;
                refresh_choices(&window.unwrap(), &lock(&shared), &form);
            }
        });
        window.on_overlay_page_change({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |direction| {
                let offset = form.borrow().choices_offset;
                form.borrow_mut().choices_offset = if direction < 0 {
                    offset.saturating_sub(OVERLAY_PAGE_SIZE)
                } else {
                    offset.saturating_add(OVERLAY_PAGE_SIZE)
                };
                refresh_choices(&window.unwrap(), &lock(&shared), &form);
            }
        });
        window.on_choose_overlay({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |entity| choose_overlay(&window.unwrap(), &lock(&shared), &form, &entity)
        });
        window.on_rebind({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || preview_rebind(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_edit_corners({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || edit_selected_corners(&window.unwrap(), &lock(&shared), &form)
        });
        window.on_preview_corners({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || preview_corners(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_handle_corners({
            let (window, shared, form, viewport) = (window.as_weak(), shared.clone(), form.clone(), viewport.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = {
                    let current = form.borrow();
                    if !current.corners
                        || current.reviewed.is_none()
                        || fields(&window).ok().as_ref() != current.reviewed.as_ref()
                        || !corner_form_matches(&window, &current)
                    {
                        Err("Preview corners before starting viewport handles".to_string())
                    } else {
                        current.token.ok_or_else(|| "Preview or Review the corner draft first".to_string()).and_then(|token| {
                            let camera = session.camera();
                            let stamp = session.overlay_corner_handle_stamp();
                            session.begin_overlay_corner_handles(Actor::User, token, camera, viewport.get(), &stamp)
                        })
                    }
                };
                match result {
                    Ok(state) => {
                        adopt(&window, &state, &form);
                        window.set_message("Drag one cyan corner in the viewport. Release to review; Escape or Cancel handles restores this draft.".into());
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form);
            }
        });
        window.on_corners_edited({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                let mut current = form.borrow_mut();
                current.reviewed_corners = None;
                current.serial += 1;
                let window = window.unwrap();
                window.set_ready(false);
                window.set_message("Corners changed. Preview corners before applying.".into());
            }
        });
        window.on_refit({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || preview_refit(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_fit({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                if form.borrow().refit {
                    return;
                }
                window.set_u_start("0".into());
                window.set_u_end("1".into());
                window.set_v_start("0".into());
                window.set_v_end("1".into());
                edited(&window, &mut lock(&shared), &form);
                preview(&window, &mut lock(&shared), &form);
            }
        });
        window.on_review({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let state = session.get_overlay_state();
                let Some(token) = state["draft"]["token"].as_u64() else {
                    window.set_message("No current overlay draft to review.".into());
                    return;
                };
                match session.review_overlay(Actor::User, token) {
                    Ok(state) => {
                        adopt(&window, &state, &form);
                        window.set_message(if state["draft"]["operation"] == "corners" { "Corner draft loaded. Apply corners updates the existing projection; Cancel discards this reviewed draft." } else if state["draft"]["operation"] == "rebind" { "Rebind draft loaded. Apply rebind updates the existing entity to the reviewed destination; Cancel discards this draft." } else if state["draft"]["operation"] == "refit" { "Refit draft loaded. Apply refit updates the existing entity; Cancel discards this reviewed draft." } else { "Current draft loaded. Apply overlay confirms it; Cancel discards this reviewed draft." }.into());
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh_material(&window, &session);
                refresh(&window, &session, &form);
            }
        });
        window.on_apply({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let token = {
                    let form = form.borrow();
                    form.token.filter(|_| {
                        form.reviewed.is_some()
                            && fields(&window).ok().as_ref() == form.reviewed.as_ref()
                            && corner_form_matches(&window, &form)
                    })
                };
                let Some(token) = token else {
                    window.set_ready(false);
                    window.set_message(
                        "Preview or Review current draft before applying.".into(),
                    );
                    return;
                };
                match session.apply_overlay(Actor::User, token) {
                    Ok(result) => {
                        *form.borrow_mut() = Form::default();
                        window.set_message(if result["operation"] == "corners" { "Overlay corners updated. Undo restores the previous projection." } else if result["operation"] == "rebind" { "Overlay rebound and selected. Undo restores its previous binding and projection." } else if result["operation"] == "refit" { "Overlay refitted and selected. Undo restores its previous projection." } else { "Overlay created and selected. Undo removes it." }.into());
                        refresh(&window, &session, &form);
                        let _ = window.hide();
                    }
                    Err(error) => {
                        window.set_message(error.into());
                        refresh(&window, &session, &form);
                    }
                }
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
        Ok(Self {
            window,
            form,
            viewport,
        })
    }

    pub fn set_viewport(&self, size: (u32, u32)) {
        self.viewport.set(size);
    }

    fn handle_token(&self) -> Result<u64, String> {
        self.form
            .borrow()
            .token
            .ok_or_else(|| "Review the overlay corner draft before using its handles".into())
    }

    pub fn pick_handle(&self, session: &mut Session, pointer: Pointer) -> Result<(), String> {
        let state =
            session.pick_overlay_corner_handle(Actor::User, self.handle_token()?, pointer)?;
        adopt(&self.window, &state, &self.form);
        refresh(&self.window, session, &self.form);
        Ok(())
    }

    pub fn move_handle(&self, session: &mut Session, pointer: Pointer) -> Result<(), String> {
        let state =
            session.move_overlay_corner_handle(Actor::User, self.handle_token()?, pointer)?;
        adopt(&self.window, &state, &self.form);
        refresh(&self.window, session, &self.form);
        Ok(())
    }

    pub fn finish_handles(&self, session: &mut Session) -> Result<(), String> {
        let state = session.finish_overlay_corner_handles(Actor::User, self.handle_token()?)?;
        adopt(&self.window, &state, &self.form);
        self.window.set_message("Review the cyan outline and local U/V values, then Apply corners or Cancel. Undo restores the previous projection.".into());
        refresh(&self.window, session, &self.form);
        self.window.show().map_err(|error| error.to_string())
    }

    pub fn cancel_handles(&self, session: &mut Session) -> Result<(), String> {
        self.discard_handles(session)?;
        self.window.show().map_err(|error| error.to_string())
    }

    pub fn discard_handles(&self, session: &mut Session) -> Result<(), String> {
        let state = session.cancel_overlay_corner_handles(Actor::User, self.handle_token()?)?;
        adopt(&self.window, &state, &self.form);
        self.window.set_message(
            "Viewport handles cancelled. The exact pre-handle corner draft is restored.".into(),
        );
        refresh(&self.window, session, &self.form);
        Ok(())
    }

    pub fn show(&self, session: &mut Session, material: &str) -> Result<(), slint::PlatformError> {
        let state = session.get_overlay_state();
        let mut form = self.form.borrow_mut();
        form.source = source(&state["selected_source"]);
        form.snapshot = state["snapshot"].as_str().unwrap_or_default().into();
        form.reviewed = None;
        form.refit = false;
        form.rebind = false;
        form.corners = false;
        form.reviewed_corners = None;
        form.target = None;
        form.choices_offset = 0;
        form.serial += 1;
        drop(form);
        self.window.set_overlay_search("".into());
        self.window.set_rebind_target(
            "Choose an existing overlay to move it to the selected destination face.".into(),
        );
        refresh_choices(&self.window, session, &self.form);
        load(&self.window, &Value::Null);
        self.window.set_material(material.into());
        self.window.set_message(if !state["selected_overlay"].is_null() { "Edit selected corners adjusts the current projection; Refit selected overlay fits its full bound face." } else { state[if session.face_mode() { "selection_error" } else { "refit_error" }].as_str().unwrap_or("Choose a material, then Fit / Preview. Review current draft loads a shared overlay preview.") }.into());
        refresh_material(&self.window, session);
        refresh(&self.window, session, &self.form);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        let generation = session.materials().generation();
        if self.form.borrow().material_generation != generation {
            self.form.borrow_mut().material_generation = generation;
            refresh_material(&self.window, session);
        }
        if self.form.borrow().seen != session.overlay_stamp() {
            refresh(&self.window, session, &self.form);
        }
    }

    pub fn material_target(&self, session: &Session) -> MaterialTarget {
        MaterialTarget {
            snapshot: session.overlay_stamp(),
            serial: self.form.borrow().serial,
        }
    }

    pub fn choose_material(
        &self,
        session: &mut Session,
        target: &MaterialTarget,
        material: &str,
    ) -> Result<(), String> {
        if self.form.borrow().refit
            || !self.window.window().is_visible()
            || target.snapshot != session.overlay_stamp()
            || target.serial != self.form.borrow().serial
        {
            return Err(
                "Overlay fields, map or draft changed. Open the material picker again.".into(),
            );
        }
        self.window.set_material(material.into());
        edited(&self.window, session, &self.form);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::{BrushGeometry, ObjectId};
    use slint::Model;
    use std::sync::{Arc, Mutex};

    #[test]
    fn overlay_controls_guard_fields_material_choices_and_shared_draft_ownership() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (assets, game) = crate::test_assets::game();
        std::fs::write(
            assets.path().join("materials/test/broken.vmt"),
            "LightmappedGeneric { $basetexture }",
        )
        .unwrap();
        let mut session = Session::new(Ok(game));
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::new(192.0, 128.0, 16.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let brush = session.document().solid(solid).unwrap();
        let geometry = BrushGeometry::from_solid(brush).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.9)
            .unwrap();
        let source = FaceId {
            solid,
            side: brush.sides[face.side].id,
        };
        session
            .select_faces(&[source], crate::session::SelectMode::Replace)
            .unwrap();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&mut lock(&shared), "test/checker").unwrap();
        assert!(controller.window.get_can_preview());
        assert!(!controller.window.get_outline().is_empty());
        assert!(controller.window.get_material_image().size().width > 0);
        let material_error = lock(&shared).material_preview("test/broken").unwrap_err();
        controller.window.set_material("test/broken".into());
        controller.window.invoke_edited();
        assert_eq!(controller.window.get_material_detail(), material_error);
        controller.window.set_material("test/checker".into());
        controller.window.invoke_edited();
        controller.window.set_targetname("floor_wayfinding".into());
        controller.window.set_render_order(1);
        controller.window.set_u_start("2".into());
        controller.window.invoke_fit();
        assert!(
            controller.window.get_ready(),
            "{}",
            controller.window.get_message()
        );
        assert_eq!(controller.window.get_u_start(), "0");
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        let before_competing = lock(&shared).get_overlay_state();
        lock(&shared)
            .edit_faces(
                Actor::Agent,
                &[source],
                map_core::surface::SurfaceEdit {
                    shift_u: Some(12.125),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        let blocked = lock(&shared).get_overlay_state();
        assert_ne!(blocked["snapshot"], before_competing["snapshot"]);
        assert_eq!(
            blocked["draft"]["token"],
            before_competing["draft"]["token"]
        );
        assert_eq!(blocked["draft"]["valid"], false);
        lock(&shared).cancel_face_preview();
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_ready());
        assert!(controller.window.get_message().is_empty());
        assert_eq!(lock(&shared).get_overlay_state(), before_competing);
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        let texture = assets.path().join("materials/test/checker.vtf");
        let texture_bytes = std::fs::read(&texture).unwrap();
        std::fs::write(&texture, b"broken VTF").unwrap();
        lock(&shared).reload_material_assets(Actor::Agent);
        controller.refresh(&lock(&shared));
        assert!(
            controller
                .window
                .get_material_detail()
                .contains("checker.vtf")
        );
        assert!(controller.window.get_ready());
        assert_eq!(lock(&shared).get_overlay_state(), before_competing);
        std::fs::write(&texture, texture_bytes).unwrap();
        lock(&shared).reload_material_assets(Actor::Agent);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_material_image().size().width > 0);
        assert!(controller.window.get_ready());
        assert_eq!(lock(&shared).get_overlay_state(), before_competing);
        controller
            .window
            .window()
            .set_size(slint::PhysicalSize::new(860, 940));
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "overlay-authoring");
        for label in [
            "Browse materials…",
            "Review current draft",
            "Fit / Preview",
            "Preview settings",
            "Apply overlay",
            "Cancel",
        ] {
            crate::ui::tests::assert_dialog_button_visible(&controller.window, label);
        }

        let owned = lock(&shared).get_overlay_state()["draft"]["token"]
            .as_u64()
            .unwrap();
        let material_target = controller.material_target(&lock(&shared));
        controller.window.set_u_end("0".into());
        controller.window.invoke_edited();
        controller.window.invoke_preview();
        assert!(!controller.window.get_ready());
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["token"], owned);
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert!(
            controller
                .choose_material(&mut lock(&shared), &material_target, "test/other")
                .is_err()
        );
        assert_eq!(controller.window.get_material(), "test/checker");

        controller.window.set_u_end("1".into());
        controller.window.invoke_edited();
        let material_target = controller.material_target(&lock(&shared));
        controller
            .choose_material(&mut lock(&shared), &material_target, "test/checker")
            .unwrap();
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        let foreign = {
            let mut session = lock(&shared);
            let snapshot = session.overlay_stamp();
            session
                .preview_overlay(
                    Actor::Agent,
                    Request {
                        source,
                        options: Options {
                            material: "test/checker".into(),
                            targetname: Some("shared_wall_mark".into()),
                            u: [1.0, 0.0],
                            ..Options::default()
                        },
                    },
                    &snapshot,
                )
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap()
        };
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["token"], foreign);
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["owner"], "agent");

        controller.show(&mut lock(&shared), "test/checker").unwrap();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.window.invoke_review();
        assert!(controller.window.get_ready());
        assert_eq!(controller.window.get_targetname(), "shared_wall_mark");
        assert_eq!(controller.window.get_u_start(), "1");
        assert_eq!(controller.window.get_u_end(), "0");
        let reviewed = lock(&shared).get_overlay_state();
        assert_ne!(reviewed["draft"]["token"], foreign);
        assert_eq!(reviewed["draft"]["owner"], "you");
        controller.window.invoke_apply();
        let applied = lock(&shared).document().vmf().clone();
        assert_eq!(applied.world.solids, original.world.solids);
        let overlay = applied
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("info_overlay"))
            .unwrap();
        assert_eq!(
            overlay.properties.get("targetname"),
            Some("shared_wall_mark")
        );
        assert_eq!(overlay.properties.get("StartU"), Some("1"));
        assert_eq!(overlay.properties.get("EndU"), Some("0"));
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared).redo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &applied);
        let overlay_id = overlay.id;
        {
            let mut session = lock(&shared);
            session
                .edit(Actor::Agent, "move bound face", |tx| {
                    tx.entity_mut(overlay_id)?
                        .properties
                        .set("StartU", "01.000");
                    map_core::edit::translate(
                        tx,
                        &[ObjectId::Solid(solid)],
                        DVec3::new(3.125, -2.375, 4.625),
                        true,
                    )
                })
                .unwrap();
            session
                .select(
                    &[ObjectId::Entity(overlay_id)],
                    crate::session::SelectMode::Replace,
                )
                .unwrap();
        }
        let before_refit = lock(&shared).document().vmf().clone();
        let history_refit = lock(&shared).document().undo_labels().count();
        controller.show(&mut lock(&shared), "test/checker").unwrap();
        assert!(controller.window.get_can_refit());
        assert!(!controller.window.get_can_preview());
        controller.window.invoke_refit();
        assert!(controller.window.get_refit_mode());
        assert!(controller.window.get_ready());
        assert!(!controller.window.get_can_preview());
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "overlay-refit");
        crate::ui::tests::assert_dialog_button_visible(&controller.window, "Apply refit");
        crate::ui::tests::assert_dialog_button_visible(
            &controller.window,
            "Refit selected overlay",
        );
        assert_eq!(
            lock(&shared).get_overlay_state()["draft"]["operation"],
            "refit"
        );
        let target = controller.material_target(&lock(&shared));
        assert!(
            controller
                .choose_material(&mut lock(&shared), &target, "test/other")
                .is_err()
        );
        assert_eq!(lock(&shared).document().vmf(), &before_refit);
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            history_refit
        );
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &before_refit);
        controller.show(&mut lock(&shared), "test/checker").unwrap();
        controller.window.invoke_refit();
        controller.window.invoke_apply();
        let refitted = lock(&shared).document().vmf().clone();
        assert_eq!(refitted.entities.len(), before_refit.entities.len());
        assert_eq!(lock(&shared).selected_ids(), [ObjectId::Entity(overlay_id)]);
        assert_eq!(
            lock(&shared)
                .document()
                .entity(overlay_id)
                .unwrap()
                .properties
                .get("StartU"),
            Some("01.000")
        );
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            history_refit + 1
        );
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before_refit);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &applied);
        lock(&shared).undo(Actor::Agent).unwrap();

        lock(&shared)
            .select_faces(&[source], crate::session::SelectMode::Replace)
            .unwrap();
        controller.show(&mut lock(&shared), "test/checker").unwrap();
        controller.window.invoke_fit();
        assert!(controller.window.get_ready());
        lock(&shared)
            .select_faces(&[], crate::session::SelectMode::Replace)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.window.invoke_cancel();
        assert!(lock(&shared).get_overlay_state()["draft"].is_null());
        assert_eq!(lock(&shared).document().vmf(), &original);

        for selected in [Vec::new(), vec![ObjectId::Solid(solid)]] {
            {
                let mut session = lock(&shared);
                session
                    .select(&selected, crate::session::SelectMode::Replace)
                    .unwrap();
                let snapshot = session.overlay_stamp();
                session
                    .preview_overlay(
                        Actor::Agent,
                        Request {
                            source,
                            options: Options {
                                material: "test/checker".into(),
                                ..Options::default()
                            },
                        },
                        &snapshot,
                    )
                    .unwrap();
                assert!(!session.face_mode());
                assert!(session.selected_faces().is_empty());
            }
            let before_open = lock(&shared).get_overlay_state();
            let app = crate::AppWindow::new().unwrap();
            app.set_can_apply_material(false);
            app.set_has_faces(false);
            let review = Rc::new(Controller::new(&shared).unwrap());
            app.on_create_overlay({
                let (review, shared) = (review.clone(), shared.clone());
                move || review.show(&mut lock(&shared), "test/checker").unwrap()
            });
            app.show().unwrap();
            app.window().set_size(slint::PhysicalSize::new(1600, 1100));
            app.window().take_snapshot().unwrap();
            let overlay = crate::ui::tests::scroll_inspector_to_control(&app, "Overlay…");
            crate::ui::tests::assert_dialog_button_visible(&app, "Overlay…");
            crate::ui::tests::save_ui_snapshot(app.window(), "overlay-draft-entry");
            overlay.invoke_accessible_default_action();
            assert!(review.window.window().is_visible());
            assert!(!review.window.get_can_preview());
            assert!(review.window.get_can_review());
            let opened = lock(&shared).get_overlay_state();
            assert_eq!(opened["snapshot"], before_open["snapshot"]);
            assert_eq!(opened["draft"]["token"], before_open["draft"]["token"]);
            assert_eq!(opened["draft"]["valid"], true);
            assert_eq!(lock(&shared).selected_ids(), selected);
            review.window.invoke_review();
            assert!(review.window.get_ready());
            assert_eq!(lock(&shared).selected_ids(), selected);
            review.window.invoke_apply();
            assert_eq!(
                lock(&shared).document().vmf().entities.len(),
                original.entities.len() + 1
            );
            assert_eq!(
                lock(&shared).document().vmf().world.solids,
                original.world.solids
            );
            lock(&shared).undo(Actor::Agent).unwrap();
            assert_eq!(lock(&shared).document().vmf(), &original);
            app.hide().unwrap();
        }
        let (overlay_id, destination) = {
            let mut session = lock(&shared);
            let snapshot = session.overlay_stamp();
            let token = session
                .preview_overlay(
                    Actor::Agent,
                    Request {
                        source,
                        options: Options {
                            material: "test/checker".into(),
                            targetname: Some("Rebind sign".into()),
                            u: [1.0, 0.0],
                            ..Default::default()
                        },
                    },
                    &snapshot,
                )
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            let applied = session.apply_overlay(Actor::Agent, token).unwrap();
            let ObjectId::Entity(id) =
                crate::identity::parse_id(applied["created"].as_str().unwrap()).unwrap()
            else {
                unreachable!()
            };
            session
                .edit(Actor::Agent, "raw rebind settings", |tx| {
                    tx.entity_mut(id)?.properties.set("StartU", "01.000");
                    Ok(())
                })
                .unwrap();
            let side = session
                .document()
                .solid(source.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id != source.side)
                .unwrap()
                .id;
            let destination = FaceId {
                solid: source.solid,
                side,
            };
            session
                .select_faces(&[destination], crate::session::SelectMode::Replace)
                .unwrap();
            (id, destination)
        };
        let before_rebind = lock(&shared).document().vmf().clone();
        let history_rebind = lock(&shared).document().undo_labels().count();
        let rebind = Controller::new(&shared).unwrap();
        rebind.show(&mut lock(&shared), "test/checker").unwrap();
        rebind
            .window
            .window()
            .set_size(slint::PhysicalSize::new(860, 940));
        rebind.window.set_overlay_search("rebind SIGN".into());
        rebind.window.invoke_search_overlays();
        assert_eq!(rebind.window.get_overlay_targets().row_count(), 1);
        let row = rebind.window.get_overlay_targets().row_data(0).unwrap();
        assert!(row.eligible);
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &rebind.window,
            &row.label,
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(rebind.window.get_rebind_mode());
        assert!(rebind.window.get_can_rebind());
        assert!(!rebind.window.get_can_preview());
        assert_eq!(lock(&shared).selected_faces(), [destination]);
        rebind.window.invoke_rebind();
        assert!(rebind.window.get_ready(), "{}", rebind.window.get_message());
        let draft = lock(&shared).get_overlay_state();
        assert_eq!(draft["draft"]["operation"], "rebind");
        assert_eq!(draft["draft"]["entity"], format!("entity:{overlay_id}"));
        assert_eq!(lock(&shared).document().vmf(), &before_rebind);
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            history_rebind
        );
        for label in ["Preview rebind", "Apply rebind", "Cancel"] {
            crate::ui::tests::assert_dialog_button_visible(&rebind.window, label);
        }
        crate::ui::tests::save_ui_snapshot(rebind.window.window(), "overlay-rebind");
        rebind.window.invoke_cancel();
        assert!(lock(&shared).get_overlay_state()["draft"].is_null());
        assert_eq!(lock(&shared).document().vmf(), &before_rebind);
        let foreign = {
            let mut session = lock(&shared);
            session
                .select(
                    &[ObjectId::Entity(overlay_id)],
                    crate::session::SelectMode::Replace,
                )
                .unwrap();
            assert!(!session.face_mode());
            let snapshot = session.overlay_stamp();
            session
                .preview_overlay_rebind(Actor::Agent, overlay_id, destination, &snapshot)
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap()
        };
        rebind.show(&mut lock(&shared), "test/checker").unwrap();
        rebind.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before_rebind);
        rebind.window.invoke_review();
        assert!(rebind.window.get_rebind_mode());
        assert!(rebind.window.get_ready());
        assert_ne!(lock(&shared).get_overlay_state()["draft"]["token"], foreign);
        rebind.window.invoke_apply();
        assert_eq!(
            lock(&shared)
                .document()
                .entity(overlay_id)
                .unwrap()
                .properties
                .get("sides"),
            Some(destination.side.to_string().as_str())
        );
        assert_eq!(
            lock(&shared)
                .document()
                .entity(overlay_id)
                .unwrap()
                .properties
                .get("StartU"),
            Some("01.000")
        );
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            history_rebind + 1
        );
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before_rebind);
        {
            let mut session = lock(&shared);
            session
                .select(
                    &[ObjectId::Entity(overlay_id)],
                    crate::session::SelectMode::Replace,
                )
                .unwrap();
        }
        let corners = Controller::new(&shared).unwrap();
        corners.show(&mut lock(&shared), "test/checker").unwrap();
        corners
            .window
            .window()
            .set_size(slint::PhysicalSize::new(900, 1100));
        assert!(corners.window.get_can_edit_corners());
        corners.window.invoke_edit_corners();
        assert!(corners.window.get_corners_mode());
        assert!(!corners.window.get_ready());
        let selected = lock(&shared).get_overlay_state()["selected_corners"].clone();
        assert_eq!(
            corner_fields(&corners.window).unwrap(),
            serde_json::from_value::<[[f64; 2]; 4]>(selected["local_corners"].clone()).unwrap()
        );
        corners.window.set_corner_0_u("-24.5".into());
        corners.window.set_corner_0_v("-16.25".into());
        corners.window.set_corner_1_u("-20".into());
        corners.window.set_corner_1_v("24.75".into());
        corners.window.set_corner_2_u("28.5".into());
        corners.window.set_corner_2_v("16.5".into());
        corners.window.set_corner_3_u("32.25".into());
        corners.window.set_corner_3_v("-20.125".into());
        corners.window.invoke_corners_edited();
        let before_corners = lock(&shared).document().vmf().clone();
        let history_corners = lock(&shared).document().undo_labels().count();
        corners.window.invoke_preview_corners();
        assert!(
            corners.window.get_ready(),
            "{}",
            corners.window.get_message()
        );
        let draft = lock(&shared).get_overlay_state();
        assert_eq!(draft["draft"]["operation"], "corners");
        assert_eq!(lock(&shared).document().vmf(), &before_corners);
        corners.window.set_corner_0_u("NaN".into());
        corners.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before_corners);
        corners.window.invoke_preview_corners();
        assert!(!corners.window.get_ready());
        assert_eq!(lock(&shared).get_overlay_state()["draft"], draft["draft"]);
        corners.window.invoke_review();
        assert!(corners.window.get_ready());
        assert_eq!(corners.window.get_corner_0_u(), "-24.5");
        for label in ["Edit selected corners", "Apply corners", "Cancel"] {
            crate::ui::tests::assert_dialog_button_visible(&corners.window, label);
        }
        crate::ui::tests::save_ui_snapshot(corners.window.window(), "overlay-corners");
        assert!(corners.window.get_can_handle_corners());
        let origin =
            serde_json::from_value::<[f64; 3]>(draft["draft"]["basis_origin"].clone()).unwrap();
        lock(&shared).set_camera(render::Camera::planar(
            render::Projection::Top,
            DVec3::from_array(origin).as_vec3(),
            200.0,
        ));
        corners.set_viewport((900, 700));
        for cancel in [true, false] {
            let pre_handle = lock(&shared).get_overlay_state()["draft"]["local_corners"].clone();
            corners.window.invoke_handle_corners();
            assert!(!corners.window.window().is_visible());
            let state = lock(&shared).get_overlay_state();
            assert_eq!(state["draft"]["handles"]["valid"], true);
            assert!(!corners.window.get_ready());
            if cancel {
                let app = crate::AppWindow::new().unwrap();
                app.show().unwrap();
                app.window().set_size(slint::PhysicalSize::new(1600, 900));
                app.window().take_snapshot().unwrap();
                {
                    let session = lock(&shared);
                    let size = (
                        app.get_viewport_width().round().max(1.0) as u32,
                        app.get_viewport_height().round().max(1.0) as u32,
                    );
                    app.set_overlay_handling(session.overlay_corner_handles_interactive(
                        Actor::User,
                        session.camera(),
                        size,
                    ));
                    app.set_can_cancel_overlay_handles(
                        session.overlay_corner_handles_owned(Actor::User),
                    );
                    app.set_overlay_handle_notice(
                        session
                            .overlay_corner_handle_notice(Actor::User, session.camera(), size)
                            .unwrap()
                            .into(),
                    );
                }
                assert!(!app.get_overlay_handling());
                assert!(app.get_can_cancel_overlay_handles());
                assert!(
                    app.get_overlay_handle_notice()
                        .contains("dimensions changed")
                );
                crate::ui::tests::assert_dialog_button_visible(&app, "Cancel handles");
                app.hide().unwrap();
            }
            let start = Pointer {
                position: DVec2::new(
                    state["draft"]["handles"]["pixels"][0][0].as_f64().unwrap(),
                    state["draft"]["handles"]["pixels"][0][1].as_f64().unwrap(),
                ),
                size: (900, 700),
            };
            corners.pick_handle(&mut lock(&shared), start).unwrap();
            corners
                .move_handle(
                    &mut lock(&shared),
                    Pointer {
                        position: start.position + DVec2::new(-12.0, 8.0),
                        ..start
                    },
                )
                .unwrap();
            assert_eq!(lock(&shared).document().vmf(), &before_corners);
            assert_eq!(
                lock(&shared).document().undo_labels().count(),
                history_corners
            );
            if cancel {
                corners.cancel_handles(&mut lock(&shared)).unwrap();
                assert_eq!(
                    lock(&shared).get_overlay_state()["draft"]["local_corners"],
                    pre_handle
                );
            } else {
                corners.finish_handles(&mut lock(&shared)).unwrap();
                assert_ne!(
                    lock(&shared).get_overlay_state()["draft"]["local_corners"],
                    pre_handle
                );
                assert_eq!(
                    serde_json::to_value(corner_fields(&corners.window).unwrap()).unwrap(),
                    lock(&shared).get_overlay_state()["draft"]["local_corners"]
                );
            }
            assert!(corners.window.window().is_visible());
            assert!(corners.window.get_ready());
            assert!(!lock(&shared).overlay_corner_handles_active());
        }
        crate::ui::tests::assert_dialog_button_visible(&corners.window, "Viewport handles");
        crate::ui::tests::save_ui_snapshot(corners.window.window(), "overlay-corner-handles");
        corners.window.invoke_apply();
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            history_corners + 1
        );
        assert_eq!(lock(&shared).selected_ids(), [ObjectId::Entity(overlay_id)]);
        let after_corners = lock(&shared).document().vmf().clone();
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before_corners);
        lock(&shared).redo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &after_corners);
        let foreign = {
            let mut session = lock(&shared);
            let snapshot = session.overlay_stamp();
            session
                .preview_overlay_corners(
                    Actor::Agent,
                    overlay_id,
                    [[-12.0, -8.0], [-10.0, 12.0], [14.0, 8.0], [16.0, -10.0]],
                    &snapshot,
                )
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap()
        };
        corners.show(&mut lock(&shared), "test/checker").unwrap();
        corners.window.invoke_apply();
        corners.window.invoke_cancel();
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["token"], foreign);
        corners.show(&mut lock(&shared), "test/checker").unwrap();
        corners.window.invoke_review();
        assert!(corners.window.get_corners_mode());
        assert!(corners.window.get_ready());
        corners.window.invoke_cancel();
        assert!(lock(&shared).get_overlay_state()["draft"].is_null());
        assert_eq!(lock(&shared).document().vmf(), &after_corners);
    }
}
