use super::*;
use crate::{ObjectId, Plane, edit, primitive};
use glam::DVec3;

fn fixture(corners: u32, owned: bool) -> (Document, u32, u32) {
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
    if owned {
        document
            .edit("detail", |tx| {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])
            })
            .unwrap();
    }
    let source = subobject::solid(&document, id).unwrap().1;
    let side = source
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    (document, id, side)
}

#[test]
fn polygon_membership_survives_reflection_reopening_rekeep_and_independent_materials() {
    for corners in [3, 5, 6, 8] {
        for owned in [false, true] {
            let (original, id, side) = fixture(corners, owned);
            let pairs =
                subobject::partition_pairs(subobject::solid(&original, id).unwrap().1, side)
                    .unwrap();
            for pair in pairs {
                for edge_parameters in [[0.5; 2], [0.2, 0.8], [0.8, 0.2]] {
                    let mut document = original.clone();
                    let part = subobject::plan_partitions_at(
                        &document,
                        &[subobject::PartitionCut {
                            source: Crease {
                                solid: id,
                                side,
                                edges: pair.edges,
                            },
                            edge_parameters,
                        }],
                    )
                    .unwrap()
                    .remove(0);
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
                    let key = (id, side);
                    let mut metadata = Metadata::parse(document.vmf()).unwrap();
                    assert_eq!(metadata.entries[&key].corners.len(), corners as usize);
                    assert_eq!(
                        metadata.entries[&key]
                            .piece_membership
                            .as_ref()
                            .unwrap()
                            .iter()
                            .map(Vec::len)
                            .sum::<usize>(),
                        corners as usize
                    );
                    metadata.extra.push(Node::string("future_root", "preserve"));
                    metadata
                        .entries
                        .get_mut(&key)
                        .unwrap()
                        .extra
                        .push(Node::string("future_piece", "preserve"));
                    document.edit("extra", |tx| metadata.write(tx)).unwrap();
                    let text = document.vmf().write(&Default::default());
                    assert!(text.contains(POLYGON_MATERIAL_KIND));
                    let mut reopened = Document::new(Vmf::parse(&text).unwrap());
                    reopened
                        .edit("reflect affine", |tx| {
                            edit::transform(
                                tx,
                                &[part.owner],
                                edit::Transform {
                                    reflection: [true, false, false],
                                    rotation: DVec3::new(17.0, -23.0, 31.0),
                                    scale: DVec3::new(1.125, 0.75, 1.5),
                                    ..Default::default()
                                },
                            )
                        })
                        .unwrap();
                    let saved = Metadata::parse(reopened.vmf()).unwrap();
                    let resolved = saved.resolve(&reopened).remove(0).result.unwrap();
                    assert_eq!(
                        resolved
                            .loops
                            .each_ref()
                            .map(|piece| piece_corners(&resolved, piece)),
                        membership
                    );
                    assert_eq!(resolved.cut().unwrap().edge_parameters, edge_parameters);
                    let history = reopened.undo_labels().count();
                    let stored = reopened.vmf().clone();
                    reopened
                        .edit("rekeep", |tx| store_partition(tx, &resolved))
                        .unwrap();
                    assert_eq!(reopened.vmf(), &stored);
                    assert_eq!(reopened.undo_labels().count(), history);
                    let current = subobject::plan_partitions_at(
                        &reopened,
                        &[subobject::PartitionCut {
                            source: resolved.source.clone(),
                            edge_parameters: [0.3, 0.7],
                        }],
                    )
                    .unwrap()
                    .remove(0);
                    reopened
                        .edit("change cut", |tx| store_partition(tx, &current))
                        .unwrap();
                    let updated = Metadata::parse(reopened.vmf()).unwrap();
                    assert_eq!(updated.entries[&key].corners, saved.entries[&key].corners);
                    assert_eq!(
                        updated.entries[&key].piece_membership,
                        saved.entries[&key].piece_membership
                    );
                    assert_eq!(
                        updated.entries[&key].top_materials,
                        saved.entries[&key].top_materials
                    );
                    assert_eq!(updated.extra, saved.extra);
                    assert_eq!(updated.entries[&key].extra, saved.entries[&key].extra);
                    reopened
                        .edit("inherit", |tx| {
                            set_piece_top_materials(tx, &resolved.source, &[0, 1], None)
                        })
                        .unwrap();
                    let text = reopened.vmf().write(&Default::default());
                    assert!(text.contains(POLYGON_KIND) && !text.contains(POLYGON_MATERIAL_KIND));
                    let unstyled = Metadata::parse(reopened.vmf()).unwrap();
                    assert_eq!(
                        unstyled.entries[&key].piece_membership,
                        saved.entries[&key].piece_membership
                    );
                    reopened.vmf().validate_ids().unwrap();
                }
            }
        }
    }
}

#[test]
fn malformed_polygon_membership_and_legacy_kinds_reject_writes_atomically() {
    let (mut document, id, side) = fixture(8, false);
    let pair = subobject::partition_pairs(document.solid(id).unwrap(), side).unwrap()[0].clone();
    document
        .edit("keep", |tx| {
            store(
                tx,
                &Crease {
                    solid: id,
                    side,
                    edges: pair.edges,
                },
            )
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let key = (id, side);
    let metadata = Metadata::parse(&before).unwrap();
    for failure in 0..6 {
        let mut invalid = metadata.clone();
        let intent = invalid.entries.get_mut(&key).unwrap();
        let membership = intent.piece_membership.as_mut().unwrap();
        match failure {
            0 => membership[0].clear(),
            1 => {
                let duplicate = membership[1][0].clone();
                membership[1].push(duplicate);
            }
            2 => membership[0][0] = vec![side; subobject::MAX_SIDES + 1],
            3 => membership[0][0] = membership[1][0].clone(),
            4 => intent.corners.push(intent.corners[0].clone()),
            _ => intent.piece_membership = None,
        }
        let raw = invalid.node(true);
        let mut malformed = before.clone();
        let index = malformed
            .extra
            .iter()
            .position(|node| node.key == KEY)
            .unwrap();
        malformed.extra[index] = raw;
        assert!(Metadata::parse(&malformed).is_err());
        assert!(document.edit("invalid", |tx| invalid.write(tx)).is_err());
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
    let text = before.write(&Default::default());
    for kind in [
        KIND,
        EDGE_KIND,
        MATERIAL_KIND,
        EDGE_MATERIAL_KIND,
        BOUNDARY_KIND,
        BOUNDARY_MATERIAL_KIND,
    ] {
        let malformed = Vmf::parse(&text.replace(POLYGON_KIND, kind)).unwrap();
        let mut rejected = Document::new(malformed.clone());
        assert!(Metadata::parse(&malformed).is_err());
        assert!(
            rejected
                .edit("replace", |tx| store(tx, &metadata.entries[&key].source))
                .is_err()
        );
        assert_eq!(rejected.vmf(), &malformed);
    }
    let mut reserved = metadata.clone();
    reserved
        .entries
        .get_mut(&key)
        .unwrap()
        .extra
        .push(Node::string(MEMBERSHIP_FIELDS[0], "1 2 3"));
    assert!(document.edit("reserved", |tx| reserved.write(tx)).is_err());
    let mut oversized = metadata.clone();
    oversized
        .entries
        .get_mut(&key)
        .unwrap()
        .piece_membership
        .as_mut()
        .unwrap()[1]
        .resize(subobject::MAX_BOUNDARY_SOURCE_CORNERS + 1, vec![1, 2, 3]);
    assert!(
        document
            .edit("oversized", |tx| oversized.write(tx))
            .is_err()
    );
    assert_eq!(document.vmf(), &before);
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
}
