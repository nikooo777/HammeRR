use super::*;
use crate::{ObjectId, edit};
use glam::DVec3;

fn fixture(direction: usize, parameters: [f64; 2], owned: bool) -> (Document, Partition) {
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
        .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let edges = subobject::partition_pairs(source, side).unwrap()[direction + 2].edges;
    let part = subobject::plan_partitions_at(
        &document,
        &[subobject::PartitionCut {
            source: Crease {
                solid: id,
                side,
                edges,
            },
            edge_parameters: parameters,
        }],
    )
    .unwrap()
    .remove(0);
    (document, part)
}

#[test]
fn boundary_intents_reopen_reflect_and_keep_original_corner_membership() {
    for owned in [false, true] {
        for reflected in [false, true] {
            for direction in 0..4 {
                for parameters in [[0.5; 2], [0.2, 0.8], [0.8, 0.2]] {
                    let (mut document, part) = fixture(direction, parameters, owned);
                    let membership = part
                        .loops
                        .each_ref()
                        .map(|piece| piece_corners(&part, piece));
                    let before = document.vmf().clone();
                    document
                        .edit("keep", |tx| {
                            store_partition_materials(
                                tx,
                                &part,
                                &[Some("brick/red".into()), Some("wood/blue".into())],
                            )
                        })
                        .unwrap();
                    assert_eq!(document.vmf().world, before.world);
                    assert_eq!(document.vmf().entities, before.entities);
                    let mut metadata = Metadata::parse(document.vmf()).unwrap();
                    let key = (part.source.solid, part.source.side);
                    assert!(metadata.entries[&key].adjacent);
                    metadata
                        .entries
                        .get_mut(&key)
                        .unwrap()
                        .extra
                        .push(Node::string("future_surface", "preserve"));
                    document.edit("future", |tx| metadata.write(tx)).unwrap();
                    let text = document.vmf().write(&Default::default());
                    assert!(text.contains(BOUNDARY_MATERIAL_KIND));
                    let mut reopened = Document::new(Vmf::parse(&text).unwrap());
                    let target = if owned {
                        part.owner
                    } else {
                        ObjectId::Solid(part.source.solid)
                    };
                    reopened
                        .edit("affine", |tx| {
                            edit::transform(
                                tx,
                                &[target],
                                edit::Transform {
                                    reflection: [reflected, false, false],
                                    rotation: DVec3::new(17.0, -23.0, 31.0),
                                    ..Default::default()
                                },
                            )
                        })
                        .unwrap();
                    let metadata = Metadata::parse(reopened.vmf()).unwrap();
                    let resolved = metadata.resolve(&reopened).remove(0).result.unwrap();
                    assert_eq!(resolved.cut().unwrap().edge_parameters, parameters);
                    assert_eq!(
                        resolved
                            .loops
                            .each_ref()
                            .map(|piece| piece_corners(&resolved, piece)),
                        membership
                    );
                    assert_eq!(
                        resolved.loops.each_ref().map(|piece| piece.vertices.len()),
                        [3, 5]
                    );
                    assert_eq!(
                        metadata.entries[&key].top_materials,
                        [Some("brick/red".into()), Some("wood/blue".into())]
                    );
                    assert_eq!(
                        metadata.entries[&key].extra,
                        vec![Node::string("future_surface", "preserve")]
                    );
                    reopened
                        .edit("inherit", |tx| {
                            set_piece_top_materials(tx, &resolved.source, &[0, 1], None)
                        })
                        .unwrap();
                    let metadata = Metadata::parse(reopened.vmf()).unwrap();
                    assert!(metadata.entries[&key].adjacent);
                    assert_eq!(metadata.entries[&key].top_materials, [None, None]);
                    assert!(
                        reopened
                            .vmf()
                            .write(&Default::default())
                            .contains(BOUNDARY_KIND)
                    );
                    reopened.vmf().validate_ids().unwrap();
                }
            }
        }
    }
}

#[test]
fn boundary_kinds_reject_legacy_reinterpretation_and_reserved_collisions() {
    let (mut document, part) = fixture(0, [0.5; 2], false);
    document
        .edit("keep", |tx| {
            store_partition_materials(tx, &part, &[Some("brick/red".into()), None])
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let text = before.write(&Default::default());
    for kind in [KIND, EDGE_KIND, MATERIAL_KIND, EDGE_MATERIAL_KIND] {
        let malformed = Vmf::parse(&text.replace(BOUNDARY_MATERIAL_KIND, kind)).unwrap();
        assert!(Metadata::parse(&malformed).is_err());
    }
    let key = (part.source.solid, part.source.side);
    let mut metadata = Metadata::parse(&before).unwrap();
    metadata
        .entries
        .get_mut(&key)
        .unwrap()
        .extra
        .push(Node::string(PARAMETERS_FIELD, "0.2 0.8"));
    assert!(document.edit("conflict", |tx| metadata.write(tx)).is_err());
    assert_eq!(document.vmf(), &before);
    assert_eq!(document.undo_labels().count(), history);
}
