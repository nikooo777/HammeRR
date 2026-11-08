use source_formats::vmf::Vmf;

use super::*;
use crate::clip::TerrainPosition as Location;
use crate::displacement::grid_clip::{Child, Context, Work};
use crate::surface::FaceId;

#[derive(Clone)]
struct Fragment {
    solid: Solid,
    origin: Child,
    inside: bool,
    refined: bool,
}

struct Group {
    owner: ObjectId,
    source: FaceId,
    power: u32,
    parts: Vec<Fragment>,
    affected: bool,
    partitioned: bool,
    verified: (usize, usize, Option<clip::TerrainBlendError>),
}

struct Structural {
    used: BTreeSet<u32>,
    next: u32,
    donors: BTreeMap<u32, (u32, u32)>,
}

struct Virtual {
    document: Document,
    context: Option<Vmf>,
    solids: BTreeMap<(u32, usize), u32>,
    sides: BTreeMap<u32, u32>,
    textures: BTreeMap<FaceId, clip::TerrainTexture>,
}

enum Partition {
    Boundary,
    Neighbor,
}

struct Selection<'a> {
    cutters: Vec<&'a Solid>,
    targets: Vec<(ObjectId, &'a Solid)>,
    tagged: Vec<Vec<clip::TerrainCutPlane>>,
}

struct CutStep<'a> {
    source: u32,
    index: usize,
    cutter: &'a Side,
    cutter_id: u32,
    keep: clip::Keep,
    plane: clip::TerrainCutPlane,
}

struct Lineage<'a> {
    parent: &'a Fragment,
    source: FaceId,
    sides: &'a BTreeMap<u32, u32>,
    donor: u32,
    donors: &'a Solid,
    carriers: Option<CarrierLineage<'a>>,
    plane: Option<clip::TerrainCutPlane>,
}

struct CarrierLineage<'a> {
    document: &'a Document,
    sources: &'a BTreeMap<(usize, usize), FaceId>,
}

struct Draft {
    parts: Vec<Vec<Solid>>,
    children: Vec<Vec<Child>>,
}

impl Draft {
    fn new(groups: &BTreeMap<u32, Group>) -> Self {
        Self {
            parts: groups
                .values()
                .map(|group| group.parts.iter().map(|part| part.solid.clone()).collect())
                .collect(),
            children: groups
                .values()
                .map(|group| group.parts.iter().map(|part| part.origin).collect())
                .collect(),
        }
    }

    fn replacements<'a>(
        &'a self,
        groups: &BTreeMap<u32, Group>,
        mode: clip::TerrainMode,
    ) -> Vec<clip::TerrainReplacement<'a>> {
        groups
            .values()
            .zip(&self.parts)
            .zip(&self.children)
            .map(|((group, parts), children)| clip::TerrainReplacement {
                source: group.source,
                source_power: group.power,
                parts,
                children: (mode == clip::TerrainMode::Grid).then_some(children.as_slice()),
                window_fields: None,
                cut: None,
                mask: None,
                subtract: true,
                changed: group.affected || group.partitioned,
            })
            .collect()
    }
}

struct ClosureRequest<'a> {
    removed: &'a [Vec<clip::TerrainCutPlane>],
    ordinary: usize,
    needed: Option<&'a mut BTreeSet<FaceId>>,
}

struct Region<'a> {
    sides: Vec<(u32, &'a Side)>,
    planes: Vec<clip::TerrainCutPlane>,
    support: Option<clip::TerrainCutPlane>,
}

fn regions<'a>(
    selection: &Selection<'a>,
    work: &mut Work,
) -> std::result::Result<Vec<Region<'a>>, EditError> {
    let mut donors = BTreeMap::new();
    for (cutter, planes) in selection.cutters.iter().zip(&selection.tagged) {
        work.consume(planes.len()).map_err(invalid)?;
        for (side, plane) in cutter.sides.iter().zip(planes) {
            donors.entry(plane.source).or_insert((cutter.id, side));
        }
    }
    let mut regions = selection
        .cutters
        .iter()
        .zip(&selection.tagged)
        .map(|(cutter, planes)| Region {
            sides: cutter.sides.iter().map(|side| (cutter.id, side)).collect(),
            planes: planes.clone(),
            support: None,
        })
        .collect::<Vec<_>>();
    for facet in clip::terrain_union_facets(&selection.tagged, work)? {
        work.consume(facet.bounds.len()).map_err(invalid)?;
        let mut sides = Vec::new();
        for bound in &facet.bounds {
            let donor = donors
                .get(&bound.source)
                .copied()
                .ok_or_else(|| invalid("Cutter union facet lost its original side provenance"))?;
            sides.push(donor);
        }
        regions.push(Region {
            sides,
            planes: facet.bounds,
            support: Some(facet.plane),
        });
    }
    Ok(regions)
}

fn selection<'a>(
    document: &'a Document,
    cutters: &[u32],
    targets: &[u32],
    work: &mut Work,
) -> std::result::Result<Selection<'a>, EditError> {
    document
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    let targets = targets.iter().copied().collect::<BTreeSet<_>>();
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(invalid("Carve between 1 and 16 target brushes at a time"));
    }
    let has_terrain = targets.iter().any(|id| {
        subobject::solid(document, *id)
            .is_ok_and(|(_, solid)| solid.sides.iter().any(|side| side.displacement.is_some()))
    });
    let ids = cutters.iter().copied().collect::<BTreeSet<_>>();
    if cutters.is_empty() || cutters.len() > MAX_CUTTERS || ids.len() != cutters.len() {
        return Err(invalid("Choose between 1 and 8 distinct cutter brushes"));
    }
    let mut cutters = Vec::new();
    let mut tagged = Vec::new();
    for id in ids {
        if targets.contains(&id) {
            return Err(invalid("A cutter cannot also be a target brush"));
        }
        let (_, solid) = subobject::solid(document, id)?;
        subobject::topology(solid)?;
        let geometry =
            BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Cutter {id}: terrain has no closed native volume and cannot be a cutter"
            )));
        }
        if has_terrain {
            let cutter_planes = solid
                .sides
                .iter()
                .enumerate()
                .map(|(index, side)| {
                    let face = geometry
                        .faces
                        .iter()
                        .find(|face| face.side == index)
                        .ok_or_else(|| invalid("Cutter lost its effective source face"))?;
                    clip::terrain_cut_plane(
                        solid,
                        face,
                        FaceId {
                            solid: id,
                            side: side.id,
                        },
                        work,
                    )
                })
                .collect::<std::result::Result<Vec<_>, EditError>>()?;
            tagged.push(cutter_planes);
        }
        cutters.push(solid);
    }
    let targets = targets
        .into_iter()
        .map(|id| {
            let (owner, solid) = subobject::solid(document, id)?;
            subobject::topology(solid)?;
            terrain_face(solid)?;
            Ok((owner, solid))
        })
        .collect::<std::result::Result<Vec<_>, EditError>>()?;
    Ok(Selection {
        cutters,
        targets,
        tagged,
    })
}

fn terrain_face(solid: &Solid) -> std::result::Result<Option<(FaceId, u32)>, EditError> {
    let faces = solid
        .sides
        .iter()
        .filter_map(|side| side.displacement.as_ref().map(|info| (side, info)))
        .collect::<Vec<_>>();
    match faces.as_slice() {
        [] => Ok(None),
        [(side, info)] if (2..=4).contains(&info.power) => Ok(Some((
            FaceId {
                solid: solid.id,
                side: side.id,
            },
            info.power,
        ))),
        [(_, _)] => Err(invalid(format!(
            "Brush {}: terrain power must be 2, 3 or 4",
            solid.id
        ))),
        _ => Err(invalid(format!(
            "Brush {}: terrain carving requires one displaced face per brush",
            solid.id
        ))),
    }
}

fn shape(solid: &Solid) -> std::result::Result<BrushGeometry, EditError> {
    subobject::topology(solid)?;
    BrushGeometry::from_solid(solid)
        .map_err(|error| invalid(format!("Brush {}: {error}", solid.id)))
}

fn location(
    solid: &Solid,
    plane: Plane,
    work: &mut Work,
) -> std::result::Result<Location, EditError> {
    let shape = shape(solid)?;
    clip::classify_terrain(solid, &shape, plane, work)
}

fn region_overlap(
    solid: &Solid,
    region: &Region<'_>,
    work: &mut Work,
) -> std::result::Result<bool, EditError> {
    let shape = shape(solid)?;
    if let Some(support) = region.support
        && !clip::terrain_on_plane(solid, &shape, &support, work)?
    {
        return Ok(false);
    }
    clip::terrain_intersects(solid, &shape, std::slice::from_ref(&region.planes), work)
}

fn count_parts(
    groups: &BTreeMap<u32, Group>,
    ordinary: usize,
    active_source: Option<u32>,
) -> std::result::Result<(), EditError> {
    let mut patches = 0usize;
    let mut vertices = 0usize;
    let mut triangles = 0usize;
    for group in groups.values() {
        if !group.affected && !group.partitioned && active_source != Some(group.source.solid) {
            continue;
        }
        patches += group.parts.len();
        for part in &group.parts {
            let (_, power) = terrain_face(&part.solid)?
                .ok_or_else(|| invalid("Terrain carving lost its displaced source surface"))?;
            let cells = 1usize
                .checked_shl(power)
                .ok_or_else(|| invalid("Invalid terrain power"))?;
            vertices = vertices
                .checked_add((cells + 1).pow(2))
                .ok_or_else(|| invalid("Terrain carving vertex accounting overflow"))?;
            triangles = triangles
                .checked_add(2 * cells.pow(2))
                .ok_or_else(|| invalid("Terrain carving triangle accounting overflow"))?;
        }
    }
    if ordinary + patches > MAX_FRAGMENTS {
        return Err(invalid(
            "Carving would exceed 256 fragments at an intermediate stage; reduce the selection",
        ));
    }
    if patches > clip::MAX_TERRAIN_PATCHES
        || vertices > clip::MAX_TERRAIN_VERTICES
        || triangles > clip::MAX_TERRAIN_TRIANGLES
    {
        return Err(invalid(
            "Terrain carving exceeds its patch, vertex or triangle limit",
        ));
    }
    Ok(())
}

fn replace(
    vmf: &mut Vmf,
    source: u32,
    replacements: &[Solid],
) -> std::result::Result<(), EditError> {
    if let Some(index) = vmf.world.solids.iter().position(|solid| solid.id == source) {
        vmf.world
            .solids
            .splice(index..=index, replacements.iter().cloned());
        return Ok(());
    }
    for entity in &mut vmf.entities {
        if let Some(index) = entity.solids.iter().position(|solid| solid.id == source) {
            entity
                .solids
                .splice(index..=index, replacements.iter().cloned());
            return Ok(());
        }
    }
    Err(invalid(format!(
        "Terrain context lost selected source {source}"
    )))
}

fn virtual_document(
    document: &Document,
    groups: &BTreeMap<u32, Group>,
    options: &clip::ClipOptions<'_>,
    work: &mut Work,
) -> std::result::Result<Virtual, EditError> {
    let originals = document.vmf();
    let context = options.terrain_context;
    let mut object_ids = originals.object_ids().collect::<BTreeSet<_>>();
    let mut side_ids = originals.side_ids().collect::<BTreeSet<_>>();
    side_ids.extend(
        groups
            .values()
            .flat_map(|group| &group.parts)
            .flat_map(|part| &part.solid.sides)
            .map(|side| side.id),
    );
    if let Some(context) = context {
        object_ids.extend(context.object_ids());
        side_ids.extend(context.side_ids());
    }
    work.consume(object_ids.len() + side_ids.len())
        .map_err(invalid)?;
    let mut next_object = 1u32;
    let mut next_side = 1u32;
    let mut vmf = originals.clone();
    let mut resolved = context.cloned();
    let mut solids = BTreeMap::new();
    let mut sides = BTreeMap::new();
    let mut textures = BTreeMap::new();
    for (&source, group) in groups {
        let mut replacements = Vec::new();
        for (index, fragment) in group.parts.iter().enumerate() {
            let mut part = fragment.solid.clone();
            if index == 0 {
                part.id = source;
            } else {
                part.id = unused_id(&mut next_object, &mut object_ids)?;
            }
            solids.insert((source, index), part.id);
            for side in &mut part.sides {
                let lineage = side.id;
                side.id = unused_id(&mut next_side, &mut side_ids)?;
                sides.insert(side.id, lineage);
                if side.displacement.is_some()
                    && let Some(texture) = options
                        .textures
                        .and_then(|textures| textures.get(&group.source))
                {
                    textures.insert(
                        FaceId {
                            solid: part.id,
                            side: side.id,
                        },
                        *texture,
                    );
                }
            }
            replacements.push(part);
        }
        work.consume(replacements.iter().map(|part| 1 + part.sides.len()).sum())
            .map_err(invalid)?;
        replace(&mut vmf, source, &replacements)?;
        if let Some(resolved) = &mut resolved {
            replace(resolved, source, &replacements)?;
        }
    }
    Ok(Virtual {
        document: Document::new(vmf),
        context: resolved,
        solids,
        sides,
        textures,
    })
}

fn unused_id(next: &mut u32, used: &mut BTreeSet<u32>) -> std::result::Result<u32, EditError> {
    while used.contains(next) {
        *next = next
            .checked_add(1)
            .ok_or_else(|| invalid("Terrain planning IDs are exhausted"))?;
    }
    let id = *next;
    used.insert(id);
    *next = next
        .checked_add(1)
        .ok_or_else(|| invalid("Terrain planning IDs are exhausted"))?;
    Ok(id)
}

fn split_fragment(
    document: &Document,
    groups: &BTreeMap<u32, Group>,
    step: CutStep<'_>,
    structural: &mut Structural,
    options: &clip::ClipOptions<'_>,
    work: Work,
) -> std::result::Result<(Vec<Fragment>, Work), EditError> {
    let CutStep {
        source,
        index,
        cutter,
        cutter_id,
        keep,
        plane,
    } = step;
    let mut work = work;
    let virtual_map = virtual_document(document, groups, options, &mut work)?;
    let (_, solid) = subobject::solid(&virtual_map.document, virtual_map.solids[&(source, index)])?;
    let shape = shape(solid)?;
    let mut context = Context::with_work(virtual_map.context.as_ref(), work);
    let plan = clip::plan_terrain(
        &virtual_map.document,
        solid,
        &shape,
        clip::TerrainHalfRequest {
            plane: plane.plane,
            keep,
            material: &cutter.material,
        },
        &clip::ClipOptions {
            terrain_mode: options.terrain_mode,
            blend_mode: options.blend_mode,
            partition_neighbors: false,
            textures: Some(&virtual_map.textures),
            terrain_context: virtual_map.context.as_ref(),
        },
        &mut context,
    )?
    .ok_or_else(|| invalid("Authored and native terrain disagree about a cutter intersection"))?;
    let parts = restore_parts(
        plan,
        Lineage {
            parent: &groups[&source].parts[index],
            source: groups[&source].source,
            sides: &virtual_map.sides,
            donor: cutter.id,
            donors: subobject::solid(document, cutter_id)?.1,
            carriers: None,
            plane: Some(plane),
        },
        structural,
        &mut context.work,
    )?;
    Ok((parts, context.into_work()))
}

fn restore_parts(
    plan: clip::TerrainPlan,
    lineage: Lineage<'_>,
    structural: &mut Structural,
    work: &mut Work,
) -> std::result::Result<Vec<Fragment>, EditError> {
    if plan
        .children
        .as_ref()
        .is_some_and(|children| children.len() != plan.parts.len())
    {
        return Err(invalid("Terrain clipping lost its child grid origins"));
    }
    let mut parts = Vec::new();
    for (index, mut solid) in plan.parts.into_iter().enumerate() {
        solid.id = lineage.source.solid;
        for (side_index, side) in solid.sides.iter_mut().enumerate() {
            if side.id == 0 {
                if side.displacement.is_some() {
                    return Err(invalid(
                        "Generated terrain carrier side cannot become a displaced source",
                    ));
                }
                side.id = unused_id(&mut structural.next, &mut structural.used)?;
                let (family, preferred) = if let Some(carriers) = &lineage.carriers
                    && let Some(source) = carriers.sources.get(&(index, side_index))
                {
                    let (_, family) = subobject::solid(carriers.document, source.solid)?;
                    if !family.sides.iter().any(|side| side.id == source.side) {
                        return Err(invalid(
                            "Terrain carrier lost its original cutter surface donor",
                        ));
                    }
                    (family, source.side)
                } else {
                    (lineage.donors, lineage.donor)
                };
                let donor = carrier_donor(side, family, preferred, work)?;
                structural.donors.insert(side.id, (family.id, donor));
            } else {
                side.id = *lineage
                    .sides
                    .get(&side.id)
                    .ok_or_else(|| invalid("Terrain carving lost its source side lineage"))?;
            }
        }
        if terrain_face(&solid)?.map(|(face, _)| face) != Some(lineage.source) {
            return Err(invalid(
                "Terrain clipping changed its original displaced-face lineage",
            ));
        }
        let inside = if let Some(plane) = lineage.plane {
            match location(&solid, plane.plane, work)? {
                Location::Back => true,
                Location::Front | Location::Coplanar => false,
                Location::Crossing => {
                    return Err(invalid(
                        "Terrain clipping retained a patch across the cutter plane",
                    ));
                }
            }
        } else {
            lineage.parent.inside
        };
        let origin = if let Some(children) = &plan.children {
            let child = children
                .get(index)
                .ok_or_else(|| invalid("Terrain clipping lost child grid origins"))?;
            Child {
                row: lineage.parent.origin.row + child.row,
                column: lineage.parent.origin.column + child.column,
                power: child.power,
            }
        } else {
            lineage.parent.origin
        };
        parts.push(Fragment {
            solid,
            origin,
            inside,
            refined: lineage.plane.is_none() || lineage.parent.refined,
        });
    }
    Ok(parts)
}

fn partition_fragment(
    document: &Document,
    groups: &BTreeMap<u32, Group>,
    key: (u32, usize),
    kind: Partition,
    structural: &mut Structural,
    options: &clip::ClipOptions<'_>,
    work: Work,
) -> std::result::Result<(Vec<Fragment>, Work), EditError> {
    let (source, index) = key;
    let mut work = work;
    let virtual_map = virtual_document(document, groups, options, &mut work)?;
    let (_, solid) = subobject::solid(&virtual_map.document, virtual_map.solids[&key])?;
    let shape = shape(solid)?;
    let face = terrain_face(solid)?
        .ok_or_else(|| invalid("Terrain partition lost its displaced face"))?
        .0;
    let material = &solid
        .sides
        .iter()
        .find(|side| side.id == face.side)
        .unwrap()
        .material;
    let mut context = Context::with_work(virtual_map.context.as_ref(), work);
    let partition = match kind {
        Partition::Boundary => clip::plan_terrain_boundary_partition,
        Partition::Neighbor => clip::plan_terrain_partition,
    };
    let plan = partition(
        &virtual_map.document,
        solid,
        &shape,
        material,
        &clip::ClipOptions {
            terrain_mode: options.terrain_mode,
            blend_mode: options.blend_mode,
            partition_neighbors: false,
            textures: Some(&virtual_map.textures),
            terrain_context: virtual_map.context.as_ref(),
        },
        &mut context,
    )?;
    let group = &groups[&source];
    let (_, original) = subobject::solid(document, source)?;
    let parts = restore_parts(
        plan,
        Lineage {
            parent: &group.parts[index],
            source: group.source,
            sides: &virtual_map.sides,
            donor: group.source.side,
            donors: original,
            carriers: None,
            plane: None,
        },
        structural,
        &mut context.work,
    )?;
    Ok((parts, context.into_work()))
}

fn carrier_donor(
    geometry: &Side,
    source: &Solid,
    preferred: u32,
    work: &mut Work,
) -> std::result::Result<u32, EditError> {
    let normal = Plane::from_points(geometry.plane.0)
        .ok_or_else(|| invalid("Terrain carrier has no valid plane"))?
        .normal;
    work.consume(source.sides.len()).map_err(invalid)?;
    if let Some(side) = source.sides.iter().find(|side| side.id == preferred)
        && crate::surface::validate_projection(side, normal, &[]).is_ok()
    {
        return Ok(side.id);
    }
    let shape = BrushGeometry::from_solid(source).map_err(|error| invalid(error.to_string()))?;
    work.consume(shape.faces.len()).map_err(invalid)?;
    let mut best = None;
    for (index, side) in source.sides.iter().enumerate() {
        if crate::surface::validate_projection(side, normal, &[]).is_err() {
            continue;
        }
        let alignment = shape
            .faces
            .iter()
            .find(|face| face.side == index)
            .ok_or_else(|| invalid("Terrain carrier donor lost its effective original face"))?
            .plane
            .normal
            .dot(normal);
        if best.is_none_or(|(_, score)| alignment > score) {
            best = Some((side.id, alignment));
        }
    }
    best.map(|(side, _)| side).ok_or_else(|| invalid("No original surface projection can map a generated terrain carrier face; adjust the source projections"))
}

fn mixed_boundary(
    solid: &Solid,
    planes: &[Vec<clip::TerrainCutPlane>],
    work: &mut Work,
) -> std::result::Result<bool, EditError> {
    let shape = shape(solid)?;
    for plane in planes.iter().flatten() {
        if clip::terrain_has_plane_area(solid, &shape, plane, work)?
            && !clip::terrain_on_plane(solid, &shape, plane, work)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn partition_boundaries(
    document: &Document,
    groups: &mut BTreeMap<u32, Group>,
    selection: &Selection<'_>,
    options: &clip::ClipOptions<'_>,
    structural: &mut Structural,
    mut work: Work,
    ordinary: usize,
) -> std::result::Result<Work, EditError> {
    for source in groups.keys().copied().collect::<Vec<_>>() {
        let original = &groups[&source].parts[0].solid;
        let original_shape = shape(original)?;
        if !clip::terrain_intersects(original, &original_shape, &selection.tagged, &mut work)? {
            continue;
        }
        let mut index = 0;
        while index < groups[&source].parts.len() {
            let part = &groups[&source].parts[index];
            if !mixed_boundary(&part.solid, &selection.tagged, &mut work)? {
                index += 1;
                continue;
            }
            if options.terrain_mode == clip::TerrainMode::Flat
                || (part.refined && options.terrain_mode != clip::TerrainMode::Grid)
            {
                return Err(invalid(
                    "Terrain has a partly coplanar cutter boundary; choose Retile surface triangles with a compatible native surface",
                ));
            }
            let (parts, remaining_work) = partition_fragment(
                document,
                groups,
                (source, index),
                Partition::Boundary,
                structural,
                options,
                work,
            )?;
            work = remaining_work;
            if parts.len() <= 1
                && parts
                    .first()
                    .is_some_and(|part| part.solid == groups[&source].parts[index].solid)
            {
                return Err(invalid(
                    "Terrain boundary partitioning cannot refine this native patch",
                ));
            }
            groups
                .get_mut(&source)
                .unwrap()
                .parts
                .splice(index..=index, parts);
            count_parts(groups, ordinary, Some(source))?;
        }
    }
    Ok(work)
}

fn subtract_batch(
    document: &Document,
    groups: &mut BTreeMap<u32, Group>,
    selection: &Selection<'_>,
    options: &clip::ClipOptions<'_>,
    structural: &mut Structural,
    mut work: Work,
    ordinary: usize,
) -> std::result::Result<Work, EditError> {
    if groups.is_empty() {
        return Ok(work);
    }
    let virtual_map = virtual_document(document, groups, options, &mut work)?;
    let sources = selection
        .tagged
        .iter()
        .flatten()
        .map(|plane| plane.source)
        .collect::<BTreeSet<_>>();
    let mut context = Context::with_work(virtual_map.context.as_ref(), work);
    context.work.consume(sources.len()).map_err(invalid)?;
    let batch_options = clip::ClipOptions {
        terrain_mode: options.terrain_mode,
        blend_mode: options.blend_mode,
        partition_neighbors: false,
        textures: Some(&virtual_map.textures),
        terrain_context: virtual_map.context.as_ref(),
    };
    for source in groups.keys().copied().collect::<Vec<_>>() {
        let (_, solid) = subobject::solid(&virtual_map.document, virtual_map.solids[&(source, 0)])?;
        let shape = shape(solid)?;
        let Some(batch) = clip::plan_terrain_subtraction(
            &virtual_map.document,
            solid,
            &shape,
            &selection.tagged,
            &batch_options,
            &mut context,
        )?
        else {
            continue;
        };
        batch.plan.validate_blend(options.blend_mode)?;
        context
            .work
            .consume(batch.carriers.len())
            .map_err(invalid)?;
        for (&(part, side), donor) in &batch.carriers {
            let side = batch
                .plan
                .parts
                .get(part)
                .and_then(|part| part.sides.get(side))
                .ok_or_else(|| invalid("Terrain batch lost a carrier surface-settings source"))?;
            if side.id != 0 || side.displacement.is_some() || !sources.contains(donor) {
                return Err(invalid(
                    "Terrain batch carrier settings must come from an original cutter face",
                ));
            }
        }
        let (_, original) = subobject::solid(document, source)?;
        let group = &groups[&source];
        let mut parts = restore_parts(
            batch.plan,
            Lineage {
                parent: &group.parts[0],
                source: group.source,
                sides: &virtual_map.sides,
                donor: group.source.side,
                donors: original,
                carriers: Some(CarrierLineage {
                    document,
                    sources: &batch.carriers,
                }),
                plane: None,
            },
            structural,
            &mut context.work,
        )?;
        for part in &mut parts {
            part.inside = false;
        }
        let group = groups.get_mut(&source).unwrap();
        group.parts = parts;
        group.affected = true;
        count_parts(groups, ordinary, Some(source))?;
    }
    Ok(context.into_work())
}

pub(super) fn plan(
    document: &Document,
    cutter_ids: &[u32],
    target_ids: &[u32],
    options: &clip::ClipOptions<'_>,
    preparation: &mut Preparation,
) -> std::result::Result<(Vec<Planned>, Vec<u32>), EditError> {
    let work = preparation.take_work()?;
    let (planned, unchanged, work) =
        plan_inner(document, cutter_ids, target_ids, options, work, None)?;
    preparation.work = Some(work);
    Ok((planned, unchanged))
}

fn plan_inner(
    document: &Document,
    cutter_ids: &[u32],
    target_ids: &[u32],
    options: &clip::ClipOptions<'_>,
    mut work: Work,
    mut needed: Option<&mut BTreeSet<FaceId>>,
) -> std::result::Result<(Vec<Planned>, Vec<u32>, Work), EditError> {
    let selection = selection(document, cutter_ids, target_ids, &mut work)?;
    let mut groups = BTreeMap::new();
    let mut planned = Vec::new();
    let mut unchanged = Vec::new();
    let mut count = 0usize;
    for &(owner, solid) in &selection.targets {
        if let Some((source, power)) = terrain_face(solid)? {
            groups.insert(
                solid.id,
                Group {
                    owner,
                    source,
                    power,
                    parts: vec![Fragment {
                        solid: solid.clone(),
                        origin: Child {
                            row: 0,
                            column: 0,
                            power,
                        },
                        inside: true,
                        refined: false,
                    }],
                    affected: false,
                    partitioned: false,
                    verified: (0, 0, None),
                },
            );
        } else {
            let (parts, affected) = ordinary_parts(solid, &selection.cutters, count)?;
            if affected {
                count += parts.len();
                planned.push(Planned {
                    owner,
                    source: solid.id,
                    parts,
                    terrain: None,
                    structural: BTreeMap::new(),
                });
            } else {
                unchanged.push(solid.id);
            }
        }
    }
    if groups.len() > clip::MAX_TERRAIN_SOURCES {
        return Err(invalid("Terrain carving exceeds its source limit"));
    }
    let mut structural = Structural {
        used: document
            .vmf()
            .side_ids()
            .chain(options.terrain_context.into_iter().flat_map(Vmf::side_ids))
            .collect(),
        next: 1,
        donors: BTreeMap::new(),
    };
    if matches!(
        options.terrain_mode,
        clip::TerrainMode::Flat | clip::TerrainMode::Fragments
    ) {
        work = subtract_batch(
            document,
            &mut groups,
            &selection,
            options,
            &mut structural,
            work,
            count,
        )?;
    } else {
        let regions = regions(&selection, &mut work)?;
        work = partition_boundaries(
            document,
            &mut groups,
            &selection,
            options,
            &mut structural,
            work,
            count,
        )?;
        for &source in &groups.keys().copied().collect::<Vec<_>>() {
            for region in &regions {
                let planes = &region.planes;
                let before = groups[&source].parts.clone();
                let potential = before
                    .iter()
                    .map(|part| region_overlap(&part.solid, region, &mut work))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                if !potential.iter().any(|overlap| *overlap) {
                    continue;
                }
                for (part, overlap) in groups
                    .get_mut(&source)
                    .unwrap()
                    .parts
                    .iter_mut()
                    .zip(potential)
                {
                    part.inside = overlap;
                }
                let mut removed = false;
                for (plane_index, &(cutter_id, side)) in region.sides.iter().enumerate() {
                    let plane = planes[plane_index];
                    let mut index = 0usize;
                    while index < groups[&source].parts.len() {
                        if !groups[&source].parts[index].inside {
                            index += 1;
                            continue;
                        }
                        match location(&groups[&source].parts[index].solid, plane.plane, &mut work)?
                        {
                            Location::Back => index += 1,
                            Location::Front | Location::Coplanar => {
                                groups.get_mut(&source).unwrap().parts[index].inside = false;
                                index += 1;
                            }
                            Location::Crossing => {
                                let mut contained = true;
                                for later in &planes[plane_index + 1..] {
                                    if !matches!(
                                        location(
                                            &groups[&source].parts[index].solid,
                                            later.plane,
                                            &mut work
                                        )?,
                                        Location::Back
                                    ) {
                                        contained = false;
                                        break;
                                    }
                                }
                                let keep = if contained {
                                    clip::Keep::Front
                                } else {
                                    clip::Keep::Both
                                };
                                let (parts, remaining_work) = split_fragment(
                                    document,
                                    &groups,
                                    CutStep {
                                        source,
                                        index,
                                        cutter: side,
                                        cutter_id,
                                        keep,
                                        plane,
                                    },
                                    &mut structural,
                                    options,
                                    work,
                                )?;
                                work = remaining_work;
                                removed |= contained;
                                let added = parts.len();
                                groups
                                    .get_mut(&source)
                                    .unwrap()
                                    .parts
                                    .splice(index..=index, parts);
                                count_parts(&groups, count, Some(source))?;
                                index += added;
                            }
                        }
                    }
                }
                let group = groups.get_mut(&source).unwrap();
                if removed || group.parts.iter().any(|part| part.inside) {
                    group.parts.retain(|part| !part.inside);
                    group.affected = true;
                } else {
                    group.parts = before;
                }
            }
            if !groups[&source].affected {
                let (_, original) = subobject::solid(document, source)?;
                let power = groups[&source].power;
                groups.get_mut(&source).unwrap().parts = vec![Fragment {
                    solid: original.clone(),
                    origin: Child {
                        row: 0,
                        column: 0,
                        power,
                    },
                    inside: true,
                    refined: false,
                }];
            }
        }
    }
    let (remaining_work, complete) = close_neighbors(
        document,
        &mut groups,
        options,
        &mut structural,
        work,
        ClosureRequest {
            removed: &selection.tagged,
            ordinary: count,
            needed: needed.as_deref_mut(),
        },
    )?;
    work = remaining_work;
    if !complete || needed.is_some() {
        return Ok((Vec::new(), Vec::new(), work));
    }
    work = certify(document, &mut groups, options, &selection.tagged, work)?;
    for (source, group) in groups {
        if !group.affected && !group.partitioned {
            unchanged.push(source);
            continue;
        }
        let parts = group
            .parts
            .into_iter()
            .map(|part| part.solid)
            .collect::<Vec<_>>();
        let powers = parts
            .iter()
            .flat_map(|part| &part.sides)
            .filter_map(|side| side.displacement.as_ref().map(|info| info.power))
            .collect::<Vec<_>>();
        let output_power = powers
            .first()
            .copied()
            .filter(|power| powers.iter().all(|other| other == power));
        let vertices = powers
            .iter()
            .map(|power| ((1usize << power) + 1).pow(2))
            .sum();
        let triangles = powers
            .iter()
            .map(|power| 2 * (1usize << power).pow(2))
            .sum();
        let structural = parts
            .iter()
            .flat_map(|part| &part.sides)
            .filter_map(|side| {
                structural
                    .donors
                    .get(&side.id)
                    .copied()
                    .map(|donor| (side.id, donor))
            })
            .collect();
        planned.push(Planned {
            owner: group.owner,
            source,
            parts,
            terrain: Some(clip::TerrainCut {
                partitioned: group.partitioned,
                source: group.source,
                faces: Vec::new(),
                source_power: group.power,
                output_power,
                face_powers: Vec::new(),
                vertices,
                triangles,
                verified_neighbors: group.verified.0,
                preserved_links: group.verified.1,
                blend_error: group.verified.2,
            }),
            structural,
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
    planned.sort_by_key(|part| part.source);
    unchanged.sort_unstable();
    Ok((planned, unchanged, work))
}

pub(super) fn validate_options(
    options: &clip::ClipOptions<'_>,
) -> std::result::Result<(), EditError> {
    if options.partition_neighbors && !options.terrain_mode.allows_neighbor_partition() {
        return Err(invalid(
            "Partition selected neighbors is unavailable for Retile flat terrain",
        ));
    }
    if options.blend_mode != clip::TerrainBlendMode::Exact
        && options.terrain_mode != clip::TerrainMode::Fragments
    {
        return Err(invalid(
            "Native float and nearest-byte blend samples require Retile surface triangles",
        ));
    }
    if let Some(context) = options.terrain_context {
        context
            .validate_ids()
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(())
}

fn certify(
    document: &Document,
    groups: &mut BTreeMap<u32, Group>,
    options: &clip::ClipOptions<'_>,
    removed: &[Vec<clip::TerrainCutPlane>],
    work: Work,
) -> std::result::Result<Work, EditError> {
    let draft = Draft::new(groups);
    let replacements = draft.replacements(groups, options.terrain_mode);
    if replacements.is_empty() {
        return Ok(work);
    }
    let mut context = Context::with_work(options.terrain_context, work);
    let verified = clip::certify_terrain(
        document,
        &replacements,
        options,
        &mut context,
        Some(removed),
    )?;
    if verified.len() != groups.len() {
        return Err(invalid("Terrain carving lost a final source certificate"));
    }
    for (group, verified) in groups.values_mut().zip(verified) {
        group.verified = verified;
    }
    Ok(context.into_work())
}

fn close_neighbors(
    document: &Document,
    groups: &mut BTreeMap<u32, Group>,
    options: &clip::ClipOptions<'_>,
    structural: &mut Structural,
    mut work: Work,
    mut request: ClosureRequest<'_>,
) -> std::result::Result<(Work, bool), EditError> {
    if !groups.values().any(|group| group.affected) {
        return Ok((work, true));
    }
    if options.terrain_mode == clip::TerrainMode::Grid {
        if !options.partition_neighbors {
            return Ok((work, true));
        }
        let draft = Draft::new(groups);
        let replacements = draft.replacements(groups, options.terrain_mode);
        let authorized = groups
            .values()
            .map(|group| {
                let (owner, solid) = subobject::solid(document, group.source.solid)?;
                Ok(clip::Source {
                    solid,
                    owner,
                    target: ObjectId::Solid(solid.id),
                })
            })
            .collect::<std::result::Result<Vec<_>, EditError>>()?;
        let mut context = Context::with_work(options.terrain_context, work);
        let plans = clip::partition_terrain_neighbors(
            document,
            &authorized,
            &replacements,
            None,
            options,
            &mut context,
            request.removed,
        )?;
        for plan in plans {
            let group = groups.get_mut(&plan.source.solid).ok_or_else(|| {
                invalid(
                    "Select the required terrain neighbor as a target to authorize its partition",
                )
            })?;
            if plan.source != group.source {
                return Err(invalid(
                    "Terrain neighbor partition changed its original face identity",
                ));
            }
            let (_, original) = subobject::solid(document, group.source.solid)?;
            let parent = Fragment {
                solid: original.clone(),
                origin: Child {
                    row: 0,
                    column: 0,
                    power: group.power,
                },
                inside: false,
                refined: false,
            };
            let sides = plan
                .parts
                .iter()
                .flat_map(|part| &part.sides)
                .filter(|side| side.id != 0)
                .map(|side| (side.id, side.id))
                .collect();
            group.parts = restore_parts(
                plan,
                Lineage {
                    parent: &parent,
                    source: group.source,
                    sides: &sides,
                    donor: group.source.side,
                    donors: original,
                    carriers: None,
                    plane: None,
                },
                structural,
                &mut context.work,
            )?;
            group.partitioned = !group.affected;
        }
        count_parts(groups, request.ordinary, None)?;
        return Ok((context.into_work(), true));
    }
    loop {
        let draft = Draft::new(groups);
        let replacements = draft.replacements(groups, options.terrain_mode);
        let mut context = Context::with_work(options.terrain_context, work);
        let required = clip::required_terrain_partitions(document, &replacements, &mut context)?
            .into_iter()
            .filter(|required| required.internal || options.partition_neighbors)
            .collect::<Vec<_>>();
        work = context.into_work();
        if required.is_empty() {
            return Ok((work, true));
        }
        let mut missing = BTreeSet::new();
        let mut parts = BTreeMap::<FaceId, BTreeSet<usize>>::new();
        for required in &required {
            let face = &required.source;
            let group = groups.get(&face.solid).ok_or_else(|| invalid(format!("Select solid:{} side:{} as a target to authorize terrain neighbor partitioning", face.solid, face.side)))?;
            if group.source != *face {
                return Err(invalid(
                    "Terrain neighbor partition lost its original face identity",
                ));
            }
            if required.part >= group.parts.len() {
                return Err(invalid(
                    "Terrain subdivision lost its current retained part",
                ));
            }
            parts.entry(*face).or_default().insert(required.part);
            if options
                .textures
                .is_none_or(|textures| !textures.contains_key(face))
            {
                missing.insert(*face);
            }
        }
        if !missing.is_empty() {
            if let Some(needed) = request.needed.as_deref_mut() {
                needed.extend(missing);
                return Ok((work, false));
            }
            return Err(invalid(
                "Required terrain neighbor partition needs guarded native texture dimensions",
            ));
        }
        for (face, indices) in parts {
            for index in indices.into_iter().rev() {
                let (parts, remaining_work) = partition_fragment(
                    document,
                    groups,
                    (face.solid, index),
                    Partition::Neighbor,
                    structural,
                    options,
                    work,
                )?;
                work = remaining_work;
                let group = groups.get_mut(&face.solid).unwrap();
                group.parts.splice(index..=index, parts);
                group.partitioned = !group.affected;
                count_parts(groups, request.ordinary, Some(face.solid))?;
            }
        }
    }
}

pub(super) fn texture_faces(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    options: &clip::ClipOptions<'_>,
    preparation: &mut Preparation,
) -> std::result::Result<Vec<FaceId>, EditError> {
    validate_options(options)?;
    let work = preparation.work()?;
    let selection = selection(document, cutters, targets, work)?;
    if options.terrain_mode == clip::TerrainMode::Grid {
        return Ok(Vec::new());
    }
    let mut faces = BTreeSet::new();
    for (_, source) in selection.targets {
        let Some((face, _)) = terrain_face(source)? else {
            continue;
        };
        if clip::terrain_intersects(source, &shape(source)?, &selection.tagged, work)? {
            faces.insert(face);
        }
    }
    Ok(faces.into_iter().collect())
}

pub(super) fn partition_faces(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    options: &clip::ClipOptions<'_>,
    preparation: &mut Preparation,
) -> std::result::Result<Vec<FaceId>, EditError> {
    validate_options(options)?;
    preparation.work()?;
    if !options.partition_neighbors || options.terrain_mode == clip::TerrainMode::Grid {
        return Ok(Vec::new());
    }
    let mut needed = BTreeSet::new();
    let work = preparation.take_work()?;
    let (_, _, work) = plan_inner(document, cutters, targets, options, work, Some(&mut needed))?;
    preparation.work = Some(work);
    Ok(needed.into_iter().collect())
}

pub(super) fn requires_context(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
    _mode: clip::TerrainMode,
) -> std::result::Result<bool, EditError> {
    let selection = selection(document, cutters, targets, &mut Work::default())?;
    let instances = document.vmf().entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    });
    if !instances {
        return Ok(false);
    }
    for (_, solid) in selection.targets {
        if terrain_face(solid)?.is_some()
            && crate::displacement::native::terrain_source_is_visible(document.vmf(), solid.id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
