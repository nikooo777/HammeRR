use source_formats::bsp::{Bsp, LightingKind, lump};

use super::*;

use crate::test_static_prop_model;

#[path = "props_collision_tests.rs"]
mod collision_tests;

#[path = "scoped_origin_tests.rs"]
mod scoped_origin_tests;

#[path = "props_skin_tests.rs"]
mod skin_tests;

#[path = "props_lighting_tests.rs"]
mod lighting_tests;

#[path = "props_visibility_tests.rs"]
mod visibility_tests;

const MODEL: &str = "models/props/instance.mdl";
const SECOND_MODEL: &str = "models/props/second.mdl";

fn static_prop(id: u32, model: &str, origin: &str) -> Entity {
    let mut entity = Entity::new(id, "prop_static");
    for (key, value) in [
        ("origin", origin),
        ("angles", "0 0 0"),
        ("model", model),
        ("solid", "0"),
        ("body", "0"),
        ("skin", "0"),
        ("modelscale", "1"),
        ("rendercolor", "255 255 255"),
        ("renderamt", "255"),
        ("rendermode", "0"),
        ("renderfx", "0"),
        ("disableshadows", "0"),
        ("screenspacefade", "0"),
        ("fademindist", "-1"),
        ("fademaxdist", "0"),
        ("fadescale", "1"),
        ("lightingorigin", ""),
        ("disablevertexlighting", "0"),
        ("disableselfshadowing", "0"),
        ("ignorenormals", "0"),
        ("mindxlevel", "0"),
        ("maxdxlevel", "0"),
    ] {
        entity.properties.set(key, value);
    }
    entity
}

fn with_collision(mut prop: Entity, solid: Option<&str>) -> Entity {
    match solid {
        Some(value) => prop.properties.set("solid", value),
        None => {
            prop.properties.remove("solid");
        }
    }
    prop
}

fn write_model(directory: &Path, path: &str, mins: [f32; 3], maxs: [f32; 3]) {
    let [mut bytes, vertices, indices] = test_static_prop_model::model();
    for (offset, values) in [(104, mins), (116, maxs), (128, mins), (140, maxs)] {
        for (axis, value) in values.into_iter().enumerate() {
            bytes[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    let path = directory.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for (extension, data) in ["mdl", "vvd", "dx90.vtx"]
        .into_iter()
        .zip([bytes, vertices, indices])
    {
        std::fs::write(path.with_extension(extension), data).unwrap();
    }
}

fn prop_fixture(props: Vec<Entity>) -> (tempfile::TempDir, Vmf, GameFs) {
    let (directory, mut parent, _, game) = fixture();
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "0 0 0");
    write_child(directory.path(), "child.vmf", &helper_file(props));
    for path in [MODEL, SECOND_MODEL] {
        write_model(
            directory.path(),
            path,
            [-16.0, -8.0, 0.0],
            [16.0, 8.0, 32.0],
        );
    }
    (directory, parent, game)
}

fn literal_props(parent: &Vmf, props: Vec<Entity>) -> Vmf {
    let mut flat = parent.clone();
    flat.entities.retain(|entity| {
        !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    });
    let first_id = flat.object_ids().chain(flat.side_ids()).max().unwrap() + 1;
    for (id, mut prop) in (first_id..).zip(props) {
        prop.id = id;
        flat.entities.push(prop);
    }
    flat
}

fn prop_output(
    parent: &Vmf,
    game: &GameFs,
    source: Option<&SourceContext>,
    lighting: crate::LightingMode,
) -> crate::Output {
    let output = compile_with_source_controlled(
        parent,
        game,
        Options {
            lighting,
            ..Default::default()
        },
        None,
        source,
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

#[test]
fn neutral_fgd_props_resolve_without_model_reads_and_keep_bounded_pose_provenance() {
    let (directory, parent, _) = prop_fixture(vec![static_prop(20, MODEL, "16 8 32")]);
    std::fs::write(directory.path().join(MODEL), b"not an MDL").unwrap();
    let context = source(directory.path());
    let mut report = Report::default();
    let prepared = prepare(&parent, Some(&context), &Control::default(), &mut report).unwrap();
    assert!(!report.has_errors());
    assert_eq!(prepared.stats().unwrap().static_props, 1);
    assert_eq!(prepared.generated_brushes().count(), 0);
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("prop_static"))
        .unwrap();
    assert_eq!(
        prepared.root_entity(generated.id),
        Some(parent.entities.last().unwrap().id)
    );
    assert_eq!(prepared.static_prop_poses[&generated.id].len(), 2);
    assert!(generated.editor.is_none());
    assert_eq!(
        std::fs::read(directory.path().join(MODEL)).unwrap(),
        b"not an MDL"
    );
    assert!(
        serde_json::to_value(InstanceStats::default())
            .unwrap()
            .get("static_props")
            .is_none()
    );
}

#[test]
fn translated_props_match_independent_literal_sprp_and_fullbright_ldr_hdr_bytes() {
    let (directory, mut parent, game) = prop_fixture(vec![
        static_prop(20, MODEL, "16 8 32"),
        static_prop(21, SECOND_MODEL, "-48 16 64"),
    ]);
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "24 -16 16");
    let id = parent.object_ids().chain(parent.side_ids()).max().unwrap() + 1;
    let mut light = Entity::new(id, "light");
    light.properties.set("origin", "0 0 192");
    light.properties.set("_light", "255 160 80 700");
    parent.entities.push(light);
    let context = source(directory.path());
    let flat = literal_props(
        &parent,
        vec![
            static_prop(2, MODEL, "40 -8 48"),
            static_prop(3, SECOND_MODEL, "-24 0 80"),
        ],
    );
    for mode in [
        crate::LightingMode::Fullbright,
        crate::LightingMode::DirectLdr,
        crate::LightingMode::DirectLdrHdr,
    ] {
        let actual = prop_output(&parent, &game, Some(&context), mode);
        let expected = prop_output(&flat, &game, None, mode);
        assert_eq!(actual.stats.instances.as_ref().unwrap().static_props, 2);
        if mode != crate::LightingMode::Fullbright {
            assert_eq!(
                (
                    actual.stats.prop_shadow_props,
                    actual.stats.prop_shadow_models
                ),
                (2, 2)
            );
            assert!(actual.stats.prop_shadow_triangles > 0);
            assert_eq!(actual.stats.prop_shadow_disabled, 0);
        }
        actual
            .static_prop_sources
            .as_ref()
            .unwrap()
            .verify_sources(&game, &Control::default())
            .unwrap();
        let bsp = actual.bsp.unwrap();
        assert_eq!(bsp.to_bytes(), expected.bsp.unwrap().to_bytes(), "{mode:?}");
        let props = bsp.static_props().unwrap().unwrap();
        assert_eq!(props.version, 6);
        assert_eq!((props.models.len(), props.props.len()), (2, 2));
        assert_eq!(props.model_name(0).unwrap(), MODEL);
        assert_eq!(props.model_name(1).unwrap(), SECOND_MODEL);
        assert_eq!(props.props[0].origin, [40.0, -8.0, 48.0]);
        assert_eq!(props.props[1].origin, [-24.0, 0.0, 80.0]);
        if mode != crate::LightingMode::Fullbright {
            assert!(!bsp.world_lights(LightingKind::Ldr).unwrap().is_empty());
            assert!(
                bsp.lighting(LightingKind::Ldr)
                    .unwrap()
                    .unwrap()
                    .samples()
                    .iter()
                    .any(|sample| sample.rgb != [0; 3])
            );
        }
        if mode == crate::LightingMode::DirectLdrHdr {
            assert!(!bsp.lump(lump::LIGHTING_HDR).data.is_empty());
        }
    }
}

#[test]
fn noncommuting_nested_prop_matches_literal_basis_pose_and_native_leaf_lists() {
    let (directory, mut parent, game) = prop_fixture(vec![static_prop(20, MODEL, "16 8 32")]);
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "0 0 160");
    root.properties.set("angles", "90 0 0");
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "48 -16 32", "0 90 0")]),
    );
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let (&id, poses) = prepared.static_prop_poses.first_key_value().unwrap();
    assert_eq!(poses.len(), 3);
    for (pose, expected) in poses.iter().zip([
        DVec3::new(16.0, 8.0, 32.0),
        DVec3::new(40.0, 0.0, 64.0),
        DVec3::new(64.0, 0.0, 120.0),
    ]) {
        assert!(pose.origin.abs_diff_eq(expected, 1e-8));
    }
    let prop = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap();
    let angles = crate::static_props::angles(prop)
        .unwrap()
        .map(f64::to_radians);
    let rotation = glam::DQuat::from_euler(glam::EulerRot::ZYX, angles.y, angles.x, angles.z);
    for (axis, expected) in [
        (DVec3::X, DVec3::Y),
        (DVec3::Y, DVec3::Z),
        (DVec3::Z, DVec3::X),
    ] {
        assert!((rotation * axis).abs_diff_eq(expected, 1e-8));
    }
    let mut literal = static_prop(2, MODEL, "64 0 120");
    literal.properties.set("angles", "0 90 90");
    let flat = literal_props(&parent, vec![literal]);
    let actual = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::DirectLdrHdr,
    )
    .bsp
    .unwrap();
    let expected = prop_output(&flat, &game, None, crate::LightingMode::DirectLdrHdr)
        .bsp
        .unwrap();
    assert_literal_prop_bsp(&actual, &expected);
}

fn assert_literal_prop_bsp(actual: &Bsp, expected: &Bsp) {
    for index in 0..64 {
        if index != lump::GAME_LUMP {
            assert_eq!(
                actual.lump(index).data,
                expected.lump(index).data,
                "lump {index}"
            );
        }
    }
    let mut actual = actual.static_props().unwrap().unwrap();
    let expected = expected.static_props().unwrap().unwrap();
    assert_eq!(actual.props.len(), expected.props.len());
    for (actual, expected) in actual.props.iter_mut().zip(&expected.props) {
        for (a, b) in actual.origin.iter().zip(expected.origin) {
            assert!((*a - b).abs() <= 1e-6);
        }
        for (a, b) in actual.angles.iter().zip(expected.angles) {
            assert!((*a - b).abs() <= 1e-6);
        }
        actual.origin = expected.origin;
        actual.angles = expected.angles;
    }
    assert_eq!(actual, expected);
}

#[test]
fn source_and_intermediate_rotated_model_boxes_are_checked_from_the_consumed_header() {
    assert_model_boxes_checked(Some("0"));
}

fn assert_model_boxes_checked(solid: Option<&str>) {
    for intermediate in [false, true] {
        let local = if intermediate { "0 0 32" } else { "16380 0 32" };
        let (directory, mut parent, game) =
            prop_fixture(vec![with_collision(static_prop(20, MODEL, local), solid)]);
        if intermediate {
            write_model(
                directory.path(),
                MODEL,
                [-64.0, -4.0, 0.0],
                [64.0, 4.0, 32.0],
            );
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "0 16380 0", "0 90 0")]),
            );
            let root = parent.entities.last_mut().unwrap();
            root.properties.set("file", "middle.vmf");
            root.properties.set("origin", "0 -16380 0");
        } else {
            parent
                .entities
                .last_mut()
                .unwrap()
                .properties
                .set("origin", "-16380 0 0");
        }
        let root_id = parent.entities.last().unwrap().id;
        let root_prop = static_prop(root_id + 1, MODEL, "0 0 64");
        parent.entities.insert(0, root_prop);
        let context = source(directory.path());
        prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .unwrap();
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none() && output.report.has_errors());
        let stage = if intermediate {
            "intermediate pose 1"
        } else {
            "source pose"
        };
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.object == Some(format!("entity:{root_id}"))
                    && diagnostic.message.contains(stage)
                    && diagnostic.message.contains("transformed model bounds")
                    && diagnostic
                        .message
                        .contains("instance child.vmf, child object 20")
            }),
            "{:?}",
            output.report
        );
    }
}

#[test]
fn every_source_and_intermediate_origin_is_bounded_before_cloning_or_model_reads() {
    for intermediate in [false, true] {
        let local = if intermediate {
            "16374 0 32"
        } else {
            "16385 0 32"
        };
        let (directory, mut parent, _) = prop_fixture(vec![static_prop(20, MODEL, local)]);
        if intermediate {
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "20 0 0", "0 0 0")]),
            );
            let root = parent.entities.last_mut().unwrap();
            root.properties.set("file", "middle.vmf");
            root.properties.set("origin", "-16374 0 0");
        }
        std::fs::write(directory.path().join(MODEL), b"unread").unwrap();
        let before = parent.clone();
        let context = source(directory.path());
        let error = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .err()
        .unwrap();
        assert!(
            error
                .message
                .contains("origin exceeds finite Source compile bounds"),
            "{error:?}"
        );
        assert!(error.message.contains("Child prop 20"));
        assert_eq!(parent, before);
    }
}

#[test]
fn strict_static_prop_envelope_rejects_unsupported_semantics_and_ambiguous_pose_keys() {
    for (key, value) in [
        ("solid", "1"),
        ("solid", "5"),
        ("solid", "-1"),
        ("solid", "6.0"),
        ("solid", ""),
        ("solid", "0.0"),
        ("skin", "-1"),
        ("body", "1"),
        ("ignorenormals", "2"),
        ("disableshadows", "2"),
        ("disablevertexlighting", "2"),
        ("disableselfshadowing", "2"),
        ("modelscale", "2"),
        ("rendercolor", "255 128 255"),
        ("rendermode", "2"),
        ("renderamt", "128"),
        ("renderamt", "255.0"),
        ("renderfx", "1"),
        ("targetname", "named"),
        ("target", " "),
        ("parentname", "parent"),
        ("lightingorigin", "@external"),
        ("name", "named"),
        ("globalname", "shared"),
        ("fademaxdist", "-1"),
        ("mindxlevel", "65536"),
        ("generatelightmaps", "1"),
        ("lightmapresolution", "32"),
        ("pitch", "0"),
        ("replace01", ""),
        ("origin", "NaN 0 32"),
        ("angles", "0 inf 0"),
        ("angle", "90"),
    ] {
        let mut prop = static_prop(20, MODEL, "0 0 32");
        prop.properties.set(key, value);
        assert!(props::admit(&prop).is_err(), "{key}={value}");
    }
    for key in ["origin", "angles", "model", "classname", "solid"] {
        let mut prop = static_prop(20, MODEL, "0 0 32");
        prop.properties
            .0
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap()
            .0 = key.to_uppercase();
        assert!(props::admit(&prop).is_err(), "{key}");
    }
    let mut prop = static_prop(20, MODEL, "0 0 32");
    prop.properties.0.push(("Origin".into(), "0 0 32".into()));
    assert!(
        props::admit(&prop)
            .unwrap_err()
            .contains("repeats property")
    );
    for key in ["origin", "model"] {
        let mut prop = static_prop(20, MODEL, "0 0 32");
        prop.properties.remove(key);
        assert!(props::admit(&prop).is_err(), "{key}");
    }
    let mut prop = static_prop(20, MODEL, "0 0 32");
    prop.extra.push(Node::block("opaque", Vec::new()));
    assert!(props::admit(&prop).is_err());
    prop.extra.clear();
    prop.solids.push(test_common::room().world.solids.remove(0));
    assert!(props::admit(&prop).is_err());
    prop.solids.clear();
    prop.connections = Some(vec![source_formats::vmf::Connection {
        output: "OnUser1".into(),
        value: "target,TurnOn,,0,-1".into(),
    }]);
    assert!(props::admit(&prop).is_err());
}

#[test]
fn hidden_and_region_omitted_props_do_not_read_invalid_models_or_check_their_boxes() {
    let mut hidden = static_prop(22, "models/props/missing.mdl", "NaN 0 32");
    hidden.hidden = true;
    hidden.properties.set("solid", "6");
    let (directory, mut parent, game) = prop_fixture(vec![
        static_prop(20, MODEL, "0 0 32"),
        static_prop(21, SECOND_MODEL, "1024 0 32"),
        hidden,
    ]);
    std::fs::write(directory.path().join(SECOND_MODEL), b"invalid, omitted MDL").unwrap();
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let context = source(directory.path());
    let output = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().static_props, 2);
    assert_eq!(output.stats.static_props, 1);
    assert_eq!(
        output
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap()
            .models
            .len(),
        1
    );
    std::fs::write(directory.path().join(SECOND_MODEL), b"changed unread MDL").unwrap();
    output
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
}

#[test]
fn original_model_and_child_source_drift_preserve_previous_bsp_lin_and_authored_state() {
    for model_drift in [false, true] {
        let (directory, parent, game) = prop_fixture(vec![static_prop(20, MODEL, "0 0 32")]);
        let before = parent.clone();
        let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
        let outputs = tempfile::tempdir().unwrap();
        let path = outputs.path().join("previous.bsp");
        std::fs::write(&path, b"previous BSP").unwrap();
        std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
        let context = source(directory.path());
        let output = prop_output(
            &parent,
            &game,
            Some(&context),
            crate::LightingMode::Fullbright,
        );
        let models = output.static_prop_sources.as_ref().unwrap();
        let children = output.instance_dependencies.as_ref().unwrap();
        models.verify_sources(&game, &Control::default()).unwrap();
        children.verify(&Control::default()).unwrap();
        if model_drift {
            let model = directory.path().join(MODEL);
            let mut bytes = std::fs::read(&model).unwrap();
            bytes[20] ^= 1;
            std::fs::write(model, bytes).unwrap();
            assert!(models.verify_sources(&game, &Control::default()).is_err());
            children.verify(&Control::default()).unwrap();
        } else {
            write_child(directory.path(), "child.vmf", &helper_file(Vec::new()));
            assert!(children.verify(&Control::default()).is_err());
            models.verify_sources(&game, &Control::default()).unwrap();
        }
        assert!(output.write(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(path.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("parent.vmf")).unwrap(),
            parent_bytes
        );
    }
}

#[test]
fn missing_model_diagnostic_selects_the_root_helper_and_retains_leaf_entity_identity() {
    let (directory, parent, game) =
        prop_fixture(vec![static_prop(20, "models/props/missing.mdl", "0 0 32")]);
    let context = source(directory.path());
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{}", parent.entities.last().unwrap().id))
                && diagnostic
                    .message
                    .contains("models/props/missing.mdl was not found")
                && diagnostic
                    .message
                    .contains("instance child.vmf, child object 20")
        }),
        "{:?}",
        output.report
    );
}

#[test]
fn four_levels_repeated_props_keep_native_ordinals_and_optional_model_read_linkage() {
    let (directory, mut parent, game) = prop_fixture(vec![
        static_prop(20, MODEL, "16 8 32"),
        static_prop(21, SECOND_MODEL, "-16 8 32"),
    ]);
    let mut file = "child.vmf".to_owned();
    for level in 1..4 {
        let name = format!("level{level}.vmf");
        write_child(
            directory.path(),
            &name,
            &helper_file(vec![nested_helper(100, &file, "0 0 0", "0 0 0")]),
        );
        file = name;
    }
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", &file);
    let mut second = parent.entities.last().unwrap().clone();
    second.id += 1;
    second.properties.set("origin", "64 0 0");
    parent.entities.push(second);
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    assert_eq!(
        (
            prepared.stats().unwrap().static_props,
            prepared.stats().unwrap().max_expansion_depth
        ),
        (4, 4)
    );
    assert!(
        prepared
            .static_prop_poses
            .values()
            .all(|poses| poses.len() == 5)
    );
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| entity.classname() == Some("prop_static"))
        .collect::<Vec<_>>();
    let mut bsp: Bsp = crate::compile(&test_common::room(), &game).bsp.unwrap();
    let mut reads = crate::lighting::asset_tracking::SourceReads::default();
    let mut report = Report::default();
    let stats = crate::static_props::bake(
        prepared.vmf(),
        &game,
        &mut bsp,
        &mut report,
        &Control::default(),
        Some(&mut reads),
        &prepared.static_prop_metadata(),
    )
    .unwrap();
    assert!(!report.has_errors(), "{report:?}");
    assert_eq!(
        stats.sources,
        generated
            .iter()
            .map(|entity| entity.id as usize)
            .collect::<Vec<_>>()
    );
    let props = bsp.static_props().unwrap().unwrap();
    assert_eq!(
        props
            .props
            .iter()
            .map(|prop| prop.model)
            .collect::<Vec<_>>(),
        [0, 1, 0, 1]
    );
    assert_eq!(
        props
            .props
            .iter()
            .map(|prop| prop.origin)
            .collect::<Vec<_>>(),
        [
            [16.0, 8.0, 32.0],
            [-16.0, 8.0, 32.0],
            [80.0, 8.0, 32.0],
            [48.0, 8.0, 32.0]
        ]
    );
    let (sources, missing) = reads.into_parts();
    assert_eq!(sources.len(), 2);
    assert!(missing.is_empty());
    stats
        .source_guard
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
}

#[test]
fn repeated_prop_properties_share_the_existing_object_and_work_budgets() {
    let (directory, mut parent, _) = prop_fixture(vec![static_prop(20, MODEL, "0 0 32")]);
    let id = parent.object_ids().max().unwrap() + 1;
    for offset in 0..MAX_OBJECTS - parent.object_ids().count() {
        parent
            .entities
            .push(Entity::new(id + offset as u32, "info_target"));
    }
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("combined objects"), "{error:?}");

    let (directory, mut parent, _) = prop_fixture(Vec::new());
    let mut prop = static_prop(20, MODEL, "0 0 32");
    for (key, value) in &mut prop.properties.0 {
        if value == "0" {
            *value = "0".repeat(MAX_FIELD_BYTES);
        } else if matches!(key.as_str(), "modelscale" | "fadescale") {
            *value = format!("1.{}", "0".repeat(MAX_FIELD_BYTES - 2));
        }
    }
    props::admit(&prop).unwrap();
    let props = (0..128)
        .map(|index| {
            let mut prop = prop.clone();
            prop.id += index;
            prop
        })
        .collect();
    write_child(directory.path(), "child.vmf", &helper_file(props));
    let mut file = "child.vmf".to_owned();
    for level in 1..4 {
        let name = format!("level{level}.vmf");
        write_child(
            directory.path(),
            &name,
            &helper_file(vec![nested_helper(100, &file, "0 0 0", "0 0 0")]),
        );
        file = name;
    }
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", &file);
    let first = parent.entities.last().unwrap().clone();
    for index in 1..32 {
        let mut helper = first.clone();
        helper.id += index;
        parent.entities.push(helper);
    }
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("geometry-work"), "{error:?}");
}

#[test]
fn cancelling_after_a_generated_prop_rolls_back_the_detached_document() {
    let (directory, mut parent, _) = prop_fixture(vec![static_prop(20, MODEL, "16 8 32")]);
    let first = parent.entities.last().unwrap().clone();
    let mut second = first.clone();
    second.id += 1;
    parent.entities.push(second.clone());
    let before = parent.clone();
    let context = source(directory.path());
    let control = Control::default();
    let mut report = Report::default();
    let mut expansion = expand::Expansion::new(&parent, &context, &control, &mut report);
    let mut document = Document::new(parent);
    let error = document
        .edit("cancel child prop", |tx| {
            expansion.helper(
                tx,
                &first,
                &first,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )?;
            assert_eq!(
                tx.vmf()
                    .entities
                    .iter()
                    .filter(|entity| entity.classname() == Some("prop_static"))
                    .count(),
                1
            );
            control.cancel().unwrap();
            expansion.helper(
                tx,
                &second,
                &second,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )
        })
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(document.vmf(), &before);
}
