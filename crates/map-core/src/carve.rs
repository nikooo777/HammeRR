use std::collections::{BTreeMap, BTreeSet};

use source_formats::vmf::{Side, Solid};

use crate::plane::ON_EPSILON;
use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction, clip, subobject};

mod terrain;

pub const MAX_TARGETS: usize = 16;
pub const MAX_CUTTERS: usize = 8;
pub const MAX_FRAGMENTS: usize = 256;
const TARGET_NORMAL_EPSILON: f64 = 1e-8;

pub struct Preparation {
    work: Option<crate::displacement::grid_clip::Work>,
}

impl Default for Preparation {
    fn default() -> Self {
        Self {
            work: Some(Default::default()),
        }
    }
}

impl Preparation {
    pub(crate) fn take_work(
        &mut self,
    ) -> std::result::Result<crate::displacement::grid_clip::Work, EditError> {
        self.work
            .take()
            .ok_or_else(|| invalid("Terrain preparation has already failed or been consumed"))
    }

    pub(crate) fn work(
        &mut self,
    ) -> std::result::Result<&mut crate::displacement::grid_clip::Work, EditError> {
        self.work
            .as_mut()
            .ok_or_else(|| invalid("Terrain preparation has already failed or been consumed"))
    }

    pub(crate) fn restore_work(&mut self, work: crate::displacement::grid_clip::Work) {
        self.work = Some(work);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CutMaterialPolicy {
    #[default]
    Cutter,
    Target,
    Material(String),
}

impl CutMaterialPolicy {
    fn normalized(&self) -> std::result::Result<Self, EditError> {
        match self {
            Self::Material(material) => {
                if material.trim().is_empty() {
                    return Err(invalid("Cut material must not be empty"));
                }
                source_formats::keyvalues::validate_string(material)
                    .map_err(|error| invalid(error.to_string()))?;
                Ok(Self::Material(material.trim().to_ascii_uppercase()))
            }
            _ => Ok(self.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub source: u32,
    pub fragments: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inheritance {
    pub solid: u32,
    pub side: u32,
    pub source_solid: u32,
    pub source_side: Option<u32>,
    pub surface_source_solid: u32,
    pub surface_source_side: u32,
    pub cutter: bool,
    pub structural: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Result {
    pub cutter: u32,
    pub cutters: Vec<u32>,
    pub cut_material: CutMaterialPolicy,
    pub terrain_mode: clip::TerrainMode,
    pub blend_mode: clip::TerrainBlendMode,
    pub partition_neighbors: bool,
    pub changes: Vec<Change>,
    pub unchanged: Vec<u32>,
    pub removed: Vec<u32>,
    pub objects: Vec<ObjectId>,
    pub faces: Vec<Inheritance>,
    pub side_mapping: BTreeMap<u32, Vec<u32>>,
    pub terrain: Vec<clip::TerrainCut>,
}

struct Planned {
    owner: ObjectId,
    source: u32,
    parts: Vec<Solid>,
    terrain: Option<clip::TerrainCut>,
    structural: BTreeMap<u32, (u32, u32)>,
}

enum Position {
    Inside,
    Outside,
    Crossing,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn position(solid: &Solid, plane: Plane) -> std::result::Result<Position, EditError> {
    let shape = BrushGeometry::from_solid(solid)
        .map_err(|error| invalid(format!("Brush {}: {error}", solid.id)))?;
    let mut front = false;
    let mut back = false;
    for vertex in shape.faces.iter().flat_map(|face| &face.vertices) {
        let distance = plane.distance_to(*vertex);
        front |= distance > ON_EPSILON;
        back |= distance < -ON_EPSILON;
    }
    Ok(if !front {
        Position::Inside
    } else if !back {
        Position::Outside
    } else {
        Position::Crossing
    })
}

fn half(solid: &Solid, plane: Plane, source: &Side) -> std::result::Result<Solid, EditError> {
    checked_half(solid, plane, source).map_err(clip::HalfError::into_edit_error)
}

fn checked_half(
    solid: &Solid,
    plane: Plane,
    source: &Side,
) -> std::result::Result<Solid, clip::HalfError> {
    let mut part = clip::checked_half(solid, plane, &source.material)?;
    let cap = part.sides.last_mut().unwrap();
    *cap = crate::brush::precise::inherit_surface(source, cap).map_err(clip::HalfError::Invalid)?;
    subobject::topology(&part).map_err(clip::HalfError::Invalid)?;
    Ok(part)
}

pub(crate) fn intersects(source: &Solid, cutter: &Solid) -> std::result::Result<bool, EditError> {
    let mut inside = source.clone();
    for side in &cutter.sides {
        let plane = Plane::from_points(side.plane.0).unwrap();
        match position(&inside, plane)? {
            Position::Inside => {}
            Position::Outside => return Ok(false),
            Position::Crossing => inside = half(&inside, plane, side)?,
        }
    }
    Ok(true)
}

pub(crate) fn subtract(
    source: &Solid,
    cutter: &Solid,
) -> std::result::Result<Vec<Solid>, EditError> {
    let mut inside = source.clone();
    let mut parts = Vec::new();
    let mut pending = cutter.sides.iter().collect::<Vec<_>>();
    while !pending.is_empty() {
        let mut deferred = Vec::new();
        let mut failure = None;
        let mut progressed = false;
        for side in pending {
            let plane = Plane::from_points(side.plane.0).unwrap();
            match position(&inside, plane)? {
                Position::Inside => progressed = true,
                Position::Outside => {
                    parts.push(inside);
                    return Ok(parts);
                }
                Position::Crossing => match split(&inside, plane, side) {
                    Ok((outside, remainder)) => {
                        parts.push(outside);
                        inside = remainder;
                        progressed = true;
                    }
                    Err(error @ clip::HalfError::SmallVolume(_)) => {
                        deferred.push(side);
                        failure.get_or_insert(error);
                    }
                    Err(error) => return Err(error.into_edit_error()),
                },
            }
        }
        if !progressed {
            return Err(failure.unwrap().into_edit_error());
        }
        pending = deferred;
    }
    Ok(parts)
}

fn split(
    solid: &Solid,
    plane: Plane,
    side: &Side,
) -> std::result::Result<(Solid, Solid), clip::HalfError> {
    let outside = checked_half(
        solid,
        Plane {
            normal: -plane.normal,
            dist: -plane.dist,
        },
        side,
    )?;
    let inside = checked_half(solid, plane, side)?;
    Ok((outside, inside))
}

fn plan(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
) -> std::result::Result<(Vec<Planned>, Vec<u32>), EditError> {
    let targets = targets.iter().copied().collect::<BTreeSet<_>>();
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(invalid("Carve between 1 and 16 target brushes at a time"));
    }
    let ids = cutters.iter().copied().collect::<BTreeSet<_>>();
    if cutters.is_empty() || cutters.len() > MAX_CUTTERS || ids.len() != cutters.len() {
        return Err(invalid("Choose between 1 and 8 distinct cutter brushes"));
    }
    let mut cutters = Vec::new();
    for id in ids {
        if targets.contains(&id) {
            return Err(invalid("A cutter cannot also be a target brush"));
        }
        let (_, cutter) = subobject::solid(document, id)?;
        subobject::topology(cutter)?;
        if cutter.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Cutter {id}: terrain has no closed native volume and cannot be a cutter"
            )));
        }
        cutters.push(cutter);
    }
    let mut planned = Vec::new();
    let mut unchanged = Vec::new();
    let mut count = 0;
    for id in targets {
        let (owner, source) = subobject::solid(document, id)?;
        subobject::topology(source)?;
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: carving displacements is unsupported"
            )));
        }
        let (parts, affected) = ordinary_parts(source, &cutters, count)?;
        if !affected {
            unchanged.push(id);
            continue;
        }
        count += parts.len();
        if count > MAX_FRAGMENTS {
            return Err(invalid(
                "Carving would exceed 256 fragments; reduce the selection",
            ));
        }
        planned.push(Planned {
            owner,
            source: id,
            parts,
            terrain: None,
            structural: BTreeMap::new(),
        });
    }
    if planned.is_empty() {
        return Err(invalid(
            "The cutter does not overlap any target brush; nothing changed",
        ));
    }
    for entity in document
        .vmf()
        .entities
        .iter()
        .filter(|entity| !entity.solids.is_empty())
    {
        if entity.solids.iter().all(|solid| {
            planned
                .iter()
                .any(|part| part.source == solid.id && part.parts.is_empty())
        }) {
            return Err(invalid(format!(
                "Carving would remove every brush from entity {}; keep a brush or delete the entity explicitly",
                entity.id
            )));
        }
    }
    Ok((planned, unchanged))
}

fn ordinary_parts(
    source: &Solid,
    cutters: &[&Solid],
    count: usize,
) -> std::result::Result<(Vec<Solid>, bool), EditError> {
    let mut parts = vec![source.clone()];
    let mut affected = false;
    for cutter in cutters {
        let mut remaining = Vec::new();
        for part in parts {
            if intersects(&part, cutter)? {
                affected = true;
                remaining.extend(subtract(&part, cutter)?);
            } else {
                remaining.push(part);
            }
            if count + remaining.len() > MAX_FRAGMENTS {
                return Err(invalid(
                    "Carving would exceed 256 fragments at an intermediate stage; reduce the selection",
                ));
            }
        }
        parts = remaining;
        if parts.is_empty() {
            break;
        }
    }
    Ok((parts, affected))
}

pub fn carve(
    tx: &mut Transaction,
    cutter: u32,
    targets: &[u32],
) -> std::result::Result<Result, EditError> {
    carve_many(tx, &[cutter], targets)
}

pub fn carve_many(
    tx: &mut Transaction,
    cutters: &[u32],
    targets: &[u32],
) -> std::result::Result<Result, EditError> {
    carve_many_with_material(tx, cutters, targets, &CutMaterialPolicy::Cutter)
}

pub fn carve_with_material(
    tx: &mut Transaction,
    cutter: u32,
    targets: &[u32],
    cut_material: &CutMaterialPolicy,
) -> std::result::Result<Result, EditError> {
    carve_many_with_material(tx, &[cutter], targets, cut_material)
}

fn target_surface<'a>(side: &Side, target: &'a Solid) -> std::result::Result<&'a Side, EditError> {
    let normal = Plane::from_points(side.plane.0).unwrap().normal;
    let geometry = if target.sides.iter().any(|side| side.displacement.is_some()) {
        Some(BrushGeometry::from_solid(target).map_err(|error| invalid(error.to_string()))?)
    } else {
        None
    };
    let mut matches = target
        .sides
        .iter()
        .enumerate()
        .filter(|(index, source)| {
            let source_normal = geometry.as_ref().map_or_else(
                || Plane::from_points(source.plane.0).unwrap().normal,
                |geometry| {
                    geometry
                        .faces
                        .iter()
                        .find(|face| face.side == *index)
                        .unwrap()
                        .plane
                        .normal
                },
            );
            source_normal.dot(normal) >= 1.0 - TARGET_NORMAL_EPSILON
        })
        .map(|(_, side)| side);
    let source = matches.next().ok_or_else(|| {
        invalid(format!(
            "Brush {} has no original face parallel to a generated cut face with the same outward normal; choose cutter or named cut material",
            target.id
        ))
    })?;
    if matches.next().is_some() {
        return Err(invalid(format!(
            "Brush {} has ambiguous original faces for a generated cut face; choose cutter or named cut material",
            target.id
        )));
    }
    Ok(source)
}

pub fn carve_many_with_material(
    tx: &mut Transaction,
    cutters: &[u32],
    targets: &[u32],
    cut_material: &CutMaterialPolicy,
) -> std::result::Result<Result, EditError> {
    carve_many_with_options(
        tx,
        cutters,
        targets,
        cut_material,
        clip::ClipOptions::default(),
    )
}

pub fn carve_many_with_options(
    tx: &mut Transaction,
    cutters: &[u32],
    targets: &[u32],
    cut_material: &CutMaterialPolicy,
    options: clip::ClipOptions<'_>,
) -> std::result::Result<Result, EditError> {
    carve_many_with_preparation(
        tx,
        cutters,
        targets,
        cut_material,
        options,
        &mut Preparation::default(),
    )
}

pub fn carve_many_with_preparation(
    tx: &mut Transaction,
    cutters: &[u32],
    targets: &[u32],
    cut_material: &CutMaterialPolicy,
    options: clip::ClipOptions<'_>,
    preparation: &mut Preparation,
) -> std::result::Result<Result, EditError> {
    let cut_material = cut_material.normalized()?;
    terrain::validate_options(&options)?;
    preparation.work()?;
    let has_terrain = targets.iter().any(|id| {
        subobject::solid(tx.document(), *id)
            .is_ok_and(|(_, solid)| solid.sides.iter().any(|side| side.displacement.is_some()))
    });
    let (planned, unchanged) = if has_terrain {
        terrain::plan(tx.document(), cutters, targets, &options, preparation)?
    } else {
        plan(tx.document(), cutters, targets)?
    };
    let cutters = cutters
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cutter_sides = cutters
        .iter()
        .map(|id| {
            subobject::solid(tx.document(), *id).map(|(_, solid)| {
                solid
                    .sides
                    .iter()
                    .map(|side| (side.id, *id))
                    .collect::<Vec<_>>()
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<BTreeMap<_, _>>();
    let mut result = Result {
        cutter: cutters[0],
        cutters,
        cut_material: cut_material.clone(),
        terrain_mode: options.terrain_mode,
        blend_mode: options.blend_mode,
        partition_neighbors: options.partition_neighbors,
        changes: Vec::new(),
        unchanged,
        removed: Vec::new(),
        objects: Vec::new(),
        faces: Vec::new(),
        side_mapping: BTreeMap::new(),
        terrain: Vec::new(),
    };
    let mut allocated = Vec::new();
    for mut part in planned {
        let (_, source) = subobject::solid(tx.document(), part.source)?;
        for side in &source.sides {
            result.side_mapping.entry(side.id).or_default();
        }
        for (index, solid) in part.parts.iter_mut().enumerate() {
            if index > 0 {
                solid.id = tx.new_object_id()?;
            }
            let geometry = if part.terrain.is_some() {
                Some(BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?)
            } else {
                None
            };
            for (side_index, side) in solid.sides.iter_mut().enumerate() {
                let source_side = side.id;
                let structural = part.structural.get(&source_side).copied();
                let from_cutter = structural.is_none() && cutter_sides.contains_key(&source_side);
                let source_solid = if from_cutter {
                    cutter_sides[&source_side]
                } else {
                    part.source
                };
                let (surface_source_solid, surface_source_side) =
                    if from_cutter || structural.is_some() {
                        let donor = if let Some((solid, side)) = structural {
                            let (_, donor) = subobject::solid(tx.document(), solid)?;
                            donor
                                .sides
                                .iter()
                                .find(|candidate| candidate.id == side)
                                .ok_or_else(|| {
                                    invalid("Terrain carrier lost its surface-settings donor")
                                })?
                        } else {
                            &*side
                        };
                        let source = match &cut_material {
                            CutMaterialPolicy::Target => {
                                let (_, target) = subobject::solid(tx.document(), part.source)?;
                                target_surface(side, target)?
                            }
                            _ => donor,
                        };
                        let surface_source = (
                            if cut_material == CutMaterialPolicy::Target {
                                part.source
                            } else {
                                structural.map_or(source_solid, |(solid, _)| solid)
                            },
                            source.id,
                        );
                        let mut surface = crate::brush::precise::inherit_surface(source, side)?;
                        surface.displacement = None;
                        if let CutMaterialPolicy::Material(material) = &cut_material {
                            surface.material = material.clone();
                        }
                        if cut_material != CutMaterialPolicy::Cutter || structural.is_some() {
                            let points = if structural.is_some() {
                                geometry
                                    .as_ref()
                                    .and_then(|geometry| {
                                        geometry.faces.iter().find(|face| face.side == side_index)
                                    })
                                    .map(|face| face.vertices.as_slice())
                                    .ok_or_else(|| {
                                        invalid("Terrain carrier lost its active geometry")
                                    })?
                            } else {
                                &[]
                            };
                            crate::surface::update(
                                &mut surface,
                                &crate::surface::SurfaceEdit::default(),
                                points,
                            )?;
                        }
                        *side = surface;
                        surface_source
                    } else {
                        (source_solid, source_side)
                    };
                if index > 0 || from_cutter || structural.is_some() {
                    side.id = tx.new_side_id()?;
                }
                if structural.is_none()
                    && !from_cutter
                    && result.side_mapping.contains_key(&source_side)
                {
                    result
                        .side_mapping
                        .entry(source_side)
                        .or_default()
                        .push(side.id);
                }
                if let Some(terrain) = &mut part.terrain
                    && let Some(info) = &side.displacement
                {
                    terrain.faces.push(crate::surface::FaceId {
                        solid: solid.id,
                        side: side.id,
                    });
                    terrain.face_powers.push(info.power);
                }
                result.faces.push(Inheritance {
                    solid: solid.id,
                    side: side.id,
                    source_solid,
                    source_side: structural.is_none().then_some(source_side),
                    surface_source_solid,
                    surface_source_side,
                    cutter: from_cutter,
                    structural: structural.is_some(),
                });
            }
            subobject::topology(solid)?;
        }
        if part.parts.is_empty() {
            result.removed.push(part.source);
        }
        result.changes.push(Change {
            source: part.source,
            fragments: part.parts.iter().map(|solid| solid.id).collect(),
        });
        if let Some(terrain) = &part.terrain {
            result.terrain.push(terrain.clone());
        }
        allocated.push(part);
    }
    let mut installed = BTreeSet::new();
    let mut fragments = Vec::new();
    if has_terrain {
        for part in &mut allocated {
            if part.parts.is_empty() {
                continue;
            }
            match part.owner {
                ObjectId::Solid(_) => result
                    .objects
                    .extend(part.parts.iter().map(|solid| ObjectId::Solid(solid.id))),
                ObjectId::Entity(_) => result.objects.push(part.owner),
            }
            installed.insert(part.source);
            fragments.extend(
                part.parts
                    .drain(..)
                    .map(|solid| (part.source, part.owner, solid)),
            );
        }
        if !fragments.is_empty() {
            subobject::fragments::install(tx, fragments)?;
        }
    }
    for mut part in allocated {
        if installed.contains(&part.source) {
            continue;
        }
        match part.owner {
            ObjectId::Solid(id) => {
                if part.parts.is_empty() {
                    tx.remove(part.owner)?;
                } else {
                    *tx.solid_mut(id)? = part.parts.remove(0);
                    result.objects.push(part.owner);
                    for solid in part.parts {
                        result.objects.push(tx.add_solid(solid)?);
                    }
                }
            }
            ObjectId::Entity(id) => {
                let entity = tx.entity_mut(id)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == part.source)
                    .unwrap();
                entity.solids.splice(index..=index, part.parts);
                result.objects.push(part.owner);
            }
        }
    }
    result.objects.sort();
    result.objects.dedup();
    Ok(result)
}

pub fn terrain_texture_faces(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    mode: clip::TerrainMode,
    partition_neighbors: bool,
    context: Option<&source_formats::vmf::Vmf>,
) -> std::result::Result<Vec<crate::surface::FaceId>, EditError> {
    terrain_texture_faces_with_preparation(
        document,
        cutters,
        targets,
        &clip::ClipOptions {
            terrain_mode: mode,
            partition_neighbors,
            terrain_context: context,
            ..Default::default()
        },
        &mut Preparation::default(),
    )
}

pub fn terrain_texture_faces_with_preparation(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    options: &clip::ClipOptions<'_>,
    preparation: &mut Preparation,
) -> std::result::Result<Vec<crate::surface::FaceId>, EditError> {
    terrain::texture_faces(document, cutters, targets, options, preparation)
}

pub fn terrain_partition_faces(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    options: clip::ClipOptions<'_>,
    preparation: &mut Preparation,
) -> std::result::Result<Vec<crate::surface::FaceId>, EditError> {
    terrain::partition_faces(document, cutters, targets, &options, preparation)
}

pub fn requires_terrain_context(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    mode: clip::TerrainMode,
) -> std::result::Result<bool, EditError> {
    terrain::requires_context(document, cutters, targets, mode)
}
