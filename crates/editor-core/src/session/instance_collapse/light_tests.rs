use super::*;
use glam::{DVec3, Vec3};
use source_formats::bsp::{LightingKind, world_light_kind};
use source_formats::vmf::{Properties, Vmf};

fn fixture() -> (tempfile::TempDir, Session, PathBuf, PathBuf, PathBuf) {
    let (directory, game, parent, child) = crate::test_assets::instance_map(false);
    let editor = Properties(vec![
        ("color".into(), "23 110 211".into()),
        ("logicalpos".into(), "[12 24]".into()),
    ]);
    let mut leaf = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    for (id, class, origin) in [
        (1000, "light", "-32 16 96"),
        (1001, "light_spot", "64 -16 96"),
        (1002, "info_target", "24 16 32"),
    ] {
        let mut entity = Entity::new(id, class);
        entity.editor = Some(editor.clone());
        entity.properties.set("origin", origin);
        if class == "info_target" {
            entity.properties.set("targetname", "aim");
        } else {
            entity.properties.set("_light", "$tone");
            entity.properties.set("_lightHDR", "$hdr");
            entity
                .properties
                .set("style", if class == "light" { "1" } else { "11" });
            if class == "light_spot" {
                entity.properties.set("pitch", "-45");
                entity.properties.set("target", "aim");
                entity.properties.set("_inner_cone", "20");
                entity.properties.set("_cone", "50");
                entity.properties.set("_exponent", "2");
            }
        }
        leaf.entities.push(entity);
    }
    std::fs::write(&child, leaf.write(&Default::default())).unwrap();
    let mut middle = Document::empty().into_vmf();
    let mut inner = Entity::new(100, "func_instance");
    for (key, value) in [
        ("file", "child.vmf"),
        ("origin", "4 -6 12"),
        ("angles", "90 0 0"),
        ("targetname", "inner"),
        ("replace01", "$tone $tone"),
        ("replace02", "$hdr $hdr"),
    ] {
        inner.properties.set(key, value);
    }
    middle.entities.push(inner);
    let middle_path = directory.path().join("middle.vmf");
    std::fs::write(&middle_path, middle.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let owner = session.document.vmf().entities.last().unwrap().id;
    session
        .edit(Actor::User, "light assemblies", |tx| {
            let mut north = tx.entity_mut(owner)?.clone();
            for (key, value) in [
                ("file", "middle.vmf"),
                ("origin", "0 -48 64"),
                ("angles", "0 90 0"),
                ("targetname", "north"),
                ("replace01", "$tone 255 80 16 250"),
                ("replace02", "$hdr 16 80 255 450"),
            ] {
                north.properties.set(key, value);
            }
            *tx.entity_mut(owner)? = north.clone();
            let mut south = north;
            south.id = tx.new_object_id()?;
            for (key, value) in [
                ("origin", "0 48 64"),
                ("angles", "0 -90 0"),
                ("targetname", "south"),
                ("replace01", "$tone 64 255 32 175"),
                ("replace02", "$hdr 255 32 96 375"),
            ] {
                south.properties.set(key, value);
            }
            tx.add_entity(south)?;
            let mut light = Entity::new(tx.new_object_id()?, "light");
            light.properties.set("origin", "0 0 200");
            light.properties.set("_light", "64 48 32 120");
            tx.add_entity(light)?;
            for _ in 0..5 {
                tx.new_object_id()?;
            }
            Ok(())
        })
        .unwrap();
    session.save(Actor::User, None).unwrap();
    (directory, session, parent, child, middle_path)
}

fn preview(session: &mut Session) -> Value {
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap()
}

#[test]
fn collapsed_light_assemblies_preserve_targeted_hdr_payload_and_fresh_authored_ids() {
    let (_directory, mut session, parent, child, middle) = fixture();
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let disk = [&parent, &child, &middle].map(|path| std::fs::read(path).unwrap());
    let ready = preview(&mut session);
    assert_eq!(ready["stats"]["point_lights"], 2);
    assert_eq!(ready["stats"]["spot_lights"], 2);
    assert_eq!(ready["stats"]["spotlight_targets"], 2);
    assert_eq!(ready["entity_provenance_total"], 6);
    let shown = session.render_document().vmf().clone();
    let provenance = ready["entity_provenance"].as_array().unwrap();
    let mapping = ready["entity_id_map"].as_array().unwrap();
    assert_eq!(mapping.len(), provenance.len());
    for row in provenance {
        let id = crate::identity::parse_id(row["entity"].as_str().unwrap()).unwrap();
        let ObjectId::Entity(id) = id else {
            panic!("entity provenance must name an entity");
        };
        let entity = shown
            .entities
            .iter()
            .find(|entity| entity.id == id)
            .unwrap();
        assert_eq!(entity.classname(), row["classname"].as_str());
        assert_eq!(
            entity.editor.as_ref().unwrap().get("color"),
            Some("23 110 211")
        );
        assert_eq!(
            entity.editor.as_ref().unwrap().get("logicalpos"),
            Some("[12 24]")
        );
        assert_eq!(row["file"], "child.vmf");
        assert!(matches!(row["original_entity"].as_u64(), Some(1000..=1002)));
    }
    let north_target = shown
        .entities
        .iter()
        .find(|entity| entity.properties.get("targetname") == Some("north-inner-aim"))
        .unwrap();
    let north_spot = shown
        .entities
        .iter()
        .find(|entity| entity.properties.get("target") == Some("north-inner-aim"))
        .unwrap();
    assert!(
        north_target
            .origin()
            .unwrap()
            .abs_diff_eq(DVec3::new(-10., -12., 52.), 1e-8)
    );
    assert!(
        north_spot
            .origin()
            .unwrap()
            .abs_diff_eq(DVec3::new(22., 52., 12.), 1e-8)
    );
    assert_eq!(
        north_spot.properties.get("_lightHDR"),
        Some("16 80 255 450")
    );
    let control = Control::default();
    let context = instances::SourceContext::capture(&parent, &control).unwrap();
    let options = compiler::Options {
        lighting: compiler::LightingMode::DirectLdrHdr,
        ..Default::default()
    };
    let expanded = compiler::compile_with_source_controlled(
        &source,
        &session.game,
        options,
        None,
        Some(&context),
        &control,
    );
    let collapsed = compiler::compile_with_options(&shown, &session.game, options);
    assert!(!expanded.report.has_errors(), "{:?}", expanded.report);
    assert!(!collapsed.report.has_errors(), "{:?}", collapsed.report);
    let mut expanded = expanded.bsp.unwrap();
    let collapsed = collapsed.bsp.unwrap();
    for channel in [LightingKind::Ldr, LightingKind::Hdr] {
        let lights = collapsed.world_lights(channel).unwrap();
        assert_eq!(
            lights.iter().map(|light| light.style).collect::<Vec<_>>(),
            [0, 1, 11, 1, 11]
        );
        assert_eq!(lights[2].kind, world_light_kind::SPOT);
        assert!(Vec3::from_array(lights[1].origin).abs_diff_eq(Vec3::new(-10., 52., 108.), 1e-6));
        assert!(
            Vec3::from_array(lights[2].normal)
                .abs_diff_eq(Vec3::new(-32., -64., 40.).normalize(), 1e-6)
        );
    }
    assert_ne!(
        collapsed.world_lights(LightingKind::Ldr).unwrap()[1].intensity,
        collapsed.world_lights(LightingKind::Hdr).unwrap()[1].intensity
    );
    let original_text = expanded.entities().unwrap();
    let mut replacements = BTreeMap::new();
    for row in mapping {
        let old = row["expanded"]
            .as_str()
            .unwrap()
            .strip_prefix("entity:")
            .unwrap();
        let new = row["authored"]
            .as_str()
            .unwrap()
            .strip_prefix("entity:")
            .unwrap();
        let before = format!("\"hammerid\" \"{old}\"\n");
        let after = format!("\"hammerid\" \"{new}\"\n");
        assert_eq!(original_text.matches(&before).count(), 1);
        assert!(replacements.insert(before, after).is_none());
    }
    assert!(mapping.iter().any(|row| row["expanded"] != row["authored"]));
    let entity_text = original_text
        .split_inclusive('\n')
        .map(|line| replacements.get(line).map_or(line, String::as_str))
        .collect::<String>();
    expanded.set_entities(&entity_text);
    assert_eq!(expanded.to_bytes(), collapsed.to_bytes());
    session
        .cancel_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(session.render_document().vmf(), &source);
    let ready = preview(&mut session);
    let request = session
        .apply_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    let applied = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    assert_eq!(
        applied["captured_preview"]["entity_id_map"],
        ready["entity_id_map"]
    );
    assert_eq!(session.document.vmf(), &shown);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    for (path, bytes) in [&parent, &child, &middle].into_iter().zip(disk) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn collapsed_light_preview_rejects_changed_sources_and_custom_editor_metadata_atomically() {
    let (_directory, mut session, _parent, child, _middle) = fixture();
    let source = session.document.vmf().clone();
    let ready = preview(&mut session);
    let shown = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let bytes = std::fs::read(&child).unwrap();
    let mut invalid = Vmf::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
    invalid
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("custom", "retained?");
    std::fs::write(&child, invalid.write(&Default::default())).unwrap();
    let request = session
        .apply_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let error = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap_err();
    assert!(error.contains("editor metadata custom"), "{error}");
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(session.history_snapshot(), history);
    std::fs::write(child, bytes).unwrap();
    let token = session.get_instance_collapse()["token"].as_u64().unwrap();
    session
        .cancel_instance_collapse(Actor::User, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &source);
}
