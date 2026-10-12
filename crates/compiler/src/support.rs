use glam::DVec3;
use map_core::brush::{BrushGeometry, Face};

use crate::Control;
use crate::emit::stored_vertex;
use crate::planes::{Plane, PlaneSet};
use crate::winding::ON_EPSILON;

const CHECKPOINT_POINT_INTERVAL: usize = 256;

pub(super) fn distance(
    geometry: &BrushGeometry,
    normal: DVec3,
    control: &Control,
) -> Result<f64, String> {
    let mut dist = f64::MIN;
    for face in &geometry.faces {
        control.checkpoint()?;
        for (index, &point) in face.vertices.iter().enumerate() {
            if index % CHECKPOINT_POINT_INTERVAL == 0 {
                control.checkpoint()?;
            }
            dist = dist.max(normal.dot(point));
        }
    }
    Ok(dist)
}

fn emitted_support(
    geometry: &BrushGeometry,
    normal: DVec3,
    control: &Control,
) -> Result<[f64; 2], String> {
    let mut dist = [f64::MIN; 2];
    for face in &geometry.faces {
        control.checkpoint()?;
        for (index, &point) in face.vertices.iter().enumerate() {
            if index % CHECKPOINT_POINT_INTERVAL == 0 {
                control.checkpoint()?;
            }
            dist[0] = dist[0].max(normal.dot(point));
            dist[1] = dist[1].max(normal.dot(stored_vertex(point)));
        }
    }
    Ok(dist)
}

pub(super) fn emitted_distance(
    normal: DVec3,
    support: f64,
    geometry: &BrushGeometry,
    control: &Control,
) -> Result<f64, String> {
    let emitted_normal = normal.as_vec3().as_dvec3();
    let emitted = emitted_support(geometry, emitted_normal, control)?;
    let boundary = support.max(emitted[0]).max(emitted[1]);
    let rounded = boundary as f32;
    Ok(f64::from(if f64::from(rounded) < boundary {
        rounded.next_up()
    } else {
        rounded
    }))
}

pub(super) fn accepts(
    geometry: &BrushGeometry,
    plane: &Plane,
    control: &Control,
) -> Result<bool, String> {
    let support = distance(geometry, plane.normal, control)?;
    let emitted = emitted_support(geometry, plane.normal.as_vec3().as_dvec3(), control)?;
    Ok((support - plane.dist).abs() <= ON_EPSILON
        && emitted
            .into_iter()
            .all(|boundary| (boundary - f64::from(plane.dist as f32)).abs() <= ON_EPSILON))
}

fn find_supporting(
    planes: &mut PlaneSet,
    geometry: &BrushGeometry,
    normal: DVec3,
    dist: f64,
    control: &Control,
    accept: impl Fn(&Plane) -> Result<bool, String>,
) -> Result<Option<usize>, String> {
    let checked = |plane: &Plane| Ok(accept(plane)? && accepts(geometry, plane, control)?);
    let plane = planes.find_surface_checked(normal, dist, checked)?;
    if plane.is_some() {
        return Ok(plane);
    }
    let support = distance(geometry, normal, control)?;
    let dist = emitted_distance(normal, support, geometry, control)?;
    planes.find_surface_checked(normal, dist, checked)
}

pub(super) fn find_authored(
    planes: &mut PlaneSet,
    geometry: &BrushGeometry,
    face: &Face,
    control: &Control,
) -> Result<Option<usize>, String> {
    find_supporting(
        planes,
        geometry,
        face.plane.normal,
        face.plane.dist,
        control,
        |plane| {
            let emitted_normal = plane.normal.as_vec3().as_dvec3();
            let emitted_dist = f64::from(plane.dist as f32);
            for (index, &point) in face.vertices.iter().enumerate() {
                if index % CHECKPOINT_POINT_INTERVAL == 0 {
                    control.checkpoint()?;
                }
                if plane.distance(point).abs() > ON_EPSILON
                    || (emitted_normal.dot(point) - emitted_dist).abs() > ON_EPSILON
                    || (emitted_normal.dot(stored_vertex(point)) - emitted_dist).abs() > ON_EPSILON
                {
                    return Ok(false);
                }
            }
            Ok(true)
        },
    )
}

pub(super) fn find_axial(
    planes: &mut PlaneSet,
    geometry: &BrushGeometry,
    normal: DVec3,
    dist: f64,
    control: &Control,
) -> Result<Option<usize>, String> {
    find_supporting(planes, geometry, normal, dist, control, |plane| {
        Ok(plane.normal == normal)
    })
}
