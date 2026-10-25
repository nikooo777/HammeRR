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
    let grid = session.transform_pivot_grid_state();
    app.set_can_snap_pivot(grid.error.is_none());
    app.set_pivot_grid_error(grid.error.unwrap_or_default().into());
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

#[cfg(test)]
pub(super) fn bind_pivot_pick_controls(app: &AppWindow, shared: SharedSession) {
    bind_pivot_pick_controls_with_access(app, &runtime::DocumentAccess::unmanaged(shared));
}

pub(super) fn bind_pivot_pick_controls_with_access(
    app: &AppWindow,
    access: &runtime::DocumentAccess,
) {
    app.on_snap_transform_pivot_to_grid({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let snapshot = session.transform_pivot_grid_stamp();
                match session.snap_transform_pivot_to_grid(Actor::User, &snapshot) {
                    Ok(report) => {
                        refresh_interaction(&app, session);
                        let point = report.after.unwrap();
                        app.set_message(
                            format!(
                                "Pivot at X {} · Y {} · Z {} on grid {}. Fixed world point.",
                                point[0], point[1], point[2], report.grid
                            )
                            .into(),
                        );
                    }
                    Err(error) => app.set_message(error.into()),
                }
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
    app.on_pick_transform_pivot({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let camera = session.camera();
                let snapshot = session.transform_pivot_pick_stamp();
                let result = session.begin_transform_pivot_pick(
                    Actor::User,
                    camera,
                    viewport_size(&app),
                    &snapshot,
                );
                if let Err(error) = result {
                    app.set_message(error.into());
                }
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
    app.on_cancel_transform_pivot_pick({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                cancel_pivot_pick(&app, session);
            })
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
