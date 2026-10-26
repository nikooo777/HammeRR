use super::*;
use source_formats::bsp::{Bsp, lump};
use source_formats::keyvalues::{FlatBlockLimits, parse_flat_blocks_bounded};
use source_formats::vmf::{Connection, Properties};

fn controller(id: u32, origin: &str) -> Entity {
    let mut entity = Entity::new(id, "env_fog_controller");
    for (key, value) in [
        ("origin", origin),
        ("fogcolor", "128 160 255"),
        ("fogcolor2", "255 255 255"),
        ("fogstart", "100.125"),
        ("fogend", "2000.5"),
        ("fogmaxdensity", "0.75"),
        ("fogenable", "1"),
        ("fogblend", "0"),
        ("fogradial", "0"),
        ("fogdir", "1 0 0"),
        ("use_angles", "0"),
        ("foglerptime", "0"),
        ("farz", "-1"),
        ("HDRColorScale", "1"),
        ("ZoomFogScale", "1"),
        ("spawnflags", "0"),
    ] {
        entity.properties.set(key, value);
    }
    entity
}

fn fog_fixture(entities: Vec<Entity>) -> (tempfile::TempDir, Vmf, GameFs) {
    let (directory, mut parent, _, game) = fixture();
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "24 -16 16");
    write_child(directory.path(), "child.vmf", &helper_file(entities));
    (directory, parent, game)
}

fn literal(parent: &Vmf, children: Vec<Entity>) -> Vmf {
    let mut document = Document::new(parent.clone());
    document
        .edit("literal fog control", |tx| {
            for mut entity in children {
                entity.id = tx.new_object_id()?;
                entity.editor = None;
                tx.add_entity(entity)?;
            }
            for helper in parent.entities.iter().filter(|entity| is_instance(entity)) {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

fn fog_rows(bsp: &Bsp) -> Vec<Properties> {
    parse_flat_blocks_bounded(
        bsp.entities().unwrap(),
        FlatBlockLimits {
            max_bytes: 1024 * 1024,
            max_blocks: 1024,
            max_pairs: 16_384,
        },
    )
    .unwrap()
    .into_iter()
    .map(Properties)
    .filter(|properties| properties.get("classname") == Some("env_fog_controller"))
    .collect()
}

fn output(parent: &Vmf, game: &GameFs, context: Option<&SourceContext>) -> crate::Output {
    let output = compile_with_source_controlled(
        parent,
        game,
        Options::default(),
        None,
        context,
        &Control::default(),
    );
    assert!(
        output.bsp.is_some() && !output.report.has_errors(),
        "{:?}",
        output.report
    );
    output
}

#[test]
fn static_child_fog_matches_literal_bsp_and_preserves_multiple_root_controller_order() {
    for nested in [false, true] {
        let original = controller(20, "4 12 48");
        let (directory, mut parent, game) = fog_fixture(vec![original.clone()]);
        let root_id = parent.object_ids().max().unwrap() + 1;
        let mut root = controller(root_id, "-16 0 64");
        root.properties
            .set("fogstart", "runtime-owned malformed root value");
        parent.entities.insert(0, root);
        let mut second = controller(root_id + 1, "16 0 64");
        second.properties.set("fogcolor", "17 18 19");
        second.properties.set("spawnflags", "1");
        parent.entities.push(second);
        let mut expected = original.clone();
        let owner = parent
            .entities
            .iter()
            .find(|entity| is_instance(entity))
            .unwrap()
            .id;
        if nested {
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "48 -16 32", "0 90 0")]),
            );
            let helper = parent
                .entities
                .iter_mut()
                .find(|entity| is_instance(entity))
                .unwrap();
            helper.properties.set("file", "middle.vmf");
            helper.properties.set("origin", "0 0 160");
            helper.properties.set("angles", "90 0 0");
            expected.properties.set("origin", "80 -12 124");
            expected.properties.set("angles", "0 90 90");
        } else {
            expected.properties.set("origin", "28 -4 64");
        }
        let before = parent.clone();
        let child_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let context = source(directory.path());
        let prepared = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .unwrap();
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| prepared.root_entity(entity.id).is_some())
            .unwrap();
        assert_eq!(prepared.stats().unwrap().fog_controllers, 1);
        assert_eq!(
            prepared.provenance[&format!("entity:{}", generated.id)].legacy,
            (owner, "child.vmf".into(), 20, None)
        );
        assert!(
            generated
                .origin()
                .unwrap()
                .abs_diff_eq(expected.origin().unwrap(), 1e-8)
        );
        assert_eq!(generated.properties.get("fogdir"), Some("1 0 0"));
        assert_eq!(
            map_core::fog::parse_static_ldr(generated).unwrap(),
            map_core::fog::parse_static_ldr(&original).unwrap()
        );
        let flat = literal(&parent, vec![expected]);
        let actual = output(&parent, &game, Some(&context));
        let control = output(&flat, &game, None);
        let rows = fog_rows(actual.bsp.as_ref().unwrap());
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].get("fogstart"),
            Some("runtime-owned malformed root value")
        );
        assert_eq!(rows[1].get("fogcolor"), Some("17 18 19"));
        assert_eq!(rows[1].get("spawnflags"), Some("1"));
        assert_eq!(rows[2].get("fogdir"), Some("1 0 0"));
        let actual = actual.bsp.unwrap();
        let control = control.bsp.unwrap();
        let actual_rows = fog_rows(&actual);
        let control_rows = fog_rows(&control);
        for (mut actual, mut expected) in actual_rows.into_iter().zip(control_rows) {
            if nested {
                for key in ["origin", "angles"] {
                    match (actual.get(key), expected.get(key)) {
                        (Some(actual), Some(expected)) => assert!(
                            parse_vec3(actual)
                                .unwrap()
                                .abs_diff_eq(parse_vec3(expected).unwrap(), 1e-8),
                            "{key}: {actual} != {expected}"
                        ),
                        (None, None) => {}
                        _ => panic!("Mismatched {key} presence"),
                    }
                    actual.remove(key);
                    expected.remove(key);
                }
            }
            assert_eq!(actual, expected);
        }
        for (index, (actual, control)) in actual.lumps.iter().zip(&control.lumps).enumerate() {
            if index != lump::ENTITIES {
                assert_eq!(actual, control, "lump {index}, nested {nested}");
            }
        }
        if !nested {
            assert_eq!(actual.to_bytes(), control.to_bytes());
        }
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            child_bytes
        );
    }
}

#[test]
fn static_child_fog_uses_transformed_region_markers_and_pre_region_counts() {
    let mut hidden = controller(22, "NaN 0 0");
    hidden.hidden = true;
    hidden.properties.set("use_angles", "1");
    let (directory, mut parent, game) = fog_fixture(vec![
        controller(20, "-512 0 64"),
        controller(21, "-384 0 96"),
        hidden,
    ]);
    let helper = parent.entities.last_mut().unwrap();
    helper.properties.set("origin", "512 0 0");
    let second = helper.id + 1;
    parent
        .entities
        .push(nested_helper(second, "child.vmf", "0 0 0", "0 0 0"));
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let flat = literal(
        &parent,
        vec![
            controller(20, "0 0 64"),
            controller(21, "128 0 96"),
            controller(22, "-512 0 64"),
            controller(23, "-384 0 96"),
        ],
    );
    let context = source(directory.path());
    let actual = output(&parent, &game, Some(&context));
    let control = output(&flat, &game, None);
    assert_eq!(actual.stats.instances.as_ref().unwrap().fog_controllers, 4);
    assert!(actual.stats.compile_region.is_some());
    let rows = fog_rows(actual.bsp.as_ref().unwrap());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].get("origin"), Some("128 0 96"));
    assert_eq!(
        actual.bsp.unwrap().to_bytes(),
        control.bsp.unwrap().to_bytes()
    );
}

#[test]
fn static_child_fog_rejects_unsupported_fields_poses_and_case_aliases_without_source_mutation() {
    let valid = controller(20, "0 0 64");
    fog::admit(&valid).unwrap();
    for (key, value) in [
        ("fogcolor", "1 2"),
        ("fogcolor", "256 0 0"),
        ("fogcolor2", "$Color"),
        ("fogstart", "$Start"),
        ("fogstart", "-1"),
        ("fogend", "100.125"),
        ("fogend", "inf"),
        ("fogmaxdensity", "1.1"),
        ("fogenable", "true"),
        ("fogblend", "1"),
        ("fogradial", "1"),
        ("farz", "1"),
        ("use_angles", "1"),
        ("use_angles", "0.0"),
        ("foglerptime", "2"),
        ("HDRColorScale", "2"),
        ("ZoomFogScale", "NaN"),
        ("fogdir", "1e50 0 0"),
        ("fogdir", "1 0"),
        ("origin", "16385 0 0"),
        ("origin", "NaN 0 0"),
        ("origin", "1e50 0 0"),
        ("angles", "0 inf 0"),
        ("angle", "1"),
        ("spawnflags", "1"),
        ("mindxlevel", "80"),
        ("targetname", "fog"),
        ("target", " "),
        ("parentname", "parent"),
        ("lightingorigin", "helper"),
        ("unknown", "0"),
    ] {
        let mut invalid = valid.clone();
        invalid.properties.set(key, value);
        let before = invalid.clone();
        assert!(fog::admit(&invalid).is_err(), "{key}={value}");
        assert_eq!(invalid, before);
    }
    for issue in [
        "duplicate",
        "alias",
        "classname",
        "origin",
        "origin_alias",
        "brush",
        "opaque",
        "connections",
    ] {
        let mut invalid = valid.clone();
        match issue {
            "duplicate" => invalid.properties.0.push(("fogend".into(), "4000".into())),
            "alias" => invalid.properties.0.push(("FOGEND".into(), "4000".into())),
            "classname" => {
                invalid.properties.remove("classname");
                invalid.properties.set("Classname", "env_fog_controller");
            }
            "origin" => {
                invalid.properties.remove("origin");
            }
            "origin_alias" => {
                invalid.properties.remove("origin");
                invalid.properties.set("Origin", "0 0 64");
            }
            "brush" => invalid
                .solids
                .push(test_common::room().world.solids.remove(0)),
            "opaque" => invalid.extra.push(Node::block("unknown", Vec::new())),
            "connections" => {
                invalid.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: "other,Kill,,0,-1".into(),
                }])
            }
            _ => unreachable!(),
        }
        assert!(fog::admit(&invalid).is_err(), "{issue}");
    }
    let (directory, parent, _) = fog_fixture(vec![controller(20, "16385 0 64")]);
    let context = source(directory.path());
    let before = parent.clone();
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("fog controller 20"), "{error:?}");
    assert_eq!(parent, before);
    write_child(
        directory.path(),
        "child.vmf",
        &helper_file(vec![controller(20, "1 0 64")]),
    );
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "16384 0 0", "0 0 0")]),
    );
    let mut out_and_back = parent.clone();
    let helper = out_and_back.entities.last_mut().unwrap();
    helper.properties.set("file", "middle.vmf");
    helper.properties.set("origin", "-16384 0 0");
    let error = prepare(
        &out_and_back,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("compile bounds"), "{error:?}");
}

#[test]
fn child_fog_remains_an_ordinary_flood_seed_with_leaf_provenance() {
    let (directory, mut parent, game) = fog_fixture(vec![controller(20, "0 0 64")]);
    parent.entities.retain(is_instance);
    parent.world.solids.remove(1);
    let owner = parent.entities[0].id;
    let context = source(directory.path());
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_none());
    let leak = output.leak.unwrap();
    assert_eq!(leak.entity, owner);
    assert_eq!(leak.points.first(), Some(&DVec3::new(24.0, -16.0, 80.0)));
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                    && diagnostic
                        .message
                        .contains("instance child.vmf, child object 20")
            ),
        "{:?}",
        output.report
    );
}
