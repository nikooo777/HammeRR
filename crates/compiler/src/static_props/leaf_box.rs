use super::*;

const CLIP_EPSILON: f64 = 1e-8;
const MAX_CLIPPED_POINTS: usize = 4_000_000;
const MAX_FRAGMENT_POINTS: usize = 4096;
const MAX_FRAGMENT_FACES: usize = 256;
const BOX_FACES: [[usize; 4]; 6] = [
    [0, 1, 3, 2],
    [4, 6, 7, 5],
    [0, 2, 6, 4],
    [1, 5, 7, 3],
    [0, 4, 5, 1],
    [2, 3, 7, 6],
];

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
struct Limits {
    work: usize,
    points: usize,
    fragment_points: usize,
    fragment_faces: usize,
    #[cfg(test)]
    cancel_after_work: Option<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        #[cfg(test)]
        if let Some(limits) = TEST_LIMITS.with(std::cell::Cell::get) {
            return limits;
        }
        Self {
            work: MAX_LEAF_WORK,
            points: MAX_CLIPPED_POINTS,
            fragment_points: MAX_FRAGMENT_POINTS,
            fragment_faces: MAX_FRAGMENT_FACES,
            #[cfg(test)]
            cancel_after_work: None,
        }
    }
}

struct Budget<'a> {
    work: &'a mut usize,
    points: &'a mut usize,
    control: &'a Control,
    limits: Limits,
}

impl Budget<'_> {
    fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        *self.work = self
            .work
            .checked_add(amount)
            .filter(|&work| work <= self.limits.work)
            .ok_or("Static-prop leaf clipping exceeded its work budget")?;
        #[cfg(test)]
        if self
            .limits
            .cancel_after_work
            .is_some_and(|limit| *self.work >= limit)
        {
            self.control.cancel()?;
            self.control.checkpoint()?;
        }
        Ok(())
    }

    fn reserve(&mut self, points: usize, faces: usize) -> Result<(), String> {
        self.consume(points + faces)?;
        if points > self.limits.fragment_points || faces > self.limits.fragment_faces {
            return Err("Static-prop leaf clipping exceeded its convex-fragment budget".into());
        }
        *self.points = self
            .points
            .checked_add(points)
            .filter(|&points| points <= self.limits.points)
            .ok_or("Static-prop leaf clipping exceeded its winding-point allocation budget")?;
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static TEST_LIMITS: std::cell::Cell<Option<Limits>> = const { std::cell::Cell::new(None) };
}

fn push(points: &mut Vec<DVec3>, point: DVec3) -> Result<(), String> {
    if points.len() == points.capacity() {
        return Err("Static-prop convex clipping exceeded its reserved point count".into());
    }
    points.push(point);
    Ok(())
}

struct Hull {
    faces: Vec<Vec<DVec3>>,
}

impl Hull {
    fn from_header(
        header: ModelHeader,
        origin: DVec3,
        rotation: DQuat,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        let mins = DVec3::from_array(header.render_mins.map(f64::from))
            .min(DVec3::from_array(header.hull_mins.map(f64::from)))
            - DVec3::splat(CLIP_EPSILON);
        let maxs = DVec3::from_array(header.render_maxs.map(f64::from))
            .max(DVec3::from_array(header.hull_maxs.map(f64::from)))
            + DVec3::splat(CLIP_EPSILON);
        let points = std::array::from_fn::<_, 8, _>(|corner| {
            origin
                + rotation
                    * DVec3::from_array(std::array::from_fn(|axis| {
                        if corner & (1 << axis) == 0 {
                            mins[axis]
                        } else {
                            maxs[axis]
                        }
                    }))
        });
        budget.reserve(BOX_FACES.len() * BOX_FACES[0].len(), BOX_FACES.len())?;
        Ok(Self {
            faces: BOX_FACES
                .iter()
                .map(|face| face.iter().map(|&index| points[index]).collect())
                .collect(),
        })
    }

    fn clip(
        &self,
        normal: DVec3,
        distance: f64,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Self>, String> {
        let points = self.faces.iter().map(Vec::len).sum::<usize>();
        budget.consume(points)?;
        let epsilon = normal.length() * CLIP_EPSILON;
        let signed = |point: DVec3| {
            let value = normal.dot(point) - distance;
            if value.abs() <= epsilon { 0.0 } else { value }
        };
        let mut inside = false;
        let mut outside = false;
        for &point in self.faces.iter().flatten() {
            let value = signed(point);
            inside |= value >= 0.0;
            outside |= value < 0.0;
        }
        if !inside {
            return Ok(None);
        }
        if !outside {
            budget.reserve(points, self.faces.len())?;
            return Ok(Some(Self {
                faces: self.faces.clone(),
            }));
        }
        budget.reserve(points + self.faces.len() * 4, self.faces.len() + 1)?;
        let mut faces = Vec::with_capacity(self.faces.len() + 1);
        let mut cap = Vec::<DVec3>::with_capacity(self.faces.len() * 2);
        for face in &self.faces {
            let mut clipped = Vec::with_capacity(face.len() + 2);
            let mut previous = *face.last().ok_or("Static-prop box has an empty face")?;
            let mut before = signed(previous);
            for &point in face {
                budget.consume(1)?;
                let after = signed(point);
                if (before < 0.0) != (after < 0.0) {
                    let crossing = previous.lerp(point, before / (before - after));
                    if !crossing.is_finite() {
                        return Err(
                            "Static-prop leaf intersection exceeds finite arithmetic".into()
                        );
                    }
                    push(&mut clipped, crossing)?;
                    budget.consume(cap.len())?;
                    if !cap
                        .iter()
                        .any(|point| point.distance_squared(crossing) <= CLIP_EPSILON.powi(2))
                    {
                        push(&mut cap, crossing)?;
                    }
                }
                if after >= 0.0 {
                    push(&mut clipped, point)?;
                }
                previous = point;
                before = after;
            }
            clipped.dedup_by(|a, b| a.distance_squared(*b) <= CLIP_EPSILON.powi(2));
            if clipped.len() > 1
                && clipped[0].distance_squared(*clipped.last().unwrap()) <= CLIP_EPSILON.powi(2)
            {
                clipped.pop();
            }
            if !clipped.is_empty() {
                faces.push(clipped);
            }
        }
        if cap.len() > 2 {
            budget.consume(cap.len() * cap.len())?;
            let center = cap.iter().sum::<DVec3>() / cap.len() as f64;
            let normal = normal.normalize();
            let axis = if normal.x.abs() < 0.9 {
                DVec3::X
            } else {
                DVec3::Y
            };
            let u = normal.cross(axis).normalize();
            let v = normal.cross(u);
            cap.sort_unstable_by(|a, b| {
                let a = *a - center;
                let b = *b - center;
                a.dot(v)
                    .atan2(a.dot(u))
                    .total_cmp(&b.dot(v).atan2(b.dot(u)))
            });
        }
        if !cap.is_empty() {
            faces.push(cap);
        }
        Ok((!faces.is_empty()).then_some(Self { faces }))
    }
}

impl Space {
    pub(super) fn prop_leaves(
        &mut self,
        header: ModelHeader,
        origin: DVec3,
        rotation: DQuat,
        control: &Control,
    ) -> Result<Vec<u16>, String> {
        let bounds = bounds(header, origin, rotation)?;
        if rotation == DQuat::IDENTITY {
            return self.intersect(bounds, control);
        }
        self.intersect_rotated(header, origin, rotation, bounds, control, Limits::default())
    }

    fn intersect_rotated(
        &mut self,
        header: ModelHeader,
        origin: DVec3,
        rotation: DQuat,
        bounds: (DVec3, DVec3),
        control: &Control,
        limits: Limits,
    ) -> Result<Vec<u16>, String> {
        let mut budget = Budget {
            work: &mut self.work,
            points: &mut self.clipped_points,
            control,
            limits,
        };
        let hull = Hull::from_header(header, origin, rotation, &mut budget)?;
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = (bounds.1 - bounds.0) * 0.5;
        let mut pending = vec![(self.head, hull)];
        let mut leaves = Vec::new();
        while let Some((index, hull)) = pending.pop() {
            budget.consume(1)?;
            if index < 0 {
                let index = (-i64::from(index) - 1) as usize;
                let leaf = self
                    .leaves
                    .get(index)
                    .ok_or("invalid static-prop leaf reference")?;
                if leaf.contents & contents::SOLID == 0 {
                    leaves.push(
                        u16::try_from(index)
                            .map_err(|_| "Static-prop leaf index exceeds the native limit")?,
                    );
                }
                continue;
            }
            let node = self
                .nodes
                .get(index as usize)
                .ok_or("invalid static-prop node reference")?;
            let plane = self
                .planes
                .get(node.plane as usize)
                .ok_or("invalid static-prop plane reference")?;
            let normal = DVec3::from_array(plane.normal.map(f64::from));
            let distance = f64::from(plane.dist);
            if !normal.is_finite() || !distance.is_finite() || normal.length_squared() == 0.0 {
                return Err("Static-prop leaf clipping needs finite nondegenerate planes".into());
            }
            let offset = normal.dot(center) - distance;
            let extent = normal.abs().dot(radius);
            let front = offset + extent >= 0.0;
            let back = offset - extent <= 0.0;
            if front && back {
                // Expand leaf half-spaces by the existing world-axis boundary bias.
                let bias = LEAF_BIAS * normal.abs().element_sum();
                for (child, normal, distance) in [
                    (node.children[0], normal, distance - bias),
                    (node.children[1], -normal, -distance - bias),
                ] {
                    if let Some(clipped) = hull.clip(normal, distance, &mut budget)? {
                        pending.push((child, clipped));
                    }
                }
            } else if front {
                pending.push((node.children[0], hull));
            } else if back {
                pending.push((node.children[1], hull));
            }
        }
        budget.consume(leaves.len() * (leaves.len().checked_ilog2().unwrap_or(0) as usize + 1))?;
        leaves.sort_unstable();
        leaves.dedup();
        if leaves.is_empty() {
            return Err("model bounds do not touch any open world leaves".into());
        }
        Ok(leaves)
    }
}
