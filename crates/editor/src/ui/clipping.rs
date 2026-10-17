use super::*;

pub(super) fn refresh_clip(app: &AppWindow, session: &Session) {
    app.set_can_clip(session.can_clip_selection());
    app.set_clipping(session.clip_tool());
    app.set_clip_positioning(session.clip_drag().is_some());
    let options = session.clip_handle_options();
    app.set_clip_handles_mode(match options.mode {
        Some(render::handles::Mode::Move) => 1,
        Some(render::handles::Mode::Rotate) => 2,
        _ => 0,
    });
    app.set_clip_handles_snap(options.snap);
    app.set_clip_angle_step(options.angle_step.to_string().into());
    app.set_has_clip_preview(session.clip_preview().is_some());
    app.set_clip_point_mode(session.clip_points().is_some());
    app.set_clip_point_count(session.clip_points().map_or(0, |d| d.points.len() as i32));
    if let Some(draft) = session.clip_points() {
        let positions = draft
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{}: {:.2}, {:.2}, {:.2}", i + 1, p.x, p.y, p.z))
            .collect::<Vec<_>>()
            .join("  ·  ");
        app.set_clip_points_summary(
            format!(
                "{} / 3 points · click surfaces to add points; drag handles to position the plane\n{}",
                draft.points.len(),
                positions
            )
            .into(),
        );
    }
    if let Some(preview) = session.clip_preview() {
        app.set_clip_normal_x(preview.plane.normal.x.to_string().into());
        app.set_clip_normal_y(preview.plane.normal.y.to_string().into());
        app.set_clip_normal_z(preview.plane.normal.z.to_string().into());
        app.set_clip_distance(preview.plane.dist.to_string().into());
        app.set_clip_keep_index(match preview.keep {
            map_core::clip::Keep::Front => 0,
            map_core::clip::Keep::Back => 1,
            map_core::clip::Keep::Both => 2,
        });
        app.set_clip_material(preview.material.clone().unwrap_or_default().into());
        app.set_clip_summary(
            format!(
                "{} brushes cut · {} pieces · {} unchanged · preview only",
                preview.result.cuts.len(),
                preview
                    .result
                    .cuts
                    .iter()
                    .map(|cut| cut.solids.len())
                    .sum::<usize>(),
                preview.result.unchanged.len()
            )
            .into(),
        );
        if let Some(error) = session.clip_drag().and_then(|drag| drag.error.as_ref()) {
            app.set_clip_summary(error.clone().into());
        }
    } else {
        app.set_clip_summary(
            "Draw a cut or preview a plane · the map stays unchanged until Apply cut".into(),
        );
    }
}

pub(super) fn clip_keep(app: &AppWindow) -> map_core::clip::Keep {
    match app.get_clip_keep_index() {
        0 => map_core::clip::Keep::Front,
        1 => map_core::clip::Keep::Back,
        _ => map_core::clip::Keep::Both,
    }
}

pub(super) fn preview_clip(
    app: &AppWindow,
    session: &mut Session,
    plane: map_core::Plane,
) -> Result<(), String> {
    let material = app.get_clip_material().trim().to_owned();
    session.clip_brushes(
        Actor::User,
        &session.selected_ids(),
        plane,
        clip_keep(app),
        (!material.is_empty()).then_some(material),
        true,
    )?;
    refresh_clip(app, session);
    Ok(())
}

pub(super) fn preview_clip_plane(app: &AppWindow, session: &mut Session) {
    let result = input_vector([
        app.get_clip_normal_x(),
        app.get_clip_normal_y(),
        app.get_clip_normal_z(),
    ])
    .and_then(|normal| {
        let dist = app
            .get_clip_distance()
            .parse()
            .map_err(|_| "Enter a numeric plane distance")?;
        preview_clip(app, session, map_core::Plane { normal, dist })
    });
    if let Err(error) = result {
        session.cancel_clip_preview();
        refresh_clip(app, session);
        app.set_message(error.into());
    }
}

pub(super) fn apply_clip(app: &AppWindow, session: &mut Session) {
    match session.commit_clip_preview(Actor::User) {
        Ok(result) => app
            .set_message(format!("Clipped {} brushes in one undo step", result.cuts.len()).into()),
        Err(error) => app.set_message(error.into()),
    }
    refresh_spatial(app, session);
    refresh_panel(app, session);
}

pub(super) fn cancel_clip(app: &AppWindow, session: &mut Session) {
    session.cancel_clip_preview();
    drop(session.set_clip_tool(Actor::User, false));
    refresh_clip(app, session);
}

pub(super) fn change_clip_points(
    app: &AppWindow,
    session: &mut Session,
    points: Vec<DVec3>,
) -> Result<(), String> {
    let material = app.get_clip_material().trim().to_owned();
    session.set_clip_points(
        Actor::User,
        session::clip_points::Draft {
            points,
            keep: clip_keep(app),
            material: (!material.is_empty()).then_some(material),
            snap: true,
        },
        &session.clip_point_stamp(),
    )
}

#[cfg(test)]
pub(super) fn bind_clip_controls(app: &AppWindow, shared: SharedSession) {
    bind_clip_controls_with_access(app, &runtime::DocumentAccess::unmanaged(shared));
}

pub(super) fn bind_clip_controls_with_access(app: &AppWindow, access: &runtime::DocumentAccess) {
    app.on_clip_handles_changed({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                let snapshot = session.clip_point_stamp();
                let result = app
                    .get_clip_angle_step()
                    .parse::<f64>()
                    .map_err(|_| "Enter a rotation step in degrees".to_owned())
                    .and_then(|angle_step| {
                        session.set_clip_handle_options(
                            session::clip_drag::Options {
                                mode: match app.get_clip_handles_mode() {
                                    1 => Some(render::handles::Mode::Move),
                                    2 => Some(render::handles::Mode::Rotate),
                                    _ => None,
                                },
                                snap: app.get_clip_handles_snap(),
                                angle_step,
                            },
                            &snapshot,
                        )
                    });
                app.set_message(result.err().unwrap_or_default().into());
                refresh_clip(&app, session);
            })
        }
    });
    app.on_cancel_clip_positioning({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                if let Some(token) = session.clip_drag().map(|d| d.token) {
                    let result = session.cancel_clip_drag(token);
                    app.set_message(result.err().unwrap_or_default().into());
                }
                refresh_clip(&app, session);
            })
        }
    });
    app.on_clip_points_action({
        let callback = admission::SessionCallback::new(app, access);
        move |action| {
            callback.input(|app, session| {
                if action == 3 {
                    session.cancel_clip_preview();
                    refresh_clip(&app, session);
                    return;
                }
                let mut points = session
                    .clip_points()
                    .map_or_else(Vec::new, |d| d.points.clone());
                if action == 1 {
                    points.pop();
                } else {
                    points.clear();
                }
                let result = change_clip_points(&app, session, points);
                app.set_message(result.err().unwrap_or_default().into());
                refresh_clip(&app, session);
            })
        }
    });
    app.on_start_clip({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                if let Err(error) = session.set_clip_tool(Actor::User, true) {
                    app.set_message(error.into());
                }
                refresh_spatial(&app, session);
            })
        }
    });
    app.on_preview_clip_plane({
        let callback = admission::SessionCallback::new(app, access);
        move || callback.input(|app, session| preview_clip_plane(&app, session))
    });
    app.on_clip_settings_changed({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                    let result = change_clip_points(&app, session, points);
                    app.set_message(result.err().unwrap_or_default().into());
                    refresh_clip(&app, session);
                    return;
                }
                if let Some(plane) = session.clip_preview().map(|preview| preview.plane)
                    && let Err(error) = preview_clip(&app, session, plane)
                {
                    session.cancel_clip_preview();
                    refresh_clip(&app, session);
                    app.set_message(error.into());
                }
            })
        }
    });
    app.on_clip_inputs_changed({
        let callback = admission::SessionCallback::new(app, access);
        move || {
            callback.input(|app, session| {
                if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                    let result = change_clip_points(&app, session, points);
                    app.set_message(result.err().unwrap_or_default().into());
                } else {
                    session.cancel_clip_preview();
                }
                refresh_clip(&app, session);
            })
        }
    });
    app.on_apply_clip({
        let callback = admission::SessionCallback::new(app, access);
        move || callback.input(|app, session| apply_clip(&app, session))
    });
    app.on_cancel_clip({
        let callback = admission::SessionCallback::new(app, access);
        move || callback.input(|app, session| cancel_clip(&app, session))
    });
}

pub(super) fn clip_drag_plane(
    session: &Session,
    drag: BrushDrag,
    end: DVec2,
) -> Result<map_core::Plane, String> {
    if !session.clip_tool()
        || drag.generation != session.document_generation()
        || drag.revision != session.document().revision()
        || drag.projection != session.camera().projection
        || drag.epoch != session.clip_epoch()
    {
        return Err("The cut was cancelled or its map/settings changed".into());
    }
    session.clip_line_plane(drag.projection, drag.start, end)
}
