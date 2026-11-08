use glam::DVec3;

use super::{Exact, FIELD_WORK, Work, invalid};
use crate::EditError;
use crate::clip::{TerrainCutPlane, TerrainUnionFacet};

const MAX_REGION_PIECES: usize = 4_096;
const REDUCE_WORK: usize = 3 * u128::BITS as usize;
pub(super) type Constraint = [Exact; 3];
pub(super) type Point = [Exact; 3];

pub(super) fn reduce(values: [Exact; 3], work: &mut Work) -> Result<[Exact; 3], EditError> {
    work.consume(REDUCE_WORK).map_err(invalid)?;
    let mut divisor = 0u128;
    for value in values {
        let mut remainder = value.value.unsigned_abs();
        while remainder != 0 {
            (divisor, remainder) = (remainder, divisor % remainder);
        }
    }
    if divisor <= 1 {
        return Ok(values);
    }
    let divisor = i128::try_from(divisor).map_err(|_| super::numeric())?;
    Ok(values.map(|value| Exact::new(value.value / divisor, value.exponent)))
}

#[derive(Clone)]
pub(super) struct Region {
    points: Vec<Point>,
    constraints: Vec<Constraint>,
}

fn domain() -> Vec<Constraint> {
    let zero = Exact::new(0, 0);
    let one = Exact::new(1, 0);
    let negative = Exact::new(-1, 0);
    vec![
        [negative, zero, zero],
        [zero, negative, zero],
        [one, one, negative],
    ]
}

fn evaluate(constraint: Constraint, point: Point) -> Result<Exact, EditError> {
    constraint[0]
        .mul(point[0])?
        .add(constraint[1].mul(point[1])?)?
        .add(constraint[2].mul(point[2])?)
}

fn distinct(first: Point, second: Point) -> Result<bool, EditError> {
    Ok(first[0]
        .mul(second[2])?
        .sub(second[0].mul(first[2])?)?
        .value
        != 0
        || first[1]
            .mul(second[2])?
            .sub(second[1].mul(first[2])?)?
            .value
            != 0)
}

impl Region {
    pub(super) fn new(
        constraints: Vec<Constraint>,
        work: &mut Work,
    ) -> Result<Option<Self>, EditError> {
        work.consume(constraints.len()).map_err(invalid)?;
        if constraints.iter().any(|constraint| {
            constraint[0].value == 0 && constraint[1].value == 0 && constraint[2].value > 0
        }) {
            return Ok(None);
        }
        let constraints = constraints
            .into_iter()
            .filter(|constraint| constraint[0].value != 0 || constraint[1].value != 0)
            .map(|constraint| reduce(constraint, work))
            .collect::<Result<Vec<_>, _>>()?;
        let count = constraints.len();
        work.consume(FIELD_WORK * count * count.saturating_sub(1) / 2 * count)
            .map_err(invalid)?;
        let mut points = Vec::new();
        let mut strict = vec![false; count];
        let mut first = vec![None; count];
        let mut edges = vec![false; count];
        for (index, a) in constraints.iter().enumerate() {
            for b in &constraints[index + 1..] {
                let mut denominator = a[0].mul(b[1])?.sub(b[0].mul(a[1])?)?;
                if denominator.value == 0 {
                    continue;
                }
                let mut u = a[1].mul(b[2])?.sub(b[1].mul(a[2])?)?;
                let mut v = a[2].mul(b[0])?.sub(b[2].mul(a[0])?)?;
                if denominator.value < 0 {
                    let negative = Exact::new(-1, 0);
                    denominator = denominator.mul(negative)?;
                    u = u.mul(negative)?;
                    v = v.mul(negative)?;
                }
                let point = reduce([u, v, denominator], work)?;
                let distances = constraints
                    .iter()
                    .map(|&constraint| evaluate(constraint, point))
                    .collect::<Result<Vec<_>, _>>()?;
                if distances.iter().any(|distance| distance.value > 0) {
                    continue;
                }
                work.consume(FIELD_WORK * points.len()).map_err(invalid)?;
                let mut duplicate = false;
                for &previous in &points {
                    if !distinct(previous, point)? {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    points.push(point);
                }
                for (index, distance) in distances.iter().enumerate() {
                    strict[index] |= distance.value < 0;
                    if distance.value != 0 || edges[index] {
                        continue;
                    }
                    match first[index] {
                        Some(previous) => edges[index] = distinct(previous, point)?,
                        None => first[index] = Some(point),
                    }
                }
            }
        }
        if count < 3 || strict.iter().any(|strict| !strict) {
            return Ok(None);
        }
        let constraints = constraints
            .into_iter()
            .zip(edges)
            .filter_map(|(constraint, edge)| edge.then_some(constraint))
            .collect::<Vec<_>>();
        if constraints.len() < 3 {
            return Err(invalid(
                "Terrain volume coverage lost its bounded convex region",
            ));
        }
        Ok(Some(Self {
            constraints,
            points,
        }))
    }

    pub(super) fn points(&self) -> &[Point] {
        &self.points
    }

    pub(super) fn subtract(
        self,
        covering: &[Constraint],
        work: &mut Work,
    ) -> Result<Vec<Self>, EditError> {
        let mut inside = self;
        let mut outside = Vec::new();
        for &constraint in covering {
            let mut constraints = inside.constraints.clone();
            constraints.push(constraint.map(|value| Exact::new(-value.value, value.exponent)));
            if let Some(region) = Self::new(constraints, work)? {
                outside.push(region);
            }
            let mut constraints = inside.constraints;
            constraints.push(constraint);
            let Some(region) = Self::new(constraints, work)? else {
                break;
            };
            inside = region;
        }
        Ok(outside)
    }
}

fn raw_normal(support: [DVec3; 3]) -> Result<[Exact; 3], EditError> {
    let mut a = [Exact::new(0, 0); 3];
    let mut b = [Exact::new(0, 0); 3];
    for axis in 0..3 {
        a[axis] = Exact::float(support[0][axis])?.sub(Exact::float(support[1][axis])?)?;
        b[axis] = Exact::float(support[2][axis])?.sub(Exact::float(support[1][axis])?)?;
    }
    let normal = [
        a[1].mul(b[2])?.sub(a[2].mul(b[1])?)?,
        a[2].mul(b[0])?.sub(a[0].mul(b[2])?)?,
        a[0].mul(b[1])?.sub(a[1].mul(b[0])?)?,
    ];
    Ok(normal)
}

fn support_normal(cut: &TerrainCutPlane) -> Result<[Exact; 3], EditError> {
    let normal = raw_normal(cut.support)?;
    if normal.iter().all(|value| value.value == 0) {
        return Err(invalid("Terrain cutter support is collapsed"));
    }
    Ok(normal)
}

pub(super) fn cut_plane(
    solid: &source_formats::vmf::Solid,
    face: &crate::Face,
    source: crate::surface::FaceId,
    work: &mut Work,
) -> Result<TerrainCutPlane, EditError> {
    let side = solid
        .sides
        .get(face.side)
        .ok_or_else(|| invalid("Terrain cutter lost its source side"))?;
    if source.solid != solid.id || source.side != side.id {
        return Err(invalid("Terrain cutter lost its original face identity"));
    }
    let precise = side
        .precise_vertices()
        .map_err(|error| invalid(error.to_string()))?
        .is_some();
    let vertices = if precise {
        face.vertices.as_slice()
    } else {
        side.plane.0.as_slice()
    };
    if vertices.len() < 3 {
        return Err(invalid("Terrain cutter has no effective planar winding"));
    }
    for b in 1..vertices.len() - 1 {
        for c in b + 1..vertices.len() {
            work.consume(16 * FIELD_WORK).map_err(invalid)?;
            let mut support = [vertices[0], vertices[b], vertices[c]];
            let normal = raw_normal(support)?;
            if normal.iter().all(|value| value.value == 0) {
                continue;
            }
            let normal = reduce(normal, work)?;
            let mut alignment = Exact::new(0, 0);
            for (coefficient, unit) in normal.into_iter().zip(face.plane.normal.to_array()) {
                alignment = alignment.add(coefficient.mul(Exact::float(unit)?)?)?;
            }
            if alignment.value == 0 {
                return Err(invalid(
                    "Terrain cutter winding disagrees with its effective plane",
                ));
            }
            if alignment.value < 0 {
                support.swap(1, 2);
            }
            let cut = TerrainCutPlane {
                plane: face.plane,
                source,
                support,
            };
            if !on_plane(vertices, &cut, work)? {
                return Err(invalid(
                    "Terrain cutter effective winding requires one exact planar support",
                ));
            }
            return Ok(cut);
        }
    }
    Err(invalid("Terrain cutter effective winding is collapsed"))
}

pub(super) fn distance(point: DVec3, cut: &TerrainCutPlane) -> Result<Exact, EditError> {
    let normal = support_normal(cut)?;
    let mut value = Exact::new(0, 0);
    for (axis, coefficient) in normal.into_iter().enumerate() {
        value = value.add(
            coefficient
                .mul(Exact::float(point[axis])?.sub(Exact::float(cut.support[1][axis])?)?)?,
        )?;
    }
    Ok(value)
}

pub(super) fn on_plane(
    points: &[DVec3],
    cut: &TerrainCutPlane,
    work: &mut Work,
) -> Result<bool, EditError> {
    work.consume(FIELD_WORK * points.len()).map_err(invalid)?;
    for &point in points {
        if distance(point, cut)?.value != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn opposite_support(
    first: &TerrainCutPlane,
    second: &TerrainCutPlane,
    work: &mut Work,
) -> Result<bool, EditError> {
    work.consume(16 * FIELD_WORK).map_err(invalid)?;
    let a = reduce(support_normal(first)?, work)?;
    let b = reduce(support_normal(second)?, work)?;
    let pivot = a
        .iter()
        .position(|value| value.value != 0)
        .ok_or_else(super::numeric)?;
    if b[pivot].value == 0 || a[pivot].value.signum() == b[pivot].value.signum() {
        return Ok(false);
    }
    for axis in 0..3 {
        if a[axis].mul(b[pivot])?.sub(b[axis].mul(a[pivot])?)?.value != 0 {
            return Ok(false);
        }
    }
    Ok(distance(second.support[1], first)?.value == 0)
}

pub(super) fn from_values(values: [Exact; 3]) -> Result<Constraint, EditError> {
    Ok([
        values[1].sub(values[0])?,
        values[2].sub(values[0])?,
        values[0],
    ])
}

fn footprint(
    points: [DVec3; 3],
    planes: &[TerrainCutPlane],
    padding: f64,
    skip_coplanar: bool,
    work: &mut Work,
) -> Result<Option<Vec<Constraint>>, EditError> {
    if padding != 0.0 {
        return Err(invalid(
            "Exact terrain cutter support requires zero padding",
        ));
    }
    work.consume(FIELD_WORK * points.len() * planes.len())
        .map_err(invalid)?;
    let padding = Exact::float(padding)?;
    let mut constraints = Vec::with_capacity(planes.len());
    for plane in planes {
        let values = points
            .map(|point| distance(point, plane))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if skip_coplanar && values.iter().all(|value| value.value == 0) {
            return Ok(None);
        }
        let values: [Exact; 3] = values
            .into_iter()
            .map(|value| value.add(padding))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| invalid("Terrain volume triangle lost its corners"))?;
        if values.iter().all(|value| value.value >= 0) {
            return Ok(None);
        }
        constraints.push(from_values(values)?);
    }
    Ok(Some(constraints))
}

fn footprints(
    points: [DVec3; 3],
    volumes: &[Vec<TerrainCutPlane>],
    facets: &[TerrainUnionFacet],
    padding: f64,
    work: &mut Work,
) -> Result<Vec<Vec<Constraint>>, EditError> {
    let mut result = Vec::new();
    for volume in volumes {
        if volume.is_empty() {
            return Err(invalid(
                "Terrain subtraction requires nonempty convex cutters",
            ));
        }
        if let Some(region) = footprint(points, volume, padding, true, work)? {
            result.push(region);
        }
    }
    for facet in facets {
        if on_plane(&points, &facet.plane, work)?
            && let Some(region) = footprint(points, &facet.bounds, padding, true, work)?
        {
            result.push(region);
        }
    }
    Ok(result)
}

pub(super) fn intersects(
    points: [DVec3; 3],
    volumes: &[Vec<TerrainCutPlane>],
    facets: &[TerrainUnionFacet],
    work: &mut Work,
) -> Result<bool, EditError> {
    for covering in footprints(points, volumes, facets, 0.0, work)? {
        let mut constraints = domain();
        constraints.extend(covering);
        if Region::new(constraints, work)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn edge_value(
    a: DVec3,
    b: DVec3,
    point: DVec3,
    axes: [usize; 2],
) -> Result<Exact, EditError> {
    let [x, y] = axes;
    let dx = Exact::float(b[x])?.sub(Exact::float(a[x])?)?;
    let dy = Exact::float(b[y])?.sub(Exact::float(a[y])?)?;
    dx.mul(Exact::float(point[y])?.sub(Exact::float(a[y])?)?)?
        .sub(dy.mul(Exact::float(point[x])?.sub(Exact::float(a[x])?)?)?)
}

fn child_footprint(
    base: [DVec3; 3],
    corners: [DVec3; 4],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Vec<Constraint>, EditError> {
    work.consume(FIELD_WORK * 16).map_err(invalid)?;
    let sign = edge_value(corners[0], corners[1], corners[2], axes)?
        .value
        .signum();
    if sign == 0 {
        return Err(invalid(
            "Terrain coverage requires convex native child cages",
        ));
    }
    let multiplier = Exact::new(-sign, 0);
    (0..corners.len())
        .map(|index| {
            let values = base.map(|point| {
                edge_value(
                    corners[index],
                    corners[(index + 1) % corners.len()],
                    point,
                    axes,
                )
            });
            let values: [Exact; 3] = values
                .into_iter()
                .map(|value| value?.mul(multiplier))
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| invalid("Terrain BASE coverage lost its triangle"))?;
            from_values(values)
        })
        .collect()
}

pub(super) struct Coverage<'a> {
    pub(super) base: [DVec3; 3],
    pub(super) points: [DVec3; 3],
    pub(super) children: &'a [[DVec3; 4]],
    pub(super) axes: [usize; 2],
    pub(super) volumes: &'a [Vec<TerrainCutPlane>],
    pub(super) facets: &'a [TerrainUnionFacet],
    pub(super) padding: f64,
}

pub(super) fn covered(request: Coverage<'_>, work: &mut Work) -> Result<bool, EditError> {
    let Coverage {
        base,
        points,
        children,
        axes,
        volumes,
        facets,
        padding,
    } = request;
    let mut covering = Vec::new();
    let bounds = super::labels::projected_bounds(&base, axes);
    for &corners in children {
        work.consume(super::labels::AABB_WORK).map_err(invalid)?;
        if super::labels::bounds_overlap(bounds, super::labels::projected_bounds(&corners, axes)) {
            covering.push(child_footprint(base, corners, axes, work)?);
        }
    }
    covering.extend(footprints(points, volumes, facets, padding, work)?);
    for shape in &covering {
        work.consume(FIELD_WORK * shape.len()).map_err(invalid)?;
        let mut contained = true;
        for constraint in shape {
            if constraint[2].value > 0
                || constraint[0].add(constraint[2])?.value > 0
                || constraint[1].add(constraint[2])?.value > 0
            {
                contained = false;
                break;
            }
        }
        if contained {
            return Ok(true);
        }
    }
    let Some(source) = Region::new(domain(), work)? else {
        return Err(invalid("Terrain coverage lost its source triangle"));
    };
    let mut remaining = vec![source];
    for shape in covering {
        let mut next = Vec::new();
        for region in remaining {
            work.consume(region.constraints.len()).map_err(invalid)?;
            next.extend(region.subtract(&shape, work)?);
            if next.len() > MAX_REGION_PIECES {
                return Err(invalid(
                    "Terrain volume coverage exceeds its convex-region limit",
                ));
            }
        }
        remaining = next;
        if remaining.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
