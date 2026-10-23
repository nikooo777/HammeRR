use super::*;

pub(super) fn refresh_spatial(app: &AppWindow, session: &Session) {
    refresh_interaction(app, session);
    refresh_clip(app, session);
    let settings = session.spatial();
    grid_defaults_ui::refresh_status(app, session);
    pivot_defaults_ui::refresh_status(app, session);
    let projection = session.camera().projection;
    app.set_projection_index(projection.index() as i32);
    app.set_workplane_axis(
        match projection {
            Projection::Front => "Y",
            Projection::Side => "X",
            _ => "Z",
        }
        .into(),
    );
    app.set_grid_size(settings.grid_size.to_string().into());
    app.set_grid_visible(settings.grid_visible);
    app.set_snap_grid(settings.snap);
    app.set_drawing_block(settings.draw_block);
    app.set_brush_elevation(settings.elevation.to_string().into());
    app.set_brush_depth(settings.depth.to_string().into());
    app.set_brush_preview(
        session
            .block_preview()
            .map(|(mins, maxs)| {
                let size = maxs - mins;
                format!(
                    "{} × {} × {} units · release to place · Esc cancels",
                    size.x, size.y, size.z
                )
            })
            .unwrap_or_default()
            .into(),
    );
}

pub(super) fn spatial_changed(app: &AppWindow, ui: &SharedUi) {
    let result = (|| {
        let grid_size = app
            .get_grid_size()
            .parse()
            .map_err(|_| "Enter a valid grid spacing")?;
        let elevation = app
            .get_brush_elevation()
            .parse()
            .map_err(|_| "Enter a valid start coordinate")?;
        let depth = app
            .get_brush_depth()
            .parse()
            .map_err(|_| "Enter a valid brush depth")?;
        lock(&session_of(ui)).set_spatial(
            Actor::User,
            spatial::Update {
                grid_size: Some(grid_size),
                grid_visible: Some(app.get_grid_visible()),
                snap: Some(app.get_snap_grid()),
                elevation: Some(elevation),
                depth: Some(depth),
                ..spatial::Update::default()
            },
        )
    })();
    if let Err(message) = result {
        app.set_message(message.into());
        refresh_spatial(app, &lock(&session_of(ui)));
    }
}

pub(super) fn frame_selection(app: &AppWindow, ui: &SharedUi) {
    let aspect = app.get_viewport_width() / app.get_viewport_height().max(1.0);
    if let Err(message) = lock(&session_of(ui)).frame_selection(Actor::User, aspect) {
        app.set_message(message.into());
    }
}

pub(super) fn plane_coordinates(
    camera: render::Camera,
    position: (f32, f32),
    size: (f32, f32),
    elevation: f64,
) -> Option<DVec2> {
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return None;
    }
    let ndc = (
        position.0 * 2.0 / size.0 - 1.0,
        1.0 - position.1 * 2.0 / size.1,
    );
    let point = camera
        .plane_point(ndc, size.0 / size.1, elevation as f32)?
        .as_dvec3();
    let (horizontal, vertical, _) = camera.projection.axes()?;
    Some(DVec2::new(point[horizontal], point[vertical]))
}

pub(super) fn drag_bounds(
    session: &Session,
    drag: BrushDrag,
    end: DVec2,
) -> Result<(DVec3, DVec3), String> {
    if drag.generation != session.document_generation()
        || drag.revision != session.document().revision()
        || drag.projection != session.camera().projection
        || drag.epoch != session.brush_epoch()
    {
        return Err("The brush drag was cancelled or its map/settings changed".into());
    }
    session.drawn_block_bounds(drag.projection, drag.start, end, None, None)
}

pub(super) fn create_block(app: &AppWindow, ui: &SharedUi) {
    let ui = ui.borrow();
    let camera = ui.viewport.camera;
    let mut session = lock(&ui.session);
    if camera.projection != Projection::Perspective {
        let drawing = !session.spatial().draw_block;
        if let Err(message) = session.set_spatial(
            Actor::User,
            spatial::Update {
                draw_block: Some(drawing),
                ..spatial::Update::default()
            },
        ) {
            app.set_message(message.into());
        }
        return;
    }
    let (origin, direction) = camera.ray(0.0, 0.0, 1.0);
    let ray = Ray {
        origin: origin.as_dvec3(),
        direction: direction.as_dvec3(),
    };
    let half = BLOCK_SIZE / 2.0;
    let (mins, maxs) = match session.pick(&ray) {
        Some(hit) => {
            let base = session.spatial().snap_point(hit.point);
            (
                base - DVec3::new(half, half, 0.0),
                base + DVec3::new(half, half, BLOCK_SIZE),
            )
        }
        None => {
            let p = ray.origin + ray.direction * 384.0;
            let center = session.spatial().snap_point(p);
            (center - DVec3::splat(half), center + DVec3::splat(half))
        }
    };
    let material = app.get_material().to_string();
    let result = session.create_block(Actor::User, mins, maxs, &material, true);
    if let Err(message) = result {
        app.set_message(message.into());
    }
}
