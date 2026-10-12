use super::*;
use crate::{edit, texel_uv};

#[test]
fn adjacent_boundary_cuts_extrude_checked_triangles_and_pentagons() {
    for oblique in [false, true] {
        for reflected in [false, true] {
            for owned in [false, true] {
                let (mut original, id) = super::tests::fixture(oblique);
                original
                    .edit("surface fields", |tx| {
                        for side in &mut tx.solid_mut(id)?.sides {
                            side.lightmap_scale = 8;
                            side.smoothing_groups = Some(7);
                            side.extra_properties.set("vendor", "preserve");
                            side.u_axis.offset = 17.123456789;
                            side.v_axis.offset = -9.125;
                            side.u_axis.scale = 0.3125;
                            side.v_axis.scale = -0.523456789;
                        }
                        Ok(())
                    })
                    .unwrap();
                if reflected {
                    original
                        .edit("reflect", |tx| {
                            edit::transform(
                                tx,
                                &[ObjectId::Solid(id)],
                                edit::Transform {
                                    reflection: [true, false, false],
                                    ..Default::default()
                                },
                            )
                        })
                        .unwrap();
                }
                if owned {
                    original
                        .edit("detail", |tx| {
                            edit::tie_to_entity(tx, &[id], "func_detail", &[])
                        })
                        .unwrap();
                }
                let (owner, source) = solid(&original, id).unwrap();
                let source = source.clone();
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
                    .unwrap();
                let choices = partition_pairs(&source, side.id).unwrap();
                assert_eq!(choices.len(), 6);
                assert_eq!(choices[..2], crease_pairs(&source, side.id).unwrap());
                assert_eq!(
                    choices
                        .iter()
                        .map(|pair| pair.edges)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    6
                );
                for pair in &choices[2..] {
                    for edge_parameters in [[0.5; 2], [0.2, 0.8], [0.8, 0.2]] {
                        let part = plan_partitions_at(
                            &original,
                            &[PartitionCut {
                                source: Crease {
                                    solid: id,
                                    side: side.id,
                                    edges: pair.edges,
                                },
                                edge_parameters,
                            }],
                        )
                        .unwrap()
                        .remove(0);
                        assert_eq!(part.owner, owner);
                        assert_eq!(part.loops[0].vertices.len(), 3);
                        assert_eq!(part.loops[1].vertices.len(), 5);
                        assert_eq!(part.cut().unwrap().edge_parameters, edge_parameters);
                        assert_eq!(checked_partition(&original, &part).unwrap(), part);
                        for texture_lock in [false, true] {
                            let mut document = original.clone();
                            let before = document.vmf().clone();
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
                            let selection = [(id, 0), (id, 1)];
                            let planned = plan_partition_extrusion_with_materials(
                                &document,
                                std::slice::from_ref(&part),
                                &selection,
                                &spec,
                                &materials,
                            )
                            .unwrap();
                            let applied = document
                                .edit("extrude", |tx| {
                                    extrude_partitions_with_materials(
                                        tx,
                                        std::slice::from_ref(&part),
                                        &selection,
                                        &spec,
                                        &materials,
                                    )
                                })
                                .unwrap();
                            assert_eq!(applied.pieces.len(), 2);
                            assert_eq!(solid(&document, id).unwrap().1, &source);
                            for (index, piece) in applied.pieces.iter().enumerate() {
                                let count = part.loops[index].vertices.len();
                                assert_eq!(piece.owner, owner);
                                assert_eq!(piece.solid.sides.len(), count + 2);
                                assert_eq!(
                                    topology(&piece.solid).unwrap().vertices.len(),
                                    count * 2
                                );
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
                                    count - 1
                                );
                                let mut replay = planned.pieces[index].solid.clone();
                                replay.id = piece.solid.id;
                                for (a, b) in replay.sides.iter_mut().zip(&piece.solid.sides) {
                                    a.id = b.id;
                                }
                                assert_eq!(replay, piece.solid);
                                let top = &piece.solid.sides[0];
                                assert_eq!(
                                    top.material,
                                    materials[index].material.to_ascii_uppercase()
                                );
                                assert_eq!(top.lightmap_scale, side.lightmap_scale);
                                assert_eq!(top.smoothing_groups, side.smoothing_groups);
                                assert_eq!(top.extra_properties, side.extra_properties);
                                assert_eq!(piece.solid.sides[1].material, side.material);
                                for &vertex in &part.loops[index].vertices {
                                    let flat = part.vertices[vertex as usize].position;
                                    let raised = flat + part.normal() * spec.height;
                                    assert!(texel_uv(top, raised).abs_diff_eq(
                                        texel_uv(side, if texture_lock { flat } else { raised }),
                                        1e-6
                                    ));
                                }
                            }
                            document.vmf().validate_ids().unwrap();
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
fn unsupported_boundary_roofs_and_invalid_plans_reject_atomically() {
    let (mut document, id) = super::tests::fixture(false);
    let source = document.solid(id).unwrap();
    let side = source
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let pair = partition_pairs(source, side).unwrap()[2].clone();
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
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let error = document
        .edit("unsupported roof", |tx| {
            crease_partitions_with_materials(tx, std::slice::from_ref(&part), 16.0, true, &[])
        })
        .err()
        .unwrap();
    assert!(error.to_string().contains("two quad"));
    for kind in 0..4 {
        let mut invalid = part.clone();
        match kind {
            0 => invalid.loops[0].vertices.resize(6, 0),
            1 => invalid.vertices.push(invalid.vertices[0].clone()),
            2 => invalid.loops[1].vertices[0] = 6,
            _ => invalid.loops[0].vertices.swap(0, 1),
        }
        assert!(checked_partition(&document, &invalid).is_err());
        assert!(
            document
                .edit("invalid extrusion", |tx| extrude_partitions(
                    tx,
                    &[invalid],
                    &[(id, 0)],
                    &ExtrusionSpec {
                        height: 16.0,
                        wall_material: None,
                        texture_lock: false
                    }
                ))
                .is_err()
        );
    }
    assert_eq!(document.vmf(), &before);
    assert_eq!(document.undo_labels().count(), history);
    let mut probe = document.clone();
    let expected = probe
        .edit("allocation", |tx| {
            Ok((tx.new_object_id()?, tx.new_side_id()?))
        })
        .unwrap();
    assert_eq!(
        document
            .edit("allocation", |tx| Ok((
                tx.new_object_id()?,
                tx.new_side_id()?
            )))
            .unwrap(),
        expected
    );
}
