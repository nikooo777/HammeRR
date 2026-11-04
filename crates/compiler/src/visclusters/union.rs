use map_core::{BrushGeometry, Plane};

use super::{
    GEOMETRY_MARGIN, MAX_INTERSECTION_PLANES, MAX_POINTS, Measurement, POINT_EPSILON,
    RELATIVE_VOLUME_MARGIN, Volume, clipping_work, measure, point_count, separated_edges,
};

const MAX_RETAINED_FRAGMENTS: usize = 256;
const MAX_RETAINED_POINTS: usize = 65_536;
const MAX_ALLOCATED_FRAGMENTS: usize = 4096;
const MAX_ALLOCATED_POINTS: usize = 1_000_000;

#[derive(Default)]
struct Budget {
    fragments: usize,
    points: usize,
    volume_error: f64,
}

impl Budget {
    fn allocate(&mut self, geometry: &BrushGeometry) -> Result<(), String> {
        self.fragments += 1;
        self.points += point_count(geometry);
        if self.fragments > MAX_ALLOCATED_FRAGMENTS || self.points > MAX_ALLOCATED_POINTS {
            return Err("Viscluster union exceeds its 4096 generated-hull or one-million allocated-point budget; simplify the brushes".into());
        }
        Ok(())
    }

    fn boundary(
        &mut self,
        geometry: &BrushGeometry,
        plane: Plane,
        width: f64,
    ) -> Result<(), String> {
        // Only this boundary slab can change the subtraction result.
        let axis = plane.normal.abs().max_position();
        let mut error = (width / plane.normal[axis].abs()).next_up();
        for other in 0..3 {
            if other == axis {
                continue;
            }
            let extent = (geometry.maxs[other] - geometry.mins[other]).next_up();
            let extent = (extent + POINT_EPSILON * 2.0).next_up();
            error = (error * extent).next_up();
        }
        self.volume_error = (self.volume_error + error).next_up();
        if !self.volume_error.is_finite() {
            return Err("Viscluster union boundary error exceeds finite volume bounds".into());
        }
        Ok(())
    }
}

enum Position {
    Outside,
    Inside,
    Crossing,
}

fn reversed(plane: Plane) -> Plane {
    Plane {
        normal: -plane.normal,
        dist: -plane.dist,
    }
}

fn distance_interval(plane: Plane, point: glam::DVec3) -> (f64, f64) {
    let mut low = -plane.dist;
    let mut high = -plane.dist;
    for axis in 0..3 {
        let product = plane.normal[axis] * point[axis];
        low = (low + product.next_down()).next_down();
        high = (high + product.next_up()).next_up();
    }
    (low, high)
}

fn position(
    geometry: &BrushGeometry,
    plane: Plane,
    budget: &mut Budget,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Position, String> {
    consume(geometry.faces.len() + point_count(geometry))?;
    let (low, high) = geometry.faces.iter().flat_map(|face| &face.vertices).fold(
        (f64::MAX, f64::MIN),
        |(low, high), &point| {
            let (point_low, point_high) = distance_interval(plane, point);
            (low.min(point_low), high.max(point_high))
        },
    );
    Ok(if high <= 0.0 {
        Position::Inside
    } else if low >= 0.0 {
        Position::Outside
    } else if high <= POINT_EPSILON {
        budget.boundary(geometry, plane, high)?;
        Position::Inside
    } else if low >= -POINT_EPSILON {
        budget.boundary(geometry, plane, -low)?;
        Position::Outside
    } else {
        Position::Crossing
    })
}

pub(super) fn separated(
    a: &BrushGeometry,
    b: &BrushGeometry,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<bool, String> {
    consume(1)?;
    if a.maxs.cmple(b.mins).any() || b.maxs.cmple(a.mins).any() {
        return Ok(true);
    }
    for (a, b) in [(a, b), (b, a)] {
        for face in &a.faces {
            consume(b.faces.len() + point_count(b))?;
            if b.faces
                .iter()
                .any(|other| other.plane == reversed(face.plane))
                || b.faces
                    .iter()
                    .flat_map(|other| &other.vertices)
                    .all(|&point| face.plane.distance_to(point) > GEOMETRY_MARGIN)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn half(
    geometry: &BrushGeometry,
    plane: Plane,
    budget: &mut Budget,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(BrushGeometry, Measurement), String> {
    let mut planes = geometry
        .faces
        .iter()
        .map(|face| face.plane)
        .collect::<Vec<_>>();
    planes.push(plane);
    if planes.len() > MAX_INTERSECTION_PLANES {
        return Err("Viscluster union fragment exceeds its 128-plane budget".into());
    }
    consume(clipping_work(planes.len()))?;
    let geometry = BrushGeometry::from_planes(&planes).map_err(|error| {
        format!("Viscluster union cannot certify a split hull: {error}; separate or simplify the boundaries")
    })?;
    let points = point_count(&geometry);
    if points > MAX_POINTS {
        return Err("Viscluster union fragment exceeds its 2048-point budget".into());
    }
    budget.allocate(&geometry)?;
    consume(points * points + points * planes.len())?;
    if geometry
        .faces
        .iter()
        .flat_map(|face| &face.vertices)
        .any(|&point| {
            planes
                .iter()
                .any(|plane| plane.distance_to(point) > POINT_EPSILON)
        })
    {
        return Err("Viscluster union split has uncertain half-space support; separate or simplify the boundaries".into());
    }
    let measurement = measure(&geometry)?;
    Ok((geometry, measurement))
}

fn retained(
    volumes: &[Volume],
    fragments: &[Volume],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(), String> {
    let faces = volumes
        .iter()
        .chain(fragments)
        .map(|volume| volume.geometry.faces.len())
        .sum::<usize>();
    consume(volumes.len() + fragments.len() + faces)?;
    if volumes.len() + fragments.len() > MAX_RETAINED_FRAGMENTS {
        return Err(
            "Viscluster union exceeds its 256 retained-fragment budget; simplify the brushes"
                .into(),
        );
    }
    let points = volumes
        .iter()
        .chain(fragments)
        .map(|volume| point_count(&volume.geometry))
        .sum::<usize>();
    if points > MAX_RETAINED_POINTS {
        return Err(
            "Viscluster union exceeds its 65536 retained-point budget; simplify the brushes".into(),
        );
    }
    Ok(())
}

fn subtract(
    source: Volume,
    cutter: &Volume,
    budget: &mut Budget,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<Volume>, String> {
    if separated(&source.geometry, &cutter.geometry, consume)?
        || separated_edges(&source.geometry, &cutter.geometry, consume)?
    {
        return Ok(vec![source]);
    }
    let mut outside = Vec::new();
    let mut inside = source;
    for face in &cutter.geometry.faces {
        match position(&inside.geometry, face.plane, budget, consume)? {
            Position::Inside => {}
            Position::Outside => {
                outside.push(inside);
                return Ok(outside);
            }
            Position::Crossing => {
                let points = point_count(&inside.geometry);
                consume(points * points)?;
                let original = measure(&inside.geometry)?;
                let (front, front_measure) =
                    half(&inside.geometry, reversed(face.plane), budget, consume)?;
                let (back, back_measure) = half(&inside.geometry, face.plane, budget, consume)?;
                let uncertainty = (original.area + front_measure.area + back_measure.area)
                    * POINT_EPSILON
                    + original.volume * RELATIVE_VOLUME_MARGIN;
                if (front_measure.volume + back_measure.volume - original.volume).abs()
                    > uncertainty
                {
                    return Err("Viscluster union split does not conserve its certified volume; separate or simplify the boundaries".into());
                }
                outside.push(Volume {
                    solid: inside.solid,
                    geometry: front,
                });
                retained(&outside, &[], consume)?;
                inside.geometry = back;
            }
        }
    }
    Ok(outside)
}

pub(super) fn build(
    sources: Vec<Volume>,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(Vec<Volume>, f64), String> {
    let mut budget = Budget::default();
    let mut volumes = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        consume(point_count(&source.geometry))?;
        budget.allocate(&source.geometry)?;
        let mut fragments = vec![Volume {
            solid: source.solid,
            geometry: source.geometry.clone(),
        }];
        for cutter in &sources[..index] {
            let mut remaining = Vec::new();
            for fragment in fragments {
                let parts =
                    subtract(fragment, cutter, &mut budget, consume).map_err(|message| {
                        format!(
                            "Viscluster solid:{} and solid:{}: {message}",
                            source.solid, cutter.solid
                        )
                    })?;
                remaining.extend(parts);
                retained(&volumes, &remaining, consume)?;
            }
            fragments = remaining;
            if fragments.is_empty() {
                break;
            }
        }
        retained(&volumes, &fragments, consume)?;
        volumes.extend(fragments);
    }
    Ok((volumes, budget.volume_error))
}
