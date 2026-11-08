use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::{
    bsp,
    vmf::{DispInfo, Side, Solid, Vmf},
};

use super::{DisplacementMesh, native, neighbors, smoothing, triangles};
use crate::clip::Keep;
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Face, ObjectId, Plane};

const SAMPLE_EPSILON: f64 = 1e-5;
pub(super) const CUT_EPSILON: f64 = 1e-7;
const NATIVE_SETUP_WORK: usize = 256;
const NATIVE_VERTEX_WORK: usize = 32;
const NATIVE_ROUNDINGS: [bsp::NativeDisplacementRounding; 4] = bsp::NativeDisplacementRounding::ALL;
const POSITION_PROFILES: usize = NATIVE_ROUNDINGS.len() + 1;

type NativePositions = [Vec<[u32; 3]>; POSITION_PROFILES];

mod network;
pub(crate) use network::Work;
pub(crate) use network::{
    MAX_NEIGHBORS, Replacement, certify, certify_subtraction, partition_neighbors,
    partition_subtraction_neighbors,
};
pub(super) use network::{overlaps, related};

pub(crate) struct Context<'a> {
    pub(super) vmf: Option<&'a Vmf>,
    pub(crate) work: Work,
    cuts: BTreeMap<FaceId, CutLine>,
}

impl<'a> Context<'a> {
    pub(crate) fn new(vmf: Option<&'a Vmf>) -> Self {
        Self::with_work(vmf, Work::default())
    }

    pub(crate) fn with_work(vmf: Option<&'a Vmf>, work: Work) -> Self {
        Self {
            vmf,
            work,
            cuts: BTreeMap::new(),
        }
    }

    pub(crate) fn into_work(self) -> Work {
        self.work
    }

    pub(crate) fn vmf(&self) -> Option<&Vmf> {
        self.vmf
    }
}

struct CutLine {
    plane: Plane,
    rows: bool,
    cut: usize,
    first_back: bool,
}

impl CutLine {
    fn certify(
        &self,
        native: &NativePositions,
        size: usize,
        work: &mut Work,
    ) -> Result<(), EditError> {
        work.consume(POSITION_PROFILES * size * size)
            .map_err(invalid)?;
        for positions in native {
            let positions = positions
                .iter()
                .map(|point| DVec3::from_array(point.map(|bits| f64::from(f32::from_bits(bits)))))
                .collect::<Vec<_>>();
            if grid_line(&positions, size, self.plane, self.rows, self.cut) != Some(self.first_back)
            {
                return Err(invalid(
                    "Cut plane cannot preserve complete terrain grid separation for native and compiler reconstruction",
                ));
            }
        }
        Ok(())
    }

    fn straddles(&self, triangle: [usize; 3], size: usize) -> bool {
        let coordinates = triangle.map(|index| {
            if self.rows {
                index / size
            } else {
                index % size
            }
        });
        coordinates.iter().any(|coordinate| *coordinate < self.cut)
            && coordinates.iter().any(|coordinate| *coordinate > self.cut)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Child {
    pub row: usize,
    pub column: usize,
    pub power: u32,
}

impl Child {
    pub(crate) fn cells(self) -> usize {
        1 << self.power
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Cut,
    Partition,
}

pub(crate) struct Plan {
    pub operation: Operation,
    pub parts: Vec<Solid>,
    pub source: FaceId,
    pub source_power: u32,
    pub output_power: Option<u32>,
    pub vertices: usize,
    pub triangles: usize,
    pub children: Vec<Child>,
}

impl Plan {
    fn replace(&mut self, child: Child, refined: Self) -> Result<(), EditError> {
        if self.operation != Operation::Partition
            || refined.operation != Operation::Partition
            || self.source != refined.source
            || self.source_power != refined.source_power
            || self.parts.len() != self.children.len()
        {
            return Err(invalid(
                "Terrain neighbor refinement lost its partition source identity",
            ));
        }
        let index = self
            .children
            .iter()
            .position(|current| *current == child)
            .ok_or_else(|| invalid("Terrain neighbor refinement lost its current coarse leaf"))?;
        self.vertices = self.vertices - (child.cells() + 1).pow(2) + refined.vertices;
        self.triangles = self.triangles - 2 * child.cells().pow(2) + refined.triangles;
        self.parts.splice(index..=index, refined.parts);
        self.children.splice(index..=index, refined.children);
        let power = self.children[0].power;
        self.output_power = self
            .children
            .iter()
            .all(|child| child.power == power)
            .then_some(power);
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn checked_rows<T>(rows: &Option<Vec<Vec<T>>>, size: usize) -> Result<(), EditError> {
    if rows
        .as_ref()
        .is_some_and(|rows| rows.len() != size || rows.iter().any(|row| row.len() != size))
    {
        return Err(invalid("Terrain grid fields have invalid row dimensions"));
    }
    Ok(())
}

fn slice<T: Clone>(
    rows: &Option<Vec<Vec<T>>>,
    row: usize,
    column: usize,
    cells: usize,
) -> Option<Vec<Vec<T>>> {
    rows.as_ref().map(|rows| {
        rows[row..=row + cells]
            .iter()
            .map(|row| row[column..=column + cells].to_vec())
            .collect()
    })
}

fn validate(info: &DispInfo, mesh: &DisplacementMesh) -> Result<Vec<u16>, EditError> {
    native::validate_allowed_vertex_cache(info).map_err(invalid)?;
    if !(3..=4).contains(&info.power) {
        return Err(invalid(
            "Terrain clipping requires a power 3 or 4 grid half-line, or a power 4 grid quarter-line",
        ));
    }
    checked_rows(&info.normals, mesh.size)?;
    checked_rows(&info.distances, mesh.size)?;
    checked_rows(&info.offsets, mesh.size)?;
    checked_rows(&info.offset_normals, mesh.size)?;
    checked_rows(&info.alphas, mesh.size)?;
    if info
        .offset_normals
        .iter()
        .flatten()
        .flatten()
        .any(|normal| !normal.is_finite())
    {
        return Err(invalid("Terrain offset normals must be finite"));
    }
    if !info.extra.is_empty()
        || info.extra_properties.iter().any(|(key, _)| {
            !["smooth", "mintess", "uaxis", "vaxis"]
                .iter()
                .any(|known| key.eq_ignore_ascii_case(known))
        })
    {
        return Err(invalid(
            "Opaque terrain payloads cannot be remapped to clipped grids",
        ));
    }
    smoothing::angle(info).map_err(invalid)?;
    mesh.validate_edit_bounds()?;
    triangles::native_tags(info, mesh)
}

fn point_segment_distance(point: DVec3, [a, b]: [DVec3; 2]) -> f64 {
    let edge = b - a;
    let length = edge.length_squared();
    if length == 0.0 {
        return point.distance(a);
    }
    point.distance(a + edge * ((point - a).dot(edge) / length).clamp(0.0, 1.0))
}

fn touches(first: &[DVec3], second: &[DVec3]) -> bool {
    (0..first.len()).any(|a| {
        let first = [first[a], first[(a + 1) % first.len()]];
        (0..second.len()).any(|b| {
            let second = [second[b], second[(b + 1) % second.len()]];
            first
                .into_iter()
                .any(|point| point_segment_distance(point, second) <= neighbors::EDGE_TOLERANCE)
                || second
                    .into_iter()
                    .any(|point| point_segment_distance(point, first) <= neighbors::EDGE_TOLERANCE)
        })
    })
}

fn contains(corners: &[DVec3], point: DVec3) -> bool {
    let Some(plane) = Plane::from_points([corners[0], corners[1], corners[2]]) else {
        return true;
    };
    plane.distance_to(point).abs() <= neighbors::EDGE_TOLERANCE
        && (0..corners.len()).all(|index| {
            let a = corners[index];
            let edge = corners[(index + 1) % corners.len()] - a;
            edge.cross(plane.normal).dot(point - a) >= -neighbors::EDGE_TOLERANCE * edge.length()
        })
}

fn resolved_instances(vmf: &Vmf) -> Result<(), EditError> {
    if vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) {
        return Err(invalid(
            "Terrain clipping requires a complete resolved context for visible map instances",
        ));
    }
    Ok(())
}

pub(crate) fn crosses(mesh: &DisplacementMesh, plane: Plane) -> bool {
    mesh.positions
        .iter()
        .any(|point| plane.distance_to(*point) > CUT_EPSILON)
        && mesh
            .positions
            .iter()
            .any(|point| plane.distance_to(*point) < -CUT_EPSILON)
}

pub(crate) fn classify(
    solid: &Solid,
    geometry: &BrushGeometry,
    plane: Plane,
    work: &mut Work,
) -> Result<crate::clip::TerrainPosition, EditError> {
    let positions = profiles(solid, geometry, work)?;
    work.consume(positions.iter().map(Vec::len).sum())
        .map_err(invalid)?;
    let mut front = false;
    let mut back = false;
    for point in positions.iter().flatten() {
        let distance = plane.distance_to(*point);
        front |= distance > CUT_EPSILON;
        back |= distance < -CUT_EPSILON;
    }
    match (front, back) {
        (true, true) => Ok(crate::clip::TerrainPosition::Crossing),
        (true, false) => Ok(crate::clip::TerrainPosition::Front),
        (false, true) => Ok(crate::clip::TerrainPosition::Back),
        (false, false) => Ok(crate::clip::TerrainPosition::Coplanar),
    }
}

pub(crate) fn profiles(
    solid: &Solid,
    geometry: &BrushGeometry,
    work: &mut Work,
) -> Result<Vec<Vec<DVec3>>, EditError> {
    let (side, face) = source_face(solid, geometry)?;
    let info = side.displacement.as_ref().unwrap();
    let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
    let positions = certified_native_positions(face, &mesh, info.power, work)?;
    work.consume((POSITION_PROFILES + 1) * mesh.positions.len())
        .map_err(invalid)?;
    Ok(std::iter::once(mesh.positions)
        .chain(positions.into_iter().map(|positions| {
            positions
                .into_iter()
                .map(|point| DVec3::from_array(point.map(|bits| f64::from(f32::from_bits(bits)))))
                .collect()
        }))
        .collect())
}

pub(crate) fn intersects(
    solid: &Solid,
    geometry: &BrushGeometry,
    volumes: &[Vec<crate::clip::TerrainCutPlane>],
    work: &mut Work,
) -> Result<bool, EditError> {
    let (side, face) = source_face(solid, geometry)?;
    let (_, mesh) =
        native::checked_mesh(face, side.displacement.as_ref().unwrap()).map_err(invalid)?;
    let triangles = mesh.triangles().collect::<Vec<_>>();
    let positions = profiles(solid, geometry, work)?;
    super::flat_clip::terrain_intersects(&positions, &triangles, volumes, work)
}

pub(crate) fn has_plane_area(
    solid: &Solid,
    geometry: &BrushGeometry,
    plane: &crate::clip::TerrainCutPlane,
    work: &mut Work,
) -> Result<bool, EditError> {
    let (side, face) = source_face(solid, geometry)?;
    let (_, mesh) =
        native::checked_mesh(face, side.displacement.as_ref().unwrap()).map_err(invalid)?;
    for positions in profiles(solid, geometry, work)? {
        for triangle in mesh.triangles() {
            let points = triangle.map(|index| positions[index]);
            if super::flat_clip::points_on_plane(&points, plane, work)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn grid_line(
    positions: &[DVec3],
    size: usize,
    plane: Plane,
    rows: bool,
    cut: usize,
) -> Option<bool> {
    let mut negative = [false; 2];
    let mut positive = [false; 2];
    for (index, point) in positions.iter().enumerate() {
        let coordinate = if rows { index / size } else { index % size };
        let distance = plane.distance_to(*point);
        if coordinate == cut {
            if distance.abs() > CUT_EPSILON {
                return None;
            }
        } else {
            if distance.abs() <= CUT_EPSILON {
                return None;
            }
            let half = usize::from(coordinate > cut);
            negative[half] |= distance < -CUT_EPSILON;
            positive[half] |= distance > CUT_EPSILON;
        }
    }
    match (negative, positive) {
        ([true, false], [false, true]) => Some(true),
        ([false, true], [true, false]) => Some(false),
        _ => None,
    }
}

fn certified_native_positions(
    face: &Face,
    mesh: &DisplacementMesh,
    power: u32,
    work: &mut Work,
) -> Result<NativePositions, EditError> {
    certified_encoded_positions(face, mesh, power, native::vertices(mesh), work)
}

fn certified_encoded_positions(
    face: &Face,
    mesh: &DisplacementMesh,
    power: u32,
    vertices: Vec<bsp::DisplacementVertex>,
    work: &mut Work,
) -> Result<NativePositions, EditError> {
    let base: [DVec3; 4] = face
        .vertices
        .as_slice()
        .try_into()
        .map_err(|_| invalid("Terrain native certificate needs a quad base"))?;
    let info = bsp::DisplacementInfo {
        start: native::stored_vertex(mesh.base_positions[0])
            .as_vec3()
            .to_array(),
        power: power as i32,
        ..Default::default()
    };
    work.consume(POSITION_PROFILES * (NATIVE_SETUP_WORK + NATIVE_VERTEX_WORK * vertices.len()))
        .map_err(invalid)?;
    let data = bsp::Displacements {
        infos: vec![info],
        vertices,
        triangles: Vec::new(),
    };
    let base = base.map(|point| native::stored_vertex(point).as_vec3().to_array());
    let mut result = std::array::from_fn(|_| Vec::new());
    for (output, rounding) in result.iter_mut().zip(NATIVE_ROUNDINGS) {
        let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
        *output = data
            .native_positions(0, base, profile)
            .map_err(|error| {
                invalid(format!(
                    "Terrain native reconstruction cannot be certified: {error}"
                ))
            })?
            .into_iter()
            .map(|point| point.map(f32::to_bits))
            .collect();
    }
    result[NATIVE_ROUNDINGS.len()] = data
        .positions(0, base)
        .map_err(|error| {
            invalid(format!(
                "Terrain compiler reconstruction cannot be certified: {error}"
            ))
        })?
        .into_iter()
        .map(|point| point.map(f32::to_bits))
        .collect();
    Ok(result)
}

fn carrier_plane(a: DVec3, b: DVec3, normal: DVec3) -> Result<Plane, EditError> {
    let normal = (b - a)
        .cross(normal)
        .try_normalize()
        .ok_or_else(|| invalid("Terrain grid line has no carrier plane"))?;
    Ok(Plane {
        normal,
        dist: normal.dot(a),
    })
}

fn behind(mut plane: Plane, point: DVec3) -> Plane {
    if plane.distance_to(point) > 0.0 {
        plane.normal = -plane.normal;
        plane.dist = -plane.dist;
    }
    plane
}

fn child_info(
    info: &DispInfo,
    mesh: &DisplacementMesh,
    child: Child,
) -> Result<DispInfo, EditError> {
    let Child { row, column, power } = child;
    let cells = child.cells();
    let mut result = info.clone();
    native::reset_allowed_vertex_cache(&mut result).map_err(invalid)?;
    result.power = power;
    result.start_position = mesh.base_positions[row * mesh.size + column];
    result.normals = slice(&info.normals, row, column, cells);
    result.distances = slice(&info.distances, row, column, cells);
    result.offsets = slice(&info.offsets, row, column, cells);
    result.offset_normals = slice(&info.offset_normals, row, column, cells);
    result.alphas = slice(&info.alphas, row, column, cells);
    result.triangle_tags = info.triangle_tags.as_ref().map(|rows| {
        rows[row..row + cells]
            .iter()
            .map(|row| row[column * 2..(column + cells) * 2].to_vec())
            .collect()
    });
    Ok(result)
}

pub(crate) fn quadrants(child: Child) -> [Child; 4] {
    let power = child.power - 1;
    let cells = 1 << power;
    [(0, 0), (0, cells), (cells, 0), (cells, cells)].map(|(row, column)| Child {
        row: child.row + row,
        column: child.column + column,
        power,
    })
}

pub(crate) fn cut_indices(power: u32) -> &'static [usize] {
    match power {
        3 => &[4],
        4 => &[8, 4, 12],
        _ => &[],
    }
}

pub(crate) fn cut_children(
    power: u32,
    rows: bool,
    cut: usize,
    first_back: bool,
    keep: Keep,
) -> Option<Vec<Child>> {
    if !cut_indices(power).contains(&cut) {
        return None;
    }
    Some(
        quadrants(Child {
            row: 0,
            column: 0,
            power,
        })
        .into_iter()
        .flat_map(|child| {
            let origin = if rows { child.row } else { child.column };
            if origin < cut && cut < origin + child.cells() {
                quadrants(child).to_vec()
            } else {
                vec![child]
            }
        })
        .filter(|child| {
            let coordinate = if rows { child.row } else { child.column };
            let back = (coordinate < cut) == first_back;
            !matches!((keep, back), (Keep::Back, false) | (Keep::Front, true))
        })
        .collect(),
    )
}

fn child_carrier(
    solid: &Solid,
    mesh: &DisplacementMesh,
    normal: DVec3,
    child: Child,
    material: &str,
) -> Result<Solid, EditError> {
    let cells = child.cells();
    let last = mesh.size - 1;
    let center =
        mesh.base_positions[(child.row + cells / 2) * mesh.size + child.column + cells / 2];
    let mut part = solid.clone();
    for row in [child.row, child.row + cells] {
        if row != 0 && row != last {
            let plane = carrier_plane(
                mesh.base_positions[row * mesh.size],
                mesh.base_positions[row * mesh.size + last],
                normal,
            )?;
            part = crate::clip::half(&part, behind(plane, center), material)?;
        }
    }
    for column in [child.column, child.column + cells] {
        if column != 0 && column != last {
            let plane = carrier_plane(
                mesh.base_positions[column],
                mesh.base_positions[last * mesh.size + column],
                normal,
            )?;
            part = crate::clip::half(&part, behind(plane, center), material)?;
        }
    }
    Ok(part)
}

struct Prepared<'a> {
    solid: &'a Solid,
    side: &'a Side,
    face: Face,
    mesh: DisplacementMesh,
    native: NativePositions,
    tags: Vec<u16>,
}

fn source_face<'a, 'b>(
    solid: &'a Solid,
    geometry: &'b BrushGeometry,
) -> Result<(&'a Side, &'b Face), EditError> {
    let displaced = solid
        .sides
        .iter()
        .enumerate()
        .filter(|(_, side)| side.displacement.is_some())
        .collect::<Vec<_>>();
    let [(side_index, side)] = displaced.as_slice() else {
        return Err(invalid(
            "Terrain clipping requires one displaced face per brush",
        ));
    };
    let face = geometry
        .faces
        .iter()
        .find(|face| face.side == *side_index)
        .ok_or_else(|| invalid("Terrain has no effective base face"))?;
    Ok((side, face))
}

fn prepare<'a>(
    document: &Document,
    solid: &'a Solid,
    geometry: &BrushGeometry,
    context: &mut Context<'_>,
) -> Result<Prepared<'a>, EditError> {
    let (side, face) = source_face(solid, geometry)?;
    let info = side.displacement.as_ref().unwrap();
    let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
    let (owner, _) = crate::subobject::solid(document, solid.id)?;
    if let ObjectId::Entity(id) = owner
        && document.entity(id).and_then(|entity| entity.classname()) != Some("func_detail")
    {
        return Err(invalid(
            "Terrain clipping requires world or func_detail brushes",
        ));
    }
    let tags = validate(info, &mesh)?;
    crate::subobject::fragments::admit_source(solid)?;
    if native::terrain_source_is_visible(document.vmf(), solid.id) {
        resolved_instances(context.vmf.unwrap_or(document.vmf()))?;
    }
    let native = certified_native_positions(face, &mesh, info.power, &mut context.work)?;
    Ok(Prepared {
        solid,
        side,
        face: face.clone(),
        mesh,
        native,
        tags,
    })
}

impl Prepared<'_> {
    fn materialize(
        &self,
        children: Vec<Child>,
        material: &str,
        operation: Operation,
        work: &mut Work,
    ) -> Result<Plan, EditError> {
        let Self {
            solid,
            side,
            face,
            mesh,
            native: original_native,
            tags: original_tags,
        } = self;
        let info = side.displacement.as_ref().unwrap();
        if children.is_empty()
            || children.iter().any(|child| {
                !(2..=info.power).contains(&child.power)
                    || child
                        .row
                        .checked_add(child.cells())
                        .is_none_or(|end| end >= mesh.size)
                    || child
                        .column
                        .checked_add(child.cells())
                        .is_none_or(|end| end >= mesh.size)
            })
        {
            return Err(invalid(
                "Terrain child origins must remain inside their original native source grid",
            ));
        }
        let last = mesh.size - 1;
        let mut parts = Vec::with_capacity(children.len());
        for &child in &children {
            let Child { row, column, .. } = child;
            let cells = child.cells();
            let mut part = child_carrier(solid, mesh, face.plane.normal, child, material)?;
            let index = part
                .sides
                .iter()
                .position(|candidate| candidate.id == side.id)
                .ok_or_else(|| invalid("Terrain child lost its base face"))?;
            part.sides[index].displacement = Some(child_info(info, mesh, child)?);
            let geometry =
                BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
            let face = geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .ok_or_else(|| invalid("Terrain child has no effective base face"))?;
            let output_info = part.sides[index].displacement.as_ref().unwrap();
            let (_, output) = native::checked_mesh(face, output_info).map_err(invalid)?;
            let output_native = certified_native_positions(face, &output, output_info.power, work)?;
            work.consume(POSITION_PROFILES * output.positions.len())
                .map_err(invalid)?;
            for child_row in 0..output.size {
                for child_column in 0..output.size {
                    let target = child_row * output.size + child_column;
                    let source = (row + child_row) * mesh.size + column + child_column;
                    let original_uv = crate::brush::texel_uv(side, mesh.base_positions[source]);
                    let child_uv = crate::brush::texel_uv(side, output.base_positions[target]);
                    if original_native
                        .iter()
                        .zip(&output_native)
                        .any(|(original, output)| original[source] != output[target])
                    {
                        return Err(invalid(
                            "Terrain carrier cannot preserve native and compiler vertex bits",
                        ));
                    }
                    if output.positions[target].distance(mesh.positions[source]) > SAMPLE_EPSILON
                        || output.base_positions[target].distance(mesh.base_positions[source])
                            > SAMPLE_EPSILON
                        || output.alphas[target] != mesh.alphas[source]
                        || !original_uv.is_finite()
                        || !child_uv.is_finite()
                        || (original_uv - child_uv).abs().max_element()
                            > crate::texture_deform::TEXEL_EPSILON
                    {
                        return Err(invalid(
                            "Terrain carrier cannot preserve its existing grid samples and base UV",
                        ));
                    }
                }
            }
            let tags = triangles::native_tags(output_info, &output)?;
            for child_row in 0..cells {
                for child_column in 0..cells {
                    for triangle in 0..2 {
                        if tags[(child_row * cells + child_column) * 2 + triangle]
                            != original_tags
                                [((row + child_row) * last + column + child_column) * 2 + triangle]
                        {
                            return Err(invalid(
                                "Terrain child cannot preserve physical triangle tags",
                            ));
                        }
                    }
                }
            }
            parts.push(part);
        }
        let power = children[0].power;
        let output_power = children
            .iter()
            .all(|child| child.power == power)
            .then_some(power);
        let vertices = children
            .iter()
            .map(|child| (child.cells() + 1).pow(2))
            .sum();
        let triangles = children.iter().map(|child| child.cells().pow(2) * 2).sum();
        Ok(Plan {
            operation,
            parts,
            source: FaceId {
                solid: solid.id,
                side: side.id,
            },
            source_power: info.power,
            output_power,
            vertices,
            triangles,
            children,
        })
    }
}

fn prepare_partition<'a>(
    document: &Document,
    solid: &'a Solid,
    geometry: &BrushGeometry,
    plane: Plane,
    context: &mut Context<'_>,
) -> Result<Prepared<'a>, EditError> {
    let source = prepare(document, solid, geometry, context)?;
    context
        .work
        .consume((POSITION_PROFILES + 1) * source.mesh.positions.len())
        .map_err(invalid)?;
    if crosses(&source.mesh, plane)
        || source.native.iter().any(|positions| {
            let mut back = false;
            let mut front = false;
            for point in positions {
                let point = DVec3::from_array(point.map(|bits| f64::from(f32::from_bits(bits))));
                let distance = plane.distance_to(point);
                back |= distance < -CUT_EPSILON;
                front |= distance > CUT_EPSILON;
            }
            back && front
        })
    {
        return Err(invalid(
            "Terrain neighbor partitioning requires a noncrossing source in authored, native and compiler geometry",
        ));
    }
    Ok(source)
}

pub(crate) fn partition(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    material: &str,
    context: &mut Context<'_>,
) -> Result<Plan, EditError> {
    let source = prepare(document, solid, geometry, context)?;
    let power = source.side.displacement.as_ref().unwrap().power;
    if power <= 2 {
        return Err(invalid(
            "Power 2 terrain requires Cells or Fragments for further partitioning",
        ));
    }
    source.materialize(
        quadrants(Child {
            row: 0,
            column: 0,
            power,
        })
        .to_vec(),
        material,
        Operation::Partition,
        &mut context.work,
    )
}

pub(crate) fn plan(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    plane: Plane,
    keep: Keep,
    material: &str,
    context: &mut Context<'_>,
) -> Result<Option<Plan>, EditError> {
    let (side, face) = source_face(solid, geometry)?;
    let info = side.displacement.as_ref().unwrap();
    let mesh = DisplacementMesh::build(face, info).map_err(|error| invalid(error.to_string()))?;
    if !crosses(&mesh, plane) {
        return Ok(None);
    }
    let source = prepare(document, solid, geometry, context)?;
    let mesh = &source.mesh;
    let (rows, cut, first_back) = cut_indices(info.power)
        .iter()
        .copied()
        .find_map(|cut| {
            grid_line(&mesh.positions, mesh.size, plane, true, cut)
                .map(|first_back| (true, cut, first_back))
                .or_else(|| {
                    grid_line(&mesh.positions, mesh.size, plane, false, cut)
                        .map(|first_back| (false, cut, first_back))
                })
        })
        .ok_or_else(|| {
            invalid("Cut plane must follow a complete existing terrain grid half-line, or a power 4 quarter-line, with each region wholly on one side")
        })?;
    let cut_line = CutLine {
        plane,
        rows,
        cut,
        first_back,
    };
    cut_line.certify(&source.native, mesh.size, &mut context.work)?;
    let mut children = Vec::new();
    for back in [true, false] {
        if matches!((keep, back), (Keep::Back, false) | (Keep::Front, true)) {
            continue;
        }
        let retained = if back { Keep::Back } else { Keep::Front };
        children.extend(
            cut_children(info.power, rows, cut, first_back, retained)
                .ok_or_else(|| invalid("Terrain cut lost its qualified grid layout"))?,
        );
    }
    let plan = source.materialize(children, material, Operation::Cut, &mut context.work)?;
    context.cuts.insert(plan.source, cut_line);
    Ok(Some(plan))
}
