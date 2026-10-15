use super::*;
use map_core::displacement::DisplacementMesh;
use map_core::surface::FaceId;

const MAX_WORK: usize = 1_000_000;
const MAX_TRIANGLES: usize = source_formats::bsp::MAX_DISPLACEMENT_TRIANGLES;
const GEOMETRY_TOLERANCE: f64 = 0.02;

#[derive(Default)]
pub(super) struct Budget {
    work: usize,
    triangles: usize,
    exhausted: bool,
}

impl Budget {
    fn consume(&mut self, amount: usize, report: &mut Report) -> bool {
        if self.exhausted {
            return false;
        }
        self.work = self.work.saturating_add(amount);
        if self.work > MAX_WORK || self.triangles > MAX_TRIANGLES {
            self.exhausted = true;
            report.add(Severity::Warning,"spawn_clearance_unchecked",&Location::default(),"Terrain spawn clearance exceeded its bounded work/triangle limit. Remaining terrain was not checked.",None);
            return false;
        }
        true
    }
}

fn unchecked(location: &Location, report: &mut Report, reason: impl std::fmt::Display) {
    report.add(
        Severity::Warning,
        "spawn_clearance_unchecked",
        location,
        format!("Terrain near a spawn was excluded from clearance: {reason}."),
        None,
    );
}

pub(super) fn inspect(
    solid: &Solid,
    location: Location,
    materials: &mut Materials<'_>,
    spawns: &mut [Spawn],
    report: &mut Report,
    budget: &mut Budget,
) {
    if budget.exhausted {
        return;
    }
    let geometry = match checked_geometry(solid) {
        Ok(geometry) => geometry,
        Err(error) => {
            unchecked(&location, report, error);
            return;
        }
    };
    for face in &geometry.faces {
        let side = &solid.sides[face.side];
        let Some(info) = &side.displacement else {
            continue;
        };
        if info.flags.unwrap_or(0) & 4 != 0 {
            continue;
        }
        let mesh = match DisplacementMesh::build(face, info) {
            Ok(mesh) => mesh,
            Err(error) => {
                unchecked(&location, report, error);
                continue;
            }
        };
        if mesh
            .positions
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > SOURCE_COORDINATE_LIMIT)
        {
            unchecked(
                &location,
                report,
                "positions are not finite and within Source bounds",
            );
            continue;
        }
        let mins = mesh
            .positions
            .iter()
            .copied()
            .fold(DVec3::splat(f64::INFINITY), DVec3::min);
        let maxs = mesh
            .positions
            .iter()
            .copied()
            .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
        let mut candidates = Vec::new();
        for (index, spawn) in spawns.iter().enumerate() {
            if !budget.consume(1, report) {
                return;
            }
            if overlaps(mins, maxs, spawn.mins, spawn.maxs) {
                candidates.push(index);
            }
        }
        if candidates.is_empty() {
            continue;
        }
        let mut location = location.clone();
        location.face = Some(FaceId {
            solid: solid.id,
            side: side.id,
        });
        location.position = Some((mins + maxs) * 0.5);
        if info.flags.unwrap_or(0) & !14 != 0
            || info.normals.is_some() != info.distances.is_some()
            || !info.start_position.is_finite()
            || face
                .vertices
                .iter()
                .all(|point| point.distance(info.start_position) > GEOMETRY_TOLERANCE)
            || info.triangle_tags.as_ref().is_some_and(|rows| {
                rows.len() != mesh.size - 1
                    || rows.iter().any(|row| {
                        row.len() != (mesh.size - 1) * 2
                            || row.iter().any(|&tag| {
                                u16::try_from(tag).is_err()
                                    || tag & i32::from(map_core::displacement::triangles::REMOVE)
                                        != 0
                            })
                    })
            })
        {
            unchecked(
                &location,
                report,
                "unsupported flags, removed triangles or malformed source geometry",
            );
            continue;
        }
        let semantics = match materials.semantics(&side.material) {
            Ok(semantics) => semantics,
            Err(error) => {
                unchecked(&location, report, error);
                continue;
            }
        };
        if semantics.contents & PLAYER_BLOCKING_CONTENTS == 0 {
            continue;
        }
        if !semantics.draws
            || semantics.contents & !contents::DETAIL != contents::SOLID
            || semantics.surface & (surf::SKY | surf::SKY2D | surf::WARP) != 0
        {
            unchecked(
                &location,
                report,
                "native terrain needs a visible ordinary solid material",
            );
            continue;
        }
        let triangles = mesh.triangles().collect::<Vec<_>>();
        budget.triangles = budget.triangles.saturating_add(triangles.len());
        if !budget.consume(1, report) {
            return;
        }
        if triangles.iter().any(|&indices| {
            let [a, b, c] = indices.map(|index| mesh.positions[index]);
            (b - a).cross(c - a).length_squared() < AXIS_TOLERANCE
        }) {
            unchecked(&location, report, "collapsed surface triangles");
            continue;
        }
        let mut blocked = vec![false; spawns.len()];
        for indices in triangles {
            let points = indices.map(|index| mesh.positions[index]);
            let low = points
                .into_iter()
                .fold(DVec3::splat(f64::INFINITY), DVec3::min);
            let high = points
                .into_iter()
                .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
            let mut projections = None;
            for &index in &candidates {
                if blocked[index] {
                    continue;
                }
                if !budget.consume(1, report) {
                    return;
                }
                let spawn = &spawns[index];
                if !overlaps(low, high, spawn.mins, spawn.maxs) {
                    continue;
                }
                let projections = projections.get_or_insert_with(|| triangle_projections(points));
                if !budget.consume(projections.len(), report) {
                    return;
                }
                if intersects_box(projections, spawn.mins, spawn.maxs) {
                    blocked[index] = true;
                }
            }
        }
        for &index in &candidates {
            if !blocked[index] {
                continue;
            }
            let spawn = &mut spawns[index];
            spawn.blocker_count += 1;
            if spawn.blockers.len() < MAX_BLOCKER_NAMES {
                spawn.blockers.push(format!(
                    "terrain face {} on brush {} ({})",
                    side.id, solid.id, location.path
                ));
            }
        }
    }
}

fn overlaps(mins: DVec3, maxs: DVec3, other_mins: DVec3, other_maxs: DVec3) -> bool {
    mins.cmplt(other_maxs - DVec3::splat(CONTACT_TOLERANCE))
        .all()
        && maxs
            .cmpgt(other_mins + DVec3::splat(CONTACT_TOLERANCE))
            .all()
}

fn triangle_projections(points: [DVec3; 3]) -> Vec<Projection> {
    let mut axes = vec![DVec3::X, DVec3::Y, DVec3::Z];
    add_axis(
        &mut axes,
        (points[1] - points[0]).cross(points[2] - points[0]),
    );
    for index in 0..3 {
        let edge = points[(index + 1) % 3] - points[index];
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            add_axis(&mut axes, edge.cross(axis));
        }
    }
    axes.into_iter()
        .map(|axis| {
            let (min, max) = points
                .into_iter()
                .map(|point| point.dot(axis))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), v| {
                    (low.min(v), high.max(v))
                });
            Projection { axis, min, max }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_box_axes_exclude_oblique_contact_and_budget_limits_are_reported_once() {
        let a = [
            DVec3::new(1.5, 0.5, 0.),
            DVec3::new(0.5, 1.5, 0.),
            DVec3::new(1.5, 1.5, 0.),
        ];
        assert!(!intersects_box(
            &triangle_projections(a),
            -DVec3::ONE,
            DVec3::ONE
        ));
        let b = a.map(|point| point - DVec3::new(0.03, 0.03, 0.));
        assert!(intersects_box(
            &triangle_projections(b),
            -DVec3::ONE,
            DVec3::ONE
        ));
        let mut budget = Budget {
            work: MAX_WORK,
            ..Default::default()
        };
        let mut report = Report::default();
        assert!(!budget.consume(1, &mut report));
        assert!(!budget.consume(1, &mut report));
        assert_eq!(report.issues.len(), 1);
    }
}
