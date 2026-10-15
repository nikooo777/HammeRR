use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec2;
use map_core::surface::{self, FaceId};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, GroupProjection, Session};
use crate::{SurfaceGroupWindow, ui::preview_image};

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
            return Err("Map or face selection changed. Cancel and reopen texture mapping.".into());
        }
        Ok(())
    }

    fn cancel_preview(&mut self, session: &mut Session) {
        if self.preview.is_some() && session.face_preview_token() == self.preview {
            session.cancel_face_preview();
        }
        self.preview = None;
    }
}

pub struct Controller {
    pub window: SurfaceGroupWindow,
    target: Rc<RefCell<Option<Target>>>,
}

fn refresh_material(window: &SurfaceGroupWindow, session: &mut Session) {
    window.set_preview_material(window.get_material());
    window.set_material_preview(
        session
            .material_preview(&window.get_material())
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
}

fn apply(
    window: &SurfaceGroupWindow,
    session: &mut Session,
    target: &mut Target,
    preview: bool,
) -> Result<usize, String> {
    target.check(session)?;
    if !preview
        && session.face_preview_token().is_some()
        && session.face_preview_token() != target.preview
    {
        return Err("Another face preview is active. Preview this mapping before applying.".into());
    }
    if !preview && target.preview.is_some() {
        session.check_face_preview_token(target.preview)?;
        return session.commit_face_preview(Actor::User);
    }
    let repeats = DVec2::new(
        window
            .get_repeats_u()
            .trim()
            .parse()
            .map_err(|_| "Enter a horizontal tile count")?,
        window
            .get_repeats_v()
            .trim()
            .parse()
            .map_err(|_| "Enter a vertical tile count")?,
    );
    let projection = if window.get_projection() == 1 {
        GroupProjection::View
    } else {
        GroupProjection::Face(Some(
            *target
                .faces
                .get(window.get_reference_index().max(0) as usize)
                .ok_or("Choose a reference face")?,
        ))
    };
    let material = window.get_material();
    let count = session.project_face_group(
        Actor::User,
        &target.faces,
        projection,
        (!material.trim().is_empty()).then_some(material.as_str()),
        repeats,
        preview,
    )?;
    target.preview = session.face_preview_token();
    Ok(count)
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = SurfaceGroupWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_edited({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel_preview(&mut lock(&shared));
                }
                window
                    .unwrap()
                    .set_message("Preview or apply the new mapping.".into());
            }
        });
        window.on_load_material({
            let (window, shared) = (window.as_weak(), shared.clone());
            move || refresh_material(&window.unwrap(), &mut lock(&shared))
        });
        for preview in [true, false] {
            let (weak, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            let action = move || {
                let window = weak.unwrap();
                let mut session = lock(&shared);
                if let Some(target) = target.borrow_mut().as_mut() {
                    match apply(&window, &mut session, target, preview) {
                        Ok(count) => {
                            window.set_message(format!("Preview: {count} faces share one texture mapping. Apply to keep it.").into());
                            if !preview {
                                let _ = window.hide();
                            }
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                }
                refresh_material(&window, &mut session);
            };
            if preview {
                window.on_preview_edit(action);
            } else {
                window.on_apply(action);
            }
        }
        window.on_cancel({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel_preview(&mut lock(&shared));
                }
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let (shared, target) = (shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel_preview(&mut lock(&shared));
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, target })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        let faces = session.selected_faces();
        if faces.is_empty() {
            return Err("Select faces before mapping a texture across them".into());
        }
        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.cancel_preview(session);
        }
        let names: std::collections::BTreeSet<_> = faces
            .iter()
            .filter_map(|face| surface::side(session.document(), *face))
            .map(|side| side.material.to_ascii_lowercase())
            .collect();
        self.window.set_material(if names.len() == 1 {
            names.first().unwrap().clone().into()
        } else {
            "".into()
        });
        self.window.set_references(ModelRc::new(VecModel::from(
            faces
                .iter()
                .map(|face| format!("Brush {} · face {}", face.solid, face.side).into())
                .collect::<Vec<_>>(),
        )));
        self.window.set_selection_label(
            format!("Texture mapping for {} selected faces", faces.len()).into(),
        );
        self.window.set_reference_index(0);
        self.window.set_projection(0);
        self.window.set_repeats_u("1".into());
        self.window.set_repeats_v("1".into());
        self.window
            .set_message("Preview the shared mapping before applying.".into());
        *self.target.borrow_mut() = Some(Target {
            faces,
            generation: session.document_generation(),
            revision: session.document().revision(),
            preview: None,
        });
        refresh_material(&self.window, session);
        self.refresh(session);
        Ok(())
    }

    pub fn choose_material(&self, session: &mut Session, material: &str) {
        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.cancel_preview(session);
        }
        self.window.set_material(material.into());
        refresh_material(&self.window, session);
        self.refresh(session);
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(target) = self.target.borrow().as_ref() {
            let stale = target.check(session).err();
            self.window.set_stale(stale.is_some());
            let foreign = session.face_preview_token().is_some()
                && session.face_preview_token() != target.preview;
            self.window.set_can_apply(!foreign);
            if let Some(error) = stale {
                self.window.set_message(error.into());
            } else if foreign {
                self.window.set_message(
                    "Another face preview is active. Preview this mapping before applying.".into(),
                );
            }
        }
    }
}
