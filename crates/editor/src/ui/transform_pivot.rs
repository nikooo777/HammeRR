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
    refresh_pivot_attachment_controls(app, session);
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

fn refresh_pivot_attachment_controls(app: &AppWindow, session: &Session) {
    let state = session.transform_pivot_attachment_state();
    let attached = state.attachment.is_some();
    if attached != app.get_pivot_attached() {
        refresh_interaction(app, session);
    }
    app.set_can_attach_pivot(state.attach_error.is_none());
    app.set_can_detach_pivot(attached && state.detach_error.is_none());
    let mut summary = if let Some(attachment) = state.attachment {
        format!(
            "{} · {}",
            attachment.object,
            if attachment.kind == "local" {
                "Rigid local point"
            } else {
                "Authored origin"
            }
        )
    } else {
        "Attach origin uses the entity's origin. Attach local point keeps the current world point."
            .into()
    };
    if attached {
        if let Some(point) = state.effective_pivot {
            summary.push_str(&format!(
                "\nResolved · X {:.3} · Y {:.3} · Z {:.3}",
                point[0], point[1], point[2]
            ));
        }
        if let Some(error) = state.error {
            summary.push_str(&format!("\nUnavailable: {error}"));
        }
    }
    if let Some(error) = state.attach_error {
        summary.push_str(&format!("\n{error}"));
    }
    app.set_pivot_attachment_summary(summary.into());
}

fn attachment_target(app: &AppWindow) -> Result<Option<ObjectId>, String> {
    let text = app.get_pivot_entity();
    if text.trim().is_empty() {
        Ok(None)
    } else {
        crate::identity::parse_id(text.trim()).map(Some)
    }
}

pub(super) fn bind_pivot_pick_controls_with_access(
    app: &AppWindow,
    access: &runtime::DocumentAccess,
) {
    app.on_attach_pivot_origin({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let snapshot = session.transform_pivot_attachment_state().snapshot;
                let result = attachment_target(&app).and_then(|target| {
                    session.attach_transform_pivot_origin(Actor::User, target, &snapshot)
                });
                app.set_message(match result {
                    Ok(_) => "Transform pivot follows the authored point-entity origin.".into(),
                    Err(error) => error.into(),
                });
                refresh_interaction(&app, session);
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
    app.on_attach_pivot_local({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let snapshot = session.transform_pivot_attachment_state().snapshot;
                let result = attachment_target(&app).and_then(|target| {
                    session.attach_transform_pivot_local(Actor::User, target, &snapshot)
                });
                app.set_message(match result {
                    Ok(_) => {
                        "Transform pivot follows the captured rigid entity-local point.".into()
                    }
                    Err(error) => error.into(),
                });
                refresh_interaction(&app, session);
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
    app.on_detach_transform_pivot({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let snapshot = session.transform_pivot_attachment_state().snapshot;
                let result = session.detach_transform_pivot(Actor::User, &snapshot);
                app.set_message(match result {
                    Ok(_) => "Transform pivot detached at its resolved world point.".into(),
                    Err(error) => error.into(),
                });
                refresh_interaction(&app, session);
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
    app.on_use_world_transform_pivot({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let result = input_vector([
                    app.get_pivot_world_x(),
                    app.get_pivot_world_y(),
                    app.get_pivot_world_z(),
                ])
                .and_then(|point| {
                    let mut tool = session.transform_tool();
                    tool.pivot = Some(point);
                    session.set_transform_tool_with_pivot(Actor::User, tool, true)
                });
                app.set_message(match result {
                    Ok(_) => "Transform pivot replaced with the entered fixed world point.".into(),
                    Err(error) => error.into(),
                });
                refresh_interaction(&app, session);
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
    app.on_transform_pivot_changed({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let result = transform_tool_from_controls(&app, session).and_then(|tool| {
                    session.set_transform_tool_with_pivot(Actor::User, tool, true)
                });
                if let Err(error) = result {
                    app.set_message(error.into());
                }
                refresh_interaction(&app, session);
                refresh_pivot_pick_controls(&app, session, session.camera());
            })
        }
    });
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
