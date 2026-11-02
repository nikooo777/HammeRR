use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::DVec3;
use map_core::{ObjectId, surface::FaceId};
use serde_json::{Value, json};
use slint::ComponentHandle;

use crate::{
    RepeatWindow,
    runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock},
    session::{
        Actor, Session,
        repeat::{PivotSource, Policies, Request, ScatterRequest},
    },
};

#[derive(Clone)]
struct Context {
    ids: Vec<ObjectId>,
    faces: Vec<FaceId>,
    scatter: bool,
    generation: u64,
    revision: u64,
    definitions: u64,
}

#[derive(Clone, PartialEq)]
enum Prepared {
    Array(Request),
    Scatter(ScatterRequest),
}

pub struct Controller {
    pub window: RepeatWindow,
    context: Rc<RefCell<Option<Context>>>,
    token: Rc<Cell<Option<u64>>>,
    prepared: Rc<RefCell<Option<Prepared>>>,
}

fn number(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "Enter finite numbers for placement settings".into())
}

fn vector(values: [slint::SharedString; 3]) -> Result<DVec3, String> {
    Ok(DVec3::new(
        number(&values[0])?,
        number(&values[1])?,
        number(&values[2])?,
    ))
}

fn policies(window: &RepeatWindow) -> Result<Policies, String> {
    let style = match window.get_name_style() {
        0 => "source_suffix",
        1 => "indexed_prefix",
        _ => return Err("Choose a name format".into()),
    };
    let internal = match window.get_internal_links() {
        0 => "remap",
        1 => "keep_original",
        _ => return Err("Choose an internal link policy".into()),
    };
    let external = match window.get_external_links() {
        0 => "keep",
        1 => "reject",
        _ => return Err("Choose an external link policy".into()),
    };
    serde_json::from_value(json!({
        "naming":{"style":style,"prefix":window.get_name_prefix().as_str(),"suffix":window.get_name_suffix().as_str(),"start_index":window.get_name_start()},
        "internal":internal,"external":external,
    }))
    .map_err(|error| format!("Invalid naming or reference settings: {error}"))
}

fn pivot_options(window: &RepeatWindow) -> Result<(Option<PivotSource>, Option<DVec3>), String> {
    let pivot_source = match window.get_pivot_source() {
        0 => PivotSource::SelectionCenter,
        1 => PivotSource::CurrentTransformPivot,
        2 => PivotSource::CustomWorldPoint,
        _ => return Err("Choose a pivot source".into()),
    };
    let pivot = if pivot_source == PivotSource::CustomWorldPoint {
        Some(vector([
            window.get_pivot_x(),
            window.get_pivot_y(),
            window.get_pivot_z(),
        ])?)
    } else {
        None
    };
    Ok((Some(pivot_source), pivot))
}

fn options(window: &RepeatWindow) -> Result<Request, String> {
    let local = match window.get_space_index() {
        0 => false,
        1 => true,
        _ => return Err("Choose World or Source local".into()),
    };
    let (pivot_source, pivot) = pivot_options(window)?;
    Ok(Request {
        count: u32::try_from(window.get_count()).map_err(|_| "Choose a positive copy count")?,
        translation: vector([
            window.get_move_x(),
            window.get_move_y(),
            window.get_move_z(),
        ])?,
        rotation: vector([
            window.get_rotate_x(),
            window.get_rotate_y(),
            window.get_rotate_z(),
        ])?,
        scale: vector([
            window.get_scale_x(),
            window.get_scale_y(),
            window.get_scale_z(),
        ])?,
        pivot,
        texture_lock: window.get_texture_lock(),
        pivot_source,
        local,
        policies: policies(window)?,
    })
}

fn scatter_options(window: &RepeatWindow, generation: u64) -> Result<ScatterRequest, String> {
    let (pivot_source, pivot) = if window.get_automatic_anchor() {
        (None, None)
    } else {
        pivot_options(window)?
    };
    Ok(ScatterRequest {
        count: u32::try_from(window.get_count()).map_err(|_| "Choose a positive copy count")?,
        seed: window
            .get_seed()
            .parse()
            .map_err(|_| "Enter a whole-number seed from 0 to 18446744073709551615")?,
        align_normal: window.get_align(),
        heading: [
            number(&window.get_heading_min())?,
            number(&window.get_heading_max())?,
        ],
        scale: [
            number(&window.get_uniform_scale_min())?,
            number(&window.get_uniform_scale_max())?,
        ],
        normal_offset: number(&window.get_normal_offset())?,
        texture_lock: window.get_texture_lock(),
        pivot_source,
        pivot,
        policies: policies(window)?,
        source_generation: generation,
    })
}

fn requested(window: &RepeatWindow, context: &Context) -> Result<Prepared, String> {
    if context.scatter {
        scatter_options(window, context.generation).map(Prepared::Scatter)
    } else {
        options(window).map(Prepared::Array)
    }
}

fn load_policies(window: &RepeatWindow, policies: &Policies) {
    let policies = json!(policies);
    window.set_name_style(i32::from(policies["naming"]["style"] == "indexed_prefix"));
    window.set_name_prefix(
        policies["naming"]["prefix"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    window.set_name_suffix(
        policies["naming"]["suffix"]
            .as_str()
            .unwrap_or_default()
            .into(),
    );
    window.set_name_start(policies["naming"]["start_index"].as_u64().unwrap_or(1) as i32);
    window.set_internal_links(i32::from(policies["internal"] == "keep_original"));
    window.set_external_links(i32::from(policies["external"] == "reject"));
}

fn load_pivot(window: &RepeatWindow, source: Option<PivotSource>, pivot: Option<DVec3>) {
    window.set_automatic_anchor(source.is_none());
    window.set_pivot_source(match source.unwrap_or_default() {
        PivotSource::SelectionCenter => 0,
        PivotSource::CurrentTransformPivot => 1,
        PivotSource::CustomWorldPoint => 2,
    });
    let pivot = pivot.unwrap_or_default();
    window.set_pivot_x(pivot.x.to_string().into());
    window.set_pivot_y(pivot.y.to_string().into());
    window.set_pivot_z(pivot.z.to_string().into());
}

fn report_pivot(
    request: &Value,
    automatic: bool,
) -> Result<(Option<PivotSource>, Option<DVec3>), String> {
    let source = match request["pivot_source"].as_str() {
        None if automatic && request["pivot_source"].is_null() => None,
        Some("selection_center") => Some(PivotSource::SelectionCenter),
        Some("current_transform_pivot") => Some(PivotSource::CurrentTransformPivot),
        Some("custom_world_point") => Some(PivotSource::CustomWorldPoint),
        _ => return Err("Could not read placement pivot source".into()),
    };
    let pivot = if source == Some(PivotSource::CustomWorldPoint) {
        Some(DVec3::from_array(
            serde_json::from_value(request["pivot"].clone())
                .map_err(|_| "Could not read placement custom pivot")?,
        ))
    } else {
        None
    };
    Ok((source, pivot))
}

fn load_settings(window: &RepeatWindow, request: Request) {
    window.set_count(request.count as i32);
    window.set_space_index(i32::from(request.local));
    window.set_move_x(request.translation.x.to_string().into());
    window.set_move_y(request.translation.y.to_string().into());
    window.set_move_z(request.translation.z.to_string().into());
    window.set_rotate_x(request.rotation.x.to_string().into());
    window.set_rotate_y(request.rotation.y.to_string().into());
    window.set_rotate_z(request.rotation.z.to_string().into());
    window.set_scale_x(request.scale.x.to_string().into());
    window.set_scale_y(request.scale.y.to_string().into());
    window.set_scale_z(request.scale.z.to_string().into());
    load_pivot(window, request.pivot_source, request.pivot);
    window.set_texture_lock(request.texture_lock);
    load_policies(window, &request.policies);
}

fn preview_settings(preview: &Value) -> Result<Request, String> {
    let request = &preview["request"];
    let vector = |key: &str| -> Result<DVec3, String> {
        serde_json::from_value::<[f64; 3]>(request[key].clone())
            .map(DVec3::from_array)
            .map_err(|_| format!("Could not read repeat {key}"))
    };
    let (pivot_source, pivot) = report_pivot(request, false)?;
    Ok(Request {
        count: request["count"]
            .as_u64()
            .and_then(|count| u32::try_from(count).ok())
            .ok_or("Could not read repeat count")?,
        translation: vector("translation")?,
        rotation: vector("rotation")?,
        scale: vector("scale")?,
        local: request["space"] == "local",
        texture_lock: request["texture_lock"] == true,
        pivot_source,
        pivot,
        policies: serde_json::from_value(request["policies"].clone())
            .map_err(|error| format!("Could not read placement policies: {error}"))?,
    })
}

fn scatter_settings(preview: &Value) -> Result<ScatterRequest, String> {
    let request = &preview["request"];
    let (pivot_source, pivot) = report_pivot(request, true)?;
    Ok(ScatterRequest {
        count: request["count"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or("Could not read scatter count")?,
        seed: request["seed"]
            .as_u64()
            .ok_or("Could not read scatter seed")?,
        align_normal: request["align_normal"]
            .as_bool()
            .ok_or("Could not read scatter alignment")?,
        heading: serde_json::from_value(request["heading"].clone())
            .map_err(|_| "Could not read scatter heading range")?,
        scale: serde_json::from_value(request["scale"].clone())
            .map_err(|_| "Could not read scatter scale range")?,
        normal_offset: request["normal_offset"]
            .as_f64()
            .ok_or("Could not read scatter offset")?,
        texture_lock: request["texture_lock"]
            .as_bool()
            .ok_or("Could not read scatter texture policy")?,
        pivot_source,
        pivot,
        policies: serde_json::from_value(request["policies"].clone())
            .map_err(|error| format!("Could not read scatter policies: {error}"))?,
        source_generation: request["source_generation"]
            .as_u64()
            .ok_or("Could not read scatter source generation")?,
    })
}

fn load_scatter_settings(window: &RepeatWindow, request: &ScatterRequest) {
    window.set_count(request.count as i32);
    window.set_seed(request.seed.to_string().into());
    window.set_align(request.align_normal);
    window.set_heading_min(request.heading[0].to_string().into());
    window.set_heading_max(request.heading[1].to_string().into());
    window.set_uniform_scale_min(request.scale[0].to_string().into());
    window.set_uniform_scale_max(request.scale[1].to_string().into());
    window.set_normal_offset(request.normal_offset.to_string().into());
    window.set_texture_lock(request.texture_lock);
    load_pivot(window, request.pivot_source, request.pivot);
    load_policies(window, &request.policies);
}

fn validate(session: &Session, context: &Context, token: Option<u64>) -> Result<(), String> {
    if context.generation != session.document_generation() {
        return Err(
            "The map was replaced. Reopen Repeat array or capture scatter objects and faces again."
                .into(),
        );
    }
    if context.revision != session.document().revision()
        || context.definitions != session.definitions_serial()
    {
        return Err("Map or entity definitions changed. Reopen Repeat array or capture scatter objects and faces again.".into());
    }
    if !context.scatter && context.ids != session.selected_ids() {
        return Err("Selection changed; reopen Repeat array".into());
    }
    let report = session.repeat_array();
    if report["preview"]["token"].as_u64() != token {
        return Err("Placement preview was closed or replaced. Reopen Repeat array or recapture scatter objects and faces.".into());
    }
    if token.is_some() && report["preview"]["valid"] != true {
        return Err(report["preview"]["stale_error"]
            .as_str()
            .unwrap_or("Placement preview is stale; capture inputs and preview again")
            .into());
    }
    Ok(())
}

fn captured(window: &RepeatWindow, context: &Context) {
    let sources = context
        .ids
        .iter()
        .copied()
        .map(crate::identity::id_string)
        .collect::<Vec<_>>()
        .join(", ");
    window.set_selection(if sources.is_empty() {
        "No source objects captured".into()
    } else {
        format!("{} source objects · {sources}", context.ids.len()).into()
    });
    let faces = context
        .faces
        .iter()
        .map(|face| format!("solid:{} / side:{}", face.solid, face.side))
        .collect::<Vec<_>>()
        .join(", ");
    window.set_target_faces(if faces.is_empty() {
        "No target faces captured".into()
    } else {
        format!("{} target faces · {faces}", context.faces.len()).into()
    });
}

fn present(window: &RepeatWindow, report: &Value) {
    let preview = &report["preview"];
    window.set_ready(preview["valid"] == true);
    let mut lines = vec![format!(
        "{} additional copies · {} new objects",
        preview["count"], preview["object_count"]
    )];
    let pivot = &preview["pivot"];
    lines.push(format!(
        "Captured placement anchor · X {} · Y {} · Z {}",
        pivot[0], pivot[1], pivot[2]
    ));
    if preview["kind"] == "surface_scatter" {
        lines.push(format!(
            "Seed {} · review each sampled point and normal in the viewport",
            preview["request"]["seed"]
        ));
        if let Some(samples) = preview["samples"].as_array() {
            for sample in samples {
                lines.push(format!(
                    "Copy {} · solid:{} / side:{} · heading {}° · scale {}",
                    sample["copy"],
                    sample["face"]["solid"],
                    sample["face"]["side"],
                    sample["heading"],
                    sample["scale"]
                ));
            }
        }
    } else {
        if preview["space"] == "local" {
            lines.push(format!(
                "Source local · fixed frame from {}",
                preview["frame_source"].as_str().unwrap_or_default()
            ));
        }
        let scale = &preview["scale"];
        lines.push(format!(
            "Scale factor per copy · X {} · Y {} · Z {}",
            scale[0], scale[1], scale[2]
        ));
    }
    if let Some(copies) = preview["copies"].as_array() {
        for copy in copies {
            lines.push(format!(
                "Copy {} · {} internal reference updates",
                copy["index"], copy["remapped_references"]
            ));
            if let Some(names) = copy["names"].as_array() {
                for name in names {
                    lines.push(format!(
                        "  {} → {}",
                        name["before"].as_str().unwrap_or_default(),
                        name["after"].as_str().unwrap_or_default()
                    ));
                }
            }
        }
    }
    window.set_summary(lines.join("\n").into());
    window.set_message("Review the placement and exact names. Apply placement creates all copies in one Undo step.".into());
}

fn with_window<R>(
    window: &slint::Weak<RepeatWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&RepeatWindow, &SharedSession) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = target
        .upgrade()
        .and_then(|access| access.with_shared(|shared| Ok(body(&window, shared))));
    match result {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = RepeatWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(720.0, 860.0));
        let context: Rc<RefCell<Option<Context>>> = Rc::new(RefCell::new(None));
        let token = Rc::new(Cell::new(None));
        let prepared = Rc::new(RefCell::new(None));
        window.on_invalidate({
            let target = access.downgrade();
            let (window, prepared) = (window.as_weak(), prepared.clone());
            move || {
                with_window(&window, &target, |window, _| {
                    prepared.borrow_mut().take();
                    window.set_ready(false);
                    window.set_message("Preview changed settings before applying them.".into());
                });
            }
        });
        window.on_preview({
            let target = access.downgrade();
            let (window, context, token, prepared) = (
                window.as_weak(),
                context.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                with_window(&window, &target, |window, shared| {
                    let result = (|| {
                        let mut session = lock(shared);
                        let context = context.borrow();
                        let context = context.as_ref().ok_or("Open a placement dialog first")?;
                        validate(&session, context, token.get())?;
                        let requested = requested(window, context)?;
                        let snapshot = session.repeat_stamp();
                        let report = match &requested {
                            Prepared::Array(request) => session.preview_repeat_array_request(
                                &context.ids,
                                request.clone(),
                                &snapshot,
                            )?,
                            Prepared::Scatter(request) => session.preview_surface_scatter(
                                &context.ids,
                                &context.faces,
                                request.clone(),
                                &snapshot,
                            )?,
                        };
                        Ok::<_, String>((requested, report))
                    })();
                    match result {
                        Ok((requested, report)) => {
                            token.set(report["preview"]["token"].as_u64());
                            *prepared.borrow_mut() = Some(requested);
                            present(window, &report);
                        }
                        Err(error) => {
                            prepared.borrow_mut().take();
                            window.set_ready(false);
                            window.set_summary("".into());
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        for capture_faces in [false, true] {
            let target = access.downgrade();
            let (weak_window, context, token, prepared) = (
                window.as_weak(),
                context.clone(),
                token.clone(),
                prepared.clone(),
            );
            let capture = move || {
                with_window(&weak_window, &target, |window, shared| {
                    prepared.borrow_mut().take();
                    let result = (|| {
                        let mut session = lock(shared);
                        let current_token = session.repeat_array()["preview"]["token"].as_u64();
                        if current_token.is_some() && current_token != token.get() {
                            return Err("Another placement preview is active; cancel it before capturing scatter inputs".into());
                        }
                        let mut context = context.borrow_mut();
                        let context = context
                            .as_mut()
                            .filter(|context| context.scatter)
                            .ok_or("Open Surface scatter first")?;
                        let ids = session.selected_ids();
                        let faces = session.selected_faces();
                        if capture_faces {
                            if faces.is_empty() || faces.len() > 32 {
                                return Err(
                                    "Select 1–32 target faces, then Use selected faces".into()
                                );
                            }
                        } else {
                            if ids.is_empty() || ids.len() > map_core::repeat::MAX_SOURCES {
                                return Err(
                                    "Select 1–16 source objects, then Use selected objects".into(),
                                );
                            }
                            session.require_root_objects(&ids)?;
                        }
                        if let Some(owned) = current_token {
                            session.cancel_repeat_array(owned)?;
                        }
                        token.set(None);
                        if context.generation != session.document_generation() {
                            context.ids.clear();
                            context.faces.clear();
                        }
                        context.generation = session.document_generation();
                        context.revision = session.document().revision();
                        context.definitions = session.definitions_serial();
                        if capture_faces {
                            context.faces = faces;
                        } else {
                            context.ids = ids;
                        }
                        captured(window, context);
                        Ok::<_, String>(())
                    })();
                    window.set_ready(false);
                    window.set_summary("".into());
                    window.set_message(result.err().unwrap_or_else(|| "Captured explicit inputs. Preview scatter when both source objects and target faces are captured.".into()).into());
                });
            };
            if capture_faces {
                window.on_capture_faces(capture);
            } else {
                window.on_capture_sources(capture);
            }
        }
        window.on_use_last_settings({
            let target = access.downgrade();
            let (window, prepared) = (window.as_weak(), prepared.clone());
            move || { with_window(&window, &target, |window, shared| {
                match lock(shared).last_repeat_array_settings() {
                    Some(request) => { load_settings(window, request); prepared.borrow_mut().take(); window.set_ready(false); window.set_summary("".into()); window.set_message("Last applied array settings loaded. Preview array before applying.".into()); }
                    None => window.set_message("Apply an array before reusing its settings.".into()),
                }
            }); }
        });
        window.on_apply({
            let target = access.downgrade();
            let (window, context, token, prepared) = (
                window.as_weak(),
                context.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                with_window(&window, &target, |window, shared| {
                    let result = (|| {
                        let context = context.borrow();
                        let context = context.as_ref().ok_or("Open a placement dialog first")?;
                        if prepared.borrow().as_ref() != Some(&requested(window, context)?) {
                            return Err("Preview changed settings before applying them".into());
                        }
                        let mut session = lock(shared);
                        validate(&session, context, token.get())?;
                        session.apply_repeat_array(
                            Actor::User,
                            token.get().ok_or("Preview placement first")?,
                        )
                    })();
                    match result {
                        Ok(_) => {
                            token.set(None);
                            prepared.borrow_mut().take();
                            window.set_ready(false);
                            let _ = window.hide();
                        }
                        Err(error) => {
                            prepared.borrow_mut().take();
                            window.set_ready(false);
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        window.on_cancel({
            let target = access.downgrade();
            let (window, token, prepared) = (window.as_weak(), token.clone(), prepared.clone());
            move || {
                with_window(&window, &target, |window, shared| {
                    let mut session = lock(shared);
                    if let Some(owned) = token.get()
                        && session.repeat_array()["preview"]["token"].as_u64() == Some(owned)
                        && let Err(error) = session.cancel_repeat_array(owned)
                    {
                        window.set_message(error.into());
                        return;
                    }
                    token.set(None);
                    prepared.borrow_mut().take();
                    window.set_ready(false);
                    let _ = window.hide();
                });
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self {
            window,
            context,
            token,
            prepared,
        })
    }

    pub fn initialize(&self, session: &Session) -> Result<(), String> {
        let ids = session.selected_ids();
        if ids.is_empty() || ids.len() > map_core::repeat::MAX_SOURCES {
            return Err("Select 1–16 objects to repeat".into());
        }
        session.require_root_objects(&ids)?;
        let report = session.repeat_array();
        let preview = &report["preview"];
        if preview["kind"] == "surface_scatter" {
            return Err(
                "Cancel the current Surface scatter placement before opening Repeat array".into(),
            );
        }
        let expected = ids
            .iter()
            .copied()
            .map(crate::identity::id_string)
            .collect::<std::collections::BTreeSet<_>>();
        let current = preview["sources"].as_array().map(|sources| {
            sources
                .iter()
                .filter_map(|source| source.as_str().map(str::to_owned))
                .collect::<std::collections::BTreeSet<_>>()
        });
        let resume = preview["valid"] == true && current.as_ref() == Some(&expected);
        if !preview.is_null() && !resume {
            return Err(
                "Cancel the current placement preview before opening a different Repeat array"
                    .into(),
            );
        }
        *self.context.borrow_mut() = Some(Context {
            ids,
            faces: Vec::new(),
            scatter: false,
            generation: session.document_generation(),
            revision: session.document().revision(),
            definitions: session.definitions_serial(),
        });
        self.window.set_scatter_mode(false);
        self.token.set(if resume {
            preview["token"].as_u64()
        } else {
            None
        });
        self.prepared.borrow_mut().take();
        captured(&self.window, self.context.borrow().as_ref().unwrap());
        load_settings(
            &self.window,
            if resume {
                preview_settings(preview)?
            } else {
                Request {
                    count: 3,
                    translation: DVec3::X * 64.0,
                    rotation: DVec3::ZERO,
                    scale: DVec3::ONE,
                    local: false,
                    texture_lock: session.transform_tool().texture_lock,
                    pivot_source: Some(PivotSource::SelectionCenter),
                    pivot: None,
                    policies: Policies::default(),
                }
            },
        );
        self.window
            .set_last_settings_available(session.last_repeat_array_settings().is_some());
        self.window.set_summary("".into());
        self.window.set_ready(false);
        self.window
            .set_message("Set copy count and steps, then Preview array.".into());
        if resume {
            *self.prepared.borrow_mut() = Some(Prepared::Array(options(&self.window)?));
            present(&self.window, &report);
        }
        Ok(())
    }

    pub fn initialize_scatter(&self, session: &Session) -> Result<(), String> {
        let report = session.repeat_array();
        let preview = &report["preview"];
        if !preview.is_null() {
            if self.window.get_scatter_mode()
                && self.window.window().is_visible()
                && self.token.get() == preview["token"].as_u64()
            {
                self.refresh(session);
                return Ok(());
            }
            if preview["kind"] != "surface_scatter" || preview["valid"] != true {
                return Err(
                    "Cancel the current placement preview before opening Surface scatter".into(),
                );
            }
            let request = scatter_settings(preview)?;
            if request.source_generation != session.document_generation() {
                return Err("The map was replaced; capture scatter objects and faces again".into());
            }
            let ids = preview["sources"]
                .as_array()
                .ok_or("Could not read captured scatter objects")?
                .iter()
                .map(|id| {
                    crate::identity::parse_id(
                        id.as_str().ok_or("Could not read a captured source ID")?,
                    )
                })
                .collect::<Result<Vec<_>, String>>()?;
            let faces = preview["faces"]
                .as_array()
                .ok_or("Could not read captured target faces")?
                .iter()
                .map(|face| {
                    Ok::<_, String>(FaceId {
                        solid: face["solid"]
                            .as_u64()
                            .and_then(|value| u32::try_from(value).ok())
                            .ok_or("Could not read a captured solid ID")?,
                        side: face["side"]
                            .as_u64()
                            .and_then(|value| u32::try_from(value).ok())
                            .ok_or("Could not read a captured side ID")?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            *self.context.borrow_mut() = Some(Context {
                ids,
                faces,
                scatter: true,
                generation: request.source_generation,
                revision: session.document().revision(),
                definitions: session.definitions_serial(),
            });
            self.window.set_scatter_mode(true);
            load_scatter_settings(&self.window, &request);
            captured(&self.window, self.context.borrow().as_ref().unwrap());
            self.token.set(preview["token"].as_u64());
            *self.prepared.borrow_mut() = Some(Prepared::Scatter(request));
            present(&self.window, &report);
            return Ok(());
        }
        if self.window.get_scatter_mode() && self.window.window().is_visible() {
            self.refresh(session);
            return Ok(());
        }
        *self.context.borrow_mut() = Some(Context {
            ids: Vec::new(),
            faces: Vec::new(),
            scatter: true,
            generation: session.document_generation(),
            revision: session.document().revision(),
            definitions: session.definitions_serial(),
        });
        self.window.set_scatter_mode(true);
        self.token.set(None);
        self.prepared.borrow_mut().take();
        captured(&self.window, self.context.borrow().as_ref().unwrap());
        self.window.set_count(16);
        self.window.set_seed("1".into());
        self.window.set_align(true);
        self.window.set_automatic_anchor(true);
        self.window.set_pivot_source(0);
        self.window.set_pivot_x("0".into());
        self.window.set_pivot_y("0".into());
        self.window.set_pivot_z("0".into());
        self.window.set_heading_min("0".into());
        self.window.set_heading_max("360".into());
        self.window.set_uniform_scale_min("1".into());
        self.window.set_uniform_scale_max("1".into());
        self.window.set_normal_offset("0".into());
        self.window
            .set_texture_lock(session.transform_tool().texture_lock);
        load_policies(&self.window, &Policies::default());
        self.window.set_ready(false);
        self.window.set_summary("".into());
        self.window.set_message("Use selected objects, then select target faces and Use selected faces. Preview scatter before Apply placement.".into());
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        self.window
            .set_last_settings_available(session.last_repeat_array_settings().is_some());
        let context = self.context.borrow();
        let Some(context) = context.as_ref() else {
            return;
        };
        if let Err(error) = validate(session, context, self.token.get()) {
            self.window.set_ready(false);
            self.window.set_message(error.into());
        } else if self.token.get().is_some() && self.prepared.borrow().is_some() {
            present(&self.window, &session.repeat_array());
        }
    }
}
