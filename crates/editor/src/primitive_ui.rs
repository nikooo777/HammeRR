use glam::DVec3;
use map_core::primitive::{Axis, Shape, Spec};
use slint::ComponentHandle;
use source_formats::vmf::format_number;

use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{PrimitiveWindow, preview_image};

fn number(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| "Enter finite numeric dimensions and angles".into())
}

fn dimension(value: f64) -> String {
    format!("{value:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .into()
}

fn spec(window: &PrimitiveWindow) -> Result<Spec, String> {
    let mut spec = Spec {
        shape: match window.get_shape() {
            0 => Shape::Wedge,
            1 => Shape::Cylinder,
            2 => Shape::Cone,
            3 => Shape::Sphere,
            4 => Shape::Arch,
            _ => return Err("Choose a shape".into()),
        },
        axis: match window.get_axis() {
            0 => Axis::X,
            1 => Axis::Y,
            2 => Axis::Z,
            _ => return Err("Choose an axis".into()),
        },
        mins: DVec3::new(
            number(&window.get_min_x())?,
            number(&window.get_min_y())?,
            number(&window.get_min_z())?,
        ),
        maxs: DVec3::new(
            number(&window.get_max_x())?,
            number(&window.get_max_y())?,
            number(&window.get_max_z())?,
        ),
        segments: window.get_segments().max(0) as u32,
        bands: window.get_bands().max(0) as u32,
        material: window.get_material().into(),
        ..Spec::default()
    };
    if spec.shape == Shape::Arch {
        spec.inset = number(&window.get_inset())?;
        spec.start_angle = number(&window.get_start_angle())?;
        spec.sweep_angle = number(&window.get_sweep_angle())?;
    }
    spec.validate().map_err(|error| error.to_string())?;
    Ok(spec)
}

fn load(window: &PrimitiveWindow, spec: &Spec) {
    window.set_shape(match spec.shape {
        Shape::Wedge => 0,
        Shape::Cylinder => 1,
        Shape::Cone => 2,
        Shape::Sphere => 3,
        Shape::Arch => 4,
        Shape::Polygon => unreachable!("Polygons have their own drawing controls"),
    });
    window.set_axis(match spec.axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    });
    window.set_segments(spec.segments as i32);
    window.set_bands(spec.bands as i32);
    window.set_min_x(format_number(spec.mins.x).into());
    window.set_min_y(format_number(spec.mins.y).into());
    window.set_min_z(format_number(spec.mins.z).into());
    window.set_max_x(format_number(spec.maxs.x).into());
    window.set_max_y(format_number(spec.maxs.y).into());
    window.set_max_z(format_number(spec.maxs.z).into());
    window.set_inset(format_number(spec.inset).into());
    window.set_start_angle(format_number(spec.start_angle).into());
    window.set_sweep_angle(format_number(spec.sweep_angle).into());
    window.set_material(spec.material.clone().into());
}

pub fn refresh(window: &PrimitiveWindow, session: &mut Session) {
    if session
        .primitive_preview()
        .is_some_and(|preview| preview.spec.shape == Shape::Polygon)
    {
        window.set_token("".into());
        window.set_can_preview(false);
        window.set_summary("A polygon preview is active. Open Draw polygon… to review it.".into());
        return;
    }
    if let Some(preview) = session.primitive_preview() {
        let token = preview.token.to_string();
        if window.get_token() != token {
            load(window, &preview.spec);
        }
        window.set_token(token.into());
        let faces = preview
            .ids
            .iter()
            .filter_map(|id| {
                let map_core::ObjectId::Solid(id) = id else {
                    return None;
                };
                Some(preview.document.solid(*id)?.sides.len())
            })
            .sum::<usize>();
        window.set_summary(
            format!(
                "Preview: {} {} · {faces} faces",
                preview.ids.len(),
                if preview.ids.len() == 1 {
                    "brush"
                } else {
                    "brushes"
                }
            )
            .into(),
        );
        let size = preview.bounds[1] - preview.bounds[0];
        window.set_actual_bounds(
            format!(
                "Actual size: {} × {} × {} units · X, Y, Z",
                dimension(size.x),
                dimension(size.y),
                dimension(size.z)
            )
            .into(),
        );
    } else {
        window.set_token("".into());
        window.set_summary("Preview the shape to see it in the map.".into());
        window.set_actual_bounds("".into());
    }
    let feedback = spec(window);
    window.set_can_preview(feedback.is_ok());
    window.set_feedback(feedback.err().unwrap_or_default().into());
    window.set_material_image(
        session
            .material_preview(&window.get_material())
            .ok()
            .map(|image| preview_image(&image))
            .unwrap_or_default(),
    );
}

pub fn initialize(window: &PrimitiveWindow, session: &mut Session, material: &str) {
    if session.primitive_preview().is_none() {
        let mut spec = Spec {
            material: material.into(),
            ..Spec::default()
        };
        if let Some((mins, maxs)) = session.selection_bounds()
            && (maxs - mins)
                .cmpge(DVec3::splat(map_core::primitive::MIN_SIZE))
                .all()
        {
            spec.mins = mins;
            spec.maxs = maxs;
        }
        load(window, &spec);
    }
    window.set_message("".into());
    refresh(window, session);
}

fn token(window: &PrimitiveWindow) -> Result<u64, String> {
    window
        .get_token()
        .parse()
        .map_err(|_| "Preview the shape first".into())
}

pub fn fields_changed(window: &PrimitiveWindow, session: &mut Session) {
    let result = if window.get_token().is_empty() || session.primitive_preview().is_none() {
        Ok(())
    } else {
        token(window).and_then(|token| session.cancel_primitive_preview(token))
    };
    window.set_message(result.err().unwrap_or_default().into());
    refresh(window, session);
}

pub fn choose_material(
    window: &PrimitiveWindow,
    session: &mut Session,
    target: (Option<u64>, u64),
    material: &str,
) -> Result<(), String> {
    if session.document_generation() != target.1
        || session.primitive_preview().map(|preview| preview.token) != target.0
    {
        return Err("Shape preview or map changed. Open the material picker again.".into());
    }
    window.set_material(material.into());
    fields_changed(window, session);
    Ok(())
}

fn cancel(window: &PrimitiveWindow, shared: &SharedSession) -> bool {
    let mut session = lock(shared);
    if session.primitive_preview().is_none() || window.get_token().is_empty() {
        return true;
    }
    let result = token(window).and_then(|token| session.cancel_primitive_preview(token));
    let okay = result.is_ok();
    window.set_message(result.err().unwrap_or_default().into());
    refresh(window, &mut session);
    okay
}

pub fn bind(window: &PrimitiveWindow, shared: &SharedSession) {
    window.on_edited({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || fields_changed(&window.unwrap(), &mut lock(&shared))
    });
    window.on_preview_shape({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result =
                spec(&window).and_then(|spec| session.create_primitive(Actor::User, spec, true));
            window.set_message(result.err().unwrap_or_default().into());
            refresh(&window, &mut session);
        }
    });
    window.on_apply({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = token(&window)
                .and_then(|token| session.commit_primitive_preview(Actor::User, token));
            let okay = result.is_ok();
            window.set_message(result.err().unwrap_or_default().into());
            refresh(&window, &mut session);
            if okay {
                let _ = window.hide();
            }
        }
    });
    window.on_focus_preview({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = token(&window)
                .and_then(|token| session.frame_primitive_preview(token, window.get_aspect()));
            window.set_message(result.err().unwrap_or_default().into());
            refresh(&window, &mut session);
        }
    });
    window.on_cancel({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if cancel(&window, &shared) {
                let _ = window.hide();
            }
        }
    });
    window.window().on_close_requested({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            if cancel(&window.unwrap(), &shared) {
                slint::CloseRequestResponse::HideWindow
            } else {
                slint::CloseRequestResponse::KeepWindowShown
            }
        }
    });
}
