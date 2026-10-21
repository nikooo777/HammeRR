use super::*;

fn helper(id: u32, file: &str, origin: &str, angles: &str) -> Entity {
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    entity
}

#[test]
fn translated_and_noncommuting_child_props_preserve_baked_vertex_ordinals_and_channels() {
    for nested in [false, true] {
        let (directory, game) = fixtures::game();
        let mut parent = fixtures::scene(false);
        parent
            .entities
            .iter_mut()
            .find(|entity| entity.classname() == Some("light"))
            .unwrap()
            .properties
            .set("origin", "200 0 160");
        let mut leaf = Document::empty().into_vmf();
        let mut prop = Entity::new(20_001, "prop_static");
        for (key, value) in [
            ("origin", "16 8 32"),
            ("angles", "0 0 0"),
            ("model", "models/props/test.mdl"),
            ("solid", "0"),
            ("skin", "0"),
        ] {
            prop.properties.set(key, value);
        }
        leaf.entities.push(prop.clone());
        std::fs::write(
            directory.path().join("leaf.vmf"),
            leaf.write(&Default::default()),
        )
        .unwrap();
        if nested {
            let mut middle = Document::empty().into_vmf();
            middle
                .entities
                .push(helper(20_002, "leaf.vmf", "48 -16 32", "0 90 0"));
            std::fs::write(
                directory.path().join("middle.vmf"),
                middle.write(&Default::default()),
            )
            .unwrap();
            parent
                .entities
                .push(helper(20_003, "middle.vmf", "0 0 160", "90 0 0"));
            prop.properties.set("origin", "64 0 120");
            prop.properties.set("angles", "0 90 90");
        } else {
            parent
                .entities
                .push(helper(20_003, "leaf.vmf", "24 -16 16", "0 0 0"));
            prop.properties.set("origin", "40 -8 48");
        }
        let mut flat = parent.clone();
        flat.entities
            .retain(|entity| entity.classname() != Some("func_instance"));
        prop.id = 20_004;
        flat.entities.push(prop);
        let path = directory.path().join("parent.vmf");
        std::fs::write(&path, parent.write(&Default::default())).unwrap();
        let source = crate::instances::SourceContext::capture(&path, &Control::default()).unwrap();
        let actual = crate::compile_with_source_controlled(
            &parent,
            &game,
            options(true),
            None,
            Some(&source),
            &Control::default(),
        );
        assert!(!actual.report.has_errors(), "{:?}", actual.report);
        assert_eq!(actual.stats.instances.as_ref().unwrap().static_props, 1);
        let stats = actual.stats.prop_vertex_lighting.as_ref().unwrap();
        assert_eq!((stats.props, stats.models), (2, 1));
        let expected = compile(&flat, &game, true);
        let actual = actual.bsp.as_ref().unwrap();
        let expected = expected.bsp.as_ref().unwrap();
        for name in ["sp_0.vhv", "sp_1.vhv", "sp_hdr_0.vhv", "sp_hdr_1.vhv"] {
            assert_eq!(
                file(actual, name),
                file(expected, name),
                "nested={nested}, {name}"
            );
        }
        let ldr = file(actual, "sp_1.vhv");
        let hdr = file(actual, "sp_hdr_1.vhv");
        assert!(
            ldr.meshes
                .iter()
                .flat_map(|mesh| &mesh.colors)
                .any(|color| color.red > 0 && color.blue == 0)
        );
        assert!(
            hdr.meshes
                .iter()
                .flat_map(|mesh| &mesh.colors)
                .any(|color| color.blue > 0 && color.red == 0)
        );
        let actual =
            crate::prop_lighting_assets::read_generated_prop_lighting(actual, &Control::default())
                .unwrap()
                .unwrap();
        let expected = crate::prop_lighting_assets::read_generated_prop_lighting(
            expected,
            &Control::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            actual
                .receivers
                .iter()
                .map(|receiver| receiver.ordinal)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(actual.receivers, expected.receivers);
        assert_eq!(actual.assets, expected.assets);
        assert_eq!(actual.sources, expected.sources);
        assert_eq!(actual.missing_sources, expected.missing_sources);
    }
}
