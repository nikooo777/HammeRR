use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec2;
use map_core::surface;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, GroupProjection, Session};
use crate::{SurfaceGroupWindow, ui::preview_image};

struct Target {
    context: crate::face_edit_ui::Context,
    preview: Option<u64>,
}

impl Target {
    fn check(&self, session: &Session) -> Result<(), String> {
        self.context.check(
            session,
            "Map or face selection changed. Cancel and reopen texture mapping.",
        )
    }

    fn cancel_preview(&mut self, session: &mut Session) {
        crate::face_edit_ui::cancel_preview(session, &mut self.preview);
    }
}

pub struct Controller {
    pub window: SurfaceGroupWindow,
    target: Rc<RefCell<Option<Target>>>,
}

fn refresh_material(window: &SurfaceGroupWindow, session: &Session) {
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
    if let Some(changed) = crate::face_edit_ui::commit_preview(
        session,
        target.preview,
        preview,
        "Another face preview is active. Preview this mapping before applying.",
    )? {
        return Ok(changed);
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
                .context
                .faces
                .get(window.get_reference_index().max(0) as usize)
                .ok_or("Choose a reference face")?,
        ))
    };
    let material = window.get_material();
    let count = session.project_face_group(
        Actor::User,
        &target.context.faces,
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
            move || refresh_material(&window.unwrap(), &lock(&shared))
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
                refresh_material(&window, &session);
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
            context: crate::face_edit_ui::Context::capture(session, faces),
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

    pub fn refresh_material(&self, session: &Session) {
        refresh_material(&self.window, session);
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(target) = self.target.borrow().as_ref() {
            let stale = target.check(session).err();
            self.window.set_stale(stale.is_some());
            let foreign = crate::face_edit_ui::foreign_preview(session, target.preview);
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
