use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::{
    bsp,
    vmf::{Side, Solid, Vmf},
};

use super::{
    Exact, FIELD_WORK, data, field::PlanarField, invalid, orient, reconstructed, same, weighted,
};
use crate::displacement::{
    grid_clip::{self, Context, Work},
    lod, native, neighbors, triangles,
};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Face, ObjectId};

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
    power: u32,
    corners: [DVec3; 4],
    base_positions: Vec<DVec3>,
    positions: Vec<Vec<DVec3>>,
    sheet: Sheet,
}

struct Sheet {
    axis: usize,
    coordinate: f64,
    field: Option<PlanarField>,
    sign: i128,
}

impl Sheet {
    fn matches(&self, other: &Self) -> Result<bool, EditError> {
        if self.axis != other.axis || self.coordinate != other.coordinate || self.sign != other.sign
        {
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

fn dot(point: DVec3, axis: DVec3) -> Result<Exact, EditError> {
    Exact::float(point.x)?
        .mul(Exact::float(axis.x)?)?
        .add(Exact::float(point.y)?.mul(Exact::float(axis.y)?)?)?
        .add(Exact::float(point.z)?.mul(Exact::float(axis.z)?)?)
}

fn range(points: &[DVec3], axis: DVec3) -> Result<[Exact; 2], EditError> {
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
    let Some(axis) = (0..3).find(|&axis| {
        first
            .iter()
            .chain(second)
            .all(|point| point[axis] == first[0][axis])
    }) else {
        return Ok(Some(0));
    };
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let projection_work = first
        .len()
        .checked_add(second.len())
        .and_then(|count| count.checked_mul(8))
        .ok_or_else(|| invalid("Flat terrain contact work overflow"))?;
    let mut touching = Vec::new();
    for points in [first, second] {
        for edge in 0..points.len() {
            let delta = points[(edge + 1) % points.len()] - points[edge];
            let mut normal = DVec3::ZERO;
            normal[axes[0]] = -delta[axes[1]];
            normal[axes[1]] = delta[axes[0]];
            work.consume(projection_work).map_err(invalid)?;
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
        if orient(DVec3::ZERO, first, second, axes)? != 0 {
            // Independent support lines confine the intersection to one corner.
            return Ok(Some(2));
        }
    }
    Ok(Some(1))
}

fn collect<'a>(vmf: &'a Vmf, work: &mut Work) -> Result<Vec<Raw<'a>>, EditError> {
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

fn prepare<'a>(raw: Raw<'a>, mode: Mode, work: &mut Work) -> Result<Surface<'a>, EditError> {
    if !raw.supported {
        return Err(invalid(
            "Flat terrain neighbors must be world or func_detail terrain",
        ));
    }
    let info = raw.side.displacement.as_ref().unwrap();
    let (_, mesh) = native::checked_mesh(&raw.face, info).map_err(invalid)?;
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
    work.consume(FIELD_WORK * corner_indices.len())
        .map_err(invalid)?;
    let field = match mode {
        Mode::Flat => Some(PlanarField::new(
            corners,
            corner_indices.map(|index| mesh.positions[index]),
        )?),
        Mode::Cells => None,
    };
    let axis = if let Some(field) = &field {
        field.axis()
    } else {
        (0..3)
            .find(|&axis| corners.iter().all(|point| point[axis] == corners[0][axis]))
            .ok_or_else(|| invalid("Terrain cell contacts require a world-axis base plane"))?
    };
    let coordinate = corners[0][axis];
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let sign = orient(corners[0], corners[1], corners[2], axes)?;
    if sign == 0 {
        return Err(invalid(
            "Flat terrain contacts require noncollapsed native cages",
        ));
    }
    super::convex(&corners, axes, sign)?;
    work.consume(mesh.positions.len() * (positions.len() + FIELD_WORK * 2))
        .map_err(invalid)?;
    for (index, &point) in mesh.positions.iter().enumerate() {
        let base = mesh.base_positions[index];
        if let Some(field) = &field {
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
        } else if base[axis] != coordinate
            || axes.iter().any(|&axis| point[axis] != base[axis])
            || positions
                .iter()
                .any(|positions| !same(point, positions[index]))
        {
            return Err(invalid(
                "Connected terrain cells require exact authored, native and compiler height-only positions",
            ));
        }
    }
    let indices = data.infos[0]
        .triangle_indices()
        .map_err(|error| invalid(error.to_string()))?;
    work.consume(indices.len()).map_err(invalid)?;
    for triangle in indices {
        let points = triangle.map(|index| positions.last().unwrap()[index]);
        if orient(points[0], points[1], points[2], axes)? != sign {
            return Err(invalid(
                "Connected flat terrain currently requires consistently oriented positive native triangles",
            ));
        }
    }
    match mode {
        Mode::Flat => super::certify_normals(&data, &positions, work)?,
        Mode::Cells => super::certify_height_normals(&data, &positions, axis, work)?,
    }
    let sheet = Sheet {
        axis,
        coordinate,
        field,
        sign,
    };
    Ok(Surface {
        power: info.power,
        raw,
        corners,
        base_positions: mesh.base_positions,
        positions,
        sheet,
    })
}

fn graph(surfaces: &[&Surface<'_>], work: &mut Work) -> Result<Graph, EditError> {
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
        edges.insert((pair.first.min(pair.second), pair.first.max(pair.second)));
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
            work.consume(FIELD_WORK).map_err(invalid)?;
            if !surfaces[first].sheet.matches(&surfaces[second].sheet)? {
                return Err(invalid(if surfaces[first].sheet.field.is_some() {
                    "Connected flat terrain currently requires compatible coplanar native fields and orientation"
                } else {
                    "Connected terrain cells require a common native base plane and orientation"
                }));
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
    Ok(result)
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
    let axis = surface.sheet.axis;
    if point[axis] != points[0][axis] {
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
        work.consume(16).map_err(invalid)?;
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
        work,
    )?;
    work.consume(original.positions.iter().map(Vec::len).sum())
        .map_err(invalid)?;
    work.consume(
        FIELD_WORK
            + if mode == Mode::Cells {
                original.base_positions.len()
            } else {
                0
            },
    )
    .map_err(invalid)?;
    if original.power != assembled.power
        || original.corners != assembled.corners
        || original.positions != assembled.positions
        || mode == Mode::Cells && original.base_positions != assembled.base_positions
        || !original.sheet.matches(&assembled.sheet)?
    {
        return Err(invalid(
            "Flat terrain context changed original native or compiler geometry",
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
    let mut affected = BTreeSet::new();
    for source in &selected {
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
                if mode == Mode::Cells && !selected.contains(&raw[second].source) {
                    return Err(invalid(
                        "Terrain cell cuts require every affected original patch to be a crossing replacement; unchanged neighbor normal custody is unproved",
                    ));
                }
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
    let before = raw
        .into_iter()
        .enumerate()
        .filter(|(index, _)| affected.contains(index))
        .map(|(_, raw)| prepare(raw, mode, work))
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
    let previous = graph(&before_refs, work)?;
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
    let current = graph(&after, work)?;
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
