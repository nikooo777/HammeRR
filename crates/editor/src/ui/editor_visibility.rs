use crate::AppWindow;
use crate::runtime::DocumentAccess;
use crate::session::{Actor, Session, editor_visibility::Action};

pub(super) fn refresh(app: &AppWindow, session: &Session) {
    app.set_editor_hidden_count(session.editor_hidden_count().min(i32::MAX as usize) as i32);
    app.set_can_hide_editor_selection(session.can_hide_selected_in_editor());
}

pub(super) fn bind(app: &AppWindow, access: &DocumentAccess) {
    app.on_hide_editor_selection({
        let callback = super::admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let result = session.set_editor_visibility(
                    Actor::User,
                    Action::Hide(session.selected_ids()),
                    &session.working_region_stamp(),
                );
                if let Err(error) = result {
                    app.set_message(error.into());
                }
                refresh(&app, session);
            });
        }
    });
    app.on_show_all_editor_hidden({
        let callback = super::admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let result = session.set_editor_visibility(
                    Actor::User,
                    Action::ShowAll,
                    &session.working_region_stamp(),
                );
                if let Err(error) = result {
                    app.set_message(error.into());
                }
                refresh(&app, session);
            });
        }
    });
}
