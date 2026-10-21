use super::*;

const KEYS: [&str; 6] = [
    "fademindist",
    "fademaxdist",
    "fadescale",
    "screenspacefade",
    "mindxlevel",
    "maxdxlevel",
];

struct Case {
    values: [&'static str; 6],
    fade: [f32; 3],
    flags: u32,
    dx: [u16; 2],
}

const CASES: [Case; 5] = [
    Case {
        values: ["128", "512", "0.5", "0", "80", "95"],
        fade: [128.0, 512.0, 0.5],
        flags: 1,
        dx: [80, 95],
    },
    Case {
        values: ["256", "64", "2", "1", "90", "0"],
        fade: [256.0, 64.0, 2.0],
        flags: 1 | 32,
        dx: [90, 0],
    },
    Case {
        values: ["-7", "300", "1", "0", "0", "65535"],
        fade: [300.0, 300.0, 1.0],
        flags: 1,
        dx: [0, 65535],
    },
    Case {
        values: ["192", "0", "0", "1", "65535", "0"],
        fade: [0.0, 0.0, 0.0],
        flags: 32,
        dx: [65535, 0],
    },
    Case {
        values: [
            "0.1000000000001",
            "100.0000001",
            "0.1000000000001",
            "0",
            "81",
            "90",
        ],
        fade: [0.1, 100.0, 0.1],
        flags: 1,
        dx: [81, 90],
    },
];

fn visibility_prop(id: u32, model: &str, origin: &str, values: [&str; 6]) -> Entity {
    let mut prop = static_prop(id, model, origin);
    for (key, value) in KEYS.into_iter().zip(values) {
        prop.properties.set(key, value);
    }
    prop
}

#[test]
fn child_visibility_matches_literal_native_fades_dx_and_screen_flags_in_translated_and_nested_poses()
 {
    for nested in [false, true] {
        let origins = ["-64 8 32", "-16 8 32", "32 8 32", "80 8 32", "128 8 32"];
        let children = CASES
            .iter()
            .zip(origins)
            .enumerate()
            .map(|(index, (case, origin))| {
                let mut prop = visibility_prop(20 + index as u32, MODEL, origin, case.values);
                prop.properties.set("skin", (index % 2).to_string());
                prop
            })
            .collect();
        let (directory, mut parent, game) = prop_fixture(children);
        let root = parent.entities.last_mut().unwrap();
        let (positions, angles) = if nested {
            root.properties.set("file", "middle.vmf");
            root.properties.set("origin", "0 0 160");
            root.properties.set("angles", "90 0 0");
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "48 -16 32", "0 90 0")]),
            );
            (
                [
                    "64 -80 120",
                    "64 -32 120",
                    "64 16 120",
                    "64 64 120",
                    "64 112 120",
                ],
                "0 90 90",
            )
        } else {
            root.properties.set("origin", "24 -16 16");
            (
                ["-40 -8 48", "8 -8 48", "56 -8 48", "104 -8 48", "152 -8 48"],
                "0 0 0",
            )
        };
        let literals = CASES
            .iter()
            .zip(positions)
            .enumerate()
            .map(|(index, (case, origin))| {
                let mut prop = visibility_prop(2 + index as u32, MODEL, origin, case.values);
                prop.properties.set("skin", (index % 2).to_string());
                prop.properties.set("angles", angles);
                prop
            })
            .collect();
        let flat = literal_props(&parent, literals);
        let before = parent.clone();
        let source_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let context = source(directory.path());
        for mode in [
            crate::LightingMode::Fullbright,
            crate::LightingMode::DirectLdrHdr,
        ] {
            let actual = prop_output(&parent, &game, Some(&context), mode);
            let expected = prop_output(&flat, &game, None, mode);
            let bsp = actual.bsp.as_ref().unwrap();
            assert_literal_prop_bsp(bsp, expected.bsp.as_ref().unwrap());
            let native = bsp.static_props().unwrap().unwrap();
            assert_eq!((native.models.len(), native.props.len()), (1, 5));
            for (index, (prop, case)) in native.props.iter().zip(&CASES).enumerate() {
                assert_eq!(
                    [
                        prop.fade_min.to_bits(),
                        prop.fade_max.to_bits(),
                        prop.fade_scale.to_bits()
                    ],
                    case.fade.map(f32::to_bits)
                );
                assert_eq!(prop.dx_levels, case.dx);
                assert_eq!(prop.flags, case.flags);
                assert_eq!(prop.skin, (index % 2) as i32);
                assert!(prop.leaf_count > 0);
            }
            actual
                .static_prop_sources
                .as_ref()
                .unwrap()
                .verify_sources(&game, &Control::default())
                .unwrap();
        }
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            source_bytes
        );
    }
}

#[test]
fn absent_visibility_options_keep_explicit_defaults_and_native_bytes() {
    let prop = static_prop(20, MODEL, "16 8 32");
    let (directory, parent, game) = prop_fixture(vec![prop.clone()]);
    let context = source(directory.path());
    let explicit = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    let mut absent = prop;
    for key in KEYS {
        absent.properties.remove(key);
    }
    write_child(directory.path(), "child.vmf", &helper_file(vec![absent]));
    let omitted = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(
        explicit.bsp.as_ref().unwrap().to_bytes(),
        omitted.bsp.as_ref().unwrap().to_bytes()
    );
    let native = omitted
        .bsp
        .as_ref()
        .unwrap()
        .static_props()
        .unwrap()
        .unwrap();
    let prop = &native.props[0];
    assert_eq!(
        (
            prop.fade_min,
            prop.fade_max,
            prop.fade_scale,
            prop.flags,
            prop.dx_levels
        ),
        (0.0, 0.0, 1.0, 0, [0, 0])
    );
}

#[test]
fn visibility_flags_compose_with_selected_skin_scoped_origin_and_existing_lighting_bits() {
    let mut prop = visibility_prop(20, MODEL, "16 8 32", ["200", "40", "0", "1", "0", "0"]);
    for (key, value) in [
        ("skin", "1"),
        ("lightingorigin", "Anchor"),
        ("ignorenormals", "1"),
        ("disableshadows", "1"),
        ("disablevertexlighting", "1"),
        ("disableselfshadowing", "1"),
    ] {
        prop.properties.set(key, value);
    }
    let mut anchor = Entity::new(21, "info_lighting");
    anchor.properties.set("origin", "4 12 48");
    anchor.properties.set("targetname", "Anchor");
    let (directory, mut parent, game) = prop_fixture(vec![prop.clone(), anchor.clone()]);
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("origin", "24 -16 16");
    root.properties.set("targetname", "Outer");
    root.properties.set("fixup_style", "0");
    prop.properties.set("origin", "40 -8 48");
    prop.properties.set("lightingorigin", "Outer-Anchor");
    anchor.properties.set("origin", "28 -4 64");
    anchor.properties.set("targetname", "Outer-Anchor");
    let flat = literal_props(&parent, vec![prop, anchor]);
    let context = source(directory.path());
    let actual = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    let expected = prop_output(&flat, &game, None, crate::LightingMode::Fullbright);
    assert_eq!(
        actual.bsp.as_ref().unwrap().to_bytes(),
        expected.bsp.as_ref().unwrap().to_bytes()
    );
    let native = actual.bsp.unwrap().static_props().unwrap().unwrap();
    let prop = &native.props[0];
    assert_eq!(prop.flags, 1 | 2 | 8 | 16 | 32 | 64 | 128);
    assert_eq!(
        (prop.fade_min, prop.fade_max, prop.fade_scale),
        (200.0, 40.0, 0.0)
    );
    assert_eq!((prop.skin, prop.lighting_origin), (1, [28.0, -4.0, 64.0]));
}

#[test]
fn visibility_admission_preserves_raw_values_without_model_or_physics_reads() {
    let values = ["-9", "+1024.125", "0.125", "+01", "00090", "+00095"];
    let mut prop = visibility_prop(20, MODEL, "16 8 32", values);
    prop.properties.set("solid", "6");
    let (directory, parent, _) = prop_fixture(vec![prop]);
    let model = directory.path().join(MODEL);
    let physics = model.with_extension("phy");
    std::fs::write(&model, b"unread MDL").unwrap();
    std::fs::write(&physics, b"unread PHY").unwrap();
    let original = std::fs::read(directory.path().join("child.vmf")).unwrap();
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
        .find(|entity| entity.classname() == Some("prop_static"))
        .unwrap();
    for (key, value) in KEYS.into_iter().zip(values) {
        assert_eq!(generated.properties.get(key), Some(value));
    }
    assert_eq!(prepared.stats().unwrap().static_props, 1);
    assert_eq!(prepared.static_prop_poses[&generated.id].len(), 2);
    assert_eq!(std::fs::read(&model).unwrap(), b"unread MDL");
    assert_eq!(std::fs::read(&physics).unwrap(), b"unread PHY");
    assert_eq!(
        std::fs::read(directory.path().join("child.vmf")).unwrap(),
        original
    );
}

#[test]
fn malformed_visibility_fails_even_outside_region_and_preserves_previous_files() {
    let (directory, mut parent, game) = prop_fixture(Vec::new());
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let owner = parent.entities.last().unwrap().id;
    let context = source(directory.path());
    for (key, value, message) in [
        ("fademindist", "NaN", "fademindist must be a finite number"),
        ("fademindist", "1e40", "fademindist must be a finite number"),
        ("fademaxdist", "inf", "fademaxdist must be a finite number"),
        (
            "fademaxdist",
            "-1",
            "fade scale and end distance must be nonnegative",
        ),
        (
            "fadescale",
            "-0.5",
            "fade scale and end distance must be nonnegative",
        ),
        ("fadescale", "1e40", "fadescale must be a finite number"),
        ("screenspacefade", "2", "screenspacefade must be 0 or 1"),
        ("screenspacefade", "-1", "screenspacefade must be 0 or 1"),
        (
            "screenspacefade",
            "1.0",
            "screenspacefade must be an integer",
        ),
        (
            "mindxlevel",
            "-1",
            "mindxlevel must fit an unsigned native short",
        ),
        (
            "maxdxlevel",
            "65536",
            "maxdxlevel must fit an unsigned native short",
        ),
        ("mindxlevel", "90.0", "mindxlevel must be an integer"),
        ("mindxlevel", "2147483648", "mindxlevel must be an integer"),
        (
            "mindxlevel",
            "96",
            "minimum DX level exceeds maximum DX level",
        ),
    ] {
        let mut prop = visibility_prop(
            20,
            "models/props/unread.mdl",
            "192 0 32",
            ["128", "512", "1", "0", "80", "95"],
        );
        prop.properties.set("solid", "6");
        prop.properties.set(key, value);
        write_child(directory.path(), "child.vmf", &helper_file(vec![prop]));
        let before = parent.clone();
        let output = compiled(&parent, &game, &context);
        assert!(
            output.bsp.is_none() && output.leak.is_none(),
            "{key}={value}"
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                        && diagnostic.message.contains(message)
                        && diagnostic.message.contains("child.vmf")
                ),
            "{key}={value}: {:?}",
            output.report
        );
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("was not found"))
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        assert_eq!(parent, before);
    }
    for key in KEYS {
        let mut prop = static_prop(20, MODEL, "0 0 32");
        prop.properties
            .0
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap()
            .0 = key.to_uppercase();
        assert!(
            props::admit(&prop)
                .unwrap_err()
                .contains("canonical property"),
            "{key}"
        );
        let mut prop = static_prop(20, MODEL, "0 0 32");
        let value = prop.properties.get(key).unwrap().to_owned();
        prop.properties.0.push((key.to_uppercase(), value));
        assert!(
            props::admit(&prop)
                .unwrap_err()
                .contains("repeats property"),
            "{key}"
        );
    }
}

#[test]
fn valid_omitted_visibility_defers_mdl_phy_and_skin_range_but_invalid_options_do_not() {
    let retained = visibility_prop(20, MODEL, "0 0 32", ["64", "256", "0.5", "0", "80", "95"]);
    let mut omitted = visibility_prop(
        21,
        SECOND_MODEL,
        "192 0 32",
        ["256", "64", "2", "1", "65535", "0"],
    );
    omitted.properties.set("solid", "6");
    omitted.properties.set("skin", "2147483647");
    let mut hidden = omitted.clone();
    hidden.id = 22;
    hidden.hidden = true;
    hidden.properties.set("fademaxdist", "NaN");
    let (directory, mut parent, game) =
        prop_fixture(vec![retained.clone(), omitted.clone(), hidden.clone()]);
    let model = directory.path().join(SECOND_MODEL);
    let physics = model.with_extension("phy");
    std::fs::write(&model, b"invalid omitted MDL").unwrap();
    std::fs::write(&physics, b"invalid omitted PHY").unwrap();
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
    std::fs::write(&model, b"changed unread MDL").unwrap();
    std::fs::write(&physics, b"changed unread PHY").unwrap();
    output
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    omitted.properties.set("fadescale", "-1");
    write_child(
        directory.path(),
        "child.vmf",
        &helper_file(vec![retained, omitted, hidden]),
    );
    let invalid = compiled(&parent, &game, &context);
    assert!(invalid.bsp.is_none() && invalid.report.has_errors());
    assert!(
        invalid
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .message
                .contains("fade scale and end distance must be nonnegative")),
        "{:?}",
        invalid.report
    );
    assert!(
        !invalid
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("MDL")
                || diagnostic.message.contains("skin must be from"))
    );
}
