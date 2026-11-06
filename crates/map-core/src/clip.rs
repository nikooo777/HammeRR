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

pub const MAX_TERRAIN_SOURCES: usize = 16;
pub const MAX_TERRAIN_PATCHES: usize = MAX_TERRAIN_SOURCES * 4;
pub const MAX_TERRAIN_VERTICES: usize = MAX_TERRAIN_PATCHES * 81;
pub const MAX_TERRAIN_TRIANGLES: usize = MAX_TERRAIN_PATCHES * 128;
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
    pub source: FaceId,
    pub faces: Vec<FaceId>,
    pub source_power: u32,
    pub output_power: Option<u32>,
    pub face_powers: Vec<u32>,
    pub vertices: usize,
    pub triangles: usize,
    pub verified_neighbors: usize,
    pub preserved_links: usize,
}

pub struct Source<'a> {
    pub solid: &'a Solid,
    pub owner: ObjectId,
    pub target: ObjectId,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
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
}

pub(crate) struct TerrainPlan {
    pub parts: Vec<Solid>,
    pub source: FaceId,
    pub source_power: u32,
    pub output_power: Option<u32>,
    pub vertices: usize,
    pub triangles: usize,
    pub children: Option<Vec<crate::displacement::grid_clip::Child>>,
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
    let context = options.terrain_context;
    if let Some(context) = context {
        context
            .validate_ids()
            .map_err(|error| invalid(error.to_string()))?;
    }
    let plane = normalized_plane(plane)?;
    if ids.is_empty() {
        return Err(invalid("Select at least one brush or brush entity to clip"));
    }
    if material.is_some_and(|name| name.trim().is_empty()) {
        return Err(invalid("Cut-face material must not be empty"));
    }
    let mut planned = Vec::new();
    let mut unchanged = Vec::new();
    let mut terrain_sources = 0;
    let mut terrain_patches = 0;
    let mut visible_sources = 0;
    let mut visible_patches = 0;
    let mut terrain_vertices = 0;
    let mut terrain_triangles = 0;
    let mut terrain_context = crate::displacement::grid_clip::Context::new(context);
    for Source {
        solid,
        owner,
        target,
    } in sources(tx.document(), ids)?
    {
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
                .map(|plan| TerrainPlan {
                    parts: plan.parts,
                    source: plan.source,
                    source_power: plan.source_power,
                    output_power: plan.output_power,
                    vertices: plan.vertices,
                    triangles: plan.triangles,
                    children: Some(plan.children),
                }),
                TerrainMode::Flat | TerrainMode::Cells => {
                    let plan = if options.terrain_mode == TerrainMode::Cells {
                        crate::displacement::flat_clip::cells::plan
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
            let visible =
                crate::displacement::native::terrain_source_is_visible(tx.vmf(), solid.id);
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
            terrain_sources += 1;
            terrain_patches += plan.parts.len();
            if visible {
                visible_sources += solid
                    .sides
                    .iter()
                    .filter(|side| side.displacement.is_some())
                    .count();
                visible_patches += plan
                    .parts
                    .iter()
                    .filter(|part| !part.hidden)
                    .flat_map(|part| &part.sides)
                    .filter(|side| side.displacement.is_some())
                    .count();
            }
            terrain_vertices += plan.vertices;
            terrain_triangles += plan.triangles;
            if terrain_sources > MAX_TERRAIN_SOURCES
                || terrain_patches > MAX_TERRAIN_PATCHES
                || terrain_vertices > MAX_TERRAIN_VERTICES
                || terrain_triangles > MAX_TERRAIN_TRIANGLES
            {
                return Err(invalid(
                    "Terrain clipping exceeds its source, patch, vertex or triangle limit",
                ));
            }
            planned.push(PlannedCut {
                owner,
                target,
                source: solid.id,
                parts: plan.parts,
                source_sides: solid.sides.iter().map(|side| side.id).collect(),
                children: plan.children,
                terrain: Some(TerrainCut {
                    source: plan.source,
                    faces: Vec::new(),
                    source_power: plan.source_power,
                    output_power: plan.output_power,
                    face_powers: Vec::new(),
                    vertices: plan.vertices,
                    triangles: plan.triangles,
                    verified_neighbors: 0,
                    preserved_links: 0,
                }),
            });
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
        });
    }
    if planned.is_empty() {
        return Err(invalid(
            "Cut plane does not cross any selected brush; nothing changed",
        ));
    }
    if terrain_sources != 0 {
        let vmf = context.unwrap_or(tx.vmf());
        let existing = crate::displacement::native::visible_terrain_sources(vmf)
            .map(|(solid, _)| solid)
            .flat_map(|solid| &solid.sides)
            .filter(|side| side.displacement.is_some())
            .count();
        let final_count = existing
            .checked_sub(visible_sources)
            .and_then(|count| count.checked_add(visible_patches))
            .ok_or_else(|| invalid("Terrain clipping patch count overflow"))?;
        if final_count > source_formats::bsp::MAX_DISPLACEMENTS {
            return Err(invalid(
                "Terrain clipping exceeds the native displacement patch limit",
            ));
        }
        let verified = match options.terrain_mode {
            TerrainMode::Grid => {
                let replacements = planned
                    .iter()
                    .filter_map(|cut| {
                        cut.terrain.as_ref().zip(cut.children.as_ref()).map(
                            |(terrain, children)| crate::displacement::grid_clip::Replacement {
                                source: terrain.source,
                                source_power: terrain.source_power,
                                parts: &cut.parts,
                                children,
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                crate::displacement::grid_clip::certify(
                    tx.document(),
                    &replacements,
                    &mut terrain_context,
                )?
                .into_iter()
                .map(|verified| (verified.neighbors, verified.links))
                .collect::<Vec<_>>()
            }
            TerrainMode::Flat | TerrainMode::Cells => {
                let replacements = planned
                    .iter()
                    .filter_map(|cut| {
                        cut.terrain.as_ref().map(|terrain| {
                            crate::displacement::flat_clip::network::Replacement {
                                source: terrain.source,
                                parts: &cut.parts,
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let certify = if options.terrain_mode == TerrainMode::Cells {
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
