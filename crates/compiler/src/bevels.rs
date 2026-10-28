use glam::DVec3;
use map_core::brush::BrushGeometry;

use super::MapSide;
use super::support;
use super::support::{distance as support_distance, emitted_distance};
use crate::Control;
use crate::planes::{NORMAL_EPSILON, PlaneSet};
use crate::winding::ON_EPSILON;

pub(super) const MAX_PLANES: usize = u16::MAX as usize + 1;
pub(super) const MAX_BRUSH_SIDES: usize = i32::MAX as usize;

#[derive(Debug)]
pub(super) enum Error {
    Cancelled(String),
    Limit(&'static str),
    Invalid(&'static str),
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Cancelled(message)
    }
}

fn check_plane_limit(planes: &PlaneSet) -> Result<(), Error> {
    if planes.len() > MAX_PLANES {
        return Err(Error::Limit(
            "BSP collision plane limit exceeded; reduce map geometry",
        ));
    }
    Ok(())
}

fn append_side(sides: &mut Vec<MapSide>, plane: usize, side_budget: usize) -> Result<(), Error> {
    if sides.len() >= side_budget {
        return Err(Error::Limit(
            "BSP collision brush side limit exceeded; reduce map geometry",
        ));
    }
    let first = &sides[0];
    sides.push(MapSide {
        source_id: None,
        plane,
        material: first.material,
        texinfo: first.texinfo,
        smoothing_groups: 0,
        winding: None,
        bevel: true,
        visible: false,
    });
    Ok(())
}

pub(super) fn add(
    sides: &mut Vec<MapSide>,
    planes: &mut PlaneSet,
    geometry: &BrushGeometry,
    side_budget: usize,
    control: &Control,
) -> Result<(), Error> {
    control.checkpoint()?;
    for axis in 0..3 {
        for sign in [1.0, -1.0] {
            let mut normal = DVec3::ZERO;
            normal[axis] = sign;
            if sides.iter().any(|s| planes.get(s.plane).normal == normal) {
                continue;
            }
            let dist = if sign > 0.0 {
                geometry.maxs[axis]
            } else {
                -geometry.mins[axis]
            };
            check_plane_limit(planes)?;
            let Some(plane) = support::find_axial(planes, geometry, normal, dist, control)? else {
                return Err(Error::Invalid(
                    "cannot preserve axial BSP plane support; simplify this brush",
                ));
            };
            check_plane_limit(planes)?;
            append_side(sides, plane, side_budget)?;
        }
    }
    if geometry.faces.len() == 6
        && geometry
            .faces
            .iter()
            .all(|face| [DVec3::X, DVec3::Y, DVec3::Z].contains(&face.plane.normal.abs()))
    {
        return Ok(());
    }

    for face in &geometry.faces {
        for (index, &point) in face.vertices.iter().enumerate() {
            control.checkpoint()?;
            let next = face.vertices[(index + 1) % face.vertices.len()];
            let Some(edge) = (next - point).try_normalize() else {
                continue;
            };
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                let Some(normal) = edge.cross(axis).try_normalize() else {
                    continue;
                };
                for normal in [normal, -normal] {
                    if sides.iter().any(|side| {
                        let plane = planes.get(side.plane);
                        plane.kind < 3
                            && (plane.normal - normal).abs().max_element() < NORMAL_EPSILON
                            && plane.distance(point).abs() <= ON_EPSILON
                            && plane.distance(next).abs() <= ON_EPSILON
                    }) {
                        continue;
                    }
                    let dist = normal.dot(point);
                    if !normal.is_finite() || !dist.is_finite() {
                        continue;
                    }
                    let support = support_distance(geometry, normal, control)?;
                    if support - dist > ON_EPSILON {
                        continue;
                    }
                    let dist = emitted_distance(normal, support, geometry, control)?;
                    let plane = planes.find_checked(normal, dist, |candidate| {
                        if candidate.kind < 3 {
                            return Ok(false);
                        }
                        Ok::<_, Error>(support::accepts(geometry, candidate, control)?)
                    })?;
                    let Some(plane) = plane else {
                        continue;
                    };
                    check_plane_limit(planes)?;
                    if sides.iter().any(|side| side.plane == plane) {
                        continue;
                    }
                    append_side(sides, plane, side_budget)?;
                }
            }
        }
    }
    Ok(())
}
