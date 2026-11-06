use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::{
    bsp,
    vmf::{DispInfo, Side, Solid},
};

use super::{
    DisplacementMesh,
    grid_clip::{CUT_EPSILON, Context, Work},
    native, neighbors, smoothing, triangles,
};
use crate::clip::{Keep, TerrainBlendMode, TerrainMode, TerrainPlan, TerrainTexture};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Face, ObjectId, Plane};

pub(crate) mod cells;
mod field;
pub(crate) mod fragments;
mod labels;
pub(crate) mod network;
mod paint;

use field::{BasePlane, PlanarField, ScalarField};
use labels::TriangleField;
use paint::PaintField;

const POWER: u32 = 2;
const SIZE: usize = (1 << POWER) + 1;
const VERTICES: usize = SIZE * SIZE;
const TRIANGLES: usize = (SIZE - 1) * (SIZE - 1) * 2;
const POSITION_WORK: usize = 256;
const NORMAL_SETUP_WORK: usize = 256;
const NORMAL_CELL_WORK: usize = 256;
const UV_WORK: usize = 1024;
const UV_VERTEX_WORK: usize = 128;
const DETAIL_UV_VERTEX_WORK: usize = 8;
const FIELD_WORK: usize = 64;
const CARRIER_EPSILON: f64 = 1e-8;

type Point = [u64; 3];
type Edges = BTreeMap<(Point, Point), usize>;

pub(crate) struct Request<'a> {
    pub plane: Plane,
    pub keep: Keep,
    pub material: &'a str,
    pub textures: Option<&'a BTreeMap<FaceId, TerrainTexture>>,
    pub blend_mode: TerrainBlendMode,
}

pub(crate) struct PartitionRequest<'a> {
    pub material: &'a str,
    pub textures: Option<&'a BTreeMap<FaceId, TerrainTexture>>,
    pub blend_mode: TerrainBlendMode,
}

pub(crate) fn partition_counts(
    power: u32,
    mode: TerrainMode,
) -> Result<(usize, usize, usize), EditError> {
    if !(2..=4).contains(&power) {
        return Err(invalid(
            "Terrain neighbor retiling requires power 2, 3 or 4",
        ));
    }
    let multiplier = match mode {
        TerrainMode::Cells => 1,
        TerrainMode::Fragments => 4,
        _ => return Err(invalid("Whole-source retiling requires Cells or Fragments")),
    };
    let count = (1usize << power).pow(2) * multiplier;
    if count > crate::clip::MAX_TERRAIN_PATCHES
        || count
            .checked_mul(VERTICES)
            .is_none_or(|vertices| vertices > crate::clip::MAX_TERRAIN_VERTICES)
        || count
            .checked_mul(TRIANGLES)
            .is_none_or(|triangles| triangles > crate::clip::MAX_TERRAIN_TRIANGLES)
    {
        return Err(invalid(
            "Terrain neighbor retiling exceeds its patch, vertex or triangle limit",
        ));
    }
    Ok((count, count * VERTICES, count * TRIANGLES))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Exact {
    value: i128,
    exponent: i32,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn numeric() -> EditError {
    invalid("Flat terrain cut exceeds the exact numeric domain")
}

impl Exact {
    fn new(value: i128, exponent: i32) -> Self {
        if value == 0 {
            return Self {
                value: 0,
                exponent: 0,
            };
        }
        let shift = value.unsigned_abs().trailing_zeros();
        Self {
            value: value >> shift,
            exponent: exponent + shift as i32,
        }
    }

    fn float(value: f64) -> Result<Self, EditError> {
        if !value.is_finite() {
            return Err(numeric());
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let significand = bits & ((1 << 52) - 1);
        let significand = if exponent == 0 {
            significand
        } else {
            significand | (1 << 52)
        };
        let value = i128::from(significand) * if bits >> 63 == 0 { 1 } else { -1 };
        Ok(Self::new(
            value,
            if exponent == 0 {
                -1074
            } else {
                exponent - 1075
            },
        ))
    }

    fn add(self, other: Self) -> Result<Self, EditError> {
        if self.value == 0 {
            return Ok(other);
        }
        if other.value == 0 {
            return Ok(self);
        }
        let exponent = self.exponent.min(other.exponent);
        let scale = |value: Self| {
            let shift = u32::try_from(value.exponent - exponent).ok()?;
            let factor = 1i128.checked_shl(shift).filter(|factor| *factor > 0)?;
            value.value.checked_mul(factor)
        };
        let a = scale(self).ok_or_else(numeric)?;
        let b = scale(other).ok_or_else(numeric)?;
        Ok(Self::new(a.checked_add(b).ok_or_else(numeric)?, exponent))
    }

    fn sub(self, other: Self) -> Result<Self, EditError> {
        self.add(Self {
            value: other.value.checked_neg().ok_or_else(numeric)?,
            ..other
        })
    }

    fn mul(self, other: Self) -> Result<Self, EditError> {
        Ok(Self::new(
            self.value.checked_mul(other.value).ok_or_else(numeric)?,
            self.exponent + other.exponent,
        ))
    }

    fn div(self, other: Self) -> Result<Self, EditError> {
        if other.value == 0 || self.value.checked_rem(other.value) != Some(0) {
            return Err(numeric());
        }
        Ok(Self::new(
            self.value.checked_div(other.value).ok_or_else(numeric)?,
            self.exponent - other.exponent,
        ))
    }

    fn value(self) -> Result<f64, EditError> {
        let result = self.value as f64 * 2.0f64.powi(self.exponent);
        if Self::float(result)? != self {
            return Err(numeric());
        }
        Ok(result)
    }
}

fn key(point: DVec3) -> Point {
    point
        .to_array()
        .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

fn same(a: DVec3, b: DVec3) -> bool {
    key(a) == key(b)
}

fn stored(point: DVec3) -> Result<DVec3, EditError> {
    if !point.is_finite() || !same(point, native::stored_vertex(point)) {
        return Err(invalid(
            "Flat terrain cage points must survive native compiler storage exactly",
        ));
    }
    Ok(point)
}

fn orient(a: DVec3, b: DVec3, c: DVec3, axes: [usize; 2]) -> Result<i128, EditError> {
    let [u, v] = axes;
    let x = Exact::float(b[u])?.sub(Exact::float(a[u])?)?;
    let y = Exact::float(b[v])?.sub(Exact::float(a[v])?)?;
    let z = Exact::float(c[u])?.sub(Exact::float(a[u])?)?;
    let w = Exact::float(c[v])?.sub(Exact::float(a[v])?)?;
    Ok(x.mul(w)?.sub(y.mul(z)?)?.value.signum())
}

fn convex(points: &[DVec3], axes: [usize; 2], sign: i128) -> Result<(), EditError> {
    for index in 0..points.len() {
        for other in 0..points.len() {
            if other == index || other == (index + 1) % points.len() {
                continue;
            }
            if orient(
                points[index],
                points[(index + 1) % points.len()],
                points[other],
                axes,
            )? != sign
            {
                return Err(invalid("Flat terrain polygons must remain strictly convex"));
            }
        }
    }
    Ok(())
}

fn rows<T: PartialEq>(rows: &Option<Vec<Vec<T>>>, size: usize) -> Result<(), EditError> {
    if let Some(rows) = rows
        && (rows.len() != size
            || rows.iter().any(|row| row.len() != size)
            || rows.iter().flatten().any(|value| value != &rows[0][0]))
    {
        return Err(invalid(
            "Terrain retiling requires uniform, correctly sized source fields",
        ));
    }
    Ok(())
}

struct Prepared<'a> {
    side: &'a Side,
    info: &'a DispInfo,
    face: &'a Face,
    mesh: DisplacementMesh,
    field: PlanarField,
    paint: PaintField,
    axes: [usize; 2],
    sign: i128,
    ring: [DVec3; 4],
    data: bsp::Displacements,
}

struct Admitted<'a> {
    side: &'a Side,
    info: &'a DispInfo,
    face: &'a Face,
    mesh: DisplacementMesh,
    data: bsp::Displacements,
    positions: Vec<Vec<DVec3>>,
}

struct HeightSheet {
    axis: usize,
    axes: [usize; 2],
    sign: i128,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Original,
    Child,
}

fn data(
    face: &Face,
    mesh: &DisplacementMesh,
    power: u32,
) -> Result<(bsp::Displacements, [[f32; 3]; 4]), EditError> {
    let base: [DVec3; 4] = face
        .vertices
        .as_slice()
        .try_into()
        .map_err(|_| invalid("Terrain retiling requires a convex quad cage"))?;
    Ok((
        bsp::Displacements {
            infos: vec![bsp::DisplacementInfo {
                start: native::stored_vertex(mesh.base_positions[0])
                    .as_vec3()
                    .to_array(),
                power: power as i32,
                ..Default::default()
            }],
            vertices: native::vertices(mesh),
            triangles: Vec::new(),
        },
        base.map(|point| native::stored_vertex(point).as_vec3().to_array()),
    ))
}

fn reconstructed(
    data: &bsp::Displacements,
    base: [[f32; 3]; 4],
    work: &mut Work,
) -> Result<Vec<Vec<DVec3>>, EditError> {
    work.consume(5 * (POSITION_WORK + 32 * data.vertices.len()))
        .map_err(invalid)?;
    let mut result = Vec::with_capacity(5);
    for rounding in bsp::NativeDisplacementRounding::ALL {
        let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
        let positions = data
            .native_positions(0, base, profile)
            .map_err(|error| invalid(error.to_string()))?;
        result.push(
            positions
                .into_iter()
                .map(|point| DVec3::from_array(point.map(f64::from)))
                .collect(),
        );
    }
    result.push(
        data.positions(0, base)
            .map_err(|error| invalid(error.to_string()))?
            .into_iter()
            .map(|point| DVec3::from_array(point.map(f64::from)))
            .collect(),
    );
    Ok(result)
}

fn native_base(
    mesh: &mut DisplacementMesh,
    data: &bsp::Displacements,
    base: [[f32; 3]; 4],
    work: &mut Work,
) -> Result<(), EditError> {
    let mut base_data = data.clone();
    for vertex in &mut base_data.vertices {
        vertex.direction = [0.0; 3];
        vertex.distance = 0.0;
    }
    let profiles = reconstructed(&base_data, base, work)?;
    let count = mesh.base_positions.len();
    work.consume(profiles.len() * count).map_err(invalid)?;
    if profiles.iter().any(|points| {
        points.len() != count || points.iter().zip(&profiles[0]).any(|(&a, &b)| !same(a, b))
    }) {
        return Err(invalid(
            "Terrain retiling requires one exact native base grid across reconstruction profiles",
        ));
    }
    mesh.base_positions = profiles.into_iter().next().unwrap();
    Ok(())
}

fn certify_normals(
    data: &bsp::Displacements,
    profiles: &[Vec<DVec3>],
    work: &mut Work,
) -> Result<(), EditError> {
    certify_normal_profiles(data, profiles, None, work)
}

fn certify_height_normals(
    data: &bsp::Displacements,
    profiles: &[Vec<DVec3>],
    axis: usize,
    work: &mut Work,
) -> Result<(), EditError> {
    certify_normal_profiles(data, profiles, Some(axis), work)
}

fn certify_normal_profiles(
    data: &bsp::Displacements,
    profiles: &[Vec<DVec3>],
    axis: Option<usize>,
    work: &mut Work,
) -> Result<(), EditError> {
    certify_normal_info(data.infos[0], profiles, axis, work)
}

fn certify_normal_info(
    info: bsp::DisplacementInfo,
    profiles: &[Vec<DVec3>],
    axis: Option<usize>,
    work: &mut Work,
) -> Result<(), EditError> {
    let mut normal_inputs = BTreeSet::new();
    let cells = info
        .grid_size()
        .map_err(|error| invalid(error.to_string()))?
        - 1;
    for (rounding, points) in bsp::NativeDisplacementRounding::ALL
        .into_iter()
        .zip(profiles)
    {
        let positions = points
            .iter()
            .map(|point| point.as_vec3().to_array())
            .collect::<Vec<_>>();
        let identity = positions
            .iter()
            .map(|point| point.map(|value| if value == 0.0 { 0 } else { value.to_bits() }))
            .collect::<Vec<_>>();
        if normal_inputs.insert(identity) {
            work.consume(NORMAL_SETUP_WORK + NORMAL_CELL_WORK * cells * cells)
                .map_err(invalid)?;
            let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
            let result = if let Some(axis) = axis {
                info.certify_native_height_normal_inputs(&positions, axis, profile)
            } else {
                info.certify_native_normal_inputs(&positions, profile)
            };
            result.map_err(|error| {
                invalid(format!(
                    "Terrain native normal inputs cannot be certified: {error}"
                ))
            })?;
        }
    }
    Ok(())
}

struct NormalProjection {
    fields: [ScalarField; 2],
    axis: usize,
    sign: i128,
}

impl NormalProjection {
    fn new(
        base: [DVec3; 4],
        visible: [DVec3; 4],
        axis: usize,
        sign: i128,
    ) -> Result<Self, EditError> {
        let axes = [(axis + 1) % 3, (axis + 2) % 3];
        Ok(Self {
            fields: [
                ScalarField::new(base, visible.map(|point| point[axes[0]]))?,
                ScalarField::new(base, visible.map(|point| point[axes[1]]))?,
            ],
            axis,
            sign,
        })
    }

    fn matches(&self, other: &Self) -> Result<bool, EditError> {
        Ok(self.axis == other.axis
            && self.sign == other.sign
            && self.fields[0].matches(&other.fields[0])?
            && self.fields[1].matches(&other.fields[1])?)
    }
}

fn certify_projected_normals(
    info: bsp::DisplacementInfo,
    base: [DVec3; 4],
    profiles: &[Vec<DVec3>],
    work: &mut Work,
) -> Result<NormalProjection, EditError> {
    let size = info
        .grid_size()
        .map_err(|error| invalid(error.to_string()))?;
    let axis = BasePlane::new([base[0], base[1], base[2]])?.axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    work.consume(FIELD_WORK * (21 + profiles.len()))
        .map_err(invalid)?;
    let sign = orient(base[0], base[1], base[2], axes)?;
    let points = profiles
        .last()
        .filter(|points| points.len() == size * size)
        .ok_or_else(|| invalid("Terrain normal projection requires a complete native grid"))?;
    let corners = [0, (size - 1) * size, size * size - 1, size - 1].map(|index| points[index]);
    let projection = NormalProjection::new(base, corners, axis, sign)?;
    if sign == 0 || orient(corners[0], corners[1], corners[2], axes)? != sign {
        return Err(invalid(
            "Noncoplanar flat terrain contacts require an orientation-preserving common projection",
        ));
    }
    for points in profiles {
        if points.len() != size * size || orient(points[0], points[size], points[1], axes)? != sign
        {
            return Err(invalid(
                "Noncoplanar flat terrain native normal projections must share one positive direction",
            ));
        }
    }
    certify_normal_info(info, profiles, Some(axis), work)?;
    Ok(projection)
}

fn admit<'a>(
    document: &Document,
    solid: &'a Solid,
    geometry: &'a BrushGeometry,
    plane: Plane,
    work: &mut Work,
) -> Result<Option<Admitted<'a>>, EditError> {
    let (source, base) = read_source(solid, geometry, work)?;
    let mut crossing = false;
    for positions in source
        .positions
        .iter()
        .chain(std::iter::once(&source.mesh.positions))
    {
        let mut front = false;
        let mut back = false;
        for &point in positions {
            let value = plane.distance_to(point);
            front |= value > CUT_EPSILON;
            back |= value < -CUT_EPSILON;
        }
        crossing |= front && back;
    }
    if !crossing {
        return Ok(None);
    }
    Ok(Some(check_source(document, solid, source, base, work)?))
}

fn admit_source<'a>(
    document: &Document,
    solid: &'a Solid,
    geometry: &'a BrushGeometry,
    work: &mut Work,
) -> Result<Admitted<'a>, EditError> {
    let (source, base) = read_source(solid, geometry, work)?;
    check_source(document, solid, source, base, work)
}

fn read_source<'a>(
    solid: &'a Solid,
    geometry: &'a BrushGeometry,
    work: &mut Work,
) -> Result<(Admitted<'a>, [[f32; 3]; 4]), EditError> {
    let displaced = solid
        .sides
        .iter()
        .enumerate()
        .filter(|(_, side)| side.displacement.is_some())
        .collect::<Vec<_>>();
    let [(index, side)] = displaced.as_slice() else {
        return Err(invalid(
            "Terrain retiling requires one displaced face per brush",
        ));
    };
    let face = geometry
        .faces
        .iter()
        .find(|face| face.side == *index)
        .ok_or_else(|| invalid("Terrain has no effective base face"))?;
    let info = side.displacement.as_ref().unwrap();
    let mesh = DisplacementMesh::build(face, info).map_err(|error| invalid(error.to_string()))?;
    let (data, base) = data(face, &mesh, info.power)?;
    let positions = reconstructed(&data, base, work)?;
    Ok((
        Admitted {
            side,
            info,
            face,
            mesh,
            data,
            positions,
        },
        base,
    ))
}

fn check_source<'a>(
    document: &Document,
    solid: &Solid,
    mut source: Admitted<'a>,
    base: [[f32; 3]; 4],
    work: &mut Work,
) -> Result<Admitted<'a>, EditError> {
    let info = source.info;
    let (owner, _) = crate::subobject::solid(document, solid.id)?;
    if let ObjectId::Entity(id) = owner
        && document.entity(id).and_then(|entity| entity.classname()) != Some("func_detail")
    {
        return Err(invalid(
            "Terrain retiling requires world or func_detail brushes",
        ));
    }
    native::checked_mesh(source.face, info).map_err(invalid)?;
    crate::subobject::fragments::admit_source(solid)?;
    smoothing::angle(info).map_err(invalid)?;
    if !info.extra.is_empty()
        || info.extra_properties.iter().any(|(key, _)| {
            !["smooth", "mintess", "uaxis", "vaxis"]
                .iter()
                .any(|known| key.eq_ignore_ascii_case(known))
        })
        || info.allowed_verts.as_ref().is_some_and(|mask| {
            mask.len() != super::ALLOWED_VERTEX_MASK_WORDS || mask.iter().any(|word| *word != -1)
        })
    {
        return Err(invalid(
            "Terrain retiling cannot remap opaque payloads or restricted vertex masks",
        ));
    }
    rows(&info.offset_normals, source.mesh.size)?;
    if info
        .offset_normals
        .as_ref()
        .is_some_and(|rows| rows.iter().flatten().any(|normal| *normal != DVec3::ZERO))
    {
        return Err(invalid(
            "Terrain retiling requires absent or zero offset normals",
        ));
    }
    let tags = triangles::native_tags(info, &source.mesh)?;
    work.consume(tags.len()).map_err(invalid)?;
    native_base(&mut source.mesh, &source.data, base, work)?;
    Ok(source)
}

fn height_sheet(source: &Admitted<'_>, work: &mut Work) -> Result<HeightSheet, EditError> {
    let sheet = projected_sheet(&source.mesh, &source.positions, Phase::Original, work)?;
    let mesh = &source.mesh;
    let cells = mesh.size - 1;
    let corners = [0, cells * mesh.size, mesh.size * mesh.size - 1, cells]
        .map(|index| mesh.base_positions[index]);
    work.consume(3 * FIELD_WORK * mesh.positions.len())
        .map_err(invalid)?;
    for row in 0..mesh.size {
        for column in 0..mesh.size {
            let weights = [
                (cells - row) * (cells - column),
                row * (cells - column),
                row * column,
                (cells - row) * column,
            ]
            .map(|weight| weight as i128);
            let base = weighted(&corners, &weights, (cells * cells) as i128)?;
            if !same(base, mesh.base_positions[row * mesh.size + column]) {
                return Err(invalid(
                    "Curved terrain sheets require an exact bilinear source base grid",
                ));
            }
        }
    }
    paint::validate_data(&source.mesh, &source.data, work)?;
    certify_height_normals(&source.data, &source.positions, sheet.axis, work)?;
    Ok(sheet)
}

fn projected_sheet(
    mesh: &DisplacementMesh,
    positions: &[Vec<DVec3>],
    phase: Phase,
    work: &mut Work,
) -> Result<HeightSheet, EditError> {
    let cells = mesh.size - 1;
    let indices = [0, cells * mesh.size, mesh.size * mesh.size - 1, cells];
    let corners = indices.map(|index| mesh.base_positions[index]);
    let plane = BasePlane::new([corners[0], corners[1], corners[2]])?;
    let axis = plane.axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let sign = orient(corners[0], corners[1], corners[2], axes)?;
    if sign == 0 {
        return Err(invalid("Curved terrain-sheet base cage is collapsed"));
    }
    convex(&corners, axes, sign)?;
    for point in corners {
        stored(point)?;
    }
    work.consume(
        FIELD_WORK * (8 + 2 * mesh.positions.len() + 4 * mesh.size)
            + positions.len() * mesh.positions.len()
            + 2 * cells * cells,
    )
    .map_err(invalid)?;
    let visible = indices.map(|index| mesh.positions[index]);
    if orient(visible[0], visible[1], visible[2], axes)? != sign {
        return Err(invalid(
            "Curved terrain tangential projection must preserve its base orientation",
        ));
    }
    convex(&visible, axes, sign)?;
    for (index, &point) in mesh.positions.iter().enumerate() {
        let base = mesh.base_positions[index];
        if !plane.contains(base)?
            || positions
                .iter()
                .any(|positions| !same(positions[index], point))
        {
            return Err(invalid(
                "Curved terrain sheets require an exact base plane and matching authored, native and compiler positions",
            ));
        }
    }
    for triangle in mesh.triangles() {
        let [a, b, c] = triangle.map(|index| mesh.positions[index]);
        if orient(a, b, c, axes)? != sign {
            return Err(invalid(
                "Curved terrain tangential projection must keep every original triangle positively oriented",
            ));
        }
    }
    match phase {
        Phase::Original => polygonal_boundary(mesh, axes, sign, work)?,
        Phase::Child => straight_boundary(mesh, visible, axes)?,
    }
    Ok(HeightSheet { axis, axes, sign })
}

fn polygonal_boundary(
    mesh: &DisplacementMesh,
    axes: [usize; 2],
    sign: i128,
    work: &mut Work,
) -> Result<(), EditError> {
    if !matches!(mesh.size, 5 | 9 | 17) {
        return Err(invalid(
            "Terrain projected boundaries require power 2, 3 or 4",
        ));
    }
    let cells = mesh.size - 1;
    let count = 4 * cells;
    work.consume((FIELD_WORK + 1) * count * count + count)
        .map_err(invalid)?;
    let mut ring = Vec::with_capacity(count);
    for side in 0..4 {
        for index in neighbors::edge_indices(mesh.size, side)
            .map_err(invalid)?
            .into_iter()
            .take(cells)
        {
            ring.push(mesh.positions[index]);
        }
    }
    for first in 0..ring.len() {
        let a = ring[first];
        let b = ring[(first + 1) % ring.len()];
        if axes.iter().all(|&axis| a[axis] == b[axis]) {
            return Err(invalid(
                "Terrain projected boundary contains a collapsed edge",
            ));
        }
        for point in ring.iter().skip(first + 1) {
            if axes.iter().all(|&axis| a[axis] == point[axis]) {
                return Err(invalid(
                    "Terrain projected boundary vertices must be unique",
                ));
            }
        }
        for &point in &ring {
            let side = orient(a, b, point, axes)?;
            if side != 0 && side != sign {
                return Err(invalid(
                    "Original curved terrain requires a convex projected polygonal boundary",
                ));
            }
        }
    }
    Ok(())
}

fn straight_boundary(
    mesh: &DisplacementMesh,
    visible: [DVec3; 4],
    axes: [usize; 2],
) -> Result<(), EditError> {
    for side in 0..4 {
        let a = visible[side];
        let b = visible[(side + 1) % 4];
        let coordinate = axes
            .into_iter()
            .find(|&coordinate| a[coordinate] != b[coordinate])
            .ok_or_else(|| invalid("Curved terrain projected boundary is collapsed"))?;
        let increasing = a[coordinate] < b[coordinate];
        let mut previous = None;
        for index in neighbors::edge_indices(mesh.size, side).map_err(invalid)? {
            let point = mesh.positions[index];
            if !on_segment(point, a, b, axes)?
                || previous.is_some_and(|previous| {
                    if increasing {
                        point[coordinate] <= previous
                    } else {
                        point[coordinate] >= previous
                    }
                })
            {
                return Err(invalid(
                    "Curved terrain requires straight, strictly monotone projected boundary edges",
                ));
            }
            previous = Some(point[coordinate]);
        }
    }
    Ok(())
}

fn original_boundary_projection(
    base_positions: &[DVec3],
    positions: &[DVec3],
    size: usize,
    axis: usize,
    sign: i128,
    work: &mut Work,
) -> Result<NormalProjection, EditError> {
    work.consume(FIELD_WORK * (16 + 8 * size))
        .map_err(invalid)?;
    let cells = size - 1;
    let indices = [0, cells * size, size * size - 1, cells];
    let projection = NormalProjection::new(
        indices.map(|index| base_positions[index]),
        indices.map(|index| positions[index]),
        axis,
        sign,
    )?;
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    for side in 0..4 {
        for index in neighbors::edge_indices(size, side).map_err(invalid)? {
            for (field, coordinate) in projection.fields.iter().zip(axes) {
                if field.value(base_positions[index])? != positions[index][coordinate] {
                    return Err(invalid(
                        "Original curved terrain boundaries must retain one exact affine tangential cage mapping",
                    ));
                }
            }
        }
    }
    Ok(projection)
}

fn prepare<'a>(
    document: &Document,
    solid: &'a Solid,
    geometry: &'a BrushGeometry,
    plane: Plane,
    work: &mut Work,
) -> Result<Option<Prepared<'a>>, EditError> {
    let Some(Admitted {
        side,
        info,
        face,
        mesh,
        data,
        positions,
    }) = admit(document, solid, geometry, plane, work)?
    else {
        return Ok(None);
    };
    let indices = [
        0,
        (mesh.size - 1) * mesh.size,
        mesh.size * mesh.size - 1,
        mesh.size - 1,
    ];
    work.consume(FIELD_WORK * (2 * mesh.positions.len() + 16))
        .map_err(invalid)?;
    let corners = indices.map(|index| mesh.base_positions[index]);
    let field = PlanarField::new(corners, indices.map(|index| mesh.positions[index]))?;
    let base_axis = field.axis();
    let base_axes = [(base_axis + 1) % 3, (base_axis + 2) % 3];
    let base_sign = orient(corners[0], corners[1], corners[2], base_axes)?;
    if base_sign == 0 {
        return Err(invalid("Flat terrain cage is collapsed"));
    }
    convex(&corners, base_axes, base_sign)?;
    for point in corners {
        stored(point)?;
    }
    for (index, &point) in mesh.positions.iter().enumerate() {
        let expected = field.position(mesh.base_positions[index])?;
        if !same(field.base_position(point)?, mesh.base_positions[index])
            || !same(point, expected)
            || positions
                .iter()
                .any(|positions| !same(positions[index], expected))
        {
            return Err(invalid(
                "Flat terrain native, compiler and authored positions must agree exactly on one affine planar sheet",
            ));
        }
    }
    let paint = PaintField::new(corners, indices.map(|index| mesh.alphas[index]), work)?;
    paint.certify(&mesh, &data, work)?;
    let world_axis = field.world_axis();
    let axes = [(world_axis + 1) % 3, (world_axis + 2) % 3];
    let world = indices.map(|index| mesh.positions[index]);
    let sign = orient(world[0], world[1], world[2], axes)?;
    if sign == 0 {
        return Err(invalid("Flat terrain visible sheet is collapsed"));
    }
    for [a, b, c] in mesh.triangles() {
        if orient(
            mesh.positions[a],
            mesh.positions[b],
            mesh.positions[c],
            axes,
        )? != sign
        {
            return Err(invalid(
                "Flat terrain source triangles must have positive orientation",
            ));
        }
    }
    certify_normals(&data, &positions, work)?;
    let mut ring = [DVec3::ZERO; 4];
    for (point, base) in ring.iter_mut().zip(corners) {
        *point = field.position(base)?;
    }
    Ok(Some(Prepared {
        side,
        info,
        face,
        mesh,
        field,
        paint,
        axes,
        sign,
        ring,
        data,
    }))
}

pub(crate) fn texture_face(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    plane: Plane,
    work: &mut Work,
) -> Result<Option<FaceId>, EditError> {
    Ok(
        prepare(document, solid, geometry, plane, work)?.map(|source| FaceId {
            solid: solid.id,
            side: source.side.id,
        }),
    )
}

fn polygon(mut result: Vec<DVec3>, axes: [usize; 2]) -> Result<Vec<DVec3>, EditError> {
    loop {
        let remove = (0..result.len())
            .find(|&index| same(result[index], result[(index + 1) % result.len()]));
        if let Some(index) = remove {
            result.remove(index);
        } else {
            break;
        }
    }
    loop {
        let mut remove = None;
        for index in 0..result.len() {
            if orient(
                result[(index + result.len() - 1) % result.len()],
                result[index],
                result[(index + 1) % result.len()],
                axes,
            )? == 0
            {
                remove = Some(index);
                break;
            }
        }
        if let Some(index) = remove {
            result.remove(index);
        } else {
            break;
        }
    }
    if !(3..=5).contains(&result.len()) {
        return Err(invalid(
            "Flat terrain cut requires a convex three-to-five-corner retained polygon",
        ));
    }
    Ok(result)
}

fn split(
    source: &Prepared<'_>,
    plane: Plane,
    work: &mut Work,
) -> Result<[Vec<DVec3>; 2], EditError> {
    split_ring(&source.ring, &source.field, source.axes, plane, work)
}

fn split_ring(
    ring: &[DVec3],
    field: &PlanarField,
    axes: [usize; 2],
    plane: Plane,
    work: &mut Work,
) -> Result<[Vec<DVec3>; 2], EditError> {
    if !(3..=5).contains(&ring.len()) {
        return Err(invalid(
            "Terrain cuts require a three-to-five-corner source ring",
        ));
    }
    work.consume(3 * ring.len() * FIELD_WORK).map_err(invalid)?;
    let classify = |point| {
        let distance = plane.distance_to(point);
        if distance > CUT_EPSILON {
            1
        } else if distance < -CUT_EPSILON {
            -1
        } else {
            0
        }
    };
    let mut result: [Vec<DVec3>; 2] = std::array::from_fn(|_| Vec::new());
    let mut cut = Vec::new();
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        let first = classify(a);
        let second = classify(b);
        if first <= 0 {
            result[0].push(a);
        }
        if first >= 0 {
            result[1].push(a);
        }
        if first == 0 {
            cut.push(a);
        }
        if first * second == -1 {
            let (a, b) = if key(a) <= key(b) { (a, b) } else { (b, a) };
            let da = plane.distance_to(a);
            let db = plane.distance_to(b);
            let parameter = da / (da - db);
            if !parameter.is_finite() || !(0.0..1.0).contains(&parameter) || parameter == 0.0 {
                return Err(numeric());
            }
            let base_a = field.base_position(a)?;
            let base_b = field.base_position(b)?;
            let base = stored(native::stored_vertex(base_a.lerp(base_b, parameter)))?;
            let point = field.position(base)?;
            if !on_segment(point, a, b, axes)? || plane.distance_to(point).abs() > CUT_EPSILON {
                return Err(invalid(
                    "Flat terrain cut endpoints must survive native storage on their original edges within Source cut-plane precision",
                ));
            }
            result[0].push(point);
            result[1].push(point);
            cut.push(point);
        }
    }
    cut.sort_by_key(|&point| key(point));
    cut.dedup_by(|a, b| same(*a, *b));
    let [a, b] = cut.as_slice() else {
        return Err(invalid(
            "Flat terrain cut requires one shared segment at Source cut-plane precision",
        ));
    };
    let mut back_side = None;
    for &point in ring {
        let side = orient(*a, *b, point, axes)?;
        match classify(point) {
            0 if side != 0 => {
                return Err(invalid(
                    "Flat terrain cut cannot preserve one exact shared cut segment",
                ));
            }
            -1 if side != 0 => {
                back_side = Some(side);
            }
            -1 => {
                return Err(invalid(
                    "Flat terrain cut moved a retained source corner onto its boundary",
                ));
            }
            _ => {}
        }
    }
    let back_side = back_side.ok_or_else(numeric)?;
    for &point in ring {
        let side = orient(*a, *b, point, axes)?;
        let classification = classify(point);
        if classification < 0 && side != back_side || classification > 0 && side != -back_side {
            return Err(invalid(
                "Flat terrain stored cut segment does not preserve the requested source-side separation",
            ));
        }
    }
    let [back, front] = result;
    Ok([polygon(back, axes)?, polygon(front, axes)?])
}

fn weighted(points: &[DVec3], weights: &[i128], denominator: i128) -> Result<DVec3, EditError> {
    let mut result = DVec3::ZERO;
    for axis in 0..3 {
        let mut value = Exact::new(0, 0);
        for (&point, &weight) in points.iter().zip(weights) {
            value = value.add(Exact::float(point[axis])?.mul(Exact::new(weight, 0))?)?;
        }
        result[axis] = value.div(Exact::new(denominator, 0))?.value()?;
    }
    Ok(result)
}

fn quads(
    ring: &[DVec3],
    axes: [usize; 2],
    sign: i128,
    fan_quad: bool,
) -> Result<Vec<[DVec3; 4]>, EditError> {
    convex(ring, axes, sign)?;
    if !fan_quad && let Ok(quad) = <[DVec3; 4]>::try_from(ring) {
        return Ok(vec![quad]);
    }
    let (weights, denominator): (&[i128], i128) = match ring.len() {
        3 => (&[3, 3, 2], 8),
        4 => (&[1, 1, 1, 1], 4),
        5 => (&[2, 2, 2, 1, 1], 8),
        _ => unreachable!(),
    };
    let center = weighted(ring, weights, denominator)?;
    let mut result = Vec::new();
    for index in 0..ring.len() {
        let next = weighted(&[ring[index], ring[(index + 1) % ring.len()]], &[1, 1], 2)?;
        let previous = weighted(
            &[ring[(index + ring.len() - 1) % ring.len()], ring[index]],
            &[1, 1],
            2,
        )?;
        let quad = [ring[index], next, center, previous];
        convex(&quad, axes, sign)?;
        result.push(quad);
    }
    Ok(result)
}

fn uv(
    data: &bsp::Displacements,
    face: &Face,
    vertices: usize,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<(), EditError> {
    let base: [DVec3; 4] = face.vertices.as_slice().try_into().unwrap();
    let base = base.map(|point| native::stored_vertex(point).as_vec3().to_array());
    for rounding in bsp::NativeDisplacementRounding::ALL {
        work.consume(UV_WORK + UV_VERTEX_WORK * vertices)
            .map_err(invalid)?;
        let coordinates = data
            .native_affine_texture_coordinates(
                0,
                base,
                texture.texture_vecs,
                texture.mapping_size,
                bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding },
            )
            .map_err(|error| {
                invalid(format!(
                    "Flat terrain native affine UV cannot be certified: {error}"
                ))
            })?;
        if let Some(scale) = texture.detail_scale {
            if !scale.is_normal() || scale <= 0.0 {
                return Err(invalid(
                    "Flat terrain detail scale must be positive and normal",
                ));
            }
            work.consume(DETAIL_UV_VERTEX_WORK * coordinates.len())
                .map_err(invalid)?;
            let scale = Exact::float(f64::from(scale))?;
            for coordinate in coordinates.into_iter().flatten() {
                let product = Exact::float(f64::from(coordinate))?.mul(scale)?.value()?;
                let encoded = product as f32;
                if !encoded.is_finite() || encoded.is_subnormal() || f64::from(encoded) != product {
                    return Err(invalid(
                        "Flat terrain detail UV scaling must remain exact at every native vertex",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn carrier_base(
    solid: &Solid,
    side: &Side,
    face: &Face,
    quad: [DVec3; 4],
    material: &str,
) -> Result<(Solid, DisplacementMesh), EditError> {
    let center = weighted(&quad, &[1, 1, 1, 1], 4)?;
    let mut part = solid.clone();
    for edge in 0..4 {
        let a = quad[edge];
        let b = quad[(edge + 1) % 4];
        let normal = (b - a)
            .cross(face.plane.normal)
            .try_normalize()
            .ok_or_else(numeric)?;
        let mut plane = Plane {
            normal,
            dist: normal.dot(a),
        };
        if plane.distance_to(center) > 0.0 {
            plane.normal = -plane.normal;
            plane.dist = -plane.dist;
        }
        let shape = BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
        if shape
            .faces
            .iter()
            .flat_map(|face| &face.vertices)
            .any(|&point| plane.distance_to(point) > CARRIER_EPSILON)
        {
            part = crate::clip::half(&part, plane, material)?;
        }
    }
    let index = part
        .sides
        .iter()
        .position(|candidate| candidate.id == side.id)
        .ok_or_else(|| invalid("Terrain carrier lost its source surface"))?;
    let geometry = BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
    let face = geometry
        .faces
        .iter()
        .find(|face| face.side == index)
        .ok_or_else(numeric)?;
    if face.vertices.len() != 4
        || face.vertices.iter().any(|&point| {
            !quad
                .iter()
                .any(|&target| same(native::stored_vertex(point), target))
        })
    {
        return Err(invalid(
            "Terrain carrier cannot preserve its exact planned quad",
        ));
    }
    crate::brush::refresh_precise(&mut part, true)?;
    part.sides[index]
        .set_precise_vertices(&quad)
        .map_err(|error| invalid(error.to_string()))?;
    crate::brush::canonicalize_precise(&mut part)?;
    let mut info = side.displacement.as_ref().unwrap().clone();
    info.power = POWER;
    info.start_position = quad[0];
    info.elevation = 0.0;
    info.normals = None;
    info.distances = None;
    info.offsets = None;
    info.offset_normals = None;
    info.alphas = None;
    info.triangle_tags = None;
    let geometry = BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
    let face = geometry
        .faces
        .iter()
        .find(|face| face.side == index)
        .ok_or_else(numeric)?;
    let mesh = DisplacementMesh::build(face, &info).map_err(|error| invalid(error.to_string()))?;
    part.sides[index].displacement = Some(info);
    Ok((part, mesh))
}

fn carrier(
    solid: &Solid,
    source: &Prepared<'_>,
    quad: [DVec3; 4],
    material: &str,
    work: &mut Work,
) -> Result<Solid, EditError> {
    carrier_field(
        solid,
        source.side,
        source.face,
        quad,
        &source.field,
        &source.paint,
        material,
        TerrainBlendMode::Exact,
        work,
    )
}

#[allow(clippy::too_many_arguments)]
fn carrier_field(
    solid: &Solid,
    side: &Side,
    face: &Face,
    quad: [DVec3; 4],
    field: &PlanarField,
    paint: &PaintField,
    material: &str,
    blend_mode: TerrainBlendMode,
    work: &mut Work,
) -> Result<Solid, EditError> {
    let (mut part, mesh) = carrier_base(solid, side, face, quad, material)?;
    let source_info = side.displacement.as_ref().unwrap();
    let info = part
        .sides
        .iter_mut()
        .find(|candidate| candidate.id == side.id)
        .unwrap()
        .displacement
        .as_mut()
        .unwrap();
    work.consume(
        FIELD_WORK * mesh.base_positions.len() * (1 + usize::from(source_info.alphas.is_some())),
    )
    .map_err(invalid)?;
    info.offsets = Some(
        mesh.base_positions
            .chunks(SIZE)
            .map(|row| row.iter().map(|&base| field.offset(base)).collect())
            .collect::<Result<_, _>>()?,
    );
    info.alphas = source_info
        .alphas
        .as_ref()
        .map(|_| {
            mesh.base_positions
                .chunks(SIZE)
                .map(|row| {
                    row.iter()
                        .map(|&base| paint.sample_alpha(base, blend_mode))
                        .collect()
                })
                .collect::<Result<_, _>>()
        })
        .transpose()?;
    Ok(part)
}

fn edge(edges: &mut Edges, a: DVec3, b: DVec3) -> Result<(), EditError> {
    let (a, b) = (key(a), key(b));
    if a == b {
        return Err(invalid("Flat terrain certificate found a collapsed edge"));
    }
    if let Some(count) = edges.get_mut(&(b, a)) {
        *count -= 1;
        if *count == 0 {
            edges.remove(&(b, a));
        }
    } else {
        *edges.entry((a, b)).or_default() += 1;
    }
    Ok(())
}

struct Child {
    base: [DVec3; 4],
    positions: Vec<DVec3>,
}

fn on_segment(point: DVec3, a: DVec3, b: DVec3, axes: [usize; 2]) -> Result<bool, EditError> {
    Ok(orient(a, b, point, axes)? == 0
        && axes
            .into_iter()
            .all(|axis| point[axis] >= a[axis].min(b[axis]) && point[axis] <= a[axis].max(b[axis])))
}

fn region(
    ring: &[DVec3],
    children: &[Child],
    axes: [usize; 2],
    sign: i128,
    plane: Plane,
    field: Option<&PlanarField>,
    work: &mut Work,
) -> Result<(), EditError> {
    region_checked(ring, children, axes, sign, Some(plane), field, work)
}

fn region_unclipped(
    ring: &[DVec3],
    children: &[Child],
    axes: [usize; 2],
    sign: i128,
    work: &mut Work,
) -> Result<(), EditError> {
    region_checked(ring, children, axes, sign, None, None, work)
}

fn region_checked(
    ring: &[DVec3],
    children: &[Child],
    axes: [usize; 2],
    sign: i128,
    plane: Option<Plane>,
    field: Option<&PlanarField>,
    work: &mut Work,
) -> Result<(), EditError> {
    let indices = bsp::DisplacementInfo {
        power: POWER as i32,
        ..Default::default()
    }
    .triangle_indices()
    .map_err(|error| invalid(error.to_string()))?;
    let mut actual = Edges::new();
    let mut boundary = Edges::new();
    for child in children {
        work.consume(indices.len() + child.positions.len() * (ring.len() + FIELD_WORK))
            .map_err(invalid)?;
        for &point in &child.positions {
            if plane.is_some_and(|plane| plane.distance_to(point) > CUT_EPSILON) {
                return Err(invalid(
                    "Flat terrain native triangles cross the retained cut-plane precision",
                ));
            }
            if let Some(field) = field {
                let _ = field.base_position(point)?;
            }
            for index in 0..ring.len() {
                let value = orient(ring[index], ring[(index + 1) % ring.len()], point, axes)?;
                if value != 0 && value != sign {
                    return Err(invalid(
                        "Flat terrain triangle lies outside its retained polygon",
                    ));
                }
            }
        }
        for triangle in &indices {
            let points = triangle.map(|index| child.positions[index]);
            if orient(points[0], points[1], points[2], axes)? != sign {
                return Err(invalid(
                    "Flat terrain native triangles must have positive orientation",
                ));
            }
            for index in 0..3 {
                edge(&mut actual, points[index], points[(index + 1) % 3])?;
            }
        }
        for side in 0..4 {
            let positions = neighbors::edge_indices(SIZE, side).map_err(invalid)?;
            let points = positions
                .iter()
                .map(|&index| child.positions[index])
                .collect::<Vec<_>>();
            let mut boundary_edge = None;
            for index in 0..ring.len() {
                let a = ring[index];
                let b = ring[(index + 1) % ring.len()];
                if on_segment(points[0], a, b, axes)? && on_segment(points[SIZE - 1], a, b, axes)? {
                    boundary_edge = Some([a, b]);
                    break;
                }
            }
            let Some([a, b]) = boundary_edge else {
                continue;
            };
            for &point in &points {
                if !on_segment(point, a, b, axes)? {
                    return Err(invalid(
                        "Flat terrain retained boundary does not lie on its exact polygon edges",
                    ));
                }
            }
            for pair in points.windows(2) {
                edge(&mut boundary, pair[0], pair[1])?;
            }
        }
    }
    if actual != boundary || boundary.values().any(|count| *count != 1) {
        return Err(invalid(
            "Flat terrain triangles do not have the exact oriented retained boundary and internal seams",
        ));
    }
    Ok(())
}

fn connections(children: &[Child], work: &mut Work) -> Result<(), EditError> {
    let grids = children
        .iter()
        .map(|child| neighbors::SurfaceGrid {
            power: POWER,
            corners: child.base,
            positions: &child.positions,
        })
        .collect::<Vec<_>>();
    let found = neighbors::find(&grids, &mut |amount| work.consume(amount)).map_err(invalid)?;
    if found.ambiguous != 0 || found.mismatched != 0 || found.unequal_power != 0 {
        return Err(invalid(
            "Flat terrain output has ambiguous or mismatched native full edges",
        ));
    }
    for pair in &found.pairs {
        for (a, b) in
            neighbors::common_edge_indices(POWER, pair.first_edge, POWER, pair.second_edge)
                .map_err(invalid)?
        {
            if !same(
                children[pair.first].positions[a],
                children[pair.second].positions[b],
            ) {
                return Err(invalid("Flat terrain native shared-edge samples differ"));
            }
        }
    }
    let _ = neighbors::native_connections(&grids, &found.pairs, &mut |amount| work.consume(amount))
        .map_err(invalid)?;
    let (lod, _) = super::lod::prepare(&grids, &found.pairs, &mut |amount| work.consume(amount))
        .map_err(invalid)?;
    if lod.iter().any(Option::is_some) {
        return Err(invalid(
            "Flat terrain output cannot require native LOD coarsening",
        ));
    }
    Ok(())
}

pub(crate) fn plan(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    request: Request<'_>,
    context: &mut Context<'_>,
) -> Result<Option<TerrainPlan>, EditError> {
    let work = &mut context.work;
    let Some(source) = prepare(document, solid, geometry, request.plane, work)? else {
        return Ok(None);
    };
    let identity = FaceId {
        solid: solid.id,
        side: source.side.id,
    };
    let texture = request.textures.and_then(|textures| textures.get(&identity)).copied()
        .ok_or_else(|| invalid("Flat terrain clipping requires guarded native texture projection and mapping dimensions"))?;
    let labels = TriangleField::new(
        source.info,
        &source.mesh,
        texture.has_secondary_surface,
        work,
    )?;
    uv(
        &source.data,
        source.face,
        source.mesh.positions.len(),
        texture,
        work,
    )?;
    let mut parts = Vec::new();
    let mut children = Vec::new();
    let polygons = split(&source, request.plane, work)?;
    let fan_quad = request.keep == Keep::Both
        && matches!([polygons[0].len(), polygons[1].len()], [3, 4] | [4, 3]);
    for back in [true, false] {
        if matches!(
            (request.keep, back),
            (Keep::Back, false) | (Keep::Front, true)
        ) {
            continue;
        }
        let plane = if back {
            request.plane
        } else {
            Plane {
                normal: -request.plane.normal,
                dist: -request.plane.dist,
            }
        };
        let ring = &polygons[usize::from(!back)];
        let first = children.len();
        for quad in quads(ring, source.axes, source.sign, fan_quad)? {
            work.consume(FIELD_WORK * quad.len()).map_err(invalid)?;
            let mut base = [DVec3::ZERO; 4];
            for (output, point) in base.iter_mut().zip(quad) {
                *output = stored(source.field.base_position(point)?)?;
            }
            let mut part = carrier(solid, &source, base, request.material, work)?;
            let shape =
                BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
            let side_index = part
                .sides
                .iter()
                .position(|side| side.id == source.side.id)
                .unwrap();
            let face = shape
                .faces
                .iter()
                .find(|face| face.side == side_index)
                .ok_or_else(numeric)?;
            let info = part.sides[side_index].displacement.as_ref().unwrap();
            let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
            let corners = [0, (SIZE - 1) * SIZE, VERTICES - 1, SIZE - 1]
                .map(|index| mesh.base_positions[index]);
            if corners.iter().zip(base).any(|(&a, b)| !same(a, b)) {
                return Err(invalid(
                    "Flat terrain child orientation differs from its planned quad",
                ));
            }
            let (data, stored_base) = data(face, &mesh, POWER)?;
            let positions = reconstructed(&data, stored_base, work)?;
            work.consume(2 * FIELD_WORK * mesh.positions.len())
                .map_err(invalid)?;
            for (index, &point) in mesh.positions.iter().enumerate() {
                if !same(
                    source.field.base_position(point)?,
                    mesh.base_positions[index],
                ) || !same(point, source.field.position(mesh.base_positions[index])?)
                    || positions
                        .iter()
                        .any(|positions| !same(positions[index], point))
                {
                    return Err(invalid(
                        "Flat terrain child native, compiler and analytic positions differ",
                    ));
                }
            }
            source.paint.certify(&mesh, &data, work)?;
            let mapped = labels.map(&mesh, work)?;
            let info = part.sides[side_index].displacement.as_mut().unwrap();
            info.triangle_tags = mapped.tags.clone();
            mapped.verify(info, &mesh, work)?;
            certify_normals(&data, &positions, work)?;
            uv(&data, face, mesh.positions.len(), texture, work)?;
            children.push(Child {
                base,
                positions: mesh.positions,
            });
            parts.push(part);
        }
        region(
            ring,
            &children[first..],
            source.axes,
            source.sign,
            plane,
            Some(&source.field),
            work,
        )?;
    }
    connections(&children, work)?;
    Ok(Some(TerrainPlan {
        vertices: parts.len() * VERTICES,
        triangles: parts.len() * TRIANGLES,
        parts,
        source: identity,
        source_power: source.info.power,
        output_power: Some(POWER),
        children: None,
        blend_error: None,
    }))
}
