use super::*;
use source_formats::vmf::{Properties, Side, UvAxis};

pub const MAX_EXTRUSION_PIECES: usize = MAX_SOLIDS * 2;
pub const MAX_EXTRUSION_SOURCE_VERTICES: usize = MAX_SOLIDS * MAX_PARTITION_VERTICES;
pub const MAX_EXTRUSION_PLANES: usize = MAX_SOLIDS * (MAX_BOUNDARY_SOURCE_CORNERS + 8);

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionSpec {
    pub height: f64,
    pub wall_material: Option<String>,
    pub texture_lock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrusionSurface {
    Top,
    Base,
    Boundary,
    Divider,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrusionFace {
    pub side: u32,
    pub source_side: u32,
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
    if partitions.is_empty() || partitions.len() > MAX_SOLIDS {
        return Err(invalid("Extrude flat partitions in 1–16 source brushes"));
    }
    if !spec.height.is_finite() || !(MIN_SPLIT_RISE..=MAX_SPLIT_RISE).contains(&spec.height) {
        return Err(invalid(
            "Extrusion height must be finite and between 0.125 and 4096 units",
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
        if spec.texture_lock {
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
                surface: ExtrusionSurface::Top,
            },
            ExtrusionFace {
                side: 2,
                source_side: original.id,
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
    Ok(ExtrusionPlan { pieces })
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
    let mut plan = plan_partition_extrusion_with_materials(
        tx.document(),
        partitions,
        selection,
        spec,
        materials,
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;

    fn fixture(
        owned: bool,
        oblique: bool,
        reflection: bool,
        direction: usize,
    ) -> (Document, Vec<Partition>) {
        let mut document = Document::empty();
        let object = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.125, -7.25, 16.5),
                    DVec3::new(64.875, 48.125, 80.25),
                    "brick/test",
                )
            })
            .unwrap();
        let ObjectId::Solid(id) = object else {
            unreachable!()
        };
        document
            .edit("projection", |tx| {
                for side in &mut tx.solid_mut(id)?.sides {
                    side.u_axis.offset = 13.125;
                    side.v_axis.offset = -7.75;
                    side.u_axis.scale = 0.375;
                    side.v_axis.scale = -0.625;
                    side.lightmap_scale = 8;
                    side.smoothing_groups = Some(32);
                    side.extra_properties.set("foreign", "keep");
                }
                Ok(())
            })
            .unwrap();
        if oblique || reflection {
            document
                .edit("transform", |tx| {
                    edit::transform(
                        tx,
                        &[object],
                        crate::transform::Transform {
                            rotation: if oblique {
                                DVec3::new(17.0, -23.0, 31.0)
                            } else {
                                DVec3::ZERO
                            },
                            scale: DVec3::new(1.125, 0.75, 1.5),
                            reflection: [reflection, false, false],
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
        }
        if owned {
            document
                .edit("owner", |tx| {
                    edit::tie_to_entity(tx, &[id], "func_detail", &[])
                })
                .unwrap();
        }
        let (_, source) = solid(&document, id).unwrap();
        let side = source
            .sides
            .iter()
            .max_by(|a, b| {
                Plane::from_points(a.plane.0)
                    .unwrap()
                    .normal
                    .z
                    .total_cmp(&Plane::from_points(b.plane.0).unwrap().normal.z)
            })
            .unwrap()
            .id;
        let request = Crease {
            solid: id,
            side,
            edges: crease_pairs(source, side).unwrap()[direction].edges,
        };
        document
            .edit("keep", |tx| crate::partition_metadata::store(tx, &request))
            .unwrap();
        let partitions = crate::partition_metadata::Metadata::parse(document.vmf())
            .unwrap()
            .resolve(&document)
            .into_iter()
            .map(|entry| entry.result.unwrap())
            .collect();
        (document, partitions)
    }

    #[test]
    fn extrusion_preserves_sources_owners_saved_intents_uvs_and_exact_history() {
        for owned in [false, true] {
            for oblique in [false, true] {
                for reflection in [false, true] {
                    for direction in 0..2 {
                        let (mut document, partitions) =
                            fixture(owned, oblique, reflection, direction);
                        let before = document.vmf().clone();
                        let history = document.undo_labels().count();
                        let original = solid(&document, partitions[0].source.solid)
                            .unwrap()
                            .1
                            .clone();
                        for texture_lock in [false, true] {
                            let spec = ExtrusionSpec {
                                height: 16.125,
                                wall_material: Some("wood/test".into()),
                                texture_lock,
                            };
                            let selection = [(original.id, 0), (original.id, 1)];
                            let plan =
                                plan_partition_extrusion(&document, &partitions, &selection, &spec)
                                    .unwrap();
                            assert_eq!(plan.pieces.len(), 2);
                            assert_eq!(document.vmf(), &before);
                            assert_eq!(document.undo_labels().count(), history);
                            let added = document
                                .edit("extrude", |tx| {
                                    extrude_partitions(tx, &partitions, &selection, &spec)
                                })
                                .unwrap();
                            assert_eq!(solid(&document, original.id).unwrap().1, &original);
                            assert_eq!(document.vmf().extra, before.extra);
                            assert_eq!(document.undo_labels().count(), history + 1);
                            document.vmf().validate_ids().unwrap();
                            for piece in &added.pieces {
                                let (owner, brush) = solid(&document, piece.solid.id).unwrap();
                                assert_eq!(
                                    owner,
                                    if owned {
                                        partitions[0].owner
                                    } else {
                                        ObjectId::Solid(piece.solid.id)
                                    }
                                );
                                assert_eq!(brush, &piece.solid);
                                assert_eq!(topology(brush).unwrap().vertices.len(), 8);
                                assert_eq!(
                                    piece
                                        .faces
                                        .iter()
                                        .filter(|face| face.surface == ExtrusionSurface::Divider)
                                        .count(),
                                    1
                                );
                                assert_eq!(
                                    piece
                                        .faces
                                        .iter()
                                        .filter(|face| face.surface == ExtrusionSurface::Boundary)
                                        .count(),
                                    3
                                );
                                let top = &brush.sides[0];
                                let source = original
                                    .sides
                                    .iter()
                                    .find(|side| side.id == partitions[0].source.side)
                                    .unwrap();
                                assert_eq!(top.material, source.material);
                                assert_eq!(top.lightmap_scale, 8);
                                assert_eq!(top.smoothing_groups, Some(32));
                                assert_eq!(top.extra_properties, source.extra_properties);
                                assert!(brush.sides[2..].iter().all(|side| side.material
                                    == "WOOD/TEST"
                                    && side.rotation == 0.0));
                                let ring = &partitions[0].loops[piece.partition as usize];
                                for &vertex in &ring.vertices {
                                    let point = partitions[0].vertices[vertex as usize].position;
                                    if texture_lock {
                                        let a = crate::texel_uv(source, point);
                                        let b = crate::texel_uv(
                                            top,
                                            point + partitions[0].normal() * spec.height,
                                        );
                                        assert!((a - b).abs().max_element() < 1e-4);
                                    } else {
                                        assert_eq!(top.u_axis, source.u_axis);
                                        assert_eq!(top.v_axis, source.v_axis);
                                    }
                                }
                            }
                            let a = added.pieces[0]
                                .faces
                                .iter()
                                .find(|face| face.surface == ExtrusionSurface::Divider)
                                .unwrap();
                            let b = added.pieces[1]
                                .faces
                                .iter()
                                .find(|face| face.surface == ExtrusionSurface::Divider)
                                .unwrap();
                            let plane = |index: usize, side: u32| {
                                Plane::from_points(
                                    added.pieces[index]
                                        .solid
                                        .sides
                                        .iter()
                                        .find(|face| face.id == side)
                                        .unwrap()
                                        .plane
                                        .0,
                                )
                                .unwrap()
                            };
                            let a = plane(0, a.side);
                            let b = plane(1, b.side);
                            assert!(a.normal.abs_diff_eq(-b.normal, 1e-9));
                            assert!((a.dist + b.dist).abs() < 1e-9);
                            let after = document.vmf().clone();
                            let reopened =
                                source_formats::vmf::Vmf::parse(&after.write(&Default::default()))
                                    .unwrap();
                            reopened.validate_ids().unwrap();
                            for piece in &added.pieces {
                                topology(
                                    reopened
                                        .solids()
                                        .find(|brush| brush.id == piece.solid.id)
                                        .unwrap(),
                                )
                                .unwrap();
                            }
                            document.undo().unwrap();
                            assert_eq!(document.vmf(), &before);
                            document.redo().unwrap();
                            assert_eq!(document.vmf(), &after);
                            document.undo().unwrap();
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_extrusions_reject_before_geometry_or_allocation_changes() {
        let (document, partitions) = fixture(false, false, false, 0);
        let id = partitions[0].source.solid;
        for (height, wall, selection) in [
            (f64::NAN, None, vec![(id, 0)]),
            (0.0, None, vec![(id, 0)]),
            (-1.0, None, vec![(id, 0)]),
            (0.01, None, vec![(id, 0)]),
            (4096.01, None, vec![(id, 0)]),
            (16.0, Some(" ".to_string()), vec![(id, 0)]),
            (16.0, None, vec![]),
            (16.0, None, vec![(id, 2)]),
            (16.0, None, vec![(u32::MAX, 0)]),
            (16.0, None, vec![(id, 0); MAX_EXTRUSION_PIECES + 1]),
        ] {
            let mut actual = document.clone();
            let mut control = document.clone();
            let spec = ExtrusionSpec {
                height,
                wall_material: wall,
                texture_lock: false,
            };
            assert!(
                actual
                    .edit("bad", |tx| extrude_partitions(
                        tx,
                        &partitions,
                        &selection,
                        &spec
                    ))
                    .is_err()
            );
            assert_eq!(actual.vmf(), document.vmf());
            assert_eq!(actual.undo_labels().count(), document.undo_labels().count());
            assert_eq!(
                actual
                    .edit("ids", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                    .unwrap(),
                control
                    .edit("ids", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                    .unwrap()
            );
        }
        let spec = ExtrusionSpec {
            height: 16.0,
            wall_material: None,
            texture_lock: false,
        };
        let mut tampered = partitions.clone();
        tampered[0].vertices[0].position.x += 1.0;
        assert!(plan_partition_extrusion(&document, &tampered, &[(id, 0)], &spec).is_err());
        tampered = partitions.clone();
        tampered[0].loops[0].vertices.swap(0, 1);
        assert!(plan_partition_extrusion(&document, &tampered, &[(id, 0)], &spec).is_err());
        let mut changed = document.clone();
        changed
            .edit("move", |tx| {
                edit::translate(tx, &[ObjectId::Solid(id)], DVec3::X, false)
            })
            .unwrap();
        assert!(plan_partition_extrusion(&changed, &partitions, &[(id, 0)], &spec).is_err());
    }

    #[test]
    fn reflected_saved_pieces_and_inherited_walls_keep_their_source_provenance() {
        for direction in 0..2 {
            let (mut document, partitions) = fixture(true, true, false, direction);
            let id = partitions[0].source.solid;
            document
                .edit("reflect", |tx| {
                    edit::transform(
                        tx,
                        &[partitions[0].owner],
                        crate::transform::Transform {
                            reflection: [true, false, false],
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
            let partitions = crate::partition_metadata::Metadata::parse(document.vmf())
                .unwrap()
                .resolve(&document)
                .into_iter()
                .map(|entry| entry.result.unwrap())
                .collect::<Vec<_>>();
            let spec = ExtrusionSpec {
                height: 32.25,
                wall_material: None,
                texture_lock: true,
            };
            let plan = plan_partition_extrusion(&document, &partitions, &[(id, 0)], &spec).unwrap();
            assert_eq!(plan.pieces.len(), 1);
            let piece = &plan.pieces[0];
            assert_eq!(piece.partition, 0);
            let (_, source) = solid(&document, id).unwrap();
            for face in &piece.faces {
                let original = source
                    .sides
                    .iter()
                    .find(|side| side.id == face.source_side)
                    .unwrap();
                let actual = piece
                    .solid
                    .sides
                    .iter()
                    .find(|side| side.id == face.side)
                    .unwrap();
                assert_eq!(actual.material, original.material);
                assert_eq!(actual.extra_properties, original.extra_properties);
            }
        }
    }

    #[test]
    fn extrusion_world_bounds_reject_without_consuming_ids() {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.0, 0.0, 65500.0),
                    DVec3::new(64.0, 64.0, 65520.0),
                    "brick/test",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let source = document.solid(id).unwrap();
        let side = source
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let requests = [Crease {
            solid: id,
            side,
            edges: crease_pairs(source, side).unwrap()[0].edges,
        }];
        let partitions = plan_partitions(&document, &requests).unwrap();
        let spec = ExtrusionSpec {
            height: 32.0,
            wall_material: None,
            texture_lock: false,
        };
        let before = document.vmf().clone();
        let error = document
            .edit("bad", |tx| {
                extrude_partitions(tx, &partitions, &[(id, 0)], &spec)
            })
            .unwrap_err();
        assert!(error.to_string().contains("65536"));
        assert_eq!(document.vmf(), &before);
    }
}
