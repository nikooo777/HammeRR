use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;

use super::{Exact, FIELD_WORK, Work, invalid, key, orient, polygon, stored, volume};
use crate::EditError;

const MAX_VERTICES: usize = crate::clip::MAX_TERRAIN_PATCHES * 4;
const MAX_EDGES: usize = MAX_VERTICES * 3;
const MAX_TRIANGLES: usize = crate::clip::MAX_TERRAIN_PATCHES * 2;
const BOUNDS_WORK: usize = 8;

type Edge = [usize; 2];
type Boundary = BTreeMap<Edge, i8>;

fn edge(a: usize, b: usize) -> Edge {
    if a < b { [a, b] } else { [b, a] }
}

fn direction(a: usize, b: usize) -> i8 {
    if a < b { 1 } else { -1 }
}

fn turn(
    a: DVec3,
    b: DVec3,
    c: DVec3,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<i128, EditError> {
    work.consume(FIELD_WORK).map_err(invalid)?;
    orient(a, b, c, axes)
}

fn on_segment(
    point: DVec3,
    a: DVec3,
    b: DVec3,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<bool, EditError> {
    work.consume(polygon::ON_SEGMENT_WORK).map_err(invalid)?;
    polygon::on_segment(point, [a, b], axes)
}

fn conflicts(
    first: Edge,
    second: Edge,
    points: &[DVec3],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<bool, EditError> {
    work.consume(BOUNDS_WORK).map_err(invalid)?;
    if first.iter().any(|point| second.contains(point)) {
        return Ok(false);
    }
    let [a, b] = first.map(|index| points[index]);
    let [c, d] = second.map(|index| points[index]);
    if axes.into_iter().any(|axis| {
        a[axis].max(b[axis]) < c[axis].min(d[axis]) || c[axis].max(d[axis]) < a[axis].min(b[axis])
    }) {
        return Ok(false);
    }
    let sides = [
        turn(a, b, c, axes, work)?,
        turn(a, b, d, axes, work)?,
        turn(c, d, a, axes, work)?,
        turn(c, d, b, axes, work)?,
    ];
    Ok(sides[0] * sides[1] < 0 && sides[2] * sides[3] < 0
        || sides[0] == 0 && polygon::in_bounds(c, [a, b], axes)
        || sides[1] == 0 && polygon::in_bounds(d, [a, b], axes)
        || sides[2] == 0 && polygon::in_bounds(a, [c, d], axes)
        || sides[3] == 0 && polygon::in_bounds(b, [c, d], axes))
}

fn area(a: DVec3, b: DVec3, axes: [usize; 2], work: &mut Work) -> Result<Exact, EditError> {
    work.consume(FIELD_WORK).map_err(invalid)?;
    Exact::float(a[axes[0]])?
        .mul(Exact::float(b[axes[1]])?)?
        .sub(Exact::float(a[axes[1]])?.mul(Exact::float(b[axes[0]])?)?)
}

fn winding(
    samples: &[DVec3],
    points: &[DVec3],
    boundary: &Boundary,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<i32, EditError> {
    let denominator = Exact::new(samples.len() as i128, 0);
    let mut sums = [Exact::new(0, 0); 2];
    work.consume(FIELD_WORK * samples.len()).map_err(invalid)?;
    for sample in samples {
        for (sum, axis) in sums.iter_mut().zip(axes) {
            *sum = sum.add(Exact::float(sample[axis])?)?;
        }
    }
    let mut result = 0;
    for (&[first, second], &orientation) in boundary {
        work.consume(FIELD_WORK * (samples.len() + 4))
            .map_err(invalid)?;
        let [a, b] = if orientation > 0 {
            [points[first], points[second]]
        } else {
            [points[second], points[first]]
        };
        let mut side = Exact::new(0, 0);
        for sample in samples {
            side = side.add(volume::edge_value(a, b, *sample, axes)?)?;
        }
        let a_y = Exact::float(a[axes[1]])?
            .mul(denominator)?
            .sub(sums[1])?
            .value
            .signum();
        let b_y = Exact::float(b[axes[1]])?
            .mul(denominator)?
            .sub(sums[1])?
            .value
            .signum();
        if side.value == 0 {
            let a_x = Exact::float(a[axes[0]])?
                .mul(denominator)?
                .sub(sums[0])?
                .value
                .signum();
            let b_x = Exact::float(b[axes[0]])?
                .mul(denominator)?
                .sub(sums[0])?
                .value
                .signum();
            if a_y * b_y <= 0 && a_x * b_x <= 0 {
                return Err(invalid("Terrain triangulation sample lies on its boundary"));
            }
        }
        if a_y <= 0 && b_y > 0 && side.value > 0 {
            result += 1;
        }
        if a_y > 0 && b_y <= 0 && side.value < 0 {
            result -= 1;
        }
    }
    Ok(result)
}

fn boundary_graph(
    rings: &[Vec<DVec3>],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<(Vec<DVec3>, Boundary), EditError> {
    let mut unique = BTreeMap::new();
    let mut projected = BTreeMap::new();
    let mut entries = 0usize;
    for ring in rings {
        if ring.len() < 3 {
            return Err(invalid(
                "Terrain boundary cycle has fewer than three vertices",
            ));
        }
        entries = entries
            .checked_add(ring.len())
            .filter(|count| *count <= MAX_EDGES)
            .ok_or_else(|| invalid("Terrain boundary triangulation exceeds its edge limit"))?;
        work.consume(ring.len()).map_err(invalid)?;
        for &point in ring {
            stored(point)?;
            let identity = key(point);
            let projection = [identity[axes[0]], identity[axes[1]]];
            if projected
                .insert(projection, identity)
                .is_some_and(|previous| previous != identity)
            {
                return Err(invalid(
                    "Terrain boundary projection merges distinct source vertices",
                ));
            }
            unique.entry(identity).or_insert(point);
            if unique.len() > MAX_VERTICES {
                return Err(invalid(
                    "Terrain boundary triangulation exceeds its vertex limit",
                ));
            }
        }
    }
    let points = unique.values().copied().collect::<Vec<_>>();
    let indices = unique
        .keys()
        .enumerate()
        .map(|(index, &point)| (point, index))
        .collect::<BTreeMap<_, _>>();
    let mut result = Boundary::new();
    for ring in rings {
        for index in 0..ring.len() {
            let a = ring[index];
            let b = ring[(index + 1) % ring.len()];
            if key(a) == key(b) {
                return Err(invalid("Terrain boundary contains a collapsed edge"));
            }
            let axis = if a[axes[0]] != b[axes[0]] {
                axes[0]
            } else {
                axes[1]
            };
            let mut split = Vec::new();
            for (index, &point) in points.iter().enumerate() {
                if on_segment(point, a, b, axes, work)? {
                    split.push(index);
                }
            }
            work.consume(split.len() * split.len()).map_err(invalid)?;
            split.sort_by(|first, second| {
                let order = points[*first][axis].total_cmp(&points[*second][axis]);
                if a[axis] < b[axis] {
                    order
                } else {
                    order.reverse()
                }
            });
            if split.first().copied() != Some(indices[&key(a)])
                || split.last().copied() != Some(indices[&key(b)])
            {
                return Err(invalid(
                    "Terrain boundary lost an original segment endpoint",
                ));
            }
            for pair in split.windows(2) {
                if result
                    .insert(edge(pair[0], pair[1]), direction(pair[0], pair[1]))
                    .is_some()
                {
                    return Err(invalid(
                        "Terrain boundary contains overlapping directed segments",
                    ));
                }
                if result.len() > MAX_EDGES {
                    return Err(invalid(
                        "Terrain boundary triangulation exceeds its edge limit",
                    ));
                }
            }
        }
    }
    let edges = result.keys().copied().collect::<Vec<_>>();
    for (index, &first) in edges.iter().enumerate() {
        for &second in &edges[index + 1..] {
            if conflicts(first, second, &points, axes, work)? {
                return Err(invalid(
                    "Terrain boundary segments cross or touch without a shared vertex",
                ));
            }
        }
    }
    Ok((points, result))
}

fn angle(
    first: usize,
    second: usize,
    center: DVec3,
    points: &[DVec3],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Ordering, EditError> {
    let a = points[first];
    let b = points[second];
    let lower = |point: DVec3| {
        point[axes[1]] < center[axes[1]]
            || point[axes[1]] == center[axes[1]] && point[axes[0]] < center[axes[0]]
    };
    match lower(a).cmp(&lower(b)) {
        Ordering::Equal => match turn(center, a, b, axes, work)? {
            1 => Ok(Ordering::Less),
            -1 => Ok(Ordering::Greater),
            _ => Err(invalid("Terrain triangulation has overlapping edge rays")),
        },
        order => Ok(order),
    }
}

fn neighbors(
    points: &[DVec3],
    edges: &BTreeSet<Edge>,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Vec<Vec<usize>>, EditError> {
    let mut result = vec![Vec::new(); points.len()];
    work.consume(2 * edges.len()).map_err(invalid)?;
    for &[a, b] in edges {
        result[a].push(b);
        result[b].push(a);
    }
    for (center, adjacent) in result.iter_mut().enumerate() {
        work.consume(adjacent.len() * adjacent.len())
            .map_err(invalid)?;
        for index in 1..adjacent.len() {
            let mut current = index;
            while current > 0
                && angle(
                    adjacent[current - 1],
                    adjacent[current],
                    points[center],
                    points,
                    axes,
                    work,
                )? == Ordering::Greater
            {
                adjacent.swap(current - 1, current);
                current -= 1;
            }
        }
    }
    Ok(result)
}

fn faces(
    points: &[DVec3],
    edges: &BTreeSet<Edge>,
    boundary: &Boundary,
    axes: [usize; 2],
    sign: i128,
    work: &mut Work,
) -> Result<Vec<[usize; 3]>, EditError> {
    let adjacent = neighbors(points, edges, axes, work)?;
    let mut visited = BTreeSet::new();
    let mut result = Vec::new();
    let mut unique = BTreeSet::new();
    for &[a, b] in edges {
        for start in [[a, b], [b, a]] {
            if visited.contains(&start) {
                continue;
            }
            let mut current = start;
            let mut ring = Vec::new();
            let mut material = None;
            loop {
                work.consume(1).map_err(invalid)?;
                if !visited.insert(current) {
                    return Err(invalid(
                        "Terrain triangulation face traversal repeated a directed edge",
                    ));
                }
                let retained =
                    boundary
                        .get(&edge(current[0], current[1]))
                        .is_none_or(|orientation| {
                            i128::from(*orientation * direction(current[0], current[1])) * sign > 0
                        });
                if material.is_some_and(|previous| previous != retained) {
                    return Err(invalid(
                        "Terrain triangulation face disagrees with its retained boundary",
                    ));
                }
                material = Some(retained);
                ring.push(current[0]);
                if ring.len() > 2 * edges.len() {
                    return Err(invalid("Terrain triangulation face exceeds its edge limit"));
                }
                let next = &adjacent[current[1]];
                work.consume(next.len()).map_err(invalid)?;
                let incoming = next
                    .iter()
                    .position(|index| *index == current[0])
                    .ok_or_else(|| invalid("Terrain triangulation lost a reciprocal edge"))?;
                current = [current[1], next[(incoming + next.len() - 1) % next.len()]];
                if current == start {
                    break;
                }
            }
            if material != Some(true) {
                continue;
            }
            let mut triangle: [usize; 3] = ring.try_into().map_err(|_| {
                invalid("Terrain boundary visibility graph did not triangulate its retained domain")
            })?;
            let positions = triangle.map(|index| points[index]);
            if turn(positions[0], positions[1], positions[2], axes, work)? != 1
                || winding(&positions, points, boundary, axes, work)? != sign as i32
            {
                return Err(invalid(
                    "Terrain triangulation produced a nonpositive or exterior face",
                ));
            }
            let mut identity = triangle;
            identity.sort_unstable();
            if !unique.insert(identity) {
                return Err(invalid("Terrain triangulation repeated a retained face"));
            }
            if sign < 0 {
                triangle.swap(1, 2);
            }
            result.push(triangle);
            if result.len() > MAX_TRIANGLES {
                return Err(invalid(
                    "Terrain boundary triangulation exceeds its triangle limit",
                ));
            }
        }
    }
    if visited.len() != 2 * edges.len() {
        return Err(invalid("Terrain triangulation lost a directed graph edge"));
    }
    Ok(result)
}

fn certify(
    points: &[DVec3],
    triangles: &[[usize; 3]],
    edges: &BTreeSet<Edge>,
    boundary: &Boundary,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<(), EditError> {
    let mut incidence = BTreeMap::<Edge, [usize; 2]>::new();
    let mut expected_area = Exact::new(0, 0);
    for (&[a, b], &orientation) in boundary {
        expected_area = expected_area.add(
            area(points[a], points[b], axes, work)?.mul(Exact::new(i128::from(orientation), 0))?,
        )?;
    }
    let mut actual_area = Exact::new(0, 0);
    for triangle in triangles {
        for index in 0..3 {
            let a = triangle[index];
            let b = triangle[(index + 1) % 3];
            incidence.entry(edge(a, b)).or_default()[usize::from(a > b)] += 1;
            actual_area = actual_area.add(area(points[a], points[b], axes, work)?)?;
        }
    }
    if incidence.len() != edges.len() || actual_area != expected_area {
        return Err(invalid(
            "Terrain triangulation lost exact boundary or area coverage",
        ));
    }
    for edge in edges {
        work.consume(1).map_err(invalid)?;
        let counts = incidence
            .get(edge)
            .ok_or_else(|| invalid("Terrain triangulation lost a conforming boundary vertex"))?;
        let expected = match boundary.get(edge) {
            Some(1) => [1, 0],
            Some(-1) => [0, 1],
            None => [1, 1],
            _ => {
                return Err(invalid(
                    "Terrain triangulation has an invalid boundary orientation",
                ));
            }
        };
        if *counts != expected {
            return Err(invalid(
                "Terrain triangulation has missing, overlapping or reversed faces",
            ));
        }
    }
    Ok(())
}

pub(super) fn triangulate(
    rings: &[Vec<DVec3>],
    axes: [usize; 2],
    sign: i128,
    work: &mut Work,
) -> Result<Vec<[DVec3; 3]>, EditError> {
    if axes[0] >= 3 || axes[1] >= 3 || axes[0] == axes[1] || !matches!(sign, -1 | 1) {
        return Err(invalid(
            "Terrain triangulation requires a valid plane chart and orientation",
        ));
    }
    if rings.is_empty() {
        return Ok(Vec::new());
    }
    let (points, boundary) = boundary_graph(rings, axes, work)?;
    let mut signed_area = Exact::new(0, 0);
    for (&[a, b], &orientation) in &boundary {
        signed_area = signed_area.add(
            area(points[a], points[b], axes, work)?.mul(Exact::new(i128::from(orientation), 0))?,
        )?;
    }
    if signed_area.value.signum() != sign {
        return Err(invalid(
            "Terrain boundary cycles have inconsistent or empty retained area",
        ));
    }
    let mut edges = boundary.keys().copied().collect::<BTreeSet<_>>();
    for first in 0..points.len() {
        for second in first + 1..points.len() {
            work.consume(1).map_err(invalid)?;
            let candidate = [first, second];
            if edges.contains(&candidate) {
                continue;
            }
            let mut blocked = false;
            for (index, &point) in points.iter().enumerate() {
                if index != first
                    && index != second
                    && on_segment(point, points[first], points[second], axes, work)?
                {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                continue;
            }
            for &existing in &edges {
                if conflicts(candidate, existing, &points, axes, work)? {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                continue;
            }
            match winding(
                &[points[first], points[second]],
                &points,
                &boundary,
                axes,
                work,
            )? {
                0 => continue,
                value if i128::from(value) == sign => {}
                _ => {
                    return Err(invalid(
                        "Terrain boundary cycles overlap or have inconsistent winding",
                    ));
                }
            }
            edges.insert(candidate);
            if edges.len() > MAX_EDGES {
                return Err(invalid(
                    "Terrain boundary triangulation exceeds its edge limit",
                ));
            }
        }
    }
    let triangles = faces(&points, &edges, &boundary, axes, sign, work)?;
    certify(&points, &triangles, &edges, &boundary, axes, work)?;
    Ok(triangles
        .into_iter()
        .map(|triangle| triangle.map(|index| points[index]))
        .collect())
}
