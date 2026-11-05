use super::*;
use source_formats::vmf::{Side, Solid};

use crate::subobject::fragments::{self, BoundaryTriangle};

pub const MAX_FREE_PARTITION_FRAGMENTS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct FreePieceSurface {
    pub solid: u32,
    pub piece: u32,
    pub material: Option<String>,
    pub texture: Option<PartitionTexture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreePartitionFace {
    pub side: u32,
    pub source_side: u32,
    pub exterior: bool,
    pub piece: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreePartitionFragment {
    pub source: u32,
    pub owner: ObjectId,
    pub solid: Solid,
    pub faces: Vec<FreePartitionFace>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreePartitionPlan {
    pub fragments: Vec<FreePartitionFragment>,
}

fn checked_surfaces<'a>(
    partitions: &[FreePartition],
    surfaces: &'a [FreePieceSurface],
) -> Result<BTreeMap<(u32, u32), &'a FreePieceSurface>, EditError> {
    let mut result = BTreeMap::new();
    for surface in surfaces {
        if !partitions.iter().any(|part| {
            part.solid == surface.solid && part.pieces.iter().any(|piece| piece.id == surface.piece)
        }) || result
            .insert((surface.solid, surface.piece), surface)
            .is_some()
        {
            return Err(invalid(
                "Free-cut surface plans need distinct existing triangle pieces",
            ));
        }
        if let Some(material) = &surface.material {
            crate::partition_metadata::checked_top_material(material).map_err(invalid)?;
        }
        if let Some(texture) = surface.texture {
            texture.validate()?;
        }
    }
    Ok(result)
}

fn surface_template(source: &Side, surface: Option<&&FreePieceSurface>) -> Result<Side, EditError> {
    let mut template = source.clone();
    if let Some(surface) = surface {
        if let Some(material) = &surface.material {
            template.material =
                crate::partition_metadata::normalize_top_material(material).map_err(invalid)?;
        }
        if let Some(texture) = &surface.texture {
            let projection = texture.project(source)?;
            template.u_axis = projection.u;
            template.v_axis = projection.v;
            template.rotation = projection.rotation;
        }
    }
    Ok(template)
}

fn point_on_segment(point: DVec3, a: DVec3, b: DVec3) -> bool {
    let direction = b - a;
    let parameter = (point - a).dot(direction) / direction.length_squared();
    let tolerance = roundoff_tolerance(point.abs().max(a.abs()).max(b.abs()).max_element());
    (0.0..=1.0).contains(&parameter) && a.lerp(b, parameter).distance(point) <= tolerance
}

fn boundary_triangles(
    source: &Solid,
    part: &FreePartition,
    surfaces: &BTreeMap<(u32, u32), &FreePieceSurface>,
) -> Result<(Vec<DVec3>, Vec<BoundaryTriangle>), EditError> {
    let shape = topology(source)?;
    let mut points = shape
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect::<Vec<_>>();
    let mut mapping = Vec::with_capacity(part.vertices.len());
    for (index, vertex) in part.vertices.iter().enumerate() {
        let index = if index < part.corners.len() {
            shape
                .vertices
                .iter()
                .position(|candidate| candidate.sides == part.corners[index])
                .ok_or_else(|| invalid("Free-cut source corner identity changed"))?
        } else {
            points.push(vertex.position);
            points.len() - 1
        };
        mapping.push(index);
    }
    let mut triangles = Vec::new();
    for face in &shape.faces {
        let side = source
            .sides
            .iter()
            .find(|side| side.id == face.side)
            .unwrap();
        if face.side == part.side {
            for piece in &part.pieces {
                triangles.push(BoundaryTriangle {
                    vertices: piece.vertices.map(|index| mapping[index as usize]),
                    template: surface_template(side, surfaces.get(&(part.solid, piece.id)))?,
                    source_side: side.id,
                    tag: Some(piece.id),
                });
            }
            continue;
        }
        let mut ring = Vec::new();
        for index in 0..face.vertices.len() {
            let a = points[face.vertices[index]];
            let next = face.vertices[(index + 1) % face.vertices.len()];
            let b = points[next];
            let mut along = points
                .iter()
                .enumerate()
                .filter(|(index, point)| *index != next && point_on_segment(**point, a, b))
                .map(|(index, point)| ((point - a).dot(b - a), index))
                .collect::<Vec<_>>();
            along.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            ring.extend(along.into_iter().map(|(_, index)| index));
        }
        let normal = Plane::from_points(side.plane.0).unwrap().normal;
        for vertices in mesh::face_triangles(&points, &ring, normal)? {
            triangles.push(BoundaryTriangle {
                vertices,
                template: side.clone(),
                source_side: side.id,
                tag: None,
            });
        }
    }
    Ok((points, triangles))
}

pub fn plan_free_partition_materialization(
    document: &Document,
    partitions: &[FreePartition],
    surfaces: &[FreePieceSurface],
) -> Result<FreePartitionPlan, EditError> {
    let mut preview = document.preview_snapshot();
    preview.edit("preview free-cut materialization", |tx| {
        materialize_free_partitions(tx, partitions, surfaces)
    })
}

pub fn materialize_free_partitions(
    tx: &mut Transaction,
    partitions: &[FreePartition],
    surfaces: &[FreePieceSurface],
) -> Result<FreePartitionPlan, EditError> {
    if partitions.is_empty() || partitions.len() > MAX_SOLIDS {
        return Err(invalid("Materialize free cuts in 1–16 source brushes"));
    }
    let mut sources = BTreeSet::new();
    for part in partitions {
        if !sources.insert(part.solid) {
            return Err(invalid(
                "Materialize free cuts on exactly one face per source brush",
            ));
        }
        checked_free_partition(tx.document(), part)?;
    }
    let surfaces = checked_surfaces(partitions, surfaces)?;
    let mut result = FreePartitionPlan {
        fragments: Vec::new(),
    };
    for part in partitions {
        let (_, source) = solid(tx.document(), part.solid)?;
        fragments::admit_source(source)?;
        let shape = topology(source)?;
        let center = shape
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .sum::<DVec3>()
            / shape.vertices.len() as f64;
        let (points, triangles) = boundary_triangles(source, part, &surfaces)?;
        let tetrahedra = fragments::tetrahedralize(
            source,
            &points,
            triangles,
            center,
            MAX_FREE_PARTITION_FRAGMENTS - result.fragments.len(),
        )?;
        result
            .fragments
            .extend(tetrahedra.into_iter().map(|fragment| {
                FreePartitionFragment {
                    source: part.solid,
                    owner: part.owner,
                    solid: fragment.solid,
                    faces: fragment
                        .faces
                        .into_iter()
                        .map(|face| FreePartitionFace {
                            side: face.side,
                            source_side: face.source_side,
                            exterior: face.exterior,
                            piece: face.tag,
                        })
                        .collect(),
                }
            }));
    }
    let mut retained_solids = BTreeSet::new();
    let mut retained_sides = BTreeSet::new();
    for fragment in &mut result.fragments {
        if !retained_solids.insert(fragment.source) {
            fragment.solid.id = tx.new_object_id()?;
        }
        for (side, face) in fragment.solid.sides.iter_mut().zip(&mut fragment.faces) {
            side.id = if face.exterior && retained_sides.insert(face.source_side) {
                face.source_side
            } else {
                tx.new_side_id()?
            };
            face.side = side.id;
        }
    }
    let mut legacy = crate::partition_metadata::Metadata::parse(tx.vmf()).map_err(invalid)?;
    let legacy_count = legacy.entries.len();
    legacy.entries.retain(|key, _| !sources.contains(&key.0));
    if legacy_count != legacy.entries.len() {
        legacy.write(tx)?;
    }
    crate::free_partition_metadata::transfer::remove_for_solids(tx, &sources)?;
    fragments::install(
        tx,
        result
            .fragments
            .iter()
            .map(|fragment| (fragment.source, fragment.owner, fragment.solid.clone())),
    )?;
    Ok(result)
}
