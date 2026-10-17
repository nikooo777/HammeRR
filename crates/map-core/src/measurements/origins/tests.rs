use super::*;
use crate::{Document, edit, measurements::Measurements};
use source_formats::keyvalues::Value;

fn ruler() -> Measurement {
    Measurement {
        id: 1,
        name: "Entity clearance".into(),
        start: DVec3::new(0.125, 7.5, 32.0),
        end: DVec3::new(300.0, 400.0, 500.0),
        extra: vec![Node::string("future", "retain")],
    }
}

fn source() -> (Document, ObjectId) {
    let mut document = Document::empty();
    let id = document
        .edit("entity", |tx| {
            edit::create_entity(tx, "logic_relay", Some(ruler().start), &[], Vec::new())
        })
        .unwrap();
    (document, id)
}

#[test]
fn origin_references_follow_supported_transforms_without_mutating_measurement_metadata() {
    let (mut document, object) = source();
    let ObjectId::Entity(id) = object else {
        panic!()
    };
    let mut measurement = ruler();
    measurement.set_origin_reference(Endpoint::Start, Some(id));
    let metadata = Measurements {
        measurements: vec![measurement.clone()],
        extra: Vec::new(),
    };
    document
        .edit("measurement", |tx| metadata.clone().write(tx))
        .unwrap();
    let original = document.vmf().clone();
    let metadata_nodes = original.extra.clone();
    for transform in [
        edit::Transform {
            translation: DVec3::new(3.125, -4.25, 5.5),
            ..Default::default()
        },
        edit::Transform {
            rotation: DVec3::new(15.0, 30.0, 90.0),
            pivot: DVec3::new(7.0, 3.0, -8.0),
            ..Default::default()
        },
        edit::Transform {
            scale: DVec3::new(0.5, 1.5, 2.0),
            pivot: DVec3::splat(16.0),
            ..Default::default()
        },
    ] {
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        document
            .edit("transform", |tx| edit::transform(tx, &[object], transform))
            .unwrap();
        assert_eq!(document.undo_labels().count(), history + 1);
        assert_eq!(document.vmf().extra, metadata_nodes);
        let expected = document.entity(id).unwrap().origin().unwrap();
        assert_eq!(
            measurement.resolved(document.vmf()).unwrap().start,
            expected
        );
        let transformed = document.vmf().clone();
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &transformed);
        let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        assert_eq!(Measurements::parse(&reopened).unwrap(), metadata);
        assert_eq!(measurement.resolved(&reopened).unwrap().start, expected);
    }
    assert_ne!(document.vmf().entities, original.entities);
    assert_eq!(measurement.start, ruler().start);
}

#[test]
fn duplicate_keeps_original_reference_and_delete_is_unavailable_until_undo() {
    let (mut document, object) = source();
    let ObjectId::Entity(id) = object else {
        panic!()
    };
    let mut measurement = ruler();
    measurement.set_origin_reference(Endpoint::Start, Some(id));
    document
        .edit("measurement", |tx| {
            Measurements {
                measurements: vec![measurement.clone()],
                extra: Vec::new(),
            }
            .write(tx)
        })
        .unwrap();
    let copy = document
        .edit("duplicate", |tx| {
            edit::duplicate(tx, &[object], DVec3::splat(64.0))
        })
        .unwrap()[0];
    let metadata = Measurements::parse(document.vmf()).unwrap();
    assert_eq!(metadata.measurements.len(), 1);
    assert_eq!(
        metadata.measurements[0]
            .origin_reference(Endpoint::Start)
            .unwrap(),
        Some(id)
    );
    let before_delete = document.vmf().clone();
    document
        .edit("delete original", |tx| edit::delete(tx, &[object]))
        .unwrap();
    assert!(document.contains(copy));
    assert_eq!(document.vmf().extra, before_delete.extra);
    assert!(
        measurement
            .resolved(document.vmf())
            .unwrap_err()
            .contains("missing")
    );
    let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    assert_eq!(Measurements::parse(&reopened).unwrap(), metadata);
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before_delete);
    assert_eq!(
        measurement.resolved(document.vmf()).unwrap().start,
        ruler().start
    );
}

#[test]
fn deleted_origin_ids_remain_reserved_after_save_reopen_create_and_history_navigation() {
    let (mut document, object) = source();
    let ObjectId::Entity(id) = object else {
        panic!()
    };
    let mut measurement = ruler();
    measurement.set_origin_reference(Endpoint::Start, Some(id));
    document
        .edit("measurement", |tx| {
            Measurements {
                measurements: vec![measurement.clone()],
                extra: Vec::new(),
            }
            .write(tx)
        })
        .unwrap();
    let original = document.vmf().clone();
    document
        .edit("delete highest entity", |tx| edit::delete(tx, &[object]))
        .unwrap();
    let missing = document.vmf().clone();
    assert!(
        measurement
            .resolved(&missing)
            .unwrap_err()
            .contains("missing")
    );
    document.undo().unwrap();
    assert_eq!(document.vmf(), &original);
    assert_eq!(
        measurement.resolved(document.vmf()).unwrap().start,
        ruler().start
    );
    document.redo().unwrap();
    assert_eq!(document.vmf(), &missing);
    let reopened = Vmf::parse(&missing.write(&Default::default())).unwrap();
    let mut document = Document::new(reopened.clone());
    let created = document
        .edit("create unrelated entity", |tx| {
            edit::create_entity(tx, "logic_relay", Some(DVec3::splat(64.0)), &[], Vec::new())
        })
        .unwrap();
    assert_eq!(created, ObjectId::Entity(id + 1));
    assert_eq!(document.vmf().extra, reopened.extra);
    assert!(
        measurement
            .resolved(document.vmf())
            .unwrap_err()
            .contains("missing")
    );
    let after_creation = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &reopened);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after_creation);
    let later = document
        .edit("create another unrelated entity", |tx| {
            edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::splat(128.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    assert_eq!(later, ObjectId::Entity(id + 2));
    assert!(!document.contains(object));
    assert!(measurement.resolved(document.vmf()).is_err());
    assert_eq!(document.vmf().extra, reopened.extra);
}

#[test]
fn origin_reservations_skip_only_exact_ids_and_refresh_after_transaction_metadata_changes() {
    let mut document = Document::empty();
    let mut measurement = ruler();
    measurement.set_origin_reference(Endpoint::Start, Some(2));
    measurement.set_origin_reference(Endpoint::End, Some(u32::MAX));
    document
        .edit("orphan origins", |tx| {
            Measurements {
                measurements: vec![measurement.clone()],
                extra: Vec::new(),
            }
            .write(tx)
        })
        .unwrap();
    let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    let mut document = Document::new(reopened.clone());
    let created = document
        .edit("create batch", |tx| {
            let entity =
                edit::create_entity(tx, "logic_relay", Some(DVec3::ZERO), &[], Vec::new())?;
            let solid =
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "tools/toolsnodraw")?;
            let another =
                edit::create_entity(tx, "logic_relay", Some(DVec3::splat(32.0)), &[], Vec::new())?;
            Ok([entity, solid, another])
        })
        .unwrap();
    assert_eq!(
        created,
        [ObjectId::Entity(3), ObjectId::Solid(4), ObjectId::Entity(5)]
    );
    assert_eq!(document.vmf().extra, reopened.extra);
    assert!(measurement.resolved(document.vmf()).is_err());

    let mut document = Document::empty();
    let created = document
        .edit("create around a new reference", |tx| {
            let first = edit::create_entity(tx, "logic_relay", Some(DVec3::ZERO), &[], Vec::new())?;
            let mut measurement = ruler();
            measurement.set_origin_reference(Endpoint::Start, Some(3));
            Measurements {
                measurements: vec![measurement],
                extra: Vec::new(),
            }
            .write(tx)?;
            let second =
                edit::create_entity(tx, "logic_relay", Some(DVec3::splat(32.0)), &[], Vec::new())?;
            Ok([first, second])
        })
        .unwrap();
    assert_eq!(created, [ObjectId::Entity(2), ObjectId::Entity(4)]);
    assert_eq!(
        Measurements::parse(document.vmf()).unwrap().measurements[0]
            .origin_reference(Endpoint::Start)
            .unwrap(),
        Some(3)
    );
}

#[test]
fn malformed_measurements_preserve_parseable_reference_ids_without_blocking_unrelated_creation() {
    let malformed = vec![
        Node::block(
            "measurement",
            vec![
                Node::string("start_entity_origin", "2"),
                Node::string("start_entity_origin", "4"),
                Node::string("end_entity_origin", u32::MAX.to_string()),
                Node::string("end_entity_origin", "invalid"),
                Node::string("end_entity_origin", "0"),
                Node::string("end_entity_origin", "4294967296"),
                Node::string("unrelated", "3"),
            ],
        ),
        Node::string("start_entity_origin", "3"),
        Node::block("unrelated", vec![Node::string("start_entity_origin", "3")]),
    ];
    let mut vmf = Document::empty().into_vmf();
    let mut fields = vec![Node::string("version", "future")];
    fields.extend(malformed);
    vmf.extra.push(Node::block(super::super::KEY, fields));
    assert!(Measurements::parse(&vmf).is_err());
    let mut document = Document::new(Vmf::parse(&vmf.write(&Default::default())).unwrap());
    let created = document
        .edit("create with retained unsupported metadata", |tx| {
            let first = edit::create_entity(tx, "logic_relay", Some(DVec3::ZERO), &[], Vec::new())?;
            let second =
                edit::create_entity(tx, "logic_relay", Some(DVec3::splat(32.0)), &[], Vec::new())?;
            Ok([first, second])
        })
        .unwrap();
    assert_eq!(created, [ObjectId::Entity(3), ObjectId::Entity(5)]);
    assert_eq!(document.vmf().extra, vmf.extra);
    assert!(Measurements::parse(document.vmf()).is_err());
    for (blocks, expected) in [
        (vec![Node::string(super::super::KEY, "malformed")], 2),
        (
            vec![Node::block(
                super::super::KEY,
                vec![
                    Node::string("version", "1"),
                    Node::block(
                        "measurement",
                        vec![Node::string("start_entity_origin", "invalid")],
                    ),
                ],
            )],
            2,
        ),
        (
            vec![Node::block(
                super::super::KEY,
                vec![
                    Node::string("version", "1"),
                    Node::block(
                        "measurement",
                        vec![Node::string("start_entity_origin", "2")],
                    ),
                ],
            )],
            3,
        ),
        (
            (2..=3)
                .map(|id| {
                    Node::block(
                        super::super::KEY,
                        vec![
                            Node::string("version", "1"),
                            Node::block(
                                "measurement",
                                vec![Node::string("end_entity_origin", id.to_string())],
                            ),
                        ],
                    )
                })
                .collect(),
            4,
        ),
    ] {
        let mut vmf = Document::empty().into_vmf();
        vmf.extra = blocks;
        let mut document = Document::new(vmf.clone());
        let created = document
            .edit("create", |tx| {
                edit::create_entity(tx, "logic_relay", Some(DVec3::ZERO), &[], Vec::new())
            })
            .unwrap();
        assert_eq!(created, ObjectId::Entity(expected));
        assert_eq!(document.vmf().extra, vmf.extra);
        assert!(Measurements::parse(document.vmf()).is_err());
    }
}

#[test]
fn unsupported_targets_and_invalid_authored_origins_have_no_fallback() {
    let (mut document, object) = source();
    let ObjectId::Entity(id) = object else {
        panic!()
    };
    let mut duplicate_id = document.vmf().clone();
    duplicate_id.entities.push(duplicate_id.entities[0].clone());
    let mut measurement = ruler();
    measurement.set_origin_reference(Endpoint::Start, Some(id));
    let reference_block = Node::block(
        super::super::KEY,
        vec![Node::string("version", "1"), measurement.to_node()],
    );
    duplicate_id.extra.push(reference_block.clone());
    let original = duplicate_id.clone();
    assert!(
        entity_origin(&duplicate_id, object)
            .unwrap_err()
            .contains("ID is duplicated")
    );
    assert_eq!(
        Measurements::parse(&duplicate_id).unwrap().measurements[0],
        measurement
    );
    assert!(measurement.resolved(&duplicate_id).is_err());
    assert_eq!(duplicate_id, original);
    let mut duplicate_origin = document.vmf().clone();
    duplicate_origin.entities[0]
        .properties
        .0
        .push(("Origin".into(), "9 8 7".into()));
    duplicate_origin.extra.push(reference_block);
    let original = duplicate_origin.clone();
    assert!(
        entity_origin(&duplicate_origin, object)
            .unwrap_err()
            .contains("duplicate origin")
    );
    assert_eq!(
        Measurements::parse(&duplicate_origin).unwrap().measurements[0],
        measurement
    );
    assert!(measurement.resolved(&duplicate_origin).is_err());
    assert_eq!(duplicate_origin, original);
    assert!(
        entity_origin(document.vmf(), ObjectId::Solid(2))
            .unwrap_err()
            .contains("point entities only")
    );
    for value in [
        None,
        Some("not a point"),
        Some("NaN 0 0"),
        Some("65537 0 0"),
    ] {
        let changes = [("origin".into(), value.map(str::to_string))];
        document
            .edit("origin", |tx| edit::set_properties(tx, id, &changes))
            .unwrap();
        assert!(entity_origin(document.vmf(), object).is_err());
        document.undo().unwrap();
    }
    document
        .edit("brush entity", |tx| {
            let brush =
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "tools/toolsnodraw")?;
            let ObjectId::Solid(solid) = brush else {
                panic!()
            };
            let crate::Object::Solid(solid) = tx.remove(ObjectId::Solid(solid))? else {
                panic!()
            };
            tx.entity_mut(id)?.solids.push(solid);
            Ok(())
        })
        .unwrap();
    assert!(
        entity_origin(document.vmf(), object)
            .unwrap_err()
            .contains("brush entity")
    );
}

#[test]
fn legacy_record_nodes_and_endpoint_spellings_survive_other_edits_and_undo() {
    let mut fields = ruler().to_node().children().unwrap().to_vec();
    *fields.iter_mut().find(|node| node.key == "start").unwrap() =
        Node::string("start", "+0.125  7.5000 32e0");
    *fields.iter_mut().find(|node| node.key == "id").unwrap() = Node::string("id", "0001");
    fields.swap(0, 3);
    let record = Node::block("measurement", fields);
    let mut vmf = Document::empty().into_vmf();
    vmf.extra.push(Node::block(
        super::super::KEY,
        vec![
            Node::string("future_root", "retain first"),
            record.clone(),
            Node::string("version", "1"),
        ],
    ));
    let mut document = Document::new(vmf.clone());
    let mut metadata = Measurements::parse(document.vmf()).unwrap();
    let mut second = ruler();
    second.id = 2;
    second.name = "Second".into();
    metadata.measurements.push(second);
    document.edit("add", |tx| metadata.write(tx)).unwrap();
    let block = document.vmf().extra[0].children().unwrap();
    assert_eq!(block[0], Node::string("future_root", "retain first"));
    assert_eq!(block[1], record);
    let mut changed = Measurements::parse(document.vmf()).unwrap();
    changed.measurements[0].name = "Renamed".into();
    document.edit("rename", |tx| changed.write(tx)).unwrap();
    let renamed = document.vmf().extra[0].children().unwrap()[1]
        .children()
        .unwrap();
    for key in ["id", "start", "end", "future"] {
        assert_eq!(
            renamed.iter().find(|node| node.key == key),
            record
                .children()
                .unwrap()
                .iter()
                .find(|node| node.key == key)
        );
    }
    document.undo().unwrap();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &vmf);
    let metadata = Measurements::parse(document.vmf()).unwrap();
    document.edit("no-op", |tx| metadata.write(tx)).unwrap();
    assert_eq!(document.vmf(), &vmf);
    assert_eq!(document.undo_labels().count(), 0);
}

#[test]
fn malformed_origin_metadata_is_preserved_and_cannot_be_overwritten() {
    for references in [
        vec![Node::string("start_entity_origin", "0")],
        vec![Node::string("start_entity_origin", "entity:2")],
        vec![Node::block("end_entity_origin", Vec::new())],
        vec![
            Node::string("start_entity_origin", "2"),
            Node::string("start_entity_origin", "3"),
        ],
    ] {
        let mut node = ruler().to_node();
        let Value::Block(fields) = &mut node.value else {
            panic!()
        };
        fields.extend(references);
        let mut vmf = Document::empty().into_vmf();
        vmf.extra.push(Node::block(
            super::super::KEY,
            vec![Node::string("version", "1"), node],
        ));
        let mut document = Document::new(vmf.clone());
        assert!(Measurements::parse(document.vmf()).is_err());
        assert!(
            document
                .edit("overwrite", |tx| Measurements::default().write(tx))
                .is_err()
        );
        assert_eq!(document.vmf(), &vmf);
        assert_eq!(document.undo_labels().count(), 0);
    }
}
