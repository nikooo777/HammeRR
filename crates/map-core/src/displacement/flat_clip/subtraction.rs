use std::cmp::Ordering;

use glam::DVec3;

use super::{Exact, FIELD_WORK, Work, invalid, stored, volume};
use crate::EditError;
use crate::clip::TerrainCutPlane;

const MAX_POLYGONS: usize = 4_096;

pub(super) struct Patch {
    pub(super) base: [DVec3; 3],
    pub(super) world: [DVec3; 3],
    pub(super) ring: Vec<DVec3>,
}

pub(super) struct Polygon {
    pub(super) patch: usize,
    pub(super) ring: Vec<DVec3>,
    pub(super) unchanged: bool,
    pub(super) conformed: bool,
}

#[derive(Clone, Copy)]
struct Vertex {
    values: [Exact; 3],
    denominator: Exact,
}

struct Logical {
    patch: usize,
    ring: Vec<Vertex>,
    unchanged: bool,
}

fn compare(a: Vertex, b: Vertex, axis: usize) -> Result<Ordering, EditError> {
    Ok(a.values[axis]
        .mul(b.denominator)?
        .sub(b.values[axis].mul(a.denominator)?)?
        .value
        .cmp(&0))
}

fn same(a: Vertex, b: Vertex) -> Result<bool, EditError> {
    for axis in 0..3 {
        if compare(a, b, axis)? != Ordering::Equal {
            return Ok(false);
        }
    }
    Ok(true)
}

fn vertex(point: DVec3) -> Result<Vertex, EditError> {
    let values: [Exact; 3] = point
        .to_array()
        .into_iter()
        .map(Exact::float)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| invalid("Terrain polygon lost a source coordinate"))?;
    Ok(Vertex {
        values,
        denominator: Exact::new(1, 0),
    })
}

fn mapped(point: volume::Point, base: [DVec3; 3]) -> Result<Vertex, EditError> {
    let mut values = [Exact::new(0, 0); 3];
    for (axis, value) in values.iter_mut().enumerate() {
        let origin = Exact::float(base[0][axis])?;
        *value = origin
            .mul(point[2])?
            .add(Exact::float(base[1][axis])?.sub(origin)?.mul(point[0])?)?
            .add(Exact::float(base[2][axis])?.sub(origin)?.mul(point[1])?)?;
    }
    Ok(Vertex {
        values,
        denominator: point[2],
    })
}

fn orientation(a: Vertex, b: Vertex, c: Vertex, axes: [usize; 2]) -> Result<i128, EditError> {
    let [x, y] = axes;
    Ok(a.values[x]
        .mul(
            b.values[y]
                .mul(c.denominator)?
                .sub(c.values[y].mul(b.denominator)?)?,
        )?
        .sub(
            a.values[y].mul(
                b.values[x]
                    .mul(c.denominator)?
                    .sub(c.values[x].mul(b.denominator)?)?,
            )?,
        )?
        .add(
            a.denominator.mul(
                b.values[x]
                    .mul(c.values[y])?
                    .sub(c.values[x].mul(b.values[y])?)?,
            )?,
        )?
        .value
        .signum())
}

fn ordered(
    mut ring: Vec<Vertex>,
    axes: [usize; 2],
    sign: i128,
    work: &mut Work,
) -> Result<Vec<Vertex>, EditError> {
    work.consume(FIELD_WORK * ring.len() * ring.len())
        .map_err(invalid)?;
    let mut first = 0;
    for index in 1..ring.len() {
        let order = compare(ring[index], ring[first], axes[0])?;
        if order == Ordering::Less
            || order == Ordering::Equal
                && compare(ring[index], ring[first], axes[1])? == Ordering::Less
        {
            first = index;
        }
    }
    ring.swap(0, first);
    for index in 2..ring.len() {
        let mut current = index;
        while current > 1 && orientation(ring[0], ring[current - 1], ring[current], axes)? < 0 {
            ring.swap(current - 1, current);
            current -= 1;
        }
    }
    if sign < 0 {
        ring[1..].reverse();
    }
    Ok(ring)
}

fn source_region(
    patch: &Patch,
    axes: [usize; 2],
    work: &mut Work,
) -> Result<volume::Region, EditError> {
    let sign = super::orient(patch.ring[0], patch.ring[1], patch.ring[2], axes)?;
    let multiplier = Exact::new(-sign, 0);
    work.consume(FIELD_WORK * patch.ring.len() * 3)
        .map_err(invalid)?;
    let mut constraints = Vec::new();
    for index in 0..patch.ring.len() {
        let values = patch.base.map(|point| {
            volume::edge_value(
                patch.ring[index],
                patch.ring[(index + 1) % patch.ring.len()],
                point,
                axes,
            )
        });
        let values: [Exact; 3] = values
            .into_iter()
            .map(|value| value?.mul(multiplier))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| invalid("Terrain polygon lost its source basis"))?;
        constraints.push(volume::from_values(values)?);
    }
    volume::Region::new(constraints, work)?
        .ok_or_else(|| invalid("Terrain subtraction lost its source region"))
}

fn footprint(
    patch: &Patch,
    planes: &[TerrainCutPlane],
    work: &mut Work,
) -> Result<Option<Vec<volume::Constraint>>, EditError> {
    work.consume(FIELD_WORK * planes.len() * 3)
        .map_err(invalid)?;
    let mut constraints = Vec::new();
    for plane in planes {
        let values: [Exact; 3] = patch
            .world
            .map(|point| volume::distance(point, plane))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| invalid("Terrain cutter lost its source basis"))?;
        if values.iter().all(|value| value.value == 0) {
            return Ok(None);
        }
        constraints.push(volume::from_values(values)?);
    }
    Ok(Some(constraints))
}

fn unchanged(ring: &[Vertex], patch: &Patch, work: &mut Work) -> Result<bool, EditError> {
    if ring.len() != patch.ring.len() {
        return Ok(false);
    }
    work.consume(FIELD_WORK * ring.len() * patch.ring.len())
        .map_err(invalid)?;
    for &point in &patch.ring {
        let point = vertex(point)?;
        let mut found = false;
        for &candidate in ring {
            if same(point, candidate)? {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

fn on_segment(point: Vertex, a: Vertex, b: Vertex, axes: [usize; 2]) -> Result<bool, EditError> {
    if orientation(a, b, point, axes)? != 0 {
        return Ok(false);
    }
    for axis in axes {
        let start = compare(point, a, axis)?;
        let end = compare(point, b, axis)?;
        if start == Ordering::Less && end == Ordering::Less
            || start == Ordering::Greater && end == Ordering::Greater
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn plan(
    patches: &[Patch],
    planes: &[Vec<TerrainCutPlane>],
    work: &mut Work,
) -> Result<Option<Vec<Polygon>>, EditError> {
    let first = patches
        .first()
        .ok_or_else(|| invalid("Terrain subtraction has no source fields"))?;
    let base = super::BasePlane::new(first.base)?;
    let axis = base.axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let facets = crate::clip::terrain_union_facets(planes, work)?;
    let mut polygons = Vec::new();
    let mut changed = false;
    for (patch_index, patch) in patches.iter().enumerate() {
        let source = source_region(patch, axes, work)?;
        let mut regions = vec![source];
        let mut footprints = Vec::new();
        for volume in planes {
            if let Some(region) = footprint(patch, volume, work)? {
                footprints.push(region);
            }
        }
        for facet in &facets {
            if volume::on_plane(&patch.world, &facet.plane, work)?
                && let Some(region) = footprint(patch, &facet.bounds, work)?
            {
                footprints.push(region);
            }
        }
        for footprint in footprints {
            let mut next = Vec::new();
            for region in regions {
                next.extend(region.subtract(&footprint, work)?);
            }
            if next.len() > MAX_POLYGONS {
                return Err(invalid(
                    "Terrain logical subtraction exceeds its region limit",
                ));
            }
            regions = next;
            if regions.is_empty() {
                break;
            }
        }
        let single = regions.len() == 1;
        let sign = super::orient(patch.ring[0], patch.ring[1], patch.ring[2], axes)?;
        if regions.is_empty() {
            changed = true;
        }
        for region in regions {
            let ring = region
                .points()
                .iter()
                .map(|&point| mapped(point, patch.base))
                .collect::<Result<Vec<_>, _>>()?;
            let ring = ordered(ring, axes, sign, work)?;
            let unchanged = single && unchanged(&ring, patch, work)?;
            changed |= !unchanged;
            polygons.push(Logical {
                patch: patch_index,
                ring,
                unchanged,
            });
        }
        if polygons.len() > MAX_POLYGONS {
            return Err(invalid(
                "Terrain logical subtraction exceeds its region limit",
            ));
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut native = true;
    work.consume(
        FIELD_WORK
            * polygons
                .iter()
                .map(|polygon| polygon.ring.len())
                .sum::<usize>(),
    )
    .map_err(invalid)?;
    for polygon in &polygons {
        for &point in &polygon.ring {
            native &= native_vertex(point)?;
        }
    }
    if !native {
        return coalesced(patches, &polygons, axes, work).map(Some);
    }
    if polygons.len() > crate::clip::MAX_TERRAIN_PATCHES {
        return Err(invalid(
            "Terrain subtraction exceeds its output patch limit",
        ));
    }
    let points = polygons
        .iter()
        .flat_map(|polygon| polygon.ring.iter().copied())
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    for polygon in polygons {
        let ring = conform(&polygon.ring, &points, axes, work)?;
        let conformed = ring.len() != polygon.ring.len();
        let ring = stored_ring(&ring)?;
        result.push(Polygon {
            patch: polygon.patch,
            ring,
            unchanged: polygon.unchanged,
            conformed,
        });
    }
    Ok(Some(result))
}

pub(super) fn triangles(
    ring: &[DVec3],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Vec<Vec<DVec3>>, EditError> {
    if ring.len() < 3 {
        return Err(invalid("Terrain conforming polygon is collapsed"));
    }
    let mut remaining = ring.to_vec();
    let sign = (0..ring.len())
        .map(|index| {
            super::orient(
                ring[index],
                ring[(index + 1) % ring.len()],
                ring[(index + 2) % ring.len()],
                axes,
            )
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|&value| value != 0)
        .ok_or_else(|| invalid("Terrain conforming polygon is collinear"))?;
    let mut result = Vec::new();
    while remaining.len() > 3 {
        work.consume(super::polygon::SEGMENT_WORK * remaining.len() * remaining.len())
            .map_err(invalid)?;
        let mut ear = None;
        for index in 0..remaining.len() {
            let a = remaining[(index + remaining.len() - 1) % remaining.len()];
            let b = remaining[index];
            let c = remaining[(index + 1) % remaining.len()];
            if super::orient(a, b, c, axes)? != sign {
                continue;
            }
            let mut blocked = false;
            for (other, &point) in remaining.iter().enumerate() {
                if [
                    index,
                    (index + 1) % remaining.len(),
                    (index + remaining.len() - 1) % remaining.len(),
                ]
                .contains(&other)
                {
                    continue;
                }
                if super::polygon::on_segment(point, [a, c], axes)? {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                ear = Some(index);
                break;
            }
        }
        let index = ear.ok_or_else(|| {
            invalid("Terrain polygon cannot preserve every conforming boundary vertex")
        })?;
        result.push(vec![
            remaining[(index + remaining.len() - 1) % remaining.len()],
            remaining[index],
            remaining[(index + 1) % remaining.len()],
        ]);
        remaining.remove(index);
    }
    result.push(remaining);
    Ok(result)
}

fn native_vertex(point: Vertex) -> Result<bool, EditError> {
    if point.denominator.value <= 0 {
        return Err(super::numeric());
    }
    let mut candidate = DVec3::ZERO;
    for (axis, value) in point.values.into_iter().enumerate() {
        if value.value.checked_rem(point.denominator.value) != Some(0) {
            return Ok(false);
        }
        let value = value.div(point.denominator)?;
        if value.value == 0 {
            continue;
        }
        let bits = i32::try_from(i128::BITS - value.value.unsigned_abs().leading_zeros())
            .map_err(|_| super::numeric())?;
        if bits > f32::MANTISSA_DIGITS as i32
            || value.exponent < f32::MIN_EXP - f32::MANTISSA_DIGITS as i32
            || value.exponent + bits > f32::MAX_EXP
        {
            return Ok(false);
        }
        candidate[axis] = value.value()?;
    }
    Ok(super::same(
        candidate,
        super::native::stored_vertex(candidate),
    ))
}

fn stored_ring(ring: &[Vertex]) -> Result<Vec<DVec3>, EditError> {
    ring.iter()
        .map(|point| {
            let mut result = DVec3::ZERO;
            for axis in 0..3 {
                result[axis] = point.values[axis].div(point.denominator)?.value()?;
            }
            stored(result)
        })
        .collect()
}

fn conform(
    ring: &[Vertex],
    points: &[Vertex],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Vec<Vertex>, EditError> {
    work.consume(FIELD_WORK * ring.len() * points.len())
        .map_err(invalid)?;
    let mut result = Vec::new();
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        let mut edge = vec![a];
        for &point in points {
            if same(point, b)? || !on_segment(point, a, b, axes)? {
                continue;
            }
            let mut duplicate = false;
            for &other in &edge {
                if same(point, other)? {
                    duplicate = true;
                    break;
                }
            }
            if !duplicate {
                edge.push(point);
            }
        }
        let axis = if compare(a, b, axes[0])? != Ordering::Equal {
            axes[0]
        } else {
            axes[1]
        };
        let direction = compare(a, b, axis)?;
        work.consume(FIELD_WORK * edge.len() * edge.len())
            .map_err(invalid)?;
        for index in 1..edge.len() {
            let mut current = index;
            while current > 0 && compare(edge[current - 1], edge[current], axis)? != direction {
                edge.swap(current - 1, current);
                current -= 1;
            }
        }
        result.extend(edge);
    }
    Ok(result)
}

fn node(nodes: &mut Vec<Vertex>, point: Vertex, work: &mut Work) -> Result<usize, EditError> {
    work.consume(FIELD_WORK * nodes.len()).map_err(invalid)?;
    for (index, &other) in nodes.iter().enumerate() {
        if same(point, other)? {
            return Ok(index);
        }
    }
    nodes.push(point);
    Ok(nodes.len() - 1)
}

fn angle(origin: Vertex, a: Vertex, b: Vertex, axes: [usize; 2]) -> Result<Ordering, EditError> {
    let half = |point| -> Result<bool, EditError> {
        let y = compare(point, origin, axes[1])?;
        Ok(y == Ordering::Less
            || y == Ordering::Equal && compare(point, origin, axes[0])? == Ordering::Less)
    };
    let halves = half(a)?.cmp(&half(b)?);
    if halves != Ordering::Equal {
        return Ok(halves);
    }
    Ok(orientation(origin, a, b, axes)?.cmp(&0).reverse())
}

fn boundaries(
    polygons: &[&Logical],
    axes: [usize; 2],
    sign: i128,
    work: &mut Work,
) -> Result<Vec<Vec<Vertex>>, EditError> {
    use std::collections::BTreeSet;

    let points = polygons
        .iter()
        .flat_map(|polygon| polygon.ring.iter().copied())
        .collect::<Vec<_>>();
    let mut nodes = Vec::new();
    let mut edges = BTreeSet::new();
    for polygon in polygons {
        let ring = conform(&polygon.ring, &points, axes, work)?;
        let ids = ring
            .iter()
            .map(|&point| node(&mut nodes, point, work))
            .collect::<Result<Vec<_>, _>>()?;
        work.consume(ids.len()).map_err(invalid)?;
        for index in 0..ids.len() {
            let edge = (ids[index], ids[(index + 1) % ids.len()]);
            if edge.0 == edge.1 {
                return Err(invalid(
                    "Terrain retained boundary contains a collapsed edge",
                ));
            }
            if !edges.remove(&(edge.1, edge.0)) && !edges.insert(edge) {
                return Err(invalid(
                    "Terrain retained domains duplicate an oriented edge",
                ));
            }
        }
    }
    let edges = edges.into_iter().collect::<Vec<_>>();
    let mut outgoing = vec![Vec::new(); nodes.len()];
    let mut incoming = vec![0usize; nodes.len()];
    work.consume(nodes.len() + 3 * edges.len())
        .map_err(invalid)?;
    for (index, &(a, b)) in edges.iter().enumerate() {
        outgoing[a].push(index);
        incoming[b] += 1;
    }
    if outgoing
        .iter()
        .zip(&incoming)
        .any(|(outgoing, incoming)| outgoing.len() != *incoming)
    {
        return Err(invalid("Terrain retained boundary is not closed"));
    }
    for (origin, candidates) in outgoing.iter_mut().enumerate() {
        work.consume(FIELD_WORK * candidates.len() * candidates.len())
            .map_err(invalid)?;
        for index in 1..candidates.len() {
            let mut current = index;
            while current > 0
                && angle(
                    nodes[origin],
                    nodes[edges[candidates[current - 1]].1],
                    nodes[edges[candidates[current]].1],
                    axes,
                )? == Ordering::Greater
            {
                candidates.swap(current - 1, current);
                current -= 1;
            }
        }
    }
    let mut successors = Vec::with_capacity(edges.len());
    for &(a, b) in &edges {
        let candidates = &outgoing[b];
        work.consume(FIELD_WORK * candidates.len())
            .map_err(invalid)?;
        let mut next = None;
        if sign > 0 {
            for &candidate in candidates {
                if angle(nodes[b], nodes[edges[candidate].1], nodes[a], axes)? == Ordering::Less {
                    next = Some(candidate);
                }
            }
            successors.push(
                next.unwrap_or(
                    *candidates
                        .last()
                        .ok_or_else(|| invalid("Terrain boundary has an open endpoint"))?,
                ),
            );
        } else {
            for &candidate in candidates {
                if angle(nodes[b], nodes[edges[candidate].1], nodes[a], axes)? == Ordering::Greater
                {
                    next = Some(candidate);
                    break;
                }
            }
            successors.push(
                next.unwrap_or(
                    *candidates
                        .first()
                        .ok_or_else(|| invalid("Terrain boundary has an open endpoint"))?,
                ),
            );
        }
    }
    let mut visited = vec![false; edges.len()];
    let mut result = Vec::new();
    for start in 0..edges.len() {
        if visited[start] {
            continue;
        }
        let mut current = start;
        let mut ring = Vec::new();
        loop {
            work.consume(1).map_err(invalid)?;
            if visited[current] {
                if current == start {
                    break;
                }
                return Err(invalid(
                    "Terrain retained boundary has an ambiguous junction",
                ));
            }
            visited[current] = true;
            ring.push(edges[current].0);
            current = successors[current];
        }
        loop {
            let mut remove = None;
            work.consume(FIELD_WORK * ring.len()).map_err(invalid)?;
            for index in 0..ring.len() {
                if outgoing[ring[index]].len() > 1 {
                    continue;
                }
                let a = nodes[ring[(index + ring.len() - 1) % ring.len()]];
                let b = nodes[ring[index]];
                let c = nodes[ring[(index + 1) % ring.len()]];
                if orientation(a, b, c, axes)? == 0 && on_segment(b, a, c, axes)? {
                    remove = Some(index);
                    break;
                }
            }
            if let Some(index) = remove {
                ring.remove(index);
            } else {
                break;
            }
            if ring.len() < 3 {
                return Err(invalid(
                    "Terrain retained boundary collapsed during cleanup",
                ));
            }
        }
        result.push(ring.into_iter().map(|index| nodes[index]).collect());
    }
    Ok(result)
}

fn coalesced(
    patches: &[Patch],
    polygons: &[Logical],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Vec<Polygon>, EditError> {
    struct Domain {
        patch: usize,
        sign: i128,
        rings: Vec<Vec<Vertex>>,
        native: bool,
    }
    let mut domains = Vec::new();
    for (patch, source) in patches.iter().enumerate() {
        let parts = polygons
            .iter()
            .filter(|polygon| polygon.patch == patch)
            .collect::<Vec<_>>();
        if parts.is_empty() {
            continue;
        }
        let mut native = true;
        work.consume(FIELD_WORK * parts.iter().map(|part| part.ring.len()).sum::<usize>())
            .map_err(invalid)?;
        for part in &parts {
            for &point in &part.ring {
                native &= native_vertex(point)?;
            }
        }
        let sign = super::orient(source.ring[0], source.ring[1], source.ring[2], axes)?;
        let rings = if native {
            parts.iter().map(|part| part.ring.clone()).collect()
        } else {
            boundaries(&parts, axes, sign, work)?
        };
        domains.push(Domain {
            patch,
            sign,
            rings,
            native,
        });
    }
    let points = domains
        .iter()
        .flat_map(|domain| domain.rings.iter().flatten().copied())
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    for domain in domains {
        let mut rings = Vec::new();
        for (index, ring) in domain.rings.iter().enumerate() {
            let conformed = conform(ring, &points, axes, work)?;
            let stored = stored_ring(&conformed)?;
            if domain.native {
                let source = polygons
                    .iter()
                    .filter(|polygon| polygon.patch == domain.patch)
                    .nth(index)
                    .ok_or_else(super::numeric)?;
                result.push(Polygon {
                    patch: domain.patch,
                    ring: stored,
                    unchanged: source.unchanged,
                    conformed: conformed.len() != ring.len(),
                });
            } else {
                rings.push(stored);
            }
        }
        if !domain.native {
            let triangles = super::triangulate::triangulate(&rings, axes, domain.sign, work)?;
            for triangle in triangles {
                result.push(Polygon {
                    patch: domain.patch,
                    ring: triangle.to_vec(),
                    unchanged: false,
                    conformed: false,
                });
            }
        }
    }
    Ok(result)
}
