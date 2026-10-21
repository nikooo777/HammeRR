use super::*;

const FLAGS: [(&str, u32); 4] = [
    ("ignorenormals", 8),
    ("disableshadows", 16),
    ("disablevertexlighting", 64),
    ("disableselfshadowing", 128),
];

#[test]
fn lighting_options_admit_only_ordinary_zero_or_one_integers_and_preserve_spelling() {
    for (key, _) in FLAGS {
        for value in ["0", "1", "+1", "0001"] {
            let mut prop = static_prop(20, MODEL, "16 8 32");
            prop.properties.set(key, value);
            props::admit(&prop).unwrap();
        }
        for value in [
            "2",
            "-1",
            "1.0",
            "0.0",
            "",
            "NaN",
            "inf",
            "2147483648",
            " 1",
        ] {
            let mut prop = static_prop(20, MODEL, "16 8 32");
            prop.properties.set(key, value);
            assert!(
                props::admit(&prop)
                    .unwrap_err()
                    .contains(&format!("requires {key} 0 or 1")),
                "{key}={value}"
            );
        }
        let mut prop = static_prop(20, MODEL, "16 8 32");
        prop.properties
            .0
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap()
            .0 = key.to_uppercase();
        assert!(
            props::admit(&prop)
                .unwrap_err()
                .contains("canonical property")
        );
    }
    let mut prop = static_prop(20, MODEL, "16 8 32");
    for (key, _) in FLAGS {
        prop.properties.set(key, "+1");
    }
    let (directory, parent, _) = prop_fixture(vec![prop]);
    std::fs::write(directory.path().join(MODEL), b"unread model").unwrap();
    std::fs::write(
        directory.path().join("models/props/instance.phy"),
        b"unread physics",
    )
    .unwrap();
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
    for (key, _) in FLAGS {
        assert_eq!(generated.properties.get(key), Some("+1"));
    }
    assert_eq!(prepared.static_prop_poses[&generated.id].len(), 2);
    assert_eq!(
        prepared.root_entity(generated.id),
        Some(parent.entities.last().unwrap().id)
    );
    assert_eq!(
        std::fs::read(directory.path().join(MODEL)).unwrap(),
        b"unread model"
    );
}

#[test]
fn child_lighting_flags_match_literal_bits_with_skin_one_and_scoped_origin() {
    for mask in [8, 16, 64, 128, 8 | 16 | 64 | 128] {
        let mut prop = static_prop(20, MODEL, "16 8 32");
        prop.properties.set("skin", "1");
        prop.properties.set("lightingorigin", "Anchor");
        for (key, bit) in FLAGS {
            prop.properties
                .set(key, if mask & bit != 0 { "1" } else { "0" });
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
        let native = actual
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap();
        assert_eq!(native.models.len(), 1);
        assert_eq!(native.model_name(0).unwrap(), MODEL);
        assert_eq!(native.props.len(), 1);
        assert_eq!(native.props[0].flags, mask | 2);
        assert_eq!(native.props[0].skin, 1);
        assert_eq!(native.props[0].origin, [40.0, -8.0, 48.0]);
        assert_eq!(native.props[0].lighting_origin, [28.0, -4.0, 64.0]);
        assert!(native.props[0].leaf_count > 0);
    }
}

#[test]
fn absent_lighting_options_keep_explicit_zero_output_and_other_child_restrictions() {
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
    for (key, _) in FLAGS {
        absent.properties.remove(key);
    }
    write_child(directory.path(), "child.vmf", &helper_file(vec![absent]));
    let missing = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(
        explicit.bsp.unwrap().to_bytes(),
        missing.bsp.unwrap().to_bytes()
    );
    for (key, value) in [
        ("body", "1"),
        ("spawnflags", "1"),
        ("screenspacefade", "1"),
        ("generatelightmaps", "1"),
        ("fadescale", "2"),
        ("mindxlevel", "90"),
        ("modelscale", "2"),
    ] {
        let mut prop = static_prop(20, MODEL, "16 8 32");
        for (flag, _) in FLAGS {
            prop.properties.set(flag, "1");
        }
        prop.properties.set(key, value);
        assert!(props::admit(&prop).is_err(), "{key}={value}");
    }
}

#[test]
fn vertex_opt_out_keeps_source_and_intermediate_model_boxes_before_in_bounds_final_poses() {
    for intermediate in [false, true] {
        let mut prop = static_prop(
            20,
            MODEL,
            if intermediate { "0 0 32" } else { "16380 0 32" },
        );
        prop.properties.set("skin", "1");
        prop.properties.set("disablevertexlighting", "1");
        let (directory, mut parent, game) = prop_fixture(vec![prop]);
        if intermediate {
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "16380 0 0", "0 0 0")]),
            );
            parent
                .entities
                .last_mut()
                .unwrap()
                .properties
                .set("file", "middle.vmf");
        }
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("origin", "-16300 0 0");
        let context = source(directory.path());
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none() && output.report.has_errors());
        let owner = parent.entities.last().unwrap().id;
        let stage = if intermediate {
            "intermediate pose 1"
        } else {
            "source pose"
        };
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                        && diagnostic
                            .message
                            .contains(&format!("Static child prop {stage}"))
                        && diagnostic
                            .message
                            .contains("instance child.vmf, child object 20")
                ),
            "{:?}",
            output.report
        );
    }
}
