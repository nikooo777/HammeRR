use super::*;

fn viewport_size(app: &AppWindow) -> (u32, u32) {
    (
        app.get_viewport_width().round().max(1.0) as u32,
        app.get_viewport_height().round().max(1.0) as u32,
    )
}

pub(super) fn refresh_pivot_pick_controls(
    app: &AppWindow,
    session: &Session,
    camera: render::Camera,
) {
    let state = session.transform_pivot_pick_state();
    let interactive =
        session.transform_pivot_pick_interactive(Actor::User, camera, viewport_size(app));
    app.set_pivot_picking(interactive);
    app.set_can_pick_pivot(state.begin_error.is_none());
    app.set_can_cancel_pivot_pick(session.transform_pivot_pick_owned(Actor::User));
    let notice = state.picking.map(|picking| {
        if picking.owner != Actor::User.label() {
            format!("Pivot pick belongs to {}. Viewport selection remains available.", picking.owner)
        } else if interactive {
            "Click a visible brush, terrain sheet or model surface. Escape or Cancel pivot pick keeps the current pivot.".into()
        } else {
            format!("{}. Cancel pivot pick to start again.", picking.error.unwrap_or_else(|| "Viewport camera or dimensions changed".into()))
        }
    }).unwrap_or_default();
    app.set_pivot_pick_notice(notice.into());
    app.set_pivot_pick_source(if state.last_pick_matches_pivot {
        state
            .last_pick
            .map(|surface| {
                format!(
                    "Picked {} · X {} · Y {} · Z {} · fixed world point",
                    surface.target, surface.point[0], surface.point[1], surface.point[2]
                )
            })
            .unwrap_or_default()
            .into()
    } else {
        "".into()
    });
}

pub(super) fn bind_pivot_pick_controls(app: &AppWindow, shared: SharedSession) {
    app.on_pick_transform_pivot({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let result = session.begin_transform_pivot_pick(
                Actor::User,
                session.camera(),
                viewport_size(&app),
                &session.transform_pivot_pick_stamp(),
            );
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_pivot_pick_controls(&app, &session, session.camera());
        }
    });
    app.on_cancel_transform_pivot_pick({
        let app = app.as_weak();
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            cancel_pivot_pick(&app, &mut session);
        }
    });
}

pub(super) fn cancel_pivot_pick(app: &AppWindow, session: &mut Session) -> bool {
    if !session.transform_pivot_pick_owned(Actor::User) {
        return false;
    }
    let token = session.transform_pivot_pick_state().picking.unwrap().token;
    match session.cancel_transform_pivot_pick(Actor::User, token) {
        Ok(_) => app.set_message("Pivot pick cancelled. The current pivot is unchanged.".into()),
        Err(error) => app.set_message(error.into()),
    }
    refresh_pivot_pick_controls(app, session, session.camera());
    true
}

pub(super) fn pick_pivot_pixel(
    app: &AppWindow,
    session: &mut Session,
    camera: render::Camera,
    pointer: Pointer,
) -> bool {
    if !session.transform_pivot_pick_interactive(Actor::User, camera, pointer.size) {
        return false;
    }
    let token = session.transform_pivot_pick_state().picking.unwrap().token;
    match session.pick_transform_pivot_pixel(Actor::User, token, pointer) {
        Ok(_) => {
            refresh_interaction(app, session);
            app.set_message("Picked custom transform pivot. The map is unchanged.".into());
        }
        Err(error) => app.set_message(error.into()),
    }
    refresh_pivot_pick_controls(app, session, camera);
    true
}

#[cfg(test)]
mod tests;
