use super::*;
use crate::{BrushGeometry, edit};
use source_formats::keyvalues::Node;

fn volume(solid: &Solid) -> f64 {
    let shape = BrushGeometry::from_solid(solid).unwrap();
    let center = shape.center();
    shape
        .faces
        .iter()
        .map(|face| {
            face.vertices[1..]
                .windows(2)
                .map(|pair| {
                    (face.vertices[0] - center).dot((pair[0] - center).cross(pair[1] - center))
                        / 6.0
                })
                .sum::<f64>()
        })
        .sum::<f64>()
        .abs()
}

fn area(partition: &Partition, piece: &PartitionLoop) -> f64 {
    let points = piece
        .vertices
        .iter()
        .map(|index| partition.vertices[*index as usize].position)
        .collect::<Vec<_>>();
    points[1..]
        .windows(2)
        .map(|pair| (pair[0] - points[0]).cross(pair[1] - points[0]).length() * 0.5)
        .sum()
}

fn contains(solid: &Solid, point: DVec3) -> bool {
    solid
        .sides
        .iter()
        .all(|side| Plane::from_points(side.plane.0).unwrap().distance_to(point) <= EPSILON)
}

#[test]
fn inward_pockets_preserve_volume_ownership_surfaces_metadata_and_exact_history() {
    for owned in [false, true] {
        for oblique in [false, true] {
            for reflection in [false, true] {
                for direction in 0..2 {
                    for both in [false, true] {
                        let (mut document, partitions) =
                            super::tests::fixture(owned, oblique, reflection, direction);
                        let id = partitions[0].source.solid;
                        document
                            .edit("metadata", |tx| {
                                let source = tx.solid_mut(id)?;
                                source.hidden = true;
                                source.extra_properties.set("foreign_brush", "keep");
                                source.extra.push(Node::block(
                                    "foreign_block",
                                    vec![Node::string("key", "value")],
                                ));
                                source
                                    .editor
                                    .get_or_insert_with(Properties::default)
                                    .set("foreign_editor", "keep");
                                Ok(())
                            })
                            .unwrap();
                        let before = document.vmf().clone();
                        let history = document.undo_labels().count();
                        let original = solid(&document, id).unwrap().1.clone();
                        let selection = if both {
                            vec![(id, 0), (id, 1)]
                        } else {
                            vec![(id, 0)]
                        };
                        let spec = ExtrusionSpec {
                            height: -16.125,
                            wall_material: Some("wood/cut".into()),
                            texture_lock: true,
                        };
                        let materials = (0..2)
                            .map(|partition| PartitionMaterial {
                                solid: id,
                                partition,
                                material: format!("floor/piece_{partition}"),
                            })
                            .collect::<Vec<_>>();
                        let textures = (0..2)
                            .map(|partition| PieceTexture {
                                solid: id,
                                partition,
                                texture: PartitionTexture {
                                    shift: [13.125 + f64::from(partition) * 17.0, -27.5],
                                    scale: [-0.375, 0.625],
                                    rotation: 37.0,
                                },
                            })
                            .collect::<Vec<_>>();
                        let plan = plan_partition_extrusion_with_surfaces(
                            &document,
                            &partitions,
                            &selection,
                            &spec,
                            &materials,
                            &textures,
                        )
                        .unwrap();
                        assert_eq!(document.vmf(), &before);
                        assert!(!plan.fragments.is_empty());
                        let expected = volume(&original)
                            - partitions[0]
                                .loops
                                .iter()
                                .filter(|piece| both || piece.id == 0)
                                .map(|piece| area(&partitions[0], piece) * spec.height.abs())
                                .sum::<f64>();
                        let actual = plan
                            .fragments
                            .iter()
                            .map(|fragment| volume(&fragment.solid))
                            .sum::<f64>();
                        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
                        for (index, first) in plan.fragments.iter().enumerate() {
                            for second in &plan.fragments[index + 1..] {
                                assert!(
                                    !crate::carve::intersects(&first.solid, &second.solid).unwrap()
                                );
                            }
                        }
                        let result = document
                            .edit("cut inward", |tx| {
                                extrude_partitions_with_surfaces(
                                    tx,
                                    &partitions,
                                    &selection,
                                    &spec,
                                    &materials,
                                    &textures,
                                )
                            })
                            .unwrap();
                        assert_eq!(result.fragments[0].solid.id, id);
                        assert_eq!(document.undo_labels().count(), history + 1);
                        document.vmf().validate_ids().unwrap();
                        assert!(
                            crate::partition_metadata::Metadata::parse(document.vmf())
                                .unwrap()
                                .entries
                                .is_empty()
                        );
                        let mut retained = BTreeSet::new();
                        for fragment in &result.fragments {
                            let (owner, brush) = solid(&document, fragment.solid.id).unwrap();
                            assert_eq!(
                                owner,
                                if owned {
                                    partitions[0].owner
                                } else {
                                    ObjectId::Solid(fragment.solid.id)
                                }
                            );
                            assert_eq!(brush, &fragment.solid);
                            assert_eq!(brush.editor, original.editor);
                            assert_eq!(brush.hidden, original.hidden);
                            assert_eq!(brush.extra_properties, original.extra_properties);
                            assert_eq!(brush.extra, original.extra);
                            topology(brush).unwrap();
                            for face in &fragment.faces {
                                let side = brush
                                    .sides
                                    .iter()
                                    .find(|side| side.id == face.side)
                                    .unwrap();
                                let source = original
                                    .sides
                                    .iter()
                                    .find(|side| side.id == face.source_side)
                                    .unwrap();
                                assert_eq!(side.lightmap_scale, source.lightmap_scale);
                                assert_eq!(side.smoothing_groups, source.smoothing_groups);
                                assert_eq!(side.extra_properties, source.extra_properties);
                                if face.surface == ExtrusionSurface::Source {
                                    let mut inherited = source.clone();
                                    inherited.id = side.id;
                                    assert_eq!(side, &inherited);
                                    if retained.insert(face.source_side) {
                                        assert_eq!(side.id, face.source_side);
                                    }
                                } else if face.surface == ExtrusionSurface::Top {
                                    let partition = face.partition.unwrap();
                                    assert_eq!(side.material, format!("FLOOR/PIECE_{partition}"));
                                    let piece = &partitions[0].loops[partition as usize];
                                    let bottom = piece
                                        .vertices
                                        .iter()
                                        .map(|index| {
                                            partitions[0].vertices[*index as usize].position
                                        })
                                        .collect::<Vec<_>>();
                                    let top = bottom
                                        .iter()
                                        .map(|point| *point + partitions[0].normal() * spec.height)
                                        .collect::<Vec<_>>();
                                    let mut expected = source.clone();
                                    textures[partition as usize]
                                        .texture
                                        .apply(source, &mut expected, &bottom, &top, true)
                                        .unwrap();
                                    for point in top {
                                        assert!(
                                            (crate::texel_uv(side, point)
                                                - crate::texel_uv(&expected, point))
                                            .length()
                                                < 0.0001
                                        );
                                    }
                                } else {
                                    assert_eq!(side.material, "WOOD/CUT");
                                }
                            }
                        }
                        for piece in &partitions[0].loops {
                            let center = piece
                                .vertices
                                .iter()
                                .map(|index| partitions[0].vertices[*index as usize].position)
                                .sum::<DVec3>()
                                / piece.vertices.len() as f64;
                            let removed = center + partitions[0].normal() * spec.height * 0.5;
                            assert_eq!(
                                result
                                    .fragments
                                    .iter()
                                    .any(|fragment| contains(&fragment.solid, removed)),
                                !both && piece.id == 1
                            );
                        }
                        let after = document.vmf().clone();
                        let reopened =
                            source_formats::vmf::Vmf::parse(&after.write(&Default::default()))
                                .unwrap();
                        reopened.validate_ids().unwrap();
                        for brush in reopened.solids() {
                            topology(brush).unwrap();
                        }
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

#[test]
fn through_cuts_open_the_bottom_and_full_removal_rolls_back_allocations() {
    for owned in [false, true] {
        let (mut document, partitions) = super::tests::fixture(owned, false, false, 0);
        let id = partitions[0].source.solid;
        let before = document.vmf().clone();
        let spec = ExtrusionSpec {
            height: -128.0,
            wall_material: None,
            texture_lock: false,
        };
        let mut control = document.clone();
        let error = document
            .edit("remove all", |tx| {
                extrude_partitions(tx, &partitions, &[(id, 0), (id, 1)], &spec)
            })
            .unwrap_err();
        assert!(error.to_string().contains("entire source"));
        assert_eq!(document.vmf(), &before);
        let result = document
            .edit("through", |tx| {
                extrude_partitions(tx, &partitions, &[(id, 0)], &spec)
            })
            .unwrap();
        let (_, source) = solid(&control, id).unwrap();
        let remaining = result
            .fragments
            .iter()
            .map(|fragment| volume(&fragment.solid))
            .sum::<f64>();
        assert!((remaining - volume(source) * 0.5).abs() < 0.0001);
        assert!(
            result
                .fragments
                .iter()
                .flat_map(|fragment| &fragment.faces)
                .all(|face| face.surface != ExtrusionSurface::Top)
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        let expected = control
            .edit("ids", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
            .unwrap();
        let mut rejected = Document::new(before);
        rejected
            .edit("bad", |tx| {
                extrude_partitions(tx, &partitions, &[(id, 0), (id, 1)], &spec)
            })
            .unwrap_err();
        assert_eq!(
            rejected
                .edit("ids", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                .unwrap(),
            expected
        );
    }
}

#[test]
fn malformed_saved_metadata_rejects_inward_without_mutating_source_or_history() {
    let (document, partitions) = super::tests::fixture(false, false, false, 0);
    let mut vmf = document.vmf().clone();
    vmf.extra.push(Node::block(
        crate::partition_metadata::KEY,
        vec![Node::string("version", "999")],
    ));
    let mut document = Document::new(vmf.clone());
    let id = partitions[0].source.solid;
    let spec = ExtrusionSpec {
        height: -16.0,
        wall_material: None,
        texture_lock: false,
    };
    assert!(
        document
            .edit("bad metadata", |tx| extrude_partitions(
                tx,
                &partitions,
                &[(id, 0)],
                &spec
            ))
            .is_err()
    );
    assert_eq!(document.vmf(), &vmf);
    assert_eq!(document.undo_labels().count(), 0);
}

#[test]
fn nonplanar_or_changed_inward_sources_reject_before_mutation() {
    let (document, partitions) = super::tests::fixture(false, false, false, 0);
    let id = partitions[0].source.solid;
    let spec = ExtrusionSpec {
        height: -16.0,
        wall_material: None,
        texture_lock: false,
    };
    let mut nonplanar = partitions.clone();
    nonplanar[0].vertices[0].position.z += 1.0;
    let mut actual = document.clone();
    assert!(
        actual
            .edit("nonplanar", |tx| extrude_partitions(
                tx,
                &nonplanar,
                &[(id, 0)],
                &spec
            ))
            .is_err()
    );
    assert_eq!(actual.vmf(), document.vmf());
    assert_eq!(actual.undo_labels().count(), document.undo_labels().count());
    actual
        .edit("move source", |tx| {
            edit::translate(tx, &[ObjectId::Solid(id)], DVec3::X, false)
        })
        .unwrap();
    let before = actual.vmf().clone();
    let history = actual.undo_labels().count();
    assert!(
        actual
            .edit("stale", |tx| extrude_partitions(
                tx,
                &partitions,
                &[(id, 0)],
                &spec
            ))
            .is_err()
    );
    assert_eq!(actual.vmf(), &before);
    assert_eq!(actual.undo_labels().count(), history);
}

#[test]
fn inward_consumes_all_changed_source_intents_and_preserves_unrelated_brushes() {
    let (mut document, partitions) = super::tests::fixture(true, false, false, 0);
    let id = partitions[0].source.solid;
    let source = solid(&document, id).unwrap().1;
    let side = source
        .sides
        .iter()
        .find(|side| side.id != partitions[0].source.side)
        .unwrap()
        .id;
    let request = Crease {
        solid: id,
        side,
        edges: crease_pairs(source, side).unwrap()[0].edges,
    };
    document
        .edit("second saved face", |tx| {
            crate::partition_metadata::store(tx, &request)
        })
        .unwrap();
    let ObjectId::Solid(other) = document
        .edit("unrelated", |tx| {
            edit::create_block(tx, DVec3::splat(256.0), DVec3::splat(320.0), "brick/other")
        })
        .unwrap()
    else {
        unreachable!()
    };
    let source = document.solid(other).unwrap();
    let side = source.sides[0].id;
    let request = Crease {
        solid: other,
        side,
        edges: crease_pairs(source, side).unwrap()[0].edges,
    };
    document
        .edit("unrelated saved face", |tx| {
            crate::partition_metadata::store(tx, &request)
        })
        .unwrap();
    let before = document.vmf().clone();
    let original = document.solid(other).unwrap().clone();
    document
        .edit("cut", |tx| {
            extrude_partitions(
                tx,
                &partitions,
                &[(id, 0)],
                &ExtrusionSpec {
                    height: -16.0,
                    wall_material: None,
                    texture_lock: false,
                },
            )
        })
        .unwrap();
    assert_eq!(document.solid(other).unwrap(), &original);
    let saved = crate::partition_metadata::Metadata::parse(document.vmf()).unwrap();
    assert_eq!(saved.entries.len(), 1);
    assert!(saved.entries.contains_key(&(other, side)));
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
}

#[test]
fn polygon_pockets_keep_exact_footprint_volume_and_sloped_sources_clip_at_their_boundary() {
    for corners in [3, 5, 8] {
        let mut document = Document::empty();
        let objects = document
            .edit("polygon", |tx| {
                crate::primitive::create(
                    tx,
                    &crate::primitive::Spec {
                        shape: crate::primitive::Shape::Cylinder,
                        segments: corners,
                        mins: DVec3::ZERO,
                        maxs: DVec3::splat(64.0),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let ObjectId::Solid(id) = objects[0] else {
            unreachable!()
        };
        let source = document.solid(id).unwrap();
        let side = source
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        for pair in partition_pairs(source, side).unwrap() {
            let partitions = plan_partitions_at(
                &document,
                &[PartitionCut {
                    source: Crease {
                        solid: id,
                        side,
                        edges: pair.edges,
                    },
                    edge_parameters: [0.2, 0.8],
                }],
            )
            .unwrap();
            let plan = plan_partition_extrusion(
                &document,
                &partitions,
                &[(id, 0)],
                &ExtrusionSpec {
                    height: -16.0,
                    wall_material: None,
                    texture_lock: false,
                },
            )
            .unwrap();
            let expected = volume(source) - area(&partitions[0], &partitions[0].loops[0]) * 16.0;
            assert!(
                (plan
                    .fragments
                    .iter()
                    .map(|fragment| volume(&fragment.solid))
                    .sum::<f64>()
                    - expected)
                    .abs()
                    < 0.001
            );
        }
    }
    let mut document = Document::empty();
    let objects = document
        .edit("wedge", |tx| {
            crate::primitive::create(
                tx,
                &crate::primitive::Spec {
                    shape: crate::primitive::Shape::Wedge,
                    mins: DVec3::ZERO,
                    maxs: DVec3::splat(64.0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let ObjectId::Solid(id) = objects[0] else {
        unreachable!()
    };
    let source = document.solid(id).unwrap();
    let side = source
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.5)
        .unwrap()
        .id;
    let partitions = plan_partitions(
        &document,
        &[Crease {
            solid: id,
            side,
            edges: crease_pairs(source, side).unwrap()[0].edges,
        }],
    )
    .unwrap();
    let plan = plan_partition_extrusion(
        &document,
        &partitions,
        &[(id, 0), (id, 1)],
        &ExtrusionSpec {
            height: -16.0,
            wall_material: None,
            texture_lock: false,
        },
    )
    .unwrap();
    for x in [7.125, 23.125, 39.125, 55.125] {
        for y in [7.125, 23.125, 39.125, 55.125] {
            for z in [5.75, 21.75, 37.75, 53.75] {
                let point = DVec3::new(x, y, z);
                let expected = contains(source, point)
                    && !plan
                        .pieces
                        .iter()
                        .any(|piece| contains(&piece.solid, point));
                assert_eq!(
                    plan.fragments
                        .iter()
                        .any(|fragment| contains(&fragment.solid, point)),
                    expected,
                    "{point:?}"
                );
            }
        }
    }
}
