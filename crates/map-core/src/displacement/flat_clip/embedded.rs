use glam::DVec3;

use super::{
    Exact, FIELD_WORK, PlanarField, Work, field::BasePlane, invalid, orient, polygon, same,
};
use crate::{EditError, displacement::DisplacementMesh};

const PAIR_WORK: usize = 16;
const PAIR_SETUP_WORK: usize = 4 * FIELD_WORK;
const AXIS_WORK: usize = 4 * FIELD_WORK;
const CONTACT_WORK: usize = 20 * FIELD_WORK;

type Vector = [Exact; 3];

struct Triangle {
    indices: [usize; 3],
    base: [DVec3; 3],
    world: [DVec3; 3],
    base_bounds: [DVec3; 2],
    bounds: [DVec3; 2],
    edges: [Vector; 3],
    normal: Vector,
    field: PlanarField,
}

impl Triangle {
    fn new(
        mesh: &DisplacementMesh,
        indices: [usize; 3],
        field: PlanarField,
    ) -> Result<Self, EditError> {
        let base = indices.map(|index| mesh.base_positions[index]);
        let world = indices.map(|index| mesh.positions[index]);
        let edges = edges(world)?;
        let normal = cross(edges[0], edges[1])?;
        if !nonzero(normal) {
            return Err(invalid("Terrain embedded triangles must be noncollapsed"));
        }
        Ok(Self {
            indices,
            base,
            world,
            base_bounds: bounds(&base),
            bounds: bounds(&world),
            edges,
            normal,
            field,
        })
    }

    fn position(&self, point: DVec3, work: &mut Work) -> Result<DVec3, EditError> {
        if let Some(index) = self.base.iter().position(|&base| same(base, point)) {
            return Ok(self.world[index]);
        }
        work.consume(FIELD_WORK).map_err(invalid)?;
        self.field.position(point)
    }
}

pub(super) struct Prepared {
    triangles: Vec<Triangle>,
    base: BasePlane,
    base_bounds: [DVec3; 2],
    bounds: [DVec3; 2],
    affine: Option<PlanarField>,
}

pub(super) fn prepare(mesh: &DisplacementMesh, work: &mut Work) -> Result<Prepared, EditError> {
    if !matches!(mesh.size, 5 | 9 | 17)
        || mesh.positions.len() != mesh.size * mesh.size
        || mesh.base_positions.len() != mesh.positions.len()
    {
        return Err(invalid(
            "Terrain embedded preparation requires complete native grids",
        ));
    }
    work.consume(FIELD_WORK + 2 * mesh.positions.len())
        .map_err(invalid)?;
    let base = BasePlane::new([
        mesh.base_positions[0],
        mesh.base_positions[(mesh.size - 1) * mesh.size],
        mesh.base_positions[mesh.size * mesh.size - 1],
    ])?;
    let first = mesh.triangles().next().unwrap();
    work.consume(17 * FIELD_WORK).map_err(invalid)?;
    let candidate = PlanarField::triangle(
        first.map(|index| mesh.base_positions[index]),
        first.map(|index| mesh.positions[index]),
    )?;
    let mut affine = Some(candidate);
    for (&base, &world) in mesh.base_positions.iter().zip(&mesh.positions) {
        work.consume(4 * FIELD_WORK).map_err(invalid)?;
        if !candidate.maps(base, world)? {
            affine = None;
            break;
        }
    }
    let triangles = if let Some(field) = affine {
        // A shared rank-two map on every vertex carries the complete BASE
        // cover to exactly these cage triangles, including all contacts.
        work.consume(2 * PAIR_SETUP_WORK).map_err(invalid)?;
        let corners = [
            0,
            (mesh.size - 1) * mesh.size,
            mesh.positions.len() - 1,
            mesh.size - 1,
        ];
        [
            [corners[0], corners[1], corners[2]],
            [corners[0], corners[2], corners[3]],
        ]
        .map(|indices| Triangle::new(mesh, indices, field))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
    } else {
        mesh.triangles()
            .map(|indices| {
                work.consume(PAIR_SETUP_WORK).map_err(invalid)?;
                let field = if indices == first {
                    candidate
                } else {
                    work.consume(17 * FIELD_WORK).map_err(invalid)?;
                    PlanarField::triangle(
                        indices.map(|index| mesh.base_positions[index]),
                        indices.map(|index| mesh.positions[index]),
                    )?
                };
                Triangle::new(mesh, indices, field)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(Prepared {
        triangles,
        base,
        base_bounds: bounds(&mesh.base_positions),
        bounds: bounds(&mesh.positions),
        affine,
    })
}

fn difference(a: DVec3, b: DVec3) -> Result<Vector, EditError> {
    Ok([
        Exact::float(a.x)?.sub(Exact::float(b.x)?)?,
        Exact::float(a.y)?.sub(Exact::float(b.y)?)?,
        Exact::float(a.z)?.sub(Exact::float(b.z)?)?,
    ])
}

fn cross(a: Vector, b: Vector) -> Result<Vector, EditError> {
    Ok([
        a[1].mul(b[2])?.sub(a[2].mul(b[1])?)?,
        a[2].mul(b[0])?.sub(a[0].mul(b[2])?)?,
        a[0].mul(b[1])?.sub(a[1].mul(b[0])?)?,
    ])
}

fn dot(a: Vector, b: Vector) -> Result<Exact, EditError> {
    a[0].mul(b[0])?.add(a[1].mul(b[1])?)?.add(a[2].mul(b[2])?)
}

fn nonzero(vector: Vector) -> bool {
    vector.into_iter().any(|value| value.value != 0)
}

fn edges(points: [DVec3; 3]) -> Result<[Vector; 3], EditError> {
    Ok([
        difference(points[1], points[0])?,
        difference(points[2], points[1])?,
        difference(points[0], points[2])?,
    ])
}

fn range(points: [DVec3; 3], axis: Vector) -> Result<[Exact; 2], EditError> {
    let mut result = [dot(difference(points[0], DVec3::ZERO)?, axis)?; 2];
    for point in &points[1..] {
        let value = dot(difference(*point, DVec3::ZERO)?, axis)?;
        if value.sub(result[0])?.value < 0 {
            result[0] = value;
        }
        if value.sub(result[1])?.value > 0 {
            result[1] = value;
        }
    }
    Ok(result)
}

fn bounds(points: &[DVec3]) -> [DVec3; 2] {
    let mut result = [points[0]; 2];
    for &point in &points[1..] {
        result[0] = result[0].min(point);
        result[1] = result[1].max(point);
    }
    result
}

fn apart(a: [DVec3; 2], b: [DVec3; 2]) -> bool {
    (0..3).any(|axis| a[1][axis] < b[0][axis] || b[1][axis] < a[0][axis])
}

fn widest(bounds: [DVec3; 2]) -> usize {
    let span = bounds[1] - bounds[0];
    let mut axis = 0;
    for candidate in 1..3 {
        if span[candidate] > span[axis] {
            axis = candidate;
        }
    }
    axis
}

struct OrderedTriangle {
    index: usize,
    low: f64,
    high: f64,
}

struct TriangleOrder {
    triangles: Vec<OrderedTriangle>,
    prefix_high: Vec<f64>,
    axis: usize,
}

impl TriangleOrder {
    fn new(
        triangles: &[Triangle],
        bounds: [DVec3; 2],
        base: bool,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        work.consume(2 * PAIR_WORK * triangles.len())
            .map_err(invalid)?;
        let axis = widest(bounds);
        let mut triangles = triangles
            .iter()
            .enumerate()
            .map(|(index, triangle)| {
                let bounds = if base {
                    triangle.base_bounds
                } else {
                    triangle.bounds
                };
                OrderedTriangle {
                    index,
                    low: bounds[0][axis],
                    high: bounds[1][axis],
                }
            })
            .collect::<Vec<_>>();
        let mut comparisons = 0;
        triangles.sort_unstable_by(|a, b| {
            comparisons += 1;
            a.low.total_cmp(&b.low)
        });
        work.consume(PAIR_WORK * comparisons).map_err(invalid)?;
        let mut high = f64::NEG_INFINITY;
        let prefix_high = triangles
            .iter()
            .map(|triangle| {
                high = high.max(triangle.high);
                high
            })
            .collect();
        Ok(Self {
            triangles,
            prefix_high,
            axis,
        })
    }

    fn range(
        &self,
        bounds: [DVec3; 2],
        work: &mut Work,
    ) -> Result<std::ops::Range<usize>, EditError> {
        let mut comparisons = 0;
        let start = self.prefix_high.partition_point(|&high| {
            comparisons += 1;
            high < bounds[0][self.axis]
        });
        let end = start
            + self.triangles[start..].partition_point(|triangle| {
                comparisons += 1;
                triangle.low <= bounds[1][self.axis]
            });
        work.consume(PAIR_WORK * comparisons).map_err(invalid)?;
        Ok(start..end)
    }
}

fn basis_add(basis: &mut Vec<Vector>, axis: Vector) -> Result<(), EditError> {
    match basis.as_slice() {
        [] => basis.push(axis),
        [first] if nonzero(cross(*first, axis)?) => basis.push(axis),
        [first, second] if dot(cross(*first, *second)?, axis)?.value != 0 => basis.push(axis),
        _ => {}
    }
    Ok(())
}

fn line_interval(
    points: [DVec3; 3],
    origin: DVec3,
    direction: Vector,
    axis: usize,
) -> Result<[f64; 2], EditError> {
    // Both planes support the triangle, so their common line contains only
    // the convex hull of triangle vertices on that line.
    let mut result: Option<[f64; 2]> = None;
    for point in points {
        if !nonzero(cross(difference(point, origin)?, direction)?) {
            result = Some(match result {
                Some(interval) => [interval[0].min(point[axis]), interval[1].max(point[axis])],
                None => [point[axis]; 2],
            });
        }
    }
    result.ok_or_else(|| invalid("Terrain embedded contact lost its exact supported interval"))
}

fn supported_contact(
    first: [DVec3; 3],
    second: [DVec3; 3],
    expected: &[DVec3],
    basis: &[Vector],
    work: &mut Work,
) -> Result<(), EditError> {
    work.consume(CONTACT_WORK).map_err(invalid)?;
    let direction = cross(basis[0], basis[1])?;
    let axis = (0..3).find(|&axis| direction[axis].value != 0).unwrap();
    let origin = expected[0];
    let first = line_interval(first, origin, direction, axis)?;
    let second = line_interval(second, origin, direction, axis)?;
    let a = expected[0][axis];
    let b = expected.last().unwrap()[axis];
    if first[0].max(second[0]) != a.min(b) || first[1].min(second[1]) != a.max(b) {
        return Err(invalid(
            "Terrain embedded contact extends beyond its exact native interval",
        ));
    }
    Ok(())
}

fn support_feature(
    triangle: [DVec3; 3],
    point: DVec3,
    normal: Vector,
    work: &mut Work,
) -> Result<Vec<DVec3>, EditError> {
    work.consume(3 * FIELD_WORK).map_err(invalid)?;
    let mut result = Vec::with_capacity(3);
    for vertex in triangle {
        if dot(difference(vertex, point)?, normal)?.value == 0 {
            result.push(vertex);
        }
    }
    Ok(result)
}

fn feature_contains(
    feature: &[DVec3],
    point: DVec3,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<bool, EditError> {
    match feature {
        [vertex] => Ok(same(*vertex, point)),
        [a, b] => {
            work.consume(polygon::ON_SEGMENT_WORK).map_err(invalid)?;
            polygon::on_segment(point, [*a, *b], axes)
        }
        [a, b, c] => {
            work.consume(3 * FIELD_WORK).map_err(invalid)?;
            let signs = [
                orient(*a, *b, point, axes)?,
                orient(*b, *c, point, axes)?,
                orient(*c, *a, point, axes)?,
            ];
            Ok(signs.iter().all(|&sign| sign >= 0) || signs.iter().all(|&sign| sign <= 0))
        }
        _ => Ok(false),
    }
}

fn supported_point(
    first: [DVec3; 3],
    second: [DVec3; 3],
    point: DVec3,
    normal: Vector,
    work: &mut Work,
) -> Result<bool, EditError> {
    let axis = (0..3).find(|&axis| normal[axis].value != 0).unwrap();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let first = support_feature(first, point, normal, work)?;
    let second = support_feature(second, point, normal, work)?;
    if !feature_contains(&first, point, axes, work)?
        || !feature_contains(&second, point, axes, work)?
    {
        return Ok(false);
    }
    if first.len() == 1 || second.len() == 1 {
        return Ok(true);
    }
    if first.len() == 2 && second.len() == 2 {
        work.consume(FIELD_WORK).map_err(invalid)?;
        return Ok(nonzero(cross(
            difference(first[1], first[0])?,
            difference(second[1], second[0])?,
        )?));
    }
    Ok(false)
}

enum Axis {
    Normal(Vector),
    Cross(Vector, Vector),
}

struct Pair<'a> {
    first: [DVec3; 3],
    second: [DVec3; 3],
    expected: &'a [DVec3],
    basis: Vec<Vector>,
}

impl Pair<'_> {
    fn axis(&mut self, axis: Axis, work: &mut Work) -> Result<bool, EditError> {
        work.consume(AXIS_WORK).map_err(invalid)?;
        let axis = match axis {
            Axis::Normal(axis) => axis,
            Axis::Cross(first, second) => cross(first, second)?,
        };
        if !nonzero(axis) {
            return Ok(false);
        }
        let a = range(self.first, axis)?;
        let b = range(self.second, axis)?;
        if a[1].sub(b[0])?.value < 0 || b[1].sub(a[0])?.value < 0 {
            return if self.expected.is_empty() {
                Ok(true)
            } else {
                Err(invalid("Terrain lost an expected embedded native contact"))
            };
        }
        if !self.expected.is_empty() && (a[1] == b[0] || b[1] == a[0]) {
            basis_add(&mut self.basis, axis)?;
            if self.basis.len() == 2 {
                supported_contact(self.first, self.second, self.expected, &self.basis, work)?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn certify_pair(
    first: &Triangle,
    second: &Triangle,
    expected: &[DVec3],
    kind: ContactKind,
    work: &mut Work,
) -> Result<(), EditError> {
    if expected.len() > 2 {
        return Err(invalid(
            "Terrain embedded triangles have overlapping native domains",
        ));
    }
    let mut pair = Pair {
        first: first.world,
        second: second.world,
        expected,
        basis: Vec::with_capacity(2),
    };
    for axis in [first.normal, second.normal] {
        if pair.axis(Axis::Normal(axis), work)? {
            return Ok(());
        }
    }
    for (normal, edges) in [(first.normal, first.edges), (second.normal, second.edges)] {
        for edge in edges {
            if pair.axis(Axis::Cross(normal, edge), work)? {
                return Ok(());
            }
        }
    }
    for a in first.edges {
        for b in second.edges {
            if pair.axis(Axis::Cross(a, b), work)? {
                return Ok(());
            }
        }
    }
    if matches!(kind, ContactKind::Authored)
        && expected.len() == 1
        && pair.basis.len() == 1
        && supported_point(first.world, second.world, expected[0], pair.basis[0], work)?
    {
        return Ok(());
    }
    Err(invalid(
        "Terrain sheets cross, overlap or intersect outside their native contacts",
    ))
}

pub(super) fn source(mesh: &DisplacementMesh, work: &mut Work) -> Result<Prepared, EditError> {
    let prepared = prepare(mesh, work)?;
    // A rank-two affine map carries the caller's complete positive BASE cover
    // injectively into world space.
    if prepared.affine.is_some() {
        return Ok(prepared);
    }
    let order = TriangleOrder::new(&prepared.triangles, prepared.bounds, false, work)?;
    for (index, first) in order.triangles.iter().enumerate() {
        for second in &order.triangles[index + 1..] {
            work.consume(PAIR_WORK).map_err(invalid)?;
            if second.low > first.high {
                break;
            }
            let first = &prepared.triangles[first.index];
            let second = &prepared.triangles[second.index];
            if apart(first.bounds, second.bounds) {
                continue;
            }
            work.consume(FIELD_WORK).map_err(invalid)?;
            if first.field.matches(&second.field)? {
                continue;
            }
            let expected = first
                .indices
                .iter()
                .filter(|index| second.indices.contains(index))
                .map(|&index| mesh.positions[index])
                .collect::<Vec<_>>();
            certify_pair(first, second, &expected, ContactKind::Native, work)?;
        }
    }
    Ok(prepared)
}

fn boundary_interval(
    triangle: &Triangle,
    allowed: [DVec3; 2],
    axes: [usize; 2],
    axis: usize,
    work: &mut Work,
) -> Result<Option<[DVec3; 2]>, EditError> {
    if same(allowed[0], allowed[1]) {
        work.consume(3 * polygon::ON_SEGMENT_WORK)
            .map_err(invalid)?;
        for index in 0..3 {
            if polygon::on_segment(
                allowed[0],
                [triangle.base[index], triangle.base[(index + 1) % 3]],
                axes,
            )? {
                return Ok(Some(allowed));
            }
        }
        return Ok(None);
    }
    work.consume(3 * FIELD_WORK).map_err(invalid)?;
    let mut interval: Option<[DVec3; 2]> = None;
    // The authorized line supports the convex BASE cage, so each triangle's
    // intersection is the convex hull of its vertices on that line.
    for point in triangle.base {
        if orient(allowed[0], allowed[1], point, axes)? == 0 {
            if let Some(bounds) = &mut interval {
                if point[axis] < bounds[0][axis] {
                    bounds[0] = point;
                }
                if point[axis] > bounds[1][axis] {
                    bounds[1] = point;
                }
            } else {
                interval = Some([point; 2]);
            }
        }
    }
    Ok(interval)
}

struct Contact {
    first: Vec<Option<[DVec3; 2]>>,
    second: Vec<Option<[DVec3; 2]>>,
    allowed: [DVec3; 2],
    axis: usize,
}

#[derive(Clone, Copy)]
enum ContactKind {
    Native,
    Authored,
}

fn common_positions(
    first: [DVec3; 2],
    second: [DVec3; 2],
    count: usize,
    kind: ContactKind,
    work: &mut Work,
) -> Result<Vec<DVec3>, EditError> {
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        if same(first[index], second[index]) {
            result.push(first[index]);
        } else if matches!(kind, ContactKind::Native) {
            return Err(invalid(
                "Terrain native seam has different exact embedded positions",
            ));
        }
    }
    if count != 2 || !result.is_empty() {
        return Ok(result);
    }
    work.consume(4 * FIELD_WORK).map_err(invalid)?;
    let start = difference(first[0], second[0])?;
    let end = difference(first[1], second[1])?;
    let axis = (0..3).find(|&axis| start[axis].value != 0).unwrap();
    if start[axis].value.signum() == end[axis].value.signum()
        || end[axis].value == 0
        || nonzero(cross(start, end)?)
    {
        return Ok(result);
    }
    let numerator = start[axis];
    let denominator = numerator.sub(end[axis])?;
    let mut point = DVec3::ZERO;
    for axis in 0..3 {
        let origin = Exact::float(first[0][axis])?;
        point[axis] = origin
            .mul(denominator)?
            .add(Exact::float(first[1][axis])?.sub(origin)?.mul(numerator)?)?
            .div(denominator)?
            .value()?;
    }
    result.push(point);
    Ok(result)
}

impl Contact {
    fn new(
        first: &Prepared,
        second: &Prepared,
        allowed: [DVec3; 2],
        axes: [usize; 2],
        work: &mut Work,
    ) -> Result<Self, EditError> {
        work.consume(PAIR_WORK * (first.triangles.len() + second.triangles.len()))
            .map_err(invalid)?;
        let axis = axes
            .into_iter()
            .find(|&axis| allowed[0][axis] != allowed[1][axis])
            .unwrap_or(axes[0]);
        let first = first
            .triangles
            .iter()
            .map(|triangle| boundary_interval(triangle, allowed, axes, axis, work))
            .collect::<Result<Vec<_>, _>>()?;
        let second = second
            .triangles
            .iter()
            .map(|triangle| boundary_interval(triangle, allowed, axes, axis, work))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            first,
            second,
            allowed,
            axis,
        })
    }

    fn expected(
        &self,
        first: &Triangle,
        first_index: usize,
        second: &Triangle,
        second_index: usize,
        kind: ContactKind,
        work: &mut Work,
    ) -> Result<Vec<DVec3>, EditError> {
        work.consume(2 * PAIR_WORK).map_err(invalid)?;
        let (Some(a), Some(b)) = (self.first[first_index], self.second[second_index]) else {
            return Ok(Vec::new());
        };
        let low = if a[0][self.axis] >= b[0][self.axis] {
            a[0]
        } else {
            b[0]
        };
        let high = if a[1][self.axis] <= b[1][self.axis] {
            a[1]
        } else {
            b[1]
        };
        if low[self.axis] > high[self.axis] {
            return Ok(Vec::new());
        }
        if low[self.axis] < self.allowed[0][self.axis].min(self.allowed[1][self.axis])
            || high[self.axis] > self.allowed[0][self.axis].max(self.allowed[1][self.axis])
        {
            return Err(invalid(
                "Terrain BASE contact extends outside its authorized native seam",
            ));
        }
        let points = [low, high];
        let count = if same(low, high) { 1 } else { 2 };
        let mut a = [first.position(points[0], work)?; 2];
        let mut b = [second.position(points[0], work)?; 2];
        if count == 2 {
            a[1] = first.position(points[1], work)?;
            b[1] = second.position(points[1], work)?;
        }
        common_positions(a, b, count, kind, work)
    }
}

// Callers certify complete, disjoint BASE interiors and native contact identity.
pub(super) fn between_prepared(
    first: &Prepared,
    second: &Prepared,
    allowed: Option<[DVec3; 2]>,
    work: &mut Work,
) -> Result<(), EditError> {
    between(first, second, allowed, ContactKind::Native, work)
}

pub(super) fn between_authored_prepared(
    first: &Prepared,
    second: &Prepared,
    allowed: Option<[DVec3; 2]>,
    work: &mut Work,
) -> Result<(), EditError> {
    between(first, second, allowed, ContactKind::Authored, work)
}

fn between(
    first: &Prepared,
    second: &Prepared,
    allowed: Option<[DVec3; 2]>,
    kind: ContactKind,
    work: &mut Work,
) -> Result<(), EditError> {
    work.consume(PAIR_WORK).map_err(invalid)?;
    if allowed.is_none() && apart(first.bounds, second.bounds) {
        return Ok(());
    }
    let axes = if let Some(allowed_contact) = allowed {
        work.consume(3 * FIELD_WORK).map_err(invalid)?;
        if !first.base.matches(&second.base)? {
            return Err(invalid(
                "Terrain embedded contacts require one exact native BASE plane",
            ));
        }
        for point in allowed_contact {
            if !first.base.contains(point)? {
                return Err(invalid(
                    "Terrain authorized native contact must belong to its exact BASE plane",
                ));
            }
        }
        let axis = first.base.axis();
        [(axis + 1) % 3, (axis + 2) % 3]
    } else {
        [0, 1]
    };
    if let (Some(a), Some(b)) = (&first.affine, &second.affine) {
        work.consume(FIELD_WORK).map_err(invalid)?;
        if a.matches(b)? {
            // A shared injective affine field carries exactly the caller's
            // complete BASE contact set into world space.
            return Ok(());
        }
    }
    let contact = allowed
        .map(|allowed| Contact::new(first, second, allowed, axes, work))
        .transpose()?;
    let first_triangles = &first.triangles;
    let second_triangles = &second.triangles;
    let world_order = TriangleOrder::new(
        second_triangles,
        [
            first.bounds[0].min(second.bounds[0]),
            first.bounds[1].max(second.bounds[1]),
        ],
        false,
        work,
    )?;
    let base_order = if allowed.is_some() {
        Some(TriangleOrder::new(
            second_triangles,
            [
                first.base_bounds[0].min(second.base_bounds[0]),
                first.base_bounds[1].max(second.base_bounds[1]),
            ],
            true,
            work,
        )?)
    } else {
        None
    };
    work.consume(second_triangles.len()).map_err(invalid)?;
    let mut visited = vec![0; second_triangles.len()];
    for (first_index, a) in first_triangles.iter().enumerate() {
        for (order, bounds) in std::iter::once((&world_order, a.bounds))
            .chain(base_order.as_ref().map(|order| (order, a.base_bounds)))
        {
            for candidate in &order.triangles[order.range(bounds, work)?] {
                work.consume(PAIR_WORK).map_err(invalid)?;
                let second_index = candidate.index;
                if visited[second_index] == first_index + 1 {
                    continue;
                }
                visited[second_index] = first_index + 1;
                let b = &second_triangles[second_index];
                let separated = apart(a.bounds, b.bounds);
                if separated && (allowed.is_none() || apart(a.base_bounds, b.base_bounds)) {
                    continue;
                }
                let expected = match &contact {
                    Some(contact) => {
                        contact.expected(a, first_index, b, second_index, kind, work)?
                    }
                    None => Vec::new(),
                };
                if separated {
                    if !expected.is_empty() {
                        return Err(invalid("Terrain lost an expected embedded native contact"));
                    }
                    continue;
                }
                work.consume(FIELD_WORK).map_err(invalid)?;
                if a.field.matches(&b.field)? {
                    continue;
                }
                certify_pair(a, b, &expected, kind, work)?;
            }
        }
    }
    Ok(())
}
