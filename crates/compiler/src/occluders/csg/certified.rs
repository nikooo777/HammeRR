use std::cmp::Ordering;
use std::collections::BTreeMap;

use glam::DVec3;
use map_core::{BrushGeometry, brush::has_precise_vertices, subobject};
use source_formats::vmf::Solid;

use super::{Budget, Control, stored_vertex};
use crate::exact::Rational as R;
use crate::winding::Winding;

pub(super) type Point = [R; 3];
const LIMBS: usize = size_of::<R>() / (2 * size_of::<i128>());
const PLANE_UNITS: usize = size_of::<Plane>().div_ceil(size_of::<DVec3>());
const POINT_UNITS: usize = size_of::<Point>().div_ceil(size_of::<DVec3>());

fn work(budget: &mut Budget, count: usize, control: &Control) -> Result<(), String> {
    budget.consume(count.saturating_mul(LIMBS), control)
}
fn allocate(
    budget: &mut Budget,
    points: usize,
    fragments: usize,
    control: &Control,
) -> Result<(), String> {
    budget.reserve(points.saturating_mul(POINT_UNITS), fragments, control)
}
fn point(value: DVec3) -> Result<Point, String> {
    Ok([
        R::from_f64(value.x)?,
        R::from_f64(value.y)?,
        R::from_f64(value.z)?,
    ])
}
fn sub(a: Point, b: Point) -> Result<Point, String> {
    Ok([a[0].sub(b[0])?, a[1].sub(b[1])?, a[2].sub(b[2])?])
}
fn dot(a: Point, b: Point) -> Result<R, String> {
    a[0].mul(b[0])?.add(a[1].mul(b[1])?)?.add(a[2].mul(b[2])?)
}
fn cross(a: Point, b: Point) -> Result<Point, String> {
    Ok([
        a[1].mul(b[2])?.sub(a[2].mul(b[1])?)?,
        a[2].mul(b[0])?.sub(a[0].mul(b[2])?)?,
        a[0].mul(b[1])?.sub(a[1].mul(b[0])?)?,
    ])
}
fn zero(value: Point) -> bool {
    value == [R::ZERO; 3]
}

#[derive(Clone)]
pub(super) struct Plane {
    normal: Point,
    distance: R,
}
impl Plane {
    fn through(points: [Point; 3]) -> Result<Self, String> {
        let normal = cross(sub(points[0], points[1])?, sub(points[2], points[1])?)?;
        if zero(normal) {
            return Err("Exact occluder face has no positive plane span".into());
        }
        Ok(Self {
            normal,
            distance: dot(normal, points[1])?,
        })
    }
    fn distance(&self, point: Point) -> Result<R, String> {
        dot(self.normal, point)?.sub(self.distance)
    }
}

fn native(point: Point) -> Result<DVec3, String> {
    let mut result = DVec3::ZERO;
    for axis in 0..3 {
        let low = point[axis].lower()?;
        let high = point[axis].upper()?;
        let lower = stored_vertex(DVec3::splat(low))[axis];
        let upper = stored_vertex(DVec3::splat(high))[axis];
        if !lower.is_finite() || lower != upper {
            return Err(
                "Exact occluder boundary has an ambiguous native coordinate conversion".into(),
            );
        }
        result[axis] = lower;
    }
    Ok(result)
}

fn intersection(planes: [&Plane; 3]) -> Result<Option<Point>, String> {
    let cofactors = [
        cross(planes[1].normal, planes[2].normal)?,
        cross(planes[2].normal, planes[0].normal)?,
        cross(planes[0].normal, planes[1].normal)?,
    ];
    let determinant = dot(planes[0].normal, cofactors[0])?;
    if determinant == R::ZERO {
        return Ok(None);
    }
    let coordinate = |axis: usize| {
        cofactors[0][axis]
            .mul(planes[0].distance)?
            .add(cofactors[1][axis].mul(planes[1].distance)?)?
            .add(cofactors[2][axis].mul(planes[2].distance)?)?
            .div(determinant)
    };
    Ok(Some([coordinate(0)?, coordinate(1)?, coordinate(2)?]))
}

fn supporting_plane(points: &[Point]) -> Result<Plane, String> {
    for pair in points[1..].windows(2) {
        let normal = cross(sub(points[0], pair[0])?, sub(pair[1], pair[0])?)?;
        if !zero(normal) {
            return Plane::through([points[0], pair[0], pair[1]]);
        }
    }
    Err("Exact occluder ring is collinear".into())
}

fn check_ring(
    points: &[Point],
    plane: &Plane,
    budget: &mut Budget,
    control: &Control,
) -> Result<(), String> {
    if points.len() < 3 {
        return Err("Exact occluder ring has fewer than three corners".into());
    }
    let mut positive = false;
    for (index, &a) in points.iter().enumerate() {
        let b = points[(index + 1) % points.len()];
        if a == b {
            return Err("Exact occluder boundary has a collapsed edge".into());
        }
        work(budget, 16, control)?;
        if plane.distance(a)? != R::ZERO {
            return Err("Exact occluder ring is nonplanar".into());
        }
        for (other_index, &other) in points.iter().enumerate() {
            work(budget, 32, control)?;
            if index != other_index && a == other {
                return Err("Exact occluder ring repeats a nonconsecutive corner".into());
            }
            match dot(cross(sub(b, a)?, sub(other, a)?)?, plane.normal)?.sign() {
                Ordering::Greater => {
                    return Err("Exact occluder ring is nonconvex or reversed".into());
                }
                Ordering::Less => positive = true,
                Ordering::Equal => {}
            }
        }
    }
    if !positive {
        return Err("Exact occluder ring has zero area".into());
    }
    Ok(())
}

pub(super) struct Brush {
    pub planes: Vec<Plane>,
    native_planes: Vec<Plane>,
    pub faces: Vec<Vec<Point>>,
    vertices: Vec<Point>,
    native_vertices: Vec<Point>,
    edges: Vec<[usize; 2]>,
}

impl Brush {
    pub fn prepare(
        solid: &Solid,
        geometry: &BrushGeometry,
        budget: &mut Budget,
        control: &Control,
    ) -> Result<Self, String> {
        let sides_count = solid.sides.len();
        if !(4..=subobject::MAX_SIDES).contains(&sides_count) {
            return Err("Exact occluder topology requires 4–64 source sides".into());
        }
        budget.reserve(
            sides_count
                .saturating_mul(sides_count)
                .saturating_mul(sides_count + 8),
            sides_count,
            control,
        )?;
        let topology = subobject::topology(solid)
            .map_err(|error| format!("Exact occluder topology: {error}"))?;
        let sides = solid
            .sides
            .iter()
            .enumerate()
            .map(|(index, side)| (side.id, index))
            .collect::<BTreeMap<_, _>>();
        budget.reserve(
            solid
                .sides
                .len()
                .saturating_mul(3)
                .saturating_mul(PLANE_UNITS),
            0,
            control,
        )?;
        allocate(budget, topology.vertices.len() * 2, 0, control)?;
        let mut authored = Vec::with_capacity(solid.sides.len());
        for side in &solid.sides {
            work(budget, 64, control)?;
            authored.push(Plane::through([
                point(side.plane.0[0])?,
                point(side.plane.0[1])?,
                point(side.plane.0[2])?,
            ])?);
        }
        let precise = has_precise_vertices(solid);
        let mut vertices = Vec::with_capacity(topology.vertices.len());
        let mut native_vertices = Vec::with_capacity(topology.vertices.len());
        for vertex in &topology.vertices {
            let exact = if precise {
                point(vertex.position)?
            } else {
                let mut found = None;
                'triples: for a in 0..vertex.sides.len() - 2 {
                    for b in a + 1..vertex.sides.len() - 1 {
                        for c in b + 1..vertex.sides.len() {
                            work(budget, 192, control)?;
                            let planes =
                                [a, b, c].map(|index| &authored[sides[&vertex.sides[index]]]);
                            if let Some(point) = intersection(planes)? {
                                found = Some(point);
                                break 'triples;
                            }
                        }
                    }
                }
                found.ok_or("Exact occluder corner has no independent incident planes")?
            };
            for (side, plane) in solid.sides.iter().zip(&authored) {
                work(budget, 16, control)?;
                let sign = plane.distance(exact)?.sign();
                if sign == Ordering::Greater
                    || ((sign == Ordering::Equal) != vertex.sides.contains(&side.id))
                {
                    return Err("Exact occluder planes do not preserve the admitted source corner incidence".into());
                }
            }
            work(budget, 64, control)?;
            let stored = native(exact)?;
            if stored != stored_vertex(vertex.position) {
                return Err(
                    "Exact occluder corner reconstruction changes its original native vertex"
                        .into(),
                );
            }
            vertices.push(exact);
            native_vertices.push(point(stored)?);
        }
        let mut faces = Vec::with_capacity(geometry.faces.len());
        let mut planes = Vec::with_capacity(geometry.faces.len());
        let mut native_planes = Vec::with_capacity(geometry.faces.len());
        for face in &geometry.faces {
            let incidence = topology
                .faces
                .iter()
                .find(|item| item.side == solid.sides[face.side].id)
                .ok_or("Exact occluder face has no topology")?;
            if incidence.vertices.len() != face.vertices.len() {
                return Err("Exact occluder face changed its corner count".into());
            }
            allocate(budget, face.vertices.len() * 2, 0, control)?;
            let ring = incidence
                .vertices
                .iter()
                .map(|&index| vertices[index])
                .collect::<Vec<_>>();
            let stored = incidence
                .vertices
                .iter()
                .map(|&index| native_vertices[index])
                .collect::<Vec<_>>();
            for ((&source, &stored), &original) in ring.iter().zip(&stored).zip(&face.vertices) {
                if precise && source != point(original)? {
                    return Err(
                        "Exact occluder precise rings contain distinct shared corners".into(),
                    );
                }
                if native(source)? != stored_vertex(original)
                    || stored != point(stored_vertex(original))?
                {
                    return Err(
                        "Exact occluder ring does not retain its original native corner sequence"
                            .into(),
                    );
                }
            }
            let plane = authored[face.side].clone();
            check_ring(&ring, &plane, budget, control)?;
            let native_plane = supporting_plane(&stored)?;
            check_ring(&stored, &native_plane, budget, control)?;
            work(budget, 16, control)?;
            if dot(plane.normal, native_plane.normal)?.sign() != Ordering::Greater {
                return Err("Exact occluder native face reverses source orientation".into());
            }
            for (index, &vertex) in native_vertices.iter().enumerate() {
                work(budget, 16, control)?;
                let sign = native_plane.distance(vertex)?.sign();
                if sign == Ordering::Greater
                    || ((sign == Ordering::Equal) != incidence.vertices.contains(&index))
                {
                    return Err(
                        "Exact occluder native storage changes the closed convex source topology"
                            .into(),
                    );
                }
            }
            planes.push(plane);
            native_planes.push(native_plane);
            faces.push(ring);
        }
        for (index, vertex) in vertices.iter().enumerate() {
            work(budget, vertices.len(), control)?;
            if vertices[index + 1..].contains(vertex)
                || native_vertices[index + 1..].contains(&native_vertices[index])
            {
                return Err("Exact occluder native storage merges distinct source corners".into());
            }
        }
        Ok(Self {
            planes,
            native_planes,
            faces,
            vertices,
            native_vertices,
            edges: topology
                .edges
                .into_iter()
                .map(|edge| edge.vertices)
                .collect(),
        })
    }

    fn directions(
        &self,
        native: bool,
        budget: &mut Budget,
        control: &Control,
    ) -> Result<Vec<Point>, String> {
        let vertices = if native {
            &self.native_vertices
        } else {
            &self.vertices
        };
        allocate(budget, self.edges.len(), 0, control)?;
        let mut directions = Vec::with_capacity(self.edges.len());
        for edge in &self.edges {
            let direction = sub(vertices[edge[1]], vertices[edge[0]])?;
            let mut duplicate = false;
            for &other in &directions {
                work(budget, 16, control)?;
                if zero(cross(direction, other)?) {
                    duplicate = true;
                    break;
                }
            }
            if !duplicate {
                directions.push(direction);
            }
        }
        Ok(directions)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Relation {
    Separate,
    Contact(usize),
    Overlap,
}

fn projection(
    points: &[Point],
    axis: Point,
    budget: &mut Budget,
    control: &Control,
) -> Result<(R, R), String> {
    let mut low = dot(points[0], axis)?;
    let mut high = low;
    for &point in &points[1..] {
        work(budget, 16, control)?;
        let value = dot(point, axis)?;
        if value.compare(low) == Ordering::Less {
            low = value;
        }
        if value.compare(high) == Ordering::Greater {
            high = value;
        }
    }
    Ok((low, high))
}

fn relation(
    a: &Brush,
    b: &Brush,
    native: bool,
    budget: &mut Budget,
    control: &Control,
) -> Result<Relation, String> {
    let planes = if native {
        (&a.native_planes, &b.native_planes)
    } else {
        (&a.planes, &b.planes)
    };
    let vertices = if native {
        (&a.native_vertices, &b.native_vertices)
    } else {
        (&a.vertices, &b.vertices)
    };
    let ad = a.directions(native, budget, control)?;
    let bd = b.directions(native, budget, control)?;
    let mut contacts = Vec::with_capacity(3);
    allocate(budget, 3, 0, control)?;
    let mut axis = |axis: Point| -> Result<bool, String> {
        work(budget, 16, control)?;
        if zero(axis) {
            return Ok(false);
        }
        let (al, ah) = projection(vertices.0, axis, budget, control)?;
        let (bl, bh) = projection(vertices.1, axis, budget, control)?;
        let order = ah.compare(bl).min(bh.compare(al));
        if order == Ordering::Less {
            return Ok(true);
        }
        if order == Ordering::Equal {
            let independent = match contacts.len() {
                0 => true,
                1 => !zero(cross(contacts[0], axis)?),
                2 => dot(cross(contacts[0], contacts[1])?, axis)? != R::ZERO,
                _ => false,
            };
            if independent {
                contacts.push(axis);
            }
        }
        Ok(false)
    };
    for plane in planes.0.iter().chain(planes.1) {
        if axis(plane.normal)? {
            return Ok(Relation::Separate);
        }
    }
    for first in ad {
        for &second in &bd {
            if axis(cross(first, second)?)? {
                return Ok(Relation::Separate);
            }
        }
    }
    Ok(if contacts.is_empty() {
        Relation::Overlap
    } else {
        Relation::Contact(contacts.len())
    })
}

pub(super) fn pair(
    a: &Brush,
    b: &Brush,
    budget: &mut Budget,
    control: &Control,
) -> Result<Relation, String> {
    let source = relation(a, b, false, budget, control)?;
    let native = relation(a, b, true, budget, control)?;
    if source != native {
        return Err(
            "Occluder native storage changes an exact source gap, contact or overlap".into(),
        );
    }
    if source != Relation::Separate {
        for (body, cutter) in [(a, b), (b, a)] {
            for (&point, &stored) in body.vertices.iter().zip(&body.native_vertices) {
                for (plane, native) in cutter.planes.iter().zip(&cutter.native_planes) {
                    work(budget, 32, control)?;
                    if plane.distance(point)?.sign() != native.distance(stored)?.sign() {
                        return Err(
                            "Occluder native storage changes source contact feature incidence"
                                .into(),
                        );
                    }
                }
            }
        }
    }
    Ok(source)
}

pub(super) fn subtract(
    winding: Vec<Point>,
    normal: &Plane,
    cutter: &Brush,
    keep_coplanar: &[bool],
    budget: &mut Budget,
    control: &Control,
) -> Result<Vec<Vec<Point>>, String> {
    for (index, plane) in cutter.planes.iter().enumerate() {
        let signs = classify(&winding, plane, budget, control)?;
        if signs == (true, false) {
            return Ok(vec![winding]);
        }
        if signs == (false, false) {
            work(budget, 16, control)?;
            if dot(normal.normal, plane.normal)?.sign() == Ordering::Greater && keep_coplanar[index]
            {
                return Ok(vec![winding]);
            }
        }
    }
    let mut output = Vec::new();
    let mut inside = winding;
    for plane in &cutter.planes {
        match classify(&inside, plane, budget, control)? {
            (true, false) => {
                output.push(inside);
                break;
            }
            (true, true) => {
                let (front, back) = split(&inside, plane, budget, control)?;
                output.push(front);
                inside = back;
            }
            _ => {}
        }
    }
    Ok(output)
}
fn classify(
    points: &[Point],
    plane: &Plane,
    budget: &mut Budget,
    control: &Control,
) -> Result<(bool, bool), String> {
    let (mut front, mut back) = (false, false);
    for &point in points {
        work(budget, 16, control)?;
        match plane.distance(point)?.sign() {
            Ordering::Less => back = true,
            Ordering::Greater => front = true,
            Ordering::Equal => {}
        }
    }
    Ok((front, back))
}
fn split(
    points: &[Point],
    plane: &Plane,
    budget: &mut Budget,
    control: &Control,
) -> Result<(Vec<Point>, Vec<Point>), String> {
    allocate(budget, points.len() * 2 + 4, 2, control)?;
    let mut front = Vec::with_capacity(points.len() + 2);
    let mut back = Vec::with_capacity(points.len() + 2);
    for (index, &p) in points.iter().enumerate() {
        let q = points[(index + 1) % points.len()];
        work(budget, 96, control)?;
        let dp = plane.distance(p)?;
        let dq = plane.distance(q)?;
        if dp.sign() != Ordering::Less {
            front.push(p);
        }
        if dp.sign() != Ordering::Greater {
            back.push(p);
        }
        if dp.sign() != Ordering::Equal && dq.sign() != Ordering::Equal && dp.sign() != dq.sign() {
            let t = dp.div(dp.sub(dq)?)?;
            let coordinate = |axis: usize| p[axis].add(q[axis].sub(p[axis])?.mul(t)?);
            let mid = [coordinate(0)?, coordinate(1)?, coordinate(2)?];
            if plane.distance(mid)? != R::ZERO {
                return Err("Exact occluder split lost its cutting plane".into());
            }
            front.push(mid);
            back.push(mid);
        }
    }
    if front.len() < 3 || back.len() < 3 {
        return Err("Exact occluder positive split has a collapsed ring".into());
    }
    Ok((front, back))
}

pub(super) fn preserves(
    points: &[Point],
    brushes: &[&Brush],
    budget: &mut Budget,
    control: &Control,
) -> Result<(), (usize, String)> {
    for &source in points {
        work(budget, 64, control).map_err(|error| (0, error))?;
        let stored =
            point(native(source).map_err(|error| (0, error))?).map_err(|error| (0, error))?;
        for (index, brush) in brushes.iter().enumerate() {
            for (plane, native) in brush.planes.iter().zip(&brush.native_planes) {
                work(budget, 32, control).map_err(|error| (index, error))?;
                let before = plane
                    .distance(source)
                    .map_err(|error| (index, error))?
                    .sign();
                let after = native
                    .distance(stored)
                    .map_err(|error| (index, error))?
                    .sign();
                if before != after {
                    return Err((index,"Occluder native conversion changes a positive fragment or cutting-plane incidence".into()));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn native_winding(
    points: Vec<Point>,
    source: &Brush,
    face: usize,
    budget: &mut Budget,
    control: &Control,
) -> Result<Winding, String> {
    check_ring(&points, &source.planes[face], budget, control)?;
    budget.reserve(points.len().saturating_mul(POINT_UNITS + 1), 0, control)?;
    let mut native_points = Vec::with_capacity(points.len());
    for point in points {
        work(budget, 64, control)?;
        native_points.push(native(point)?);
    }
    let exact = native_points
        .iter()
        .copied()
        .map(point)
        .collect::<Result<Vec<_>, _>>()?;
    let plane = supporting_plane(&exact)?;
    check_ring(&exact, &plane, budget, control)?;
    work(budget, 16, control)?;
    if dot(plane.normal, source.planes[face].normal)?.sign() != Ordering::Greater {
        return Err("Exact occluder conversion reverses its boundary".into());
    }
    Ok(Winding(native_points))
}

fn axis_bound(
    solid: &Solid,
    geometry: &BrushGeometry,
    axis: DVec3,
    budget: &mut Budget,
    control: &Control,
) -> Result<Option<R>, String> {
    let exact_axis = point(axis)?;
    if has_precise_vertices(solid) {
        let mut bound = None;
        for &vertex in geometry.faces.iter().flat_map(|face| &face.vertices) {
            work(budget, 32, control)?;
            let value = dot(point(vertex)?, exact_axis)?;
            if bound.is_none_or(|bound: R| value.compare(bound) == Ordering::Greater) {
                bound = Some(value);
            }
        }
        return Ok(bound);
    }
    let mut candidate = None;
    for &vertex in geometry.faces.iter().flat_map(|face| &face.vertices) {
        budget.consume(4, control)?;
        let projection = vertex.dot(axis);
        if candidate.is_none_or(|(_, value)| projection > value) {
            candidate = Some((vertex, projection));
        }
    }
    let (vertex, _) = candidate.ok_or("Exact occluder support has no prepared corners")?;
    budget.consume(geometry.faces.len().saturating_mul(2), control)?;
    let candidates = geometry
        .faces
        .iter()
        .filter(|face| face.plane.distance_to(vertex).abs() <= crate::winding::ON_EPSILON)
        .count();
    budget.reserve(candidates.saturating_mul(PLANE_UNITS), 0, control)?;
    let mut planes = Vec::with_capacity(candidates);
    for face in geometry
        .faces
        .iter()
        .filter(|face| face.plane.distance_to(vertex).abs() <= crate::winding::ON_EPSILON)
    {
        work(budget, 64, control)?;
        let points = solid.sides[face.side].plane.0;
        planes.push(Plane::through([
            point(points[0])?,
            point(points[1])?,
            point(points[2])?,
        ])?);
    }
    for plane in &planes {
        work(budget, 32, control)?;
        if zero(cross(plane.normal, exact_axis)?) {
            let coordinate = exact_axis
                .iter()
                .position(|&value| value != R::ZERO)
                .ok_or("Exact occluder separation axis is zero")?;
            let weight = exact_axis[coordinate].div(plane.normal[coordinate])?;
            if weight.sign() != Ordering::Less {
                return Ok(Some(weight.mul(plane.distance)?));
            }
        }
    }
    for a in 0..planes.len() {
        for b in a + 1..planes.len() {
            for c in b + 1..planes.len() {
                work(budget, 192, control)?;
                let selected = [&planes[a], &planes[b], &planes[c]];
                let cofactors = [
                    cross(selected[1].normal, selected[2].normal)?,
                    cross(selected[2].normal, selected[0].normal)?,
                    cross(selected[0].normal, selected[1].normal)?,
                ];
                let determinant = dot(selected[0].normal, cofactors[0])?;
                if determinant == R::ZERO {
                    continue;
                }
                let weights = [
                    dot(exact_axis, cofactors[0])?.div(determinant)?,
                    dot(exact_axis, cofactors[1])?.div(determinant)?,
                    dot(exact_axis, cofactors[2])?.div(determinant)?,
                ];
                if weights.iter().any(|value| value.sign() == Ordering::Less) {
                    continue;
                }
                for (coordinate, &expected) in exact_axis.iter().enumerate() {
                    let restored = weights[0]
                        .mul(selected[0].normal[coordinate])?
                        .add(weights[1].mul(selected[1].normal[coordinate])?)?
                        .add(weights[2].mul(selected[2].normal[coordinate])?)?;
                    if restored != expected {
                        return Err("Exact occluder dual support failed axis reconstruction".into());
                    }
                }
                return Ok(Some(
                    weights[0]
                        .mul(selected[0].distance)?
                        .add(weights[1].mul(selected[1].distance)?)?
                        .add(weights[2].mul(selected[2].distance)?)?,
                ));
            }
        }
    }
    Ok(None)
}

pub(super) fn source_separated(
    a: &Solid,
    ga: &BrushGeometry,
    b: &Solid,
    gb: &BrushGeometry,
    axis: DVec3,
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    let Some(upper_a) = axis_bound(a, ga, axis, budget, control)? else {
        return Ok(false);
    };
    let Some(upper_negative_b) = axis_bound(b, gb, -axis, budget, control)? else {
        return Ok(false);
    };
    work(budget, 8, control)?;
    Ok(upper_a.add(upper_negative_b)?.sign() == Ordering::Less)
}
