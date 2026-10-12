use super::*;
use crate::{edit, primitive, texel_uv};

fn fixture(corners: u32, reflected: bool, owned: bool) -> (Document, u32, u32) {
    let mut document = Document::empty();
    let objects = document
        .edit("polygon", |tx| {
            primitive::create(
                tx,
                &primitive::Spec {
                    shape: primitive::Shape::Cylinder,
                    segments: corners,
                    mins: DVec3::new(0.125, -7.25, 16.5),
                    maxs: DVec3::new(64.875, 48.125, 80.25),
                    material: "brick/test".into(),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let ObjectId::Solid(id) = objects[0] else {
        unreachable!()
    };
    document
        .edit("surface fields", |tx| {
            for side in &mut tx.solid_mut(id)?.sides {
                side.u_axis.offset = 17.125;
                side.v_axis.offset = -9.125;
                side.u_axis.scale = 0.3125;
                side.v_axis.scale = -0.523456789;
                side.lightmap_scale = 8;
                side.smoothing_groups = Some(7);
                side.extra_properties.set("vendor", "preserve");
            }
            Ok(())
        })
        .unwrap();
    document
        .edit("affine", |tx| {
            edit::transform(
                tx,
                &objects,
                edit::Transform {
                    rotation: DVec3::new(17.0, -23.0, 31.0),
                    reflection: [reflected, false, false],
                    scale: DVec3::new(1.125, 0.75, 1.5),
                    ..Default::default()
                },
            )
        })
        .unwrap();
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
    (document, id, side)
}

#[test]
fn polygon_cuts_cover_all_pairs_and_replay_material_prisms_with_original_surface_fields() {
    for corners in [3, 5, 6, 8] {
        for reflected in [false, true] {
            for owned in [false, true] {
                let (original, id, side) = fixture(corners, reflected, owned);
                let (owner, source) = solid(&original, id).unwrap();
                let shape = topology(source).unwrap();
                let face = shape.faces.iter().find(|face| face.side == side).unwrap();
                let ring = face
                    .vertices
                    .iter()
                    .map(|index| shape.vertices[*index].position)
                    .collect::<Vec<_>>();
                for pair in partition_pairs(source, side).unwrap() {
                    for edge_parameters in [[0.5; 2], [0.2, 0.8], [0.8, 0.2]] {
                        let cut = PartitionCut {
                            source: Crease {
                                solid: id,
                                side,
                                edges: pair.edges,
                            },
                            edge_parameters,
                        };
                        let part = plan_partitions_at(&original, std::slice::from_ref(&cut))
                            .unwrap()
                            .remove(0);
                        assert_eq!(part.cut().unwrap(), cut);
                        assert_eq!(part.vertices.len(), corners as usize + 2);
                        assert_eq!(part.owner, owner);
                        part.validate(&ring).unwrap();
                        assert_eq!(
                            part.loops
                                .iter()
                                .map(|piece| piece.vertices.len())
                                .sum::<usize>(),
                            corners as usize + 4
                        );
                        assert_eq!(checked_partition(&original, &part).unwrap(), part);
                        for (index, point) in part.vertices[corners as usize..].iter().enumerate() {
                            let VertexSource::Edge { corners, parameter } = &point.source else {
                                unreachable!()
                            };
                            let endpoints = corners.each_ref().map(|corner| {
                                shape
                                    .vertices
                                    .iter()
                                    .find(|vertex| &vertex.sides == corner)
                                    .unwrap()
                                    .position
                            });
                            let expected = if *parameter == 0.5 {
                                pair.midpoints[index]
                            } else {
                                endpoints[0].lerp(endpoints[1], *parameter)
                            };
                            assert_eq!(point.position, expected);
                        }
                        for texture_lock in [false, true] {
                            let mut document = original.clone();
                            let before = document.vmf().clone();
                            let history = document.undo_labels().count();
                            let spec = ExtrusionSpec {
                                height: 16.125,
                                wall_material: None,
                                texture_lock,
                            };
                            let materials = [
                                PartitionMaterial {
                                    solid: id,
                                    partition: 0,
                                    material: "brick/red".into(),
                                },
                                PartitionMaterial {
                                    solid: id,
                                    partition: 1,
                                    material: "wood/blue".into(),
                                },
                            ];
                            let planned = plan_partition_extrusion_with_materials(
                                &document,
                                std::slice::from_ref(&part),
                                &[(id, 0), (id, 1)],
                                &spec,
                                &materials,
                            )
                            .unwrap();
                            let applied = document
                                .edit("extrude", |tx| {
                                    extrude_partitions_with_materials(
                                        tx,
                                        std::slice::from_ref(&part),
                                        &[(id, 0), (id, 1)],
                                        &spec,
                                        &materials,
                                    )
                                })
                                .unwrap();
                            assert_eq!(solid(&document, id).unwrap().1, source);
                            assert_eq!(document.vmf().extra, before.extra);
                            for (index, piece) in applied.pieces.iter().enumerate() {
                                let count = part.loops[index].vertices.len();
                                assert_eq!(piece.owner, owner);
                                assert_eq!(piece.solid.sides.len(), count + 2);
                                assert_eq!(
                                    topology(&piece.solid).unwrap().vertices.len(),
                                    count * 2
                                );
                                let mut replay = planned.pieces[index].solid.clone();
                                replay.id = piece.solid.id;
                                for (planned, applied) in
                                    replay.sides.iter_mut().zip(&piece.solid.sides)
                                {
                                    planned.id = applied.id;
                                }
                                assert_eq!(replay, piece.solid);
                                assert_eq!(
                                    piece
                                        .faces
                                        .iter()
                                        .filter(|face| face.surface == ExtrusionSurface::Divider)
                                        .count(),
                                    1
                                );
                                let original_side =
                                    source.sides.iter().find(|face| face.id == side).unwrap();
                                let top = &piece.solid.sides[0];
                                assert_eq!(
                                    top.material,
                                    materials[index].material.to_ascii_uppercase()
                                );
                                assert_eq!(piece.solid.sides[1].material, original_side.material);
                                assert_eq!(top.lightmap_scale, original_side.lightmap_scale);
                                assert_eq!(top.smoothing_groups, original_side.smoothing_groups);
                                assert_eq!(top.extra_properties, original_side.extra_properties);
                                for vertex in &part.loops[index].vertices {
                                    let point = part.vertices[*vertex as usize].position;
                                    if texture_lock {
                                        assert!(texel_uv(original_side, point).abs_diff_eq(
                                            texel_uv(top, point + part.normal() * spec.height),
                                            1e-4
                                        ));
                                    } else {
                                        assert_eq!(top.u_axis, original_side.u_axis);
                                        assert_eq!(top.v_axis, original_side.v_axis);
                                    }
                                }
                                for face in &piece.faces[2..] {
                                    let template = source
                                        .sides
                                        .iter()
                                        .find(|side| side.id == face.source_side)
                                        .unwrap();
                                    let wall = piece
                                        .solid
                                        .sides
                                        .iter()
                                        .find(|side| side.id == face.side)
                                        .unwrap();
                                    assert_eq!(wall.material, template.material);
                                    assert_eq!(wall.extra_properties, template.extra_properties);
                                }
                            }
                            assert_eq!(document.undo_labels().count(), history + 1);
                            let after = document.vmf().clone();
                            document.undo().unwrap();
                            assert_eq!(document.vmf(), &before);
                            document.redo().unwrap();
                            assert_eq!(document.vmf(), &after);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn polygon_topology_bounds_and_coverage_reject_without_consuming_geometry_ids() {
    let (mut document, id, side) = fixture(8, false, false);
    let pair = partition_pairs(document.solid(id).unwrap(), side).unwrap()[0].clone();
    let part = plan_partitions(
        &document,
        &[Crease {
            solid: id,
            side,
            edges: pair.edges,
        }],
    )
    .unwrap()
    .remove(0);
    let spec = ExtrusionSpec {
        height: 16.0,
        wall_material: None,
        texture_lock: false,
    };
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    for failure in 0..7 {
        let mut invalid = part.clone();
        match failure {
            0 => invalid
                .vertices
                .resize(MAX_PARTITION_VERTICES + 1, part.vertices[0].clone()),
            1 => invalid.loops[0]
                .vertices
                .resize(MAX_PARTITION_LOOP_VERTICES + 1, 0),
            2 => invalid.vertices[0].source = VertexSource::Corner(vec![side; MAX_SIDES + 1]),
            3 => invalid.loops[0].vertices.reverse(),
            4 => invalid.loops[1].vertices = invalid.loops[0].vertices.clone(),
            5 => invalid.vertices[8].position += DVec3::ONE,
            _ => invalid.loops[0].vertices[0] = u32::MAX,
        }
        assert!(checked_partition(&document, &invalid).is_err());
        assert!(
            document
                .edit("invalid", |tx| extrude_partitions(
                    tx,
                    &[invalid],
                    &[(id, 0)],
                    &spec
                ))
                .is_err()
        );
    }
    let mut oversized = Vec::new();
    let mut selection = Vec::new();
    for index in 0..MAX_SOLIDS {
        let mut source = part.clone();
        source.source.solid = index as u32 + 1;
        for piece in &mut source.loops {
            piece.vertices.resize(MAX_PARTITION_LOOP_VERTICES, 0);
            selection.push((source.source.solid, piece.id));
        }
        oversized.push(source);
    }
    let error = document
        .edit("aggregate budget", |tx| {
            extrude_partitions(tx, &oversized, &selection, &spec)
        })
        .unwrap_err();
    assert!(error.to_string().contains("aggregate plane budget"));
    assert!(
        document
            .edit("ridge", |tx| crease_partitions_with_materials(
                tx,
                std::slice::from_ref(&part),
                16.0,
                true,
                &[]
            ))
            .err()
            .unwrap()
            .to_string()
            .contains("two quad")
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(document.undo_labels().count(), history);
    let mut control = document.clone();
    let next = control
        .edit("allocate", |tx| {
            Ok((tx.new_object_id()?, tx.new_side_id()?))
        })
        .unwrap();
    assert_eq!(
        document
            .edit("allocate", |tx| Ok((
                tx.new_object_id()?,
                tx.new_side_id()?
            )))
            .unwrap(),
        next
    );
    for corners in [9, 16] {
        let (document, id, side) = fixture(corners, false, false);
        assert!(
            plan_partitions(
                &document,
                &[Crease {
                    solid: id,
                    side,
                    edges: [1, 2]
                }]
            )
            .unwrap_err()
            .to_string()
            .contains("3–8")
        );
    }
}
