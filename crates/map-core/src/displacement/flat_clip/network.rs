use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::{
    bsp,
    vmf::{Side, Solid, Vmf},
};

use super::{
    CUT_EPSILON, Exact, FIELD_WORK, Phase, data,
    field::{BasePlane, PlanarField},
    invalid, orient, reconstructed, same, weighted,
};
use crate::clip::Keep;
use crate::displacement::{
    grid_clip::{self, Context, Work},
    lod, native, neighbors, triangles,
};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Face, ObjectId, Plane};

pub(crate) struct Replacement<'a> {
    pub source: FaceId,
    pub parts: &'a [Solid],
}

pub(crate) struct Verification {
    pub neighbors: usize,
    pub links: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Cells,
}

struct Raw<'a> {
    source: FaceId,
    side: &'a Side,
    face: Face,
    supported: bool,
    cage: Vec<DVec3>,
}

struct Surface<'a> {
    raw: Raw<'a>,
    info: bsp::DisplacementInfo,
    power: u32,
    corners: [DVec3; 4],
    base_positions: Vec<DVec3>,
    positions: Vec<Vec<DVec3>>,
    sheet: Sheet,
}

struct Sheet {
    base: BasePlane,
    field: Option<PlanarField>,
    sign: i128,
}

impl Sheet {
    fn same_base(&self, other: &Self) -> Result<bool, EditError> {
        Ok(self.sign == other.sign && self.base.matches(&other.base)?)
    }

    fn matches(&self, other: &Self) -> Result<bool, EditError> {
        if !self.same_base(other)? {
            return Ok(false);
        }
        match (&self.field, &other.field) {
            (Some(first), Some(second)) => first.matches(second),
            (None, None) => Ok(true),
            _ => Ok(false),
        }
    }
}

impl Surface<'_> {
    fn grid(&self) -> neighbors::SurfaceGrid<'_> {
        neighbors::SurfaceGrid {
            power: self.power,
            corners: self.corners,
            positions: self.positions.last().unwrap(),
        }
    }
}

struct Contact {
    sources: [FaceId; 2],
    points: [DVec3; 2],
}

impl Contact {
    fn new(mut sources: [FaceId; 2], points: [DVec3; 2]) -> Self {
        sources.sort_unstable();
        Self { sources, points }
    }
}

#[derive(Default)]
struct Graph {
    pairs: Vec<neighbors::NeighborPair>,
    edges: Vec<Contact>,
    corners: Vec<Contact>,
}

fn bounds(points: &[DVec3]) -> (DVec3, DVec3) {
    points
        .iter()
        .fold((DVec3::MAX, DVec3::MIN), |(low, high), &point| {
            (low.min(point), high.max(point))
        })
}

fn apart(first: &[DVec3], second: &[DVec3]) -> bool {
    let (a, b) = bounds(first);
    let (c, d) = bounds(second);
    (0..3).any(|axis| {
        b[axis] + neighbors::EDGE_TOLERANCE < c[axis]
            || d[axis] + neighbors::EDGE_TOLERANCE < a[axis]
    })
}

fn dot(point: DVec3, axis: [Exact; 3]) -> Result<Exact, EditError> {
    Exact::float(point.x)?
        .mul(axis[0])?
        .add(Exact::float(point.y)?.mul(axis[1])?)?
        .add(Exact::float(point.z)?.mul(axis[2])?)
}

fn range(points: &[DVec3], axis: [Exact; 3]) -> Result<[Exact; 2], EditError> {
    let mut result = [dot(points[0], axis)?; 2];
    for &point in &points[1..] {
        let value = dot(point, axis)?;
        if value.sub(result[0])?.value < 0 {
            result[0] = value;
        }
        if value.sub(result[1])?.value > 0 {
            result[1] = value;
        }
    }
    Ok(result)
}

fn contact_rank(
    first: &[DVec3],
    second: &[DVec3],
    work: &mut Work,
) -> Result<Option<usize>, EditError> {
    work.consume(1).map_err(invalid)?;
    if apart(first, second) {
        return Ok(None);
    }
    if first.len() < 3 || second.len() < 3 {
        return Ok(Some(0));
    }
    let plane_work = first
        .len()
        .checked_add(second.len())
        .and_then(|count| count.checked_add(1))
        .and_then(|count| count.checked_mul(FIELD_WORK))
        .ok_or_else(|| invalid("Flat terrain contact work overflow"))?;
    work.consume(plane_work).map_err(invalid)?;
    let base = BasePlane::new([first[0], first[1], first[2]])?;
    for &point in first.iter().chain(second) {
        if !base.contains(point)? {
            return Ok(Some(0));
        }
    }
    let axis = base.axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    projected_contact_rank(first, second, axes, work)
}

fn projected_contact_rank(
    first: &[DVec3],
    second: &[DVec3],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<Option<usize>, EditError> {
    if first.len() < 3 || second.len() < 3 {
        return Err(invalid(
            "Terrain projected footprints require complete polygons",
        ));
    }
    let projection_work = first
        .len()
        .checked_add(second.len())
        .and_then(|count| count.checked_mul(8))
        .ok_or_else(|| invalid("Flat terrain contact work overflow"))?;
    let mut touching = Vec::new();
    for points in [first, second] {
        for edge in 0..points.len() {
            work.consume(projection_work + 8).map_err(invalid)?;
            let a = points[edge];
            let b = points[(edge + 1) % points.len()];
            let mut normal = [Exact::new(0, 0); 3];
            normal[axes[0]] = Exact::float(a[axes[1]])?.sub(Exact::float(b[axes[1]])?)?;
            normal[axes[1]] = Exact::float(b[axes[0]])?.sub(Exact::float(a[axes[0]])?)?;
            let a = range(first, normal)?;
            let b = range(second, normal)?;
            if a[1].sub(b[0])?.value < 0 || b[1].sub(a[0])?.value < 0 {
                return Ok(None);
            }
            if a[1] == b[0] || b[1] == a[0] {
                touching.push(normal);
            }
        }
    }
    let Some(&first) = touching.first() else {
        return Ok(Some(0));
    };
    for &second in &touching[1..] {
        work.consume(4).map_err(invalid)?;
        if first[axes[0]]
            .mul(second[axes[1]])?
            .sub(first[axes[1]].mul(second[axes[0]])?)?
            .value
            != 0
        {
            // Independent support lines confine the intersection to one corner.
            return Ok(Some(2));
        }
    }
    Ok(Some(1))
}

fn collect<'a>(vmf: &'a Vmf, work: &mut Work) -> Result<Vec<Raw<'a>>, EditError> {
    if vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
    }) {
        return Err(invalid(
            "Connected flat terrain requires complete resolved visible-instance context",
        ));
    }
    let mut result = Vec::new();
    for (solid, supported) in native::visible_terrain_sources(vmf) {
        work.consume(solid.sides.len()).map_err(invalid)?;
        let shape = BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
        for face in shape.faces {
            let side = &solid.sides[face.side];
            if side.displacement.is_none() {
                continue;
            }
            if result.len() == bsp::MAX_DISPLACEMENTS {
                return Err(invalid(
                    "Flat terrain context exceeds the native displacement limit",
                ));
            }
            work.consume(face.vertices.len()).map_err(invalid)?;
            let cage = face
                .vertices
                .iter()
                .copied()
                .map(native::stored_vertex)
                .collect();
            result.push(Raw {
                source: FaceId {
                    solid: solid.id,
                    side: side.id,
                },
                side,
                face,
                supported,
                cage,
            });
        }
    }
    Ok(result)
}

fn affected_sources<'a>(
    raw: Vec<Raw<'a>>,
    selected: &BTreeSet<FaceId>,
    work: &mut Work,
) -> Result<Vec<Raw<'a>>, EditError> {
    let mut affected = BTreeSet::new();
    for source in selected {
        let found = raw
            .iter()
            .enumerate()
            .filter(|(_, raw)| raw.source == *source)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [index] = found.as_slice() else {
            return Err(invalid(
                "Flat terrain context lost a unique selected source identity",
            ));
        };
        affected.insert(*index);
    }
    let mut queue = affected.iter().copied().collect::<Vec<_>>();
    let mut cursor = 0;
    while cursor < queue.len() {
        let first = queue[cursor];
        cursor += 1;
        for second in 0..raw.len() {
            if affected.contains(&second) {
                continue;
            }
            work.consume(1).map_err(invalid)?;
            if apart(&raw[first].cage, &raw[second].cage) {
                continue;
            }
            let comparisons = raw[first]
                .cage
                .len()
                .checked_mul(raw[second].cage.len())
                .ok_or_else(|| invalid("Flat terrain contact work overflow"))?;
            work.consume(comparisons).map_err(invalid)?;
            if grid_clip::related(&raw[first].cage, &raw[second].cage)
                || contact_rank(&raw[first].cage, &raw[second].cage, work)?.is_some()
            {
                if affected.len() >= selected.len() + grid_clip::MAX_NEIGHBORS {
                    return Err(invalid(
                        "Flat terrain context exceeds its affected-neighbor limit",
                    ));
                }
                affected.insert(second);
                queue.push(second);
            }
        }
    }
    Ok(raw
        .into_iter()
        .enumerate()
        .filter(|(index, _)| affected.contains(index))
        .map(|(_, raw)| raw)
        .collect())
}

fn prepare<'a>(
    raw: Raw<'a>,
    mode: Mode,
    phase: Phase,
    work: &mut Work,
) -> Result<Surface<'a>, EditError> {
    if !raw.supported {
        return Err(invalid(
            "Flat terrain neighbors must be world or func_detail terrain",
        ));
    }
    let info = raw.side.displacement.as_ref().unwrap();
    let (_, mut mesh) = native::checked_mesh(&raw.face, info).map_err(invalid)?;
    work.consume(mesh.positions.len() + (mesh.size - 1).pow(2) * 2)
        .map_err(invalid)?;
    triangles::native_tags(info, &mesh)?;
    let corner_indices = [
        0,
        (mesh.size - 1) * mesh.size,
        mesh.size * mesh.size - 1,
        mesh.size - 1,
    ];
    let corners = corner_indices.map(|index| native::stored_vertex(mesh.base_positions[index]));
    let (data, base) = data(&raw.face, &mesh, info.power)?;
    let positions = reconstructed(&data, base, work)?;
    super::native_base(&mut mesh, &data, base, work)?;
    let base_plane = BasePlane::new([corners[0], corners[1], corners[2]])?;
    let field = match mode {
        Mode::Flat => {
            work.consume(FIELD_WORK * 17).map_err(invalid)?;
            Some(PlanarField::new(
                corners,
                corner_indices.map(|index| mesh.positions[index]),
            )?)
        }
        Mode::Cells => {
            super::projected_sheet(&mesh, &positions, phase, work)?;
            None
        }
    };
    let axis = base_plane.axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let sign = orient(corners[0], corners[1], corners[2], axes)?;
    if sign == 0 {
        return Err(invalid(
            "Flat terrain contacts require noncollapsed native cages",
        ));
    }
    super::convex(&corners, axes, sign)?;
    let visible_axis = field.as_ref().map_or(axis, PlanarField::world_axis);
    let visible_axes = [(visible_axis + 1) % 3, (visible_axis + 2) % 3];
    let visible_sign = orient(
        mesh.positions[corner_indices[0]],
        mesh.positions[corner_indices[1]],
        mesh.positions[corner_indices[2]],
        visible_axes,
    )?;
    if visible_sign == 0 {
        return Err(invalid(
            "Connected terrain contacts require noncollapsed visible native sheets",
        ));
    }
    if let Some(field) = &field {
        work.consume(mesh.positions.len() * (positions.len() + FIELD_WORK * 3))
            .map_err(invalid)?;
        for (index, &point) in mesh.positions.iter().enumerate() {
            let base = mesh.base_positions[index];
            if !base_plane.contains(base)? {
                return Err(invalid(
                    "Connected terrain base samples must share one exact native plane",
                ));
            }
            let expected = field.position(base)?;
            if !same(base, field.base_position(point)?)
                || !same(point, expected)
                || positions
                    .iter()
                    .any(|positions| !same(expected, positions[index]))
            {
                return Err(invalid(
                    "Connected flat terrain requires authored, native and compiler positions on one exact planar field",
                ));
            }
        }
    }
    if mode == Mode::Flat {
        let indices = data.infos[0]
            .triangle_indices()
            .map_err(|error| invalid(error.to_string()))?;
        work.consume(indices.len()).map_err(invalid)?;
        for triangle in indices {
            let points = triangle.map(|index| positions.last().unwrap()[index]);
            if orient(points[0], points[1], points[2], visible_axes)? != visible_sign {
                return Err(invalid(
                    "Connected flat terrain currently requires consistently oriented positive native triangles",
                ));
            }
        }
    }
    match mode {
        Mode::Flat => super::certify_normals(&data, &positions, work)?,
        Mode::Cells => super::certify_height_normals(&data, &positions, axis, work)?,
    }
    let sheet = Sheet {
        base: base_plane,
        field,
        sign,
    };
    Ok(Surface {
        info: data.infos[0],
        power: info.power,
        raw,
        corners,
        base_positions: mesh.base_positions,
        positions,
        sheet,
    })
}

fn graph(
    surfaces: &[&Surface<'_>],
    mode: Mode,
    phase: Phase,
    work: &mut Work,
) -> Result<Graph, EditError> {
    let grids = surfaces
        .iter()
        .map(|surface| surface.grid())
        .collect::<Vec<_>>();
    let found =
        neighbors::find_spans(&grids, &mut |amount| work.consume(amount)).map_err(invalid)?;
    if found.ambiguous != 0 || found.mismatched != 0 {
        return Err(invalid(
            "Flat terrain requires unambiguous matching native full/half contacts",
        ));
    }
    let connections =
        neighbors::native_connections(&grids, &found.pairs, &mut |amount| work.consume(amount))
            .map_err(invalid)?;
    let (patches, _) =
        lod::prepare(&grids, &found.pairs, &mut |amount| work.consume(amount)).map_err(invalid)?;
    if patches.iter().any(Option::is_some) {
        return Err(invalid(
            "Connected flat terrain cuts cannot change or retain affected native LOD coarsening",
        ));
    }
    let mut edges = BTreeSet::new();
    let mut corners = BTreeSet::new();
    let mut visible_spans = BTreeMap::new();
    let mut varying = BTreeSet::new();
    let mut result = Graph::default();
    for pair in &found.pairs {
        let a = surfaces[pair.first];
        let b = surfaces[pair.second];
        let common = neighbors::common_span_indices(
            (a.power, pair.first_edge, pair.first_span),
            (b.power, pair.second_edge, pair.second_span),
        )
        .map_err(invalid)?;
        work.consume(common.len() * a.positions.len())
            .map_err(invalid)?;
        for &(i, j) in &common {
            if a.positions
                .iter()
                .zip(&b.positions)
                .any(|(a, b)| !same(a[i], b[j]))
            {
                return Err(invalid(
                    "Flat terrain shared-edge native and compiler samples differ",
                ));
            }
        }
        let key = (pair.first.min(pair.second), pair.first.max(pair.second));
        edges.insert(key);
        if mode == Mode::Cells && phase == Phase::Original {
            work.consume(1).map_err(invalid)?;
            let first = common
                .first()
                .ok_or_else(|| invalid("Native terrain edge span has no shared endpoints"))?
                .0;
            let last = common
                .last()
                .ok_or_else(|| invalid("Native terrain edge span has no shared endpoints"))?
                .0;
            if first == last
                || visible_spans
                    .insert(
                        key,
                        [
                            a.positions.last().unwrap()[first],
                            a.positions.last().unwrap()[last],
                        ],
                    )
                    .is_some()
            {
                return Err(invalid(
                    "Terrain source pair has multiple or ambiguous native edge spans",
                ));
            }
        }
        if a.raw.source != b.raw.source {
            let base = &a.corners;
            let middle = ((base[pair.first_edge] + base[(pair.first_edge + 1) % 4]) * 0.5)
                .as_vec3()
                .as_dvec3();
            let span = match pair.first_span {
                neighbors::Span::Full => [base[pair.first_edge], base[(pair.first_edge + 1) % 4]],
                neighbors::Span::FirstHalf => [base[pair.first_edge], middle],
                neighbors::Span::SecondHalf => [middle, base[(pair.first_edge + 1) % 4]],
            };
            for point in span {
                if !on_segment(
                    point,
                    [base[pair.first_edge], base[(pair.first_edge + 1) % 4]],
                )? {
                    return Err(invalid(
                        "Flat terrain contacts currently require exact native edge midpoints",
                    ));
                }
            }
            result
                .edges
                .push(Contact::new([a.raw.source, b.raw.source], span));
        }
    }
    for (first, lists) in connections.corners.iter().enumerate() {
        for (corner, list) in lists.iter().enumerate() {
            for &second in &list.neighbors[..usize::from(list.count)] {
                let second = usize::from(second);
                if first >= second {
                    continue;
                }
                let a = surfaces[first];
                let b = surfaces[second];
                let point = a.corners[corner];
                let other = b
                    .corners
                    .iter()
                    .position(|&other| same(point, other))
                    .ok_or_else(|| invalid("Flat terrain native corner identity changed"))?;
                let indices = |surface: &Surface<'_>, corner| {
                    let size = (1usize << surface.power) + 1;
                    [0, (size - 1) * size, size * size - 1, size - 1][corner]
                };
                work.consume(a.positions.len()).map_err(invalid)?;
                if a.positions
                    .iter()
                    .zip(&b.positions)
                    .any(|(a_points, b_points)| {
                        !same(a_points[indices(a, corner)], b_points[indices(b, other)])
                    })
                {
                    return Err(invalid("Flat terrain native corner samples differ"));
                }
                corners.insert((first, second));
                if a.raw.source != b.raw.source {
                    result
                        .corners
                        .push(Contact::new([a.raw.source, b.raw.source], [point; 2]));
                }
            }
        }
    }
    for first in 0..surfaces.len() {
        for second in first + 1..surfaces.len() {
            let a = &surfaces[first].corners;
            let b = &surfaces[second].corners;
            let Some(rank) = contact_rank(a, b, work)? else {
                if grid_clip::related(a, b) {
                    return Err(invalid(
                        "Flat terrain near-contact geometry cannot be certified exactly",
                    ));
                }
                continue;
            };
            work.consume(2 * FIELD_WORK).map_err(invalid)?;
            if !surfaces[first].sheet.matches(&surfaces[second].sheet)? {
                if surfaces[first].sheet.field.is_some()
                    && surfaces[second].sheet.field.is_some()
                    && surfaces[first].sheet.same_base(&surfaces[second].sheet)?
                {
                    varying.insert(first);
                    varying.insert(second);
                } else {
                    return Err(invalid(
                        "Connected terrain requires a common native base plane and orientation",
                    ));
                }
            }
            if grid_clip::overlaps(a, b) || rank == 0 {
                return Err(invalid("Flat terrain native base domains overlap"));
            }
            if !(edges.contains(&(first, second)) && rank == 1
                || corners.contains(&(first, second)) && rank == 2)
            {
                return Err(invalid(
                    "Flat terrain contact lacks complete native full/half or corner coverage",
                ));
            }
        }
    }
    certify_contact_normals(surfaces, &edges, &corners, &varying, work)?;
    if mode == Mode::Cells && phase == Phase::Original {
        certify_original_footprints(surfaces, &edges, &corners, &visible_spans, work)?;
    }
    result.pairs = found.pairs;
    Ok(result)
}

fn contact_component(
    seed: usize,
    edges: &BTreeSet<(usize, usize)>,
    corners: &BTreeSet<(usize, usize)>,
    work: &mut Work,
) -> Result<BTreeSet<usize>, EditError> {
    let mut component = BTreeSet::from([seed]);
    let mut pending = vec![seed];
    while let Some(current) = pending.pop() {
        for &(a, b) in edges.iter().chain(corners) {
            work.consume(1).map_err(invalid)?;
            let peer = if a == current {
                b
            } else if b == current {
                a
            } else {
                continue;
            };
            if component.insert(peer) {
                pending.push(peer);
            }
        }
    }
    Ok(component)
}

fn certify_original_footprints(
    surfaces: &[&Surface<'_>],
    edges: &BTreeSet<(usize, usize)>,
    corners: &BTreeSet<(usize, usize)>,
    spans: &BTreeMap<(usize, usize), [DVec3; 2]>,
    work: &mut Work,
) -> Result<(), EditError> {
    let mut verified = BTreeSet::new();
    for &(seed, _) in edges.iter().chain(corners) {
        if verified.contains(&seed) {
            continue;
        }
        let component = contact_component(seed, edges, corners, work)?;
        let axis = surfaces[seed].sheet.base.axis();
        let axes = [(axis + 1) % 3, (axis + 2) % 3];
        let mut rings = BTreeMap::new();
        for &index in &component {
            let surface = surfaces[index];
            let size = surface
                .info
                .grid_size()
                .map_err(|error| invalid(error.to_string()))?;
            rings.insert(
                index,
                super::ordered_boundary(surface.positions.last().unwrap(), size, work)?,
            );
        }
        for &first in &component {
            for &second in component.range(first + 1..) {
                work.consume(2 * FIELD_WORK).map_err(invalid)?;
                if !surfaces[first].sheet.same_base(&surfaces[second].sheet)? {
                    return Err(invalid(
                        "Connected curved terrain footprints require one exact base chart and orientation",
                    ));
                }
                let rank = projected_contact_rank(&rings[&first], &rings[&second], axes, work)?;
                let key = (first, second);
                if edges.contains(&key) {
                    let span = spans.get(&key).ok_or_else(|| {
                        invalid("Terrain footprint lost its verified native edge span")
                    })?;
                    if rank != Some(1) {
                        return Err(invalid(
                            "Curved terrain projected edge contact differs from its native seam",
                        ));
                    }
                    certify_span_overlap(&rings[&first], &rings[&second], *span, axes, work)?;
                } else if corners.contains(&key) {
                    if rank != Some(2) {
                        return Err(invalid(
                            "Curved terrain projected corner contact is ambiguous or overlaps",
                        ));
                    }
                } else if rank.is_some() {
                    return Err(invalid(
                        "Curved terrain projected footprints overlap or introduce an unsewn contact",
                    ));
                }
            }
        }
        verified.extend(component);
    }
    Ok(())
}

fn certify_span_overlap(
    first: &[DVec3],
    second: &[DVec3],
    span: [DVec3; 2],
    axes: [usize; 2],
    work: &mut Work,
) -> Result<(), EditError> {
    let coordinate = axes
        .into_iter()
        .find(|&axis| span[0][axis] != span[1][axis])
        .ok_or_else(|| invalid("Native terrain visible edge span is projected to a point"))?;
    let first = support_interval(first, span, axes, coordinate, work)?;
    let second = support_interval(second, span, axes, coordinate, work)?;
    let actual = [first[0].max(second[0]), first[1].min(second[1])];
    let expected = [
        span[0][coordinate].min(span[1][coordinate]),
        span[0][coordinate].max(span[1][coordinate]),
    ];
    if actual != expected {
        return Err(invalid(
            "Curved terrain projected seam extends beyond or omits its exact native span",
        ));
    }
    Ok(())
}

fn support_interval(
    ring: &[DVec3],
    span: [DVec3; 2],
    axes: [usize; 2],
    coordinate: usize,
    work: &mut Work,
) -> Result<[f64; 2], EditError> {
    work.consume((FIELD_WORK + 2) * ring.len())
        .map_err(invalid)?;
    let mut interval: Option<[f64; 2]> = None;
    for &point in ring {
        if orient(span[0], span[1], point, axes)? == 0 {
            let value = point[coordinate];
            interval = Some(interval.map_or([value; 2], |range| {
                [range[0].min(value), range[1].max(value)]
            }));
        }
    }
    interval.ok_or_else(|| invalid("Terrain projected footprint lost its native support line"))
}

fn certify_contact_normals(
    surfaces: &[&Surface<'_>],
    edges: &BTreeSet<(usize, usize)>,
    corners: &BTreeSet<(usize, usize)>,
    varying: &BTreeSet<usize>,
    work: &mut Work,
) -> Result<(), EditError> {
    let mut verified = BTreeSet::new();
    for &first in varying {
        if verified.contains(&first) {
            continue;
        }
        let component = contact_component(first, edges, corners, work)?;
        let mut projection: Option<super::NormalProjection> = None;
        for &index in &component {
            let surface = surfaces[index];
            let actual = super::certify_projected_normals(
                surface.info,
                surface.corners,
                &surface.positions,
                work,
            )?;
            work.consume(2 * FIELD_WORK).map_err(invalid)?;
            if let Some(expected) = &projection
                && !expected.matches(&actual)?
            {
                return Err(invalid(
                    "Noncoplanar flat terrain contacts require one exact common affine tangential projection",
                ));
            }
            projection = Some(actual);
        }
        verified.extend(component);
    }
    Ok(())
}

fn on_segment(point: DVec3, segment: [DVec3; 2]) -> Result<bool, EditError> {
    let [a, b] = segment;
    for axes in [[0, 1], [0, 2], [1, 2]] {
        if orient(a, b, point, axes)? != 0 {
            return Ok(false);
        }
    }
    Ok((0..3)
        .all(|axis| point[axis] >= a[axis].min(b[axis]) && point[axis] <= a[axis].max(b[axis])))
}

fn contains(surface: &Surface<'_>, point: DVec3) -> Result<bool, EditError> {
    let points = surface.corners;
    let axis = surface.sheet.base.axis();
    if !surface.sheet.base.contains(point)? {
        return Ok(false);
    }
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let sign = surface.sheet.sign;
    for edge in 0..4 {
        let side = orient(points[edge], points[(edge + 1) % 4], point, axes)?;
        if side != 0 && side != sign {
            return Ok(false);
        }
    }
    Ok(true)
}

fn retained(
    source: FaceId,
    point: DVec3,
    after: &[&Surface<'_>],
    work: &mut Work,
) -> Result<bool, EditError> {
    for surface in after.iter().filter(|surface| surface.raw.source == source) {
        work.consume(16 + FIELD_WORK).map_err(invalid)?;
        if contains(surface, point)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn covered(
    sources: [FaceId; 2],
    point: DVec3,
    graph: &Graph,
    work: &mut Work,
) -> Result<bool, EditError> {
    for contact in graph
        .edges
        .iter()
        .chain(&graph.corners)
        .filter(|contact| contact.sources == sources)
    {
        work.consume(4).map_err(invalid)?;
        if on_segment(point, contact.points)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn coverage(
    before: &Graph,
    after: &Graph,
    surfaces: &[&Surface<'_>],
    work: &mut Work,
) -> Result<(), EditError> {
    for contact in &after.edges {
        let mut original = false;
        for previous in before
            .edges
            .iter()
            .filter(|previous| previous.sources == contact.sources)
        {
            work.consume(8).map_err(invalid)?;
            if on_segment(contact.points[0], previous.points)?
                && on_segment(contact.points[1], previous.points)?
            {
                original = true;
                break;
            }
        }
        if !original {
            return Err(invalid(
                "Flat terrain cut introduced a native contact outside its original source join",
            ));
        }
    }
    for contact in &after.corners {
        if !covered(contact.sources, contact.points[0], before, work)? {
            return Err(invalid(
                "Flat terrain cut introduced an unproved native corner contact",
            ));
        }
    }
    for contact in &before.edges {
        let mut points = contact.points.to_vec();
        for surface in surfaces
            .iter()
            .filter(|surface| contact.sources.contains(&surface.raw.source))
        {
            for &point in &surface.corners {
                work.consume(4).map_err(invalid)?;
                if on_segment(point, contact.points)? {
                    points.push(point);
                }
            }
        }
        let delta = contact.points[1] - contact.points[0];
        let axis = (0..3)
            .max_by(|&a, &b| delta[a].abs().total_cmp(&delta[b].abs()))
            .unwrap();
        points.sort_by(|a, b| a[axis].total_cmp(&b[axis]));
        points.dedup_by(|a, b| same(*a, *b));
        for &point in &points {
            if retained(contact.sources[0], point, surfaces, work)?
                && retained(contact.sources[1], point, surfaces, work)?
                && !covered(contact.sources, point, after, work)?
            {
                return Err(invalid(
                    "Flat terrain cut disconnected a retained native contact endpoint",
                ));
            }
        }
        for pair in points.windows(2) {
            let point = weighted(pair, &[1, 1], 2)?;
            let expected = retained(contact.sources[0], point, surfaces, work)?
                && retained(contact.sources[1], point, surfaces, work)?;
            let mut actual = 0;
            for edge in after
                .edges
                .iter()
                .filter(|edge| edge.sources == contact.sources)
            {
                work.consume(4).map_err(invalid)?;
                actual += usize::from(on_segment(point, edge.points)?);
            }
            if actual != usize::from(expected) {
                return Err(invalid(
                    "Flat terrain cut cannot preserve complete retained native edge coverage",
                ));
            }
        }
    }
    for contact in &before.corners {
        let point = contact.points[0];
        if retained(contact.sources[0], point, surfaces, work)?
            && retained(contact.sources[1], point, surfaces, work)?
            && !covered(contact.sources, point, after, work)?
        {
            return Err(invalid(
                "Flat terrain cut disconnected a retained native corner",
            ));
        }
    }
    Ok(())
}

fn verify_source(
    document: &Document,
    vmf: &Vmf,
    assembled: &Surface<'_>,
    mode: Mode,
    work: &mut Work,
) -> Result<(), EditError> {
    let source = assembled.raw.source;
    let (owner, solid) = crate::subobject::solid(document, source.solid)?;
    let owned = match owner {
        ObjectId::Solid(_) => vmf
            .world
            .solids
            .iter()
            .any(|solid| solid.id == source.solid),
        ObjectId::Entity(id) => vmf.entities.iter().any(|entity| {
            entity.id == id
                && entity.classname() == document.entity(id).and_then(|entity| entity.classname())
                && entity.solids.iter().any(|solid| solid.id == source.solid)
        }),
    };
    let index = solid
        .sides
        .iter()
        .position(|side| side.id == source.side)
        .ok_or_else(|| invalid("Flat terrain context lost an original source face"))?;
    let side = &solid.sides[index];
    let mut canonical = solid.clone();
    crate::brush::canonicalize_precise(&mut canonical)?;
    if !owned
        || assembled.raw.side.plane != side.plane
            && assembled.raw.side.plane != canonical.sides[index].plane
    {
        return Err(invalid(
            "Flat terrain context changed original source ownership or base planes",
        ));
    }
    let mut normalized = side.clone();
    normalized.plane = assembled.raw.side.plane;
    if normalized != *assembled.raw.side {
        return Err(invalid(
            "Flat terrain context changed original source fields or texture projections",
        ));
    }
    let shape = BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
    let face = shape
        .faces
        .into_iter()
        .find(|face| face.side == index)
        .ok_or_else(|| invalid("Flat terrain source has no effective native face"))?;
    let cage = face
        .vertices
        .iter()
        .copied()
        .map(native::stored_vertex)
        .collect();
    let original = prepare(
        Raw {
            source,
            side,
            face,
            supported: true,
            cage,
        },
        mode,
        Phase::Original,
        work,
    )?;
    work.consume(original.positions.iter().map(Vec::len).sum())
        .map_err(invalid)?;
    work.consume(2 * FIELD_WORK + original.base_positions.len())
        .map_err(invalid)?;
    if original.power != assembled.power
        || original.corners != assembled.corners
        || original.positions != assembled.positions
        || original.base_positions != assembled.base_positions
        || !original.sheet.matches(&assembled.sheet)?
    {
        return Err(invalid(
            "Flat terrain context changed original native or compiler geometry",
        ));
    }
    Ok(())
}

fn crosses(surface: &Surface<'_>, plane: Plane, work: &mut Work) -> Result<bool, EditError> {
    work.consume(surface.positions.iter().map(Vec::len).sum())
        .map_err(invalid)?;
    Ok(surface.positions.iter().any(|positions| {
        positions
            .iter()
            .any(|point| plane.distance_to(*point) > CUT_EPSILON)
            && positions
                .iter()
                .any(|point| plane.distance_to(*point) < -CUT_EPSILON)
    }))
}

fn retained_span(
    surface: &Surface<'_>,
    edge: usize,
    span: neighbors::Span,
    plane: Plane,
    keep: Keep,
    work: &mut Work,
) -> Result<bool, EditError> {
    if keep == Keep::Both {
        work.consume(1).map_err(invalid)?;
        return Ok(true);
    }
    let indices = span.indices(surface.power, edge).map_err(invalid)?;
    work.consume(surface.positions.len() * indices.len())
        .map_err(invalid)?;
    let sign = if keep == Keep::Front { 1.0 } else { -1.0 };
    let mut triangles = None;
    for positions in &surface.positions {
        if indices
            .iter()
            .any(|&index| sign * plane.distance_to(positions[index]) < -CUT_EPSILON)
        {
            return Ok(false);
        }
        for pair in indices.windows(2) {
            if pair
                .iter()
                .any(|&index| plane.distance_to(positions[index]).abs() > CUT_EPSILON)
            {
                continue;
            }
            if triangles.is_none() {
                triangles = Some(
                    surface
                        .info
                        .triangle_indices()
                        .map_err(|error| invalid(error.to_string()))?,
                );
            }
            let triangles = triangles.as_ref().unwrap();
            work.consume(triangles.len()).map_err(invalid)?;
            let triangle = triangles
                .iter()
                .find(|triangle| pair.iter().all(|index| triangle.contains(index)))
                .ok_or_else(|| {
                    invalid("Terrain edge closure lost its original boundary triangle")
                })?;
            let third = triangle.iter().find(|index| !pair.contains(index)).unwrap();
            let distance = sign * plane.distance_to(positions[*third]);
            if distance < -CUT_EPSILON {
                return Ok(false);
            }
            if distance <= CUT_EPSILON {
                return Err(invalid(
                    "Terrain retained edge closure is ambiguous within cut-plane precision",
                ));
            }
        }
    }
    Ok(true)
}

pub(crate) fn required_partitions(
    document: &Document,
    authorized: &[crate::clip::Source<'_>],
    crossing: &[FaceId],
    plane: Plane,
    keep: Keep,
    context: &mut Context<'_>,
) -> Result<Vec<FaceId>, EditError> {
    if crossing.len() > crate::clip::MAX_TERRAIN_SOURCES
        || crossing.iter().collect::<BTreeSet<_>>().len() != crossing.len()
    {
        return Err(invalid(
            "Terrain edge closure requires bounded unique crossing source faces",
        ));
    }
    let selected = crossing
        .iter()
        .filter(|face| native::terrain_source_is_visible(document.vmf(), face.solid))
        .copied()
        .collect::<BTreeSet<_>>();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let vmf = context.vmf.unwrap_or(document.vmf());
    vmf.validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    if context.vmf.is_some() {
        document
            .vmf()
            .validate_ids()
            .map_err(|error| invalid(error.to_string()))?;
    }
    let work = &mut context.work;
    let raw = collect(vmf, work)?;
    let before = affected_sources(raw, &selected, work)?
        .into_iter()
        .map(|raw| prepare(raw, Mode::Cells, Phase::Original, work))
        .collect::<Result<Vec<_>, _>>()?;
    let references = before.iter().collect::<Vec<_>>();
    let original = graph(&references, Mode::Cells, Phase::Original, work)?;
    let mut queue = Vec::new();
    for &face in &selected {
        let index = before
            .iter()
            .position(|surface| surface.raw.source == face)
            .ok_or_else(|| invalid("Terrain edge closure lost its selected crossing source"))?;
        authorize(document, authorized, face)?;
        if context.vmf.is_some() {
            verify_source(document, vmf, &before[index], Mode::Cells, work)?;
        }
        if !crosses(&before[index], plane, work)? {
            return Err(crate::clip::source_error(
                face.solid,
                invalid("Terrain edge closure requires a genuine crossing source"),
            ));
        }
        queue.push(index);
    }
    let mut required = BTreeSet::new();
    let mut cursor = 0;
    while cursor < queue.len() {
        let current = queue[cursor];
        cursor += 1;
        for pair in &original.pairs {
            work.consume(1).map_err(invalid)?;
            let (peer, edge, span) = if pair.first == current {
                (pair.second, pair.first_edge, pair.first_span)
            } else if pair.second == current {
                (pair.first, pair.second_edge, pair.second_span)
            } else {
                continue;
            };
            let face = before[peer].raw.source;
            if selected.contains(&face) || required.contains(&face) {
                continue;
            }
            if selected.contains(&before[current].raw.source)
                && !retained_span(&before[current], edge, span, plane, keep, work)?
            {
                continue;
            }
            authorize(document, authorized, face)?;
            if context.vmf.is_some() {
                verify_source(document, vmf, &before[peer], Mode::Cells, work)?;
            }
            if crosses(&before[peer], plane, work)? {
                return Err(crate::clip::source_error(
                    face.solid,
                    invalid(
                        "Terrain edge closure cannot partition a crossing neighbor; include it as a crossing replacement",
                    ),
                ));
            }
            if crossing.len() + required.len() + 1 > crate::clip::MAX_TERRAIN_SOURCES {
                return Err(invalid(
                    "Terrain edge closure exceeds the terrain source limit",
                ));
            }
            required.insert(face);
            queue.push(peer);
        }
    }
    Ok(required.into_iter().collect())
}

fn authorize(
    document: &Document,
    sources: &[crate::clip::Source<'_>],
    face: FaceId,
) -> Result<(), EditError> {
    let matching = sources
        .iter()
        .filter(|source| source.solid.id == face.solid)
        .collect::<Vec<_>>();
    let source = match matching.as_slice() {
        [source] => source,
        [] => {
            return Err(crate::clip::source_error(
                face.solid,
                invalid(format!(
                    "Select original terrain side {} to authorize retained-edge neighbor partitioning",
                    face.side,
                )),
            ));
        }
        _ => {
            return Err(crate::clip::source_error(
                face.solid,
                invalid("Terrain edge closure has an ambiguous selected source identity"),
            ));
        }
    };
    if !native::terrain_source_is_visible(document.vmf(), face.solid)
        || source
            .solid
            .sides
            .iter()
            .filter(|side| side.displacement.is_some())
            .count()
            != 1
        || !source
            .solid
            .sides
            .iter()
            .any(|side| side.id == face.side && side.displacement.is_some())
    {
        return Err(crate::clip::source_error(
            face.solid,
            invalid(
                "Terrain edge closure requires one selected original parent terrain face with an unambiguous identity",
            ),
        ));
    }
    Ok(())
}

pub(crate) fn certify(
    document: &Document,
    replacements: &[Replacement<'_>],
    context: &mut Context<'_>,
) -> Result<Vec<Verification>, EditError> {
    certify_mode(document, replacements, context, Mode::Flat)
}

pub(crate) fn certify_cells(
    document: &Document,
    replacements: &[Replacement<'_>],
    context: &mut Context<'_>,
) -> Result<Vec<Verification>, EditError> {
    certify_mode(document, replacements, context, Mode::Cells)
}

fn certify_mode(
    document: &Document,
    replacements: &[Replacement<'_>],
    context: &mut Context<'_>,
    mode: Mode,
) -> Result<Vec<Verification>, EditError> {
    let empty = || {
        replacements
            .iter()
            .map(|_| Verification {
                neighbors: 0,
                links: 0,
            })
            .collect()
    };
    let selected = replacements
        .iter()
        .filter(|replacement| {
            native::terrain_source_is_visible(document.vmf(), replacement.source.solid)
        })
        .map(|replacement| replacement.source)
        .collect::<BTreeSet<_>>();
    for replacement in replacements {
        let (_, source) = crate::subobject::solid(document, replacement.source.solid)?;
        if replacement
            .parts
            .iter()
            .any(|part| part.hidden != source.hidden)
        {
            return Err(invalid(
                "Flat terrain replacements must preserve saved-hidden visibility",
            ));
        }
    }
    if selected.is_empty() {
        return Ok(empty());
    }
    if replacements.len() > crate::clip::MAX_TERRAIN_SOURCES
        || selected.len()
            != replacements
                .iter()
                .filter(|replacement| {
                    native::terrain_source_is_visible(document.vmf(), replacement.source.solid)
                })
                .count()
    {
        return Err(invalid(
            "Flat terrain replacement sources must be bounded and unique",
        ));
    }
    let vmf = context.vmf.unwrap_or(document.vmf());
    let work = &mut context.work;
    let raw = collect(vmf, work)?;
    let retained_count = raw
        .len()
        .checked_sub(selected.len())
        .ok_or_else(|| invalid("Flat terrain context lost selected source identities"))?;
    let output_count = replacements
        .iter()
        .filter(|replacement| selected.contains(&replacement.source))
        .try_fold(retained_count, |count, replacement| {
            count.checked_add(replacement.parts.len())
        })
        .ok_or_else(|| invalid("Flat terrain output exceeds the native displacement limit"))?;
    if output_count > bsp::MAX_DISPLACEMENTS {
        return Err(invalid(
            "Flat terrain output exceeds the native displacement limit",
        ));
    }
    let before = affected_sources(raw, &selected, work)?
        .into_iter()
        .map(|raw| prepare(raw, mode, Phase::Original, work))
        .collect::<Result<Vec<_>, _>>()?;
    if context.vmf.is_some() {
        for surface in before
            .iter()
            .filter(|surface| selected.contains(&surface.raw.source))
        {
            verify_source(document, vmf, surface, mode, work)?;
        }
    }
    let before_refs = before.iter().collect::<Vec<_>>();
    let previous = graph(&before_refs, mode, Phase::Original, work)?;
    let mut children = Vec::new();
    for replacement in replacements
        .iter()
        .filter(|replacement| selected.contains(&replacement.source))
    {
        if replacement.parts.is_empty() {
            return Err(invalid(
                "Flat terrain replacement lost every retained patch",
            ));
        }
        for part in replacement.parts {
            let shape =
                BrushGeometry::from_solid(part).map_err(|error| invalid(error.to_string()))?;
            let faces = shape
                .faces
                .into_iter()
                .filter(|face| part.sides[face.side].displacement.is_some())
                .collect::<Vec<_>>();
            let [face] = faces.as_slice() else {
                return Err(invalid("Flat terrain child requires one native face"));
            };
            let side = &part.sides[face.side];
            if side.id != replacement.source.side {
                return Err(invalid("Flat terrain child lost its original source side"));
            }
            let cage = face
                .vertices
                .iter()
                .copied()
                .map(native::stored_vertex)
                .collect();
            children.push(prepare(
                Raw {
                    source: replacement.source,
                    side,
                    face: face.clone(),
                    supported: true,
                    cage,
                },
                mode,
                Phase::Child,
                work,
            )?);
        }
    }
    let mut after = Vec::new();
    for surface in &before {
        if selected.contains(&surface.raw.source) {
            after.extend(
                children
                    .iter()
                    .filter(|child| child.raw.source == surface.raw.source),
            );
        } else {
            after.push(surface);
        }
    }
    if after.len() > bsp::MAX_DISPLACEMENTS {
        return Err(invalid(
            "Flat terrain output exceeds the native displacement limit",
        ));
    }
    let current = graph(&after, mode, Phase::Child, work)?;
    coverage(&previous, &current, &after, work)?;
    let mut adjacency = BTreeMap::<FaceId, BTreeSet<FaceId>>::new();
    for contact in previous.edges.iter().chain(&previous.corners) {
        adjacency
            .entry(contact.sources[0])
            .or_default()
            .insert(contact.sources[1]);
        adjacency
            .entry(contact.sources[1])
            .or_default()
            .insert(contact.sources[0]);
    }
    Ok(replacements
        .iter()
        .map(|replacement| {
            if !selected.contains(&replacement.source) {
                return Verification {
                    neighbors: 0,
                    links: 0,
                };
            }
            let mut component = BTreeSet::from([replacement.source]);
            let mut queue = vec![replacement.source];
            while let Some(source) = queue.pop() {
                if let Some(peers) = adjacency.get(&source) {
                    for peer in peers {
                        if component.insert(*peer) {
                            queue.push(*peer);
                        }
                    }
                }
            }
            let neighbors = component
                .iter()
                .filter(|source| !selected.contains(source))
                .count();
            let links = current
                .edges
                .iter()
                .chain(&current.corners)
                .filter(|contact| contact.sources.contains(&replacement.source))
                .map(|contact| {
                    if contact.sources[0] == replacement.source {
                        contact.sources[1]
                    } else {
                        contact.sources[0]
                    }
                })
                .collect::<BTreeSet<_>>()
                .len();
            Verification { neighbors, links }
        })
        .collect())
}
