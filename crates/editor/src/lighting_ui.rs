use std::cell::RefCell;
use std::rc::Rc;

use map_core::surface::{self, FaceId, SurfaceEdit};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, Session};
use crate::{LightingGroup, LightingWindow};

#[cfg(test)]
use crate::runtime::SharedSession;

#[cfg(test)]
mod admission_tests;

#[derive(Clone)]
struct Target {
    context: crate::face_edit_ui::Context,
    preview: Option<u64>,
    initial_scale: String,
}

impl Target {
    fn check(&self, session: &Session) -> Result<(), String> {
        self.context.check(
            session,
            "Map or face selection changed. Cancel and reopen face lighting.",
        )
    }

    fn cancel_preview(&mut self, session: &mut Session) {
        crate::face_edit_ui::cancel_preview(session, &mut self.preview);
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
    if let Some(changed) = crate::face_edit_ui::commit_preview(
        session,
        target.preview,
        preview,
        "Another face preview is active. Preview these fields before applying.",
    )? {
        return Ok(changed);
    }
    let count = session.edit_faces(
        Actor::User,
        &target.context.faces,
        patch(window, &target.initial_scale)?,
        false,
        preview,
    )?;
    target.preview = session.face_preview_token();
    Ok(count)
}

fn edited(window: &LightingWindow, session: &mut Session, target: &RefCell<Option<Target>>) {
    if let Some(target) = target.borrow_mut().as_mut() {
        target.cancel_preview(session);
    }
    window.set_message("Fields changed. Preview or apply the new settings.".into());
}

fn callback(
    window: &slint::Weak<LightingWindow>,
    continuation: &DocumentContinuation,
    body: impl FnOnce(LightingWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = continuation.upgrade().and_then(|access| {
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
    #[cfg(test)]
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let continuation = access.downgrade();
        let window = LightingWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_edited({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    edited(&window, session, &target)
                })
            }
        });
        window.on_toggle_group({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move |index, checked| {
                callback(&window, &continuation, |window, session| {
                    let groups = window.get_groups();
                    if let Some(mut group) = groups.row_data(index as usize) {
                        group.state = i32::from(checked);
                        group.action = if checked { 1 } else { 2 };
                        groups.set_row_data(index as usize, group);
                        edited(&window, session, &target);
                    }
                })
            }
        });
        window.on_clear_groups({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let groups = window.get_groups();
                    for index in 0..groups.row_count() {
                        if let Some(mut group) = groups.row_data(index) {
                            group.state = 0;
                            group.action = 2;
                            groups.set_row_data(index, group);
                        }
                    }
                    edited(&window, session, &target);
                })
            }
        });
        window.on_reset_groups({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    if let Some(target) = target.borrow().as_ref() {
                        window.set_groups(groups(session, &target.context.faces));
                    }
                    edited(&window, session, &target);
                })
            }
        });
        window.on_view_changed({
            let (window, continuation) = (window.as_weak(), continuation.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let mode =
                        render::SurfaceView::from_index(window.get_view_mode()).unwrap_or_default();
                    let result = session.set_surface_view(mode, window.get_view_group() as u32);
                    window.set_message(result.err().unwrap_or_default().into());
                })
            }
        });
        window.on_preview_edit({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        let result = apply(&window, session, target, true);
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
                })
            }
        });
        window.on_apply({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        match apply(&window, session, target, false) {
                            Ok(_) => {
                                let _ = window.hide();
                            }
                            Err(error) => window.set_message(error.into()),
                        }
                    }
                })
            }
        });
        window.on_cancel({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |_, session| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        target.cancel_preview(session);
                    }
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
            context: crate::face_edit_ui::Context::capture(session, faces),
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
            let foreign_preview = crate::face_edit_ui::foreign_preview(session, target.preview);
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
