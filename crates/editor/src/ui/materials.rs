use super::*;

pub(crate) fn preview_image(preview: &session::MaterialPreview) -> slint::Image {
    preview
        .image
        .as_ref()
        .map_or_else(slint::Image::default, |image| {
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &image.pixels,
                image.width,
                image.height,
            );
            slint::Image::from_rgba8(buffer)
        })
}

pub(super) fn refresh_material_preview(app: &AppWindow, session: &mut Session) {
    match session.material_preview(&app.get_material()) {
        Ok(preview) => {
            app.set_has_material_preview(preview.image.is_some());
            app.set_material_preview(preview_image(&preview));
        }
        Err(_) => {
            app.set_has_material_preview(false);
            app.set_material_preview(slint::Image::default());
        }
    }
}

pub(crate) fn refresh_material_window(window: &MaterialWindow, session: &mut Session) {
    window.set_favorite(session.is_material_favorite(&window.get_material()));
    match session.material_preview(&window.get_material()) {
        Ok(preview) => {
            window.set_available(preview.image.is_some());
            window.set_preview(preview_image(&preview));
            window.set_detail(
                format!(
                    "{}\n{} × {}{}",
                    preview.name,
                    preview.width,
                    preview.height,
                    if preview.missing {
                        " · Missing texture"
                    } else if preview.tool {
                        " · Tool material"
                    } else {
                        ""
                    }
                )
                .into(),
            );
        }
        Err(error) => {
            window.set_available(false);
            window.set_preview(slint::Image::default());
            window.set_detail(error.into());
        }
    }
}

pub(super) fn optional_number(value: SharedString) -> Result<Option<f64>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let number = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Enter a number: {value}"))?;
    if !number.is_finite() {
        return Err("Texture values must be finite".into());
    }
    Ok(Some(number))
}

pub(super) fn surface_patch(
    window: &SurfaceWindow,
) -> Result<map_core::surface::SurfaceEdit, String> {
    use map_core::surface::{Alignment, Justify};
    let justify = |index| match index {
        1 => Some((Justify::Start, 0)),
        2 => Some((Justify::Center, 0)),
        3 => Some((Justify::End, 0)),
        _ => None,
    };
    Ok(map_core::surface::SurfaceEdit {
        material: (!window.get_material().trim().is_empty())
            .then(|| window.get_material().trim().to_owned()),
        shift_u: optional_number(window.get_shift_u())?,
        shift_v: optional_number(window.get_shift_v())?,
        scale_u: optional_number(window.get_scale_u())?,
        scale_v: optional_number(window.get_scale_v())?,
        rotation: optional_number(window.get_rotation())?,
        alignment: match window.get_alignment_index() {
            1 => Some(Alignment::World),
            2 => Some(Alignment::Face),
            _ => None,
        },
        justify_u: justify(window.get_justify_u_index()),
        justify_v: justify(window.get_justify_v_index()),
        ..Default::default()
    })
}

pub(super) fn apply_surface_dialog(
    window: &SurfaceWindow,
    session: &mut Session,
    target: &(Vec<map_core::surface::FaceId>, u64, u64),
    preview: bool,
) -> Result<usize, String> {
    if session.document_generation() != target.1
        || session.document().revision() != target.2
        || session.selected_faces() != target.0
    {
        return Err("Map or face selection changed. Cancel and reopen texture editing.".into());
    }
    if window.get_operation_index() == 4 {
        return session
            .propagate_face_texture(Actor::User, &target.0, preview)
            .map(|result| result.changed);
    }
    if window.get_operation_index() != 0 {
        use map_core::surface::PaintMode;
        return session.paint_face_texture(
            Actor::User,
            &target.0,
            match window.get_operation_index() {
                1 => PaintMode::Material,
                2 => PaintMode::Projection,
                _ => PaintMode::Wrap,
            },
            preview,
        );
    }
    session.edit_faces(
        Actor::User,
        &target.0,
        surface_patch(window)?,
        window.get_fit(),
        preview,
    )
}

pub(crate) fn common_face_number(
    document: &map_core::Document,
    faces: &[map_core::surface::FaceId],
    value: fn(&source_formats::vmf::Side) -> f64,
) -> SharedString {
    let values: Vec<_> = faces
        .iter()
        .filter_map(|face| map_core::surface::side(document, *face))
        .map(value)
        .collect();
    values
        .first()
        .filter(|first| values.iter().all(|value| value == *first))
        .map(|value| value.to_string())
        .unwrap_or_default()
        .into()
}

pub(super) fn refresh_surface_dialog(
    window: &SurfaceWindow,
    session: &mut Session,
    target: &(Vec<map_core::surface::FaceId>, u64, u64),
) {
    refresh_surface_sample(window, session);
    let stale = session.document_generation() != target.1
        || session.document().revision() != target.2
        || session.selected_faces() != target.0;
    window.set_stale(stale);
    if stale {
        window.set_message(
            "Map or face selection changed. Cancel and reopen texture editing.".into(),
        );
        return;
    }
    let Some(document) = session.face_preview_document() else {
        window.set_message("No active preview. Preview or apply the current fields.".into());
        return;
    };
    if session.preview_faces() != target.0 {
        window.set_message(
            "A preview targets other faces. Preview these fields to replace it.".into(),
        );
        return;
    }
    let names: Vec<_> = target
        .0
        .iter()
        .filter_map(|face| map_core::surface::side(document, *face))
        .map(|side| side.material.to_ascii_lowercase())
        .collect();
    let material = names
        .first()
        .filter(|first| names.iter().all(|name| name == *first))
        .cloned()
        .unwrap_or_default();
    window.set_material(material.clone().into());
    window.set_shift_u(common_face_number(document, &target.0, |side| {
        side.u_axis.offset
    }));
    window.set_shift_v(common_face_number(document, &target.0, |side| {
        side.v_axis.offset
    }));
    window.set_scale_u(common_face_number(document, &target.0, |side| {
        side.u_axis.scale
    }));
    window.set_scale_v(common_face_number(document, &target.0, |side| {
        side.v_axis.scale
    }));
    window.set_rotation(common_face_number(document, &target.0, |side| {
        side.rotation
    }));
    window.set_preview(
        session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_message("Shared preview shown in the viewport. Apply to keep it.".into());
}

pub(super) fn refresh_surface_sample(window: &SurfaceWindow, session: &mut Session) {
    let sample = session.surface_sample().cloned();
    window.set_sample_present(sample.is_some());
    window.set_sample_label(
        sample
            .as_ref()
            .map(|sample| {
                format!(
                    "{} · brush {}, face {}",
                    sample.material.to_ascii_lowercase(),
                    sample.face.solid,
                    sample.face.side
                )
            })
            .unwrap_or_else(|| {
                "Select one source face and choose Sample face texture first.".into()
            })
            .into(),
    );
    window.set_sample_preview(
        sample
            .and_then(|sample| session.material_preview(&sample.material).ok())
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
}

pub(super) fn show_surface_window(app: &AppWindow, ui: &SharedUi) {
    if ui.borrow().surface_window.is_none() {
        let window = match SurfaceWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_cancel({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                lock(&session_of(&ui)).cancel_face_preview();
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let ui = ui.clone();
            move || {
                lock(&session_of(&ui)).cancel_face_preview();
                slint::CloseRequestResponse::HideWindow
            }
        });
        window.on_browse({
            let (app, ui) = (app.as_weak(), ui.clone());
            move || show_material_window(&app.unwrap(), &ui, "@faces")
        });
        window.on_preview_edit({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let target = ui.borrow().surface_target.clone();
                if let Some(target) = target {
                    let result =
                        apply_surface_dialog(&window, &mut lock(&session_of(&ui)), &target, true);
                    window.set_message(match result {
                        Ok(_) => "Preview shown in the viewport. Apply to keep it.".into(),
                        Err(error) => error.into(),
                    });
                }
            }
        });
        window.on_apply({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let target = ui.borrow().surface_target.clone();
                if let Some(target) = target {
                    match apply_surface_dialog(&window, &mut lock(&session_of(&ui)), &target, false)
                    {
                        Ok(_) => {
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                }
            }
        });
        ui.borrow_mut().surface_window = Some(window);
    }
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let faces = session.selected_faces();
    if faces.is_empty() {
        app.set_message("Select faces in the viewport first.".into());
        return;
    }
    session.cancel_face_preview();
    let window = ui.borrow().surface_window.as_ref().unwrap().clone_strong();
    window.set_selection_label(
        format!(
            "Textures on {} selected face{}",
            faces.len(),
            if faces.len() == 1 { "" } else { "s" }
        )
        .into(),
    );
    let names: Vec<_> = faces
        .iter()
        .filter_map(|face| map_core::surface::side(session.document(), *face))
        .map(|side| side.material.to_ascii_lowercase())
        .collect();
    let material = names
        .first()
        .filter(|first| names.iter().all(|name| name == *first))
        .cloned()
        .unwrap_or_default();
    window.set_material(material.clone().into());
    window.set_preview(
        session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_shift_u(common_face_number(session.document(), &faces, |side| {
        side.u_axis.offset
    }));
    window.set_shift_v(common_face_number(session.document(), &faces, |side| {
        side.v_axis.offset
    }));
    window.set_scale_u(common_face_number(session.document(), &faces, |side| {
        side.u_axis.scale
    }));
    window.set_scale_v(common_face_number(session.document(), &faces, |side| {
        side.v_axis.scale
    }));
    window.set_rotation(common_face_number(session.document(), &faces, |side| {
        side.rotation
    }));
    window.set_fit(false);
    window.set_operation_index(0);
    window.set_alignment_index(0);
    window.set_justify_u_index(0);
    window.set_justify_v_index(0);
    refresh_surface_sample(&window, &mut session);
    window.set_stale(false);
    window.set_message("Preview changes before applying, or Cancel to discard.".into());
    ui.borrow_mut().surface_target = Some((
        faces,
        session.document_generation(),
        session.document().revision(),
    ));
    drop(session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(crate) fn refresh_material_catalog(window: &MaterialWindow, session: &mut Session) {
    let catalog = match session.material_catalog(
        &window.get_filter(),
        &window.get_group(),
        window.get_offset().max(0) as usize,
        36,
    ) {
        Ok(catalog) => catalog,
        Err(error) => {
            window.set_rows(ModelRc::new(VecModel::from(Vec::<MaterialTileRow>::new())));
            window.set_groups(ModelRc::new(VecModel::from(Vec::<MaterialGroup>::new())));
            window.set_total(0);
            window.set_catalog_label("Material catalog unavailable".into());
            window.set_available(false);
            window.set_preview(slint::Image::default());
            window.set_detail(error.into());
            return;
        }
    };
    let groups = catalog
        .groups
        .into_iter()
        .map(|group| MaterialGroup {
            id: group.id.into(),
            label: format!("{} ({})", group.label, group.count).into(),
        })
        .collect::<Vec<_>>();
    window.set_groups(ModelRc::new(VecModel::from(groups)));
    window.set_favorites_status(
        catalog
            .favorites
            .error
            .as_deref()
            .unwrap_or(match catalog.favorites.persistence {
                material_favorites::Persistence::Session => "Favorites last for this session",
                material_favorites::Persistence::Persistent => "Favorites saved across sessions",
                material_favorites::Persistence::Blocked => "Favorites storage unavailable",
            })
            .into(),
    );
    window.set_total(catalog.total as i32);
    window.set_catalog_label(if catalog.total == 0 {
        "No matching materials".into()
    } else {
        format!(
            "{}–{} of {} materials",
            catalog.offset + 1,
            catalog.offset + catalog.entries.len(),
            catalog.total
        )
        .into()
    });
    let tiles = catalog
        .entries
        .into_iter()
        .map(|entry| MaterialTile {
            preview: session
                .material_preview(&entry.name)
                .ok()
                .map(|preview| preview_image(&preview))
                .unwrap_or_default(),
            name: entry.name.into(),
            favorite: entry.favorite,
        })
        .collect::<Vec<_>>();
    let rows = tiles
        .chunks(3)
        .map(|tiles| MaterialTileRow {
            tiles: ModelRc::new(VecModel::from(tiles.to_vec())),
        })
        .collect::<Vec<_>>();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    window.set_favorite(session.is_material_favorite(&window.get_material()));
}

pub(super) fn show_material_window(app: &AppWindow, ui: &SharedUi, key: &str) {
    if ui.borrow().material_window.is_none() {
        let window = match MaterialWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_search({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                refresh_material_catalog(&window, &mut lock(&session_of(&ui)));
            }
        });
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.on_toggle_favorite({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                if let Err(error) =
                    session.favorite_material(&window.get_material(), !window.get_favorite())
                {
                    window.set_detail(error.into());
                }
                refresh_material_catalog(&window, &mut session);
            }
        });
        window.on_preview_material({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || refresh_material_window(&window.unwrap(), &mut lock(&session_of(&ui)))
        });
        window.on_use_material({
            let (window, app, ui) = (window.as_weak(), app.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let app = app.unwrap();
                let material = window.get_material();
                let target = ui.borrow().material_target.clone();
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                if let Some((entity, key, generation)) = target {
                    if session.document_generation() != generation {
                        window.set_detail("Map changed. Open this picker again.".into());
                        return;
                    }
                    let result = session.set_entity_properties(
                        Actor::User,
                        entity,
                        &[(key, Some(material.to_string()))],
                    );
                    if let Err(error) = result {
                        window.set_detail(error.into());
                        return;
                    }
                } else if let Some(target) = ui.borrow().overlay_material_target.clone() {
                    if let Some(controller) = &ui.borrow().overlay
                        && let Err(error) =
                            controller.choose_material(&mut session, &target, &material)
                    {
                        window.set_detail(error.into());
                        return;
                    }
                } else if let Some(replacement) = ui.borrow().material_for_replace {
                    if let Some(controller) = &ui.borrow().material_replace {
                        controller.choose_material(&mut session, replacement, &material);
                    }
                } else if ui.borrow().material_for_clip {
                    app.set_clip_material(material);
                    if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                        if let Err(error) = change_clip_points(&app, &mut session, points) {
                            window.set_detail(error.into());
                            return;
                        }
                    } else {
                        session.cancel_clip_preview();
                    }
                    refresh_clip(&app, &session);
                } else if ui.borrow().material_for_primitive {
                    if let Some(primitive) = &ui.borrow().primitive_window {
                        let Some(target) = ui.borrow().primitive_material_target else {
                            return;
                        };
                        if let Err(error) = primitive_ui::choose_material(
                            primitive,
                            &mut session,
                            target,
                            &material,
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_polygon {
                    if let Some(polygon) = &ui.borrow().polygon {
                        let Some(target) = ui.borrow().primitive_material_target else {
                            return;
                        };
                        if let Err(error) = polygon.choose_material(&mut session, target, &material)
                        {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_partition {
                    if let Some(components) = &ui.borrow().components_window {
                        let Some(target) = ui.borrow().partition_material_target.clone() else {
                            return;
                        };
                        if let Err(error) = components_ui::choose_partition_material(
                            components,
                            &mut session,
                            &target,
                            Some(material.as_str()),
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_extrusion {
                    if let Some(components) = &ui.borrow().components_window {
                        let Some(target) = ui.borrow().extrusion_material_target else {
                            return;
                        };
                        if let Err(error) = components_ui::choose_extrusion_material(
                            components,
                            &mut session,
                            target,
                            &material,
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_group {
                    if let Some(controller) = &ui.borrow().surface_group {
                        controller.choose_material(&mut session, &material);
                    }
                } else if ui.borrow().material_for_faces {
                    if let Some(surface) = &ui.borrow().surface_window {
                        surface.set_material(material.clone());
                        surface.set_preview(
                            session
                                .material_preview(&material)
                                .ok()
                                .map(|preview| preview_image(&preview))
                                .unwrap_or_default(),
                        );
                    }
                } else {
                    app.set_material(material);
                    refresh_material_preview(&app, &mut session);
                }
                drop(session);
                let _ = window.hide();
            }
        });
        ui.borrow_mut().material_window = Some(window);
    }
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let overlay_target = if key == "@overlay" {
        ui.borrow()
            .overlay
            .as_ref()
            .map(|controller| controller.material_target(&session))
    } else {
        None
    };
    ui.borrow_mut().overlay_material_target = overlay_target;
    ui.borrow_mut().material_for_replace = match key {
        "@replace-source" => Some(false),
        "@replace-target" => Some(true),
        _ => None,
    };
    ui.borrow_mut().material_for_group = key == "@group";
    ui.borrow_mut().material_for_faces = key == "@faces";
    ui.borrow_mut().material_for_clip = key == "@clip";
    ui.borrow_mut().material_for_primitive = key == "@primitive";
    ui.borrow_mut().material_for_polygon = key == "@polygon";
    ui.borrow_mut().material_for_extrusion = key == "@extrusion";
    ui.borrow_mut().material_for_partition = key == "@partition";
    ui.borrow_mut().partition_material_target =
        (key == "@partition").then(|| components_ui::partition_material_target(&session));
    ui.borrow_mut().extrusion_material_target = (key == "@extrusion").then(|| {
        (
            session.partition_preview_token(),
            session.document_generation(),
        )
    });
    ui.borrow_mut().primitive_material_target =
        (key == "@primitive" || key == "@polygon").then(|| {
            (
                session.primitive_preview().map(|preview| preview.token),
                session.document_generation(),
            )
        });
    let target = if key.is_empty()
        || key == "@faces"
        || key == "@overlay"
        || key == "@clip"
        || key == "@primitive"
        || key == "@polygon"
        || key == "@partition"
        || key == "@extrusion"
        || key.starts_with("@replace-")
        || key == "@group"
    {
        None
    } else {
        single_selected_entity(&session)
            .map(|id| (id, key.to_owned(), session.document_generation()))
    };
    let material = if let Some(replacement) = ui.borrow().material_for_replace {
        ui.borrow()
            .material_replace
            .as_ref()
            .map(|controller| {
                if replacement {
                    controller.window.get_replacement().to_string()
                } else {
                    controller.window.get_source().to_string()
                }
            })
            .unwrap_or_default()
    } else if key == "@overlay" {
        ui.borrow()
            .overlay
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@partition" {
        ui.borrow()
            .components_window
            .as_ref()
            .map(|window| window.get_top_material().to_string())
            .unwrap_or_default()
    } else if key == "@extrusion" {
        ui.borrow()
            .components_window
            .as_ref()
            .map(|window| window.get_wall_material().to_string())
            .unwrap_or_default()
    } else if key == "@primitive" {
        ui.borrow()
            .primitive_window
            .as_ref()
            .map(|window| window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@polygon" {
        ui.borrow()
            .polygon
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@group" {
        ui.borrow()
            .surface_group
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@clip" {
        app.get_clip_material().to_string()
    } else if key == "@faces" {
        ui.borrow()
            .surface_window
            .as_ref()
            .map(|window| window.get_material().to_string())
            .unwrap_or_default()
    } else {
        target
            .as_ref()
            .and_then(|(id, key, _)| {
                session
                    .entity_inspector(*id)
                    .ok()?
                    .properties
                    .into_iter()
                    .find(|field| field.key == *key)
                    .map(|field| field.value)
            })
            .unwrap_or_else(|| app.get_material().to_string())
    };
    ui.borrow_mut().material_target = target;
    let window = ui.borrow().material_window.as_ref().unwrap().clone_strong();
    window.set_filter("".into());
    window.set_material(material.into());
    window.set_group("all".into());
    window.set_offset(0);
    refresh_material_catalog(&window, &mut session);
    refresh_material_window(&window, &mut session);
    drop(session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}
