use super::super::*;

pub(super) fn report(
    session: &Session,
    args: HandlesArgs,
    guide: Option<render::handles::Guide>,
) -> Result<Value, String> {
    let size = render_size(args.width, args.height);
    let camera = args
        .camera
        .unwrap_or_default()
        .resolve(session, size.0 as f32 / size.1 as f32)?;
    let shapes = guide.map_or_else(Vec::new, |guide| {
        render::handles::shapes(&camera, size, &guide)
    });
    let picked = if let (Some(guide), Some(point)) = (guide, args.point) {
        let point = glam::Vec2::new(point[0] as f32, point[1] as f32);
        if !point.is_finite() {
            return Err("Handle point must be finite".into());
        }
        render::handles::pick(&camera, size, &guide, point).map(render::handles::Handle::name)
    } else {
        None
    };
    Ok(json!({
        "camera": camera_json(&camera),
        "width": size.0,
        "height": size.1,
        "picked": picked,
        "origin": guide.map(|guide| guide.origin.to_array()),
        "handles": shapes.iter().map(|shape| json!({
            "handle": shape.handle.name(),
            "color": shape.color,
            "segments": shape.segments.iter().map(|points| points.map(|point| point.to_array())).collect::<Vec<_>>(),
            "pixel_segments": shape.segments.iter().filter_map(|points| Some([
                render::handles::project(&camera, points[0], size)?.to_array(),
                render::handles::project(&camera, points[1], size)?.to_array(),
            ])).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests;
