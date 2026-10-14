use std::cell::{Cell, RefCell};
use std::rc::Rc;

use map_core::surface::FaceId;
use slint::ComponentHandle;

use crate::TerrainWindow;
use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};

struct Target {
    faces: Vec<FaceId>,
    generation: u64,
    revision: u64,
    preview: Option<u64>,
}

impl Target {
    fn check(&self, session: &Session) -> Result<(), String> {
        if self.generation != session.document_generation()
            || self.revision != session.document().revision()
            || self.faces != session.selected_faces()
        {
            return Err("Map or face selection changed. Cancel and reopen terrain editing.".into());
        }
        Ok(())
    }

    fn cancel(&mut self, session: &mut Session) {
        if self.preview.is_some() && self.preview == session.face_preview_token() {
            session.cancel_face_preview();
        }
        self.preview = None;
    }
}

pub struct Controller {
    pub window: TerrainWindow,
    target: Rc<RefCell<Option<Target>>>,
    picking: Rc<Cell<bool>>,
}

fn number(value: slint::SharedString, label: &str) -> Result<f64, String> {
    let value = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Enter a number for {label}"))?;
    if !value.is_finite() {
        return Err(format!("{label} must be finite"));
    }
    Ok(value)
}

fn operation(window: &TerrainWindow) -> Result<map_core::displacement::Operation, String> {
    use map_core::displacement::{
        Operation,
        brush::{Axis, Blend, Brush, Sculpt},
    };
    let mode = window.get_operation_index();
    if mode == 10 {
        let angle = match window.get_smoothing_index() {
            0 => None,
            1 => Some(0.0),
            2 => Some(number(
                window.get_smoothing_angle(),
                "border smoothing angle",
            )?),
            _ => return Err("Choose a valid border smoothing mode".into()),
        };
        return Ok(Operation::Smoothing(angle));
    }
    if mode == 8 {
        let choice = |index| match index {
            0 => Ok(None),
            1 => Ok(Some(true)),
            2 => Ok(Some(false)),
            _ => Err("Choose a valid collision setting".to_owned()),
        };
        return Ok(Operation::Collision(
            map_core::displacement::collision::Settings {
                physics: choice(window.get_physics_index())?,
                player: choice(window.get_player_index())?,
                ray: choice(window.get_ray_index())?,
            },
        ));
    }
    if mode == 0 {
        let index = u32::try_from(window.get_grid_index())
            .ok()
            .filter(|index| *index <= 2)
            .ok_or("Choose a valid terrain grid")?;
        return Ok(Operation::Resolution(index + 2));
    }
    let brush = if (1..=5).contains(&mode) {
        Brush {
            center: glam::DVec3::new(
                number(window.get_center_x(), "center X")?,
                number(window.get_center_y(), "center Y")?,
                number(window.get_center_z(), "center Z")?,
            ),
            radius: number(window.get_radius(), "radius")?,
            axis: match window.get_direction_index() {
                0 => Axis::WorldZ,
                1 => Axis::FaceNormal,
                _ => return Err("Choose a valid brush direction".into()),
            },
        }
    } else {
        Brush {
            center: glam::DVec3::ZERO,
            radius: 128.0,
            axis: Axis::WorldZ,
        }
    };
    Ok(match mode {
        1 => Operation::Sculpt {
            brush,
            operation: Sculpt::Raise(number(window.get_amount(), "height change")?),
        },
        2 => Operation::Sculpt {
            brush,
            operation: Sculpt::Flatten(number(window.get_strength(), "strength")?),
        },
        3 => Operation::Sculpt {
            brush,
            operation: Sculpt::Smooth(number(window.get_strength(), "strength")?),
        },
        4 => Operation::Sculpt {
            brush,
            operation: Sculpt::Noise {
                amplitude: number(window.get_amount(), "noise amount")?,
                seed: window
                    .get_seed()
                    .trim()
                    .parse()
                    .map_err(|_| "Enter a whole noise seed from 0 to 4294967295")?,
            },
        },
        5 => Operation::Blend {
            brush,
            operation: Blend::Paint {
                value: number(window.get_blend_value(), "blend value")?,
                strength: number(window.get_strength(), "strength")?,
            },
        },
        6 => Operation::Blend {
            brush,
            operation: Blend::Fill(number(window.get_blend_value(), "blend value")?),
        },
        7 => Operation::Blend {
            brush,
            operation: Blend::Invert,
        },
        _ => return Err("Choose a valid terrain operation".into()),
    })
}

fn sync_sliders(window: &TerrainWindow) {
    for (value, setter) in [
        (
            window.get_radius(),
            TerrainWindow::set_radius_slider as fn(&TerrainWindow, f32),
        ),
        (window.get_amount(), TerrainWindow::set_amount_slider),
        (window.get_strength(), TerrainWindow::set_strength_slider),
        (window.get_blend_value(), TerrainWindow::set_blend_slider),
        (
            window.get_smoothing_angle(),
            TerrainWindow::set_smoothing_slider,
        ),
    ] {
        if let Ok(value) = value.trim().parse::<f32>()
            && value.is_finite()
        {
            setter(window, value);
        }
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = TerrainWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        let picking = Rc::new(Cell::new(false));
        window.on_pick_center({
            let (window, shared, target, picking) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                picking.clone(),
            );
            move || {
                let window = window.unwrap();
                let session = lock(&shared);
                let result = target
                    .borrow()
                    .as_ref()
                    .ok_or("Reopen terrain editing".to_owned())
                    .and_then(|target| target.check(&session));
                match result {
                    Ok(()) => {
                        picking.set(true);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_edited({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                let window = window.unwrap();
                sync_sliders(&window);
                window.set_ready(false);
                window.set_message("Preview the edited terrain before applying.".into());
            }
        });
        window.on_preview_edit({
            let (window,shared,target)=(window.as_weak(),shared.clone(),target.clone());
            move || {
                let window=window.unwrap();
                let mut session=lock(&shared);
                if let Some(target)=target.borrow_mut().as_mut() {
                    let result=target.check(&session).and_then(|_|{
                        if window.get_operation_index()==9 {session.preview_terrain_sew(Actor::User,&target.faces,window.get_sew_blend()).map(|result|result.summary())} else {session.preview_terrain_edit(Actor::User,&target.faces,operation(&window)?).map(|count|format!("Preview changes {count} terrain surfaces. Apply to keep this edit."))}
                    });
                    match result {
                        Ok(message)=>{target.preview=session.face_preview_token();window.set_ready(true);window.set_message(message.into());}
                        Err(error)=>window.set_message(error.into()),
                    }
                }
            }
        });
        window.on_apply({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                if let Some(target) = target.borrow_mut().as_mut() {
                    let result = target.check(&session).and_then(|_| {
                        let token = target.preview.ok_or("Preview terrain before applying")?;
                        session.check_face_preview_token(Some(token))?;
                        session.commit_face_preview(Actor::User)
                    });
                    match result {
                        Ok(_) => {
                            target.preview = None;
                            window.set_ready(false);
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                }
            }
        });
        window.on_cancel({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let (shared, target) = (shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            target,
            picking,
        })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        self.picking.set(false);
        let faces = session.selected_faces();
        let grids = session.terrain_grids(&faces, false)?;
        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.cancel(session);
        }
        let center = grids
            .iter()
            .flat_map(|grid| grid.mesh.positions.iter())
            .copied()
            .fold(glam::DVec3::ZERO, |sum, p| sum + p)
            / grids
                .iter()
                .map(|grid| grid.mesh.positions.len())
                .sum::<usize>() as f64;
        self.window.set_center_x(format!("{:.3}", center.x).into());
        self.window.set_center_y(format!("{:.3}", center.y).into());
        self.window.set_center_z(format!("{:.3}", center.z).into());
        self.window.set_operation_index(0);
        sync_sliders(&self.window);
        let current = |field: fn(&map_core::displacement::edit::TerrainGrid) -> bool| {
            let first = field(&grids[0]);
            if grids.iter().all(|grid| field(grid) == first) {
                if first { "enabled" } else { "disabled" }
            } else {
                "mixed"
            }
        };
        self.window
            .set_physics_current(current(|grid| grid.collision.physics).into());
        self.window
            .set_player_current(current(|grid| grid.collision.player).into());
        self.window
            .set_ray_current(current(|grid| grid.collision.ray).into());
        self.window.set_physics_index(0);
        self.window.set_player_index(0);
        self.window.set_ray_index(0);
        let angle = grids[0].smoothing_angle;
        let smoothing_common = grids
            .iter()
            .all(|grid| grid.smoothing_angle == angle && grid.smoothing_error.is_none());
        let smoothing_label = if grids.iter().any(|grid| grid.smoothing_error.is_some()) {
            "invalid · choose a setting to repair".to_owned()
        } else if !smoothing_common {
            "mixed".to_owned()
        } else {
            match angle {
                None => "automatic".into(),
                Some(0.0) => "hard borders".into(),
                Some(angle) => format!("{angle}° angle limit"),
            }
        };
        self.window.set_smoothing_current(smoothing_label.into());
        self.window.set_smoothing_index(if smoothing_common {
            match angle {
                None => 0,
                Some(0.0) => 1,
                Some(_) => 2,
            }
        } else {
            0
        });
        self.window.set_smoothing_angle(
            angle
                .filter(|angle| *angle > 0.0)
                .unwrap_or(45.0)
                .to_string()
                .into(),
        );
        sync_sliders(&self.window);
        let power = grids[0].power;
        let common = grids.iter().all(|grid| grid.power == power);
        self.window
            .set_grid_index(if common { power as i32 - 2 } else { 0 });
        self.window.set_selection_label(
            format!(
                "{} terrain surface{} · {}",
                grids.len(),
                if grids.len() == 1 { "" } else { "s" },
                if common {
                    format!(
                        "current grid {} × {}",
                        grids[0].mesh.size, grids[0].mesh.size
                    )
                } else {
                    "mixed grid sizes".into()
                }
            )
            .into(),
        );
        *self.target.borrow_mut() = Some(Target {
            faces,
            generation: session.document_generation(),
            revision: session.document().revision(),
            preview: None,
        });
        self.window.set_message(
            "Choose an operation and preview the result. Cancel leaves the map unchanged.".into(),
        );
        self.refresh(session);
        Ok(())
    }

    pub fn is_picking(&self) -> bool {
        self.picking.get()
    }

    pub fn cancel_center_pick(&self) {
        self.picking.set(false);
        self.window
            .set_message("Center picking cancelled. Terrain fields are unchanged.".into());
        let _ = self.window.show();
    }

    pub fn select_center(
        &self,
        session: &mut Session,
        ray: &map_core::query::Ray,
    ) -> Result<glam::DVec3, String> {
        if !self.is_picking() {
            return Err("Center picking is not active".into());
        }
        let mut target = self.target.borrow_mut();
        let target = target.as_mut().ok_or("Reopen terrain editing")?;
        if let Err(error) = target.check(session) {
            self.picking.set(false);
            self.window.set_stale(true);
            return Err(error);
        }
        let hit = session
            .pick(ray)
            .ok_or("Click a selected terrain surface, or cancel picking")?;
        let face = map_core::surface::FaceId {
            solid: hit.solid.ok_or("Click a selected terrain surface")?,
            side: hit.side.ok_or("Click a selected terrain surface")?,
        };
        if !target.faces.contains(&face) {
            return Err("Click one of the selected terrain surfaces, or cancel picking".into());
        }
        target.cancel(session);
        self.window.set_center_x(hit.point.x.to_string().into());
        self.window.set_center_y(hit.point.y.to_string().into());
        self.window.set_center_z(hit.point.z.to_string().into());
        self.window.set_ready(false);
        self.window
            .set_message("Brush center selected. Preview the new edit before applying.".into());
        self.picking.set(false);
        Ok(hit.point)
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(target) = self.target.borrow().as_ref() {
            let stale = target.check(session).err();
            self.window.set_stale(stale.is_some());
            self.window.set_ready(
                stale.is_none()
                    && target.preview.is_some()
                    && target.preview == session.face_preview_token(),
            );
            if let Some(error) = stale {
                self.window.set_message(error.into());
            } else if session.face_preview_token().is_some()
                && target.preview != session.face_preview_token()
            {
                self.window.set_message(
                    "Another face preview is active. Preview these settings to replace it.".into(),
                );
            }
        }
    }
}
