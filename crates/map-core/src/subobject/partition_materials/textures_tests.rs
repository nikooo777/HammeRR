use super::*;
use crate::{partition_metadata, texel_uv};
use source_formats::vmf::Vmf;

fn texture(index: usize) -> PartitionTexture {
    PartitionTexture {
        shift: [[13.125, -27.5], [-41.75, 6.25]][index],
        scale: [[-0.375, 0.625], [0.25, -0.5]][index],
        rotation: [37.0, -73.0][index],
    }
}

#[test]
fn independent_piece_textures_preserve_roof_corners_prism_policies_and_history() {
    for owned in [false, true] {
        for reflected in [false, true] {
            for direction in 0..2 {
                for attached in [false, true] {
                    let (document, part) = tests::fixture(owned, reflected, direction);
                    let original = document.vmf().clone();
                    let source = crate::surface::side(
                        &document,
                        crate::surface::FaceId {
                            solid: part.source.solid,
                            side: part.source.side,
                        },
                    )
                    .unwrap();
                    let textures = (0..2)
                        .map(|index| PieceTexture {
                            solid: part.source.solid,
                            partition: index as u32,
                            texture: texture(index),
                        })
                        .collect::<Vec<_>>();
                    let materials = tests::materials(&part);
                    let mut raised = document.clone();
                    raised
                        .edit("roof textures", |tx| {
                            crease_partitions_with_surfaces(
                                tx,
                                std::slice::from_ref(&part),
                                16.125,
                                attached,
                                &materials,
                                &textures,
                            )
                        })
                        .unwrap();
                    let heights = part.positions(16.125);
                    for piece in &part.loops {
                        let roof = solid(&raised, part.source.solid)
                            .unwrap()
                            .1
                            .sides
                            .iter()
                            .find(|side| {
                                side.material
                                    == materials[piece.id as usize].material.to_ascii_uppercase()
                            })
                            .unwrap();
                        let projected = texture(piece.id as usize).project(source).unwrap();
                        let mut expected = source.clone();
                        expected.u_axis = projected.u;
                        expected.v_axis = projected.v;
                        for vertex in &piece.vertices {
                            let before = part.vertices[*vertex as usize].position;
                            let after = heights[*vertex as usize];
                            assert!(texel_uv(roof, after).abs_diff_eq(
                                texel_uv(&expected, if attached { before } else { after }),
                                1e-7
                            ));
                        }
                        assert_eq!(roof.lightmap_scale, source.lightmap_scale);
                        assert_eq!(roof.smoothing_groups, source.smoothing_groups);
                    }
                    let changed = raised.vmf().clone();
                    raised.undo().unwrap();
                    assert_eq!(raised.vmf(), &original);
                    raised.redo().unwrap();
                    assert_eq!(raised.vmf(), &changed);
                    let spec = ExtrusionSpec {
                        height: 32.125,
                        wall_material: Some("concrete/wall".into()),
                        texture_lock: attached,
                    };
                    let selection = [(part.source.solid, 0), (part.source.solid, 1)];
                    let baseline = plan_partition_extrusion_with_materials(
                        &document,
                        std::slice::from_ref(&part),
                        &selection,
                        &spec,
                        &materials,
                    )
                    .unwrap();
                    let plan = plan_partition_extrusion_with_surfaces(
                        &document,
                        std::slice::from_ref(&part),
                        &selection,
                        &spec,
                        &materials,
                        &textures,
                    )
                    .unwrap();
                    for (piece, inherited) in plan.pieces.iter().zip(&baseline.pieces) {
                        assert_eq!(piece.solid.sides[1..], inherited.solid.sides[1..]);
                        let projection = texture(piece.partition as usize).project(source).unwrap();
                        let mut expected = source.clone();
                        expected.u_axis = projection.u;
                        expected.v_axis = projection.v;
                        for vertex in &part.loops[piece.partition as usize].vertices {
                            let before = part.vertices[*vertex as usize].position;
                            let after = before
                                + Plane::from_points(part.plane).unwrap().normal * spec.height;
                            assert!(texel_uv(&piece.solid.sides[0], after).abs_diff_eq(
                                texel_uv(&expected, if attached { before } else { after }),
                                1e-7
                            ));
                        }
                    }
                    assert_eq!(document.vmf(), &original);
                }
            }
        }
    }
}

#[test]
fn saved_texture_plans_reopen_reflect_and_keep_original_piece_membership() {
    for direction in 0..6 {
        let (mut document, original_part) = tests::fixture(false, false, 0);
        let brush = document.solid(original_part.source.solid).unwrap();
        let part = plan_partitions(
            &document,
            &[Crease {
                edges: partition_pairs(brush, original_part.source.side).unwrap()[direction].edges,
                ..original_part.source.clone()
            }],
        )
        .unwrap()
        .remove(0);
        let source = document.vmf().clone();
        document
            .edit("keep surfaces", |tx| {
                partition_metadata::store_partition_surfaces(
                    tx,
                    &part,
                    &[Some("brick/planned".into()), None],
                    &[Some(texture(0)), Some(texture(1))],
                )
            })
            .unwrap();
        assert_eq!(document.vmf().world.solids, source.world.solids);
        let vmf = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        let mut reopened = Document::new(vmf);
        reopened
            .edit("reflect", |tx| {
                crate::edit::transform(
                    tx,
                    &[ObjectId::Solid(part.source.solid)],
                    crate::transform::Transform {
                        reflection: [true, false, false],
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let metadata = partition_metadata::Metadata::parse(reopened.vmf()).unwrap();
        let intent = &metadata.entries[&(part.source.solid, part.source.side)];
        assert_eq!(intent.top_textures, [Some(texture(0)), Some(texture(1))]);
        let resolved = intent.resolve(&reopened).unwrap();
        for piece in &resolved.loops {
            let members = |part: &Partition, piece: &PartitionLoop| {
                piece
                    .vertices
                    .iter()
                    .filter_map(|vertex| match &part.vertices[*vertex as usize].source {
                        VertexSource::Corner(sides) => Some(sides.clone()),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>()
            };
            assert_eq!(
                members(&resolved, piece),
                members(&part, &part.loops[piece.id as usize])
            );
        }
        reopened
            .edit("clear plans", |tx| {
                partition_metadata::store_partition_surfaces(
                    tx,
                    &resolved,
                    &[None, None],
                    &[None, None],
                )
            })
            .unwrap();
        let intent = &partition_metadata::Metadata::parse(reopened.vmf())
            .unwrap()
            .entries[&(part.source.solid, part.source.side)];
        assert_eq!(intent.top_textures, [None, None]);
    }
}

#[test]
fn invalid_texture_plans_reject_before_geometry_or_identifier_changes() {
    let (mut document, part) = tests::fixture(false, false, 0);
    let before = document.vmf().clone();
    for invalid in [
        PartitionTexture {
            scale: [0.0, 1.0],
            ..texture(0)
        },
        PartitionTexture {
            shift: [f64::NAN, 0.0],
            ..texture(0)
        },
        PartitionTexture {
            rotation: f64::INFINITY,
            ..texture(0)
        },
        PartitionTexture {
            scale: [1e-9, 1.0],
            ..texture(0)
        },
        PartitionTexture {
            shift: [65537.0, 0.0],
            ..texture(0)
        },
    ] {
        assert!(
            document
                .edit("invalid plan", |tx| {
                    partition_metadata::store_partition_surfaces(
                        tx,
                        &part,
                        &[None, None],
                        &[Some(invalid), None],
                    )
                })
                .is_err()
        );
        assert!(
            document
                .edit("invalid roof", |tx| crease_partitions_with_surfaces(
                    tx,
                    std::slice::from_ref(&part),
                    16.0,
                    false,
                    &[],
                    &[PieceTexture {
                        solid: part.source.solid,
                        partition: 0,
                        texture: invalid
                    }]
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
    }
    let valid = PieceTexture {
        solid: part.source.solid,
        partition: 0,
        texture: texture(0),
    };
    let spec = ExtrusionSpec {
        height: 16.0,
        wall_material: None,
        texture_lock: false,
    };
    assert!(
        plan_partition_extrusion_with_surfaces(
            &document,
            std::slice::from_ref(&part),
            &[(part.source.solid, 0)],
            &spec,
            &[],
            &[valid.clone(), valid]
        )
        .is_err()
    );
    assert_eq!(document.vmf(), &before);
}

#[test]
fn texture_metadata_preserves_unknown_fields_and_rejects_malformed_or_conflicting_plans() {
    let (mut document, part) = tests::fixture(false, false, 0);
    let key = (part.source.solid, part.source.side);
    let mut metadata = partition_metadata::Metadata::parse(document.vmf()).unwrap();
    metadata
        .entries
        .get_mut(&key)
        .unwrap()
        .extra
        .push(source_formats::keyvalues::Node::string("vendor", "keep"));
    document
        .edit("unknown field", |tx| metadata.write(tx))
        .unwrap();
    document
        .edit("texture plan", |tx| {
            partition_metadata::store_partition_surfaces(
                tx,
                &part,
                &[None, None],
                &[Some(texture(0)), None],
            )
        })
        .unwrap();
    let metadata = partition_metadata::Metadata::parse(document.vmf()).unwrap();
    assert_eq!(
        metadata.entries[&key].extra[0],
        source_formats::keyvalues::Node::string("vendor", "keep")
    );
    let text = document.vmf().write(&Default::default());
    for invalid in [
        "NaN -27.5 -0.375 0.625 37",
        "13.125 -27.5 0 0.625 37",
        "13.125 -27.5 -0.375 0.625 inf",
        "13.125 -27.5 -0.375 0.625",
        "13.125 -27.5 -0.375 0.625 37 8",
    ] {
        let malformed = text.replace("13.125 -27.5 -0.375 0.625 37", invalid);
        assert_ne!(malformed, text);
        let vmf = Vmf::parse(&malformed).unwrap();
        assert!(partition_metadata::Metadata::parse(&vmf).is_err());
        assert!(vmf.write(&Default::default()).contains(invalid));
    }
    document
        .edit("clear texture", |tx| {
            partition_metadata::store_partition_surfaces(tx, &part, &[None, None], &[None, None])
        })
        .unwrap();
    let mut metadata = partition_metadata::Metadata::parse(document.vmf()).unwrap();
    metadata
        .entries
        .get_mut(&key)
        .unwrap()
        .extra
        .push(source_formats::keyvalues::Node::string(
            "top_texture_0",
            "vendor-owned",
        ));
    document
        .edit("reserved future field", |tx| metadata.write(tx))
        .unwrap();
    let before = document.vmf().clone();
    assert!(
        document
            .edit("conflicting plan", |tx| {
                partition_metadata::store_partition_surfaces(
                    tx,
                    &part,
                    &[None, None],
                    &[Some(texture(0)), None],
                )
            })
            .is_err()
    );
    assert_eq!(document.vmf(), &before);
}
