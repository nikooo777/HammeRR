use super::*;

pub(super) fn refresh_interaction(app: &AppWindow, session: &Session) {
    let tool = session.transform_tool();
    let index = match tool.mode {
        None => 0,
        Some(render::handles::Mode::Move) => 1,
        Some(render::handles::Mode::Rotate) => 2,
        Some(render::handles::Mode::Scale) => 3,
    };
    let index = if session.paint_enabled() {
        if session.paint_operation() == session::paint::Operation::Selection {
            4
        } else {
            5
        }
    } else {
        index
    };
    app.set_handles_mode_index(index);
    app.set_paint_radius(session.paint_radius() as f32);
    app.set_texture_paint_mode(i32::from(
        session.paint_operation() == session::paint::Operation::Projection,
    ));
    app.set_has_texture_stroke(
        session
            .paint_stroke()
            .is_some_and(|stroke| stroke.texture.is_some()),
    );
    app.set_has_paint_preview(session.paint_stroke().is_some());
    app.set_paint_token(
        session
            .paint_stroke()
            .map(|stroke| stroke.token.to_string())
            .unwrap_or_default()
            .into(),
    );
    if index > 0 && index < 4 {
        app.set_transform_mode(index - 1);
    }
    app.set_transform_space_index(i32::from(tool.local));
    app.set_rotation_step(tool.angle_step.to_string().into());
    let (texture_lock, texture_scale_lock) = session.transform_preview().map_or(
        (
            tool.texture_lock,
            tool.texture_scale_lock.unwrap_or(tool.texture_lock),
        ),
        |preview| {
            (
                preview.transform.texture_lock,
                preview
                    .transform
                    .texture_scale_lock
                    .unwrap_or(preview.transform.texture_lock),
            )
        },
    );
    app.set_transform_texture_lock(texture_lock);
    app.set_transform_texture_scale_lock(texture_scale_lock);
    app.set_transform_pivot_mode(match tool.pivot {
        None => 0,
        Some(pivot) if pivot == DVec3::ZERO => 1,
        Some(_) => 2,
    });
    if let Some(pivot) = tool.pivot {
        app.set_pivot_x(pivot.x.to_string().into());
        app.set_pivot_y(pivot.y.to_string().into());
        app.set_pivot_z(pivot.z.to_string().into());
    }
    app.set_area_rule_index(i32::from(
        session.area_rule() == map_core::region::Match::Contained,
    ));
    let selection_mode = session
        .paint_stroke()
        .map_or(session.area_mode(), |stroke| stroke.spec.mode);
    app.set_area_mode_index(match selection_mode {
        SelectMode::Replace => 0,
        SelectMode::Add => 1,
        SelectMode::Remove => 2,
        SelectMode::Toggle => 3,
    });
    if let Some(preview) = session.transform_preview() {
        let spec = preview.transform;
        let presentation = preview.presentation();
        app.set_transform_mode(presentation.primary.index());
        app.set_transform_preview_summary(presentation.summary.into());
        if let Some(shear) = spec.shear {
            app.set_transform_shear_axis(shear.axis as i32);
            app.set_transform_shear_reference(shear.reference as i32);
            app.set_transform_shear_factor(shear.factor.to_string().into());
        }
        app.set_transform_reflect_x(spec.reflection[0]);
        app.set_transform_reflect_y(spec.reflection[1]);
        app.set_transform_reflect_z(spec.reflection[2]);
        let translation = spec.basis.transpose() * spec.translation;
        app.set_transform_preview_values(
            Rc::new(VecModel::from(
                [translation, spec.rotation, spec.scale]
                    .iter()
                    .flat_map(|value| {
                        value
                            .to_array()
                            .map(|value| SharedString::from(value.to_string()))
                    })
                    .collect::<Vec<_>>(),
            ))
            .into(),
        );
        app.set_transform_space_index(i32::from(
            !spec.basis.abs_diff_eq(glam::DMat3::IDENTITY, 1e-8),
        ));
        app.set_transform_pivot_mode(2);
        app.set_pivot_x(spec.pivot.x.to_string().into());
        app.set_pivot_y(spec.pivot.y.to_string().into());
        app.set_pivot_z(spec.pivot.z.to_string().into());
    } else {
        app.set_transform_preview_summary("".into());
    }
    app.set_has_transform_preview(session.transform_preview().is_some());
    app.set_has_area_preview(session.area_preview().is_some());
    let summary = if let Some(drag) = session.component_drag() {
        drag.error.clone().unwrap_or_else(|| {
            format!(
                "Move components · {} · release applies · Esc cancels",
                drag.handle.name()
            )
        })
    } else if let Some(picking) = session.component_picking() {
        picking.hint()
    } else if let Some(drag) = session.transform_drag() {
        let transform = session.transform_preview().map(|preview| preview.transform);
        match (drag.mode, transform) {
            (render::handles::Mode::Move, Some(transform)) => format!(
                "Move · {:.3} {:.3} {:.3} units",
                transform.translation.x, transform.translation.y, transform.translation.z
            ),
            (render::handles::Mode::Rotate, Some(transform)) => format!(
                "Rotate {} · {:.3}°",
                drag.handle.name(),
                transform.rotation[drag.handle.axis().unwrap()]
            ),
            (render::handles::Mode::Scale, Some(transform)) => format!(
                "Scale · {:.3} {:.3} {:.3}",
                transform.scale.x, transform.scale.y, transform.scale.z
            ),
            _ => "Drag to preview · release applies · Esc cancels".into(),
        }
    } else if let Some(preview) = session.transform_preview() {
        format!("{} objects · transform preview", preview.ids.len())
    } else if let Some(stroke) = session.paint_stroke() {
        if let Some(texture) = &stroke.texture {
            format!(
                "{} {} will change · texture preview",
                texture.changed,
                if texture.changed == 1 {
                    "face"
                } else {
                    "faces"
                }
            )
        } else {
            format!(
                "{} {} · paint preview",
                stroke.objects.len() + stroke.faces.len(),
                match (stroke.spec.faces, stroke.objects.len() + stroke.faces.len()) {
                    (true, 1) => "face",
                    (true, _) => "faces",
                    (false, 1) => "object",
                    (false, _) => "objects",
                }
            )
        }
    } else if let Some(preview) = session.area_preview() {
        if preview.faces {
            format!(
                "{} matching faces · selection preview",
                preview.matches.faces.len()
            )
        } else {
            format!(
                "{} matching objects · selection preview",
                preview.matches.objects.len()
            )
        }
    } else {
        String::new()
    };
    app.set_interaction_summary(summary.into());
}

pub(super) fn transform_tool_from_controls(app: &AppWindow) -> Result<TransformTool, String> {
    let mode = match app.get_handles_mode_index() {
        0 => None,
        1 => Some(render::handles::Mode::Move),
        2 => Some(render::handles::Mode::Rotate),
        3 => Some(render::handles::Mode::Scale),
        _ => None,
    };
    let pivot = match app.get_transform_pivot_mode() {
        0 => None,
        1 => Some(DVec3::ZERO),
        _ => Some(input_vector([
            app.get_pivot_x(),
            app.get_pivot_y(),
            app.get_pivot_z(),
        ])?),
    };
    Ok(TransformTool {
        mode,
        local: app.get_transform_space_index() == 1,
        pivot,
        texture_lock: app.get_transform_texture_lock(),
        texture_scale_lock: Some(app.get_transform_texture_scale_lock()),
        angle_step: app
            .get_rotation_step()
            .parse()
            .map_err(|_| "Enter a numeric angle snap")?,
    })
}

pub(super) fn paint_operation_from_controls(app: &AppWindow) -> session::paint::Operation {
    if app.get_handles_mode_index() != 5 {
        session::paint::Operation::Selection
    } else if app.get_texture_paint_mode() == 1 {
        session::paint::Operation::Projection
    } else {
        session::paint::Operation::Material
    }
}

pub(super) fn refresh_texture_brush_sample(app: &AppWindow, session: &mut Session) {
    let material = session
        .surface_sample()
        .map(|sample| sample.material.to_ascii_lowercase())
        .unwrap_or_default();
    if app.get_texture_sample_material().as_str() != material {
        let preview = if material.is_empty() {
            slint::Image::default()
        } else {
            session
                .material_preview(&material)
                .ok()
                .map(|preview| preview_image(&preview))
                .unwrap_or_default()
        };
        app.set_texture_sample_material(material.into());
        app.set_texture_sample_preview(preview);
    }
}

pub(super) fn refresh_terrain_controls(app: &AppWindow, session: &Session) {
    app.set_terrain_creation_error(session.terrain_creation_error().unwrap_or_default().into());
    app.set_can_remove_terrain(session.terrain_removal_error().is_none());
    let faces = session.selected_faces();
    let choices = match session.terrain_creation_faces(&faces) {
        Ok(faces) if faces.len() == 1 => faces[0]
            .corners
            .iter()
            .enumerate()
            .map(|(index, p)| {
                format!(
                    "Corner {} · X {:.1} · Y {:.1} · Z {:.1}",
                    index + 1,
                    p.x,
                    p.y,
                    p.z
                )
            })
            .collect::<Vec<_>>(),
        _ => (1..=4)
            .map(|index| format!("Corner {index} · each face"))
            .collect(),
    };
    app.set_terrain_corner_choices(
        Rc::new(VecModel::from(
            choices
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
}

pub(super) fn bind_terrain_controls(app: &AppWindow, shared: SharedSession) {
    app.on_create_terrain({
        let shared = shared.clone();
        let app = app.as_weak();
        move |power, corner| {
            let mut session = lock(&shared);
            let faces = session.selected_faces();
            let result = u32::try_from(power)
                .map_err(|_| "Invalid terrain power".to_owned())
                .and_then(|power| {
                    let corner = u32::try_from(corner)
                        .map_err(|_| "Invalid terrain start corner".to_owned())?;
                    session.create_terrain(Actor::User, &faces, power, corner)
                });
            let app = app.unwrap();
            refresh_terrain_controls(&app, &session);
            app.set_message(
                match result {
                    Ok(count) => format!(
                        "Created {count} terrain surface{}",
                        if count == 1 { "" } else { "s" }
                    ),
                    Err(error) => error,
                }
                .into(),
            );
        }
    });
    app.on_remove_terrain({
        let app = app.as_weak();
        move || {
            let mut session = lock(&shared);
            let faces = session.selected_faces();
            let result = session.remove_terrain(Actor::User, &faces);
            let app = app.unwrap();
            refresh_terrain_controls(&app, &session);
            app.set_message(
                match result {
                    Ok(count) => format!(
                        "Removed {count} terrain surface{}",
                        if count == 1 { "" } else { "s" }
                    ),
                    Err(error) => error,
                }
                .into(),
            );
        }
    });
}

pub(super) fn bind_manipulation_controls(app: &AppWindow, shared: SharedSession) {
    app.on_transform_tool_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let result = transform_tool_from_controls(&app)
                .and_then(|tool| session.set_transform_tool(Actor::User, tool))
                .and_then(|()| {
                    session.set_paint_operation(
                        Actor::User,
                        app.get_handles_mode_index() >= 4,
                        f64::from(app.get_paint_radius()),
                        paint_operation_from_controls(&app),
                    )
                });
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_spatial(&app, &session);
        }
    });
    app.on_paint_settings_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Err(error) = session.set_paint_operation(
                Actor::User,
                app.get_handles_mode_index() >= 4,
                f64::from(app.get_paint_radius()),
                paint_operation_from_controls(&app),
            ) {
                app.set_message(error.into());
            }
            refresh_interaction(&app, &session);
        }
    });
    app.on_area_settings_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let rule = if app.get_area_rule_index() == 1 {
                map_core::region::Match::Contained
            } else {
                map_core::region::Match::Intersecting
            };
            let mode = match app.get_area_mode_index() {
                1 => SelectMode::Add,
                2 => SelectMode::Remove,
                3 => SelectMode::Toggle,
                _ => SelectMode::Replace,
            };
            session.cancel_area_selection();
            session.set_area_settings(rule, mode);
            refresh_interaction(&app, &session);
        }
    });
    app.on_apply_transform_preview({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Err(error) = session.commit_transform_preview(Actor::User) {
                app.set_message(error.into());
            }
            refresh_panel(&app, &mut session);
            refresh_interaction(&app, &session);
        }
    });
    app.on_apply_area_selection({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let result = if app.get_has_paint_preview() {
                app.get_paint_token()
                    .parse()
                    .map_err(|_| "Paint preview changed; refresh it".to_string())
                    .and_then(|token| session.commit_active_stroke(Actor::User, token))
                    .map(|_| ())
            } else {
                session.commit_area_selection().map(|_| ())
            };
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_panel(&app, &mut session);
            refresh_interaction(&app, &session);
        }
    });
    app.on_cancel_interaction({
        let app = app.as_weak();
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if session.paint_stroke().is_some() {
                let result = app
                    .get_paint_token()
                    .parse()
                    .map_err(|_| "Paint preview changed; refresh it".to_string())
                    .and_then(|token| session.cancel_paint_selection(token));
                if let Err(error) = result {
                    app.set_message(error.into());
                }
            } else {
                session.cancel_transform_preview();
                session.cancel_area_selection();
                session.clear_volume_selection();
            }
            refresh_interaction(&app, &session);
        }
    });
}

pub(super) fn input_vector(values: [SharedString; 3]) -> Result<DVec3, String> {
    let mut result = DVec3::ZERO;
    for (index, value) in values.iter().enumerate() {
        result[index] = value
            .trim()
            .parse::<f64>()
            .map_err(|_| "Enter a number for each X, Y and Z component")?;
    }
    if !result.is_finite() {
        return Err("Transform components must be finite".into());
    }
    Ok(result)
}

pub(super) fn apply_selection_transform(
    app: &AppWindow,
    session: &mut Session,
) -> Result<(), String> {
    let values = if app.get_transform_mode() >= 3 {
        DVec3::ZERO
    } else {
        input_vector([
            app.get_transform_x(),
            app.get_transform_y(),
            app.get_transform_z(),
        ])?
    };
    let mut transform = edit::Transform {
        texture_lock: app.get_transform_texture_lock(),
        texture_scale_lock: Some(app.get_transform_texture_scale_lock()),
        ..edit::Transform::default()
    };
    match app.get_transform_mode() {
        0 => transform.translation = values,
        1 => transform.rotation = values,
        2 => transform.scale = values,
        3 => {
            transform.reflection = [
                app.get_transform_reflect_x(),
                app.get_transform_reflect_y(),
                app.get_transform_reflect_z(),
            ];
            if !transform.reflection.into_iter().any(|axis| axis) {
                return Err("Choose at least one mirror axis".into());
            }
        }
        4 => {
            transform.shear = Some(map_core::transform::Shear {
                axis: app.get_transform_shear_axis() as usize,
                reference: app.get_transform_shear_reference() as usize,
                factor: app
                    .get_transform_shear_factor()
                    .parse()
                    .map_err(|_| "Enter a numeric shear factor")?,
            });
        }
        _ => return Err("Choose Move, Rotate, Scale, Mirror or Shear".into()),
    }
    transform.basis = session.transform_basis(
        &session.selected_ids(),
        app.get_transform_space_index() == 1,
    )?;
    transform.translation = transform.basis * transform.translation;
    let pivot = if app.get_transform_mode() == 0 {
        None
    } else {
        match app.get_transform_pivot_mode() {
            0 => None,
            1 => Some(DVec3::ZERO),
            2 => Some(input_vector([
                app.get_pivot_x(),
                app.get_pivot_y(),
                app.get_pivot_z(),
            ])?),
            _ => return Err("Choose a pivot".into()),
        }
    };
    let ids = session.selected_ids();
    session.set_transform_tool(Actor::User, transform_tool_from_controls(app)?)?;
    session.transform_objects(Actor::User, &ids, transform, pivot)?;
    Ok(())
}

pub(super) fn apply_transform(app: &AppWindow, ui: &SharedUi) {
    let result = apply_selection_transform(app, &mut lock(&session_of(ui)));
    if let Err(message) = result {
        app.set_message(message.into());
    }
}

/// The horizontal world axis closest to the camera's forward direction, and
/// the one to its right.
pub(super) fn nudge_axes(forward: Vec3) -> (DVec3, DVec3) {
    let f = if forward.x.abs() >= forward.y.abs() {
        DVec3::new(forward.x.signum().into(), 0.0, 0.0)
    } else {
        DVec3::new(0.0, forward.y.signum().into(), 0.0)
    };
    (f, DVec3::new(f.y, -f.x, 0.0))
}
