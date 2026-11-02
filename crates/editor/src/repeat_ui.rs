use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::DVec3;
use map_core::ObjectId;
use slint::ComponentHandle;

use crate::{
    RepeatWindow,
    runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock},
    session::{
        Actor, Session,
        repeat::{PivotSource, Request},
    },
};

#[derive(Clone)]
struct Context {
    ids: Vec<ObjectId>,
    generation: u64,
    revision: u64,
    definitions: u64,
}

pub struct Controller {
    pub window: RepeatWindow,
    context: Rc<RefCell<Option<Context>>>,
    token: Rc<Cell<Option<u64>>>,
    prepared: Rc<Cell<Option<Request>>>,
}

fn options(window: &RepeatWindow) -> Result<Request, String> {
    let vector = |values: [slint::SharedString; 3]| {
        let mut result = DVec3::ZERO;
        for (index, value) in values.iter().enumerate() {
            result[index] = value
                .parse::<f64>()
                .map_err(|_| "Enter finite XYZ numbers")?;
        }
        Ok::<DVec3, String>(result)
    };
    let local = match window.get_space_index() {
        0 => false,
        1 => true,
        _ => return Err("Choose World or Source local".into()),
    };
    let pivot_source = match window.get_pivot_source() {
        0 => PivotSource::SelectionCenter,
        1 => PivotSource::CurrentTransformPivot,
        2 => PivotSource::CustomWorldPoint,
        _ => return Err("Choose a pivot source".into()),
    };
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
        pivot: if pivot_source == PivotSource::CustomWorldPoint {
            Some(vector([
                window.get_pivot_x(),
                window.get_pivot_y(),
                window.get_pivot_z(),
            ])?)
        } else {
            None
        },
        texture_lock: window.get_texture_lock(),
        pivot_source: Some(pivot_source),
        local,
    })
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
    window.set_pivot_source(match request.pivot_source.unwrap_or_default() {
        PivotSource::SelectionCenter => 0,
        PivotSource::CurrentTransformPivot => 1,
        PivotSource::CustomWorldPoint => 2,
    });
    let pivot = request.pivot.unwrap_or_default();
    window.set_pivot_x(pivot.x.to_string().into());
    window.set_pivot_y(pivot.y.to_string().into());
    window.set_pivot_z(pivot.z.to_string().into());
    window.set_texture_lock(request.texture_lock);
}

fn preview_settings(preview: &serde_json::Value) -> Result<Request, String> {
    let request = &preview["request"];
    let vector = |key: &str| -> Result<DVec3, String> {
        serde_json::from_value::<[f64; 3]>(request[key].clone())
            .map(DVec3::from_array)
            .map_err(|_| format!("Could not read repeat {key}"))
    };
    let source = match request["pivot_source"].as_str() {
        Some("selection_center") => PivotSource::SelectionCenter,
        Some("current_transform_pivot") => PivotSource::CurrentTransformPivot,
        Some("custom_world_point") => PivotSource::CustomWorldPoint,
        _ => return Err("Could not read repeat pivot source".into()),
    };
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
        pivot_source: Some(source),
        pivot: if source == PivotSource::CustomWorldPoint {
            Some(vector("pivot")?)
        } else {
            None
        },
    })
}

fn validate(
    session: &Session,
    context: &RefCell<Option<Context>>,
    token: Option<u64>,
) -> Result<Vec<ObjectId>, String> {
    let context = context.borrow();
    let context = context.as_ref().ok_or("Reopen Repeat array")?;
    if context.generation != session.document_generation()
        || context.revision != session.document().revision()
        || context.definitions != session.definitions_serial()
        || context.ids != session.selected_ids()
    {
        return Err("Map, selection or entity definitions changed; reopen Repeat array".into());
    }
    if session.repeat_array()["preview"]["token"].as_u64() != token {
        return Err("Repeat array was replaced; reopen to review it".into());
    }
    Ok(context.ids.clone())
}

fn present(window: &RepeatWindow, report: &serde_json::Value) {
    let preview = &report["preview"];
    window.set_ready(preview["valid"] == true);
    let mut lines = vec![format!(
        "{} additional copies · {} new objects",
        preview["count"], preview["object_count"]
    )];
    let pivot = &preview["pivot"];
    lines.push(format!(
        "Captured pivot · X {} · Y {} · Z {}",
        pivot[0], pivot[1], pivot[2]
    ));
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
    window.set_message(
        "Review the copies and names. Apply array creates them in one Undo step.".into(),
    );
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
            .set_size(slint::LogicalSize::new(660.0, 860.0));
        let context = Rc::new(RefCell::new(None));
        let token = Rc::new(Cell::new(None));
        let prepared = Rc::new(Cell::new(None));
        window.on_invalidate({
            let target = access.downgrade();
            let (window, prepared) = (window.as_weak(), prepared.clone());
            move || {
                with_window(&window, &target, |window, _shared| {
                    prepared.set(None);

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
                        let ids = validate(&session, &context, token.get())?;
                        let requested = options(window)?;
                        let snapshot = session.repeat_stamp();
                        let report =
                            session.preview_repeat_array_request(&ids, requested, &snapshot)?;
                        Ok::<_, String>((requested, report))
                    })();
                    match result {
                        Ok((requested, report)) => {
                            token.set(report["preview"]["token"].as_u64());
                            prepared.set(Some(requested));
                            present(window, &report);
                        }
                        Err(error) => {
                            prepared.set(None);
                            window.set_ready(false);
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        window.on_use_last_settings({
            let target = access.downgrade();
            let (window, prepared) = (window.as_weak(), prepared.clone());
            move || {
                with_window(&window, &target, |window, shared| {
                    let request = lock(shared).last_repeat_array_settings();
                    match request {
                        Some(request) => {
                            load_settings(window, request);
                            prepared.set(None);
                            window.set_ready(false);
                            window.set_summary("".into());
                            window.set_message("Last applied settings loaded. Preview array for the current selection before applying.".into());
                        }
                        None => window.set_message("Apply an array before reusing its settings.".into()),
                    }
                });
            }
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
                        if prepared.get() != Some(options(window)?) {
                            return Err("Preview changed settings before applying them".into());
                        }
                        let mut session = lock(shared);
                        validate(&session, &context, token.get())?;
                        session.apply_repeat_array(
                            Actor::User,
                            token.get().ok_or("Preview the array first")?,
                        )
                    })();
                    match result {
                        Ok(_) => {
                            token.set(None);
                            prepared.set(None);
                            window.set_ready(false);
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_cancel({
            let target = access.downgrade();
            let (window, token, prepared) = (window.as_weak(), token.clone(), prepared.clone());
            move || {
                with_window(&window, &target, |_window, shared| {
                    if let Some(token) = token.take() {
                        let _ = lock(shared).cancel_repeat_array(token);
                    }
                    prepared.set(None);
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
        let expected = ids
            .iter()
            .copied()
            .map(crate::identity::id_string)
            .collect::<std::collections::BTreeSet<_>>();
        let preview = &report["preview"];
        let current = preview["sources"].as_array().map(|sources| {
            sources
                .iter()
                .filter_map(|source| source.as_str().map(str::to_owned))
                .collect::<std::collections::BTreeSet<_>>()
        });
        let resume = preview["valid"] == true && current.as_ref() == Some(&expected);
        *self.context.borrow_mut() = Some(Context {
            ids: ids.clone(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            definitions: session.definitions_serial(),
        });
        self.token.set(if resume {
            preview["token"].as_u64()
        } else {
            None
        });
        self.prepared.set(None);
        self.window.set_selection(
            format!(
                "{} source object{}",
                ids.len(),
                if ids.len() == 1 { "" } else { "s" }
            )
            .into(),
        );
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
            self.prepared.set(Some(options(&self.window)?));
            present(&self.window, &report);
        }
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        self.window
            .set_last_settings_available(session.last_repeat_array_settings().is_some());
        if let Err(error) = validate(session, &self.context, self.token.get()) {
            self.window.set_ready(false);
            self.window.set_message(error.into());
        } else if self.token.get().is_some() && self.prepared.get().is_some() {
            present(&self.window, &session.repeat_array());
        }
    }
}
