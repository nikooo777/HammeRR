use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::vmf::{Side, Solid, UvAxis, Vmf};

use crate::edit::{DEFAULT_LIGHTMAP_SCALE, DEFAULT_TEXTURE_SCALE, world_texture_axes};
use crate::plane::ON_EPSILON;
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction};

const PLANE_POINT_SPAN: f64 = 64.0;
const MAX_COORDINATE: f64 = 65_536.0;
const MIN_VOLUME: f64 = 1e-6;
const NATIVE_ALPHA_QUANTIZATION_SPAN: f64 = 1.0 + 1.0 / 65_536.0;

pub const MAX_TERRAIN_SOURCES: usize = 16;
pub const MAX_TERRAIN_PATCHES: usize = 128;
pub const MAX_TERRAIN_VERTICES: usize = 5_184;
pub const MAX_TERRAIN_TRIANGLES: usize = 8_192;
pub const MAX_TERRAIN_NEIGHBOR_FACES: usize = crate::displacement::grid_clip::MAX_NEIGHBORS;

#[derive(Debug)]
pub(crate) enum HalfError {
    SmallVolume(u32),
    Invalid(EditError),
}

impl HalfError {
    pub(crate) fn into_edit_error(self) -> EditError {
        match self {
            Self::SmallVolume(id) => invalid(format!(
                "Brush {id} has empty, unbounded or invalid geometry"
            )),
            Self::Invalid(error) => error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Front,
    Back,
    Both,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerrainMode {
    #[default]
    Grid,
    Flat,
    Cells,
    Fragments,
}

impl TerrainMode {
    pub fn allows_neighbor_partition(self) -> bool {
        matches!(self, Self::Grid | Self::Cells | Self::Fragments)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerrainBlendMode {
    #[default]
    Exact,
    NativeFloat,
    NearestByte,
}

impl TerrainBlendMode {
    pub fn sample_alpha(self, value: f64) -> Result<f64, EditError> {
        if !value.is_finite() || !(0.0..=255.0).contains(&value) {
            return Err(invalid(format!(
                "Terrain paint sample {value} must remain between 0 and 255"
            )));
        }
        let sampled = match self {
            Self::Exact => {
                if f64::from(value as f32) != value {
                    return Err(invalid(format!(
                        "Terrain paint sample {value} must be an exact native float alpha from 0 to 255"
                    )));
                }
                value
            }
            Self::NativeFloat => f64::from(value as f32),
            Self::NearestByte => value.round_ties_even(),
        };
        Ok(sampled)
    }

    pub fn error_limits(self) -> (f64, f64) {
        match self {
            Self::Exact => (0.0, 0.0),
            Self::NativeFloat => {
                let raw = 1.0 / 131_072.0;
                (raw, NATIVE_ALPHA_QUANTIZATION_SPAN + raw)
            }
            Self::NearestByte => (0.5, 1.5),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TerrainBlendError {
    pub raw: f64,
    pub packed: [f64; 4],
}

impl TerrainBlendError {
    fn validate_bounds(&self) -> Result<(), EditError> {
        if !self.raw.is_finite()
            || self.raw < 0.0
            || self
                .packed
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(invalid(
                "Terrain blend error bounds must be finite and nonnegative",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate(&self, mode: TerrainBlendMode) -> Result<(), EditError> {
        self.validate_bounds()?;
        let (raw_limit, packed_limit) = mode.error_limits();
        if self.raw > raw_limit || self.packed.iter().any(|&value| value > packed_limit) {
            return Err(invalid(format!(
                "Invalid {mode:?} terrain blend error: raw {} (limit {}), packed {:?} (limit {} per profile)",
                self.raw, raw_limit, self.packed, packed_limit,
            )));
        }
        Ok(())
    }

    pub(crate) fn merge(&mut self, other: Self) -> Result<(), EditError> {
        self.validate_bounds()?;
        other.validate_bounds()?;
        self.raw = self.raw.max(other.raw);
        for (value, other) in self.packed.iter_mut().zip(other.packed) {
            *value = value.max(other);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainTexture {
    pub texture_vecs: [[f32; 4]; 2],
    pub mapping_size: [u32; 2],
    pub detail_scale: Option<f32>,
    pub has_secondary_surface: bool,
}

#[derive(Default)]
pub struct ClipOptions<'a> {
    pub terrain_mode: TerrainMode,
    pub blend_mode: TerrainBlendMode,
    pub partition_neighbors: bool,
    pub textures: Option<&'a BTreeMap<FaceId, TerrainTexture>>,
    pub terrain_context: Option<&'a Vmf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub source: u32,
    pub solids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipResult {
    pub cuts: Vec<Cut>,
    pub objects: Vec<ObjectId>,
    pub unchanged: Vec<u32>,
    pub side_mapping: BTreeMap<u32, Vec<u32>>,
    pub terrain: Vec<TerrainCut>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainCut {
    pub partitioned: bool,
    pub source: FaceId,
    pub faces: Vec<FaceId>,
    pub source_power: u32,
    pub output_power: Option<u32>,
    pub face_powers: Vec<u32>,
    pub vertices: usize,
    pub triangles: usize,
    pub verified_neighbors: usize,
    pub preserved_links: usize,
    pub blend_error: Option<TerrainBlendError>,
}

pub struct Source<'a> {
    pub solid: &'a Solid,
    pub owner: ObjectId,
    pub target: ObjectId,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn validate_terrain_context(context: Option<&Vmf>) -> Result<(), EditError> {
    if let Some(context) = context {
        context
            .validate_ids()
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(())
}

pub(crate) fn source_error(id: u32, error: EditError) -> EditError {
    match error {
        EditError::Invalid(message) => invalid(format!("solid:{id}: {message}")),
        error => error,
    }
}

pub fn sources<'a>(
    document: &'a Document,
    ids: &[ObjectId],
) -> std::result::Result<Vec<Source<'a>>, EditError> {
    if ids.is_empty() {
        return Err(invalid("Select at least one brush or brush entity to clip"));
    }
    let targets = ids.iter().copied().collect::<BTreeSet<_>>();
    let mut sources = Vec::new();
    let mut entity_solids = BTreeSet::new();
    for &target in &targets {
        match target {
            ObjectId::Solid(id) => {
                let (owner, solid) = crate::subobject::solid(document, id)?;
                if matches!(owner, ObjectId::Entity(_)) && targets.contains(&owner) {
                    continue;
                }
                sources.push(Source {
                    solid,
                    owner,
                    target,
                });
            }
            ObjectId::Entity(id) => {
                let entity = document.entity(id).ok_or(EditError::NotFound(target))?;
                if entity.solids.is_empty() {
                    return Err(invalid(format!(
                        "{target} is a point entity; select brushes to clip"
                    )));
                }
                for solid in &entity.solids {
                    entity_solids.insert(solid.id);
                    sources.push(Source {
                        solid,
                        owner: target,
                        target,
                    });
                }
            }
        }
    }
    if !entity_solids.is_empty() {
        let mut seen = BTreeSet::new();
        for entity in &document.vmf().entities {
            if targets.contains(&ObjectId::Entity(entity.id)) && !seen.insert(entity.id) {
                return Err(invalid(format!(
                    "Entity {} has an ambiguous duplicate ID",
                    entity.id
                )));
            }
        }
        seen.clear();
        for solid in document.vmf().solids() {
            if entity_solids.contains(&solid.id) && !seen.insert(solid.id) {
                return Err(invalid(format!(
                    "Brush {} has an ambiguous duplicate ID",
                    solid.id
                )));
            }
        }
    }
    Ok(sources)
}

pub fn normalized_plane(plane: Plane) -> std::result::Result<Plane, EditError> {
    let length = plane.normal.length();
    if !plane.normal.is_finite() || !plane.dist.is_finite() || !length.is_finite() || length < 1e-8
    {
        return Err(invalid(
            "Cut plane needs a finite, nonzero normal and finite distance",
        ));
    }
    let length = if (length - 1.0).abs() < 1e-12 {
        1.0
    } else {
        length
    };
    let plane = Plane {
        normal: plane.normal / length,
        dist: plane.dist / length,
    };
    if !plane.dist.is_finite() || plane.dist.abs() > MAX_COORDINATE {
        return Err(invalid(
            "Cut plane is outside the supported coordinate range",
        ));
    }
    Ok(plane)
}

pub fn plane_points(plane: Plane) -> [DVec3; 3] {
    let center = plane.normal * plane.dist;
    let u = plane.normal.any_orthonormal_vector() * PLANE_POINT_SPAN;
    let v = plane.normal.cross(u);
    [center + u, center, center + v]
}

fn geometry(solid: &Solid) -> std::result::Result<BrushGeometry, EditError> {
    checked_geometry(solid).map_err(HalfError::into_edit_error)
}

fn checked_geometry(solid: &Solid) -> std::result::Result<BrushGeometry, HalfError> {
    if solid
        .sides
        .iter()
        .any(|side| side.plane.0.iter().any(|point| !point.is_finite()))
    {
        return Err(HalfError::Invalid(invalid(format!(
            "Brush {} has nonfinite plane points",
            solid.id
        ))));
    }
    let geometry = BrushGeometry::from_solid(solid)
        .map_err(|error| HalfError::Invalid(invalid(format!("Brush {}: {error}", solid.id))))?;
    let center = geometry.center();
    let volume: f64 = geometry
        .faces
        .iter()
        .map(|face| {
            let a = face.vertices[0] - center;
            face.vertices[1..]
                .windows(2)
                .map(|pair| a.dot((pair[0] - center).cross(pair[1] - center)) / 6.0)
                .sum::<f64>()
        })
        .sum();
    if !volume.is_finite()
        || geometry
            .faces
            .iter()
            .flat_map(|face| &face.vertices)
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
    {
        return Err(HalfError::Invalid(invalid(format!(
            "Brush {} has empty, unbounded or invalid geometry",
            solid.id
        ))));
    }
    if volume.abs() <= MIN_VOLUME {
        return Err(HalfError::SmallVolume(solid.id));
    }
    Ok(geometry)
}

pub(crate) fn half(
    solid: &Solid,
    plane: Plane,
    material: &str,
) -> std::result::Result<Solid, EditError> {
    checked_half(solid, plane, material).map_err(HalfError::into_edit_error)
}

pub(crate) fn checked_half(
    solid: &Solid,
    plane: Plane,
    material: &str,
) -> std::result::Result<Solid, HalfError> {
    let mut part = solid.clone();
    let precise = crate::brush::prepare_precise_edit(&mut part).map_err(HalfError::Invalid)?;
    let (u, v) = world_texture_axes(plane.normal);
    part.sides.push(Side {
        id: 0,
        plane: source_formats::vmf::Plane(plane_points(plane)),
        material: material.to_ascii_uppercase(),
        u_axis: UvAxis {
            axis: u,
            offset: 0.0,
            scale: DEFAULT_TEXTURE_SCALE,
        },
        v_axis: UvAxis {
            axis: v,
            offset: 0.0,
            scale: DEFAULT_TEXTURE_SCALE,
        },
        rotation: 0.0,
        lightmap_scale: DEFAULT_LIGHTMAP_SCALE,
        smoothing_groups: Some(0),
        displacement: None,
        extra_properties: Default::default(),
        extra: Vec::new(),
    });
    let shape = checked_geometry(&part)?;
    let active: BTreeSet<_> = shape.faces.iter().map(|face| face.side).collect();
    part.sides = part
        .sides
        .into_iter()
        .enumerate()
        .filter_map(|(index, side)| active.contains(&index).then_some(side))
        .collect();
    if part.sides.last().is_none_or(|side| {
        side.id != 0 || side.plane != source_formats::vmf::Plane(plane_points(plane))
    }) {
        return Err(HalfError::Invalid(invalid(
            "Cut would produce a degenerate cap face",
        )));
    }
    checked_geometry(&part)?;
    crate::brush::refresh_precise(&mut part, precise).map_err(HalfError::Invalid)?;
    Ok(part)
}

struct PlannedCut {
    owner: ObjectId,
    target: ObjectId,
    source: u32,
    parts: Vec<Solid>,
    source_sides: Vec<u32>,
    terrain: Option<TerrainCut>,
    children: Option<Vec<crate::displacement::grid_clip::Child>>,
    window_fields: Option<crate::displacement::flat_clip::WindowFields>,
}

#[derive(Default)]
struct TerrainBudget {
    sources: usize,
    patches: usize,
    visible_sources: usize,
    visible_patches: usize,
    vertices: usize,
    triangles: usize,
}

impl TerrainBudget {
    fn ensure_capacity(
        &self,
        sources: usize,
        patches: usize,
        vertices: usize,
        triangles: usize,
    ) -> Result<(), EditError> {
        for (current, added, limit) in [
            (self.sources, sources, MAX_TERRAIN_SOURCES),
            (self.patches, patches, MAX_TERRAIN_PATCHES),
            (self.vertices, vertices, MAX_TERRAIN_VERTICES),
            (self.triangles, triangles, MAX_TERRAIN_TRIANGLES),
        ] {
            if current.checked_add(added).is_none_or(|count| count > limit) {
                return Err(invalid(
                    "Terrain clipping exceeds its source, patch, vertex or triangle limit",
                ));
            }
        }
        Ok(())
    }

    fn add(
        &mut self,
        document: &Document,
        context: Option<&Vmf>,
        solid: &Solid,
        plan: &TerrainPlan,
    ) -> Result<(), EditError> {
        self.ensure_capacity(1, plan.parts.len(), plan.vertices, plan.triangles)?;
        let visible =
            crate::displacement::native::terrain_source_is_visible(document.vmf(), solid.id);
        if visible && let Some(context) = context {
            let matches = crate::displacement::native::visible_terrain_sources(context)
                .filter(|(candidate, _)| candidate.id == solid.id)
                .collect::<Vec<_>>();
            if !matches!(matches.as_slice(), [(_, true)]) {
                return Err(source_error(
                    solid.id,
                    invalid(
                        "Resolved terrain context lost a visible selected terrain source or supported owner",
                    ),
                ));
            }
        }
        self.sources += 1;
        self.patches += plan.parts.len();
        if visible {
            self.visible_sources += solid
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
                .count();
            self.visible_patches += plan
                .parts
                .iter()
                .filter(|part| !part.hidden)
                .flat_map(|part| &part.sides)
                .filter(|side| side.displacement.is_some())
                .count();
        }
        self.vertices += plan.vertices;
        self.triangles += plan.triangles;
        Ok(())
    }
}

impl PlannedCut {
    fn terrain(source: &Source<'_>, plan: TerrainPlan, partitioned: bool) -> Self {
        let Source {
            solid,
            owner,
            target,
        } = *source;
        Self {
            owner,
            target,
            source: solid.id,
            parts: plan.parts,
            source_sides: solid.sides.iter().map(|side| side.id).collect(),
            children: plan.children,
            window_fields: plan.window_fields,
            terrain: Some(TerrainCut {
                partitioned,
                source: plan.source,
                faces: Vec::new(),
                source_power: plan.source_power,
                output_power: plan.output_power,
                face_powers: Vec::new(),
                vertices: plan.vertices,
                triangles: plan.triangles,
                verified_neighbors: 0,
                preserved_links: 0,
                blend_error: plan.blend_error,
            }),
        }
    }
}

fn partition_source<'a, 's>(
    sources: &'s [Source<'a>],
    face: FaceId,
) -> Result<(&'s Source<'a>, u32), EditError> {
    let source = sources
        .iter()
        .find(|source| source.solid.id == face.solid)
        .ok_or_else(|| invalid("Terrain neighbor partitioning lost its selected source"))?;
    let power = source
        .solid
        .sides
        .iter()
        .find(|side| side.id == face.side)
        .and_then(|side| side.displacement.as_ref())
        .ok_or_else(|| invalid("Required terrain neighbor lost its source face"))?
        .power;
    Ok((source, power))
}

fn grid_replacements(
    planned: &[PlannedCut],
) -> Vec<crate::displacement::grid_clip::Replacement<'_>> {
    planned
        .iter()
        .filter_map(|cut| {
            cut.terrain
                .as_ref()
                .zip(cut.children.as_ref())
                .map(
                    |(terrain, children)| crate::displacement::grid_clip::Replacement {
                        operation: if terrain.partitioned {
                            crate::displacement::grid_clip::Operation::Partition
                        } else {
                            crate::displacement::grid_clip::Operation::Cut
                        },
                        source: terrain.source,
                        source_power: terrain.source_power,
                        parts: &cut.parts,
                        children,
                    },
                )
        })
        .collect()
}

pub(crate) struct TerrainPlan {
    pub parts: Vec<Solid>,
    pub source: FaceId,
    pub source_power: u32,
    pub output_power: Option<u32>,
    pub vertices: usize,
    pub triangles: usize,
    pub children: Option<Vec<crate::displacement::grid_clip::Child>>,
    pub window_fields: Option<crate::displacement::flat_clip::WindowFields>,
    pub blend_error: Option<TerrainBlendError>,
}

impl TerrainPlan {
    fn validate_blend(&self, mode: TerrainBlendMode) -> Result<(), EditError> {
        match (mode, self.blend_error) {
            (TerrainBlendMode::Exact, None) => Ok(()),
            (mode, Some(error)) if mode != TerrainBlendMode::Exact => error.validate(mode),
            _ => Err(invalid(
                "Terrain blend error certificate is missing or unexpected",
            )),
        }
    }
}

impl From<crate::displacement::grid_clip::Plan> for TerrainPlan {
    fn from(plan: crate::displacement::grid_clip::Plan) -> Self {
        Self {
            parts: plan.parts,
            source: plan.source,
            source_power: plan.source_power,
            output_power: plan.output_power,
            vertices: plan.vertices,
            triangles: plan.triangles,
            children: Some(plan.children),
            window_fields: None,
            blend_error: None,
        }
    }
}

pub fn terrain_texture_faces(
    document: &Document,
    ids: &[ObjectId],
    plane: Plane,
    mode: TerrainMode,
) -> Result<Vec<FaceId>, EditError> {
    let plane = normalized_plane(plane)?;
    let inspect = match mode {
        TerrainMode::Grid => return Ok(Vec::new()),
        TerrainMode::Flat => crate::displacement::flat_clip::texture_face,
        TerrainMode::Cells => crate::displacement::flat_clip::cells::texture_face,
        TerrainMode::Fragments => crate::displacement::flat_clip::fragments::texture_face,
    };
    let mut work = crate::displacement::grid_clip::Work::default();
    let mut result = Vec::new();
    for source in sources(document, ids)? {
        if source
            .solid
            .sides
            .iter()
            .any(|side| side.displacement.is_some())
        {
            let shape =
                geometry(source.solid).map_err(|error| source_error(source.solid.id, error))?;
            if let Some(face) = inspect(document, source.solid, &shape, plane, &mut work)
                .map_err(|error| source_error(source.solid.id, error))?
            {
                result.push(face);
            }
        }
    }
    if result.len() > MAX_TERRAIN_SOURCES {
        return Err(invalid("Terrain clipping exceeds its source limit"));
    }
    Ok(result)
}

pub fn terrain_partition_faces(
    document: &Document,
    ids: &[ObjectId],
    plane: Plane,
    keep: Keep,
    mode: TerrainMode,
    context: Option<&Vmf>,
) -> Result<Vec<FaceId>, EditError> {
    if mode == TerrainMode::Grid {
        return Ok(Vec::new());
    }
    if !mode.allows_neighbor_partition() {
        return Err(invalid(
            "Partition selected neighbors is unavailable for Retile flat terrain",
        ));
    }
    let plane = normalized_plane(plane)?;
    validate_terrain_context(context)?;
    let crossing = terrain_texture_faces(document, ids, plane, mode)?;
    if crossing.is_empty() {
        return Ok(Vec::new());
    }
    let authorized = sources(document, ids)?;
    let mut context = crate::displacement::grid_clip::Context::new(context);
    crate::displacement::flat_clip::network::required_partitions(
        document,
        &authorized,
        &crossing,
        plane,
        keep,
        &mut context,
    )
}

pub fn requires_terrain_context(
    document: &Document,
    ids: &[ObjectId],
    plane: Plane,
) -> Result<bool, EditError> {
    if !document.vmf().entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
    }) {
        return Ok(false);
    }
    let plane = normalized_plane(plane)?;
    for source in sources(document, ids)? {
        if !crate::displacement::native::terrain_source_is_visible(document.vmf(), source.solid.id)
        {
            continue;
        }
        if !source
            .solid
            .sides
            .iter()
            .any(|side| side.displacement.is_some())
        {
            continue;
        }
        let geometry =
            geometry(source.solid).map_err(|error| source_error(source.solid.id, error))?;
        for (index, side) in source.solid.sides.iter().enumerate() {
            let Some(info) = &side.displacement else {
                continue;
            };
            let face = geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .ok_or_else(|| {
                    source_error(
                        source.solid.id,
                        invalid("Terrain has no effective base face"),
                    )
                })?;
            let mesh = crate::DisplacementMesh::build(face, info)
                .map_err(|error| source_error(source.solid.id, invalid(error.to_string())))?;
            if crate::displacement::grid_clip::crosses(&mesh, plane) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn clip(
    tx: &mut Transaction,
    ids: &[ObjectId],
    plane: Plane,
    keep: Keep,
    material: Option<&str>,
) -> std::result::Result<ClipResult, EditError> {
    clip_with_options(tx, ids, plane, keep, material, ClipOptions::default())
}

pub fn clip_with_terrain_context(
    tx: &mut Transaction,
    ids: &[ObjectId],
    plane: Plane,
    keep: Keep,
    material: Option<&str>,
    context: &Vmf,
) -> std::result::Result<ClipResult, EditError> {
    clip_with_options(
        tx,
        ids,
        plane,
        keep,
        material,
        ClipOptions {
            terrain_context: Some(context),
            ..Default::default()
        },
    )
}

pub fn clip_with_options(
    tx: &mut Transaction,
    ids: &[ObjectId],
    plane: Plane,
    keep: Keep,
    material: Option<&str>,
    options: ClipOptions<'_>,
) -> std::result::Result<ClipResult, EditError> {
    if options.partition_neighbors && !options.terrain_mode.allows_neighbor_partition() {
        return Err(invalid(
            "Partition selected neighbors is unavailable for Retile flat terrain",
        ));
    }
    if options.blend_mode != TerrainBlendMode::Exact
        && options.terrain_mode != TerrainMode::Fragments
    {
        return Err(invalid(
            "Native float and nearest-byte blend samples require Retile surface triangles",
        ));
    }
    let context = options.terrain_context;
    validate_terrain_context(context)?;
    let plane = normalized_plane(plane)?;
    if ids.is_empty() {
        return Err(invalid("Select at least one brush or brush entity to clip"));
    }
    if material.is_some_and(|name| name.trim().is_empty()) {
        return Err(invalid("Cut-face material must not be empty"));
    }
    let mut planned = Vec::new();
    let mut unchanged = Vec::new();
    let mut terrain_budget = TerrainBudget::default();
    let mut terrain_context = crate::displacement::grid_clip::Context::new(context);
    let sources = sources(tx.document(), ids)?;
    for source in &sources {
        let Source {
            solid,
            owner,
            target,
        } = *source;
        let shape = geometry(solid).map_err(|error| source_error(solid.id, error))?;
        let material = material.map(str::trim).unwrap_or(&solid.sides[0].material);
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            let plan = match options.terrain_mode {
                TerrainMode::Grid => crate::displacement::grid_clip::plan(
                    tx.document(),
                    solid,
                    &shape,
                    plane,
                    keep,
                    material,
                    &mut terrain_context,
                )
                .map_err(|error| source_error(solid.id, error))?
                .map(TerrainPlan::from),
                TerrainMode::Flat | TerrainMode::Cells | TerrainMode::Fragments => {
                    let plan = if options.terrain_mode == TerrainMode::Cells {
                        crate::displacement::flat_clip::cells::plan
                    } else if options.terrain_mode == TerrainMode::Fragments {
                        crate::displacement::flat_clip::fragments::plan
                    } else {
                        crate::displacement::flat_clip::plan
                    };
                    plan(
                        tx.document(),
                        solid,
                        &shape,
                        crate::displacement::flat_clip::Request {
                            plane,
                            keep,
                            material,
                            textures: options.textures,
                            blend_mode: options.blend_mode,
                        },
                        &mut terrain_context,
                    )
                    .map_err(|error| source_error(solid.id, error))?
                }
            };
            let Some(plan) = plan else {
                unchanged.push(solid.id);
                continue;
            };
            plan.validate_blend(options.blend_mode)?;
            terrain_budget.add(tx.document(), context, solid, &plan)?;
            planned.push(PlannedCut::terrain(source, plan, false));
            continue;
        }
        let mut front = false;
        let mut back = false;
        for point in shape.faces.iter().flat_map(|face| &face.vertices) {
            let distance = plane.distance_to(*point);
            front |= distance > ON_EPSILON;
            back |= distance < -ON_EPSILON;
        }
        if !front || !back {
            unchanged.push(solid.id);
            continue;
        }
        let mut parts = Vec::new();
        if matches!(keep, Keep::Back | Keep::Both) {
            parts.push(half(solid, plane, material)?);
        }
        if matches!(keep, Keep::Front | Keep::Both) {
            parts.push(half(
                solid,
                Plane {
                    normal: -plane.normal,
                    dist: -plane.dist,
                },
                material,
            )?);
        }
        planned.push(PlannedCut {
            owner,
            target,
            source: solid.id,
            parts,
            source_sides: solid.sides.iter().map(|side| side.id).collect(),
            terrain: None,
            children: None,
            window_fields: None,
        });
    }
    if options.partition_neighbors && terrain_budget.sources != 0 {
        let partitions = if options.terrain_mode == TerrainMode::Grid {
            let replacements = grid_replacements(&planned);
            crate::displacement::grid_clip::partition_neighbors(
                tx.document(),
                &sources,
                &replacements,
                plane,
                material,
                &mut terrain_context,
            )?
            .into_iter()
            .map(|plan| {
                if plan.operation != crate::displacement::grid_clip::Operation::Partition {
                    return Err(invalid(
                        "Terrain neighbor partitioning returned an unexpected operation",
                    ));
                }
                Ok(TerrainPlan::from(plan))
            })
            .collect::<Result<Vec<_>, _>>()?
        } else {
            let crossing = planned
                .iter()
                .filter_map(|cut| cut.terrain.as_ref().map(|terrain| terrain.source))
                .collect::<Vec<_>>();
            let required = crate::displacement::flat_clip::network::required_partitions(
                tx.document(),
                &sources,
                &crossing,
                plane,
                keep,
                &mut terrain_context,
            )?;
            if options.terrain_mode == TerrainMode::Cells {
                terrain_budget.ensure_capacity(required.len(), 0, 0, 0)?;
            } else {
                let mut totals = [0usize; 3];
                for face in &required {
                    let (_, power) = partition_source(&sources, *face)?;
                    let (patches, vertices, triangles) =
                        crate::displacement::flat_clip::partition_counts(
                            power,
                            options.terrain_mode,
                        )
                        .map_err(|error| source_error(face.solid, error))?;
                    for (total, added) in totals.iter_mut().zip([patches, vertices, triangles]) {
                        *total = total.checked_add(added).ok_or_else(|| {
                            invalid("Terrain neighbor output accounting overflow")
                        })?;
                    }
                }
                terrain_budget.ensure_capacity(required.len(), totals[0], totals[1], totals[2])?;
            }
            let partition = if options.terrain_mode == TerrainMode::Cells {
                crate::displacement::flat_clip::cells::partition
            } else {
                crate::displacement::flat_clip::fragments::partition
            };
            required
                .into_iter()
                .map(|face| {
                    let (source, _) = partition_source(&sources, face)?;
                    let shape =
                        geometry(source.solid).map_err(|error| source_error(face.solid, error))?;
                    let plan = partition(
                        tx.document(),
                        source.solid,
                        &shape,
                        crate::displacement::flat_clip::PartitionRequest {
                            material: material
                                .map(str::trim)
                                .unwrap_or(&source.solid.sides[0].material),
                            textures: options.textures,
                            blend_mode: options.blend_mode,
                        },
                        &mut terrain_context,
                    )
                    .map_err(|error| source_error(face.solid, error))?;
                    if plan.source != face {
                        return Err(invalid(
                            "Terrain neighbor partitioning lost its authorized source face",
                        ));
                    }
                    Ok(plan)
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        for plan in partitions {
            let (source, _) = partition_source(&sources, plan.source)?;
            plan.validate_blend(options.blend_mode)?;
            terrain_budget.add(tx.document(), context, source.solid, &plan)?;
            unchanged.retain(|id| *id != source.solid.id);
            planned.push(PlannedCut::terrain(source, plan, true));
        }
    }
    if planned.is_empty() {
        return Err(invalid(
            "Cut plane does not cross any selected brush; nothing changed",
        ));
    }
    if terrain_budget.sources != 0 {
        let vmf = context.unwrap_or(tx.vmf());
        let existing = crate::displacement::native::visible_terrain_sources(vmf)
            .map(|(solid, _)| solid)
            .flat_map(|solid| &solid.sides)
            .filter(|side| side.displacement.is_some())
            .count();
        let final_count = existing
            .checked_sub(terrain_budget.visible_sources)
            .and_then(|count| count.checked_add(terrain_budget.visible_patches))
            .ok_or_else(|| invalid("Terrain clipping patch count overflow"))?;
        if final_count > source_formats::bsp::MAX_DISPLACEMENTS {
            return Err(invalid(
                "Terrain clipping exceeds the native displacement patch limit",
            ));
        }
        let verified = match options.terrain_mode {
            TerrainMode::Grid => {
                let replacements = grid_replacements(&planned);
                crate::displacement::grid_clip::certify(
                    tx.document(),
                    &replacements,
                    &mut terrain_context,
                )?
                .into_iter()
                .map(|verified| (verified.neighbors, verified.links))
                .collect::<Vec<_>>()
            }
            TerrainMode::Flat | TerrainMode::Cells | TerrainMode::Fragments => {
                let replacements = planned
                    .iter()
                    .filter_map(|cut| {
                        cut.terrain.as_ref().map(|terrain| {
                            crate::displacement::flat_clip::network::Replacement {
                                source: terrain.source,
                                parts: &cut.parts,
                                windows: cut.children.as_deref(),
                                window_fields: cut.window_fields.as_ref(),
                                cut: (!terrain.partitioned).then_some((plane, keep)),
                                texture: options
                                    .textures
                                    .and_then(|textures| textures.get(&terrain.source))
                                    .copied(),
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let certify = if matches!(
                    options.terrain_mode,
                    TerrainMode::Cells | TerrainMode::Fragments
                ) {
                    crate::displacement::flat_clip::network::certify_cells
                } else {
                    crate::displacement::flat_clip::network::certify
                };
                certify(tx.document(), &replacements, &mut terrain_context)?
                    .into_iter()
                    .map(|verified| (verified.neighbors, verified.links))
                    .collect::<Vec<_>>()
            }
        };
        for (cut, (neighbors, links)) in planned
            .iter_mut()
            .filter(|cut| cut.terrain.is_some())
            .zip(verified)
        {
            let terrain = cut.terrain.as_mut().unwrap();
            terrain.verified_neighbors = neighbors;
            terrain.preserved_links = links;
        }
    }
    let mut result = ClipResult {
        cuts: Vec::new(),
        objects: Vec::new(),
        unchanged,
        side_mapping: BTreeMap::new(),
        terrain: Vec::new(),
    };
    let mut terrain_fragments = Vec::new();
    for mut cut in planned {
        for source in cut.source_sides {
            result.side_mapping.entry(source).or_default();
        }
        for (index, part) in cut.parts.iter_mut().enumerate() {
            if index != 0 {
                part.id = tx.new_object_id()?;
            }
            for side in &mut part.sides {
                let source = side.id;
                if index != 0 || source == 0 {
                    side.id = tx.new_side_id()?;
                }
                if source != 0 {
                    result.side_mapping.entry(source).or_default().push(side.id);
                }
                if side.displacement.is_some()
                    && let Some(terrain) = &mut cut.terrain
                {
                    terrain.faces.push(FaceId {
                        solid: part.id,
                        side: side.id,
                    });
                    terrain
                        .face_powers
                        .push(side.displacement.as_ref().unwrap().power);
                }
            }
        }
        result.cuts.push(Cut {
            source: cut.source,
            solids: cut.parts.iter().map(|solid| solid.id).collect(),
        });
        match cut.target {
            ObjectId::Solid(_) => result
                .objects
                .extend(cut.parts.iter().map(|part| ObjectId::Solid(part.id))),
            ObjectId::Entity(_) => result.objects.push(cut.target),
        }
        if let Some(terrain) = cut.terrain {
            result.terrain.push(terrain);
            terrain_fragments.extend(
                cut.parts
                    .into_iter()
                    .map(|part| (cut.source, cut.owner, part)),
            );
            continue;
        }
        match cut.owner {
            ObjectId::Solid(id) => {
                *tx.solid_mut(id)? = cut.parts.remove(0);
                for part in cut.parts {
                    tx.add_solid(part)?;
                }
            }
            ObjectId::Entity(_) if matches!(cut.target, ObjectId::Solid(_)) => {
                crate::subobject::fragments::install(
                    tx,
                    cut.parts
                        .into_iter()
                        .map(|part| (cut.source, cut.owner, part)),
                )?;
            }
            ObjectId::Entity(id) => {
                let entity = tx.entity_mut(id)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == cut.source)
                    .unwrap();
                entity.solids.splice(index..=index, cut.parts);
            }
        }
    }
    if !terrain_fragments.is_empty() {
        crate::subobject::fragments::install(tx, terrain_fragments)?;
    }
    result.objects.sort();
    result.objects.dedup();
    Ok(result)
}
