use super::*;
use crate::context::{Note, Scope};
use source_formats::vmf::Connection;

fn options(count: u32) -> Options {
    Options { count, translation: DVec3::X * 64.0, rotation: DVec3::ZERO, pivot: DVec3::ZERO, texture_lock: true }
}

fn entity(document: &mut Document, name: &str, origin: DVec3) -> ObjectId {
    document.edit("entity", |tx| edit::create_entity(tx, "info_target", Some(origin), &[("targetname".into(), name.into())], Vec::new())).unwrap()
}

fn number(id: ObjectId) -> u32 {
    let ObjectId::Entity(id) = id else { panic!() };
    id
}

#[test]
fn mixed_array_remaps_only_copied_internal_links_and_is_one_undo_step() {
    let mut document = Document::empty();
    let brush = document.edit("block", |tx| edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "brick/wall")).unwrap();
    let a = entity(&mut document, "parent", DVec3::X * 24.0);
    let b = entity(&mut document, "child", DVec3::X * 48.0);
    document.edit("links", |tx| {
        let entity = tx.entity_mut(number(b))?;
        entity.properties.0.extend([("parentname".into(), "parent,attachment".into()), ("custom".into(), "untouched".into())]);
        entity.connections = Some(vec![Connection { output: "OnUser1".into(), value: "parent,SetParent,parent,0.00,-1".into() }, Connection { output: "OnUser2".into(), value: "outside,Trigger,,0,-1".into() }]);
        Ok(())
    }).unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let result = document.edit("repeat array", |tx| create(tx, &[brush, a, b], options(2), &Catalog::default())).unwrap();
    assert_eq!(document.undo_labels().count(), history + 1);
    assert_eq!(result.objects().len(), 6);
    document.vmf().validate_ids().unwrap();
    assert_eq!(document.entity(number(b)), before.entities.iter().find(|entity| entity.id == number(b)));
    for copy in &result.copies {
        let child = document.entity(number(copy.objects[2])).unwrap();
        assert_eq!(child.properties.get("targetname"), Some(format!("child_copy{}", copy.index).as_str()));
        assert_eq!(child.properties.get("parentname"), Some(format!("parent_copy{},attachment", copy.index).as_str()));
        assert_eq!(child.connections.as_ref().unwrap()[0].value, format!("parent_copy{0},SetParent,parent_copy{0},0.00,-1", copy.index));
        assert_eq!(child.connections.as_ref().unwrap()[1].value, "outside,Trigger,,0,-1");
        assert_eq!(copy.remapped_references, 3);
        let ObjectId::Solid(new) = copy.objects[0] else { panic!() };
        let ObjectId::Solid(old) = brush else { panic!() };
        let source = document.solid(old).unwrap();
        let target = document.solid(new).unwrap();
        for (old, new) in source.sides.iter().zip(&target.sides) {
            assert_ne!(old.id, new.id);
            let delta = DVec3::X * 64.0 * f64::from(copy.index);
            assert!(crate::texel_uv(old, old.plane.0[0]).abs_diff_eq(crate::texel_uv(new, old.plane.0[0] + delta), 1e-8));
        }
    }
    let after = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after);
}

#[test]
fn nth_copy_uses_original_and_fixed_pivot_with_world_translation() {
    let mut document = Document::empty();
    let source = entity(&mut document, "", DVec3::X * 10.0);
    let result = document.edit("repeat", |tx| create(tx, &[source], Options { translation: DVec3::Y * 2.0, rotation: DVec3::Z * 90.0, ..options(2) }, &Catalog::default())).unwrap();
    let expected = [DVec3::new(0.0, 12.0, 0.0), DVec3::new(-10.0, 4.0, 0.0)];
    for (copy, expected) in result.copies.iter().zip(expected) {
        let entity = document.entity(number(copy.objects[0])).unwrap();
        assert!(entity.origin().unwrap().abs_diff_eq(expected, 1e-8));
        assert_eq!(entity.properties.get("targetname"), Some(""));
    }
}

#[test]
fn names_avoid_collisions_and_reject_new_reference_acquisition() {
    let mut document = Document::empty();
    let source = entity(&mut document, "part", DVec3::ZERO);
    entity(&mut document, "part_copy1", DVec3::Y * 64.0);
    let result = document.edit("repeat", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap();
    assert_eq!(result.copies[0].names[0].after, "part_copy1_2");
    document.undo().unwrap();
    document.edit("external reference", |tx| {
        tx.entity_mut(number(source))?.connections = Some(vec![Connection { output: "OnUser1".into(), value: "part_copy1_2,Trigger,,0,-1".into() }]);
        Ok(())
    }).unwrap();
    let before = document.vmf().clone();
    assert!(document.edit("repeat", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap_err().to_string().contains("acquire"));
    assert_eq!(document.vmf(), &before);
}

#[test]
fn shared_names_wildcards_and_untyped_internal_text_reject_atomically() {
    for value in ["part", "par*"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        document.edit("unsafe", |tx| {
            tx.entity_mut(number(source))?.properties.set("custom", value);
            Ok(())
        }).unwrap();
        let before = document.vmf().clone();
        assert!(document.edit("repeat", |tx| create(tx, &[source], options(2), &Catalog::default())).is_err());
        assert_eq!(document.vmf(), &before);
    }
    let mut document = Document::empty();
    let source = entity(&mut document, "shared", DVec3::ZERO);
    entity(&mut document, "SHARED", DVec3::X * 16.0);
    assert!(document.edit("repeat", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap_err().to_string().contains("shared"));
}

#[test]
fn limits_and_late_geometry_failures_restore_history_and_allocators() {
    let mut document = Document::empty();
    let source = entity(&mut document, "", DVec3::ZERO);
    let baseline = document.clone();
    for options in [options(0), options(MAX_COPIES + 1), Options { translation: DVec3::ZERO, ..options(1) }, Options { translation: DVec3::X * 40_000.0, ..options(2) }, Options { rotation: DVec3::splat(f64::NAN), ..options(1) }] {
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        assert!(document.edit("rejected", |tx| create(tx, &[source], options, &Catalog::default())).is_err());
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
    let mut control = baseline;
    let expected = control.edit("repeat", |tx| create(tx, &[source], options(2), &Catalog::default())).unwrap();
    let actual = document.edit("repeat", |tx| create(tx, &[source], options(2), &Catalog::default())).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(document.vmf(), control.vmf());
}

#[test]
fn context_copy_budget_is_enforced_and_rolls_back_the_entire_array() {
    let mut document = Document::empty();
    let source = entity(&mut document, "", DVec3::ZERO);
    document.edit("notes", |tx| {
        let mut metadata = tx.document().context()?.clone();
        let note = Note { summary: "n".repeat(200), body: "x".repeat(3800), author: "agent".into(), observed: String::new(), region: None, extra: Vec::new() };
        metadata.notes.insert(Scope::Object(source), note.clone());
        for id in 1000..1259 {
            metadata.notes.insert(Scope::Object(ObjectId::Entity(id)), note.clone());
        }
        tx.set_context(metadata)
    }).unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let error = document.edit("repeat", |tx| create(tx, &[source], options(4), &Catalog::default())).unwrap_err();
    assert!(error.to_string().contains("budget"), "{error}");
    assert_eq!(document.vmf(), &before);
    assert_eq!(document.undo_labels().count(), history);
    let result = document.edit("one copy", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap();
    assert!(document.context().unwrap().notes.contains_key(&Scope::Object(result.objects()[0])));
    assert_eq!(document.context().unwrap().notes[&Scope::Object(source)].body.len(), 3800);
}

#[test]
fn inherited_fgd_references_and_duplicate_esc_connections_follow_each_copy() {
    let catalog = Catalog::parse(r#"
@BaseClass = Linked [
 target(target_destination) : "Target"
 input CustomLink(target_destination) : "Link"
]
@PointClass base(Linked) = info_target []
"#).unwrap();
    let mut document = Document::empty();
    let parent = entity(&mut document, "parent", DVec3::ZERO);
    let child = entity(&mut document, "child", DVec3::Y * 16.0);
    document.edit("links", |tx| {
        let child = tx.entity_mut(number(child))?;
        child.properties.0.extend([("target".into(), "parent".into()), ("target".into(), "PARENT".into())]);
        let connection = Connection { output: "OnUser1".into(), value: "parent\u{1b}CustomLink\u{1b}PARENT\u{1b}0.00\u{1b}01".into() };
        child.connections = Some(vec![connection.clone(), connection]);
        Ok(())
    }).unwrap();
    let before = document.vmf().clone();
    let result = document.edit("repeat", |tx| create(tx, &[parent, child], options(2), &catalog)).unwrap();
    for copy in result.copies {
        let child = document.entity(number(copy.objects[1])).unwrap();
        let name = format!("parent_copy{}", copy.index);
        assert_eq!(child.properties.get_all("target").collect::<Vec<_>>(), [name.as_str(), name.as_str()]);
        let connections = child.connections.as_ref().unwrap();
        assert_eq!(connections[0].value, format!("{name}\u{1b}CustomLink\u{1b}{name}\u{1b}0.00\u{1b}01"));
        assert_eq!(connections[0], connections[1]);
        assert_eq!(copy.remapped_references, 6);
    }
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
}

#[test]
fn owned_brush_notes_copy_with_observations_and_reserved_orphan_ids() {
    let mut document = Document::empty();
    let brush = document.edit("block", |tx| edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "brick/wall")).unwrap();
    let ObjectId::Solid(brush_id) = brush else { panic!() };
    let source = document.edit("tie", |tx| edit::tie_to_entity(tx, &[brush_id], "func_detail", &[])).unwrap();
    let note = Note { summary: "Assembly part".into(), body: "Keep this brush".into(), author: "agent".into(), observed: "0123456789abcdef".into(), region: None, extra: Vec::new() };
    let orphan = ObjectId::Entity(number(source) + 1);
    document.edit("notes", |tx| {
        let mut metadata = tx.document().context()?.clone();
        for id in [source, brush, orphan] {
            metadata.notes.insert(Scope::Object(id), note.clone());
        }
        tx.set_context(metadata)
    }).unwrap();
    let before = document.vmf().clone();
    let result = document.edit("repeat", |tx| create(tx, &[source], options(2), &Catalog::default())).unwrap();
    for copy in result.copies {
        let copied = copy.objects[0];
        assert_ne!(copied, orphan);
        let owned = ObjectId::Solid(document.entity(number(copied)).unwrap().solids[0].id);
        assert_ne!(owned, brush);
        for id in [copied, owned, orphan] {
            assert_eq!(document.context().unwrap().notes[&Scope::Object(id)], note);
        }
    }
    document.vmf().validate_ids().unwrap();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
}

#[test]
fn named_script_and_dynamic_io_sources_reject_without_rejecting_generic_text() {
    for key in ["vscripts", "thinkfunction"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        document.edit("script", |tx| {
            tx.entity_mut(number(source))?.properties.set(key, "script_entry");
            Ok(())
        }).unwrap();
        let before = document.vmf().clone();
        assert!(document.edit("repeat", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap_err().to_string().contains("script"));
        assert_eq!(document.vmf(), &before);
    }
    for input in ["AddOutput", "RunScriptCode", "RunScriptFile"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        document.edit("dynamic I/O", |tx| {
            tx.entity_mut(number(source))?.connections = Some(vec![Connection { output: "OnUser1".into(), value: format!("!self\u{1b}{input}\u{1b}opaque\u{1b}0\u{1b}-1") }]);
            Ok(())
        }).unwrap();
        let before = document.vmf().clone();
        assert!(document.edit("repeat", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap_err().to_string().contains("dynamic I/O"));
        assert_eq!(document.vmf(), &before);
    }
    let mut document = Document::empty();
    let source = entity(&mut document, "part", DVec3::ZERO);
    document.edit("description", |tx| {
        tx.entity_mut(number(source))?.properties.set("description", "The part remains visible near part_copy1");
        Ok(())
    }).unwrap();
    let result = document.edit("repeat", |tx| create(tx, &[source], options(1), &Catalog::default())).unwrap();
    assert_eq!(document.entity(number(result.objects()[0])).unwrap().properties.get("description"), Some("The part remains visible near part_copy1"));
}

#[test]
fn duplicate_classnames_and_numeric_node_or_side_references_reject_atomically() {
    let catalog = Catalog::parse(r#"
@BaseClass = NumericLinks [ link(node_dest) : "Node" faces(sidelist) : "Faces" ]
@PointClass base(NumericLinks) = info_target []
"#).unwrap();
    for key in ["classname", "nodeid", "link", "faces"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "", DVec3::ZERO);
        document.edit("unsafe IDs", |tx| {
            tx.entity_mut(number(source))?.properties.0.push((key.into(), "17".into()));
            Ok(())
        }).unwrap();
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        assert!(document.edit("repeat", |tx| create(tx, &[source], options(1), &catalog)).is_err());
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
}
