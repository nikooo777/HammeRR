use std::collections::{BTreeMap, BTreeSet};

use source_formats::vmf::{Solid, Vmf};

use super::grid_clip::{Context, Work};
use crate::carve::Preparation;
use crate::clip::{self, ClipOptions, ClipResult, TerrainTriangleMask};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, ObjectId, Transaction, subobject};

struct Source<'a> {
    face: FaceId,
    owner: ObjectId,
    solid: &'a Solid,
    ids: &'a [usize],
}

struct Group {
    owner: ObjectId,
    mask: TerrainTriangleMask,
    parts: Vec<Solid>,
    partitioned: bool,
    verified: (usize, usize, Option<clip::TerrainBlendError>),
}

struct Ids {
    used: BTreeSet<u32>,
    structural: BTreeSet<u32>,
    next: u32,
}

impl Ids {
    fn new(document: &Document, context: Option<&Vmf>) -> Self {
        Self {
            used: document
                .vmf()
                .side_ids()
                .chain(context.into_iter().flat_map(Vmf::side_ids))
                .collect(),
            structural: BTreeSet::new(),
            next: 1,
        }
    }

    fn allocate(&mut self) -> Result<u32, EditError> {
        while self.used.contains(&self.next) {
            self.next = self
                .next
                .checked_add(1)
                .ok_or_else(|| invalid("Terrain planning IDs are exhausted"))?;
        }
        let result = self.next;
        self.used.insert(result);
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("Terrain planning IDs are exhausted"))?;
        Ok(result)
    }
}

struct Build<'a> {
    source: FaceId,
    solid: &'a Solid,
    mask: Option<&'a TerrainTriangleMask>,
}

struct Plan {
    groups: Vec<Group>,
    structural: BTreeSet<u32>,
    work: Work,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn options(options: &ClipOptions<'_>) -> Result<(), EditError> {
    if options.terrain_mode != clip::TerrainMode::Fragments {
        return Err(invalid(
            "Triangle removal requires the native surface-triangle representation",
        ));
    }
    if let Some(context) = options.terrain_context {
        context
            .validate_ids()
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(())
}

fn sources<'a>(
    document: &'a Document,
    targets: &'a [(FaceId, Vec<usize>)],
) -> Result<Vec<Source<'a>>, EditError> {
    if targets.is_empty() || targets.len() > clip::MAX_TERRAIN_SOURCES {
        return Err(invalid(
            "Remove triangles from 1–16 terrain faces at a time",
        ));
    }
    document
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    let mut result = BTreeMap::new();
    let mut parents = BTreeSet::new();
    let mut selected = false;
    for (face, ids) in targets {
        if !parents.insert(face.solid) {
            return Err(invalid(
                "Use one triangle selection per terrain parent brush",
            ));
        }
        let (owner, solid) = subobject::solid(document, face.solid)?;
        subobject::topology(solid)?;
        if let ObjectId::Entity(id) = owner
            && document.entity(id).and_then(|entity| entity.classname()) != Some("func_detail")
        {
            return Err(invalid(
                "Triangle removal requires world or func_detail terrain",
            ));
        }
        let displaced = solid
            .sides
            .iter()
            .filter(|side| side.displacement.is_some())
            .collect::<Vec<_>>();
        let [side] = displaced.as_slice() else {
            return Err(invalid(
                "Triangle removal requires one displaced face per brush",
            ));
        };
        if side.id != face.side {
            return Err(invalid(
                "Selected terrain face is missing or is not displaced",
            ));
        }
        let power = side.displacement.as_ref().unwrap().power;
        if !(2..=4).contains(&power) {
            return Err(invalid("Terrain power must be 2, 3 or 4"));
        }
        let count = 2 * (1usize << power).pow(2);
        if ids.iter().any(|id| *id >= count) {
            return Err(invalid(
                "Select valid native terrain inspection triangle IDs",
            ));
        }
        selected |= !ids.is_empty();
        result.insert(
            *face,
            Source {
                face: *face,
                owner,
                solid,
                ids,
            },
        );
    }
    if !selected {
        return Err(invalid("Select at least one terrain triangle to remove"));
    }
    Ok(result.into_values().collect())
}

fn masks(
    document: &Document,
    sources: &[Source<'_>],
    options: &ClipOptions<'_>,
    work: &mut Work,
) -> Result<Vec<TerrainTriangleMask>, EditError> {
    if !options.partition_neighbors && sources.iter().any(|source| source.ids.is_empty()) {
        return Err(invalid(
            "Empty triangle selections only authorize explicitly partitioned neighbors",
        ));
    }
    sources
        .iter()
        .map(|source| clip::terrain_triangle_mask(document, source.face, source.ids, work))
        .collect()
}

fn total_triangles(mask: &TerrainTriangleMask) -> usize {
    2 * (1usize << mask.power()).pow(2)
}

fn capacity(patches: usize, vertices: usize, triangles: usize) -> Result<(), EditError> {
    if patches > clip::MAX_TERRAIN_PATCHES
        || vertices > clip::MAX_TERRAIN_VERTICES
        || triangles > clip::MAX_TERRAIN_TRIANGLES
    {
        return Err(invalid(
            "Terrain triangle removal exceeds its shared patch, vertex or triangle limit",
        ));
    }
    Ok(())
}

fn planned_capacity(masks: &[TerrainTriangleMask], work: &mut Work) -> Result<(), EditError> {
    let mut totals = [0usize; 3];
    for mask in masks {
        let (patches, vertices, triangles) =
            super::flat_clip::fragments::removal_counts(mask, work)?;
        for (total, added) in totals.iter_mut().zip([patches, vertices, triangles]) {
            *total = total
                .checked_add(added)
                .ok_or_else(|| invalid("Terrain output accounting overflow"))?;
        }
    }
    capacity(totals[0], totals[1], totals[2])
}

fn current_totals(groups: &BTreeMap<FaceId, Group>) -> Result<[usize; 3], EditError> {
    let mut totals = [0usize; 3];
    for group in groups
        .values()
        .filter(|group| !group.mask.ids().is_empty() || group.partitioned)
    {
        for info in group
            .parts
            .iter()
            .flat_map(|part| &part.sides)
            .filter_map(|side| side.displacement.as_ref())
        {
            if !(2..=4).contains(&info.power) {
                return Err(invalid("Native terrain requires power 2, 3 or 4"));
            }
            let cells = 1usize << info.power;
            for (total, added) in totals
                .iter_mut()
                .zip([1, (cells + 1).pow(2), 2 * cells.pow(2)])
            {
                *total = total
                    .checked_add(added)
                    .ok_or_else(|| invalid("Terrain output accounting overflow"))?;
            }
        }
    }
    Ok(totals)
}

fn current_capacity(groups: &BTreeMap<FaceId, Group>) -> Result<(), EditError> {
    let totals = current_totals(groups)?;
    capacity(totals[0], totals[1], totals[2])
}

fn partition_capacity(
    groups: &BTreeMap<FaceId, Group>,
    face: FaceId,
    index: usize,
    work: &mut Work,
) -> Result<(), EditError> {
    work.consume(1).map_err(invalid)?;
    let mut totals = current_totals(groups)?;
    let group = &groups[&face];
    let info = group.parts[index]
        .sides
        .iter()
        .find_map(|side| side.displacement.as_ref())
        .ok_or_else(|| invalid("Terrain subdivision lost its displaced face"))?;
    let cells = 1usize << info.power;
    if !group.mask.ids().is_empty() || group.partitioned {
        for (total, removed) in totals
            .iter_mut()
            .zip([1, (cells + 1).pow(2), 2 * cells.pow(2)])
        {
            *total = total
                .checked_sub(removed)
                .ok_or_else(|| invalid("Terrain subdivision output accounting underflow"))?;
        }
    }
    let (patches, vertices, triangles) =
        super::flat_clip::partition_counts(info.power, clip::TerrainMode::Fragments)?;
    for (total, added) in totals.iter_mut().zip([patches, vertices, triangles]) {
        *total = total
            .checked_add(added)
            .ok_or_else(|| invalid("Terrain output accounting overflow"))?;
    }
    capacity(totals[0], totals[1], totals[2])
}

fn shape(solid: &Solid) -> Result<BrushGeometry, EditError> {
    BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))
}

fn carrier_surface(
    source: &Solid,
    geometry: &BrushGeometry,
    side: &source_formats::vmf::Side,
    work: &mut Work,
) -> Result<source_formats::vmf::Side, EditError> {
    let normal = crate::Plane::from_points(side.plane.0)
        .ok_or_else(|| invalid("Terrain carrier lost its plane"))?
        .normal;
    work.consume(source.sides.len()).map_err(invalid)?;
    let mut best = None;
    for face in &geometry.faces {
        let candidate = &source.sides[face.side];
        if crate::surface::validate_projection(candidate, normal, &[]).is_err() {
            continue;
        }
        let alignment = face.plane.normal.dot(normal);
        if best.is_none_or(|(_, score)| alignment > score) {
            best = Some((candidate, alignment));
        }
    }
    let (donor, _) = best.ok_or_else(|| invalid("No original cage projection maps a generated terrain carrier face; adjust the source projections"))?;
    let mut result = crate::brush::precise::inherit_surface(donor, side)?;
    result.displacement = None;
    Ok(result)
}

fn build(
    document: &Document,
    request: Build<'_>,
    options: &ClipOptions<'_>,
    ids: &mut Ids,
    mut work: Work,
) -> Result<(Vec<Solid>, Work), EditError> {
    let (_, original) = subobject::solid(document, request.source.solid)?;
    let original_shape = shape(original)?;
    let mut solid = request.solid.clone();
    let mut mapping = BTreeMap::new();
    let mut temporary_face = None;
    work.consume(solid.sides.len()).map_err(invalid)?;
    for side in &mut solid.sides {
        let original = side.id;
        side.id = ids.allocate()?;
        mapping.insert(side.id, original);
        if side.displacement.is_some() {
            temporary_face = Some(FaceId {
                solid: solid.id,
                side: side.id,
            });
        }
    }
    let face = temporary_face.ok_or_else(|| invalid("Terrain source lost its displaced face"))?;
    let texture = options
        .textures
        .and_then(|textures| textures.get(&request.source))
        .copied();
    let textures = texture
        .map(|texture| BTreeMap::from([(face, texture)]))
        .unwrap_or_default();
    let mut context = Context::with_work(options.terrain_context, work);
    let geometry = shape(&solid)?;
    let material = &original
        .sides
        .iter()
        .find(|side| side.id == request.source.side)
        .unwrap()
        .material;
    let plan = if let Some(mask) = request.mask {
        super::flat_clip::fragments::remove_triangles(
            document,
            &solid,
            &geometry,
            mask,
            super::flat_clip::PartitionRequest {
                material,
                textures: Some(&textures),
                blend_mode: options.blend_mode,
            },
            &mut context,
        )?
    } else {
        clip::plan_terrain_partition(
            document,
            &solid,
            &geometry,
            material,
            &ClipOptions {
                terrain_mode: clip::TerrainMode::Fragments,
                blend_mode: options.blend_mode,
                partition_neighbors: false,
                textures: Some(&textures),
                terrain_context: options.terrain_context,
            },
            &mut context,
        )?
    };
    plan.validate_blend(options.blend_mode)?;
    let mut parts = plan.parts;
    for part in &mut parts {
        part.id = request.source.solid;
        for side in &mut part.sides {
            if side.id == 0 {
                if side.displacement.is_some() {
                    return Err(invalid(
                        "Generated terrain carrier cannot replace a displaced source",
                    ));
                }
                let marker = ids.allocate()?;
                *side = carrier_surface(original, &original_shape, side, &mut context.work)?;
                side.id = marker;
                ids.structural.insert(marker);
            } else {
                side.id = *mapping
                    .get(&side.id)
                    .ok_or_else(|| invalid("Terrain removal lost its original side lineage"))?;
            }
        }
        if part
            .sides
            .iter()
            .find(|side| side.displacement.is_some())
            .map(|side| side.id)
            != Some(request.source.side)
        {
            return Err(invalid(
                "Terrain removal changed its original displaced-face identity",
            ));
        }
        subobject::topology(part)?;
    }
    Ok((parts, context.into_work()))
}

fn replacements<'a>(groups: &'a BTreeMap<FaceId, Group>) -> Vec<clip::TerrainReplacement<'a>> {
    groups
        .values()
        .map(|group| clip::TerrainReplacement {
            source: group.mask.source(),
            source_power: group.mask.power(),
            parts: &group.parts,
            children: None,
            window_fields: None,
            cut: None,
            subtract: false,
            changed: !group.mask.ids().is_empty() || group.partitioned,
            mask: Some(&group.mask),
        })
        .collect()
}

fn owner_capacity(document: &Document, groups: &BTreeMap<FaceId, Group>) -> Result<(), EditError> {
    for entity in document
        .vmf()
        .entities
        .iter()
        .filter(|entity| !entity.solids.is_empty())
    {
        if entity.solids.iter().all(|solid| {
            groups
                .values()
                .any(|group| group.mask.source().solid == solid.id && group.parts.is_empty())
        }) {
            return Err(invalid(format!(
                "Triangle removal would remove every brush from entity {}; keep a brush or delete the entity explicitly",
                entity.id
            )));
        }
    }
    Ok(())
}

fn plan(
    document: &Document,
    targets: &[(FaceId, Vec<usize>)],
    options: &ClipOptions<'_>,
    mut work: Work,
    mut needed: Option<&mut BTreeSet<FaceId>>,
) -> Result<Plan, EditError> {
    let sources = sources(document, targets)?;
    super::native_topology::check(
        options.terrain_context.unwrap_or(document.vmf()),
        &sources
            .iter()
            .filter(|source| !source.ids.is_empty())
            .map(|source| source.face)
            .collect::<Vec<_>>(),
        &[],
        &mut |amount| work.consume(amount),
    )?;
    let masks = masks(document, &sources, options, &mut work)?;
    planned_capacity(&masks, &mut work)?;
    let mut ids = Ids::new(document, options.terrain_context);
    let mut groups = BTreeMap::new();
    for (source, mask) in sources.into_iter().zip(masks) {
        let parts = if mask.ids().is_empty() {
            vec![source.solid.clone()]
        } else if mask.ids().len() == total_triangles(&mask) {
            Vec::new()
        } else {
            let (parts, remaining) = build(
                document,
                Build {
                    source: source.face,
                    solid: source.solid,
                    mask: Some(&mask),
                },
                options,
                &mut ids,
                work,
            )?;
            work = remaining;
            parts
        };
        groups.insert(
            source.face,
            Group {
                owner: source.owner,
                mask,
                parts,
                partitioned: false,
                verified: (0, 0, None),
            },
        );
    }
    owner_capacity(document, &groups)?;
    loop {
        let mut context = Context::with_work(options.terrain_context, work);
        let required =
            clip::required_terrain_partitions(document, &replacements(&groups), &mut context)?;
        work = context.into_work();
        let mut parts = BTreeMap::<FaceId, BTreeSet<usize>>::new();
        let mut missing = BTreeSet::new();
        for required in required
            .into_iter()
            .filter(|required| required.internal || options.partition_neighbors)
        {
            let group = groups.get(&required.source).ok_or_else(|| invalid(format!("Select terrain face {}:{} with an empty triangle list to authorize neighbor partitioning", required.source.solid, required.source.side)))?;
            if required.part >= group.parts.len() {
                return Err(invalid(
                    "Terrain subdivision lost its current retained part",
                ));
            }
            parts
                .entry(required.source)
                .or_default()
                .insert(required.part);
            if options
                .textures
                .is_none_or(|textures| !textures.contains_key(&required.source))
            {
                missing.insert(required.source);
            }
        }
        if !missing.is_empty() {
            if let Some(needed) = needed.as_deref_mut() {
                needed.extend(missing);
                return Ok(Plan {
                    groups: Vec::new(),
                    structural: ids.structural,
                    work,
                });
            }
            return Err(invalid(
                "Required terrain neighbor partition needs guarded native texture dimensions",
            ));
        }
        if parts.is_empty() {
            break;
        }
        for (face, indices) in parts {
            for index in indices.into_iter().rev() {
                partition_capacity(&groups, face, index, &mut work)?;
                let (parts, remaining) = build(
                    document,
                    Build {
                        source: face,
                        solid: &groups[&face].parts[index],
                        mask: None,
                    },
                    options,
                    &mut ids,
                    work,
                )?;
                work = remaining;
                let group = groups.get_mut(&face).unwrap();
                group.parts.splice(index..=index, parts);
                group.partitioned = group.mask.ids().is_empty();
                current_capacity(&groups)?;
            }
        }
    }
    if needed.is_none() {
        let mut context = Context::with_work(options.terrain_context, work);
        let verified = clip::certify_terrain(
            document,
            &replacements(&groups),
            options,
            &mut context,
            None,
        )?;
        if verified.len() != groups.len() {
            return Err(invalid("Terrain removal lost a source certificate"));
        }
        for (group, verified) in groups.values_mut().zip(verified) {
            group.verified = verified;
        }
        work = context.into_work();
    }
    Ok(Plan {
        groups: groups.into_values().collect(),
        structural: ids.structural,
        work,
    })
}

pub fn requires_terrain_context(
    document: &Document,
    targets: &[(FaceId, Vec<usize>)],
) -> Result<bool, EditError> {
    let sources = sources(document, targets)?;
    Ok(document.vmf().entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) && sources
        .iter()
        .any(|source| super::native::terrain_source_is_visible(document.vmf(), source.face.solid)))
}

pub fn texture_faces(
    document: &Document,
    targets: &[(FaceId, Vec<usize>)],
    settings: &ClipOptions<'_>,
    preparation: &mut Preparation,
) -> Result<Vec<FaceId>, EditError> {
    options(settings)?;
    let sources = sources(document, targets)?;
    let work = preparation.work()?;
    let masks = masks(document, &sources, settings, work)?;
    planned_capacity(&masks, work)?;
    Ok(masks
        .into_iter()
        .filter(|mask| !mask.ids().is_empty() && mask.ids().len() != total_triangles(mask))
        .map(|mask| mask.source())
        .collect())
}

pub fn partition_faces(
    document: &Document,
    targets: &[(FaceId, Vec<usize>)],
    settings: ClipOptions<'_>,
    preparation: &mut Preparation,
) -> Result<Vec<FaceId>, EditError> {
    options(&settings)?;
    preparation.work()?;
    let selected = sources(document, targets)?;
    if !settings.partition_neighbors {
        if selected.iter().any(|source| source.ids.is_empty()) {
            return Err(invalid(
                "Empty triangle selections only authorize explicitly partitioned neighbors",
            ));
        }
        return Ok(Vec::new());
    }
    let mut needed = BTreeSet::new();
    let work = preparation.take_work()?;
    let plan = plan(document, targets, &settings, work, Some(&mut needed))?;
    preparation.restore_work(plan.work);
    Ok(needed.into_iter().collect())
}

pub fn remove(
    tx: &mut Transaction,
    targets: &[(FaceId, Vec<usize>)],
    settings: ClipOptions<'_>,
    preparation: &mut Preparation,
) -> Result<ClipResult, EditError> {
    options(&settings)?;
    let work = preparation.take_work()?;
    let plan = plan(tx.document(), targets, &settings, work, None)?;
    preparation.restore_work(plan.work);
    let mut result = ClipResult {
        cuts: Vec::new(),
        objects: Vec::new(),
        unchanged: Vec::new(),
        side_mapping: BTreeMap::new(),
        terrain: Vec::new(),
    };
    let mut survivors = Vec::new();
    let mut removed = Vec::new();
    for mut group in plan.groups {
        let source = group.mask.source();
        if group.mask.ids().is_empty() && !group.partitioned {
            result.unchanged.push(source.solid);
            continue;
        }
        let (_, original) = subobject::solid(tx.document(), source.solid)?;
        for side in &original.sides {
            result.side_mapping.entry(side.id).or_default();
        }
        let mut terrain = clip::TerrainCut {
            partitioned: group.partitioned,
            source,
            faces: Vec::new(),
            source_power: group.mask.power(),
            output_power: None,
            face_powers: Vec::new(),
            vertices: 0,
            triangles: 0,
            verified_neighbors: group.verified.0,
            preserved_links: group.verified.1,
            blend_error: group.verified.2,
        };
        for (index, part) in group.parts.iter_mut().enumerate() {
            if index != 0 {
                part.id = tx.new_object_id()?;
            }
            let geometry = shape(part)?;
            for (side_index, side) in part.sides.iter_mut().enumerate() {
                let origin = side.id;
                let structural = plan.structural.contains(&origin);
                if structural {
                    let points = &geometry
                        .faces
                        .iter()
                        .find(|face| face.side == side_index)
                        .ok_or_else(|| invalid("Terrain carrier lost its active face"))?
                        .vertices;
                    crate::surface::update(side, &crate::surface::SurfaceEdit::default(), points)?;
                }
                if index != 0 || structural {
                    side.id = tx.new_side_id()?;
                }
                if !structural {
                    result.side_mapping.entry(origin).or_default().push(side.id);
                }
                if let Some(info) = &side.displacement {
                    terrain.faces.push(FaceId {
                        solid: part.id,
                        side: side.id,
                    });
                    terrain.face_powers.push(info.power);
                    let cells = 1usize << info.power;
                    terrain.vertices += (cells + 1).pow(2);
                    terrain.triangles += 2 * cells.pow(2);
                }
            }
            subobject::topology(part)?;
            result.objects.push(ObjectId::Solid(part.id));
        }
        terrain.output_power = terrain
            .face_powers
            .first()
            .copied()
            .filter(|power| terrain.face_powers.iter().all(|other| other == power));
        result.cuts.push(clip::Cut {
            source: source.solid,
            solids: group.parts.iter().map(|part| part.id).collect(),
        });
        result.terrain.push(terrain);
        if group.parts.is_empty() {
            removed.push((source.solid, group.owner));
        } else {
            survivors.extend(
                group
                    .parts
                    .into_iter()
                    .map(|part| (source.solid, group.owner, part)),
            );
        }
    }
    if !survivors.is_empty() {
        subobject::fragments::install(tx, survivors)?;
    }
    for (source, owner) in removed {
        match owner {
            ObjectId::Solid(_) => {
                tx.remove(ObjectId::Solid(source))?;
            }
            ObjectId::Entity(id) => tx.entity_mut(id)?.solids.retain(|solid| solid.id != source),
        }
    }
    result.objects.sort();
    result.objects.dedup();
    Ok(result)
}
