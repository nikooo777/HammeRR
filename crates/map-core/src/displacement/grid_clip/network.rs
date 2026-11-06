use std::collections::{BTreeMap, BTreeSet};

use glam::{DVec2, DVec3};
use source_formats::{
    bsp,
    vmf::{Side, Solid, Vmf},
};

use super::{
    Child, Context, CutLine, NativePositions, Operation, POSITION_PROFILES, Plan, SAMPLE_EPSILON,
    certified_encoded_positions, certified_native_positions, contains, invalid, touches,
};
use crate::displacement::{
    DisplacementMesh, lod, native,
    neighbors::{self, SurfaceGrid},
    triangles,
};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Face, Plane};

const MAX_WORK: usize = 32_000_000;
pub const MAX_NEIGHBORS: usize = 128;
const SPAN_STEPS: i32 = 16;

struct Leaf {
    origin: Child,
    corners: [DVec3; 4],
}

#[derive(Clone, Copy)]
struct Interval {
    start: i32,
    end: i32,
    points: [DVec3; 2],
    origin: Child,
}

struct RequiredPartition {
    source: FaceId,
    child: Child,
}

impl Interval {
    fn native_span(self, start: i32, end: i32) -> Option<neighbors::Span> {
        let middle = (self.start + self.end) / 2;
        match (start, end) {
            (a, b) if a == self.start && b == self.end => Some(neighbors::Span::Full),
            (a, b) if a == self.start && b == middle => Some(neighbors::Span::FirstHalf),
            (a, b) if a == middle && b == self.end => Some(neighbors::Span::SecondHalf),
            _ => None,
        }
    }
}

fn intervals(
    leaves: &[Leaf],
    power: u32,
    edge: usize,
    span: neighbors::Span,
    reverse: bool,
) -> Vec<Interval> {
    let last = 1 << power;
    let (start, end) = match span {
        neighbors::Span::Full => (0, last),
        neighbors::Span::FirstHalf => (0, last / 2),
        neighbors::Span::SecondHalf => (last / 2, last),
    };
    leaves
        .iter()
        .filter_map(|leaf| {
            let Child { row, column, .. } = leaf.origin;
            let cells = leaf.origin.cells();
            let (a, b) = match edge {
                0 if column == 0 => (row, row + cells),
                1 if row + cells == last => (column, column + cells),
                2 if column + cells == last => (last - row - cells, last - row),
                3 if row == 0 => (last - column - cells, last - column),
                _ => return None,
            };
            if a.max(start) >= b.min(end) {
                return None;
            }
            let mut points = [leaf.corners[edge], leaf.corners[(edge + 1) % 4]];
            let (a, b) = if reverse {
                points.swap(0, 1);
                (end as i32 - b as i32, end as i32 - a as i32)
            } else {
                (a as i32 - start as i32, b as i32 - start as i32)
            };
            let scale = SPAN_STEPS / (end - start) as i32;
            Some(Interval {
                start: a * scale,
                end: b * scale,
                points,
                origin: leaf.origin,
            })
        })
        .collect()
}

fn half_edges([a, b]: [DVec3; 2]) -> [[DVec3; 2]; 2] {
    let middle = ((a + b) * 0.5).as_vec3().as_dvec3();
    [[a, middle], [middle, b]]
}

fn dyadic_subedge(small: Interval, mut large: Interval) -> bool {
    while large.end - large.start > small.end - small.start {
        let middle = (large.start + large.end) / 2;
        let halves = half_edges(large.points);
        if small.end <= middle {
            large.end = middle;
            large.points = halves[0];
        } else if small.start >= middle {
            large.start = middle;
            large.points = halves[1];
        } else {
            return false;
        }
    }
    large.start == small.start && large.end == small.end && large.points == small.points
}

fn dyadic_ratio(large: i32, small: i32) -> bool {
    large % small == 0 && large / small >= 4 && ((large / small) as u32).is_power_of_two()
}

fn child_leaves(
    parts: &[Solid],
    children: &[Child],
    work: &mut Work,
) -> Result<Vec<Leaf>, EditError> {
    if parts.len() != children.len() {
        return Err(invalid("Terrain clipping lost its child grid origins"));
    }
    let mut result = Vec::with_capacity(parts.len());
    for (part, &origin) in parts.iter().zip(children) {
        work.consume(part.sides.len() + (origin.cells() + 1).pow(2))
            .map_err(invalid)?;
        let geometry =
            BrushGeometry::from_solid(part).map_err(|error| invalid(error.to_string()))?;
        let (side, face) = super::source_face(part, &geometry)?;
        let (_, mesh) =
            native::checked_mesh(face, side.displacement.as_ref().unwrap()).map_err(invalid)?;
        let size = mesh.size;
        result.push(Leaf {
            origin,
            corners: [0, (size - 1) * size, size * size - 1, size - 1]
                .map(|index| native::stored_vertex(mesh.base_positions[index])),
        });
    }
    Ok(result)
}

fn required_partition(
    surfaces: &[Surface<'_>],
    pairs: &[neighbors::NeighborPair],
    leaves: &BTreeMap<FaceId, Vec<Leaf>>,
    work: &mut Work,
) -> Result<Option<RequiredPartition>, EditError> {
    for pair in pairs {
        let first = &surfaces[pair.first];
        let second = &surfaces[pair.second];
        let a = intervals(
            &leaves[&first.raw.id],
            first.grid().power,
            pair.first_edge,
            pair.first_span,
            false,
        );
        let b = intervals(
            &leaves[&second.raw.id],
            second.grid().power,
            pair.second_edge,
            pair.second_span,
            true,
        );
        work.consume(16 * (a.len() * b.len() + a.len() + b.len()))
            .map_err(invalid)?;
        for &a in &a {
            for &b in &b {
                let start = a.start.max(b.start).max(0);
                let end = a.end.min(b.end).min(SPAN_STEPS);
                if start >= end {
                    continue;
                }
                let spans = (a.native_span(start, end), b.native_span(start, end));
                if matches!(
                    spans,
                    (Some(neighbors::Span::Full), Some(_)) | (Some(_), Some(neighbors::Span::Full))
                ) {
                    continue;
                }
                let (small, large, source) = match spans {
                    (Some(neighbors::Span::Full), None)
                        if dyadic_ratio(b.end - b.start, end - start) =>
                    {
                        (a, b, second.raw.id)
                    }
                    (None, Some(neighbors::Span::Full))
                        if dyadic_ratio(a.end - a.start, end - start) =>
                    {
                        (b, a, first.raw.id)
                    }
                    _ => {
                        return Err(invalid(
                            "Terrain neighbor partitioning requires retained full edges on an exact dyadic subdivision",
                        ));
                    }
                };
                if !dyadic_subedge(small, large) {
                    return Err(invalid(
                        "Terrain neighbor partitioning requires exact native dyadic edge coordinates",
                    ));
                }
                return Ok(Some(RequiredPartition {
                    source,
                    child: large.origin,
                }));
            }
        }
    }
    Ok(None)
}

pub(crate) fn partition_neighbors(
    document: &Document,
    authorized: &[crate::clip::Source<'_>],
    replacements: &[Replacement<'_>],
    plane: Plane,
    material: Option<&str>,
    context: &mut Context<'_>,
) -> Result<Vec<Plan>, EditError> {
    if !replacements
        .iter()
        .any(|replacement| replacement.operation == Operation::Cut)
    {
        return Ok(Vec::new());
    }
    let before = component(document, replacements, context.vmf, &mut context.work)?;
    if before.is_empty() {
        return Ok(Vec::new());
    }
    let original = graph(&before, &mut context.work)?;
    let mut leaves = before
        .iter()
        .map(|surface| {
            (
                surface.raw.id,
                vec![Leaf {
                    origin: surface.origin,
                    corners: surface.grid().corners,
                }],
            )
        })
        .collect::<BTreeMap<_, _>>();
    for replacement in replacements {
        if let Some(leaves) = leaves.get_mut(&replacement.source) {
            *leaves = child_leaves(replacement.parts, replacement.children, &mut context.work)?;
        }
    }
    let mut plans = BTreeMap::<FaceId, Plan>::new();
    let mut prepared = BTreeMap::new();
    let mut patches = replacements
        .iter()
        .map(|replacement| replacement.parts.len())
        .sum::<usize>();
    let mut vertices = replacements
        .iter()
        .flat_map(|replacement| replacement.children)
        .map(|child| (child.cells() + 1).pow(2))
        .sum::<usize>();
    let mut triangles = replacements
        .iter()
        .flat_map(|replacement| replacement.children)
        .map(|child| 2 * child.cells().pow(2))
        .sum::<usize>();
    while let Some(RequiredPartition {
        source: face,
        child,
    }) = required_partition(&before, &original.pairs, &leaves, &mut context.work)?
    {
        if replacements
            .iter()
            .any(|replacement| replacement.source == face)
        {
            return Err(crate::clip::source_error(
                face.solid,
                invalid("Terrain neighbor partitioning requires a noncrossing original source"),
            ));
        }
        let source = authorized
            .iter()
            .find(|source| source.solid.id == face.solid)
            .ok_or_else(|| {
                crate::clip::source_error(
                    face.solid,
                    invalid("Select this terrain neighbor to authorize native grid partitioning"),
                )
            })?;
        if child.power <= 2 {
            return Err(crate::clip::source_error(
                face.solid,
                invalid(
                    "Terrain neighbor leaf is already power 2; its native join cannot be partitioned without resampling",
                ),
            ));
        }
        let existing = plans.contains_key(&face);
        let removed = usize::from(existing);
        let cells = child.cells() / 2;
        patches = patches - removed + 4;
        vertices = vertices - removed * (child.cells() + 1).pow(2) + 4 * (cells + 1).pow(2);
        triangles = triangles - removed * 2 * child.cells().pow(2) + 8 * cells.pow(2);
        if replacements.len() + plans.len() + usize::from(!existing)
            > crate::clip::MAX_TERRAIN_SOURCES
            || patches > crate::clip::MAX_TERRAIN_PATCHES
            || vertices > crate::clip::MAX_TERRAIN_VERTICES
            || triangles > crate::clip::MAX_TERRAIN_TRIANGLES
        {
            return Err(invalid(
                "Terrain clipping exceeds its source, patch, vertex or triangle limit",
            ));
        }
        let material = material
            .map(str::trim)
            .unwrap_or(&source.solid.sides[0].material);
        let original = match prepared.entry(face) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let geometry = BrushGeometry::from_solid(source.solid)
                    .map_err(|error| invalid(error.to_string()))?;
                let original =
                    super::prepare_partition(document, source.solid, &geometry, plane, context)
                        .map_err(|error| crate::clip::source_error(face.solid, error))?;
                entry.insert(original)
            }
        };
        let plan = original
            .materialize(
                super::quadrants(child).to_vec(),
                material,
                Operation::Partition,
                &mut context.work,
            )
            .map_err(|error| crate::clip::source_error(face.solid, error))?;
        if plan.source != face || plan.operation != Operation::Partition {
            return Err(invalid(
                "Terrain neighbor partitioning lost its authorized source face",
            ));
        }
        let refined = child_leaves(&plan.parts, &plan.children, &mut context.work)?;
        let current = leaves.get_mut(&face).unwrap();
        let index = current
            .iter()
            .position(|leaf| leaf.origin == child)
            .ok_or_else(|| invalid("Terrain neighbor partitioning lost its current coarse leaf"))?;
        current.splice(index..=index, refined);
        match plans.entry(face) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().replace(child, plan)?
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(plan);
            }
        }
    }
    Ok(plans.into_values().collect())
}

pub(crate) struct Replacement<'a> {
    pub operation: Operation,
    pub source: FaceId,
    pub source_power: u32,
    pub parts: &'a [Solid],
    pub children: &'a [Child],
}

pub(crate) struct Verification {
    pub neighbors: usize,
    pub links: usize,
}

#[derive(Default)]
pub(crate) struct Work(usize);

impl Work {
    pub(crate) fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.0 = self
            .0
            .checked_add(amount)
            .filter(|work| *work <= MAX_WORK)
            .ok_or("Terrain clipping certificate exceeds its work limit")?;
        Ok(())
    }
}

struct Raw<'a> {
    id: FaceId,
    side: &'a Side,
    face: Face,
    supported_owner: bool,
}

struct Surface<'a> {
    raw: Raw<'a>,
    origin: Child,
    original_size: usize,
    mesh: DisplacementMesh,
    positions: Vec<DVec3>,
    uv: Vec<DVec2>,
    vertices: Vec<bsp::DisplacementVertex>,
    indices: Vec<[usize; 3]>,
    tags: Vec<u16>,
}

impl Surface<'_> {
    fn vertex(&self, index: usize) -> Vertex {
        Vertex {
            face: self.raw.id,
            index: (self.origin.row + index / self.mesh.size) * self.original_size
                + self.origin.column
                + index % self.mesh.size,
        }
    }

    fn grid(&self) -> SurfaceGrid<'_> {
        let size = self.mesh.size;
        SurfaceGrid {
            power: self.raw.side.displacement.as_ref().unwrap().power,
            corners: [0, (size - 1) * size, size * size - 1, size - 1]
                .map(|index| native::stored_vertex(self.mesh.base_positions[index])),
            positions: &self.positions,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Vertex {
    face: FaceId,
    index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Segment {
    first: [Vertex; 2],
    second: [Vertex; 2],
}

#[derive(Default)]
struct Links {
    segments: BTreeSet<Segment>,
    vertices: BTreeSet<[Vertex; 2]>,
}

struct Graph {
    pairs: Vec<neighbors::NeighborPair>,
    links: Links,
    lod: Vec<Option<LodState>>,
}

#[derive(PartialEq, Eq)]
struct LodState {
    mask: [u32; crate::displacement::ALLOWED_VERTEX_MASK_WORDS],
    active: Vec<bool>,
    ideal_positions: Vec<[u64; 3]>,
    encoded_vertices: Vec<[u32; 5]>,
    emitted_positions: Vec<[u32; 3]>,
    indices: Vec<[usize; 3]>,
}

struct NativeState {
    allowed: Vec<bool>,
    active: Vec<bool>,
    alphas: Vec<u32>,
    positions: NativePositions,
    indices: Vec<[usize; 3]>,
}

impl NativeState {
    fn prepare(
        surface: &Surface<'_>,
        lod: Option<&LodState>,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        let count = surface.vertices.len();
        let indices = lod.map_or(surface.indices.as_slice(), |lod| lod.indices.as_slice());
        work.consume(count * 4 + indices.len()).map_err(invalid)?;
        let allowed = (0..count)
            .map(|index| lod.is_none_or(|lod| lod.mask[index / 32] & (1 << (index % 32)) != 0))
            .collect::<Vec<_>>();
        let active = lod.map_or_else(|| vec![true; count], |lod| lod.active.clone());
        let size = surface.mesh.size;
        if [
            0,
            size - 1,
            size * (size - 1),
            size * size - 1,
            size * (size / 2) + size / 2,
        ]
        .into_iter()
        .any(|index| !allowed[index] || !active[index])
        {
            return Err(invalid(
                "Terrain clipping requires active native roots and corners in every output grid",
            ));
        }
        let vertices = lod.map_or_else(
            || surface.vertices.clone(),
            |lod| {
                lod.encoded_vertices
                    .iter()
                    .map(|vertex| bsp::DisplacementVertex {
                        direction: [vertex[0], vertex[1], vertex[2]].map(f32::from_bits),
                        distance: f32::from_bits(vertex[3]),
                        alpha: f32::from_bits(vertex[4]),
                    })
                    .collect()
            },
        );
        let alphas = vertices
            .iter()
            .map(|vertex| vertex.alpha.to_bits())
            .collect();
        let positions = certified_encoded_positions(
            &surface.raw.face,
            &surface.mesh,
            surface.raw.side.displacement.as_ref().unwrap().power,
            vertices,
            work,
        )?;
        Ok(Self {
            allowed,
            active,
            alphas,
            positions,
            indices: indices.to_vec(),
        })
    }
}

fn lod_state(
    surface: &Surface<'_>,
    patch: lod::Patch,
    work: &mut Work,
) -> Result<LodState, EditError> {
    work.consume(surface.vertices.len() * 4 + patch.indices.len())
        .map_err(invalid)?;
    let vertices =
        lod::encoded_vertices(&surface.vertices, &surface.positions, &patch).map_err(invalid)?;
    let encoded_vertices = vertices
        .iter()
        .map(|vertex| {
            [
                vertex.direction[0].to_bits(),
                vertex.direction[1].to_bits(),
                vertex.direction[2].to_bits(),
                vertex.distance.to_bits(),
                vertex.alpha.to_bits(),
            ]
        })
        .collect();
    let base: [DVec3; 4] = surface
        .raw
        .face
        .vertices
        .as_slice()
        .try_into()
        .map_err(|_| invalid("Terrain LOD certificate needs a native quad base"))?;
    let info = bsp::DisplacementInfo {
        start: native::stored_vertex(surface.mesh.base_positions[0])
            .as_vec3()
            .to_array(),
        power: surface.raw.side.displacement.as_ref().unwrap().power as i32,
        ..Default::default()
    };
    let emitted_positions = native::positions(base, info, vertices)
        .map_err(invalid)?
        .into_iter()
        .map(|point| point.map(f32::to_bits))
        .collect();
    Ok(LodState {
        mask: patch.mask,
        active: patch.active,
        ideal_positions: patch
            .positions
            .into_iter()
            .map(|point| point.to_array().map(f64::to_bits))
            .collect(),
        encoded_vertices,
        emitted_positions,
        indices: patch.indices,
    })
}

fn collect(vmf: &Vmf) -> Result<Vec<Raw<'_>>, EditError> {
    let mut raw = Vec::new();
    for (solid, supported_owner) in native::visible_terrain_sources(vmf) {
        let geometry =
            BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
        for face in geometry.faces {
            let side = &solid.sides[face.side];
            if side.displacement.is_none() {
                continue;
            }
            if raw.len() == bsp::MAX_DISPLACEMENTS {
                return Err(invalid(
                    "Terrain clipping exceeds the native displacement patch limit",
                ));
            }
            raw.push(Raw {
                id: FaceId {
                    solid: solid.id,
                    side: side.id,
                },
                side,
                face,
                supported_owner,
            });
        }
    }
    Ok(raw)
}

pub(in crate::displacement) fn overlaps(first: &[DVec3], second: &[DVec3]) -> bool {
    let Some(plane) = Plane::from_points([first[0], first[1], first[2]]) else {
        return true;
    };
    if second
        .iter()
        .any(|point| plane.distance_to(*point).abs() > neighbors::EDGE_TOLERANCE)
    {
        return false;
    }
    for polygon in [first, second] {
        for index in 0..polygon.len() {
            let axis = (polygon[(index + 1) % polygon.len()] - polygon[index])
                .cross(plane.normal)
                .normalize_or_zero();
            if axis == DVec3::ZERO {
                return true;
            }
            let range = |vertices: &[DVec3]| {
                vertices
                    .iter()
                    .map(|point| point.dot(axis))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                        (low.min(value), high.max(value))
                    })
            };
            let (a, b) = range(first);
            let (c, d) = range(second);
            if b.min(d) - a.max(c) <= neighbors::EDGE_TOLERANCE {
                return false;
            }
        }
    }
    true
}

pub(in crate::displacement) fn related(first: &[DVec3], second: &[DVec3]) -> bool {
    touches(first, second)
        || first.iter().any(|point| contains(second, *point))
        || second.iter().any(|point| contains(first, *point))
        || overlaps(first, second)
}

fn prepare(raw: Raw<'_>, origin: Child, original_size: usize) -> Result<Surface<'_>, EditError> {
    if !raw.supported_owner {
        return Err(invalid(
            "Terrain clipping neighbors must be world or func_detail terrain",
        ));
    }
    let info = raw.side.displacement.as_ref().unwrap();
    let (base, mesh) = native::checked_mesh(&raw.face, info).map_err(invalid)?;
    let native_info = bsp::DisplacementInfo {
        start: native::stored_vertex(mesh.base_positions[0])
            .as_vec3()
            .to_array(),
        power: info.power as i32,
        ..Default::default()
    };
    let vertices = native::vertices(&mesh);
    let positions = native::positions(base, native_info, vertices.clone())
        .map_err(invalid)?
        .into_iter()
        .map(|point| DVec3::from_array(point.map(f64::from)))
        .collect();
    let uv = native::positions(
        base,
        native_info,
        vec![bsp::DisplacementVertex::default(); vertices.len()],
    )
    .map_err(invalid)?
    .into_iter()
    .map(|point| crate::brush::texel_uv(raw.side, DVec3::from_array(point.map(f64::from))))
    .collect::<Vec<_>>();
    if uv.iter().any(|point| !point.is_finite()) {
        return Err(invalid(
            "Terrain neighbor native base UV must remain finite",
        ));
    }
    let indices = native_info
        .triangle_indices()
        .map_err(|error| invalid(error.to_string()))?;
    let source_tags = triangles::native_tags(info, &mesh)?;
    let order = triangles::correspondence(&indices, &triangles::source_indices(info.power)?)?;
    let tags = order.iter().map(|index| source_tags[*index]).collect();
    Ok(Surface {
        raw,
        origin,
        original_size,
        mesh,
        positions,
        uv,
        vertices,
        indices,
        tags,
    })
}

fn pair(mut a: Vertex, mut b: Vertex) -> [Vertex; 2] {
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    [a, b]
}

fn graph(surfaces: &[Surface<'_>], work: &mut Work) -> Result<Graph, EditError> {
    let grids = surfaces.iter().map(Surface::grid).collect::<Vec<_>>();
    let neighbors =
        neighbors::find_spans(&grids, &mut |amount| work.consume(amount)).map_err(invalid)?;
    if neighbors.ambiguous != 0 || neighbors.mismatched != 0 {
        return Err(invalid(
            "Terrain clipping requires unambiguous matching native full/half neighbors",
        ));
    }
    let native =
        neighbors::native_connections(&grids, &neighbors.pairs, &mut |amount| work.consume(amount))
            .map_err(invalid)?;
    let (lod, _) = lod::prepare(&grids, &neighbors.pairs, &mut |amount| work.consume(amount))
        .map_err(invalid)?;
    let lod = surfaces
        .iter()
        .zip(lod)
        .map(|(surface, patch)| {
            patch
                .map(|patch| lod_state(surface, patch, work))
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut links = Links::default();
    let mut connected = BTreeSet::new();
    for link in &neighbors.pairs {
        connected.insert((link.first.min(link.second), link.first.max(link.second)));
        let a = &surfaces[link.first];
        let b = &surfaces[link.second];
        if a.raw.id == b.raw.id {
            continue;
        }
        let common = neighbors::common_span_indices(
            (a.grid().power, link.first_edge, link.first_span),
            (b.grid().power, link.second_edge, link.second_span),
        )
        .map_err(invalid)?;
        for &(i, j) in &common {
            links.vertices.insert(pair(a.vertex(i), b.vertex(j)));
        }
        for endpoints in common.windows(2) {
            let mut first = [a.vertex(endpoints[0].0), a.vertex(endpoints[1].0)];
            let mut second = [b.vertex(endpoints[0].1), b.vertex(endpoints[1].1)];
            if first[0].face > second[0].face {
                std::mem::swap(&mut first, &mut second);
            }
            if first[0] > first[1] {
                first.swap(0, 1);
                second.swap(0, 1);
            }
            links.segments.insert(Segment { first, second });
        }
    }
    for (index, corners) in native.corners.iter().enumerate() {
        let a = &surfaces[index];
        let size = a.mesh.size;
        for (corner, peers) in corners.iter().enumerate() {
            for peer in &peers.neighbors[..usize::from(peers.count)] {
                connected.insert((index.min(usize::from(*peer)), index.max(usize::from(*peer))));
                let b = &surfaces[usize::from(*peer)];
                if a.raw.id == b.raw.id {
                    continue;
                }
                let position = grids[index].corners[corner];
                let other_corner = b
                    .grid()
                    .corners
                    .iter()
                    .position(|point| *point == position)
                    .ok_or_else(|| {
                        invalid("Terrain corner neighbor lost its native base identity")
                    })?;
                let other_size = b.mesh.size;
                links.vertices.insert(pair(
                    a.vertex([0, (size - 1) * size, size * size - 1, size - 1][corner]),
                    b.vertex(
                        [
                            0,
                            (other_size - 1) * other_size,
                            other_size * other_size - 1,
                            other_size - 1,
                        ][other_corner],
                    ),
                ));
            }
        }
    }
    for first in 0..surfaces.len() {
        for second in first + 1..surfaces.len() {
            work.consume(16).map_err(invalid)?;
            let a = &surfaces[first].raw.face.vertices;
            let b = &surfaces[second].raw.face.vertices;
            if overlaps(a, b) {
                return Err(invalid(
                    "Terrain clipping cannot preserve overlapping base domains",
                ));
            }
            if related(a, b) && !connected.contains(&(first, second)) {
                return Err(invalid(
                    "Terrain clipping contact lacks a complete native full/half or corner connection",
                ));
            }
        }
    }
    Ok(Graph {
        pairs: neighbors.pairs,
        links,
        lod,
    })
}

fn canonical(mut triangle: [usize; 3]) -> [usize; 3] {
    let first = (0..3).min_by_key(|index| triangle[*index]).unwrap();
    triangle.rotate_left(first);
    triangle
}

fn certify_lod(
    original: &Surface<'_>,
    previous: Option<&LodState>,
    outputs: &[Surface<'_>],
    current: &Graph,
    retained: &BTreeSet<Vertex>,
    cut: Option<&CutLine>,
    work: &mut Work,
) -> Result<(), EditError> {
    let previous = NativeState::prepare(original, previous, work)?;
    if let Some(cut) = cut {
        cut.certify(&previous.positions, original.mesh.size, work)?;
    }
    let mut expected = BTreeSet::new();
    for &triangle in &previous.indices {
        work.consume(1).map_err(invalid)?;
        if cut.is_some_and(|cut| cut.straddles(triangle, original.mesh.size)) {
            return Err(invalid(
                "Native terrain LOD triangles straddle the cut line",
            ));
        }
        if triangle
            .iter()
            .all(|index| retained.contains(&original.vertex(*index)))
        {
            expected.insert(if cut.is_some() {
                canonical(triangle)
            } else {
                triangle
            });
        }
    }
    let mut actual = BTreeSet::new();
    for (output_index, output) in outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| output.raw.id == original.raw.id)
    {
        let state = NativeState::prepare(output, current.lod[output_index].as_ref(), work)?;
        work.consume(POSITION_PROFILES * output.vertices.len() + state.indices.len())
            .map_err(invalid)?;
        for index in 0..output.vertices.len() {
            let source = output.vertex(index).index;
            if previous.allowed[source] != state.allowed[index]
                || previous.active[source] != state.active[index]
                || previous.alphas[source] != state.alphas[index]
                || previous
                    .positions
                    .iter()
                    .zip(&state.positions)
                    .any(|(original, output)| original[source] != output[index])
            {
                return Err(invalid(
                    "Terrain clipping cannot preserve mapped native LOD masks, active vertices, alpha and encoded geometry",
                ));
            }
        }
        for triangle in state.indices {
            let mapped = triangle.map(|index| output.vertex(index).index);
            let mapped = if cut.is_some() {
                canonical(mapped)
            } else {
                mapped
            };
            if !actual.insert(mapped) {
                return Err(invalid(
                    "Terrain clipping duplicated a maximal native LOD triangle",
                ));
            }
        }
    }
    if actual != expected {
        return Err(invalid(
            "Terrain clipping cannot preserve the retained maximal native LOD triangle multiset",
        ));
    }
    Ok(())
}

fn verify_selected(
    document: &Document,
    context: &Vmf,
    raw: &[Raw<'_>],
    replacements: &[Replacement<'_>],
    work: &mut Work,
) -> Result<(), EditError> {
    for replacement in replacements {
        verify_selected_source(document, context, raw, replacement, work)
            .map_err(|error| crate::clip::source_error(replacement.source.solid, error))?;
    }
    Ok(())
}

fn verify_selected_source(
    document: &Document,
    context: &Vmf,
    raw: &[Raw<'_>],
    replacement: &Replacement<'_>,
    work: &mut Work,
) -> Result<(), EditError> {
    let (owner, solid) = crate::subobject::solid(document, replacement.source.solid)?;
    if !native::terrain_source_is_visible(document.vmf(), solid.id) {
        return Ok(());
    }
    let owned = match owner {
        crate::ObjectId::Solid(_) => context
            .world
            .solids
            .iter()
            .any(|solid| solid.id == replacement.source.solid),
        crate::ObjectId::Entity(id) => context.entities.iter().any(|entity| {
            entity.id == id
                && entity.classname() == document.entity(id).and_then(|entity| entity.classname())
                && entity
                    .solids
                    .iter()
                    .any(|solid| solid.id == replacement.source.solid)
        }),
    };
    let matches = raw
        .iter()
        .filter(|face| face.id == replacement.source)
        .collect::<Vec<_>>();
    let [assembled] = matches.as_slice() else {
        return Err(invalid(
            "Resolved terrain context lost a selected source face identity",
        ));
    };
    let side_index = solid
        .sides
        .iter()
        .position(|side| side.id == replacement.source.side)
        .ok_or_else(|| invalid("Selected terrain source face is missing"))?;
    let side = &solid.sides[side_index];
    let mut canonical = solid.clone();
    crate::brush::canonicalize_precise(&mut canonical)?;
    if assembled.side.plane != side.plane
        && assembled.side.plane != canonical.sides[side_index].plane
    {
        return Err(invalid(
            "Resolved terrain context changed a selected source plane beyond precise canonicalization",
        ));
    }
    let mut normalized = side.clone();
    normalized.plane = assembled.side.plane;
    if !owned || normalized != *assembled.side {
        return Err(invalid(
            "Resolved terrain context changed selected source ownership, UV or terrain fields",
        ));
    }
    let geometry = BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
    let face = geometry
        .faces
        .into_iter()
        .find(|face| face.side == side_index)
        .ok_or_else(|| invalid("Selected terrain source has no effective face"))?;
    let power = replacement.source_power;
    let size = (1 << power) + 1;
    let origin = Child {
        row: 0,
        column: 0,
        power,
    };
    let original = prepare(
        Raw {
            id: replacement.source,
            side,
            face,
            supported_owner: true,
        },
        origin,
        size,
    )?;
    let assembled = prepare(
        Raw {
            id: assembled.id,
            side: assembled.side,
            face: assembled.face.clone(),
            supported_owner: assembled.supported_owner,
        },
        origin,
        size,
    )?;
    if original.indices != assembled.indices
        || original.tags != assembled.tags
        || original.vertices != assembled.vertices
        || original.mesh.size != assembled.mesh.size
    {
        return Err(invalid(
            "Resolved terrain context changed selected native vertices, triangle identities or tags",
        ));
    }
    let original_native =
        certified_native_positions(&original.raw.face, &original.mesh, power, work)?;
    let assembled_native =
        certified_native_positions(&assembled.raw.face, &assembled.mesh, power, work)?;
    work.consume(POSITION_PROFILES * original.vertices.len())
        .map_err(invalid)?;
    if original_native != assembled_native {
        return Err(invalid(
            "Resolved terrain context changed selected native or compiler vertex bits",
        ));
    }
    for index in 0..original.vertices.len() {
        if original.positions[index].distance(assembled.positions[index]) > SAMPLE_EPSILON
            || original.mesh.positions[index].distance(assembled.mesh.positions[index])
                > SAMPLE_EPSILON
            || original.mesh.base_positions[index].distance(assembled.mesh.base_positions[index])
                > SAMPLE_EPSILON
            || (original.uv[index] - assembled.uv[index])
                .abs()
                .max_element()
                > crate::texture_deform::TEXEL_EPSILON
        {
            return Err(invalid(
                "Resolved terrain context cannot preserve selected source geometry and base UV",
            ));
        }
    }
    Ok(())
}

fn component<'a>(
    document: &'a Document,
    replacements: &[Replacement<'_>],
    vmf: Option<&'a Vmf>,
    work: &mut Work,
) -> Result<Vec<Surface<'a>>, EditError> {
    let selected = replacements
        .iter()
        .filter(|replacement| {
            native::terrain_source_is_visible(document.vmf(), replacement.source.solid)
        })
        .map(|replacement| replacement.source)
        .collect::<BTreeSet<_>>();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let hidden = replacements
        .iter()
        .map(|replacement| replacement.source)
        .filter(|face| !selected.contains(face))
        .collect::<BTreeSet<_>>();
    let mut raw = collect(vmf.unwrap_or(document.vmf()))?;
    raw.retain(|face| !hidden.contains(&face.id));
    for face in &selected {
        if raw.iter().filter(|raw| raw.id == *face).count() != 1 {
            return Err(crate::clip::source_error(
                face.solid,
                invalid("Resolved terrain context lost a selected source face identity"),
            ));
        }
    }
    if let Some(context) = vmf {
        verify_selected(document, context, &raw, replacements, work)?;
    }
    let internal = replacements
        .iter()
        .filter(|replacement| {
            selected.contains(&replacement.source)
                && replacement
                    .children
                    .iter()
                    .any(|child| child.power + 1 < replacement.source_power)
        })
        .map(|replacement| replacement.source)
        .collect::<BTreeSet<_>>();
    let mut affected = raw
        .iter()
        .enumerate()
        .filter_map(|(index, face)| selected.contains(&face.id).then_some(index))
        .collect::<BTreeSet<_>>();
    let mut queue = affected.iter().copied().collect::<Vec<_>>();
    let mut cursor = 0;
    while cursor < queue.len() {
        let first = queue[cursor];
        cursor += 1;
        for second in 0..raw.len() {
            if affected.contains(&second) {
                continue;
            }
            work.consume(16).map_err(invalid)?;
            if related(&raw[first].face.vertices, &raw[second].face.vertices) {
                if affected.len() >= selected.len() + MAX_NEIGHBORS {
                    return Err(invalid(
                        "Terrain clipping exceeds its certified neighbor-face limit",
                    ));
                }
                affected.insert(second);
                queue.push(second);
            }
        }
    }
    let mut engaged = BTreeSet::new();
    for (offset, first) in queue.iter().enumerate() {
        if internal.contains(&raw[*first].id) {
            engaged.insert(*first);
        }
        for second in &queue[offset + 1..] {
            if related(&raw[*first].face.vertices, &raw[*second].face.vertices) {
                engaged.insert(*first);
                engaged.insert(*second);
            }
        }
    }
    if engaged.is_empty() {
        return Ok(Vec::new());
    }
    raw.into_iter()
        .enumerate()
        .filter(|(index, _)| engaged.contains(index))
        .map(|(_, raw)| {
            let power = raw.side.displacement.as_ref().unwrap().power;
            if !(2..=4).contains(&power) {
                return Err(invalid(
                    "Terrain clipping neighbors require power 2, 3 or 4",
                ));
            }
            let size = (1 << power) + 1;
            prepare(
                raw,
                Child {
                    row: 0,
                    column: 0,
                    power,
                },
                size,
            )
        })
        .collect::<Result<Vec<_>, _>>()
}

pub(crate) fn certify(
    document: &Document,
    replacements: &[Replacement<'_>],
    context: &mut Context<'_>,
) -> Result<Vec<Verification>, EditError> {
    let work = &mut context.work;
    let cuts = &context.cuts;
    for replacement in replacements {
        if (replacement.operation == Operation::Cut) != cuts.contains_key(&replacement.source) {
            return Err(crate::clip::source_error(
                replacement.source.solid,
                invalid("Terrain clipping lost its explicit cut or partition identity"),
            ));
        }
    }
    let selected = replacements
        .iter()
        .filter(|replacement| {
            native::terrain_source_is_visible(document.vmf(), replacement.source.solid)
        })
        .map(|replacement| replacement.source)
        .collect::<BTreeSet<_>>();
    let before = component(document, replacements, context.vmf, work)?;
    if before.is_empty() {
        return Ok(replacements
            .iter()
            .map(|_| Verification {
                neighbors: 0,
                links: 0,
            })
            .collect());
    }
    let previous = graph(&before, work)?;
    let mut after = Vec::new();
    for surface in &before {
        if let Some(replacement) = replacements
            .iter()
            .find(|replacement| replacement.source == surface.raw.id)
        {
            if replacement.parts.len() != replacement.children.len() {
                return Err(invalid("Terrain clipping lost its child grid origins"));
            }
            for (part, origin) in replacement.parts.iter().zip(replacement.children) {
                let geometry =
                    BrushGeometry::from_solid(part).map_err(|error| invalid(error.to_string()))?;
                let face = geometry
                    .faces
                    .into_iter()
                    .find(|face| part.sides[face.side].displacement.is_some())
                    .ok_or_else(|| invalid("Terrain child has no native face"))?;
                let side = &part.sides[face.side];
                after.push(prepare(
                    Raw {
                        id: surface.raw.id,
                        side,
                        face,
                        supported_owner: true,
                    },
                    *origin,
                    surface.mesh.size,
                )?);
            }
        } else {
            after.push(prepare(
                Raw {
                    id: surface.raw.id,
                    side: surface.raw.side,
                    face: surface.raw.face.clone(),
                    supported_owner: true,
                },
                Child {
                    row: 0,
                    column: 0,
                    power: surface.raw.side.displacement.as_ref().unwrap().power,
                },
                surface.mesh.size,
            )?);
        }
    }
    let current = graph(&after, work)?;
    let mut retained = BTreeSet::new();
    for surface in &after {
        retained.extend((0..surface.vertices.len()).map(|index| surface.vertex(index)));
    }
    let expected_segments = previous
        .links
        .segments
        .iter()
        .filter(|segment| {
            segment
                .first
                .iter()
                .chain(&segment.second)
                .all(|vertex| retained.contains(vertex))
        })
        .copied()
        .collect::<BTreeSet<_>>();
    let expected_vertices = previous
        .links
        .vertices
        .iter()
        .filter(|vertices| vertices.iter().all(|vertex| retained.contains(vertex)))
        .copied()
        .collect::<BTreeSet<_>>();
    if current.links.segments != expected_segments || current.links.vertices != expected_vertices {
        return Err(invalid(
            "Terrain clipping cannot preserve every retained native neighbor span and corner",
        ));
    }
    for (original_index, original) in before.iter().enumerate() {
        if selected.contains(&original.raw.id) {
            let replacement = replacements
                .iter()
                .find(|replacement| replacement.source == original.raw.id)
                .unwrap();
            let cut = match replacement.operation {
                Operation::Cut => Some(
                    cuts.get(&original.raw.id)
                        .ok_or_else(|| invalid("Terrain clipping lost its certified cut line"))?,
                ),
                Operation::Partition => {
                    if cuts.contains_key(&original.raw.id)
                        || (0..original.vertices.len())
                            .any(|index| !retained.contains(&original.vertex(index)))
                    {
                        return Err(invalid(
                            "Terrain neighbor partitioning must retain its entire noncrossing source grid",
                        ));
                    }
                    None
                }
            };
            certify_lod(
                original,
                previous.lod[original_index].as_ref(),
                &after,
                &current,
                &retained,
                cut,
                work,
            )?;
        }
        // Native collision classifies alpha with an ordered three-f32 sum.
        let mut expected = BTreeMap::new();
        for (index, triangle) in original.indices.iter().enumerate() {
            if triangle
                .iter()
                .all(|vertex| retained.contains(&original.vertex(*vertex)))
            {
                expected.insert(*triangle, original.tags[index]);
            }
        }
        let mut actual = BTreeMap::new();
        for (output_index, output) in after
            .iter()
            .enumerate()
            .filter(|(_, surface)| surface.raw.id == original.raw.id)
        {
            if !selected.contains(&original.raw.id) {
                work.consume(original.vertices.len() * 4 + original.indices.len())
                    .map_err(invalid)?;
                if previous.lod[original_index] != current.lod[output_index] {
                    return Err(invalid(
                        "Terrain clipping changed an unselected native neighbor LOD state",
                    ));
                }
            }
            if !selected.contains(&original.raw.id)
                && (output.raw.side != original.raw.side
                    || output.positions != original.positions
                    || output.indices != original.indices
                    || output.uv != original.uv
                    || output.vertices != original.vertices)
            {
                return Err(invalid(
                    "Terrain clipping changed an unselected native neighbor",
                ));
            }
            for (index, triangle) in output.indices.iter().enumerate() {
                let key = triangle.map(|vertex| output.vertex(vertex).index);
                if actual.insert(key, output.tags[index]).is_some() {
                    return Err(invalid(
                        "Terrain clipping duplicated a native source triangle",
                    ));
                }
            }
            for index in 0..output.vertices.len() {
                work.consume(1).map_err(invalid)?;
                let source = output.vertex(index).index;
                if output.positions[index].distance(original.positions[source]) > SAMPLE_EPSILON
                    || output.vertices[index].alpha != original.vertices[source].alpha
                    || (output.uv[index] - original.uv[source]).abs().max_element()
                        > crate::texture_deform::TEXEL_EPSILON
                {
                    return Err(invalid(
                        "Terrain clipping cannot preserve native neighbor geometry, UV and alpha",
                    ));
                }
            }
        }
        if actual != expected {
            return Err(invalid(
                "Terrain clipping cannot preserve the retained ordered native triangle and tag multiset",
            ));
        }
    }
    let mut adjacency = BTreeMap::<FaceId, BTreeSet<FaceId>>::new();
    for [a, b] in &previous.links.vertices {
        work.consume(1).map_err(invalid)?;
        adjacency.entry(a.face).or_default().insert(b.face);
        adjacency.entry(b.face).or_default().insert(a.face);
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
            while let Some(face) = queue.pop() {
                if let Some(peers) = adjacency.get(&face) {
                    for peer in peers {
                        if component.insert(*peer) {
                            queue.push(*peer);
                        }
                    }
                }
            }
            let neighbors = component
                .iter()
                .filter(|face| !selected.contains(face))
                .count();
            let links = expected_vertices
                .iter()
                .filter_map(|[a, b]| {
                    if a.face == replacement.source {
                        Some(b.face)
                    } else if b.face == replacement.source {
                        Some(a.face)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>()
                .len();
            Verification { neighbors, links }
        })
        .collect())
}
