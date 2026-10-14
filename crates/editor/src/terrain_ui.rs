use std::cell::RefCell;
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
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = TerrainWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_edited({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                let window = window.unwrap();
                window.set_ready(false);
                window.set_message("Preview the new grid before applying.".into());
            }
        });
        window.on_preview_edit({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window=window.unwrap();
                let mut session=lock(&shared);
                if let Some(target)=target.borrow_mut().as_mut() {
                    let result=target.check(&session).and_then(|_|{
                        let index=u32::try_from(window.get_grid_index()).ok().filter(|index|*index<=2).ok_or("Choose a valid terrain grid")?;
                        session.preview_terrain_resolution(Actor::User,&target.faces,index+2)
                    });
                    match result {
                        Ok(count) => {
                            target.preview=session.face_preview_token();
                            window.set_ready(true);
                            window.set_message(format!("Preview changes {count} terrain surfaces. Apply to keep this grid.").into());
                        }
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
                        let token = target.preview.ok_or("Preview the grid before applying")?;
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
        Ok(Self { window, target })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        let faces = session.selected_faces();
        let grids = session.terrain_grids(&faces, false)?;
        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.cancel(session);
        }
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
            "Choose a grid and preview the result. Cancel leaves the map unchanged.".into(),
        );
        self.refresh(session);
        Ok(())
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
                    "Another face preview is active. Preview this grid to replace it.".into(),
                );
            }
        }
    }
}
