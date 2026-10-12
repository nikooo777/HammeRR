use super::tests::{fixture, materials};
use super::*;
use crate::texel_uv;

#[test]
fn adjusted_planar_roofs_preserve_cut_points_material_identity_and_surface_policy() {
    for owned in [false, true] {
        for reflected in [false, true] {
            for direction in 0..2 {
                for parameter in [0.2, 0.8] {
                    for texture_lock in [false, true] {
                        let (mut document, mut part) = fixture(owned, reflected, direction);
                        let loops = part.loops.clone();
                        part = plan_partitions_at(
                            &document,
                            &[PartitionCut {
                                source: part.source.clone(),
                                edge_parameters: [parameter; 2],
                            }],
                        )
                        .unwrap()
                        .remove(0);
                        part.loops = loops;
                        let styles = materials(&part);
                        let before = document.vmf().clone();
                        let source = solid(&document, part.source.solid).unwrap().1.clone();
                        let original = source
                            .sides
                            .iter()
                            .find(|side| side.id == part.source.side)
                            .unwrap();
                        let positions = part.positions(16.125);
                        let plan = document
                            .edit("adjusted ridge", |tx| {
                                crease_partitions_with_materials(
                                    tx,
                                    std::slice::from_ref(&part),
                                    16.125,
                                    texture_lock,
                                    &styles,
                                )
                            })
                            .unwrap();
                        assert_eq!(plan.changes.len(), 1);
                        assert_eq!(plan.changes[0].positions, [positions[4], positions[5]]);
                        let output = solid(&document, part.source.solid).unwrap().1;
                        assert_eq!(output.sides.len(), source.sides.len() + 1);
                        assert_eq!(solid(&document, part.source.solid).unwrap().0, part.owner);
                        let shape = topology(output).unwrap();
                        assert_eq!(shape.vertices.len(), 10);
                        for corner in topology(&source).unwrap().vertices {
                            assert!(
                                shape
                                    .vertices
                                    .iter()
                                    .any(|vertex| vertex.position.distance(corner.position)
                                        < EPSILON * 10.0)
                            );
                        }
                        for piece in &part.loops {
                            let roof = output
                                .sides
                                .iter()
                                .find(|side| {
                                    side.material
                                        == styles[piece.id as usize].material.to_ascii_uppercase()
                                })
                                .unwrap();
                            let plane = Plane::from_points(roof.plane.0).unwrap();
                            assert!(
                                piece.vertices.iter().all(|index| plane
                                    .distance_to(positions[*index as usize])
                                    .abs()
                                    < EPSILON)
                            );
                            assert_eq!(roof.lightmap_scale, original.lightmap_scale);
                            assert_eq!(roof.smoothing_groups, original.smoothing_groups);
                            assert_eq!(roof.extra_properties, original.extra_properties);
                            assert_eq!(roof.rotation, original.rotation);
                            for index in piece.vertices {
                                let flat = part.vertices[index as usize].position;
                                let raised = positions[index as usize];
                                let expected =
                                    texel_uv(original, if texture_lock { flat } else { raised });
                                assert!(texel_uv(roof, raised).abs_diff_eq(expected, 1e-6));
                            }
                            if !texture_lock {
                                assert_eq!(roof.u_axis, original.u_axis);
                                assert_eq!(roof.v_axis, original.v_axis);
                            }
                        }
                        for side in &source.sides {
                            if side.id != part.source.side {
                                assert_eq!(
                                    output
                                        .sides
                                        .iter()
                                        .find(|output| output.id == side.id)
                                        .unwrap(),
                                    side
                                );
                            }
                        }
                        document.vmf().validate_ids().unwrap();
                        let applied = document.vmf().clone();
                        document.undo().unwrap();
                        assert_eq!(document.vmf(), &before);
                        document.redo().unwrap();
                        assert_eq!(document.vmf(), &applied);
                    }
                }
            }
        }
    }
}

#[test]
fn nonplanar_adjusted_roofs_reject_the_complete_batch_before_allocation() {
    let (mut document, first) = fixture(false, false, 0);
    let ObjectId::Solid(second) = document
        .edit("second", |tx| {
            crate::edit::create_block(
                tx,
                DVec3::new(128.0, 0.0, 0.0),
                DVec3::new(192.0, 64.0, 64.0),
                "brick/test",
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    let brush = document.solid(second).unwrap();
    let side = brush
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let cuts = [
        PartitionCut {
            source: first.source.clone(),
            edge_parameters: [0.2; 2],
        },
        PartitionCut {
            source: Crease {
                solid: second,
                side,
                edges: crease_pairs(brush, side).unwrap()[1].edges,
            },
            edge_parameters: [0.2, 0.8],
        },
    ];
    let parts = plan_partitions_at(&document, &cuts).unwrap();
    let original = document.vmf().clone();
    let history = document.undo_labels().count();
    let mut probe = document.clone();
    let expected = probe.edit("next", |tx| tx.new_side_id()).unwrap();
    let error = document
        .edit("invalid batch", |tx| {
            crease_partitions_with_materials(tx, &parts, 16.125, true, &[])
        })
        .err()
        .unwrap();
    assert!(error.to_string().contains("nonplanar"));
    assert_eq!(document.vmf(), &original);
    assert_eq!(document.undo_labels().count(), history);
    assert_eq!(
        document.edit("next", |tx| tx.new_side_id()).unwrap(),
        expected
    );
}
