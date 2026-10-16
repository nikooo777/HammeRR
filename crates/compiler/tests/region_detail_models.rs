mod common;
#[path = "common/detail_model.rs"]
mod detail_model;

use glam::DVec3;
use map_core::{Document, compile_region, edit};
use source_formats::{
    bsp::{self, Leaf},
    gamefs::{GameFs, Mount, MountedPath},
    vmf::{Entity, Vmf},
};

fn fixture() -> (tempfile::TempDir, GameFs, Vmf) {
    let directory = tempfile::tempdir().unwrap();
    detail_model::write(directory.path(), "test");
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut document = Document::new(common::room());
    document
        .edit("region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::splat(128.0),
                    },
                }),
            )
        })
        .unwrap();
    (directory, game, document.into_vmf())
}

fn add(source: &mut Vmf, class: &str, origin: &str) -> usize {
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut entity = Entity::new(id, class);
    entity.properties.set("origin", origin);
    if ["prop_detail", "detail_prop"].contains(&class) {
        entity.properties.set("model", "models/props/test.mdl");
    }
    source.entities.push(entity);
    source.entities.len() - 1
}

#[test]
fn region_models_emit_checked_native_records_and_omit_outside_or_hidden_invalid_sources() {
    let (_directory, game, mut source) = fixture();
    let inside = add(&mut source, "prop_detail", "0.125 0.25 32.5");
    source.entities[inside].properties.set("angles", "0 450 0");
    let crossing = add(&mut source, "detail_prop", "127.75 127.75 32");
    let outside = add(&mut source, "detail_prop", "128.001 0 32");
    source.entities[outside]
        .properties
        .set("model", "models/missing.mdl");
    source.entities[outside].properties.set("modelscale", "2");
    let hidden = add(&mut source, "prop_detail_sprite", "NaN 0 0");
    source.entities[hidden].hidden = true;
    let helper = add(&mut source, "info_lighting", "400 0 64");
    source.entities[helper]
        .properties
        .set("targetname", "outside_helper");
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.detail_props, 2);
    let scope = output.stats.compile_region.as_ref().unwrap();
    for index in [inside, crossing, helper] {
        assert!(
            scope
                .included
                .contains(&format!("entity:{}", source.entities[index].id))
        );
    }
    for index in [outside, hidden] {
        assert!(
            scope
                .omitted
                .contains(&format!("entity:{}", source.entities[index].id))
        );
    }
    let bsp = output.bsp.unwrap();
    let props = bsp.detail_props().unwrap().unwrap();
    assert_eq!(props.props.len(), 2);
    assert!(
        props
            .props
            .iter()
            .any(|prop| prop.origin == [0.125, 0.25, 32.5] && prop.angles == [0.0, 90.0, 0.0])
    );
    assert!(
        props
            .props
            .iter()
            .any(|prop| prop.origin == [127.75, 127.75, 32.0])
    );
    let leaves = bsp.records::<Leaf>().unwrap();
    for prop in &props.props {
        let leaf = &leaves[usize::from(prop.leaf)];
        assert_eq!(leaf.contents & bsp::contents::SOLID, 0);
        assert!(leaf.cluster >= 0);
    }
    let text = String::from_utf8_lossy(&bsp.lump(bsp::lump::ENTITIES).data);
    assert!(!text.contains("prop_detail") && !text.contains("detail_prop"));
    assert_eq!(source, before);
}

#[test]
fn retained_detail_errors_preserve_prior_output_and_source_without_region_fallback() {
    let (directory, game, base) = fixture();
    for (origin, key, value, expected) in [
        ("0 0 32", "model", "models/missing.mdl", "model"),
        ("0 0 32", "modelscale", "2", "modelscale"),
        (
            "0 0 32",
            "lightingorigin",
            "outside_helper",
            "lighting origins",
        ),
        ("0 0 32", "parentname", "runtime_parent", "parentname"),
        ("128 0 32", "angles", "0 0 0", "open visible world leaf"),
    ] {
        let mut source = base.clone();
        let index = add(&mut source, "prop_detail", origin);
        source.entities[index].properties.set(key, value);
        let helper = add(&mut source, "info_lighting", "400 0 64");
        source.entities[helper]
            .properties
            .set("targetname", "outside_helper");
        let before = source.clone();
        let path = directory.path().join("previous.bsp");
        std::fs::write(&path, b"previous BSP").unwrap();
        let output = compiler::compile(&source, &game);
        assert!(
            output.bsp.is_none()
                && output
                    .report
                    .diagnostics
                    .iter()
                    .any(|item| item.message.contains(expected)),
            "{origin} {key}: {:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(source, before);
    }
    let mut document = Document::new(base);
    document
        .edit("solid leaf", |tx| {
            edit::create_block(
                tx,
                DVec3::splat(16.0),
                DVec3::splat(48.0),
                "tools/toolsblack",
            )
        })
        .unwrap();
    let mut source = document.into_vmf();
    add(&mut source, "detail_prop", "32 32 32");
    let output = compiler::compile(&source, &game);
    assert!(
        output.bsp.is_none()
            && output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("open visible world leaf")),
        "{:?}",
        output.report
    );
    let model = directory.path().join("models/props/test.mdl");
    let mut bytes = std::fs::read(&model).unwrap();
    bytes[140..144].copy_from_slice(&70000.0_f32.to_le_bytes());
    std::fs::write(model, bytes).unwrap();
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(
        output.bsp.is_none()
            && output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("model bounds")),
        "{:?}",
        output.report
    );
    let path = directory.path().join("previous.bsp");
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
    assert_eq!(source, before);
}

#[test]
fn disabled_regions_keep_full_map_bytes_and_lit_regions_keep_detail_lighting() {
    let (_directory, game, mut source) = fixture();
    add(&mut source, "prop_detail", "0 0 32");
    let light = add(&mut source, "light", "0 0 96");
    source.entities[light]
        .properties
        .set("_light", "255 64 32 200");
    source.entities[light].properties.set("_constant_attn", "1");
    let options = compiler::Options {
        lighting: compiler::LightingMode::DirectLdr,
        bounces: 0,
        ..Default::default()
    };
    let output = compiler::compile_with_options(&source, &game, options);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    let props = bsp.detail_props().unwrap().unwrap();
    assert_eq!(props.props.len(), 1);
    assert_ne!(props.props[0].lighting.rgb, [255; 3]);
    let mut document = Document::new(source.clone());
    let definition = compile_region::inspect(&source).unwrap().unwrap();
    document
        .edit("disable region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: false,
                    ..definition
                }),
            )
        })
        .unwrap();
    source.cordon = None;
    let full = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    let disabled = compiler::compile(document.vmf(), &game);
    assert!(disabled.stats.compile_region.is_none());
    assert_eq!(disabled.bsp.unwrap().to_bytes(), full);
}
