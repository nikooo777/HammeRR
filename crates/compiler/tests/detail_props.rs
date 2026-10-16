mod common;

use glam::DVec3;
use map_core::{Document, compile_region};
use source_formats::bsp::{self, Bsp, Leaf};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Entity, Vmf};

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("models/props")).unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut model = vec![0; 240];
    model[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
        model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, values) in [
        (104, [-16.0_f32, -8.0, 0.0]),
        (116, [16.0, 8.0, 32.0]),
        (128, [-16.0, -8.0, 0.0]),
        (140, [16.0, 8.0, 32.0]),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            model[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(directory.path().join("models/props/test.mdl"), model).unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn add(vmf: &mut Vmf, class: &str, origin: &str) -> usize {
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap_or(0) + 1;
    let mut entity = Entity::new(id, class);
    entity.properties.set("model", "models/props/test.mdl");
    entity.properties.set("origin", origin);
    vmf.entities.push(entity);
    vmf.entities.len() - 1
}

#[test]
fn explicit_detail_models_emit_real_records_consume_entities_and_preserve_geometry() {
    let (_directory, game) = game();
    let mut vmf = common::room();
    let baseline = compiler::compile(&vmf, &game).bsp.unwrap();
    let first = add(&mut vmf, "prop_detail", "64.125 -32.25 16.5");
    vmf.entities[first].properties.set("angles", "10 450 -20");
    add(&mut vmf, "detail_prop", "-64.25 32.5 32.125");
    let hidden = add(&mut vmf, "prop_detail_sprite", "NaN 0 0");
    vmf.entities[hidden].hidden = true;
    vmf.entities[hidden].properties.set("model", "missing.mdl");
    let before = vmf.write(&Default::default());
    let output = compiler::compile(&vmf, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.detail_props, 2);
    assert_eq!(output.stats.detail_prop_models, 1);
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("unbaked white lighting")
            && diagnostic.message.contains("non-colliding")
    }));
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let props = bsp.detail_props().unwrap().unwrap();
    assert_eq!(props.props.len(), 2);
    assert_eq!(props.model_name(0).unwrap(), "models/props/test.mdl");
    let prop = props
        .props
        .iter()
        .find(|prop| prop.origin[0] > 0.0)
        .unwrap();
    assert_eq!(prop.origin, [64.125, -32.25, 16.5]);
    assert_eq!(prop.angles, [10.0, 90.0, -20.0]);
    let leaves = bsp.records::<Leaf>().unwrap();
    for prop in &props.props {
        assert_eq!(prop.kind, 0);
        assert_eq!(prop.scale, 1.0);
        assert_eq!(prop.lighting.rgb, [255; 3]);
        assert_eq!(prop.lighting.exponent, 0);
        assert_eq!(prop.light_style_count, 0);
        assert_eq!(
            leaves[usize::from(prop.leaf)].contents & bsp::contents::SOLID,
            0
        );
        assert!(leaves[usize::from(prop.leaf)].cluster >= 0);
    }
    assert!(
        props
            .props
            .windows(2)
            .all(|pair| pair[0].leaf <= pair[1].leaf)
    );
    for index in 0..bsp::LUMP_COUNT {
        assert_eq!(
            bsp.lump(index),
            baseline.lump(index),
            "Unrelated lump {index} changed"
        );
    }
    assert_eq!(bsp.game_lump(*b"sprp"), baseline.game_lump(*b"sprp"));
    assert_eq!(vmf.write(&Default::default()), before);
    let reopened = Vmf::parse(&before).unwrap();
    assert_eq!(
        compiler::compile(&reopened, &game).bsp.unwrap().to_bytes(),
        bsp.to_bytes()
    );
}

#[test]
fn invalid_detail_models_preserve_previous_output_and_source() {
    let (directory, game) = game();
    for (key, value) in [
        ("model", "models/missing.mdl"),
        ("model", "models/../escape.mdl"),
        ("origin", "NaN 0 0"),
        ("origin", "0 0 -8"),
        ("origin", "65536 0 0"),
        ("angles", "bad"),
        ("angles", "0 inf 0"),
        ("skin", "1"),
        ("body", "1"),
        ("modelscale", "2"),
        ("scale", "0.5"),
        ("detailOrientation", "1"),
        ("rendercolor", "128 255 255"),
        ("renderamt", "128"),
        ("solid", "6"),
        ("lightingorigin", "sample"),
        ("fademaxdist", "500"),
    ] {
        let mut source = common::room();
        let index = add(&mut source, "prop_detail", "0 0 16");
        source.entities[index].properties.set(key, value);
        let before = source.write(&Default::default());
        let path = directory.path().join("previous.bsp");
        std::fs::write(&path, b"previous BSP").unwrap();
        let output = compiler::compile(&source, &game);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{key}={value}: {:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(source.write(&Default::default()), before);
    }
}

#[test]
fn model_type_bounds_sprites_and_compile_regions_reject_without_silent_fallback() {
    let (directory, game) = game();
    let path = directory.path().join("models/props/test.mdl");
    let original = std::fs::read(&path).unwrap();
    for (offset, value) in [
        (152, 0_u32),
        (128, f32::NAN.to_bits()),
        (140, 70000.0_f32.to_bits()),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let mut source = common::room();
        add(&mut source, "prop_detail", "0 0 16");
        let output = compiler::compile(&source, &game);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{:?}",
            output.report
        );
    }
    std::fs::write(&path, original).unwrap();
    let mut source = common::room();
    add(&mut source, "prop_detail_sprite", "0 0 16");
    let output = compiler::compile(&source, &game);
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("prop_detail_sprite compilation is unsupported")
    }));
    let mut document = Document::new(common::room());
    document
        .edit("region fixture", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 128.0),
                    },
                }),
            )
        })
        .unwrap();
    let mut source = document.vmf().clone();
    add(&mut source, "prop_detail", "0 0 16");
    let output = compiler::compile(&source, &game);
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("Compile regions do not support prop_detail")),
        "{:?}",
        output.report
    );
}

#[test]
fn maps_without_visible_detail_props_keep_the_existing_empty_payload() {
    let (_directory, game) = game();
    let mut source = common::room();
    let baseline = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    let index = add(&mut source, "prop_detail", "NaN 0 0");
    source.entities[index].hidden = true;
    let output = compiler::compile(&source, &game);
    assert_eq!(output.stats.detail_props, 0);
    assert_eq!(output.stats.detail_prop_models, 0);
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
}
