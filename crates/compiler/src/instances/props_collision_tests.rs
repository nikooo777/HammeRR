use super::*;

const PHYSICS: &str = "models/props/instance.phy";

fn write_physics(directory: &Path) -> Vec<u8> {
    let model = std::fs::read(directory.join(MODEL)).unwrap();
    let checksum = i32::from_le_bytes(model[8..12].try_into().unwrap());
    let mut bytes = [16_i32, 0, 1, checksum].map(i32::to_le_bytes).concat();
    bytes.extend_from_slice(&32_i32.to_le_bytes());
    bytes.extend_from_slice(b"VPHY");
    bytes.extend_from_slice(&0x100_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 20]);
    bytes.extend_from_slice(b"solid {}\0");
    std::fs::write(directory.join(PHYSICS), &bytes).unwrap();
    bytes
}

fn collision_fixture(solid: Option<&str>, nested: bool) -> (tempfile::TempDir, Vmf, GameFs, Vmf) {
    let child = with_collision(static_prop(20, MODEL, "16 8 32"), solid);
    let (directory, mut parent, game) = prop_fixture(vec![child]);
    let root = parent.entities.last_mut().unwrap();
    let (origin, angles) = if nested {
        root.properties.set("file", "middle.vmf");
        root.properties.set("origin", "0 0 160");
        root.properties.set("angles", "90 0 0");
        write_child(
            directory.path(),
            "middle.vmf",
            &helper_file(vec![nested_helper(100, "child.vmf", "48 -16 32", "0 90 0")]),
        );
        ("64 0 120", "0 90 90")
    } else {
        root.properties.set("origin", "24 -16 16");
        ("40 -8 48", "0 0 0")
    };
    let mut literal = with_collision(static_prop(2, MODEL, origin), solid);
    literal.properties.set("angles", angles);
    let flat = literal_props(&parent, vec![literal]);
    (directory, parent, game, flat)
}

#[test]
fn collision_admission_preserves_implicit_default_without_resolving_assets() {
    for solid in [Some("0"), Some("2"), Some("6"), None] {
        let (directory, parent, _, _) = collision_fixture(solid, true);
        std::fs::write(directory.path().join(MODEL), b"unread model").unwrap();
        std::fs::write(directory.path().join(PHYSICS), b"unread physics").unwrap();
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
        assert_eq!(prop.properties.get("solid"), solid);
        assert_eq!(prepared.static_prop_poses[&prop.id].len(), 3);
        assert_eq!(
            prepared.root_entity(prop.id),
            Some(parent.entities.last().unwrap().id)
        );
    }
}

#[test]
fn translated_and_noncommuting_collision_props_match_literal_and_default_six_output() {
    for nested in [false, true] {
        for solid in [Some("2"), Some("6"), None] {
            let (directory, parent, game, flat) = collision_fixture(solid, nested);
            write_physics(directory.path());
            let context = source(directory.path());
            for mode in [
                crate::LightingMode::Fullbright,
                crate::LightingMode::DirectLdr,
                crate::LightingMode::DirectLdrHdr,
            ] {
                let actual = prop_output(&parent, &game, Some(&context), mode);
                let expected = prop_output(&flat, &game, None, mode);
                assert_eq!(actual.stats.instances.as_ref().unwrap().static_props, 1);
                assert_eq!(actual.stats.static_props, 1);
                assert_eq!(actual.stats.physics_solids, 0);
                actual
                    .static_prop_sources
                    .as_ref()
                    .unwrap()
                    .verify_sources(&game, &Control::default())
                    .unwrap();
                let actual = actual.bsp.unwrap();
                let expected = expected.bsp.unwrap();
                if nested {
                    assert_literal_prop_bsp(&actual, &expected);
                } else {
                    assert_eq!(actual.to_bytes(), expected.to_bytes());
                }
                let props = actual.static_props().unwrap().unwrap();
                assert_eq!(props.props[0].solid, if solid == Some("2") { 2 } else { 6 });
                if solid.is_none() {
                    let mut explicit = flat.clone();
                    explicit
                        .entities
                        .last_mut()
                        .unwrap()
                        .properties
                        .set("solid", "6");
                    let explicit = prop_output(&explicit, &game, None, mode);
                    assert_eq!(expected.to_bytes(), explicit.bsp.unwrap().to_bytes());
                }
            }
        }
    }
}

#[test]
fn collision_modes_keep_source_and_intermediate_model_box_diagnostics() {
    for solid in [Some("2"), Some("6"), None] {
        assert_model_boxes_checked(solid);
    }
}

fn oversized_physics_vpk(directory: &Path) -> Mount {
    let mut tree = b"phy\0models/props\0instance\0".to_vec();
    tree.extend(0_u32.to_le_bytes());
    tree.extend(1_u16.to_le_bytes());
    tree.extend(42_u16.to_le_bytes());
    tree.extend(0_u32.to_le_bytes());
    tree.extend(
        u32::try_from(source_formats::bsp::MAX_PHYSICS_BYTES)
            .unwrap()
            .to_le_bytes(),
    );
    tree.extend(0xffff_u16.to_le_bytes());
    tree.extend(b"x\0\0\0");
    let mut bytes = 0x55aa_1234_u32.to_le_bytes().to_vec();
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(u32::try_from(tree.len()).unwrap().to_le_bytes());
    bytes.extend(tree);
    let path = directory.join("oversized_dir.vpk");
    std::fs::write(&path, bytes).unwrap();
    Mount::Pak(source_formats::vpk::Vpk::open(path).unwrap())
}

#[test]
fn selected_child_physics_failures_keep_leaf_provenance_and_previous_outputs() {
    for solid in [Some("6"), None] {
        for oversized in [false, true] {
            let (directory, parent, mut game, _) = collision_fixture(solid, true);
            let physics = write_physics(directory.path());
            let outputs = tempfile::tempdir().unwrap();
            let destination = outputs.path().join("previous.bsp");
            let context = source(directory.path());
            let previous = prop_output(
                &parent,
                &game,
                Some(&context),
                crate::LightingMode::Fullbright,
            );
            assert!(previous.write(&destination).unwrap());
            let previous_bytes = std::fs::read(&destination).unwrap();
            std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
            let expected = if oversized {
                let mount = oversized_physics_vpk(directory.path());
                game.mounts.insert(
                    0,
                    MountedPath {
                        ids: vec!["game".into()],
                        mount,
                    },
                );
                "VPK entry exceeds the caller's byte read limit"
            } else {
                std::fs::remove_file(directory.path().join(PHYSICS)).unwrap();
                "needs its .phy collision asset for VPhysics"
            };
            let before = parent.clone();
            let child_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
            let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
            let output = compiled(&parent, &game, &context);
            assert!(output.bsp.is_none() && output.report.has_errors());
            let root = parent.entities.last().unwrap().id;
            assert!(
                output.report.diagnostics.iter().any(|diagnostic| {
                    diagnostic.object == Some(format!("entity:{root}"))
                        && diagnostic.message.contains(expected)
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
            assert_eq!(parent, before);
            assert_eq!(
                std::fs::read(directory.path().join("child.vmf")).unwrap(),
                child_bytes
            );
            assert_eq!(
                std::fs::read(directory.path().join("parent.vmf")).unwrap(),
                parent_bytes
            );
            if oversized {
                assert!(!directory.path().join("oversized_042.vpk").exists());
                assert_eq!(
                    std::fs::read(directory.path().join(PHYSICS)).unwrap(),
                    physics
                );
            }
        }
    }
}

#[test]
fn required_child_physics_remains_original_through_deferred_publication() {
    for solid in [Some("0"), Some("2"), Some("6"), None] {
        let (directory, parent, game, _) = collision_fixture(solid, true);
        let mut physics = write_physics(directory.path());
        let outputs = tempfile::tempdir().unwrap();
        let destination = outputs.path().join("previous.bsp");
        std::fs::write(&destination, b"previous BSP").unwrap();
        std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
        let context = source(directory.path());
        let output = prop_output(
            &parent,
            &game,
            Some(&context),
            crate::LightingMode::Fullbright,
        );
        let bytes = output.bsp.as_ref().unwrap().to_bytes();
        let guard = output.static_prop_sources.as_ref().unwrap();
        let control = Control::default();
        let lease = crate::OutputLease::acquire(&destination, &control).unwrap();
        let prepared = output
            .prepare_write_controlled(&destination, &control)
            .unwrap()
            .unwrap();
        let space = physics.len() - 4;
        assert_eq!(physics[space], b' ');
        physics[space] = b'\t';
        std::fs::write(directory.path().join(PHYSICS), physics).unwrap();
        output
            .instance_dependencies
            .as_ref()
            .unwrap()
            .verify(&control)
            .unwrap();
        if solid == Some("6") || solid.is_none() {
            assert!(guard.verify_sources(&game, &control).is_err());
            let current = prop_output(
                &parent,
                &game,
                Some(&context),
                crate::LightingMode::Fullbright,
            );
            assert_eq!(current.bsp.unwrap().to_bytes(), bytes);
            assert!(guard.verify_sources(&game, &control).is_err());
            assert!(prepared.commit_checked(&lease, &control).is_err());
            assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        } else {
            guard.verify_sources(&game, &control).unwrap();
            assert!(prepared.commit_checked(&lease, &control).unwrap());
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        }
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn mixed_collision_orders_share_model_ordinals_and_defer_only_unused_physics() {
    for order in [
        [Some("0"), Some("2"), Some("6")],
        [Some("6"), Some("2"), Some("0")],
        [Some("0"), Some("2"), None],
    ] {
        let props = order
            .iter()
            .enumerate()
            .map(|(index, solid)| {
                with_collision(
                    static_prop(20 + index as u32, MODEL, &format!("{} 0 32", index * 32)),
                    *solid,
                )
            })
            .collect();
        let (directory, parent, game) = prop_fixture(props);
        let physics = write_physics(directory.path());
        let context = source(directory.path());
        let output = prop_output(
            &parent,
            &game,
            Some(&context),
            crate::LightingMode::Fullbright,
        );
        let props = output
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap();
        assert_eq!(output.stats.instances.as_ref().unwrap().static_props, 3);
        assert_eq!(output.stats.static_prop_models, 1);
        assert_eq!(
            props
                .props
                .iter()
                .map(|prop| prop.model)
                .collect::<Vec<_>>(),
            [0, 0, 0]
        );
        assert_eq!(
            props
                .props
                .iter()
                .map(|prop| prop.solid)
                .collect::<Vec<_>>(),
            order.map(|solid| solid.unwrap_or("6").parse::<u8>().unwrap())
        );
        assert_eq!(
            std::fs::read(directory.path().join(PHYSICS)).unwrap(),
            physics
        );
        std::fs::remove_file(directory.path().join(PHYSICS)).unwrap();
        let missing = compiled(&parent, &game, &context);
        assert!(missing.bsp.is_none() && missing.report.has_errors());
        let required = order
            .iter()
            .position(|solid| solid.is_none() || *solid == Some("6"))
            .unwrap();
        assert!(
            missing.report.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("needs its .phy collision asset for VPhysics")
                    && diagnostic.message.contains(&format!(
                        "instance child.vmf, child object {}",
                        20 + required
                    ))
            }),
            "{:?}",
            missing.report
        );
    }
}

#[test]
fn hidden_and_region_omitted_six_props_do_not_consult_the_shared_models_physics() {
    for solid in [Some("6"), None] {
        for selected in ["0", "2"] {
            let mut hidden = with_collision(static_prop(21, MODEL, "0 0 32"), solid);
            hidden.hidden = true;
            let omitted = with_collision(static_prop(22, MODEL, "192 0 32"), solid);
            let (directory, mut parent, game) = prop_fixture(vec![
                with_collision(static_prop(20, MODEL, "0 0 32"), Some(selected)),
                hidden,
                omitted,
            ]);
            std::fs::write(directory.path().join(PHYSICS), b"invalid unused physics").unwrap();
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
            std::fs::write(directory.path().join(PHYSICS), b"changed unused physics").unwrap();
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
    }
}
