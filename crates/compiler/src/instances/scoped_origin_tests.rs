use super::*;

fn anchor(id: u32, name: &str, point: &str) -> Entity {
    let mut entity = Entity::new(id, "info_lighting");
    entity.properties.set("targetname", name);
    entity.properties.set("origin", point);
    entity.properties.set("angles", "15 25 35");
    entity
}

fn scoped_prop(id: u32, model: &str, point: &str, name: &str) -> Entity {
    let mut entity = static_prop(id, model, point);
    entity.properties.set("lightingorigin", name);
    entity
}

fn scope(entity: &mut Entity, name: &str) {
    entity.properties.set("targetname", name);
    entity.properties.set("fixup_style", "0");
}

fn scoped_fixture(entities: Vec<Entity>) -> (tempfile::TempDir, Vmf, GameFs) {
    let (directory, mut parent, game) = prop_fixture(entities);
    scope(parent.entities.last_mut().unwrap(), "Outer");
    (directory, parent, game)
}

fn scoped_prepare<'a>(parent: &'a Vmf, context: &SourceContext) -> PreparedInstances<'a> {
    let mut report = Report::default();
    let prepared = prepare(parent, Some(context), &Control::default(), &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    prepared
}

fn scoped_failure(parent: &Vmf, context: &SourceContext) -> Diagnostic {
    let before = parent.clone();
    let error = prepare(
        parent,
        Some(context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert_eq!(parent, &before);
    error
}

fn region(parent: &mut Vmf) {
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
}

#[test]
fn scoped_points_match_literal_translation_and_noncommuting_native_output_in_all_modes() {
    for nested in [false, true] {
        let (directory, mut parent, game) = scoped_fixture(vec![
            scoped_prop(20, MODEL, "16 8 32", "Anchor"),
            anchor(22, "Anchor", "4 12 48"),
            scoped_prop(21, SECOND_MODEL, "-16 8 32", "Anchor"),
        ]);
        let root = parent.entities.last_mut().unwrap();
        let (name, point, first, second, angles) = if nested {
            let mut inner = nested_helper(100, "child.vmf", "48 -16 32", "0 90 0");
            scope(&mut inner, "Inner");
            write_child(directory.path(), "middle.vmf", &helper_file(vec![inner]));
            root.properties.set("file", "middle.vmf");
            root.properties.set("origin", "0 0 160");
            root.properties.set("angles", "90 0 0");
            (
                "Outer-Inner-Anchor",
                [80.0, -12.0, 124.0],
                "64 0 120",
                "64 -32 120",
                "0 90 90",
            )
        } else {
            root.properties.set("origin", "24 -16 16");
            (
                "Outer-Anchor",
                [28.0, -4.0, 64.0],
                "40 -8 48",
                "8 -8 48",
                "0 0 0",
            )
        };
        let mut light = Entity::new(
            parent.object_ids().chain(parent.side_ids()).max().unwrap() + 1,
            "light",
        );
        light.properties.set("origin", "0 0 192");
        light.properties.set("_light", "255 0 0 700");
        light.properties.set("_lightHDR", "0 0 255 700");
        parent.entities.insert(parent.entities.len() - 1, light);
        let mut props = [
            scoped_prop(2, MODEL, first, name),
            scoped_prop(3, SECOND_MODEL, second, name),
        ];
        for prop in &mut props {
            prop.properties.set("angles", angles);
        }
        let flat = literal_props(
            &parent,
            vec![
                props[0].clone(),
                anchor(4, name, &format!("{} {} {}", point[0], point[1], point[2])),
                props[1].clone(),
            ],
        );
        let context = source(directory.path());
        let prepared = scoped_prepare(&parent, &context);
        assert_eq!(prepared.stats().unwrap().lighting_origins, 1);
        assert_eq!(prepared.stats().unwrap().static_props, 2);
        let helper_id = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("info_lighting"))
            .unwrap()
            .id;
        let metadata = prepared.static_prop_metadata();
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| matches!(entity.classname(), Some("prop_static" | "info_lighting")))
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 3);
        for entity in generated {
            assert_eq!(
                prepared.root_entity(entity.id),
                Some(parent.entities.last().unwrap().id)
            );
            if entity.classname() == Some("prop_static") {
                assert_eq!(entity.properties.get("lightingorigin"), Some(name));
                assert!(
                    matches!(metadata.lighting_origins.unwrap().get(&entity.id), Some(crate::lighting_origins::Binding::Helper(id)) if *id == helper_id)
                );
            } else {
                assert_eq!(entity.properties.get("targetname"), Some(name));
                assert!(
                    entity
                        .origin()
                        .unwrap()
                        .abs_diff_eq(DVec3::from_array(point.map(f64::from)), 1e-8)
                );
            }
        }
        for mode in [
            crate::LightingMode::Fullbright,
            crate::LightingMode::DirectLdr,
            crate::LightingMode::DirectLdrHdr,
        ] {
            let actual = prop_output(&parent, &game, Some(&context), mode);
            let expected = prop_output(&flat, &game, None, mode);
            assert_literal_prop_bsp(actual.bsp.as_ref().unwrap(), expected.bsp.as_ref().unwrap());
            let bsp = actual.bsp.as_ref().unwrap();
            let records = bsp.static_props().unwrap().unwrap();
            assert_eq!(records.models.len(), 2);
            assert_eq!(records.model_name(0).unwrap(), MODEL);
            assert_eq!(records.model_name(1).unwrap(), SECOND_MODEL);
            assert_eq!(
                records
                    .props
                    .iter()
                    .map(|prop| prop.model)
                    .collect::<Vec<_>>(),
                [0, 1]
            );
            assert!(records.props.iter().all(|prop| prop.flags & 2 == 2
                && prop.lighting_origin == point
                && prop.leaf_count > 0));
            assert!(!bsp.entities().unwrap().contains("info_lighting"));
            if mode != crate::LightingMode::Fullbright {
                assert!(!bsp.world_lights(LightingKind::Ldr).unwrap().is_empty());
                assert!(
                    bsp.lighting(LightingKind::Ldr)
                        .unwrap()
                        .unwrap()
                        .samples()
                        .iter()
                        .any(|sample| sample.rgb[0] > 0 && sample.rgb[2] == 0)
                );
            }
            if mode == crate::LightingMode::DirectLdrHdr {
                assert!(!bsp.world_lights(LightingKind::Hdr).unwrap().is_empty());
                assert!(
                    bsp.lighting(LightingKind::Hdr)
                        .unwrap()
                        .unwrap()
                        .samples()
                        .iter()
                        .any(|sample| sample.rgb[2] > 0 && sample.rgb[0] == 0)
                );
                assert_ne!(
                    bsp.lump(lump::LIGHTING).data,
                    bsp.lump(lump::LIGHTING_HDR).data
                );
            }
        }
    }
}

#[test]
fn source_order_repeated_siblings_and_grandchildren_keep_distinct_source_prefixes() {
    for helper_first in [false, true] {
        let mut entities = vec![
            scoped_prop(20, MODEL, "16 8 32", "Anchor"),
            scoped_prop(21, MODEL, "-16 8 32", "Anchor"),
        ];
        entities.insert(
            if helper_first { 0 } else { 2 },
            anchor(22, "Anchor", "4 12 48"),
        );
        let (directory, mut parent, _) = scoped_fixture(entities);
        let mut left = nested_helper(100, "child.vmf", "0 0 0", "0 0 0");
        scope(&mut left, "Left");
        let mut right = nested_helper(101, "child.vmf", "64 0 0", "0 0 0");
        scope(&mut right, "Right");
        write_child(
            directory.path(),
            "middle.vmf",
            &helper_file(vec![left, right]),
        );
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("file", "middle.vmf");
        let mut second = parent.entities.last().unwrap().clone();
        second.id += 1;
        scope(&mut second, "Other");
        second.properties.set("origin", "0 64 0");
        parent.entities.push(second);
        let context = source(directory.path());
        let prepared = scoped_prepare(&parent, &context);
        assert_eq!(prepared.stats().unwrap().lighting_origins, 4);
        assert_eq!(prepared.stats().unwrap().static_props, 8);
        let helpers = prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| entity.classname() == Some("info_lighting"))
            .collect::<Vec<_>>();
        assert_eq!(
            helpers
                .iter()
                .map(|entity| entity.properties.get("targetname").unwrap())
                .collect::<Vec<_>>(),
            [
                "Outer-Left-Anchor",
                "Outer-Right-Anchor",
                "Other-Left-Anchor",
                "Other-Right-Anchor"
            ]
        );
        assert_eq!(
            helpers
                .iter()
                .map(|entity| entity.origin().unwrap())
                .collect::<Vec<_>>(),
            [
                DVec3::new(4.0, 12.0, 48.0),
                DVec3::new(68.0, 12.0, 48.0),
                DVec3::new(4.0, 76.0, 48.0),
                DVec3::new(68.0, 76.0, 48.0)
            ]
        );
        for helper in helpers {
            let props = prepared
                .vmf()
                .entities
                .iter()
                .filter(|entity| {
                    entity.classname() == Some("prop_static")
                        && entity.properties.get("lightingorigin")
                            == helper.properties.get("targetname")
                })
                .collect::<Vec<_>>();
            assert_eq!(props.len(), 2);
            for prop in props {
                assert!(
                    matches!(prepared.static_prop_metadata().lighting_origins.unwrap().get(&prop.id), Some(crate::lighting_origins::Binding::Helper(id)) if *id == helper.id)
                );
            }
        }
    }
}

#[test]
fn missing_or_hidden_local_helper_cannot_fall_back_to_a_coincident_root_name() {
    for hidden in [false, true] {
        let mut child = vec![scoped_prop(20, MODEL, "0 0 32", "Anchor")];
        if hidden {
            let mut helper = anchor(22, "Anchor", "0 0 64");
            helper.hidden = true;
            child.push(helper);
        }
        let (directory, mut parent, game) = scoped_fixture(child);
        let root = parent.entities.last().unwrap().id;
        parent
            .entities
            .push(anchor(root + 1, "Outer-Anchor", "0 0 64"));
        let context = source(directory.path());
        let prepared = scoped_prepare(&parent, &context);
        assert_eq!(prepared.stats().unwrap().lighting_origins, 0);
        let prop_id = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("prop_static"))
            .unwrap()
            .id;
        assert!(matches!(
            prepared
                .static_prop_metadata()
                .lighting_origins
                .unwrap()
                .get(&prop_id),
            Some(crate::lighting_origins::Binding::Missing)
        ));
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.object == Some(format!("entity:{root}"))
                        && diagnostic.message.contains("lightingorigin")
                        && diagnostic.message.contains("child object 20")
                ),
            "{:?}",
            output.report
        );
    }
}

#[test]
fn child_references_do_not_search_parent_or_sibling_occurrences() {
    for requested in ["Anchor", "Inner_Anchor"] {
        let (directory, mut parent, game) =
            scoped_fixture(vec![scoped_prop(20, MODEL, "0 0 32", requested)]);
        let mut nested = nested_helper(100, "child.vmf", "0 0 0", "0 0 0");
        scope(&mut nested, "Inner");
        write_child(
            directory.path(),
            "middle.vmf",
            &helper_file(vec![anchor(101, requested, "0 0 64"), nested]),
        );
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("file", "middle.vmf");
        let context = source(directory.path());
        scoped_prepare(&parent, &context);
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("lightingorigin")
                    && diagnostic.message.contains("child object 20")),
            "{:?}",
            output.report
        );
    }
    let (directory, mut parent, game) = scoped_fixture(vec![
        scoped_prop(20, MODEL, "0 0 32", "Anchor"),
        anchor(22, "Anchor", "0 0 64"),
    ]);
    write_child(
        directory.path(),
        "missing.vmf",
        &helper_file(vec![scoped_prop(20, MODEL, "0 0 32", "Anchor")]),
    );
    let mut sibling = parent.entities.last().unwrap().clone();
    sibling.id += 1;
    sibling.properties.set("file", "missing.vmf");
    scope(&mut sibling, "Other");
    let missing_root = sibling.id;
    parent.entities.push(sibling);
    parent
        .entities
        .push(anchor(missing_root + 1, "Other-Anchor", "0 0 64"));
    let context = source(directory.path());
    scoped_prepare(&parent, &context);
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{missing_root}"))
                    && diagnostic.message.contains("lightingorigin")
                    && diagnostic
                        .message
                        .contains("instance missing.vmf, child object 20")
            ),
        "{:?}",
        output.report
    );
}

#[test]
fn ordinary_consumer_requires_the_retained_local_helper_identity() {
    let (directory, parent, game) = scoped_fixture(vec![
        scoped_prop(20, MODEL, "0 0 32", "Anchor"),
        anchor(22, "Anchor", "0 0 64"),
    ]);
    let context = source(directory.path());
    let prepared = scoped_prepare(&parent, &context);
    let mut replaced = prepared.vmf().clone();
    let id = replaced
        .object_ids()
        .chain(replaced.side_ids())
        .max()
        .unwrap()
        + 1;
    replaced
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("info_lighting"))
        .unwrap()
        .id = id;
    let mut bsp = crate::compile(&test_common::room(), &game).bsp.unwrap();
    let mut report = Report::default();
    crate::static_props::bake(
        &replaced,
        &game,
        &mut bsp,
        &mut report,
        &Control::default(),
        None,
        &prepared.static_prop_metadata(),
        None,
    )
    .unwrap();
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("lightingorigin does not match its local instance helper")),
        "{report:?}"
    );
}

#[test]
fn region_omitted_missing_reference_does_not_read_its_model_or_physics() {
    let mut omitted = scoped_prop(21, SECOND_MODEL, "192 0 32", "Missing");
    omitted.properties.set("solid", "6");
    let mut hidden = scoped_prop(22, "models/props/absent.mdl", "NaN 0 0", "Missing");
    hidden.properties.set("solid", "6");
    hidden.hidden = true;
    let (directory, mut parent, game) =
        scoped_fixture(vec![static_prop(20, MODEL, "0 0 32"), omitted, hidden]);
    std::fs::write(
        directory.path().join(SECOND_MODEL),
        b"invalid omitted model",
    )
    .unwrap();
    let phy = directory.path().join("models/props/second.phy");
    std::fs::write(&phy, b"invalid omitted physics").unwrap();
    region(&mut parent);
    let context = source(directory.path());
    let output = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().static_props, 2);
    assert_eq!(output.stats.static_props, 1);
    std::fs::write(directory.path().join(SECOND_MODEL), b"changed unread model").unwrap();
    std::fs::write(phy, b"changed unread physics").unwrap();
    output
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    output
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
}

#[test]
fn scoped_origin_metadata_coexists_with_original_required_physics_and_default_six() {
    let mut prop = scoped_prop(20, MODEL, "0 0 32", "Anchor");
    prop.properties.set("solid", "6");
    let (directory, parent, game) =
        scoped_fixture(vec![prop.clone(), anchor(22, "Anchor", "0 0 64")]);
    let physics = collision_tests::write_physics(directory.path());
    let context = source(directory.path());
    let explicit = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    let records = explicit
        .bsp
        .as_ref()
        .unwrap()
        .static_props()
        .unwrap()
        .unwrap();
    assert_eq!(records.props[0].solid, 6);
    assert_eq!(records.props[0].flags & 2, 2);
    assert_eq!(records.props[0].lighting_origin, [0.0, 0.0, 64.0]);
    explicit
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    explicit
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("models/props/instance.phy")).unwrap(),
        physics
    );
    prop.properties.remove("solid");
    write_child(
        directory.path(),
        "child.vmf",
        &helper_file(vec![prop, anchor(22, "Anchor", "0 0 64")]),
    );
    let default = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(
        default.bsp.as_ref().unwrap().to_bytes(),
        explicit.bsp.as_ref().unwrap().to_bytes()
    );
    std::fs::remove_file(directory.path().join("models/props/instance.phy")).unwrap();
    let missing = compiled(&parent, &game, &context);
    assert!(missing.bsp.is_none() && missing.report.has_errors());
    let root = parent.entities.last().unwrap().id;
    assert!(
        missing
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{root}"))
                    && diagnostic
                        .message
                        .contains("needs its .phy collision asset for VPhysics")
                    && diagnostic
                        .message
                        .contains("instance child.vmf, child object 20")
            ),
        "{:?}",
        missing.report
    );
}

#[test]
fn outside_region_helpers_remain_available_and_root_props_use_final_prefixed_names() {
    let (directory, mut parent, game) = scoped_fixture(vec![
        scoped_prop(20, MODEL, "0 0 32", "Anchor"),
        anchor(22, "Anchor", "192 0 64"),
    ]);
    let root = parent.entities.last().unwrap().id;
    parent.entities.push(scoped_prop(
        root + 1,
        SECOND_MODEL,
        "32 0 32",
        "Outer-Anchor",
    ));
    region(&mut parent);
    let context = source(directory.path());
    let output = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().lighting_origins, 1);
    let props = output
        .bsp
        .as_ref()
        .unwrap()
        .static_props()
        .unwrap()
        .unwrap();
    assert_eq!(props.props.len(), 2);
    assert!(
        props
            .props
            .iter()
            .all(|prop| prop.flags & 2 == 2 && prop.lighting_origin == [192.0, 0.0, 64.0])
    );
}

#[test]
fn exact_and_case_ambiguous_local_helpers_and_generated_root_collisions_reject() {
    for second in ["Anchor", "anchor"] {
        let (directory, mut parent, _) = scoped_fixture(vec![
            anchor(22, "Anchor", "0 0 64"),
            anchor(23, second, "192 0 64"),
        ]);
        region(&mut parent);
        let context = source(directory.path());
        let error = scoped_failure(&parent, &context);
        assert!(
            error.message.contains("lighting") || error.message.contains("name"),
            "{error:?}"
        );
    }
    for name in ["Outer-Anchor", "outer-anchor"] {
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
        let id = parent.entities.last().unwrap().id;
        parent.entities.push(anchor(id + 1, name, "192 0 64"));
        let context = source(directory.path());
        let error = scoped_failure(&parent, &context);
        assert!(
            error.message.contains("collision") || error.message.contains("name"),
            "{error:?}"
        );
    }
    for second in ["Outer", "outer"] {
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
        let mut helper = parent.entities.last().unwrap().clone();
        helper.id += 1;
        scope(&mut helper, second);
        parent.entities.push(helper);
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
}

#[test]
fn name_bearing_branches_require_explicit_canonical_prefix_ancestors_only() {
    for (key, value) in [
        ("targetname", ""),
        ("targetname", "bad-name"),
        ("targetname", "@global"),
        ("targetname", "has space"),
        ("fixup_style", "1"),
        ("fixup_style", "2"),
        ("name", "fallback"),
        ("Name", "fallback"),
    ] {
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set(key, value);
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
    for key in ["targetname", "fixup_style"] {
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .0
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap()
            .0 = key.to_uppercase();
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
    let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .remove("fixup_style");
    let context = source(directory.path());
    scoped_prepare(&parent, &context);
    let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
    let mut inner = nested_helper(100, "child.vmf", "0 0 0", "0 0 0");
    scope(&mut inner, "Inner");
    write_child(directory.path(), "middle.vmf", &helper_file(vec![inner]));
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.remove("targetname");
    let context = source(directory.path());
    scoped_failure(&parent, &context);
    for (key, value) in [
        ("fixup_style", "1"),
        ("fixup_style", "2"),
        ("name", "fallback"),
    ] {
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
        let mut inner = nested_helper(100, "child.vmf", "0 0 0", "0 0 0");
        scope(&mut inner, "Inner");
        inner.properties.set(key, value);
        write_child(directory.path(), "middle.vmf", &helper_file(vec![inner]));
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("file", "middle.vmf");
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
    for style in ["0", "1", "2"] {
        let (directory, mut parent, game) = prop_fixture(vec![static_prop(20, MODEL, "0 0 32")]);
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("fixup_style", style);
        let context = source(directory.path());
        let flat = literal_props(&parent, vec![static_prop(2, MODEL, "0 0 32")]);
        let actual = prop_output(
            &parent,
            &game,
            Some(&context),
            crate::LightingMode::Fullbright,
        );
        let expected = prop_output(&flat, &game, None, crate::LightingMode::Fullbright);
        assert_eq!(
            actual.bsp.unwrap().to_bytes(),
            expected.bsp.unwrap().to_bytes()
        );
    }
}

#[test]
fn local_name_components_and_composed_length_have_explicit_boundaries() {
    for name in ["@Anchor", "!Anchor", "$Anchor", "A-B", "A B", "A*", "é", ""] {
        let (directory, parent, _) = scoped_fixture(vec![anchor(22, name, "0 0 64")]);
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
    for name in ["A_1", "012"] {
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, name, "0 0 64")]);
        scope(parent.entities.last_mut().unwrap(), "Root_1");
        let context = source(directory.path());
        let prepared = scoped_prepare(&parent, &context);
        let helper = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("info_lighting"))
            .unwrap();
        assert_eq!(
            helper.properties.get("targetname"),
            Some(format!("Root_1-{name}").as_str())
        );
    }
    for length in [125, 126] {
        let name = "A".repeat(length);
        let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, &name, "0 0 64")]);
        scope(parent.entities.last_mut().unwrap(), "X");
        let context = source(directory.path());
        if length == 125 {
            let prepared = scoped_prepare(&parent, &context);
            assert_eq!(
                prepared
                    .vmf()
                    .entities
                    .iter()
                    .find(|entity| entity.classname() == Some("info_lighting"))
                    .unwrap()
                    .properties
                    .get("targetname")
                    .unwrap()
                    .len(),
                127
            );
        } else {
            scoped_failure(&parent, &context);
        }
    }
    for name in ["@Anchor", "!Anchor", "Outer-Anchor", "A*", "é"] {
        let (directory, parent, _) = scoped_fixture(vec![scoped_prop(20, MODEL, "0 0 32", name)]);
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
}

#[test]
fn lighting_helpers_keep_a_canonical_point_only_source_envelope() {
    for (key, value) in [
        ("parentname", "parent"),
        ("target", "target"),
        ("name", "fallback"),
        ("globalname", "shared"),
        ("killtarget", "victim"),
        ("spawnflags", "1"),
        ("angles", "NaN 0 0"),
        ("angle", "inf"),
    ] {
        let mut helper = anchor(22, "Anchor", "0 0 64");
        helper.properties.set(key, value);
        let (directory, parent, _) = scoped_fixture(vec![helper]);
        let context = source(directory.path());
        let error = scoped_failure(&parent, &context);
        assert!(
            error.message.contains("child.vmf"),
            "{key}={value}: {error:?}"
        );
    }
    for key in ["classname", "origin", "targetname"] {
        let mut helper = anchor(22, "Anchor", "0 0 64");
        helper
            .properties
            .0
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap()
            .0 = key.to_uppercase();
        let (directory, parent, _) = scoped_fixture(vec![helper]);
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
    let mut duplicate = anchor(22, "Anchor", "0 0 64");
    duplicate
        .properties
        .0
        .push(("Targetname".into(), "Other".into()));
    let (directory, parent, _) = scoped_fixture(vec![duplicate]);
    let context = source(directory.path());
    scoped_failure(&parent, &context);
    let mut helper = anchor(22, "Anchor", "0 0 64");
    helper.extra.push(Node::block("opaque", Vec::new()));
    let (directory, parent, _) = scoped_fixture(vec![helper]);
    let context = source(directory.path());
    scoped_failure(&parent, &context);
}

#[test]
fn helper_source_and_every_intermediate_point_stay_within_source_bounds() {
    for point in ["NaN 0 0", "0 inf 0", "16385 0 64"] {
        let (directory, parent, _) = scoped_fixture(vec![anchor(22, "Anchor", point)]);
        let context = source(directory.path());
        scoped_failure(&parent, &context);
    }
    let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "16000 0 64")]);
    let mut inner = nested_helper(100, "child.vmf", "1000 0 0", "0 0 0");
    scope(&mut inner, "Inner");
    write_child(directory.path(), "middle.vmf", &helper_file(vec![inner]));
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "-1000 0 0");
    let context = source(directory.path());
    let error = scoped_failure(&parent, &context);
    assert!(
        error.message.contains("bound") && error.message.contains("child.vmf"),
        "{error:?}"
    );
    assert_eq!(
        error.object,
        Some(format!("entity:{}", parent.entities.last().unwrap().id))
    );
    assert!(error.message.contains("22"), "{error:?}");
}

#[test]
fn scoped_helper_expansion_shares_combined_object_budget_and_cancellation() {
    let (directory, mut parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
    let id = parent.object_ids().max().unwrap() + 1;
    let remaining = MAX_OBJECTS - parent.object_ids().count();
    for offset in 0..remaining {
        parent
            .entities
            .push(Entity::new(id + offset as u32, "info_target"));
    }
    let context = source(directory.path());
    let error = scoped_failure(&parent, &context);
    assert!(error.message.contains("combined objects"), "{error:?}");
    let (directory, parent, _) = scoped_fixture(vec![anchor(22, "Anchor", "0 0 64")]);
    let before = parent.clone();
    let context = source(directory.path());
    let control = Control::default();
    control.cancel().unwrap();
    let error = prepare(&parent, Some(&context), &control, &mut Report::default())
        .err()
        .unwrap();
    assert!(error.message.contains("cancelled"), "{error:?}");
    assert_eq!(parent, before);
    assert!(
        serde_json::to_value(InstanceStats::default())
            .unwrap()
            .get("lighting_origins")
            .is_none()
    );
}

#[test]
fn scoped_name_bytes_share_the_work_limit_and_checked_overflow() {
    let mut helper = nested_helper(100, "child.vmf", "0 0 0", "0 0 0");
    scope(&mut helper, "Outer");
    let control = Control::default();
    let mut work = 0;
    let captured = names::Scope::capture(&helper, &mut work, &control).unwrap();
    assert_eq!(
        names::compose(&[captured], "Anchor", &mut work, &control).unwrap(),
        "Outer-Anchor"
    );
    assert!(work >= "Outer".len() + "Outer-Anchor".len());
    let captured = names::Scope::capture(&helper, &mut work, &control).unwrap();
    work = MAX_GEOMETRY_WORK - 1;
    assert!(
        names::compose(&[captured], "Anchor", &mut work, &control)
            .unwrap_err()
            .contains("work")
    );
    let mut work = usize::MAX;
    assert!(names::Scope::capture(&helper, &mut work, &control).is_err());
}

#[test]
fn cancellation_after_a_real_scoped_branch_rolls_back_names_and_points() {
    let (directory, mut parent, _) = scoped_fixture(vec![
        scoped_prop(20, MODEL, "0 0 32", "Anchor"),
        anchor(22, "Anchor", "0 0 64"),
    ]);
    let first = parent.entities.last().unwrap().clone();
    let mut second = first.clone();
    second.id += 1;
    scope(&mut second, "Other");
    parent.entities.push(second.clone());
    let before = parent.clone();
    let context = source(directory.path());
    let control = Control::default();
    let mut report = Report::default();
    let mut expansion = expand::Expansion::new(&parent, &context, &control, &mut report);
    let mut document = Document::new(parent);
    let error = document
        .edit("cancel scoped lighting origin", |tx| {
            expansion.helper(
                tx,
                &first,
                &first,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )?;
            let helper = tx
                .vmf()
                .entities
                .iter()
                .find(|entity| entity.classname() == Some("info_lighting"))
                .unwrap();
            assert_eq!(helper.properties.get("targetname"), Some("Outer-Anchor"));
            assert_eq!(helper.origin(), Some(DVec3::new(0.0, 0.0, 64.0)));
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

#[test]
fn unnamed_instances_preserve_unused_duplicate_and_originless_root_helpers() {
    let (directory, mut parent, game) = prop_fixture(vec![static_prop(20, MODEL, "0 0 32")]);
    let flat = literal_props(&parent, vec![static_prop(2, MODEL, "0 0 32")]);
    let original = prop_output(&flat, &game, None, crate::LightingMode::Fullbright);
    let id = parent.object_ids().chain(parent.side_ids()).max().unwrap() + 1;
    let mut originless = anchor(id + 1, "unused", "0 0 64");
    originless.properties.remove("origin");
    let mut hidden = anchor(id + 2, "unused", "NaN 0 0");
    hidden.hidden = true;
    parent
        .entities
        .extend([anchor(id, "unused", "0 0 64"), originless, hidden]);
    let context = source(directory.path());
    let actual = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(actual.stats.instances.as_ref().unwrap().lighting_origins, 0);
    assert_eq!(
        actual.bsp.unwrap().to_bytes(),
        original.bsp.unwrap().to_bytes()
    );
}

#[test]
fn scoped_child_or_model_drift_refuses_publication_and_preserves_real_prior_outputs() {
    for model_drift in [false, true] {
        let (directory, parent, game) = scoped_fixture(vec![
            scoped_prop(20, MODEL, "0 0 32", "Anchor"),
            anchor(22, "Anchor", "0 0 64"),
        ]);
        let outputs = tempfile::tempdir().unwrap();
        let destination = outputs.path().join("previous.bsp");
        let context = source(directory.path());
        let prior = prop_output(
            &parent,
            &game,
            Some(&context),
            crate::LightingMode::Fullbright,
        );
        assert!(prior.write(&destination).unwrap());
        let prior_bytes = std::fs::read(&destination).unwrap();
        std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
        let before = parent.clone();
        let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
        let output = prop_output(
            &parent,
            &game,
            Some(&context),
            crate::LightingMode::Fullbright,
        );
        let children = output.instance_dependencies.as_ref().unwrap();
        let models = output.static_prop_sources.as_ref().unwrap();
        children.verify(&Control::default()).unwrap();
        models.verify_sources(&game, &Control::default()).unwrap();
        let lease = crate::OutputLease::acquire(&destination, &Control::default()).unwrap();
        let staged = output
            .prepare_write_controlled(&destination, &Control::default())
            .unwrap()
            .unwrap();
        if model_drift {
            let path = directory.path().join(MODEL);
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[20] ^= 1;
            std::fs::write(path, bytes).unwrap();
            assert!(models.verify_sources(&game, &Control::default()).is_err());
            children.verify(&Control::default()).unwrap();
        } else {
            write_child(
                directory.path(),
                "child.vmf",
                &helper_file(vec![
                    scoped_prop(20, MODEL, "0 0 32", "Anchor"),
                    anchor(22, "Anchor", "0 0 80"),
                ]),
            );
            assert!(children.verify(&Control::default()).is_err());
            models.verify_sources(&game, &Control::default()).unwrap();
        }
        assert!(staged.commit_checked(&lease, &Control::default()).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), prior_bytes);
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("parent.vmf")).unwrap(),
            parent_bytes
        );
    }
}
