use super::*;

const MODEL: &str = "models/props/test.mdl";

fn receiver(origin: &str, name: &str) -> Entity {
    let mut prop = Entity::new(20_001, "prop_static");
    for (key, value) in [
        ("origin", origin),
        ("angles", "0 0 0"),
        ("model", MODEL),
        ("solid", "0"),
        ("body", "0"),
        ("skin", "0"),
        ("lightingorigin", name),
    ] {
        prop.properties.set(key, value);
    }
    prop
}

fn anchor(point: &str, name: &str) -> Entity {
    let mut helper = Entity::new(20_002, "info_lighting");
    helper.properties.set("origin", point);
    helper.properties.set("targetname", name);
    helper
}

fn child(directory: &std::path::Path, receiver_point: &str, helper_point: &str) {
    let mut vmf = Document::empty().into_vmf();
    vmf.entities.extend([
        receiver(receiver_point, "Anchor"),
        anchor(helper_point, "Anchor"),
    ]);
    std::fs::write(directory.join("child.vmf"), vmf.write(&Default::default())).unwrap();
}

fn bake(parent: &Vmf, game: &GameFs, directory: &std::path::Path) -> crate::Output {
    let source = crate::instances::SourceContext::capture(
        &directory.join("parent.vmf"),
        &Control::default(),
    )
    .unwrap();
    let output = crate::compile_with_source_controlled(
        parent,
        game,
        options(true),
        None,
        Some(&source),
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().lighting_origins, 1);
    assert_eq!(output.stats.prop_vertex_lighting.as_ref().unwrap().props, 2);
    output
}

fn payload(bsp: &Bsp, name: &str) -> Vec<u8> {
    bsp.pak_data(bsp::PakLimits::default())
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap()
        .data
}

fn manifest(bsp: &Bsp) -> crate::prop_lighting_assets::PropLightingManifest {
    crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
        .unwrap()
        .unwrap()
}

#[test]
fn moving_only_scoped_helper_preserves_complete_all_lod_dual_vhv_but_receiver_movement_changes_colors()
 {
    let (directory, game) = fixtures::game();
    fixtures::add_lod(directory.path());
    let mut parent = fixtures::scene(false);
    let mut helper = Entity::new(20_003, "func_instance");
    for (key, value) in [
        ("file", "child.vmf"),
        ("origin", "24 -16 16"),
        ("angles", "0 0 0"),
        ("targetname", "Scope"),
        ("fixup_style", "0"),
    ] {
        helper.properties.set(key, value);
    }
    parent.entities.push(helper);
    std::fs::write(
        directory.path().join("parent.vmf"),
        parent.write(&Default::default()),
    )
    .unwrap();
    child(directory.path(), "16 8 32", "4 12 48");
    let baseline = bake(&parent, &game, directory.path());
    let baseline_bsp = baseline.bsp.as_ref().unwrap();
    let baseline_manifest = manifest(baseline_bsp);
    assert_eq!(
        baseline_manifest
            .receivers
            .iter()
            .map(|receiver| receiver.ordinal)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(
        baseline_manifest
            .receivers
            .iter()
            .all(|receiver| receiver.meshes
                == [
                    crate::prop_lighting_assets::MeshLayout {
                        lod: 0,
                        vertex_count: 6
                    },
                    crate::prop_lighting_assets::MeshLayout {
                        lod: 1,
                        vertex_count: 4
                    }
                ])
    );
    let names = ["sp_0.vhv", "sp_hdr_0.vhv", "sp_1.vhv", "sp_hdr_1.vhv"];
    let original_payloads = names.map(|name| payload(baseline_bsp, name));
    for (name, hdr) in [("sp_1.vhv", false), ("sp_hdr_1.vhv", true)] {
        let parsed = file(baseline_bsp, name);
        assert_eq!(parsed.meshes.len(), 2);
        assert!(
            parsed
                .meshes
                .iter()
                .all(|mesh| mesh.colors.iter().any(|color| if hdr {
                    color.blue > 0 && color.red == 0
                } else {
                    color.red > 0 && color.blue == 0
                }))
        );
    }
    let original_props = baseline_bsp.static_props().unwrap().unwrap();
    assert_eq!(original_props.props[1].origin, [40.0, -8.0, 48.0]);
    assert_eq!(original_props.props[1].lighting_origin, [28.0, -4.0, 64.0]);
    assert_eq!(original_props.props[1].flags & 2, 2);
    let mut flat = parent.clone();
    flat.entities
        .retain(|entity| entity.classname() != Some("func_instance"));
    flat.entities.extend([
        receiver("40 -8 48", "Scope-Anchor"),
        anchor("28 -4 64", "Scope-Anchor"),
    ]);
    let literal = compile(&flat, &game, true);
    assert_eq!(
        literal
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap(),
        original_props
    );
    assert_eq!(
        names.map(|name| payload(literal.bsp.as_ref().unwrap(), name)),
        original_payloads
    );
    assert_eq!(
        manifest(literal.bsp.as_ref().unwrap()).receivers,
        baseline_manifest.receivers
    );

    child(directory.path(), "16 8 32", "64 32 64");
    let moved_helper = bake(&parent, &game, directory.path());
    let moved_bsp = moved_helper.bsp.as_ref().unwrap();
    let moved_manifest = manifest(moved_bsp);
    assert_eq!(
        moved_bsp.static_props().unwrap().unwrap().props[1].lighting_origin,
        [88.0, 16.0, 80.0]
    );
    assert_ne!(
        moved_manifest.static_props_sha256,
        baseline_manifest.static_props_sha256
    );
    assert_eq!(moved_manifest.receivers, baseline_manifest.receivers);
    assert_eq!(moved_manifest.assets, baseline_manifest.assets);
    assert_eq!(moved_manifest.sources, baseline_manifest.sources);
    assert_eq!(
        moved_manifest.missing_sources,
        baseline_manifest.missing_sources
    );
    assert_eq!(
        names.map(|name| payload(moved_bsp, name)),
        original_payloads
    );
    for index in 0..64 {
        if ![bsp::lump::GAME_LUMP, bsp::lump::PAKFILE].contains(&index) {
            assert_eq!(
                baseline_bsp.lump(index).data,
                moved_bsp.lump(index).data,
                "helper-only movement, lump {index}"
            );
        }
    }

    child(directory.path(), "16 8 160", "64 32 64");
    let moved_receiver = bake(&parent, &game, directory.path());
    let receiver_bsp = moved_receiver.bsp.as_ref().unwrap();
    assert_eq!(
        receiver_bsp.static_props().unwrap().unwrap().props[1].lighting_origin,
        [88.0, 16.0, 80.0]
    );
    for (name, original) in names.into_iter().zip(original_payloads) {
        if name.contains("_1.") {
            assert_ne!(
                payload(receiver_bsp, name),
                original,
                "moving the actual receiver must discriminate {name}"
            );
            assert!(
                file(receiver_bsp, name)
                    .meshes
                    .iter()
                    .flat_map(|mesh| &mesh.colors)
                    .all(|color| color.red == 0 && color.green == 0 && color.blue == 0)
            );
        } else {
            assert_eq!(payload(receiver_bsp, name), original);
        }
    }
    assert_eq!(
        parent.entities.last().unwrap().properties.get("targetname"),
        Some("Scope")
    );
}
