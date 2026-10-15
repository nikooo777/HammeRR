use super::*;
use crate::{context, groups::Member};
use source_formats::vmf::Connection;

fn entity(document: &mut Document, name: &str) -> ObjectId {
    document
        .edit("entity", |tx| {
            edit::create_entity(
                tx,
                "info_target",
                Some(DVec3::new(24.0, 32.0, 40.0)),
                &[("targetname".into(), name.into())],
                Vec::new(),
            )
        })
        .unwrap()
}
fn number(id: ObjectId) -> u32 {
    match id {
        ObjectId::Entity(id) | ObjectId::Solid(id) => id,
    }
}
fn brush(document: &mut Document) -> ObjectId {
    document
        .edit("brush", |tx| {
            edit::create_block(tx, DVec3::ZERO, DVec3::splat(32.0), "brick/test")
        })
        .unwrap()
}
fn group(document: &mut Document, name: &str, parent: Option<u32>, members: Vec<Member>) -> u32 {
    document
        .edit("group", |tx| {
            groups::apply(
                tx,
                &groups::Edit {
                    group: None,
                    name: name.into(),
                    parent,
                    members,
                    ungroup: false,
                },
            )
        })
        .unwrap()
}

#[test]
fn detached_payload_survives_source_changes_and_destination_id_collisions() {
    let catalog = Catalog::default();
    let mut source = Document::empty();
    let a = brush(&mut source);
    let b = entity(&mut source, "child");
    source
        .edit("metadata", |tx| {
            tx.entity_mut(number(b))?.extra.push(Node::block(
                "custom",
                vec![Node::string("value", "retained")],
            ));
            context::set_note(
                tx,
                Scope::Object(a),
                "source note".into(),
                "remember".into(),
                "human",
                None,
            )
        })
        .unwrap();
    let before = source.vmf().clone();
    let payload = Payload::capture(&source, &[a, b], &catalog).unwrap();
    assert_eq!(source.vmf(), &before);
    source
        .edit("remove source", |tx| edit::delete(tx, &[a, b]))
        .unwrap();
    let mut destination = Document::empty();
    let collision = entity(&mut destination, "destination");
    assert_eq!(number(collision), number(a));
    let original = destination.vmf().clone();
    let history = destination.undo_labels().count();
    let decoded = Payload::decode(payload.text(), &catalog).unwrap();
    let result = destination
        .edit("paste", |tx| {
            paste(
                tx,
                &decoded,
                Options {
                    offset: DVec3::X * 80.0,
                    ..Default::default()
                },
                &catalog,
            )
        })
        .unwrap();
    assert_eq!(
        destination
            .entity(number(collision))
            .unwrap()
            .properties
            .get("targetname"),
        Some("destination")
    );
    for (old, new) in &result.mapping {
        assert_ne!(number(*old), number(*new));
    }
    for (old, new) in &result.sides {
        assert_ne!(old, new);
    }
    assert_eq!(destination.undo_labels().count(), history + 1);
    let pasted = destination.solid(number(result.objects[0])).unwrap();
    let original_brush = &before.world.solids[0];
    for (a, b) in original_brush.sides.iter().zip(&pasted.sides) {
        assert!(
            crate::texel_uv(a, a.plane.0[0])
                .abs_diff_eq(crate::texel_uv(b, a.plane.0[0] + DVec3::X * 80.0), 1e-8)
        );
    }
    let note = &destination.context().unwrap().notes[&Scope::Object(result.objects[0])];
    assert_eq!(note.author, "human");
    assert_eq!(
        note.observed,
        payload.document.context().unwrap().notes[&Scope::Object(a)].observed
    );
    assert_eq!(
        destination.entity(number(result.objects[1])).unwrap().extra[0],
        before.entities[0].extra[0]
    );
    let after = destination.vmf().clone();
    let reopened =
        Document::try_new(Vmf::parse(&after.write(&keyvalues::WriteOptions::default())).unwrap())
            .unwrap();
    assert_eq!(reopened.vmf(), &after);
    destination.undo().unwrap();
    assert_eq!(destination.vmf(), &original);
    destination.redo().unwrap();
    assert_eq!(destination.vmf(), &after);
}

#[test]
fn partial_nested_groups_and_owned_brush_memberships_receive_new_ids() {
    let mut source = Document::empty();
    let a = brush(&mut source);
    let b = entity(&mut source, "member");
    let other = entity(&mut source, "other");
    let root = group(&mut source, "root", None, vec![Member::Object(other)]);
    let child = group(
        &mut source,
        "child",
        Some(root),
        vec![Member::Object(a), Member::Object(b)],
    );
    let payload = Payload::capture(&source, &[a, b], &Catalog::default()).unwrap();
    assert_eq!(payload.document.vmf().world.groups.len(), 2);
    assert_eq!(payload.document.vmf().entities.len(), 1);
    let mut destination = Document::empty();
    let result = destination
        .edit("paste", |tx| {
            paste(tx, &payload, Options::default(), &Catalog::default())
        })
        .unwrap();
    let new_root = result.groups.iter().find(|pair| pair.0 == root).unwrap().1;
    let new_child = result.groups.iter().find(|pair| pair.0 == child).unwrap().1;
    let graph = groups::Graph::read(&destination).unwrap();
    assert_eq!(graph.parents[&new_child], Some(new_root));
    assert_eq!(graph.objects[&result.objects[0]], Some(new_child));
    assert_eq!(graph.objects[&result.objects[1]], Some(new_child));
    assert_ne!(new_child, child);
}

#[test]
fn internal_names_and_parent_connection_spans_remap_external_links_are_explicit() {
    let mut source = Document::empty();
    let a = entity(&mut source, "parent");
    let b = entity(&mut source, "child");
    source
        .edit("links", |tx| {
            let entity = tx.entity_mut(number(b))?;
            entity.properties.set("parentname", "parent,attachment");
            entity.connections = Some(vec![
                Connection {
                    output: "OnUser1".into(),
                    value: "parent\u{1b}SetParent\u{1b}parent\u{1b}0.00\u{1b}-1".into(),
                },
                Connection {
                    output: "OnUser2".into(),
                    value: "outside,Trigger,,0,-1".into(),
                },
            ]);
            Ok(())
        })
        .unwrap();
    let payload = Payload::capture(&source, &[a, b], &Catalog::default()).unwrap();
    let mut destination = Document::empty();
    let outside = entity(&mut destination, "outside");
    let before = destination.vmf().clone();
    let history = destination.undo_labels().count();
    assert!(
        destination
            .edit("bad paste", |tx| paste(
                tx,
                &payload,
                Options::default(),
                &Catalog::default()
            ))
            .is_err()
    );
    assert_eq!(destination.vmf(), &before);
    assert_eq!(destination.undo_labels().count(), history);
    let result = destination
        .edit("paste", |tx| {
            paste(
                tx,
                &payload,
                Options {
                    keep_external: true,
                    ..Default::default()
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    assert_eq!(result.external[0].matches, vec![number(outside)]);
    let child = destination.entity(number(result.objects[1])).unwrap();
    assert_eq!(
        child.properties.get("parentname"),
        Some("parent_copy1,attachment")
    );
    assert_eq!(
        child.connections.as_ref().unwrap()[0].value,
        "parent_copy1\u{1b}SetParent\u{1b}parent_copy1\u{1b}0.00\u{1b}-1"
    );
    assert_eq!(
        child.connections.as_ref().unwrap()[1].value,
        "outside,Trigger,,0,-1"
    );
    let second = destination
        .edit("paste again", |tx| {
            paste(
                tx,
                &payload,
                Options {
                    keep_external: true,
                    ..Default::default()
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    assert!(
        second
            .names
            .iter()
            .all(|name| name.after.ends_with("_copy1_2"))
    );
}

#[test]
fn invalid_payload_limits_links_groups_and_bounds_preserve_destination() {
    let catalog = Catalog::default();
    for text in [
        "world {",
        "world { id 1 }\0",
        "hammerr_transfer { version 999 } world { id 1 }",
        "world { id 1 } entity { id 2 classname info_target } entity { id 2 classname info_target }",
    ] {
        assert!(Payload::decode(text, &catalog).is_err(), "{text}");
    }
    let depth = format!(
        "{}{}",
        "a {".repeat(MAX_DEPTH + 1),
        "}".repeat(MAX_DEPTH + 1)
    );
    assert!(Payload::decode(&depth, &catalog).is_err());
    assert!(Payload::decode(&" ".repeat(MAX_BYTES + 1), &catalog).is_err());
    let mut source = Document::empty();
    let a = brush(&mut source);
    source
        .edit("membership", |tx| {
            tx.solid_mut(number(a))?
                .editor
                .as_mut()
                .unwrap()
                .set("visgroupid", "5");
            Ok(())
        })
        .unwrap();
    assert!(Payload::capture(&source, &[a], &catalog).is_err());
    source.undo().unwrap();
    let payload = Payload::capture(&source, &[a], &catalog).unwrap();
    let mut destination = Document::empty();
    let before = destination.vmf().clone();
    for offset in [DVec3::splat(f64::NAN), DVec3::splat(65536.0)] {
        assert!(
            destination
                .edit("invalid", |tx| paste(
                    tx,
                    &payload,
                    Options {
                        offset,
                        ..Default::default()
                    },
                    &catalog
                ))
                .is_err()
        );
        assert_eq!(destination.vmf(), &before);
        assert_eq!(destination.undo_labels().count(), 0);
    }
}

#[test]
fn owned_brush_links_notes_and_native_terrain_transfer_without_rebinding_source_ids() {
    let mut source = Document::empty();
    let owned = brush(&mut source);
    let owner = source
        .edit("tie", |tx| {
            edit::tie_to_entity(tx, &[number(owned)], "func_detail", &[])
        })
        .unwrap();
    let root = group(&mut source, "assembly", None, vec![Member::Object(owner)]);
    source
        .edit("child metadata", |tx| {
            tx.entity_mut(number(owner))?.solids[0]
                .editor
                .as_mut()
                .unwrap()
                .set("groupid", root.to_string());
            context::set_note(
                tx,
                Scope::Object(owned),
                "Owned brush".into(),
                "Keep the child note".into(),
                "agent",
                None,
            )
        })
        .unwrap();
    let terrain = brush(&mut source);
    let solid = source.solid(number(terrain)).unwrap();
    let geometry = crate::BrushGeometry::from_solid(solid).unwrap();
    let top = geometry
        .faces
        .iter()
        .find(|face| face.plane.normal.z > 0.99)
        .unwrap();
    let face = crate::surface::FaceId {
        solid: solid.id,
        side: solid.sides[top.side].id,
    };
    source
        .edit("terrain", |tx| {
            crate::displacement::edit::create(tx, &[face], 2, 1)
        })
        .unwrap();
    let payload = Payload::capture(&source, &[owner, terrain], &Catalog::default()).unwrap();
    let payload = Payload::decode(payload.text(), &Catalog::default()).unwrap();
    let mut destination = Document::empty();
    let offset = DVec3::new(64.0, 32.0, 8.0);
    let result = destination
        .edit("paste", |tx| {
            paste(
                tx,
                &payload,
                Options {
                    offset,
                    ..Default::default()
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    let new_owned = result
        .mapping
        .iter()
        .find(|pair| pair.0 == owned)
        .unwrap()
        .1;
    let new_owner = result
        .mapping
        .iter()
        .find(|pair| pair.0 == owner)
        .unwrap()
        .1;
    let new_terrain = result
        .mapping
        .iter()
        .find(|pair| pair.0 == terrain)
        .unwrap()
        .1;
    let entity = destination.entity(number(new_owner)).unwrap();
    assert_eq!(
        groups::parent(entity.solids[0].editor.as_ref()).unwrap(),
        Some(result.groups[0].1)
    );
    assert!(
        destination
            .context()
            .unwrap()
            .notes
            .contains_key(&Scope::Object(new_owned))
    );
    let original = source
        .solid(number(terrain))
        .unwrap()
        .sides
        .iter()
        .find_map(|side| side.displacement.as_ref())
        .unwrap();
    let pasted = destination
        .solid(number(new_terrain))
        .unwrap()
        .sides
        .iter()
        .find_map(|side| side.displacement.as_ref())
        .unwrap();
    assert_eq!(pasted.start_position, original.start_position + offset);
    assert_eq!(pasted.normals, original.normals);
    assert_eq!(pasted.distances, original.distances);
    destination.vmf().validate_ids().unwrap();
}

#[test]
fn capture_refuses_unpasteable_geometry_unencodable_fields_and_reference_capture() {
    let mut source = Document::empty();
    let named = entity(&mut source, "parent");
    source
        .edit("unencodable", |tx| {
            tx.entity_mut(number(named))?
                .properties
                .set("message", "literal\"quote");
            Ok(())
        })
        .unwrap();
    assert!(Payload::capture(&source, &[named], &Catalog::default()).is_err());
    source.undo().unwrap();
    source
        .edit("unpasteable", |tx| {
            tx.entity_mut(number(named))?
                .properties
                .set("origin", "100000 0 0");
            Ok(())
        })
        .unwrap();
    assert!(Payload::capture(&source, &[named], &Catalog::default()).is_err());
    source.undo().unwrap();
    source
        .edit("name acquisition", |tx| {
            tx.entity_mut(number(named))?.connections = Some(vec![Connection {
                output: "OnUser1".into(),
                value: "parent_copy1,Trigger,,0,-1".into(),
            }]);
            Ok(())
        })
        .unwrap();
    assert!(
        Payload::capture(&source, &[named], &Catalog::default())
            .unwrap_err()
            .to_string()
            .contains("acquire")
    );
}
