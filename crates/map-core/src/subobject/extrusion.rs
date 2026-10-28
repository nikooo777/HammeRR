use super::*;
use source_formats::vmf::{Properties, Side, UvAxis};

pub const MAX_EXTRUSION_PIECES: usize = MAX_SOLIDS * 2;
pub const MAX_EXTRUSION_SOURCE_VERTICES: usize = MAX_SOLIDS * MAX_PARTITION_VERTICES;
pub const MAX_EXTRUSION_PLANES: usize = MAX_SOLIDS * (MAX_BOUNDARY_SOURCE_CORNERS + 8);
pub const MAX_EXTRUSION_FRAGMENTS: usize = crate::carve::MAX_FRAGMENTS;

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionSpec {
    pub height: f64,
    pub wall_material: Option<String>,
    pub texture_lock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrusionSurface {
    Source,
    Top,
    Base,
    Boundary,
    Divider,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionFace {
    pub side: u32,
    pub source_side: u32,
    pub partition: Option<u32>,
    pub surface: ExtrusionSurface,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionPiece {
    pub source: Crease,
    pub partition: u32,
    pub owner: ObjectId,
    pub solid: Solid,
    pub faces: Vec<ExtrusionFace>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionPlan {
    pub pieces: Vec<ExtrusionPiece>,
    pub fragments: Vec<ExtrusionFragment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionFragment {
    pub source: u32,
    pub owner: ObjectId,
    pub solid: Solid,
    pub faces: Vec<ExtrusionFace>,
}

fn incidence(vertex: &PartitionVertex) -> BTreeSet<u32> {
    match &vertex.source {
        VertexSource::Corner(sides) => sides.iter().copied().collect(),
        VertexSource::Edge { corners, .. } => corners[0]
            .iter()
            .filter(|side| corners[1].contains(side))
            .copied()
            .collect(),
    }
}

fn side_on_plane(template: &Side, id: u32, points: [DVec3; 3]) -> Side {
    let mut side = template.clone();
    side.id = id;
    side.plane = source_formats::vmf::Plane(points);
    side
}

pub fn plan_partition_extrusion(
    document: &Document,
    partitions: &[Partition],
    selection: &[(u32, u32)],
    spec: &ExtrusionSpec,
) -> Result<ExtrusionPlan, EditError> {
    plan_partition_extrusion_with_materials(document, partitions, selection, spec, &[])
}

pub fn plan_partition_extrusion_with_materials(
    document: &Document,
    partitions: &[Partition],
    selection: &[(u32, u32)],
    spec: &ExtrusionSpec,
    materials: &[PartitionMaterial],
) -> Result<ExtrusionPlan, EditError> {
    plan_partition_extrusion_with_surfaces(document, partitions, selection, spec, materials, &[])
}

pub fn plan_partition_extrusion_with_surfaces(
    document: &Document,
    partitions: &[Partition],
    selection: &[(u32, u32)],
    spec: &ExtrusionSpec,
    materials: &[PartitionMaterial],
    textures: &[PieceTexture],
) -> Result<ExtrusionPlan, EditError> {
    if partitions.is_empty() || partitions.len() > MAX_SOLIDS {
        return Err(invalid("Extrude flat partitions in 1–16 source brushes"));
    }
    if !spec.height.is_finite() || !(MIN_SPLIT_RISE..=MAX_SPLIT_RISE).contains(&spec.height.abs()) {
        return Err(invalid(
            "Extrusion depth must have magnitude 0.125–4096 units; positive adds outward, negative cuts inward",
        ));
    }
    if spec
        .wall_material
        .as_ref()
        .is_some_and(|material| material.trim().is_empty())
    {
        return Err(invalid(
            "Choose a nonempty wall material, or inherit boundary materials",
        ));
    }
    if selection.is_empty() || selection.len() > MAX_EXTRUSION_PIECES {
        return Err(invalid("Select 1–32 flat pieces to extrude"));
    }
    let mut source_vertices = 0;
    for part in partitions {
        super::partition::check_partition_bounds(part)?;
        source_vertices += part.vertices.len();
    }
    if source_vertices > MAX_EXTRUSION_SOURCE_VERTICES {
        return Err(invalid(
            "Extrusion source topology exceeds its aggregate vertex budget",
        ));
    }
    let mut sources = BTreeMap::new();
    for part in partitions {
        if sources.insert(part.source.solid, part).is_some() {
            return Err(invalid("Select one partition per source brush"));
        }
    }
    let selection = selection.iter().copied().collect::<BTreeSet<_>>();
    let mut planes = 0;
    for (solid, piece) in &selection {
        let piece = sources
            .get(solid)
            .and_then(|part| part.loops.iter().find(|part| part.id == *piece))
            .ok_or_else(|| invalid("Choose pieces from the current flat partition"))?;
        planes += piece.vertices.len() + 2;
    }
    if planes > MAX_EXTRUSION_PLANES {
        return Err(invalid("Extrusion exceeds its aggregate plane budget"));
    }
    let materials = super::partition_materials::checked_materials(document, partitions, materials)?;
    let textures = super::partition_textures::checked_textures(document, partitions, textures)?;
    let mut pieces = Vec::new();
    for (solid_id, partition_id) in selection {
        let part = sources
            .get(&solid_id)
            .ok_or_else(|| invalid("Choose pieces from the current flat partition"))?;
        let piece = part
            .loops
            .iter()
            .find(|piece| piece.id == partition_id)
            .ok_or_else(|| invalid("Choose pieces from the current flat partition"))?;
        let (_, source) = solid(document, solid_id)?;
        let original = source
            .sides
            .iter()
            .find(|side| side.id == part.source.side)
            .unwrap();
        let bottom = piece
            .vertices
            .iter()
            .map(|vertex| part.vertices[*vertex as usize].position)
            .collect::<Vec<_>>();
        let top = bottom
            .iter()
            .map(|point| *point + part.normal() * spec.height)
            .collect::<Vec<_>>();
        let count = bottom.len();
        if top
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
        {
            return Err(invalid(format!(
                "Brush {solid_id}: extruded piece exceeds ±65536 units"
            )));
        }
        let mut polygons = vec![top.clone(), bottom.iter().copied().rev().collect()];
        for index in 0..count {
            let next = (index + 1) % count;
            polygons.push(vec![bottom[index], bottom[next], top[next], top[index]]);
        }
        let planes = crate::primitive::convex_planes(polygons)?;
        let mut top_side = side_on_plane(original, 1, planes[0]);
        if let Some(material) = materials.get(&(solid_id, partition_id)) {
            top_side.material.clone_from(material);
        }
        if let Some(texture) = textures.get(&(solid_id, partition_id)) {
            texture.apply(original, &mut top_side, &bottom, &top, spec.texture_lock)?;
        } else if spec.texture_lock {
            let projection =
                crate::texture_deform::attach(original, &bottom, &top).map_err(|error| {
                    invalid(format!("Brush {solid_id}, face {}: {error}", original.id))
                })?;
            top_side.u_axis = projection.u;
            top_side.v_axis = projection.v;
            top_side.rotation = projection.rotation;
        }
        let mut sides = vec![top_side, side_on_plane(original, 2, planes[1])];
        let mut faces = vec![
            ExtrusionFace {
                side: 1,
                source_side: original.id,
                partition: Some(partition_id),
                surface: ExtrusionSurface::Top,
            },
            ExtrusionFace {
                side: 2,
                source_side: original.id,
                partition: Some(partition_id),
                surface: ExtrusionSurface::Base,
            },
        ];
        for index in 0..count {
            let a = incidence(&part.vertices[piece.vertices[index] as usize]);
            let b = incidence(&part.vertices[piece.vertices[(index + 1) % count] as usize]);
            let neighbor = a
                .intersection(&b)
                .find(|side| **side != original.id)
                .copied();
            let template = neighbor
                .and_then(|id| source.sides.iter().find(|side| side.id == id))
                .unwrap_or(original);
            let id = (index + 3) as u32;
            let mut side = side_on_plane(template, id, planes[index + 2]);
            let normal = Plane::from_points(side.plane.0).unwrap().normal;
            let (u, v) = crate::edit::world_texture_axes(normal);
            side.u_axis = UvAxis {
                axis: u,
                offset: 0.0,
                scale: crate::edit::DEFAULT_TEXTURE_SCALE,
            };
            side.v_axis = UvAxis {
                axis: v,
                offset: 0.0,
                scale: crate::edit::DEFAULT_TEXTURE_SCALE,
            };
            side.rotation = 0.0;
            if let Some(material) = &spec.wall_material {
                side.material = material.trim().to_ascii_uppercase();
            }
            sides.push(side);
            faces.push(ExtrusionFace {
                side: id,
                source_side: template.id,
                partition: Some(partition_id),
                surface: if neighbor.is_some() {
                    ExtrusionSurface::Boundary
                } else {
                    ExtrusionSurface::Divider
                },
            });
        }
        let prism = Solid {
            id: 1,
            sides,
            editor: source.editor.clone(),
            hidden: source.hidden,
            extra_properties: Properties::default(),
            extra: Vec::new(),
        };
        let shape = topology(&prism)?;
        if shape.vertices.len() != count * 2
            || shape.faces.iter().any(|face| {
                let expected = if face.side <= 2 { count } else { 4 };
                face.vertices.len() != expected
            })
            || bottom.iter().chain(&top).any(|point| {
                !shape
                    .vertices
                    .iter()
                    .any(|vertex| vertex.position.distance(*point) < EPSILON)
            })
        {
            return Err(invalid(format!(
                "Brush {solid_id}: extrusion does not retain its planned corners"
            )));
        }
        pieces.push(ExtrusionPiece {
            source: part.source.clone(),
            partition: partition_id,
            owner: part.owner,
            solid: prism,
            faces,
        });
    }
    let mut plan = ExtrusionPlan {
        pieces,
        fragments: Vec::new(),
    };
    if spec.height < 0.0 {
        plan_inward(document, &mut plan)?;
    }
    Ok(plan)
}

fn plan_inward(document: &Document, plan: &mut ExtrusionPlan) -> Result<(), EditError> {
    crate::partition_metadata::Metadata::parse(document.vmf()).map_err(EditError::Invalid)?;
    let used = document.vmf().side_ids().collect::<BTreeSet<_>>();
    let mut temporary = u32::MAX;
    let mut cut_faces = BTreeMap::new();
    for piece in &mut plan.pieces {
        piece.solid.id = 0;
        // Bound each floor to its footprint before splitting at the depth plane.
        piece.solid.sides.rotate_left(2);
        piece.faces.rotate_left(2);
        for (side, face) in piece.solid.sides.iter_mut().zip(&mut piece.faces) {
            while used.contains(&temporary) {
                temporary = temporary
                    .checked_sub(1)
                    .ok_or_else(|| invalid("No temporary side IDs remain for inward extrusion"))?;
            }
            side.id = temporary;
            face.side = temporary;
            cut_faces.insert(temporary, face.clone());
            temporary = temporary
                .checked_sub(1)
                .ok_or_else(|| invalid("No temporary side IDs remain for inward extrusion"))?;
        }
    }
    let sources = plan
        .pieces
        .iter()
        .map(|piece| piece.source.solid)
        .collect::<BTreeSet<_>>();
    for id in sources {
        let (owner, source) = solid(document, id)?;
        let mut fragments = vec![source.clone()];
        for piece in plan.pieces.iter().filter(|piece| piece.source.solid == id) {
            let mut remaining = Vec::new();
            for fragment in fragments {
                if crate::carve::intersects(&fragment, &piece.solid)? {
                    remaining.extend(crate::carve::subtract(&fragment, &piece.solid)?);
                } else {
                    remaining.push(fragment);
                }
                if plan.fragments.len() + remaining.len() > MAX_EXTRUSION_FRAGMENTS {
                    return Err(invalid("Inward extrusion exceeds 256 convex fragments"));
                }
            }
            fragments = remaining;
        }
        if fragments.is_empty() {
            return Err(invalid(format!(
                "Brush {id}: inward extrusion would remove the entire source; select fewer pieces or reduce the depth"
            )));
        }
        for fragment in fragments {
            topology(&fragment)?;
            let faces = fragment
                .sides
                .iter()
                .map(|side| {
                    cut_faces.get(&side.id).cloned().unwrap_or(ExtrusionFace {
                        side: side.id,
                        source_side: side.id,
                        partition: None,
                        surface: ExtrusionSurface::Source,
                    })
                })
                .collect();
            plan.fragments.push(ExtrusionFragment {
                source: id,
                owner,
                solid: fragment,
                faces,
            });
        }
    }
    Ok(())
}

fn apply_inward(tx: &mut Transaction, plan: &mut ExtrusionPlan) -> Result<(), EditError> {
    let mut retained_solids = BTreeSet::new();
    let mut retained_sides = BTreeSet::new();
    for fragment in &mut plan.fragments {
        if !retained_solids.insert(fragment.source) {
            fragment.solid.id = tx.new_object_id()?;
        }
        for (side, face) in fragment.solid.sides.iter_mut().zip(&mut fragment.faces) {
            if face.partition.is_some() || !retained_sides.insert(side.id) {
                side.id = tx.new_side_id()?;
            }
            face.side = side.id;
        }
    }
    let mut metadata =
        crate::partition_metadata::Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    let count = metadata.entries.len();
    metadata
        .entries
        .retain(|(solid, _), _| !retained_solids.contains(solid));
    if count != metadata.entries.len() {
        metadata.write(tx)?;
    }
    for id in retained_solids {
        let fragments = plan
            .fragments
            .iter()
            .filter(|fragment| fragment.source == id)
            .collect::<Vec<_>>();
        match fragments[0].owner {
            ObjectId::Solid(_) => {
                *tx.solid_mut(id)? = fragments[0].solid.clone();
                for fragment in &fragments[1..] {
                    tx.add_solid(fragment.solid.clone())?;
                }
            }
            ObjectId::Entity(owner) => {
                let entity = tx.entity_mut(owner)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == id)
                    .unwrap();
                entity.solids.splice(
                    index..=index,
                    fragments.iter().map(|fragment| fragment.solid.clone()),
                );
            }
        }
    }
    Ok(())
}

pub fn extrude_partitions(
    tx: &mut Transaction,
    partitions: &[Partition],
    selection: &[(u32, u32)],
    spec: &ExtrusionSpec,
) -> Result<ExtrusionPlan, EditError> {
    extrude_partitions_with_materials(tx, partitions, selection, spec, &[])
}

pub fn extrude_partitions_with_materials(
    tx: &mut Transaction,
    partitions: &[Partition],
    selection: &[(u32, u32)],
    spec: &ExtrusionSpec,
    materials: &[PartitionMaterial],
) -> Result<ExtrusionPlan, EditError> {
    extrude_partitions_with_surfaces(tx, partitions, selection, spec, materials, &[])
}

pub fn extrude_partitions_with_surfaces(
    tx: &mut Transaction,
    partitions: &[Partition],
    selection: &[(u32, u32)],
    spec: &ExtrusionSpec,
    materials: &[PartitionMaterial],
    textures: &[PieceTexture],
) -> Result<ExtrusionPlan, EditError> {
    let mut plan = plan_partition_extrusion_with_surfaces(
        tx.document(),
        partitions,
        selection,
        spec,
        materials,
        textures,
    )?;
    if spec.height < 0.0 {
        apply_inward(tx, &mut plan)?;
        return Ok(plan);
    }
    for piece in &mut plan.pieces {
        piece.solid.id = tx.new_object_id()?;
        for (side, face) in piece.solid.sides.iter_mut().zip(&mut piece.faces) {
            side.id = tx.new_side_id()?;
            face.side = side.id;
        }
    }
    for piece in &plan.pieces {
        match piece.owner {
            ObjectId::Solid(_) => {
                tx.add_solid(piece.solid.clone())?;
            }
            ObjectId::Entity(id) => {
                tx.entity_mut(id)?.solids.push(piece.solid.clone());
            }
        }
    }
    Ok(plan)
}
