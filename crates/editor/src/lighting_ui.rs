use std::cell::RefCell;
use std::rc::Rc;

use map_core::surface::{self, FaceId, SurfaceEdit};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{LightingGroup, LightingWindow};

#[derive(Clone)]
struct Target {
    faces: Vec<FaceId>,
    generation: u64,
    revision: u64,
    preview: Option<u64>,
    initial_scale: String,
}

impl Target {
    fn check(&self, session: &Session) -> Result<(), String> {
        if self.generation != session.document_generation()
            || self.revision != session.document().revision()
            || self.faces != session.selected_faces()
        {
            return Err("Map or face selection changed. Cancel and reopen face lighting.".into());
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
    pub window: LightingWindow,
    target: Rc<RefCell<Option<Target>>>,
}

fn groups(session: &Session, faces: &[FaceId]) -> ModelRc<LightingGroup> {
    let masks = faces
        .iter()
        .filter_map(|face| surface::side(session.document(), *face))
        .map(|side| side.smoothing_groups.unwrap_or(0))
        .collect::<Vec<_>>();
    let values: Vec<_> = (1..=32)
        .map(|number| {
            let bit = 1 << (number - 1);
            let count = masks.iter().filter(|mask| **mask & bit != 0).count();
            LightingGroup {
                number,
                state: if count == 0 {
                    0
                } else if count == masks.len() {
                    1
                } else {
                    -1
                },
                action: 0,
            }
        })
        .collect();
    ModelRc::new(VecModel::from(values))
}

fn patch(window: &LightingWindow, initial_scale: &str) -> Result<SurfaceEdit, String> {
    let value = window.get_lightmap_scale();
    let lightmap_scale = if value.trim().is_empty() || value == initial_scale {
        None
    } else {
        Some(
            value
                .trim()
                .parse::<u32>()
                .map_err(|_| "Enter a whole lightmap scale from 1 to 1024")?,
        )
    };
    let mut patch = SurfaceEdit {
        lightmap_scale,
        ..Default::default()
    };
    for group in window.get_groups().iter() {
        match group.action {
            1 => patch.smoothing_add |= 1 << (group.number - 1),
            2 => patch.smoothing_remove |= 1 << (group.number - 1),
            _ => {}
        }
    }
    Ok(patch)
}

fn apply(
    window: &LightingWindow,
    session: &mut Session,
    target: &mut Target,
    preview: bool,
) -> Result<usize, String> {
    target.check(session)?;
    if !preview
        && session.face_preview_token().is_some()
        && session.face_preview_token() != target.preview
    {
        return Err("Another face preview is active. Preview these fields before applying.".into());
    }
    if !preview && target.preview.is_some() {
        session.check_face_preview_token(target.preview)?;
        return session.commit_face_preview(Actor::User);
    }
    let count = session.edit_faces(
        Actor::User,
        &target.faces,
        patch(window, &target.initial_scale)?,
        false,
        preview,
    )?;
    target.preview = session.face_preview_token();
    Ok(count)
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = LightingWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_edited({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel_preview(&mut lock(&shared));
                }
                window
                    .unwrap()
                    .set_message("Fields changed. Preview or apply the new settings.".into());
            }
        });
        window.on_toggle_group({
            let window = window.as_weak();
            move |index, checked| {
                let window = window.unwrap();
                let groups = window.get_groups();
                if let Some(mut group) = groups.row_data(index as usize) {
                    group.state = i32::from(checked);
                    group.action = if checked { 1 } else { 2 };
                    groups.set_row_data(index as usize, group);
                    window.invoke_edited();
                }
            }
        });
        window.on_clear_groups({
            let window = window.as_weak();
            move || {
                let window = window.unwrap();
                let groups = window.get_groups();
                for index in 0..groups.row_count() {
                    if let Some(mut group) = groups.row_data(index) {
                        group.state = 0;
                        group.action = 2;
                        groups.set_row_data(index, group);
                    }
                }
                window.invoke_edited();
            }
        });
        window.on_reset_groups({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                if let Some(target) = target.borrow().as_ref() {
                    window.set_groups(groups(&lock(&shared), &target.faces));
                }
                window.invoke_edited();
            }
        });
        window.on_view_changed({
            let (window, shared) = (window.as_weak(), shared.clone());
            move || {
                let window = window.unwrap();
                let mode =
                    render::SurfaceView::from_index(window.get_view_mode()).unwrap_or_default();
                let result = lock(&shared).set_surface_view(mode, window.get_view_group() as u32);
                window.set_message(result.err().unwrap_or_default().into());
            }
        });
        window.on_preview_edit({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                if let Some(target) = target.borrow_mut().as_mut() {
                    let result = apply(&window, &mut lock(&shared), target, true);
                    window.set_message(
                        match result {
                            Ok(count) => format!(
                                "Preview: {count} faces change. Apply to keep these settings."
                            ),
                            Err(error) => error,
                        }
                        .into(),
                    );
                }
            }
        });
        window.on_apply({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                if let Some(target) = target.borrow_mut().as_mut() {
                    match apply(&window, &mut lock(&shared), target, false) {
                        Ok(_) => {
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
            return Err("Select faces before editing their lighting settings".into());
        }
        session.cancel_face_preview();
        self.window
            .set_selection_label(format!("Settings for {} selected faces", faces.len()).into());
        self.window
            .set_lightmap_scale(crate::ui::common_face_number(
                session.document(),
                &faces,
                |side| f64::from(side.lightmap_scale),
            ));
        self.window.set_groups(groups(session, &faces));
        *self.target.borrow_mut() = Some(Target {
            faces,
            generation: session.document_generation(),
            revision: session.document().revision(),
            preview: None,
            initial_scale: self.window.get_lightmap_scale().into(),
        });
        self.window
            .set_message("Preview changes, or Cancel to keep the current settings.".into());
        self.refresh(session);
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(target) = self.target.borrow().as_ref() {
            let stale = target.check(session).err();
            self.window.set_stale(stale.is_some());
            let foreign_preview = session.face_preview_token().is_some()
                && session.face_preview_token() != target.preview;
            self.window.set_can_apply(!foreign_preview);
            if let Some(error) = stale {
                self.window.set_message(error.into());
            } else if foreign_preview {
                self.window.set_message(
                    "Another face preview is active. Preview these fields before applying.".into(),
                );
            }
        }
        self.window.set_view_mode(session.view().surface.index());
        self.window
            .set_view_group(session.view().smoothing_group as i32);
    }
}
