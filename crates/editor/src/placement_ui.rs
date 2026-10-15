use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use map_core::{ObjectId, query::Ray};
use slint::ComponentHandle;

use crate::{
    PlacementWindow,
    runtime::{SharedSession, lock},
    session::{
        Actor, Session,
        placement::{Method, Options},
    },
};

#[derive(Clone)]
struct Context {
    ids: Vec<ObjectId>,
    generation: u64,
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
    if context.generation != session.document_generation() || context.ids != session.selected_ids()
    {
        return Err("Map or selection changed; reopen surface placement".into());
    }
    if session.surface_placement()["token"].as_u64() != token {
        return Err("Surface placement was replaced; reopen this window to review it".into());
    }
    Ok(context.ids.clone())
}

fn options(window: &PlacementWindow) -> Result<Options, String> {
    let offset = window
        .get_offset()
        .parse::<f64>()
        .map_err(|_| "Enter a finite surface offset")?;
    Ok(Options {
        align: window.get_align(),
        offset,
        texture_lock: window.get_texture_lock(),
    })
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
        "Click a visible surface in the viewport. Escape cancels picking.".into()
    } else {
        "Review the placement. Apply placement changes the map in one Undo step.".into()
    });
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = PlacementWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(560.0, 520.0));
        let context = Rc::new(RefCell::new(None));
        let token = Rc::new(Cell::new(None));
        let picking = Rc::new(Cell::new(false));
        let dirty = Rc::new(Cell::new(false));
        window.on_invalidate({
            let (window, dirty) = (window.as_weak(), dirty.clone());
            move || {
                dirty.set(true);
                let window = window.unwrap();
                window.set_ready(false);
                window.set_message("Preview changed settings before applying them.".into());
            }
        });
        window.on_preview({
            let (window, shared, context, token, picking, dirty) = (
                window.as_weak(),
                shared.clone(),
                context.clone(),
                token.clone(),
                picking.clone(),
                dirty.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    let ids = validate(&session, &context, token.get())?;
                    let options = options(&window)?;
                    if window.get_method() == 2 {
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
                        present(&window, &report);
                        if picking.get() {
                            let _ = window.hide();
                        }
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_apply({
            let (window, shared, token, context, dirty) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                context.clone(),
                dirty.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    if dirty.get() {
                        return Err("Preview changed settings before applying them".into());
                    }
                    let mut session = lock(&shared);
                    validate(&session, &context, token.get())?;
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
            }
        });
        window.on_frame({
            let (window, shared, token, context, dirty) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                context.clone(),
                dirty.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    validate(&session, &context, token.get())?;
                    if dirty.get() || session.surface_placement()["valid"] != true {
                        return Err("Preview placement before framing".into());
                    }
                    session.frame_selection(Actor::User, window.get_aspect())
                })();
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_cancel({
            let (window, shared, token, picking) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                picking.clone(),
            );
            move || {
                if let Some(token) = token.take() {
                    let _ = lock(&shared).cancel_surface_placement(token);
                }
                picking.set(false);
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_cancel();
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
        *self.context.borrow_mut() = Some(Context {
            ids: ids.clone(),
            generation: session.document_generation(),
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
        self.window.set_method(if self.picking.get() {
            2
        } else {
            match report["plan"]["method"].as_str() {
                Some("ceiling") => 1,
                Some("ray") => 2,
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use std::sync::{Arc, Mutex};

    #[test]
    fn surface_placement_controls_share_pick_review_apply_and_foreign_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, id) = crate::test_assets::placement();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared)).unwrap();
        controller.window.set_method(2);
        controller.window.set_align(true);
        controller.window.set_offset("4".into());
        controller.window.invoke_invalidate();
        controller.window.show().unwrap();
        controller.window.invoke_preview();
        assert!(controller.is_picking());
        assert!(!controller.window.window().is_visible());
        assert!(
            controller
                .pick_at_ray(
                    &mut lock(&shared),
                    Ray {
                        origin: DVec3::splat(1000.0),
                        direction: DVec3::Z
                    }
                )
                .is_err()
        );
        assert!(controller.is_picking());
        controller
            .pick_at_ray(
                &mut lock(&shared),
                Ray {
                    origin: DVec3::new(0.0, 0.0, 64.0),
                    direction: DVec3::Y,
                },
            )
            .unwrap();
        assert!(!controller.is_picking());
        assert!(controller.window.get_ready());
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "surface-placement");
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller.window.set_offset("8".into());
        controller.window.invoke_invalidate();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller.window.set_method(0);
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        controller.window.invoke_apply();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller.initialize(&mut lock(&shared)).unwrap();
        controller.window.set_method(0);
        controller.window.invoke_preview();
        let transform = lock(&shared).transform_preview().unwrap().transform;
        lock(&shared)
            .preview_transform(Actor::Agent, &[id], transform, Some(transform.pivot))
            .unwrap();
        let foreign = lock(&shared).render_document().vmf().clone();
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).render_document().vmf(), &foreign);
        lock(&shared).cancel_transform_preview();
    }
}
