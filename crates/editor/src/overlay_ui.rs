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
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Pointer, Session, overlay::Request},
    ui::preview_image,
};

#[derive(Default)]
struct Form {
    source: Option<FaceId>,
    additional_sources: Vec<FaceId>,
    reference: Option<FaceId>,
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

fn additional_sources(value: &Value) -> Vec<FaceId> {
    value["sources"]
        .as_array()
        .map(|sources| sources.iter().skip(1).filter_map(source).collect())
        .unwrap_or_default()
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
                            detail: if entry["eligible"] == true
                                && entry["source_count"]
                                    .as_u64()
                                    .is_some_and(|count| count > 1)
                            {
                                format!(
                                    "{} · {} bound faces",
                                    entry["material"].as_str().unwrap(),
                                    entry["source_count"]
                                )
                            } else if entry["eligible"] == true {
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
    state.reference = None;
    state.serial += 1;
    drop(state);
    window.set_rebind_target(
        format!(
            "Rebind {} · {} to the destination faces above",
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

fn choose_refit_reference(
    window: &OverlayWindow,
    session: &Session,
    form: &RefCell<Form>,
    key: &str,
) {
    let state = session.get_overlay_state();
    if form.borrow().snapshot != state["snapshot"].as_str().unwrap_or_default() {
        window.set_message(
            "Map or selection changed. Reopen Overlay to choose a reference face.".into(),
        );
        return;
    }
    let reference = state["selected_overlay"]["references"]
        .as_array()
        .and_then(|references| {
            references
                .iter()
                .filter_map(source)
                .find(|face| format!("{}:{}", face.solid, face.side) == key)
        });
    let Some(reference) = reference else {
        return;
    };
    let mut current = form.borrow_mut();
    current.reference = Some(reference);
    current.reviewed = None;
    current.serial += 1;
    drop(current);
    window.set_message("The reference face's current edges set the projection axes. Refit, then review the new outline.".into());
    refresh(window, session, form);
}

fn preview_rebind(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let request = {
        let form = form.borrow();
        form.target.zip(form.source).map(|(id, source)| {
            (
                id,
                source,
                form.additional_sources.clone(),
                form.snapshot.clone(),
            )
        })
    };
    let result = request
        .ok_or("Select destination faces and choose an existing overlay".into())
        .and_then(|(id, source, additional_sources, snapshot)| {
            session.preview_overlay_rebind_faces(
                Actor::User,
                id,
                source,
                &additional_sources,
                &snapshot,
            )
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
    let reference_required =
        !form.corners && !form.rebind && state["selected_overlay"]["reference_required"] == true;
    let references = state["selected_overlay"]["references"].as_array();
    let reference_valid = form.reference.is_some_and(|reference| {
        references.is_some_and(|references| {
            references
                .iter()
                .any(|value| source(value) == Some(reference))
        })
    });
    window.set_reference_required(reference_required);
    window.set_refit_references(ModelRc::new(VecModel::from(
        references
            .into_iter()
            .flatten()
            .filter_map(source)
            .map(|face| OverlayTargetRow {
                entity: format!("{}:{}", face.solid, face.side).into(),
                label: format!("Brush {} · face {}", face.solid, face.side).into(),
                detail: String::new().into(),
                eligible: form.snapshot == snapshot,
            })
            .collect::<Vec<_>>(),
    )));
    window.set_refit_reference_label(if reference_valid {
        let reference = form.reference.unwrap();
        format!(
            "Reference · brush {} · face {}",
            reference.solid, reference.side
        )
        .into()
    } else {
        "Choose a reference face".into()
    });
    window.set_can_refit(
        !state["selected_overlay"].is_null()
            && !form.corners
            && !form.rebind
            && form.snapshot == snapshot
            && (!reference_required || reference_valid),
    );
    let matches = draft["token"].as_u64() == form.token && draft["owner"] == "you";
    window.set_ready(
        matches
            && draft["valid"] == true
            && form.reviewed.is_some()
            && fields(window).ok().as_ref() == form.reviewed.as_ref()
            && corner_form_matches(window, &form)
            && (draft["reference"].is_null() || source(&draft["reference"]) == form.reference)
            && draft["handles"].is_null(),
    );
    window.set_can_handle_corners(form.corners && window.get_ready());
    window.set_draft_label(if draft.is_null() {
        "No overlay draft · Fit / Preview leaves the map unchanged".into()
    } else if draft["valid"] != true {
        "The shared draft is stale · reopen Overlay to preview the current source".into()
    } else if matches && draft["operation"] == "corners" {
        if form.additional_sources.is_empty() {
            format!(
                "Corner edit {} · Apply corners changes this existing projection",
                draft["entity"].as_str().unwrap_or("overlay")
            )
            .into()
        } else {
            format!(
                "Corner edit {} · {} bound faces · stored projection axes",
                draft["entity"].as_str().unwrap_or("overlay"),
                form.additional_sources.len() + 1
            )
            .into()
        }
    } else if matches && draft["operation"] == "rebind" {
        format!(
            "Rebind {} · Apply rebind moves this existing overlay to the destination",
            draft["entity"].as_str().unwrap_or("overlay")
        )
        .into()
    } else if matches && draft["operation"] == "refit" {
        if form.additional_sources.is_empty() {
            format!(
                "Refit {} · Apply refit updates this existing entity",
                draft["entity"].as_str().unwrap_or("overlay")
            )
            .into()
        } else {
            format!(
                "Refit {} · {} bound faces · reference brush {} / face {}",
                draft["entity"].as_str().unwrap_or("overlay"),
                form.additional_sources.len() + 1,
                draft["reference"]["solid"],
                draft["reference"]["side"]
            )
            .into()
        }
    } else if matches {
        if form.additional_sources.is_empty() {
            "Your fitted overlay draft · Apply overlay creates one entity".into()
        } else {
            format!(
                "Your fitted overlay draft · {} bound faces · Apply overlay creates one entity",
                form.additional_sources.len() + 1
            )
            .into()
        }
    } else {
        "A shared draft is available · Review current draft to use it".into()
    });
    let display_source = if matches && draft["valid"] == true && form.reviewed.is_some() {
        &draft["source"]
    } else if form.corners {
        &state["selected_corners"]["source"]
    } else if form.rebind {
        &state["selected_source"]
    } else if !state["selected_overlay"].is_null() {
        &state["selected_overlay"]["source"]
    } else {
        &state["selected_source"]
    };
    let source_count = if matches && draft["valid"] == true && form.reviewed.is_some() {
        draft["source_count"].as_u64().unwrap_or(1)
    } else if form.corners {
        state["selected_corners"]["source_count"]
            .as_u64()
            .unwrap_or(1)
    } else {
        display_source["source_count"]
            .as_u64()
            .or_else(|| state["selected_overlay"]["source_count"].as_u64())
            .unwrap_or(1)
    };
    window.set_source_count(source_count as i32);
    window.set_source_label(if let Some(source) = source(display_source) {
        if source_count > 1 {
            if form.corners {
                format!(
                    "Bound sources · {source_count} coplanar faces · flat or isolated planar terrain · stored projection axes"
                )
                .into()
            } else {
                format!(
                    "Sources · {source_count} coplanar faces · flat or isolated planar terrain · first quad: brush {} · face {}",
                    source.solid, source.side
                )
                .into()
            }
        } else {
            format!(
                "Source · brush {} · face {} · {}",
                source.solid, source.side,
                if map_core::overlay::is_terrain(session.document().vmf(), source) { "isolated planar terrain quad" } else { "world / detail quad" }
            )
            .into()
        }
    } else if reference_required {
        format!("Bound sources · {source_count} coplanar faces · flat or isolated planar terrain · {}", window.get_refit_reference_label()).into()
    } else {
        state[if session.face_mode() {
            "selection_error"
        } else {
            "refit_error"
        }]
        .as_str()
        .unwrap_or("Select coplanar world/detail or isolated planar terrain faces")
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
    form.additional_sources = additional_sources(draft);
    form.reference = source(&draft["reference"]);
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
        let label = if draft["previous_source_count"].is_number() {
            let labels = |key: &str| {
                draft[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(source)
                    .map(|face| format!("brush {} / face {}", face.solid, face.side))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!(
                "Existing {} · previous {} faces: {} → destination {} faces: {}. The first destination quad sets the projection axes.",
                draft["entity"].as_str().unwrap_or("overlay"),
                draft["previous_source_count"],
                labels("previous_sources"),
                draft["source_count"],
                labels("sources")
            )
        } else {
            format!(
                "Existing {} · previous brush {} / face {} → destination brush {} / face {}",
                draft["entity"].as_str().unwrap_or("overlay"),
                draft["previous_source"]["solid"],
                draft["previous_source"]["side"],
                draft["source"]["solid"],
                draft["source"]["side"]
            )
        };
        window.set_rebind_target(label.into());
    }
}

fn preview(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let result = (|| {
        let (source, additional_sources, snapshot) = {
            let form = form.borrow();
            if form.refit {
                return Err("Existing-overlay updates preserve settings. Use Preview rebind, Apply or Cancel.".into());
            }
            (
                form.source
                    .ok_or("Select coplanar world/detail or isolated planar terrain faces, including a quad, and reopen Overlay")?,
                form.additional_sources.clone(),
                form.snapshot.clone(),
            )
        };
        session.preview_overlay_faces(
            Actor::User,
            Request {
                source,
                options: fields(window)?,
            },
            &additional_sources,
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
        let form = form.borrow();
        session.preview_overlay_refit_reference(Actor::User, id, form.reference, &form.snapshot)
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
    current.additional_sources = additional_sources(selected);
    current.reference = None;
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

fn callback(
    window: &slint::Weak<OverlayWindow>,
    document: &DocumentContinuation,
    body: impl FnOnce(OverlayWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = document.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(window.clone_strong(), session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = OverlayWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let viewport = Rc::new(Cell::new((1024, 576)));
        window.on_edited({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    edited(&window, session, &form);
                });
            }
        });
        window.on_preview({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview(&window, session, &form);
                });
            }
        });
        window.on_search_overlays({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    form.borrow_mut().choices_offset = 0;
                    refresh_choices(&window, session, &form);
                });
            }
        });
        window.on_overlay_page_change({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |direction| {
                callback(&window, &document, |window, session| {
                    let offset = form.borrow().choices_offset;
                    form.borrow_mut().choices_offset = if direction < 0 {
                        offset.saturating_sub(OVERLAY_PAGE_SIZE)
                    } else {
                        offset.saturating_add(OVERLAY_PAGE_SIZE)
                    };
                    refresh_choices(&window, session, &form);
                });
            }
        });
        window.on_choose_overlay({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |entity| {
                callback(&window, &document, |window, session| {
                    choose_overlay(&window, session, &form, &entity);
                });
            }
        });
        window.on_rebind({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview_rebind(&window, session, &form);
                });
            }
        });
        window.on_edit_corners({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    edit_selected_corners(&window, session, &form);
                });
            }
        });
        window.on_preview_corners({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview_corners(&window, session, &form);
                });
            }
        });
        window.on_handle_corners({
            let (window, document, form, viewport) = (window.as_weak(), access.downgrade(), form.clone(), viewport.clone());
            move || {
                callback(&window, &document, |window, session| {
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
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_corners_edited({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, _session| {
                    let mut current = form.borrow_mut();
                    current.reviewed_corners = None;
                    current.serial += 1;
                    window.set_ready(false);
                    window.set_message("Corners changed. Preview corners before applying.".into());
                });
            }
        });
        window.on_refit({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview_refit(&window, session, &form);
                });
            }
        });
        window.on_choose_refit_reference({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |key| {
                callback(&window, &document, |window, session| {
                    choose_refit_reference(&window, session, &form, &key);
                });
            }
        });
        window.on_fit({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    if form.borrow().refit {
                        return;
                    }
                    window.set_u_start("0".into());
                    window.set_u_end("1".into());
                    window.set_v_start("0".into());
                    window.set_v_end("1".into());
                    edited(&window, session, &form);
                    preview(&window, session, &form);
                });
            }
        });
        window.on_review({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
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
                    refresh_material(&window, session);
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_apply({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
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
                            refresh(&window, session, &form);
                            let _ = window.hide();
                        }
                        Err(error) => {
                            window.set_message(error.into());
                            refresh(&window, session, &form);
                        }
                    }
                });
            }
        });
        window.on_cancel({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel_owned(session, &form);
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel_owned(session, &form);
                });
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
        form.additional_sources = additional_sources(&state["selected_source"]);
        form.reference = None;
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
            "Choose an existing overlay to move it to the selected destination faces.".into(),
        );
        refresh_choices(&self.window, session, &self.form);
        load(&self.window, &Value::Null);
        self.window.set_material(material.into());
        self.window.set_message(if state["selected_overlay"]["reference_required"] == true { "Edit selected corners uses the stored projection axes. For Refit selected overlay, choose a reference face whose current edges set the new axes." } else if !state["selected_overlay"].is_null() { "Edit selected corners adjusts the current projection; Refit selected overlay fits its full bound face." } else { state[if session.face_mode() { "selection_error" } else { "refit_error" }].as_str().unwrap_or("Choose a material, then Fit / Preview. Review current draft loads a shared overlay preview.") }.into());
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
