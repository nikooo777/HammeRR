use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

#[cfg(test)]
use crate::runtime::SharedSession;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::material_search::{Query, Scope};
use crate::session::{Actor, SelectMode, Session};
use crate::{MaterialReplaceWindow, ui::preview_image};

#[derive(Default)]
struct Target {
    report: Option<u64>,
    preview: Option<u64>,
}

impl Target {
    fn cancel_preview(&mut self, session: &mut Session) {
        crate::face_edit_ui::cancel_preview(session, &mut self.preview);
    }
}

pub struct Controller {
    pub window: MaterialReplaceWindow,
    target: Rc<RefCell<Target>>,
}

fn query(window: &MaterialReplaceWindow) -> Query {
    Query {
        material: window.get_source().into(),
        scope: match window.get_scope() {
            1 => Scope::SelectedObjects,
            2 => Scope::SelectedFaces,
            _ => Scope::Map,
        },
        include_hidden: window.get_include_hidden(),
    }
}

fn refresh_images(window: &MaterialReplaceWindow, session: &Session) {
    for (replacement, material) in [
        (false, window.get_source()),
        (true, window.get_replacement()),
    ] {
        let image = session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default();
        if replacement {
            window.set_replacement_preview(image);
        } else {
            window.set_source_preview(image);
        }
    }
}

fn refresh(window: &MaterialReplaceWindow, session: &Session, target: &Target) {
    let report = session
        .material_search_report()
        .filter(|report| Some(report.token) == target.report);
    let ready =
        report.is_some_and(|report| !report.faces.is_empty()) && !session.material_search_stale();
    window.set_ready(ready);
    window.set_can_apply(
        session.face_preview_token().is_none() || session.face_preview_token() == target.preview,
    );
    if let Some(report) = report {
        window.set_summary(
            format!(
                "{} matching faces · {} hidden included · {} hidden skipped",
                report.faces.len(),
                report.hidden_matches,
                report.hidden_excluded
            )
            .into(),
        );
        if session.material_search_stale() {
            window.set_message("Map or search selection changed. Find faces again.".into());
        }
    } else if target.report.is_some() {
        window.set_message("Another search replaced these results. Find faces again.".into());
    }
}

fn with_session<R>(
    window: &slint::Weak<MaterialReplaceWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&MaterialReplaceWindow, &mut Session) -> R,
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
    #[cfg(test)]
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = MaterialReplaceWindow::new()?;
        let target = Rc::new(RefCell::new(Target::default()));
        window.on_search_edited({
            let (window, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut target = target.borrow_mut();
                    target.cancel_preview(session);
                    target.report = None;
                    window.set_ready(false);
                    window.set_summary("Search changed. Find faces to refresh the results.".into());
                    window.set_message("".into());
                });
            }
        });
        window.on_replacement_edited({
            let (window, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut target = target.borrow_mut();
                    target.cancel_preview(session);
                    window.set_message("Preview or apply the new replacement.".into());
                    refresh(window, session, &target);
                });
            }
        });
        window.on_find({
            let (window, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut target = target.borrow_mut();
                    target.cancel_preview(session);
                    target.report = None;
                    match session.find_material_faces(query(window)) {
                        Ok(token) => {
                            target.report = Some(token);
                            window.set_message(
                                "Review the match count, then choose a replacement.".into(),
                            );
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh_images(window, session);
                    refresh(window, session, &target);
                });
            }
        });
        window.on_select_matches({
            let (window, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut target = target.borrow_mut();
                    target.cancel_preview(session);
                    if let Some(token) = target.report {
                        match session.select_material_faces(token, SelectMode::Replace) {
                            Ok(count) => window.set_message(
                                format!(
                                    "Selected {count} matching faces. Hidden faces stay hidden."
                                )
                                .into(),
                            ),
                            Err(error) => window.set_message(error.into()),
                        }
                    }
                    refresh(window, session, &target);
                });
            }
        });
        for preview in [true, false] {
            let (weak, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            let action = move || {
                let hide = with_session(&weak, &access, |window, session| {
                    let mut target = target.borrow_mut();
                    let result = (|| {
                        let token = target.report.ok_or("Find material faces first")?;
                        if !preview
                            && session.face_preview_token().is_some()
                            && session.face_preview_token() != target.preview
                        {
                            return Err("Another face preview is active. Preview this replacement before applying.".into());
                        }
                        if !preview && target.preview.is_some() {
                            if session.material_search_stale()
                                || session.material_search_report().map(|report| report.token)
                                    != Some(token)
                            {
                                return Err("Material results changed. Find faces again.".into());
                            }
                            session.check_face_preview_token(target.preview)?;
                            session.commit_face_preview(Actor::User)
                        } else {
                            session.replace_materials(
                                Actor::User,
                                token,
                                &window.get_replacement(),
                                preview,
                            )
                        }
                    })();
                    let hide = match result {
                        Ok(count) => {
                            target.preview = session.face_preview_token();
                            window.set_message(format!("Preview: {count} faces change. Replace faces to keep the result.").into());
                            !preview
                        }
                        Err(error) => {
                            window.set_message(error.into());
                            false
                        }
                    };
                    refresh_images(window, session);
                    refresh(window, session, &target);
                    hide
                });
                if hide == Some(true)
                    && let Some(window) = weak.upgrade()
                {
                    let _ = window.hide();
                }
            };
            if preview {
                window.on_preview_edit(action);
            } else {
                window.on_apply(action);
            }
        }
        window.on_cancel({
            let (window, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            move || {
                let Some(owner) = window.upgrade() else {
                    return;
                };
                with_session(&window, &access, |_, session| {
                    target.borrow_mut().cancel_preview(session)
                });
                let _ = owner.hide();
            }
        });
        window.window().on_close_requested({
            let (window, access, target) = (window.as_weak(), access.downgrade(), target.clone());
            move || {
                with_session(&window, &access, |_, session| {
                    target.borrow_mut().cancel_preview(session)
                });
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, target })
    }

    pub fn initialize(&self, session: &mut Session, material: &str) {
        let mut target = self.target.borrow_mut();
        target.cancel_preview(session);
        target.report = None;
        self.window.set_source(material.into());
        self.window.set_replacement("".into());
        self.window.set_scope(0);
        self.window.set_include_hidden(false);
        self.window
            .set_summary("Choose a material and find its faces.".into());
        self.window.set_message("".into());
        refresh_images(&self.window, session);
        refresh(&self.window, session, &target);
    }

    pub fn choose_material(&self, session: &mut Session, replacement: bool, material: &str) {
        let mut target = self.target.borrow_mut();
        target.cancel_preview(session);
        if replacement {
            self.window.set_replacement(material.into());
        } else {
            self.window.set_source(material.into());
            target.report = None;
            self.window
                .set_summary("Source changed. Find faces to refresh the results.".into());
        }
        refresh_images(&self.window, session);
        refresh(&self.window, session, &target);
    }

    pub fn refresh_materials(&self, session: &Session) {
        refresh_images(&self.window, session);
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.target.borrow());
    }
}

#[cfg(test)]
mod tests;
