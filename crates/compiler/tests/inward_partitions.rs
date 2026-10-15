mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit, subobject};
use source_formats::bsp::{Brush, BrushSide, Bsp, Face, Plane};

fn occupied(bsp: &Bsp, point: DVec3) -> bool {
    let planes = bsp.records::<Plane>().unwrap();
    let sides = bsp.records::<BrushSide>().unwrap();
    bsp.records::<Brush>().unwrap().iter().any(|brush| {
        sides[brush.first_side as usize..(brush.first_side + brush.side_count) as usize]
            .iter()
            .all(|side| {
                let plane = &planes[side.plane as usize];
                DVec3::from_array(plane.normal.map(f64::from)).dot(point) - f64::from(plane.dist)
                    <= 0.001
            })
    })
}

#[test]
fn inward_pockets_and_through_openings_compile_after_vmf_reopening() {
    for owned in [false, true] {
        for through in [false, true] {
            for parameters in [[0.5; 2], [0.2, 0.8]] {
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
                let (_, brush) = subobject::solid(&document, id).unwrap();
                let side = brush
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                    })
                    .unwrap()
                    .id;
                let partitions = subobject::plan_partitions_at(
                    &document,
                    &[subobject::PartitionCut {
                        source: subobject::Crease {
                            solid: id,
                            side,
                            edges: subobject::crease_pairs(brush, side).unwrap()[0].edges,
                        },
                        edge_parameters: parameters,
                    }],
                )
                .unwrap();
                document
                    .edit("keep", |tx| {
                        map_core::partition_metadata::store_partition(tx, &partitions[0])
                    })
                    .unwrap();
                let selected = if through {
                    vec![(id, 0)]
                } else {
                    vec![(id, 0), (id, 1)]
                };
                let result = document
                    .edit("cut inward", |tx| {
                        subobject::extrude_partitions_with_materials(
                            tx,
                            &partitions,
                            &selected,
                            &subobject::ExtrusionSpec {
                                height: if through { -128.0 } else { -16.0 },
                                wall_material: Some("dev/dev_measurewall01a".into()),
                                texture_lock: true,
                            },
                            &[
                                subobject::PartitionMaterial {
                                    solid: id,
                                    partition: 0,
                                    material: "floor/first".into(),
                                },
                                subobject::PartitionMaterial {
                                    solid: id,
                                    partition: 1,
                                    material: "floor/second".into(),
                                },
                            ],
                        )
                    })
                    .unwrap();
                let vmf =
                    source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default()))
                        .unwrap();
                vmf.validate_ids().unwrap();
                assert!(
                    map_core::partition_metadata::Metadata::parse(&vmf)
                        .unwrap()
                        .entries
                        .is_empty()
                );
                for solid in vmf.solids() {
                    subobject::topology(solid).unwrap();
                }
                let output = compiler::compile(&vmf, &common::empty_game());
                assert!(!output.report.has_errors(), "{:?}", output.report);
                assert!(output.leak.is_none());
                assert_eq!(output.stats.brushes, 6 + result.fragments.len());
                let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
                assert!(!bsp.records::<Face>().unwrap().is_empty());
                for piece in &partitions[0].loops {
                    let center = piece
                        .vertices
                        .iter()
                        .map(|index| partitions[0].vertices[*index as usize].position)
                        .sum::<DVec3>()
                        / piece.vertices.len() as f64;
                    assert_eq!(
                        occupied(&bsp, center - DVec3::Z * 8.0),
                        through && piece.id == 1
                    );
                    assert_eq!(
                        occupied(&bsp, center - DVec3::Z * 48.0),
                        !through || piece.id == 1
                    );
                }
                if !through {
                    for material in ["FLOOR/FIRST", "FLOOR/SECOND"] {
                        assert!(
                            vmf.solids()
                                .flat_map(|solid| &solid.sides)
                                .any(|side| side.material == material)
                        );
                    }
                }
            }
        }
    }
}
