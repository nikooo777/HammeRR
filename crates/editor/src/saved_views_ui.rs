use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::saved_views::Guard;
use crate::session::{Actor, Session};
use crate::{SavedViewRow, SavedViewsWindow};

#[cfg(test)]
use crate::runtime::{SharedSession, lock};

#[derive(Default)]
struct Form {
    guard: Option<Guard>,
    selected: Option<u32>,
}

pub struct Controller {
    pub window: SavedViewsWindow,
    form: Rc<RefCell<Form>>,
}

fn refresh(window: &SavedViewsWindow, session: &Session, form: &RefCell<Form>) {
    let mut form = form.borrow_mut();
    let guard = session.camera_view_guard();
    if form.guard != Some(guard) {
        if form.guard.is_some() {
            window.set_message("Map changed. Select a view or enter a new name.".into());
        }
        form.guard = Some(guard);
        form.selected = None;
        window.set_selected_id("".into());
        window.set_view_name("".into());
        match session.camera_views() {
            Ok(views) => {
                window.set_editable(true);
                window.set_views(
                    Rc::new(VecModel::from(
                        views
                            .views
                            .iter()
                            .map(|view| SavedViewRow {
                                id: view.id.to_string().into(),
                                name: view.name.clone().into(),
                                projection: view.projection.clone().into(),
                            })
                            .collect::<Vec<_>>(),
                    ))
                    .into(),
                );
            }
            Err(error) => {
                window.set_editable(false);
                window.set_views(Default::default());
                window.set_message(error.into());
            }
        }
    }
    let camera = session.camera();
    window.set_current(
        format!(
            "Current camera: {}{}",
            camera.projection.name(),
            if camera.projection == render::Projection::Perspective {
                format!(" · {:.1}° field of view", camera.fov_y_degrees)
            } else {
                format!(" · {:.1} units tall", camera.ortho_height)
            }
        )
        .into(),
    );
}

fn choose(window: &SavedViewsWindow, session: &Session, form: &RefCell<Form>, id: u32) {
    refresh(window, session, form);
    match session.camera_views().and_then(|views| {
        views
            .views
            .into_iter()
            .find(|view| view.id == id)
            .ok_or_else(|| "Saved view no longer exists".into())
    }) {
        Ok(view) => {
            form.borrow_mut().selected = Some(id);
            window.set_selected_id(id.to_string().into());
            window.set_view_name(view.name.into());
            window.set_message("Go to view moves the camera. Replace stores your current camera in the selected view.".into());
        }
        Err(error) => window.set_message(error.into()),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Save,
    Replace,
    Rename,
    Restore,
    Delete,
}

fn apply(window: &SavedViewsWindow, session: &mut Session, form: &RefCell<Form>, action: Action) {
    let (guard, selected) = {
        let form = form.borrow();
        (form.guard, form.selected)
    };
    let name = window.get_view_name();
    let result = match action {
        Action::Save => session
            .save_camera_view(Actor::User, &name, None, guard)
            .map(Some),
        _ => selected
            .ok_or_else(|| "Select a saved view first".to_string())
            .and_then(|id| match action {
                Action::Replace => session
                    .save_camera_view(Actor::User, &name, Some(id), guard)
                    .map(Some),
                Action::Rename => session
                    .rename_camera_view(Actor::User, id, &name, guard)
                    .map(|()| Some(id)),
                Action::Restore => session
                    .restore_camera_view(Actor::User, id, guard)
                    .map(|()| Some(id)),
                Action::Delete => session
                    .delete_camera_view(Actor::User, id, guard)
                    .map(|()| None),
                Action::Save => unreachable!(),
            }),
    };
    match result {
        Ok(selected) => {
            refresh(window, session, form);
            if let Some(id) = selected {
                choose(window, session, form, id);
            }
            window.set_message(
                match action {
                    Action::Save => "View saved. Save the VMF to keep it.",
                    Action::Replace => "Saved view updated to the current camera.",
                    Action::Rename => "View renamed.",
                    Action::Restore => "Camera restored. Map and selection unchanged.",
                    Action::Delete => "View deleted. Undo restores it.",
                }
                .into(),
            );
        }
        Err(error) => window.set_message(error.into()),
    }
}

fn callback(
    window: &slint::Weak<SavedViewsWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(SavedViewsWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
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
        let window = SavedViewsWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_choose({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |id| {
                callback(&window, &target, |window, session| {
                    if let Ok(id) = id.parse() {
                        choose(&window, session, &form, id);
                    }
                });
            }
        });
        let action_callback = |action| {
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &target, |window, session| {
                    apply(&window, session, &form, action)
                })
            }
        };
        window.on_save_new(action_callback(Action::Save));
        window.on_replace_view(action_callback(Action::Replace));
        window.on_rename_view(action_callback(Action::Rename));
        window.on_restore_view(action_callback(Action::Restore));
        window.on_delete_view(action_callback(Action::Delete));
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        Ok(Self { window, form })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        *self.form.borrow_mut() = Form::default();
        self.window
            .set_message("Choose a saved view, or name and save your current camera.".into());
        self.refresh(session);
        self.window.show()
    }
}

#[cfg(test)]
mod admission_tests;
