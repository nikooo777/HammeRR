use super::*;
use crate::{edit, transfer};
use glam::DVec3;
use source_formats::keyvalues::{Node, WriteOptions};

fn object(document: &mut Document) -> ObjectId {
    document
        .edit("brush", |tx| {
            edit::create_block(tx, DVec3::ZERO, DVec3::splat(32.0), "brick/test")
        })
        .unwrap()
}
fn layer(document: &mut Document, name: &str, parent: Option<u32>, members: Vec<ObjectId>) -> u32 {
    document
        .edit("layer", |tx| {
            apply(
                tx,
                &Edit {
                    id: None,
                    name: name.into(),
                    color: [40, 160, 220],
                    parent,
                    members,
                    delete: false,
                },
            )
        })
        .unwrap()
}

#[test]
fn records_members_and_nested_moves_share_atomic_undo_redo_and_metadata() {
    let mut document = Document::empty();
    let a = object(&mut document);
    let b = object(&mut document);
    let root = layer(&mut document, "Architecture", None, vec![a]);
    let child = layer(&mut document, "Trim", Some(root), vec![b]);
    document
        .edit("imported extras", |tx| {
            let mut groups = tx.document().vmf().visgroups.clone().unwrap();
            groups[0].extra.push(Node::block(
                "custom",
                vec![Node::string("value", "preserve")],
            ));
            tx.set_visgroups(Some(groups));
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    document
        .edit("move layer", |tx| {
            apply(
                tx,
                &Edit {
                    id: Some(child),
                    name: "Details".into(),
                    color: [220, 40, 80],
                    parent: None,
                    members: vec![a, b],
                    delete: false,
                },
            )
        })
        .unwrap();
    let graph = Graph::read(&document).unwrap();
    assert_eq!(graph.records[&child].parent, None);
    assert_eq!(
        graph.memberships[&a].layers,
        [root, child].into_iter().collect()
    );
    assert_eq!(graph.members(root, true, true).unwrap(), vec![a]);
    assert_eq!(document.undo_labels().count(), history + 1);
    assert_eq!(
        document.vmf().visgroups.as_ref().unwrap()[0].extra,
        before.visgroups.as_ref().unwrap()[0].extra
    );
    let after = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after);
    assert_eq!(
        Vmf::parse(&document.vmf().write(&WriteOptions::default())).unwrap(),
        after
    );
}

#[test]
fn deleting_a_layer_promotes_children_and_only_removes_its_membership() {
    let mut document = Document::empty();
    let a = object(&mut document);
    let root = layer(&mut document, "Root", None, vec![a]);
    let child = layer(&mut document, "Child", Some(root), vec![a]);
    let grandchild = layer(&mut document, "Grandchild", Some(child), vec![]);
    let before = document.vmf().clone();
    document
        .edit("delete layer", |tx| {
            apply(
                tx,
                &Edit {
                    id: Some(child),
                    name: String::new(),
                    color: [0; 3],
                    parent: Some(root),
                    members: vec![],
                    delete: true,
                },
            )
        })
        .unwrap();
    let graph = Graph::read(&document).unwrap();
    assert_eq!(graph.records[&grandchild].parent, Some(root));
    assert_eq!(graph.memberships[&a].layers, [root].into_iter().collect());
    assert!(
        document
            .solid(match a {
                ObjectId::Solid(id) => id,
                _ => panic!(),
            })
            .is_some()
    );
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
}

#[test]
fn invalid_parent_member_or_exhausted_id_rolls_back_layers_members_and_allocators() {
    let mut document = Document::empty();
    let a = object(&mut document);
    let root = layer(&mut document, "Root", None, vec![a]);
    let child = layer(&mut document, "Child", Some(root), vec![]);
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    for edit in [
        Edit {
            id: Some(root),
            name: "Root".into(),
            color: [0; 3],
            parent: Some(child),
            members: vec![a],
            delete: false,
        },
        Edit {
            id: None,
            name: "Missing".into(),
            color: [0; 3],
            parent: None,
            members: vec![ObjectId::Entity(999)],
            delete: false,
        },
    ] {
        assert!(document.edit("invalid", |tx| apply(tx, &edit)).is_err());
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
    let expected = document
        .preview_snapshot()
        .edit("next", |tx| tx.new_visgroup_id())
        .unwrap();
    let _: Result<(), _> = document.edit("fail after allocation", |tx| {
        tx.new_visgroup_id()?;
        tx.set_visgroups(Some(Vec::new()));
        Err(invalid("failed"))
    });
    assert_eq!(document.vmf(), &before);
    let next = layer(&mut document, "After failure", None, vec![]);
    assert_eq!(next, expected);
    let mut vmf = document.vmf().clone();
    vmf.visgroups.as_mut().unwrap()[0].id = u32::MAX;
    for solid in &mut vmf.world.solids {
        solid
            .editor
            .as_mut()
            .unwrap()
            .0
            .retain(|(key, _)| key != "visgroupid");
    }
    let mut exhausted = Document::new(vmf);
    assert!(
        exhausted
            .edit("exhausted", |tx| apply(
                tx,
                &Edit {
                    id: None,
                    name: "No ID".into(),
                    color: [0; 3],
                    parent: None,
                    members: vec![],
                    delete: false
                }
            ))
            .is_err()
    );
}

#[test]
fn owned_brush_memberships_report_exact_scope_and_select_whole_entity() {
    let mut document = Document::empty();
    let a = object(&mut document);
    let ObjectId::Solid(a) = a else { panic!() };
    let owner = document
        .edit("tie", |tx| {
            edit::tie_to_entity(tx, &[a], "func_detail", &[])
        })
        .unwrap();
    let root = layer(&mut document, "Owned", None, vec![ObjectId::Solid(a)]);
    let graph = Graph::read(&document).unwrap();
    assert_eq!(graph.memberships[&ObjectId::Solid(a)].owner, owner);
    assert_eq!(graph.members(root, true, false).unwrap(), vec![owner]);
    assert!(graph.memberships[&owner].layers.is_empty());
    let ObjectId::Entity(owner) = owner else {
        panic!()
    };
    document
        .edit("hidden child", |tx| {
            tx.entity_mut(owner)?.solids[0].hidden = true;
            Ok(())
        })
        .unwrap();
    assert!(
        Graph::read(&document)
            .unwrap()
            .members(root, true, false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn clipboard_clones_only_required_layer_ancestors_in_a_distinct_id_namespace() {
    let mut source = Document::empty();
    let a = object(&mut source);
    let b = object(&mut source);
    let root = layer(&mut source, "Root", None, vec![]);
    let child = layer(&mut source, "Child", Some(root), vec![a]);
    layer(&mut source, "Unselected", None, vec![b]);
    let payload = transfer::Payload::capture(&source, &[a], &Default::default()).unwrap();
    let payload = transfer::Payload::decode(payload.text(), &Default::default()).unwrap();
    assert_eq!(Graph::read(payload.document()).unwrap().records.len(), 2);
    let mut destination = Document::empty();
    let existing = object(&mut destination);
    let old_layer = layer(&mut destination, "Existing", None, vec![existing]);
    let before = destination.vmf().clone();
    let result = destination
        .edit("paste layers", |tx| {
            transfer::paste(
                tx,
                &payload,
                transfer::Options::default(),
                &Default::default(),
            )
        })
        .unwrap();
    assert_eq!(result.visgroups.len(), 2);
    assert!(
        result
            .visgroups
            .iter()
            .all(|(old, new)| old != new && *new != old_layer)
    );
    let graph = Graph::read(&destination).unwrap();
    let mapped_child = result
        .visgroups
        .iter()
        .find(|pair| pair.0 == child)
        .unwrap()
        .1;
    assert_eq!(
        graph.memberships[&result.objects[0]].layers,
        [mapped_child].into_iter().collect()
    );
    assert_eq!(
        graph.memberships[&existing].layers,
        [old_layer].into_iter().collect()
    );
    let after = destination.vmf().clone();
    destination.undo().unwrap();
    assert_eq!(destination.vmf(), &before);
    destination.redo().unwrap();
    assert_eq!(destination.vmf(), &after);
}

#[test]
fn imported_layer_metadata_is_bounded_before_cloning_or_reparenting() {
    let mut document = Document::empty();
    let id = layer(&mut document, "Root", None, vec![]);
    let mut nested = Node::string("leaf", "value");
    for _ in 0..64 {
        nested = Node::block("custom", vec![nested]);
    }
    document
        .edit("import metadata", |tx| {
            let mut groups = tx.document().vmf().visgroups.clone().unwrap();
            groups[0].extra.push(nested);
            tx.set_visgroups(Some(groups));
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    assert!(
        Graph::read(&document)
            .unwrap_err()
            .to_string()
            .contains("64 levels")
    );
    assert!(
        document
            .edit("edit", |tx| apply(
                tx,
                &Edit {
                    id: Some(id),
                    name: "Other".into(),
                    color: [0, 0, 0],
                    parent: None,
                    members: vec![],
                    delete: false
                }
            ))
            .is_err()
    );
    assert_eq!(document.vmf(), &before);
}
