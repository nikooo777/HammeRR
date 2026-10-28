use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use map_core::{ObjectId, query::Ray, subobject::Component};
use slint::ComponentHandle;

use crate::{
    PlacementWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{
        Actor, Session,
        placement::{Method, Options, SourceCorner},
    },
};

#[derive(Clone)]
struct Context {
    ids: Vec<ObjectId>,
    generation: u64,
    revision: u64,
    source_corner: Option<Component>,
    corners: Vec<Component>,
}

pub struct Controller {
    pub window: PlacementWindow,
    context: Rc<RefCell<Option<Context>>>,
    token: Rc<Cell<Option<u64>>>,
    picking: Rc<Cell<bool>>,
    dirty: Rc<Cell<bool>>,
}

fn validate(
    session: &Session,
    context: &RefCell<Option<Context>>,
    token: Option<u64>,
) -> Result<Vec<ObjectId>, String> {
    let context = context.borrow();
    let context = context.as_ref().ok_or("Reopen surface placement")?;
    if context.generation != session.document_generation()
        || context.revision != session.document().revision()
        || context.ids != session.selected_ids()
    {
        return Err("Map or selection changed; reopen surface placement".into());
    }
    if session.surface_placement()["token"].as_u64() != token {
        return Err("Surface placement was replaced; reopen this window to review it".into());
    }
    Ok(context.ids.clone())
}

fn options(window: &PlacementWindow, source: Option<Component>) -> Result<Options, String> {
    let offset = window
        .get_offset()
        .parse::<f64>()
        .map_err(|_| "Enter a finite surface offset")?;
    Ok(Options {
        source_corner: if window.get_method() == 3 {
            Some(source.ok_or("Choose a Source corner before picking the target")?)
        } else {
            None
        },
        align: window.get_align(),
        offset,
        texture_lock: window.get_texture_lock(),
    })
}

fn source_label(session: &Session, source: &SourceCorner) -> slint::SharedString {
    let label = match source.owner {
        ObjectId::Entity(id) => session.rename_entity_label(id),
        ObjectId::Solid(id) => session
            .document()
            .solid(id)
            .and_then(|brush| brush.sides.first())
            .map_or_else(
                || "World brush".into(),
                |side| format!("{} brush", side.material),
            ),
    };
    format!(
        "{label} · {:.3}, {:.3}, {:.3} · brush {} · faces {:?}",
        source.position.x,
        source.position.y,
        source.position.z,
        source.component.solid,
        source.component.sides
    )
    .into()
}

fn present(window: &PlacementWindow, report: &serde_json::Value) {
    window.set_ready(report["valid"] == true && !report["plan"].is_null());
    window.set_picking(report["picking"] == true);
    if let Some(error) = report["error"].as_str() {
        window.set_message(error.into());
        return;
    }
    let plan = &report["plan"];
    window.set_summary(if plan.is_null() {
        "".into()
    } else if !plan["corner"].is_null() {
        let source = &plan["corner"]["source"];
        let target = &plan["corner"]["position"];
        let delta = &plan["translation"];
        let identity = &plan["corner"]["target"];
        let target_label = if identity["kind"] == "terrain_vertex" {
            format!("Target terrain vertex · brush {} · face {} · triangle {}\nGrid {} × {} · vertex {} (row {}, column {}) · prepared surface",
                identity["solid"], identity["side"], identity["triangle"],
                identity["grid_size"], identity["grid_size"], identity["index"], identity["row"], identity["column"])
        } else if identity["kind"] == "model_vertex" {
            format!("Part {} · triangle {} · vertex {}", identity["part"], identity["triangle"], identity["vertex"])
        } else {
            format!("Target brush corner · brush {} · faces {}", identity["solid"], identity["sides"])
        };
        let point_label = if identity["kind"] == "model_vertex" { "Target model vertex" } else { "Target corner" };
        format!("{} · {:.3}, {:.3}, {:.3}\n{}\nSource corner · {:.3}, {:.3}, {:.3}\nMove · {:.3}, {:.3}, {:.3}",
            point_label,
            target[0].as_f64().unwrap(),target[1].as_f64().unwrap(),target[2].as_f64().unwrap(),
            target_label,
            source[0].as_f64().unwrap(),source[1].as_f64().unwrap(),source[2].as_f64().unwrap(),
            delta[0].as_f64().unwrap(),delta[1].as_f64().unwrap(),delta[2].as_f64().unwrap()).into()
    } else {
        let move_by = &plan["translation"];
        format!(
            "Move · X {:.2} · Y {:.2} · Z {:.2}\nSurface offset · {:.3} units",
            move_by[0].as_f64().unwrap(),
            move_by[1].as_f64().unwrap(),
            move_by[2].as_f64().unwrap(),
            plan["contact_offset"].as_f64().unwrap()
        )
        .into()
    });
    window.set_message(if report["picking"] == true {
        if report["method"] == "brush_corner" { "Click a visible brush face, terrain triangle or model mesh; its nearest corner is the target. Escape cancels picking.".into() }
        else { "Click a visible surface in the viewport. Escape cancels picking.".into() }
    } else {
        "Review the placement. Apply placement changes the map in one Undo step.".into()
    });
}

fn with_session<R>(
    window: &slint::Weak<PlacementWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&PlacementWindow, &mut Session) -> R,
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
        let access = access.downgrade();
        let window = PlacementWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(640.0, 720.0));
        let context = Rc::new(RefCell::new(None::<Context>));
        let token = Rc::new(Cell::new(None));
        let picking = Rc::new(Cell::new(false));
        let dirty = Rc::new(Cell::new(false));
        window.on_invalidate({
            let (window, access, dirty) = (window.as_weak(), access.clone(), dirty.clone());
            move || {
                with_session(&window, &access, |window, _session| {
                    dirty.set(true);
                    window.set_ready(false);
                    window.set_message("Preview changed settings before applying them.".into());
                });
            }
        });
        window.on_choose_source({
            let (window, access, context, dirty) = (
                window.as_weak(),
                access.clone(),
                context.clone(),
                dirty.clone(),
            );
            move |index| {
                with_session(&window, &access, |window, _session| {
                    if let Some(context) = context.borrow_mut().as_mut() {
                        context.source_corner = usize::try_from(index - 1)
                            .ok()
                            .and_then(|index| context.corners.get(index).cloned());
                    }
                    dirty.set(true);
                    window.set_ready(false);
                    window
                        .set_message("Pick the target again to preview this source corner.".into());
                });
            }
        });
        window.on_preview({
            let (window, access, context, token, picking, dirty) = (
                window.as_weak(),
                access.clone(),
                context.clone(),
                token.clone(),
                picking.clone(),
                dirty.clone(),
            );
            move || {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        let ids = validate(session, &context, token.get())?;
                        let options = options(
                            window,
                            context
                                .borrow()
                                .as_ref()
                                .and_then(|context| context.source_corner.clone()),
                        )?;
                        if window.get_method() >= 2 {
                            session.begin_surface_placement_pick(&ids, options)
                        } else {
                            session.preview_surface_placement(
                                Actor::User,
                                &ids,
                                if window.get_method() == 1 {
                                    Method::Ceiling
                                } else {
                                    Method::Floor
                                },
                                None,
                                options,
                            )
                        }
                    })();
                    match result {
                        Ok(report) => {
                            token.set(report["token"].as_u64());
                            picking.set(report["picking"] == true);
                            dirty.set(false);
                            present(window, &report);
                            if picking.get() {
                                let _ = window.hide();
                            }
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_apply({
            let (window, access, token, context, dirty) = (
                window.as_weak(),
                access.clone(),
                token.clone(),
                context.clone(),
                dirty.clone(),
            );
            move || {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        if dirty.get() {
                            return Err("Preview changed settings before applying them".into());
                        }

                        validate(session, &context, token.get())?;
                        session.apply_surface_placement(
                            Actor::User,
                            token.get().ok_or("Preview placement first")?,
                        )
                    })();
                    match result {
                        Ok(_) => {
                            token.set(None);
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_frame({
            let (window, access, token, context, dirty) = (
                window.as_weak(),
                access.clone(),
                token.clone(),
                context.clone(),
                dirty.clone(),
            );
            move || {
                with_session(&window, &access, |window, session| {
                    let result = (|| {
                        validate(session, &context, token.get())?;
                        if dirty.get() || session.surface_placement()["valid"] != true {
                            return Err("Preview placement before framing".into());
                        }
                        session.frame_selection(Actor::User, window.get_aspect())
                    })();
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_cancel({
            let (window, access, token, picking) = (
                window.as_weak(),
                access.clone(),
                token.clone(),
                picking.clone(),
            );
            move || {
                with_session(&window, &access, |_window, session| {
                    if let Some(token) = token.take() {
                        let _ = session.cancel_surface_placement(token);
                    }
                    picking.set(false);
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
            picking,
            dirty,
        })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        let ids = session.selected_ids();
        if ids.is_empty() {
            return Err("Select objects to place".into());
        }
        let report = session.surface_placement();
        let expected = serde_json::json!(
            ids.iter()
                .copied()
                .map(crate::identity::id_string)
                .collect::<Vec<_>>()
        );
        let resume = report["valid"] == true && report["ids"] == expected;
        if !resume {
            if let Some(token) = self.token.take() {
                let _ = session.cancel_surface_placement(token);
            }
            if report["valid"] != true
                && let Some(token) = report["token"].as_u64()
            {
                let _ = session.cancel_surface_placement(token);
            }
        }
        let source_corner = if resume {
            report["source_corner"]["solid"]
                .as_u64()
                .map(|solid| Component {
                    solid: solid as u32,
                    sides: report["source_corner"]["sides"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|side| side.as_u64().unwrap() as u32)
                        .collect(),
                })
        } else if session.component_selection().len() == 1 {
            session.component_selection().iter().next().cloned()
        } else {
            None
        };
        let choices = session.surface_placement_source_corners(&ids);
        let source_error = choices.as_ref().err().cloned();
        let mut choices = choices.unwrap_or_default();
        if let Some(component) = &source_corner
            && !choices.iter().any(|choice| &choice.component == component)
            && let Ok(shapes) = session.component_topology(&[component.solid])
            && let Some(shape) = shapes.first()
            && let Ok(indices) = shape.resolve(&component.sides)
            && indices.len() == 1
            && component.sides.len() >= 3
            && let Some(owner) = session.document().owner_of_solid(component.solid)
            && ids.contains(&owner)
        {
            choices.push(SourceCorner {
                component: component.clone(),
                owner,
                position: if resume {
                    serde_json::from_value::<[f64; 3]>(report["source_position"].clone())
                        .map(glam::DVec3::from_array)
                        .unwrap_or(shape.vertices[indices[0]].position)
                } else {
                    shape.vertices[indices[0]].position
                },
            });
        }
        let source_index = source_corner.as_ref().and_then(|corner| {
            choices
                .iter()
                .position(|choice| &choice.component == corner)
        });
        let source_corner = source_index.map(|index| choices[index].component.clone());
        self.window
            .set_source_choices(slint::ModelRc::new(slint::VecModel::from(
                std::iter::once("Choose a source corner…".into())
                    .chain(choices.iter().map(|choice| source_label(session, choice)))
                    .collect::<Vec<slint::SharedString>>(),
            )));
        self.window
            .set_source_index(source_index.map_or(0, |index| index as i32 + 1));
        self.window.set_source_error(source_error.map_or_else(
            || {
                if choices.is_empty() {
                    "Select a visible flat brush to supply the source corner.".into()
                } else {
                    "".into()
                }
            },
            Into::into,
        ));
        *self.context.borrow_mut() = Some(Context {
            ids: ids.clone(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            source_corner,
            corners: choices.into_iter().map(|choice| choice.component).collect(),
        });
        self.token.set(if resume {
            report["token"].as_u64()
        } else {
            None
        });
        self.dirty.set(false);
        self.picking.set(resume && report["picking"] == true);
        self.window.set_selection(
            format!(
                "{} object{}",
                ids.len(),
                if ids.len() == 1 { "" } else { "s" }
            )
            .into(),
        );
        self.window.set_offset(if resume {
            report["offset"].as_f64().unwrap().to_string().into()
        } else {
            "0".into()
        });
        self.window.set_align(resume && report["align"] == true);
        self.window.set_texture_lock(if resume {
            report["texture_lock"] == true
        } else {
            session.transform_tool().texture_lock
        });
        self.window.set_method({
            match report["method"].as_str() {
                Some("ceiling") => 1,
                Some("ray") => 2,
                Some("brush_corner") => 3,
                _ => 0,
            }
        });
        self.window.set_summary("".into());
        self.window.set_ready(false);
        self.window
            .set_message("Choose a target and preview the placement.".into());
        if resume {
            present(&self.window, &report);
        }
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(token) = self.token.get() {
            let report = session.surface_placement();
            if report["token"] != token {
                self.picking.set(false);
                self.window.set_ready(false);
                self.window
                    .set_message("Surface placement was replaced; reopen to review it.".into());
            } else {
                self.picking.set(report["picking"] == true);
                if !self.dirty.get() {
                    present(&self.window, &report);
                }
            }
        }
    }

    pub fn is_picking(&self) -> bool {
        self.picking.get()
    }

    pub fn pick_at_ray(&self, session: &mut Session, ray: Ray) -> Result<(), String> {
        validate(session, &self.context, self.token.get())?;
        let report = session.pick_surface_placement(
            Actor::User,
            self.token.get().ok_or("Start Pick surface first")?,
            ray,
        )?;
        self.picking.set(false);
        self.dirty.set(false);
        present(&self.window, &report);
        Ok(())
    }

    pub fn cancel_pick(&self, session: &mut Session) {
        if let Some(token) = self.token.take() {
            let _ = session.cancel_surface_placement(token);
        }
        self.picking.set(false);
        self.window.set_ready(false);
        self.window.set_message("Surface picking cancelled.".into());
    }
}
