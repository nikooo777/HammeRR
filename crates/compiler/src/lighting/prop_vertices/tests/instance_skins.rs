use super::*;

const MODEL: &str = "models/props/test.mdl";
const SELECTED_MATERIAL: &str = "materials/models/test/two.vmt";

fn instance(id: u32, file: &str, origin: &str, angles: &str, name: Option<&str>) -> Entity {
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    if let Some(name) = name {
        entity.properties.set("targetname", name);
        entity.properties.set("fixup_style", "0");
    }
    entity
}

pub(super) fn scene(directory: &std::path::Path, skin: &str, scoped: bool) -> (Vmf, Vmf) {
    let mut parent = fixtures::scene(false);
    parent
        .entities
        .retain(|entity| entity.classname() != Some("prop_static"));
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap()
        .properties
        .set("origin", "200 0 160");
    let mut prop = Entity::new(20_001, "prop_static");
    for (key, value) in [
        ("origin", "16 8 32"),
        ("angles", "0 0 0"),
        ("model", MODEL),
        ("solid", "0"),
        ("skin", skin),
    ] {
        prop.properties.set(key, value);
    }
    let mut leaf = Document::empty().into_vmf();
    let mut anchor = Entity::new(20_002, "info_lighting");
    if scoped {
        prop.properties.set("lightingorigin", "Anchor");
        anchor.properties.set("targetname", "Anchor");
        anchor.properties.set("origin", "4 12 48");
        leaf.entities.push(anchor.clone());
    }
    leaf.entities.push(prop.clone());
    std::fs::write(directory.join("leaf.vmf"), leaf.write(&Default::default())).unwrap();
    let mut middle = Document::empty().into_vmf();
    middle.entities.push(instance(
        20_004,
        "leaf.vmf",
        "48 -16 32",
        "0 90 0",
        scoped.then_some("Inner"),
    ));
    std::fs::write(
        directory.join("middle.vmf"),
        middle.write(&Default::default()),
    )
    .unwrap();
    parent.entities.push(instance(
        20_003,
        "middle.vmf",
        "0 0 160",
        "90 0 0",
        scoped.then_some("Outer"),
    ));
    std::fs::write(
        directory.join("parent.vmf"),
        parent.write(&Default::default()),
    )
    .unwrap();
    let mut flat = parent.clone();
    flat.entities
        .retain(|entity| entity.classname() != Some("func_instance"));
    prop.properties.set("origin", "64 0 120");
    prop.properties.set("angles", "0 90 90");
    if scoped {
        prop.properties.set("lightingorigin", "Outer-Inner-Anchor");
        anchor.properties.set("targetname", "Outer-Inner-Anchor");
        anchor.properties.set("origin", "80 -12 124");
        flat.entities.push(anchor);
    }
    flat.entities.push(prop);
    (parent, flat)
}

pub(super) fn child_output(
    parent: &Vmf,
    game: &GameFs,
    directory: &std::path::Path,
) -> crate::Output {
    let source = crate::instances::SourceContext::capture(
        &directory.join("parent.vmf"),
        &Control::default(),
    )
    .unwrap();
    crate::compile_with_source_controlled(
        parent,
        game,
        options(true),
        None,
        Some(&source),
        &Control::default(),
    )
}

fn manifest(output: &crate::Output) -> crate::prop_lighting_assets::PropLightingManifest {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    crate::prop_lighting_assets::read_generated_prop_lighting(
        output.bsp.as_ref().unwrap(),
        &Control::default(),
    )
    .unwrap()
    .unwrap()
}

#[test]
fn selected_skin_keeps_literal_scoped_origin_and_complete_all_lod_dual_vhv() {
    let (directory, game) = fixtures::game();
    fixtures::add_lod(directory.path());
    let (parent, flat) = scene(directory.path(), "1", true);
    let before = parent.clone();
    let child_bytes = std::fs::read(directory.path().join("leaf.vmf")).unwrap();
    let actual = child_output(&parent, &game, directory.path());
    let expected = compile(&flat, &game, true);
    let actual_manifest = manifest(&actual);
    let expected_manifest = manifest(&expected);
    assert_eq!(actual.stats.instances.as_ref().unwrap().static_props, 1);
    assert_eq!(actual.stats.instances.as_ref().unwrap().lighting_origins, 1);
    assert_eq!(actual_manifest.receivers, expected_manifest.receivers);
    assert_eq!(actual_manifest.assets, expected_manifest.assets);
    assert_eq!(actual_manifest.sources, expected_manifest.sources);
    assert_eq!(
        actual_manifest.missing_sources,
        expected_manifest.missing_sources
    );
    assert_eq!(actual_manifest.receivers.len(), 1);
    assert_eq!(actual_manifest.receivers[0].ordinal, 0);
    assert_eq!(
        actual_manifest.receivers[0].meshes,
        [
            crate::prop_lighting_assets::MeshLayout {
                lod: 0,
                vertex_count: 6
            },
            crate::prop_lighting_assets::MeshLayout {
                lod: 1,
                vertex_count: 4
            },
        ]
    );
    let bsp = actual.bsp.as_ref().unwrap();
    let literal = expected.bsp.as_ref().unwrap();
    let native = bsp.static_props().unwrap().unwrap();
    let literal_native = literal.static_props().unwrap().unwrap();
    assert_eq!(native.models, literal_native.models);
    assert_eq!(native.leaves, literal_native.leaves);
    assert_eq!(native.props.len(), 1);
    let prop = &native.props[0];
    assert_eq!(prop.skin, 1);
    assert_eq!(prop.origin, [64.0, 0.0, 120.0]);
    assert_eq!(prop.lighting_origin, [80.0, -12.0, 124.0]);
    assert_eq!(prop.flags & 2, 2);
    for (a, b) in prop.angles.iter().zip(literal_native.props[0].angles) {
        assert!((*a - b).abs() <= 1e-6);
    }
    for (name, hdr) in [("sp_0.vhv", false), ("sp_hdr_0.vhv", true)] {
        let actual_file = file(bsp, name);
        assert_eq!(actual_file, file(literal, name));
        assert_eq!(actual_file.meshes.len(), 2);
        assert!(
            actual_file
                .meshes
                .iter()
                .all(|mesh| mesh.colors.iter().any(|color| {
                    if hdr {
                        color.blue > 0 && color.red == 0
                    } else {
                        color.red > 0 && color.blue == 0
                    }
                }))
        );
    }
    assert!(
        actual_manifest
            .sources
            .iter()
            .any(|source| source.path == SELECTED_MATERIAL)
    );
    assert!(
        !actual_manifest
            .sources
            .iter()
            .any(|source| source.path == "materials/one.vmt")
    );
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        child_bytes
    );
}

#[test]
fn mixed_skin_orders_admit_every_selected_material_on_one_cached_model() {
    for reverse in [false, true] {
        let (directory, game) = fixtures::game();
        let (parent, mut flat) = scene(directory.path(), "0", false);
        let path = directory.path().join("leaf.vmf");
        let mut leaf = Vmf::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut second = leaf.entities[0].clone();
        second.id = 20_002;
        second.properties.set("origin", "-16 8 32");
        second.properties.set("skin", "1");
        leaf.entities.push(second.clone());
        let first = flat.entities.pop().unwrap();
        assert_eq!(first.classname(), Some("prop_static"));
        second.properties.set("origin", "64 -32 120");
        second.properties.set("angles", "0 90 90");
        let mut literals = vec![first, second];
        if reverse {
            leaf.entities.reverse();
            literals.reverse();
        }
        flat.entities.extend(literals);
        std::fs::write(&path, leaf.write(&Default::default())).unwrap();
        let actual = child_output(&parent, &game, directory.path());
        let expected = compile(&flat, &game, true);
        let actual_manifest = manifest(&actual);
        let expected_manifest = manifest(&expected);
        assert_eq!(
            actual.stats.prop_vertex_lighting.as_ref().unwrap().models,
            1
        );
        assert_eq!(actual_manifest.receivers, expected_manifest.receivers);
        assert_eq!(actual_manifest.assets, expected_manifest.assets);
        assert_eq!(actual_manifest.sources, expected_manifest.sources);
        assert!(
            actual_manifest
                .sources
                .iter()
                .any(|source| source.path == SELECTED_MATERIAL)
        );
        assert!(
            actual_manifest
                .sources
                .iter()
                .any(|source| source.path == "materials/one.vmt")
        );
        let bsp = actual.bsp.as_ref().unwrap();
        let props = bsp.static_props().unwrap().unwrap();
        assert_eq!(props.models.len(), 1);
        assert_eq!(
            props.props.iter().map(|prop| prop.skin).collect::<Vec<_>>(),
            if reverse { [1, 0] } else { [0, 1] }
        );
        for name in ["sp_0.vhv", "sp_1.vhv", "sp_hdr_0.vhv", "sp_hdr_1.vhv"] {
            assert_eq!(file(bsp, name), file(expected.bsp.as_ref().unwrap(), name));
        }
        std::fs::write(
            directory.path().join(SELECTED_MATERIAL),
            "VertexLitGeneric { $bumpmap unsupported }",
        )
        .unwrap();
        let unsupported = child_output(&parent, &game, directory.path());
        assert!(unsupported.bsp.is_none() && unsupported.report.has_errors());
        assert!(
            unsupported.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("$bumpmap")
                    && diagnostic
                        .message
                        .contains("instance leaf.vmf, child object 20002")
            }),
            "reverse={reverse}: {:?}",
            unsupported.report
        );
    }
}

#[test]
fn selected_material_discriminates_skin_and_remains_guarded_after_output_drop() {
    let (directory, game) = fixtures::game();
    let outputs = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
    let destination = outputs.path().join("previous.bsp");
    let (parent_zero, _) = scene(directory.path(), "0", false);
    let zero = child_output(&parent_zero, &game, directory.path());
    let zero_manifest = manifest(&zero);
    assert!(
        !zero_manifest
            .sources
            .iter()
            .any(|source| source.path == SELECTED_MATERIAL)
    );
    assert!(zero.write(&destination).unwrap());
    let previous = std::fs::read(&destination).unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let selected_path = directory.path().join(SELECTED_MATERIAL);
    let original_material = std::fs::read(&selected_path).unwrap();
    std::fs::write(
        &selected_path,
        "VertexLitGeneric { $basetexture other $bumpmap unsupported }",
    )
    .unwrap();
    zero.prop_lighting_assets
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    zero.static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &Control::default())
        .unwrap();
    let (parent_one, _) = scene(directory.path(), "1", false);
    let before = parent_one.clone();
    let child_bytes = std::fs::read(directory.path().join("leaf.vmf")).unwrap();
    let unsupported = child_output(&parent_one, &game, directory.path());
    assert!(unsupported.bsp.is_none() && unsupported.report.has_errors());
    assert!(
        unsupported.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object.as_deref() == Some("entity:20003")
                && diagnostic.message.contains("$bumpmap")
                && diagnostic
                    .message
                    .contains("instance leaf.vmf, child object 20001")
        }),
        "{:?}",
        unsupported.report
    );
    assert!(!unsupported.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    assert_eq!(parent_one, before);
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        child_bytes
    );
    std::fs::write(&selected_path, original_material).unwrap();
    let selected = child_output(&parent_one, &game, directory.path());
    let selected_manifest = manifest(&selected);
    assert!(
        selected_manifest
            .sources
            .iter()
            .any(|source| source.path == SELECTED_MATERIAL)
    );
    assert!(
        !selected_manifest
            .sources
            .iter()
            .any(|source| source.path == "materials/one.vmt")
    );
    let control = Control::default();
    let prepared = selected
        .prepare_write_controlled(&destination, &control)
        .unwrap()
        .unwrap();
    std::fs::write(
        directory.path().join("materials/one.vmt"),
        "VertexLitGeneric { $bumpmap unused }",
    )
    .unwrap();
    selected
        .prop_lighting_assets
        .as_ref()
        .unwrap()
        .verify_sources(&game, &control)
        .unwrap();
    selected
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify_sources(&game, &control)
        .unwrap();
    std::fs::write(&selected_path, "VertexLitGeneric { $basetexture changed }").unwrap();
    assert!(
        selected
            .prop_lighting_assets
            .as_ref()
            .unwrap()
            .verify_sources(&game, &control)
            .is_err()
    );
    assert!(
        selected
            .static_prop_sources
            .as_ref()
            .unwrap()
            .verify_sources(&game, &control)
            .is_err()
    );
    drop(selected);
    assert!(prepared.commit(&control).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}
