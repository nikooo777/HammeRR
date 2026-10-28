use crate::AppWindow;
use crate::runtime::DocumentAccess;
use crate::session::{Actor, Session, selection_filters::SelectionFilters};

pub(super) fn refresh(app: &AppWindow, session: &Session) {
    let filters = session.selection_filters();
    app.set_pick_world_brushes(filters.world_brushes);
    app.set_pick_brush_entities(filters.brush_entities);
    app.set_pick_point_entities(filters.point_entities);
}

pub(super) fn bind(app: &AppWindow, access: &DocumentAccess) {
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
