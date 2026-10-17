use super::*;
use crate::measurements::{Endpoint, Measurement, Measurements};
use glam::DVec3;
use std::cell::Cell;

fn fixture() -> (Document, u32, u32) {
    let mut document = Document::empty();
    let (first, second) = document
        .edit("assembly", |tx| {
            let mut solids = Vec::new();
            for x in [0.0, 64.0] {
                let ObjectId::Solid(id) = crate::edit::create_block(
                    tx,
                    DVec3::new(x, 0.0, 0.0),
                    DVec3::new(x + 16.0, 16.0, 16.0),
                    "test",
                )?
                else {
                    unreachable!()
                };
                context::set_note(
                    tx,
                    Scope::Object(ObjectId::Solid(id)),
                    format!("Brush {id}"),
                    String::new(),
                    "system",
                    None,
                )?;
                solids.push(id);
            }
            let reserved = tx.doc.next_object_id as u32;
            let mut metadata = tx.document().context()?.clone();
            metadata.extra.push(Node::block(
                "future_context",
                vec![Node::string("opaque", "retain")],
            ));
            metadata.notes.insert(
                Scope::Object(ObjectId::Entity(reserved)),
                context::Note {
                    summary: "Reserved missing object".into(),
                    body: String::new(),
                    author: "system".into(),
                    observed: String::new(),
                    region: None,
                    extra: vec![Node::string("future", "retain")],
                },
            );
            tx.set_context(metadata)?;
            let mut measurement = Measurement {
                id: 1,
                name: "Missing origin".into(),
                start: DVec3::ZERO,
                end: DVec3::splat(16.0),
                extra: Vec::new(),
            };
            measurement.set_origin_reference(Endpoint::Start, Some(reserved + 1));
            Measurements {
                measurements: vec![measurement],
                extra: Vec::new(),
            }
            .write(tx)?;
            tx.set_editor_metadata("opaque", Some(Node::string("opaque", "original")));
            Ok((solids[0], solids[1]))
        })
        .unwrap();
    document
        .edit("redo branch", |tx| {
            tx.world_properties_mut().set("skyname", "other");
            Ok(())
        })
        .unwrap();
    document.undo().unwrap();
    document.mark_saved();
    document.context_map_fingerprint();
    (document, first, second)
}

fn stage(tx: &mut Transaction, first: u32, second: u32) -> Result<ObjectId, EditError> {
    let next = tx.doc.next_object_id as u32;
    assert_eq!(tx.new_object_id()?, next + 2);
    tx.new_side_id()?;
    tx.new_visgroup_id()?;
    let group = tx.new_object_id()?;
    tx.groups_mut().push(Group {
        id: group,
        editor: None,
        extra_properties: Properties::default(),
        extra: Vec::new(),
    });
    tx.set_visgroups(Some(vec![VisGroup {
        name: "Staged".into(),
        id: 8_000,
        color: source_formats::vmf::Color([30, 60, 90]),
        children: Vec::new(),
        extra_properties: Properties::default(),
        extra: Vec::new(),
    }]));
    assert_eq!(tx.new_visgroup_id()?, 8_001);
    let mut entity = Entity::new(5_000, "func_detail");
    let mut solid = tx.document().solid(first).unwrap().clone();
    solid.id = 6_000;
    for (index, side) in solid.sides.iter_mut().enumerate() {
        side.id = 7_000 + index as u32;
    }
    entity.solids.push(solid);
    let added = tx.add_entity(entity)?;
    assert_eq!(tx.new_object_id()?, 6_001);
    assert_eq!(tx.new_side_id()?, 7_006);
    tx.remove(ObjectId::Solid(first))?;
    tx.solid_mut(second)?.hidden = true;
    tx.world_properties_mut().set("skyname", "staged");
    tx.set_cordon(Some(Node::block(
        "cordons",
        vec![Node::string("active", "1")],
    )));
    tx.set_editor_metadata("opaque", Some(Node::string("opaque", "staged")));
    Ok(added)
}

fn assert_history(actual: &[Step], expected: &[Step]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.label, expected.label);
        assert_eq!(actual.world, expected.world);
        assert_eq!(actual.extra, expected.extra);
        assert_eq!(actual.cordon, expected.cordon);
        assert_eq!(actual.groups, expected.groups);
        assert_eq!(actual.visgroups, expected.visgroups);
        assert_eq!(actual.changes.len(), expected.changes.len());
        for (actual, expected) in actual.changes.iter().zip(&expected.changes) {
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.before, expected.before);
            assert_eq!(actual.after, expected.after);
        }
    }
}

fn assert_unchanged(actual: &Document, expected: &Document) {
    assert_eq!(actual.vmf, expected.vmf);
    assert_eq!(actual.revision, expected.revision);
    assert_eq!(actual.saved_revision, expected.saved_revision);
    assert_eq!(actual.is_dirty(), expected.is_dirty());
    assert_eq!(actual.next_object_id, expected.next_object_id);
    assert_eq!(actual.next_side_id, expected.next_side_id);
    assert_eq!(actual.next_visgroup_id, expected.next_visgroup_id);
    assert_eq!(actual.context(), expected.context());
    assert_eq!(
        actual.context_map_fingerprint(),
        expected.context_map_fingerprint()
    );
    assert_history(&actual.undo, &expected.undo);
    assert_history(&actual.redo, &expected.redo);
}

#[test]
fn closure_failure_never_finalizes_and_restores_all_reserved_state() {
    for saved in [false, true] {
        let (mut document, first, second) = fixture();
        if !saved {
            document.mark_unsaved();
        }
        let before = document.clone();
        let called = Cell::new(false);
        let result = document.edit_with_finalizer(
            "failed edit",
            |tx| {
                stage(tx, first, second)?;
                Err::<ObjectId, _>(EditError::Invalid("edit rejected".into()))
            },
            |_| {
                called.set(true);
                Ok(())
            },
        );
        assert_eq!(result, Err(EditError::Invalid("edit rejected".into())));
        assert!(!called.get());
        assert_unchanged(&document, &before);
    }
}

#[test]
fn cleanup_failure_never_finalizes_and_restores_original_context() {
    let (mut document, first, second) = fixture();
    let before = document.clone();
    let called = Cell::new(false);
    let result = document.edit_with_finalizer(
        "failed cleanup",
        |tx| {
            let added = stage(tx, first, second)?;
            let mut invalid = tx.document().context()?.clone();
            invalid
                .notes
                .get_mut(&Scope::Object(ObjectId::Solid(second)))
                .unwrap()
                .author = "invalid cleanup fixture".into();
            tx.doc.context_cache = OnceLock::from(Ok(invalid));
            Ok(added)
        },
        |_| {
            called.set(true);
            Ok(())
        },
    );
    assert!(result.unwrap_err().to_string().contains("Context author"));
    assert!(!called.get());
    assert_unchanged(&document, &before);
}

#[test]
fn publication_failure_restores_cleanup_ids_history_and_saved_identity() {
    for saved in [false, true] {
        let (mut document, first, second) = fixture();
        if !saved {
            document.mark_unsaved();
        }
        let before = document.clone();
        let calls = Cell::new(0);
        let result = document.edit_with_finalizer(
            "failed publication",
            |tx| stage(tx, first, second),
            |added| {
                assert_eq!(*added, ObjectId::Entity(5_000));
                calls.set(calls.get() + 1);
                Err(EditError::Invalid("publication rejected".into()))
            },
        );
        assert_eq!(
            result,
            Err(EditError::Invalid("publication rejected".into()))
        );
        assert_eq!(calls.get(), 1);
        assert_unchanged(&document, &before);
        document.redo().unwrap();
        assert_eq!(
            document.vmf().world.properties.get("skyname"),
            Some("other")
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), before.vmf());
    }
}

#[test]
fn successful_publication_commits_one_step_and_history_never_republishes() {
    let (mut document, first, second) = fixture();
    let before = document.clone();
    let calls = Cell::new(0);
    let once = String::from("owned publication");
    let added = document
        .edit_with_finalizer(
            "publish assembly",
            |tx| stage(tx, first, second),
            |added| {
                assert_eq!(*added, ObjectId::Entity(5_000));
                calls.set(calls.get() + 1);
                drop(once);
                Ok(())
            },
        )
        .unwrap();
    assert!(document.contains(added));
    assert!(!document.contains(ObjectId::Solid(first)));
    assert!(
        !document
            .context()
            .unwrap()
            .notes
            .contains_key(&Scope::Object(ObjectId::Solid(first)))
    );
    assert!(
        document
            .context()
            .unwrap()
            .notes
            .contains_key(&Scope::Object(ObjectId::Solid(second)))
    );
    assert_eq!(document.undo.len(), before.undo.len() + 1);
    assert_eq!(document.undo.last().unwrap().label, "publish assembly");
    assert!(document.redo.is_empty());
    assert_eq!(document.revision, before.revision + 1);
    assert!(document.is_dirty());
    let committed = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), before.vmf());
    assert_eq!(document.context(), before.context());
    document.redo().unwrap();
    assert_eq!(document.vmf(), &committed);
    assert_eq!(calls.get(), 1);
    document.vmf().validate_ids().unwrap();
}

fn allocate_without_map_changes(tx: &mut Transaction) -> Result<[u32; 3], EditError> {
    tx.world_properties_mut().set("skyname", "sky_day01_01");
    Ok([
        tx.new_object_id()?,
        tx.new_side_id()?,
        tx.new_visgroup_id()?,
    ])
}

#[test]
fn successful_no_op_finalizes_once_and_preserves_ordinary_allocation_semantics() {
    let (mut document, _, _) = fixture();
    let before = document.clone();
    let mut ordinary = document.clone();
    let expected = ordinary
        .edit("ordinary no-op", allocate_without_map_changes)
        .unwrap();
    let calls = Cell::new(0);
    let actual = document
        .edit_with_finalizer("finalized no-op", allocate_without_map_changes, |ids| {
            assert_eq!(*ids, expected);
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(calls.get(), 1);
    assert_eq!(document.vmf(), before.vmf());
    assert_eq!(document.revision, before.revision);
    assert_eq!(document.is_dirty(), before.is_dirty());
    assert_history(&document.undo, &before.undo);
    assert_history(&document.redo, &before.redo);
    assert_unchanged(&document, &ordinary);
    assert_eq!(document.next_object_id, before.next_object_id + 3);
    assert_eq!(document.next_side_id, before.next_side_id + 1);
    assert_eq!(document.next_visgroup_id, before.next_visgroup_id + 1);
}

#[test]
fn rejected_no_op_finalizer_restores_allocations_and_redo() {
    let (mut document, _, _) = fixture();
    let before = document.clone();
    let calls = Cell::new(0);
    let result =
        document.edit_with_finalizer("rejected no-op", allocate_without_map_changes, |_| {
            calls.set(calls.get() + 1);
            Err(EditError::Invalid("reject no-op".into()))
        });
    assert_eq!(result, Err(EditError::Invalid("reject no-op".into())));
    assert_eq!(calls.get(), 1);
    assert_unchanged(&document, &before);
}
