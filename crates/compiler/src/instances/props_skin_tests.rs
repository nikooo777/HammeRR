use super::*;

fn skin_prop(id: u32, model: &str, origin: &str, skin: Option<&str>) -> Entity {
    let mut prop = static_prop(id, model, origin);
    match skin {
        Some(skin) => prop.properties.set("skin", skin),
        None => {
            prop.properties.remove("skin");
        }
    }
    prop
}

#[test]
fn skin_admission_preserves_original_integer_spelling_without_model_reads() {
    for skin in [
        None,
        Some("0"),
        Some("1"),
        Some("+1"),
        Some("0001"),
        Some("2147483647"),
    ] {
        let (directory, parent, _) = prop_fixture(vec![skin_prop(20, MODEL, "16 8 32", skin)]);
        std::fs::write(directory.path().join(MODEL), b"unread model").unwrap();
        let before = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let context = source(directory.path());
        let prepared = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .unwrap();
        let prop = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("prop_static"))
            .unwrap();
        assert_eq!(prop.properties.get("skin"), skin);
        assert_eq!(prepared.stats().unwrap().static_props, 1);
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(directory.path().join(MODEL)).unwrap(),
            b"unread model"
        );
    }
    for skin in ["-1", "1.0", "", "NaN", "inf", "2147483648", " 1"] {
        let prop = skin_prop(20, MODEL, "16 8 32", Some(skin));
        assert!(
            props::admit(&prop)
                .unwrap_err()
                .contains("nonnegative integer"),
            "{skin}"
        );
    }
    let mut prop = skin_prop(20, MODEL, "16 8 32", Some("1"));
    prop.properties.0.push(("Skin".into(), "1".into()));
    assert!(
        props::admit(&prop)
            .unwrap_err()
            .contains("repeats property")
    );
}

#[test]
fn mixed_same_model_skins_match_independent_translation_and_noncommuting_literals() {
    for nested in [false, true] {
        let skins = if nested { ["1", "0"] } else { ["0", "1"] };
        let (directory, mut parent, game) = prop_fixture(vec![
            skin_prop(20, MODEL, "16 8 32", Some(skins[0])),
            skin_prop(21, MODEL, "-16 8 32", Some(skins[1])),
        ]);
        let root = parent.entities.last_mut().unwrap();
        let (origins, angles) = if nested {
            root.properties.set("file", "middle.vmf");
            root.properties.set("origin", "0 0 160");
            root.properties.set("angles", "90 0 0");
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "48 -16 32", "0 90 0")]),
            );
            (["64 0 120", "64 -32 120"], "0 90 90")
        } else {
            root.properties.set("origin", "24 -16 16");
            (["40 -8 48", "8 -8 48"], "0 0 0")
        };
        let mut literals = vec![
            skin_prop(2, MODEL, origins[0], Some(skins[0])),
            skin_prop(3, MODEL, origins[1], Some(skins[1])),
        ];
        for prop in &mut literals {
            prop.properties.set("angles", angles);
        }
        let flat = literal_props(&parent, literals);
        let before = parent.clone();
        let context = source(directory.path());
        for mode in [
            if nested {
                crate::LightingMode::DirectLdr
            } else {
                crate::LightingMode::Fullbright
            },
            crate::LightingMode::DirectLdrHdr,
        ] {
            let actual = prop_output(&parent, &game, Some(&context), mode);
            let expected = prop_output(&flat, &game, None, mode);
            actual
                .static_prop_sources
                .as_ref()
                .unwrap()
                .verify_sources(&game, &Control::default())
                .unwrap();
            let bsp = actual.bsp.as_ref().unwrap();
            assert_literal_prop_bsp(bsp, expected.bsp.as_ref().unwrap());
            let props = bsp.static_props().unwrap().unwrap();
            assert_eq!((props.models.len(), props.props.len()), (1, 2));
            assert_eq!(
                props
                    .props
                    .iter()
                    .map(|prop| prop.model)
                    .collect::<Vec<_>>(),
                [0, 0]
            );
            assert_eq!(
                props.props.iter().map(|prop| prop.skin).collect::<Vec<_>>(),
                if nested { [1, 0] } else { [0, 1] }
            );
            assert_eq!(actual.stats.instances.as_ref().unwrap().static_props, 2);
            assert_eq!(parent, before);
        }
    }
}

#[test]
fn absent_skin_keeps_explicit_zero_bsp_bytes() {
    let (directory, parent, game) = prop_fixture(vec![skin_prop(20, MODEL, "16 8 32", Some("0"))]);
    let context = source(directory.path());
    let explicit = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    write_child(
        directory.path(),
        "child.vmf",
        &helper_file(vec![skin_prop(20, MODEL, "16 8 32", None)]),
    );
    let absent = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    assert_eq!(
        absent.bsp.as_ref().unwrap().to_bytes(),
        explicit.bsp.as_ref().unwrap().to_bytes()
    );
}

#[test]
fn retained_skin_range_uses_original_header_and_preserves_previous_outputs() {
    let mut prop = skin_prop(20, MODEL, "16 8 32", Some("1"));
    prop.properties.set("solid", "6");
    let (directory, parent, game) = prop_fixture(vec![prop.clone()]);
    collision_tests::write_physics(directory.path());
    let outputs = tempfile::tempdir().unwrap();
    let destination = outputs.path().join("previous.bsp");
    let context = source(directory.path());
    let previous = prop_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    previous
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    assert!(previous.write(&destination).unwrap());
    let previous_bytes = std::fs::read(&destination).unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    for skin in ["2", "2147483647"] {
        prop.properties.set("skin", skin);
        write_child(
            directory.path(),
            "child.vmf",
            &helper_file(vec![prop.clone()]),
        );
        let child_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
        let before = parent.clone();
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none() && output.report.has_errors());
        let root = parent.entities.last().unwrap().id;
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.object == Some(format!("entity:{root}"))
                    && diagnostic
                        .message
                        .contains("skin must be from 0 to 1 for this model")
                    && diagnostic
                        .message
                        .contains("instance child.vmf, child object 20")
            }),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous_bytes);
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            child_bytes
        );
        assert_eq!(
            std::fs::read(directory.path().join("parent.vmf")).unwrap(),
            parent_bytes
        );
        assert_eq!(parent, before);
    }
}

#[test]
fn hidden_and_region_omitted_skins_defer_range_and_asset_admission() {
    let mut omitted = skin_prop(22, SECOND_MODEL, "256 0 32", Some("2147483647"));
    omitted.properties.set("solid", "6");
    let mut hidden = skin_prop(23, "models/props/absent.mdl", "NaN 0 0", Some("-1"));
    hidden.hidden = true;
    let mut children = vec![
        skin_prop(20, MODEL, "0 0 32", Some("1")),
        skin_prop(21, MODEL, "192 0 32", Some("2")),
        omitted,
        hidden,
    ];
    let (directory, mut parent, game) = prop_fixture(children.clone());
    std::fs::write(
        directory.path().join(SECOND_MODEL),
        b"invalid omitted model",
    )
    .unwrap();
    let physics = directory.path().join("models/props/second.phy");
    std::fs::write(&physics, b"invalid omitted physics").unwrap();
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
    assert_eq!(output.stats.instances.as_ref().unwrap().static_props, 3);
    assert_eq!(output.stats.static_props, 1);
    assert_eq!(
        output
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap()
            .props[0]
            .skin,
        1
    );
    std::fs::write(directory.path().join(SECOND_MODEL), b"changed unread model").unwrap();
    std::fs::write(physics, b"changed unread physics").unwrap();
    output
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    children[1].properties.set("origin", "16 0 32");
    write_child(directory.path(), "child.vmf", &helper_file(children));
    let retained = compiled(&parent, &game, &context);
    assert!(retained.bsp.is_none() && retained.report.has_errors());
    assert!(
        retained.report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("skin must be from 0 to 1 for this model")
                && diagnostic
                    .message
                    .contains("instance child.vmf, child object 21")
        }),
        "{:?}",
        retained.report
    );
}
