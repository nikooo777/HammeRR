use crate::AppWindow;
use crate::runtime::DocumentAccess;
use crate::session::{
    Actor, Session,
    selection_filters::{ObjectSelectionMode, SelectionFilters},
};

pub(super) fn refresh(app: &AppWindow, session: &Session) {
    let filters = session.selection_filters();
    app.set_pick_world_brushes(filters.world_brushes);
    app.set_pick_brush_entities(filters.brush_entities);
    app.set_pick_point_entities(filters.point_entities);
    app.set_object_selection_mode(match session.object_selection_mode() {
        ObjectSelectionMode::Groups => 0,
        ObjectSelectionMode::Objects => 1,
        ObjectSelectionMode::Solids => 2,
    });
}

pub(super) fn bind(app: &AppWindow, access: &DocumentAccess) {
    app.on_object_selection_mode_changed({
        let callback = super::admission::SessionCallback::new(app, access);
        move |mode| {
            callback.input(|app, session| {
                let mode = match mode {
                    0 => ObjectSelectionMode::Groups,
                    1 => ObjectSelectionMode::Objects,
                    2 => ObjectSelectionMode::Solids,
                    _ => {
                        app.set_message("Choose Groups, Objects or Solids".into());
                        return;
                    }
                };
                if let Err(error) = session.set_object_selection_mode(Actor::User, mode) {
                    app.set_message(error.into());
                }
                refresh(&app, session);
            });
        }
    });
    app.on_selection_filters_changed({
        let callback = super::admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let filters = SelectionFilters {
                    world_brushes: app.get_pick_world_brushes(),
                    brush_entities: app.get_pick_brush_entities(),
                    point_entities: app.get_pick_point_entities(),
                };
                if let Err(error) = session.set_selection_filters(Actor::User, filters) {
                    app.set_message(error.into());
                }
                refresh(&app, session);
            });
        }
    });
}
