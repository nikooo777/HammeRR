use super::*;
use crate::{edit, partition_metadata, transform::Transform};

pub(super) fn fixture(owned: bool, reflected: bool, direction: usize) -> (Document, Partition) {
    let mut document = Document::empty();
    let ObjectId::Solid(id) = document
        .edit("block", |tx| {
            edit::create_block(
                tx,
                DVec3::new(0.125, -7.25, 16.5),
                DVec3::new(64.875, 48.125, 80.25),
                "brick/test",
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    document
        .edit("surface settings", |tx| {
            for side in &mut tx.solid_mut(id)?.sides {
                side.u_axis.scale = 0.3125;
                side.v_axis.scale = -0.523456789;
                side.u_axis.offset = 17.123456789;
                side.v_axis.offset = -9.125;
                side.lightmap_scale = 8;
                side.smoothing_groups = Some(7);
                side.extra_properties.set("vendor", "preserve");
            }
            Ok(())
        })
        .unwrap();
    let brush = document.solid(id).unwrap();
    let side = brush
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let source = Crease {
        solid: id,
        side,
        edges: crease_pairs(brush, side).unwrap()[direction].edges,
    };
    document
        .edit("keep", |tx| partition_metadata::store(tx, &source))
        .unwrap();
    if reflected {
        document
            .edit("reflect", |tx| {
                edit::transform(
                    tx,
                    &[ObjectId::Solid(id)],
                    Transform {
                        reflection: [true, false, false],
                        rotation: DVec3::new(17.0, -23.0, 31.0),
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
    let partition = partition_metadata::Metadata::parse(document.vmf())
        .unwrap()
        .resolve(&document)
        .remove(0)
        .result
        .unwrap();
    (document, partition)
}

pub(super) fn materials(partition: &Partition) -> Vec<PartitionMaterial> {
    ["brick/red", "wood/blue"]
        .into_iter()
        .enumerate()
        .map(|(piece, material)| PartitionMaterial {
            solid: partition.source.solid,
            partition: piece as u32,
            material: material.into(),
        })
        .collect()
}

#[test]
fn planned_materials_change_only_corresponding_roof_and_prism_top_surfaces() {
    for owned in [false, true] {
        for reflected in [false, true] {
            for direction in 0..2 {
                for texture_lock in [false, true] {
                    let (document, partition) = fixture(owned, reflected, direction);
                    let source = document.vmf().clone();
                    let materials = materials(&partition);
                    let mut inherited = document.clone();
                    let baseline = inherited
                        .edit("ridge", |tx| {
                            crease_with_roof_textures(
                                tx,
                                std::slice::from_ref(&partition.source),
                                16.125,
                                texture_lock,
                            )
                        })
                        .unwrap();
                    let mut changed = document.clone();
                    let actual = changed
                        .edit("ridge materials", |tx| {
                            crease_partitions_with_materials(
                                tx,
                                std::slice::from_ref(&partition),
                                16.125,
                                texture_lock,
                                &materials,
                            )
                        })
                        .unwrap();
                    let mut normalized = actual.clone();
                    for (replacement, baseline) in normalized
                        .replacements
                        .iter_mut()
                        .zip(&baseline.replacements)
                    {
                        assert_eq!(
                            replacement
                                .solid
                                .sides
                                .iter()
                                .filter(|side| ["BRICK/RED", "WOOD/BLUE"]
                                    .contains(&side.material.as_str()))
                                .count(),
                            2
                        );
                        for (side, original) in replacement
                            .solid
                            .sides
                            .iter_mut()
                            .zip(&baseline.solid.sides)
                        {
                            side.material.clone_from(&original.material);
                        }
                    }
                    assert!(normalized == baseline);
                    let brush = solid(&changed, partition.source.solid).unwrap().1;
                    let positions = partition.positions(16.125);
                    for piece in &partition.loops {
                        let side = brush
                            .sides
                            .iter()
                            .find(|side| {
                                side.material
                                    == materials[piece.id as usize].material.to_ascii_uppercase()
                            })
                            .unwrap();
                        let plane = Plane::from_points(side.plane.0).unwrap();
                        assert!(piece.vertices.iter().all(|vertex| {
                            plane.distance_to(positions[*vertex as usize]).abs() < EPSILON
                        }));
                    }
                    let after = changed.vmf().clone();
                    changed.undo().unwrap();
                    assert_eq!(changed.vmf(), &source);
                    changed.redo().unwrap();
                    assert_eq!(changed.vmf(), &after);
                    let spec = ExtrusionSpec {
                        height: 32.125,
                        wall_material: Some("concrete/wall".into()),
                        texture_lock,
                    };
                    let selection = [(partition.source.solid, 0), (partition.source.solid, 1)];
                    let expected = plan_partition_extrusion(
                        &document,
                        std::slice::from_ref(&partition),
                        &selection,
                        &spec,
                    )
                    .unwrap();
                    let mut plan = plan_partition_extrusion_with_materials(
                        &document,
                        std::slice::from_ref(&partition),
                        &selection,
                        &spec,
                        &materials,
                    )
                    .unwrap();
                    for piece in &mut plan.pieces {
                        assert_eq!(
                            piece.solid.sides[0].material,
                            materials[piece.partition as usize]
                                .material
                                .to_ascii_uppercase()
                        );
                        piece.solid.sides[0].material.clone_from(
                            &expected.pieces[piece.partition as usize].solid.sides[0].material,
                        );
                    }
                    assert_eq!(plan, expected);
                    let mut changed = document.clone();
                    changed
                        .edit("extrude", |tx| {
                            extrude_partitions_with_materials(
                                tx,
                                std::slice::from_ref(&partition),
                                &selection,
                                &spec,
                                &materials,
                            )
                        })
                        .unwrap();
                    assert_eq!(
                        solid(&changed, partition.source.solid).unwrap().1,
                        solid(&document, partition.source.solid).unwrap().1
                    );
                    let after = changed.vmf().clone();
                    changed.undo().unwrap();
                    assert_eq!(changed.vmf(), &source);
                    changed.redo().unwrap();
                    assert_eq!(changed.vmf(), &after);
                }
            }
        }
    }
}

#[test]
fn invalid_piece_material_plans_reject_atomically_before_allocating() {
    let (document, partition) = fixture(false, false, 0);
    let source = document.vmf().clone();
    let good = materials(&partition)[0].clone();
    let mut duplicate = good.clone();
    duplicate.material = "another/material".into();
    for incoming in [
        vec![PartitionMaterial {
            solid: 9999,
            ..good.clone()
        }],
        vec![PartitionMaterial {
            partition: 2,
            ..good.clone()
        }],
        vec![good.clone(), duplicate],
        vec![PartitionMaterial {
            material: " ".into(),
            ..good.clone()
        }],
        vec![PartitionMaterial {
            material: "a\nb".into(),
            ..good.clone()
        }],
        vec![PartitionMaterial {
            material: "x".repeat(partition_metadata::MAX_TEXT_BYTES + 1),
            ..good.clone()
        }],
        vec![
            PartitionMaterial {
                material: "x".repeat(33 * 1024),
                ..good.clone()
            },
            PartitionMaterial {
                partition: 1,
                material: "y".repeat(33 * 1024),
                ..good.clone()
            },
        ],
        vec![good.clone(); MAX_SOLIDS * 2 + 1],
    ] {
        let mut changed = document.clone();
        assert!(
            changed
                .edit("bad ridge", |tx| crease_partitions_with_materials(
                    tx,
                    std::slice::from_ref(&partition),
                    16.0,
                    false,
                    &incoming
                ))
                .is_err()
        );
        assert!(
            changed
                .edit("bad extrusion", |tx| extrude_partitions_with_materials(
                    tx,
                    std::slice::from_ref(&partition),
                    &[(partition.source.solid, 0)],
                    &ExtrusionSpec {
                        height: 32.0,
                        wall_material: None,
                        texture_lock: false
                    },
                    &incoming
                ))
                .is_err()
        );
        assert_eq!(changed.vmf(), &source);
        let mut control = document.clone();
        assert_eq!(
            changed
                .edit("IDs", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                .unwrap(),
            control
                .edit("IDs", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                .unwrap()
        );
    }
}
