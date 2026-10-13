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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winding::Winding;

    fn tetrahedron(size: f64) -> (BrushGeometry, Vec<MapSide>, PlaneSet) {
        let authored = [
            map_core::Plane {
                normal: DVec3::NEG_X,
                dist: 0.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_Y,
                dist: 0.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_Z,
                dist: 0.0,
            },
            map_core::Plane {
                normal: DVec3::ONE.normalize(),
                dist: size / 3.0f64.sqrt(),
            },
        ];
        let geometry = BrushGeometry::from_planes(&authored).unwrap();
        let mut planes = PlaneSet::default();
        let sides = geometry
            .faces
            .iter()
            .map(|face| MapSide {
                source_id: None,
                plane: planes.find(face.plane.normal, face.plane.dist),
                material: face.side,
                texinfo: face.side,
                smoothing_groups: 0,
                winding: Some(Winding(face.vertices.clone())),
                bevel: false,
                visible: true,
            })
            .collect();
        (geometry, sides, planes)
    }

    #[test]
    fn short_edges_and_duplicate_points_produce_finite_supporting_bevels() {
        let (mut geometry, mut sides, mut planes) = tetrahedron(0.125);
        let point = geometry.faces[0].vertices[0];
        geometry.faces[0].vertices.insert(0, point);
        add(
            &mut sides,
            &mut planes,
            &geometry,
            MAX_BRUSH_SIDES,
            &Control::default(),
        )
        .unwrap();
        assert_eq!(sides.len(), 10);
        for side in sides.iter().filter(|side| side.bevel) {
            let plane = planes.get(side.plane);
            assert!(plane.normal.is_finite() && plane.dist.is_finite());
            assert!(
                support_distance(&geometry, plane.normal, &Control::default()).unwrap()
                    - plane.dist
                    <= ON_EPSILON
            );
        }
    }

    #[test]
    fn cancellation_prevents_bevel_work_and_encoded_side_limits_return_errors() {
        let (geometry, mut sides, mut planes) = tetrahedron(64.0);
        let control = Control::default();
        control.cancel().unwrap();
        let side_count = sides.len();
        let plane_count = planes.len();
        assert!(matches!(
            add(
                &mut sides,
                &mut planes,
                &geometry,
                MAX_BRUSH_SIDES,
                &control
            ),
            Err(Error::Cancelled(_))
        ));
        assert_eq!(sides.len(), side_count);
        assert_eq!(planes.len(), plane_count);
        assert!(matches!(
            add(
                &mut sides,
                &mut planes,
                &geometry,
                side_count,
                &Control::default()
            ),
            Err(Error::Limit(_))
        ));
        assert_eq!(sides.len(), side_count);
    }

    #[test]
    fn plane_index_limit_allows_reuse_but_rejects_new_pairs() {
        let mut planes = PlaneSet::default();
        for dist in 0..MAX_PLANES / 2 {
            planes.find(DVec3::X, dist as f64);
        }
        assert_eq!(planes.len(), MAX_PLANES);
        let (geometry, _, _) = tetrahedron(32.0);
        assert!(
            support::find_axial(&mut planes, &geometry, DVec3::X, 32.0, &Control::default())
                .unwrap()
                .is_some()
        );
        assert!(check_plane_limit(&planes).is_ok());
        support::find_axial(&mut planes, &geometry, DVec3::Y, 32.0, &Control::default())
            .unwrap()
            .unwrap();
        assert!(matches!(check_plane_limit(&planes), Err(Error::Limit(_))));
    }
}
