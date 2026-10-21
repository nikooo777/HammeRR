use super::*;

const SELECTED_MATERIAL: &str = "materials/models/test/two.vmt";

fn flags(directory: &std::path::Path, flat: &mut Vmf, values: &[(&str, &str)]) {
    let path = directory.join("leaf.vmf");
    let mut leaf = Vmf::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for vmf in [&mut leaf, flat] {
        let prop = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == 20_001 && entity.classname() == Some("prop_static"))
            .unwrap();
        for &(key, value) in values {
            prop.properties.set(key, value);
        }
    }
    std::fs::write(path, leaf.write(&Default::default())).unwrap();
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
fn ignored_normals_change_real_dual_all_lod_colors_and_match_literal_selected_skin() {
    let (directory, game) = fixtures::game();
    fixtures::add_lod(directory.path());
    fixtures::normals(directory.path(), [0.0, 0.0, -1.0]);
    let (mut parent, mut flat) = instance_skins::scene(directory.path(), "1", true);
    for vmf in [&mut parent, &mut flat] {
        let light = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.classname() == Some("light"))
            .unwrap();
        light.properties.set("_light", "255 0 0 100");
        light.properties.set("_lightHDR", "0 0 255 200");
    }
    std::fs::write(
        directory.path().join("parent.vmf"),
        parent.write(&Default::default()),
    )
    .unwrap();
    for ignore in [false, true] {
        flags(
            directory.path(),
            &mut flat,
            &[
                ("disableshadows", "1"),
                ("ignorenormals", if ignore { "1" } else { "0" }),
            ],
        );
        let actual = instance_skins::child_output(&parent, &game, directory.path());
        let expected = compile(&flat, &game, true);
        let actual_manifest = manifest(&actual);
        let expected_manifest = manifest(&expected);
        assert_eq!(actual_manifest.receivers, expected_manifest.receivers);
        assert_eq!(actual_manifest.assets, expected_manifest.assets);
        assert_eq!(actual_manifest.sources, expected_manifest.sources);
        assert_eq!(actual_manifest.receivers.len(), 1);
        assert_eq!(actual.stats.prop_shadow_disabled, 1);
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
        let native = bsp.static_props().unwrap().unwrap();
        assert_eq!(native.props[0].skin, 1);
        assert_eq!(native.props[0].origin, [64.0, 0.0, 120.0]);
        assert_eq!(native.props[0].lighting_origin, [80.0, -12.0, 124.0]);
        assert_eq!(native.props[0].flags, if ignore { 26 } else { 18 });
        for (name, hdr) in [("sp_0.vhv", false), ("sp_hdr_0.vhv", true)] {
            let actual = file(bsp, name);
            assert_eq!(actual, file(expected.bsp.as_ref().unwrap(), name));
            let expected = vhv::Bgra8 {
                red: if ignore && !hdr { 52 } else { 0 },
                blue: if ignore && hdr { 71 } else { 0 },
                green: 0,
                alpha: 255,
            };
            assert!(
                actual.meshes.iter().all(|mesh| !mesh.colors.is_empty()
                    && mesh.colors.iter().all(|color| *color == expected)),
                "ignore={ignore}, {name}: {:?}",
                actual.meshes
            );
        }
    }
}

#[test]
fn vertex_opt_out_skips_only_unsupported_selected_receiver_and_still_requires_mdl_and_phy() {
    let (directory, game) = fixtures::game();
    let (mut parent, mut flat) = instance_skins::scene(directory.path(), "1", true);
    flags(directory.path(), &mut flat, &[("disableshadows", "1")]);
    let root_receiver = fixtures::scene(false)
        .entities
        .into_iter()
        .find(|entity| entity.classname() == Some("prop_static"))
        .unwrap();
    parent.entities.push(root_receiver.clone());
    flat.entities.insert(0, root_receiver);
    std::fs::write(
        directory.path().join("parent.vmf"),
        parent.write(&Default::default()),
    )
    .unwrap();
    std::fs::write(
        directory.path().join(SELECTED_MATERIAL),
        "VertexLitGeneric { $basetexture other $bumpmap unsupported }",
    )
    .unwrap();
    let rejected = instance_skins::child_output(&parent, &game, directory.path());
    assert!(rejected.bsp.is_none() && rejected.report.has_errors());
    assert!(
        rejected
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:20003")
                    && diagnostic.message.contains("$bumpmap")
                    && diagnostic
                        .message
                        .contains("instance leaf.vmf, child object 20001")
            ),
        "{:?}",
        rejected.report
    );
    flags(
        directory.path(),
        &mut flat,
        &[("disablevertexlighting", "1")],
    );
    let selected = instance_skins::child_output(&parent, &game, directory.path());
    let selected_manifest = manifest(&selected);
    let expected = compile(&flat, &game, true);
    assert_eq!(selected_manifest.receivers, manifest(&expected).receivers);
    assert_eq!(
        selected_manifest
            .receivers
            .iter()
            .map(|receiver| receiver.ordinal)
            .collect::<Vec<_>>(),
        [0]
    );
    let bsp = selected.bsp.as_ref().unwrap();
    let native = bsp.static_props().unwrap().unwrap();
    assert_eq!(native.props.len(), 2);
    assert_eq!(native.props[0].flags, 0);
    assert_eq!(native.props[1].flags, 2 | 16 | 64);
    assert_eq!(native.props[1].skin, 1);
    assert_eq!(native.props[1].lighting_origin, [80.0, -12.0, 124.0]);
    let archive = bsp.pak_data(bsp::PakLimits::default()).unwrap();
    assert!(
        archive
            .entries
            .iter()
            .all(|entry| !matches!(entry.name.as_str(), "sp_1.vhv" | "sp_hdr_1.vhv"))
    );
    for (name, hdr) in [("sp_0.vhv", false), ("sp_hdr_0.vhv", true)] {
        let remaining = file(bsp, name);
        assert_eq!(remaining, file(expected.bsp.as_ref().unwrap(), name));
        assert!(
            remaining
                .meshes
                .iter()
                .flat_map(|mesh| &mesh.colors)
                .any(|color| if hdr {
                    color.blue > 0 && color.red == 0
                } else {
                    color.red > 0 && color.blue == 0
                })
        );
    }
    for (key, value, message) in [
        (
            "model",
            "models/props/missing.mdl",
            "models/props/missing.mdl was not found",
        ),
        ("solid", "6", "needs its .phy collision asset for VPhysics"),
    ] {
        let mut ordinary = flat.clone();
        flags(directory.path(), &mut ordinary, &[(key, value)]);
        let failure = instance_skins::child_output(&parent, &game, directory.path());
        assert!(failure.bsp.is_none() && failure.report.has_errors());
        assert!(
            failure
                .report
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.object.as_deref() == Some("entity:20003")
                        && diagnostic.message.contains(message)
                        && diagnostic
                            .message
                            .contains("instance leaf.vmf, child object 20001")
                ),
            "{:?}",
            failure.report
        );
        flags(
            directory.path(),
            &mut ordinary,
            &[("model", "models/props/test.mdl"), ("solid", "0")],
        );
    }
}
