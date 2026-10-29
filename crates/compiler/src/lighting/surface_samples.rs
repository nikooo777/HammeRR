use glam::{DVec2, DVec3};
use source_formats::bsp::TexInfo;

use super::LuxelFrame;
use super::bake::{Allocation, Budget};

const PROJECTION_WORK: usize = 48;
const EDGE_WORK: usize = 24;
const CLIP_WORK: usize = 48;
const CENTROID_WORK: usize = 24;
const NEAREST_WORK: usize = 64;
const LIFT_WORK: usize = 16;
const CELL_RADIUS: f64 = 0.5;
const CLIP_PLANES: usize = 4;

pub(super) struct Face {
    frame: LuxelFrame,
    points: Vec<DVec3>,
    polygon: Vec<DVec2>,
    orientation: f64,
    _allocation: Allocation,
}

pub(super) struct Workspace {
    first: Vec<DVec2>,
    second: Vec<DVec2>,
    capacity: usize,
    _allocation: Allocation,
}

fn lift(frame: &LuxelFrame, point: DVec2) -> DVec3 {
    frame.origin + frame.axes[0] * point.x + frame.axes[1] * point.y
}

fn world_delta(frame: &LuxelFrame, delta: DVec2) -> DVec3 {
    frame.axes[0] * delta.x + frame.axes[1] * delta.y
}

fn push(points: &mut Vec<DVec2>, point: DVec2, capacity: usize) -> Result<(), String> {
    if !point.is_finite() {
        return Err("Covered luxel clipping produced a nonfinite point".into());
    }
    if points.last() == Some(&point) || points.first() == Some(&point) {
        return Ok(());
    }
    if points.len() >= capacity {
        return Err("Covered luxel clipping exceeded its prepared vertex bound".into());
    }
    points.push(point);
    Ok(())
}

impl Workspace {
    pub(super) fn new(max_vertices: usize, budget: &mut Budget<'_>) -> Result<Self, String> {
        let capacity = if max_vertices == 0 {
            0
        } else {
            max_vertices
                .checked_add(CLIP_PLANES)
                .ok_or("Covered luxel workspace vertex count overflow")?
        };
        let count = capacity
            .checked_mul(2)
            .ok_or("Covered luxel workspace allocation overflow")?;
        let allocation = budget.reserve::<DVec2>(count)?;
        let mut first = Vec::new();
        let mut second = Vec::new();
        first
            .try_reserve_exact(capacity)
            .map_err(|error| format!("Cannot allocate covered luxel workspace: {error}"))?;
        second
            .try_reserve_exact(capacity)
            .map_err(|error| format!("Cannot allocate covered luxel workspace: {error}"))?;
        Ok(Self {
            first,
            second,
            capacity,
            _allocation: allocation,
        })
    }
}

impl Face {
    pub(super) fn new(
        frame: LuxelFrame,
        points: Vec<DVec3>,
        info: &TexInfo,
        mins: [i32; 2],
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        budget.consume(PROJECTION_WORK)?;
        if points.len() < 3 {
            return Err("Covered luxel sampling needs a finite convex face".into());
        }
        let rows = info
            .lightmap_vecs
            .map(|row| DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])));
        if info
            .lightmap_vecs
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err("Covered luxel projection is nonfinite".into());
        }
        let allocation = budget.reserve::<DVec2>(points.len())?;
        let mut polygon = Vec::new();
        polygon
            .try_reserve_exact(points.len())
            .map_err(|error| format!("Cannot allocate covered luxel projection: {error}"))?;
        for &point in &points {
            budget.consume(PROJECTION_WORK)?;
            let projected = DVec2::new(
                rows[0].dot(point) + f64::from(info.lightmap_vecs[0][3]) - f64::from(mins[0]),
                rows[1].dot(point) + f64::from(info.lightmap_vecs[1][3]) - f64::from(mins[1]),
            );
            if !point.is_finite() || !projected.is_finite() || !lift(&frame, projected).is_finite()
            {
                return Err("Covered luxel face projection is nonfinite".into());
            }
            polygon.push(projected);
        }
        let anchor = polygon[0];
        let mut area = 0.0;
        for index in 1..polygon.len() - 1 {
            budget.consume(CENTROID_WORK)?;
            area += (polygon[index] - anchor).perp_dot(polygon[index + 1] - anchor);
        }
        if !area.is_finite() || area == 0.0 {
            return Err("Covered luxel face has degenerate projected area".into());
        }
        let orientation = area.signum();
        for index in 0..polygon.len() {
            budget.consume(EDGE_WORK)?;
            let a = polygon[index];
            let edge = polygon[(index + 1) % polygon.len()] - a;
            let length = world_delta(&frame, edge).length_squared();
            if !length.is_finite() || length <= 0.0 {
                return Err("Covered luxel face has a degenerate projected edge".into());
            }
            for (candidate, &point) in polygon.iter().enumerate() {
                budget.consume(EDGE_WORK)?;
                if candidate != index && point == a {
                    return Err("Covered luxel face has repeated projected vertices".into());
                }
                let distance = orientation * edge.perp_dot(point - a);
                if !distance.is_finite() || distance < 0.0 {
                    return Err("Covered luxel face projection is not convex".into());
                }
            }
        }
        Ok(Self {
            frame,
            points,
            polygon,
            orientation,
            _allocation: allocation,
        })
    }

    pub(super) fn frame(&self) -> &LuxelFrame {
        &self.frame
    }

    pub(super) fn points(&self) -> &[DVec3] {
        &self.points
    }

    pub(super) fn projected_points(&self) -> impl Iterator<Item = DVec3> + '_ {
        self.polygon.iter().map(|&point| lift(&self.frame, point))
    }

    pub(super) fn vertex_count(&self) -> usize {
        self.polygon.len()
    }

    fn covers(&self, center: DVec2, budget: &mut Budget<'_>) -> Result<bool, String> {
        for index in 0..self.polygon.len() {
            budget.consume(EDGE_WORK)?;
            let a = self.polygon[index];
            let edge = self.polygon[(index + 1) % self.polygon.len()] - a;
            let distance = self.orientation * edge.perp_dot(center - a);
            let radius = CELL_RADIUS * edge.abs().element_sum();
            if !distance.is_finite() || !radius.is_finite() {
                return Err("Covered luxel footprint has nonfinite support".into());
            }
            if distance < radius {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn nearest(&self, center: DVec2, budget: &mut Budget<'_>) -> Result<DVec2, String> {
        let mut nearest = None;
        let mut nearest_distance = f64::INFINITY;
        for index in 0..self.polygon.len() {
            budget.consume(NEAREST_WORK)?;
            let a = self.polygon[index];
            let edge = self.polygon[(index + 1) % self.polygon.len()] - a;
            let world_edge = world_delta(&self.frame, edge);
            let delta = world_delta(&self.frame, center - a);
            let fraction = delta.dot(world_edge) / world_edge.length_squared();
            if !fraction.is_finite() {
                return Err("Covered luxel padding projection is nonfinite".into());
            }
            let fraction = fraction.clamp(0.0, 1.0);
            let candidate = a + edge * fraction;
            let distance = (delta - world_edge * fraction).length_squared();
            if !candidate.is_finite() || !distance.is_finite() {
                return Err("Covered luxel padding projection is nonfinite".into());
            }
            if distance < nearest_distance {
                nearest = Some(candidate);
                nearest_distance = distance;
            }
        }
        nearest.ok_or_else(|| "Covered luxel padding has no nearest face point".into())
    }

    fn clipped_centroid(
        &self,
        center: DVec2,
        workspace: &mut Workspace,
        budget: &mut Budget<'_>,
    ) -> Result<Option<DVec2>, String> {
        workspace.first.clear();
        workspace.second.clear();
        for &point in &self.polygon {
            budget.consume(EDGE_WORK)?;
            push(&mut workspace.first, point - center, workspace.capacity)?;
        }
        for (axis, boundary, direction) in [
            (0, -CELL_RADIUS, 1.0),
            (0, CELL_RADIUS, -1.0),
            (1, -CELL_RADIUS, 1.0),
            (1, CELL_RADIUS, -1.0),
        ] {
            workspace.second.clear();
            let Some(&last) = workspace.first.last() else {
                return Ok(None);
            };
            let mut previous = last;
            let mut previous_distance = direction * (previous[axis] - boundary);
            for &current in &workspace.first {
                budget.consume(CLIP_WORK)?;
                let distance = direction * (current[axis] - boundary);
                if !distance.is_finite() || !previous_distance.is_finite() {
                    return Err("Covered luxel clipping has nonfinite support".into());
                }
                if (distance >= 0.0) != (previous_distance >= 0.0) {
                    let intersection = if previous_distance == 0.0 {
                        previous
                    } else if distance == 0.0 {
                        current
                    } else {
                        let denominator = previous_distance - distance;
                        let fraction = previous_distance / denominator;
                        if !denominator.is_finite()
                            || !fraction.is_finite()
                            || !(0.0..=1.0).contains(&fraction)
                        {
                            return Err("Covered luxel clipping has an invalid crossing".into());
                        }
                        let mut point = previous + (current - previous) * fraction;
                        point[axis] = boundary;
                        point
                    };
                    push(&mut workspace.second, intersection, workspace.capacity)?;
                }
                if distance >= 0.0 {
                    push(&mut workspace.second, current, workspace.capacity)?;
                }
                previous = current;
                previous_distance = distance;
            }
            if workspace.second.len() > 1 && workspace.second.first() == workspace.second.last() {
                workspace.second.pop();
            }
            std::mem::swap(&mut workspace.first, &mut workspace.second);
        }
        if workspace.first.len() < 3 {
            return Ok(None);
        }
        let anchor = workspace.first[0];
        let mut area = 0.0;
        let mut weighted = DVec2::ZERO;
        for index in 1..workspace.first.len() - 1 {
            budget.consume(CENTROID_WORK)?;
            let a = workspace.first[index] - anchor;
            let b = workspace.first[index + 1] - anchor;
            let weight = self.orientation * a.perp_dot(b);
            if !weight.is_finite() || weight < 0.0 {
                return Err("Covered luxel clipping produced a degenerate winding".into());
            }
            area += weight;
            weighted += (a + b) * weight;
        }
        if !area.is_finite() || !weighted.is_finite() {
            return Err("Covered luxel centroid is nonfinite".into());
        }
        if area == 0.0 {
            return Ok(None);
        }
        let centroid = center + (anchor + weighted / (3.0 * area));
        if !centroid.is_finite() {
            return Err("Covered luxel centroid is nonfinite".into());
        }
        for index in 0..self.polygon.len() {
            budget.consume(EDGE_WORK)?;
            let a = self.polygon[index];
            let edge = self.polygon[(index + 1) % self.polygon.len()] - self.polygon[index];
            let distance = self.orientation * edge.perp_dot(centroid - a);
            if !distance.is_finite() || distance < 0.0 {
                return Err("Covered luxel centroid is outside its prepared face".into());
            }
        }
        Ok(Some(centroid))
    }

    pub(super) fn sample(
        &self,
        s: usize,
        t: usize,
        workspace: &mut Workspace,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        budget.consume(LIFT_WORK)?;
        let grid = self.frame.position(s, t)?;
        let center = DVec2::new(s as f64, t as f64);
        if self.covers(center, budget)? {
            return Ok(grid);
        }
        if self
            .polygon
            .len()
            .checked_add(CLIP_PLANES)
            .is_none_or(|count| count > workspace.capacity)
        {
            return Err("Covered luxel sampling workspace is too small".into());
        }
        let centroid = if let Some(centroid) = self.clipped_centroid(center, workspace, budget)? {
            centroid
        } else {
            let center = self.nearest(center, budget)?;
            self.clipped_centroid(center, workspace, budget)?
                .ok_or("Covered luxel padding has no positive face coverage")?
        };
        budget.consume(LIFT_WORK)?;
        let point = lift(&self.frame, centroid);
        if !point.is_finite() {
            return Err("Covered luxel sample is nonfinite".into());
        }
        Ok(point)
    }
}
