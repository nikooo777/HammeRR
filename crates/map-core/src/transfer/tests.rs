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
fn prefab_transform_preserves_source_identity_metadata_and_attached_uvs() {
    let catalog = Catalog::default();
    let mut source = Document::empty();
    let id = brush(&mut source);
    source
        .edit("note", |tx| {
            context::set_note(
                tx,
                Scope::Object(id),
                "Assembly".into(),
                "Keep together".into(),
                "human",
                None,
            )
        })
        .unwrap();
    let payload = Payload::capture(&source, &[id], &catalog).unwrap();
    let original = payload.document().vmf().clone();
    let pivot = DVec3::new(8.0, 4.0, 0.0);
    let transformed = payload
        .transformed(DVec3::Z * 90.0, DVec3::new(2.0, 0.5, 1.25), pivot, &catalog)
        .unwrap();
    assert_eq!(payload.document().vmf(), &original);
    assert_eq!(transformed.ids(), payload.ids());
    assert_eq!(
        transformed.document().context().unwrap(),
        payload.document().context().unwrap()
    );
    let before = payload.document().solid(number(id)).unwrap();
    let after = transformed.document().solid(number(id)).unwrap();
    for (old, new) in before.sides.iter().zip(&after.sides) {
        assert_eq!(old.id, new.id);
        assert_eq!(old.material, new.material);
        assert_eq!(old.lightmap_scale, new.lightmap_scale);
        for (point, actual) in old.plane.0.iter().zip(&new.plane.0) {
            let expected = DVec3::new(
                8.0 - (point.y - 4.0) * 0.5,
                4.0 + (point.x - 8.0) * 2.0,
                point.z * 1.25,
            );
            assert!(actual.abs_diff_eq(expected, 1e-8));
            assert!(crate::texel_uv(old, *point).abs_diff_eq(crate::texel_uv(new, *actual), 1e-8));
        }
    }
    let mut destination = Document::empty();
    let result = destination
        .edit("paste transformed", |tx| {
            paste(tx, &transformed, Options::default(), &catalog)
        })
        .unwrap();
    assert_ne!(result.objects, payload.ids());
    let saved = destination.vmf().write(&keyvalues::WriteOptions::default());
    let reopened = Document::try_new(Vmf::parse(&saved).unwrap()).unwrap();
    assert_eq!(destination.vmf(), reopened.vmf());
    destination.undo().unwrap();
    assert!(destination.vmf().world.solids.is_empty());
    destination.redo().unwrap();
    assert_eq!(destination.vmf(), reopened.vmf());
}

#[test]
fn prefab_rotation_updates_point_origins_orientation_and_internal_parent_links() {
    let catalog = Catalog::default();
    let mut source = Document::empty();
    let parent = entity(&mut source, "anchor");
    let child = entity(&mut source, "child");
    source
        .edit("parent", |tx| {
            tx.entity_mut(number(child))?
                .properties
                .set("parentname", "anchor");
            tx.entity_mut(number(parent))?
                .properties
                .set("angles", "0 30 0");
            Ok(())
        })
        .unwrap();
    let payload = Payload::capture(&source, &[parent, child], &catalog).unwrap();
    let transformed = payload
        .transformed(DVec3::Z * 90.0, DVec3::ONE, DVec3::ZERO, &catalog)
        .unwrap();
    let rotated = transformed.document().entity(number(parent)).unwrap();
    let origin =
        source_formats::vmf::parse_vec3(rotated.properties.get("origin").unwrap()).unwrap();
    let angles =
        source_formats::vmf::parse_vec3(rotated.properties.get("angles").unwrap()).unwrap();
    assert!(origin.abs_diff_eq(DVec3::new(-32.0, 24.0, 40.0), 1e-8));
    assert!((angles.y - 120.0).abs() < 1e-8);
    let mut destination = source.clone();
    let result = destination
        .edit("paste", |tx| {
            paste(tx, &transformed, Options::default(), &catalog)
        })
        .unwrap();
    let copied_parent = destination.entity(number(result.objects[0])).unwrap();
    let copied_child = destination.entity(number(result.objects[1])).unwrap();
    assert_ne!(copied_parent.properties.get("targetname"), Some("anchor"));
    assert_eq!(
        copied_child.properties.get("parentname"),
        copied_parent.properties.get("targetname")
    );
}

#[test]
fn prefab_transform_rejects_point_scale_and_opaque_or_spatial_metadata() {
    let catalog = Catalog::default();
    let mut source = Document::empty();
    let id = entity(&mut source, "point");
    let payload = Payload::capture(&source, &[id], &catalog).unwrap();
    for scale in [
        DVec3::splat(2.0),
        DVec3::ZERO,
        DVec3::splat(-1.0),
        DVec3::splat(f64::NAN),
    ] {
        assert!(
            payload
                .transformed(DVec3::ZERO, scale, DVec3::ZERO, &catalog)
                .is_err()
        );
    }
    source
        .edit("direction", |tx| {
            tx.entity_mut(number(id))?
                .properties
                .set("movedir", "0 90 0");
            Ok(())
        })
        .unwrap();
    let payload = Payload::capture(&source, &[id], &catalog).unwrap();
    assert!(
        payload
            .transformed(DVec3::Z * 90.0, DVec3::ONE, DVec3::ZERO, &catalog)
            .unwrap_err()
            .to_string()
            .contains("movedir")
    );
    assert_eq!(
        payload
            .transformed(DVec3::ZERO, DVec3::ONE, DVec3::ZERO, &catalog)
            .unwrap()
            .text(),
        payload.text()
    );
    let id = brush(&mut source);
    source
        .edit("opaque", |tx| {
            tx.solid_mut(number(id))?
                .extra
                .push(Node::block("custom", vec![Node::string("vector", "1 2 3")]));
            Ok(())
        })
        .unwrap();
    let payload = Payload::capture(&source, &[id], &catalog).unwrap();
    assert!(
        payload
            .transformed(DVec3::Z * 90.0, DVec3::ONE, DVec3::ZERO, &catalog)
            .is_err()
    );
    assert_eq!(
        payload
            .transformed(DVec3::ZERO, DVec3::ONE, DVec3::ZERO, &catalog)
            .unwrap()
            .text(),
        payload.text()
    );
}

#[test]
fn prefab_scale_supports_brush_entities_and_native_terrain_with_child_notes() {
    let catalog = Catalog::default();
    let mut source = Document::empty();
    let owned = brush(&mut source);
    let owner = source
        .edit("owner", |tx| {
            edit::tie_to_entity(tx, &[number(owned)], "func_detail", &[])
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
        .edit("terrain and child note", |tx| {
            crate::displacement::edit::create(tx, &[face], 2, 1)?;
            context::set_note(
                tx,
                Scope::Object(owned),
                "Child".into(),
                "Retained".into(),
                "agent",
                None,
            )
        })
        .unwrap();
    let original = source.vmf().clone();
    let payload = Payload::capture(&source, &[owner, terrain], &catalog).unwrap();
    let transformed = payload
        .transformed(
            DVec3::Z * 90.0,
            DVec3::new(2.0, 0.5, 1.25),
            DVec3::ZERO,
            &catalog,
        )
        .unwrap();
    assert_eq!(source.vmf(), &original);
    let old_solid = payload.document().solid(number(terrain)).unwrap();
    let new_solid = transformed.document().solid(number(terrain)).unwrap();
    let old_geometry = crate::BrushGeometry::from_solid(old_solid).unwrap();
    let new_geometry = crate::BrushGeometry::from_solid(new_solid).unwrap();
    let old_face = old_geometry
        .faces
        .iter()
        .find(|side| old_solid.sides[side.side].id == face.side)
        .unwrap();
    let new_face = new_geometry
        .faces
        .iter()
        .find(|side| new_solid.sides[side.side].id == face.side)
        .unwrap();
    let old_disp = old_solid.sides[old_face.side]
        .displacement
        .as_ref()
        .unwrap();
    let new_disp = new_solid.sides[new_face.side]
        .displacement
        .as_ref()
        .unwrap();
    let old_mesh = crate::DisplacementMesh::build(old_face, old_disp).unwrap();
    let new_mesh = crate::DisplacementMesh::build(new_face, new_disp).unwrap();
    for (old, new) in old_mesh.positions.iter().zip(&new_mesh.positions) {
        assert!(new.abs_diff_eq(DVec3::new(-old.y * 0.5, old.x * 2.0, old.z * 1.25), 1e-8));
    }
    assert_eq!(new_disp.alphas, old_disp.alphas);
    assert_eq!(new_disp.triangle_tags, old_disp.triangle_tags);
    let mut destination = Document::empty();
    let result = destination
        .edit("paste transformed", |tx| {
            paste(tx, &transformed, Options::default(), &catalog)
        })
        .unwrap();
    let new_owned = result
        .mapping
        .iter()
        .find(|(old, _)| *old == owned)
        .unwrap()
        .1;
    assert_ne!(new_owned, owned);
    assert!(
        destination
            .context()
            .unwrap()
            .notes
            .contains_key(&Scope::Object(new_owned))
    );
    let new_owner = result
        .mapping
        .iter()
        .find(|(old, _)| *old == owner)
        .unwrap()
        .1;
    assert_eq!(
        destination.entity(number(new_owner)).unwrap().solids[0].id,
        number(new_owned)
    );
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
