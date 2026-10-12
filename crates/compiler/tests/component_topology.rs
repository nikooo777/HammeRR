mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit, subobject};
use source_formats::bsp::{Brush, Face, Plane};

#[test]
fn independently_extruded_partition_prisms_compile_after_vmf_reopening() {
    for edge_parameters in [[0.5; 2], [0.2, 0.8]] {
        for owned in [false, true] {
            for oblique in [false, true] {
                for direction in 0..2 {
                    let mut document = Document::new(common::room());
                    let object = document
                        .edit("block", |tx| {
                            edit::create_block(
                                tx,
                                DVec3::new(0.125, -7.25, 16.5),
                                DVec3::new(64.875, 48.125, 80.25),
                                "dev/dev_measuregeneric01",
                            )
                        })
                        .unwrap();
                    let ObjectId::Solid(id) = object else {
                        unreachable!()
                    };
                    if oblique {
                        document
                            .edit("rotate", |tx| {
                                edit::transform(
                                    tx,
                                    &[object],
                                    map_core::transform::Transform {
                                        rotation: DVec3::new(17.0, -23.0, 31.0),
                                        pivot: DVec3::new(32.5, 20.4375, 48.375),
                                        ..Default::default()
                                    },
                                )
                            })
                            .unwrap();
                    }
                    if owned {
                        document
                            .edit("detail", |tx| {
                                edit::tie_to_entity(tx, &[id], "func_detail", &[])
                            })
                            .unwrap();
                    }
                    let (_, source) = subobject::solid(&document, id).unwrap();
                    let side = source
                        .sides
                        .iter()
                        .max_by(|a, b| {
                            map_core::Plane::from_points(a.plane.0)
                                .unwrap()
                                .normal
                                .z
                                .total_cmp(
                                    &map_core::Plane::from_points(b.plane.0).unwrap().normal.z,
                                )
                        })
                        .unwrap()
                        .id;
                    let request = subobject::Crease {
                        solid: id,
                        side,
                        edges: subobject::crease_pairs(source, side).unwrap()[direction].edges,
                    };
                    let partitions = subobject::plan_partitions_at(
                        &document,
                        &[subobject::PartitionCut {
                            source: request,
                            edge_parameters,
                        }],
                    )
                    .unwrap();
                    document
                        .edit("keep", |tx| {
                            map_core::partition_metadata::store_partition(tx, &partitions[0])
                        })
                        .unwrap();
                    let spec = subobject::ExtrusionSpec {
                        height: 16.125,
                        wall_material: Some("dev/dev_measurewall01a".into()),
                        texture_lock: true,
                    };
                    document
                        .edit("extrude", |tx| {
                            subobject::extrude_partitions_with_materials(
                                tx,
                                &partitions,
                                &[(id, 0), (id, 1)],
                                &spec,
                                &[
                                    subobject::PartitionMaterial {
                                        solid: id,
                                        partition: 0,
                                        material: "brick/planned_red".into(),
                                    },
                                    subobject::PartitionMaterial {
                                        solid: id,
                                        partition: 1,
                                        material: "wood/planned_blue".into(),
                                    },
                                ],
                            )
                        })
                        .unwrap();
                    let reopened =
                        source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default()))
                            .unwrap();
                    reopened.validate_ids().unwrap();
                    let tops = reopened
                        .world
                        .solids
                        .iter()
                        .chain(reopened.entities.iter().flat_map(|entity| &entity.solids))
                        .flat_map(|solid| &solid.sides)
                        .filter(|side| {
                            ["BRICK/PLANNED_RED", "WOOD/PLANNED_BLUE"]
                                .contains(&side.material.as_str())
                        })
                        .count();
                    assert_eq!(tops, 2);
                    let output = compiler::compile(&reopened, &common::empty_game());
                    assert!(!output.report.has_errors(), "{:?}", output.report);
                    assert!(output.leak.is_none());
                    assert_eq!(output.stats.brushes, 9);
                    assert!(
                        output
                            .bsp
                            .unwrap()
                            .records::<Brush>()
                            .unwrap()
                            .iter()
                            .all(|brush| brush.side_count >= 6)
                    );
                }
            }
        }
    }
}

#[test]
fn saved_flat_partition_intents_leave_native_bsp_bytes_unchanged() {
    let mut document = Document::new(common::room());
    let ObjectId::Solid(id) = document
        .edit("block", |tx| {
            edit::create_block(
                tx,
                DVec3::new(0.125, -7.25, 16.5),
                DVec3::new(64.875, 48.125, 80.25),
                "dev/dev_measuregeneric01",
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
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let request = subobject::Crease {
        solid: id,
        side,
        edges: subobject::crease_pairs(source, side).unwrap()[0].edges,
    };
    let before = compiler::compile(document.vmf(), &common::empty_game());
    assert!(!before.report.has_errors());
    assert!(before.leak.is_none());
    let bytes = before.bsp.unwrap().to_bytes();
    for edge_parameters in [[0.5; 2], [0.2, 0.8]] {
        let partition = subobject::plan_partitions_at(
            &document,
            &[subobject::PartitionCut {
                source: request.clone(),
                edge_parameters,
            }],
        )
        .unwrap()
        .remove(0);
        document
            .edit("store flat intent", |tx| {
                map_core::partition_metadata::store_partition(tx, &partition)
            })
            .unwrap();
        for styled in [false, true] {
            document
                .edit("planned piece materials", |tx| {
                    map_core::partition_metadata::set_piece_top_materials(
                        tx,
                        &request,
                        &[0],
                        styled.then_some("brick/planned_red"),
                    )?;
                    map_core::partition_metadata::set_piece_top_materials(
                        tx,
                        &request,
                        &[1],
                        styled.then_some("wood/planned_blue"),
                    )
                })
                .unwrap();
            let reopened =
                source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default()))
                    .unwrap();
            let metadata = map_core::partition_metadata::Metadata::parse(&reopened).unwrap();
            assert_eq!(metadata.entries.len(), 1);
            assert_eq!(
                metadata.entries[&(id, side)]
                    .top_materials
                    .iter()
                    .all(Option::is_some),
                styled
            );
            let after = compiler::compile(&reopened, &common::empty_game());
            assert!(!after.report.has_errors());
            assert!(after.leak.is_none());
            assert_eq!(after.bsp.unwrap().to_bytes(), bytes);
        }
    }
}

#[test]
fn planned_roof_materials_compile_after_vmf_reopening() {
    for owned in [false, true] {
        for direction in 0..2 {
            let mut document = Document::new(common::room());
            let ObjectId::Solid(id) = document
                .edit("block", |tx| {
                    edit::create_block(
                        tx,
                        DVec3::new(0.0, 0.0, 16.0),
                        DVec3::new(64.0, 64.0, 80.0),
                        "dev/dev_measuregeneric01",
                    )
                })
                .unwrap()
            else {
                unreachable!()
            };
            if owned {
                document
                    .edit("detail", |tx| {
                        edit::tie_to_entity(tx, &[id], "func_detail", &[])
                    })
                    .unwrap();
            }
            let brush = subobject::solid(&document, id).unwrap().1;
            let side = brush
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let partition = subobject::plan_partitions(
                &document,
                &[subobject::Crease {
                    solid: id,
                    side,
                    edges: subobject::crease_pairs(brush, side).unwrap()[direction].edges,
                }],
            )
            .unwrap()
            .remove(0);
            document
                .edit("roof", |tx| {
                    subobject::crease_partitions_with_materials(
                        tx,
                        std::slice::from_ref(&partition),
                        16.0,
                        true,
                        &[
                            subobject::PartitionMaterial {
                                solid: id,
                                partition: 0,
                                material: "brick/planned_red".into(),
                            },
                            subobject::PartitionMaterial {
                                solid: id,
                                partition: 1,
                                material: "wood/planned_blue".into(),
                            },
                        ],
                    )
                })
                .unwrap();
            let reopened =
                source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default()))
                    .unwrap();
            reopened.validate_ids().unwrap();
            let brush = subobject::solid(&Document::new(reopened.clone()), id)
                .unwrap()
                .1
                .clone();
            assert_eq!(
                brush
                    .sides
                    .iter()
                    .filter(|side| ["BRICK/PLANNED_RED", "WOOD/PLANNED_BLUE"]
                        .contains(&side.material.as_str()))
                    .count(),
                2
            );
            let output = compiler::compile(&reopened, &common::empty_game());
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert!(output.leak.is_none());
            assert_eq!(output.stats.brushes, 7);
        }
    }
}

#[test]
fn sheared_oblique_brushes_compile_after_vmf_round_trip() {
    for factor in [-1.25, 0.75] {
        let mut document = Document::new(common::room());
        let id = document
            .edit("brush", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.0, 0.0, 16.0),
                    DVec3::new(64.0, 32.0, 80.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap();
        document
            .edit("rotate", |tx| {
                map_core::transform::transform(
                    tx,
                    &[id],
                    map_core::transform::Transform {
                        rotation: DVec3::new(17.0, -23.0, 31.0),
                        pivot: DVec3::new(32.0, 16.0, 48.0),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        document
            .edit("shear", |tx| {
                map_core::transform::transform(
                    tx,
                    &[id],
                    map_core::transform::Transform {
                        shear: Some(map_core::transform::Shear {
                            axis: 0,
                            reference: 1,
                            factor,
                        }),
                        basis: glam::DMat3::from_rotation_z(0.37),
                        pivot: DVec3::new(32.125, 16.25, 48.5),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        assert!(!output.bsp.unwrap().records::<Face>().unwrap().is_empty());
    }
}

#[test]
fn reflected_oblique_brushes_compile_after_vmf_round_trip() {
    for reflection in [
        [true, false, false],
        [true, true, false],
        [true, true, true],
    ] {
        let mut document = Document::new(common::room());
        let id = document
            .edit("brush", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap();
        document
            .edit("rotate", |tx| {
                map_core::transform::transform(
                    tx,
                    &[id],
                    map_core::transform::Transform {
                        rotation: DVec3::new(17.0, -23.0, 31.0),
                        pivot: DVec3::splat(32.0),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        document
            .edit("mirror", |tx| {
                map_core::transform::transform(
                    tx,
                    &[id],
                    map_core::transform::Transform {
                        reflection,
                        basis: glam::DMat3::from_rotation_z(0.37),
                        pivot: DVec3::new(32.125, 32.25, 64.5),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(!bsp.records::<Face>().unwrap().is_empty());
        assert!(
            bsp.records::<Plane>()
                .unwrap()
                .iter()
                .all(|plane| plane.dist.is_finite()
                    && plane.normal.iter().all(|value| value.is_finite()))
        );
    }
}

#[test]
fn repeatedly_rotated_thin_slope_compiles_inside_a_sealed_room() {
    let source = source_formats::vmf::Vmf::parse(include_str!("fixtures/thin_slope.vmf")).unwrap();
    let mut document = Document::new(common::room());
    let ObjectId::Solid(id) = document
        .edit("thin slope", |tx| {
            let ObjectId::Solid(id) =
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")?
            else {
                unreachable!()
            };
            for (target, source) in tx
                .solid_mut(id)?
                .sides
                .iter_mut()
                .zip(&source.world.solids[0].sides)
            {
                let id = target.id;
                *target = source.clone();
                target.id = id;
            }
            Ok(ObjectId::Solid(id))
        })
        .unwrap()
    else {
        unreachable!()
    };
    let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
    let components = shape
        .faces
        .iter()
        .map(|face| subobject::Component {
            solid: id,
            sides: vec![face.side],
        })
        .collect::<Vec<_>>();
    let pivot = subobject::component_center(&document, &components).unwrap();
    for degrees in [DVec3::X * 17.0, DVec3::Y * -23.0, DVec3::Z * 31.0] {
        for degrees in [degrees, -degrees] {
            let plan = subobject::plan_transform(
                &document,
                &components,
                &subobject::ComponentTransform::Rotate(degrees),
                Some(pivot),
                true,
            )
            .unwrap();
            document
                .edit("rotate slope", |tx| {
                    subobject::apply(tx, &plan.replacements)
                })
                .unwrap();
            let reopened =
                source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default()))
                    .unwrap();
            reopened.validate_ids().unwrap();
            document = Document::try_new(reopened).unwrap();
        }
    }
    let after = subobject::topology(document.solid(id).unwrap()).unwrap();
    for vertex in &shape.vertices {
        assert!(
            after.vertices[after.resolve(&vertex.sides).unwrap()[0]]
                .position
                .abs_diff_eq(vertex.position, 1e-6)
        );
    }
    document
        .edit("place slope in room", |tx| {
            map_core::transform::transform(
                tx,
                &[ObjectId::Solid(id)],
                map_core::transform::Transform {
                    translation: DVec3::new(-4162.0, -5836.0, 16.0),
                    texture_lock: true,
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    vmf.validate_ids().unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert_eq!(output.stats.brushes, 7);
    let bsp = output.bsp.unwrap();
    assert!(!bsp.records::<Face>().unwrap().is_empty());
    assert!(bsp.records::<Plane>().unwrap().iter().all(|plane| {
        plane.dist.is_finite() && plane.normal.iter().all(|value| value.is_finite())
    }));
}

#[test]
fn welded_pyramid_compiles_inside_a_sealed_room_after_vmf_round_trip() {
    for target in [
        subobject::WeldTarget::Mean,
        subobject::WeldTarget::Position(DVec3::new(16.125, 24.25, 96.5)),
    ] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
        let selected = shape
            .vertices
            .iter()
            .filter(|vertex| vertex.position.z == 64.0)
            .map(|vertex| subobject::Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        let plan = subobject::plan_weld_with_textures(&document, &selected, &target, true).unwrap();
        document
            .edit("weld", |tx| subobject::apply(tx, &plan.replacements))
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(
            bsp.records::<Brush>()
                .unwrap()
                .iter()
                .any(|brush| brush.side_count >= 5)
        );
        assert!(!bsp.records::<Face>().unwrap().is_empty());
        assert!(bsp.records::<Plane>().unwrap().iter().any(|plane| {
            plane
                .normal
                .iter()
                .filter(|value| value.abs() > 0.01)
                .count()
                == 2
        }));
    }
}

#[test]
fn split_face_and_edge_compile_inside_a_sealed_room_after_vmf_round_trip() {
    for edge in [false, true] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
        let sides = if edge {
            shape.edges[0].sides.clone()
        } else {
            vec![shape.faces[0].side]
        };
        document
            .edit("split", |tx| {
                subobject::split_with_textures(
                    tx,
                    &[subobject::Component { solid: id, sides }],
                    16.0,
                    true,
                )
            })
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(!bsp.records::<Face>().unwrap().is_empty());
        assert!(bsp.records::<Plane>().unwrap().iter().any(|plane| {
            plane
                .normal
                .iter()
                .filter(|value| value.abs() > 0.01)
                .count()
                >= 2
        }));
    }
}

#[test]
fn carved_room_compiles_after_removing_the_retained_cutter_and_vmf_round_trip() {
    let mut document = Document::empty();
    let (outer, cutter) = document
        .edit("room volumes", |tx| {
            let outer = edit::create_block(
                tx,
                DVec3::new(-144.0, -144.0, -16.0),
                DVec3::splat(144.0),
                "dev/dev_measuregeneric01",
            )?;
            let cutter = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::splat(128.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok((outer, cutter))
        })
        .unwrap();
    let (ObjectId::Solid(outer), ObjectId::Solid(cutter)) = (outer, cutter) else {
        unreachable!()
    };
    let result = document
        .edit("carve room", |tx| {
            map_core::carve::carve(tx, cutter, &[outer])
        })
        .unwrap();
    assert_eq!(result.changes[0].fragments.len(), 6);
    document
        .edit("remove cutter and add spawn", |tx| {
            edit::delete(tx, &[ObjectId::Solid(cutter)])?;
            edit::create_entity(
                tx,
                "info_player_counterterrorist",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    vmf.validate_ids().unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert_eq!(output.stats.brushes, 6);
    assert!(!output.bsp.unwrap().records::<Face>().unwrap().is_empty());
}

#[test]
fn scaled_and_rotated_components_compile_after_vmf_round_trip() {
    for (spec, whole) in [
        (
            subobject::ComponentTransform::Scale(DVec3::new(0.5, 0.5, 1.0)),
            false,
        ),
        (
            subobject::ComponentTransform::Rotate(DVec3::new(30.0, 0.0, 0.0)),
            false,
        ),
        (
            subobject::ComponentTransform::Scale(DVec3::new(2.0, 0.5, 1.25)),
            true,
        ),
        (
            subobject::ComponentTransform::Rotate(DVec3::new(15.0, -20.0, 30.0)),
            true,
        ),
    ] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
        let selected = shape
            .vertices
            .iter()
            .filter(|vertex| whole || vertex.position.z == 64.0)
            .map(|vertex| subobject::Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        let plan = subobject::plan_transform(&document, &selected, &spec, None, whole).unwrap();
        document
            .edit("transform", |tx| subobject::apply(tx, &plan.replacements))
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        assert!(!output.bsp.unwrap().records::<Face>().unwrap().is_empty());
    }
}

#[test]
fn multi_cutter_room_compiles_after_retaining_then_removing_both_cutters() {
    let mut document = Document::empty();
    let (outer, left, right) = document
        .edit("room inputs", |tx| {
            let outer = edit::create_block(
                tx,
                DVec3::new(-144.0, -144.0, -16.0),
                DVec3::splat(144.0),
                "dev/dev_measuregeneric01",
            )?;
            let left = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(0.0, 128.0, 128.0),
                "dev/dev_measuregeneric01",
            )?;
            let right = edit::create_block(
                tx,
                DVec3::new(0.0, -128.0, 0.0),
                DVec3::splat(128.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok((outer, left, right))
        })
        .unwrap();
    let (ObjectId::Solid(outer), ObjectId::Solid(left), ObjectId::Solid(right)) =
        (outer, left, right)
    else {
        unreachable!()
    };
    document
        .edit("carve room", |tx| {
            map_core::carve::carve_many(tx, &[left, right], &[outer])
        })
        .unwrap();
    document
        .edit("remove cutters and add spawn", |tx| {
            edit::delete(tx, &[ObjectId::Solid(left), ObjectId::Solid(right)])?;
            edit::create_entity(
                tx,
                "info_player_counterterrorist",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    vmf.validate_ids().unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert!(output.stats.brushes >= 6);
}

#[test]
fn opposing_edge_ridges_compile_inside_a_sealed_room_after_vmf_round_trip() {
    for index in 0..2 {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let solid = document.solid(id).unwrap();
        let side = solid
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let pair = subobject::crease_pairs(solid, side).unwrap()[index].edges;
        let before = document.vmf().clone();
        let partitions = subobject::plan_partitions(
            &document,
            &[subobject::Crease {
                solid: id,
                side,
                edges: pair,
            }],
        )
        .unwrap();
        assert_eq!(document.vmf(), &before);
        assert_eq!(partitions[0].loops.len(), 2);
        let raised = partitions[0].positions(16.125);
        document
            .edit("crease", |tx| {
                subobject::crease_with_roof_textures(
                    tx,
                    &[partitions[0].source.clone()],
                    16.125,
                    true,
                )
            })
            .unwrap();
        let topology = subobject::topology(document.solid(id).unwrap()).unwrap();
        for position in raised {
            assert!(
                topology
                    .vertices
                    .iter()
                    .any(|vertex| vertex.position.abs_diff_eq(position, 1e-8))
            );
        }
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        assert_eq!(
            subobject::topology(
                vmf.world
                    .solids
                    .iter()
                    .find(|solid| solid.id == id)
                    .unwrap()
            )
            .unwrap()
            .vertices
            .len(),
            10
        );
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(
            bsp.records::<Brush>()
                .unwrap()
                .iter()
                .any(|brush| brush.side_count >= 7)
        );
        assert!(!bsp.records::<Face>().unwrap().is_empty());
        assert_eq!(
            bsp.records::<Plane>()
                .unwrap()
                .iter()
                .filter(|plane| plane.normal[2] > 0.1 && plane.normal[2] < 0.99)
                .count(),
            2
        );
    }
}
