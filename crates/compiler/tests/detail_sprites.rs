mod common;
#[path = "common/detail_model.rs"]
mod detail_model;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, compile_region, edit, surface::FaceId};
use source_formats::bsp::{Bsp, DetailProps, Leaf};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Entity, Vmf};

const SPRITE: &str =
    "sprite \"0 0 4 4 8\" spritesize \"0.25 0.5 16 32\" detailOrientation 2 upright 1";
const ATLAS: &str = "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 }";

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    source: Vmf,
    previous: Vec<u8>,
}

fn texture(width: u16, height: u16, frames: u16) -> Vec<u8> {
    let mut data =
        vec![255; 80 + usize::from(width) * usize::from(height) * usize::from(frames) * 4];
    data[..80].fill(0);
    data[..4].copy_from_slice(b"VTF\0");
    data[4..8].copy_from_slice(&7_u32.to_le_bytes());
    data[8..12].copy_from_slice(&2_u32.to_le_bytes());
    data[12..16].copy_from_slice(&80_u32.to_le_bytes());
    data[16..18].copy_from_slice(&width.to_le_bytes());
    data[18..20].copy_from_slice(&height.to_le_bytes());
    data[24..26].copy_from_slice(&frames.to_le_bytes());
    data[20..24].copy_from_slice(&source_formats::vtf::FLAG_NOMIP.to_le_bytes());
    data[56] = 1;
    data[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    data[63..65].copy_from_slice(&1_u16.to_le_bytes());
    data
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/test")).unwrap();
    std::fs::write(
        directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    std::fs::write(directory.path().join("materials/test/atlas.vmt"), ATLAS).unwrap();
    std::fs::write(
        directory.path().join("materials/test/atlas.vtf"),
        texture(16, 16, 1),
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut source = common::room();
    source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .material = "test/ground".into();
    source.world.properties.set("detailmaterial", "test/atlas");
    let mut fixture = Fixture {
        directory,
        game,
        source,
        previous: Vec::new(),
    };
    dictionary(&fixture, 64, SPRITE);
    fixture.previous = compiled(&fixture).to_bytes();
    fixture
}

fn dictionary(fixture: &Fixture, density: u32, entry: &str) {
    std::fs::write(
        fixture.directory.path().join("detail.vbsp"),
        format!(
            "detail {{ tufts {{ density {density} group {{ alpha 1 item {{ {entry} }} }} }} }}"
        ),
    )
    .unwrap();
}

fn compiled(fixture: &Fixture) -> Bsp {
    let output = compiler::compile(&fixture.source, &fixture.game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    output.bsp.unwrap()
}

fn details(fixture: &Fixture) -> DetailProps {
    compiled(fixture).detail_props().unwrap().unwrap()
}

fn rejected(fixture: &Fixture, expected: &str) {
    let source = fixture.source.write(&Default::default());
    let destination = fixture.directory.path().join("previous.bsp");
    std::fs::write(&destination, &fixture.previous).unwrap();
    let output = compiler::compile(&fixture.source, &fixture.game);
    assert!(output.bsp.is_none(), "Invalid sprite build published a BSP");
    let messages = output
        .report
        .diagnostics
        .iter()
        .map(|row| row.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains(expected), "{messages}");
    assert!(!output.write(&destination).unwrap());
    assert_eq!(std::fs::read(destination).unwrap(), fixture.previous);
    assert_eq!(fixture.source.write(&Default::default()), source);
}

#[test]
fn sprites_preserve_logical_uvs_anchors_native_layout_and_unrelated_lumps() {
    let fixture = fixture();
    let output = compiler::compile(&fixture.source, &fixture.game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.detail_props, 16);
    assert_eq!(output.stats.detail_prop_models, 0);
    assert_eq!(output.stats.detail_prop_sprites, 1);
    let bytes = output.bsp.unwrap().to_bytes();
    let bsp = Bsp::parse(&bytes).unwrap();
    let rows = bsp.detail_props().unwrap().unwrap();
    assert!(rows.models.is_empty());
    assert_eq!(rows.sprites.len(), 1);
    let sprite = rows.sprites[0];
    assert_eq!(sprite.upper_left, [-4.0, 16.0]);
    assert_eq!(sprite.lower_right, [12.0, -16.0]);
    assert_eq!(sprite.texture_upper_left, [0.0625; 2]);
    assert_eq!(sprite.texture_lower_right, [0.4375; 2]);
    let leaves = bsp.records::<Leaf>().unwrap();
    for prop in rows.props {
        assert_eq!(prop.model, 0);
        assert_eq!(prop.kind, 1);
        assert_eq!(prop.orientation, 2);
        assert_eq!(
            (
                prop.light_styles,
                prop.light_style_count,
                prop.sway,
                prop.shape_angle,
                prop.shape_size
            ),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(prop.padding, [0; 3]);
        assert_eq!(prop.padding2, [0; 3]);
        assert_eq!(prop.scale, 1.0);
        assert_eq!(prop.lighting.rgb, [255; 3]);
        assert_eq!(prop.lighting.exponent, 0);
        assert_eq!(prop.origin[2], 0.0);
        assert!(leaves[usize::from(prop.leaf)].cluster >= 0);
    }
    assert_eq!(compiled(&fixture).to_bytes(), bytes);
    let reopened = Vmf::parse(&fixture.source.write(&Default::default())).unwrap();
    assert_eq!(
        compiler::compile(&reopened, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        bytes
    );
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let empty = compiled(&fixture);
    for index in 0..source_formats::bsp::LUMP_COUNT {
        assert_eq!(
            bsp.lump(index),
            empty.lump(index),
            "Unrelated lump {index} changed"
        );
    }
    assert_eq!(bsp.game_lump(*b"sprp"), empty.game_lump(*b"sprp"));
}

#[test]
fn sprite_atlas_patch_and_texture_keep_original_source_guards() {
    for path in [
        "materials/test/atlas.vmt",
        "materials/test/atlas_parent.vmt",
        "materials/test/atlas.vtf",
    ] {
        let fixture = fixture();
        std::fs::write(
            fixture
                .directory
                .path()
                .join("materials/test/atlas_parent.vmt"),
            ATLAS,
        )
        .unwrap();
        std::fs::write(
            fixture.directory.path().join("materials/test/atlas.vmt"),
            "Patch { include materials/test/atlas_parent.vmt }",
        )
        .unwrap();
        let output = compiler::compile(&fixture.source, &fixture.game);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.detail_props, 16);
        let control = compiler::Control::default();
        let guard = compiler::asset_sources::capture_output_asset_source_guard(
            &output,
            &fixture.game,
            &control,
        )
        .unwrap();
        let file = fixture.directory.path().join(path);
        let mut bytes = std::fs::read(&file).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(file, bytes).unwrap();
        assert!(guard.verify_metadata().is_err(), "{path}");
    }
}

#[test]
fn fixed_and_full_billboards_preserve_sampling_dictionary_and_native_records() {
    let fixture = fixture();
    let vertical = details(&fixture);
    for orientation in [0, 1] {
        dictionary(
            &fixture,
            64,
            &SPRITE.replace(
                "detailOrientation 2",
                &format!("detailOrientation {orientation}"),
            ),
        );
        let rows = details(&fixture);
        assert_eq!(rows.sprites, vertical.sprites);
        assert_eq!(rows.models, vertical.models);
        assert_eq!(rows.props.len(), vertical.props.len());
        for (mut prop, expected) in rows.props.into_iter().zip(&vertical.props) {
            assert_eq!(prop.orientation, orientation);
            assert_eq!(prop.scale, 1.0);
            prop.orientation = 2;
            assert_eq!(&prop, expected);
        }
        assert_eq!(compiled(&fixture).to_bytes(), compiled(&fixture).to_bytes());
    }
    dictionary(&fixture, 64, SPRITE);
    assert_eq!(compiled(&fixture).to_bytes(), fixture.previous);
}

#[test]
fn static_shapes_reuse_sprite_dictionary_and_preserve_native_parameter_bytes() {
    let fixture = fixture();
    let plain = details(&fixture);
    let fixed = SPRITE.replace("detailOrientation 2", "detailOrientation 0");
    for (shape, angle, size, kind, encoded) in [
        ("cross", 0, "0", 2, 0),
        ("tri", 35, "0.5", 3, 127),
        ("tri", 255, "1", 3, 255),
    ] {
        dictionary(
            &fixture,
            64,
            &format!("{fixed} sprite_shape {shape} shape_angle {angle} shape_size {size} sway 0"),
        );
        let rows = details(&fixture);
        assert_eq!(rows.sprites, plain.sprites);
        assert_eq!(rows.props.len(), plain.props.len());
        for (mut prop, expected) in rows.props.into_iter().zip(&plain.props) {
            assert_eq!(
                (
                    prop.kind,
                    prop.orientation,
                    prop.shape_angle,
                    prop.shape_size,
                    prop.sway,
                    prop.scale
                ),
                (kind, 0, angle, encoded, 0, 1.0)
            );
            prop.kind = 1;
            prop.orientation = 2;
            prop.shape_angle = 0;
            prop.shape_size = 0;
            assert_eq!(&prop, expected);
        }
        assert_eq!(compiled(&fixture).to_bytes(), compiled(&fixture).to_bytes());
    }
    for (extra, expected) in [
        ("sprite_shape unknown", "cross or tri"),
        ("sprite_shape cross shape_angle 1", "ignores"),
        ("sprite_shape cross shape_size 0.1", "ignores"),
        ("sprite_shape tri shape_angle 0.5", "integer"),
        ("sprite_shape tri shape_angle 256", "shape_angle"),
        ("sprite_shape tri shape_size -0.1", "shape_size"),
        ("sprite_shape tri shape_size NaN", "shape_size"),
        ("sprite_shape tri sway NaN", "sway"),
        (
            "sprite_shape tri spriterandomscale 1.1",
            "spriterandomscale",
        ),
    ] {
        dictionary(&fixture, 64, &format!("{fixed} {extra}"));
        rejected(&fixture, expected);
    }
    dictionary(&fixture, 64, &format!("{SPRITE} sprite_shape tri"));
    rejected(&fixture, "detailOrientation=0");
    for shape in ["cross", "tri"] {
        dictionary(
            &fixture,
            64,
            &format!(
                "{} sprite_shape {shape}",
                fixed.replace("16 32", "65536 32")
            ),
        );
        rejected(&fixture, "shape bounds");
    }
    std::fs::write(fixture.directory.path().join("detail.vbsp"), format!("detail {{ tufts {{ density 512 group {{ first {{ {fixed} sprite_shape cross }} second {{ {fixed} sprite_shape tri }} }} }} }}")).unwrap();
    let mixed = details(&fixture);
    assert_eq!(mixed.sprites.len(), 1);
    assert!(mixed.props.iter().any(|prop| prop.kind == 2));
    assert!(mixed.props.iter().any(|prop| prop.kind == 3));
    assert!(mixed.props.iter().all(|prop| prop.model == 0));
    dictionary(&fixture, 64, SPRITE);
    assert_eq!(compiled(&fixture).to_bytes(), fixture.previous);
}

#[test]
fn generated_sprite_sway_encodes_native_bytes_without_changing_sampling_or_zero_output() {
    let mut fixture = fixture();
    let source = fixture.source.write(&Default::default());
    let fixed = SPRITE.replace("detailOrientation 2", "detailOrientation 0");
    for entry in [
        format!("{fixed} sprite_shape cross spriterandomscale 0.2"),
        format!("{fixed} sprite_shape tri spriterandomscale 0.2"),
        format!("{fixed} spriterandomscale 0.2"),
        format!(
            "{} spriterandomscale 0.2",
            SPRITE.replace("detailOrientation 2", "detailOrientation 1")
        ),
        format!("{SPRITE} spriterandomscale 0.2"),
    ] {
        dictionary(&fixture, 64, &entry);
        let baseline = compiled(&fixture);
        let expected = baseline.detail_props().unwrap().unwrap();
        for (sway, encoded) in [
            ("0", 0),
            ("1e-100", 0),
            ("0.003921568627", 1),
            ("0.5", 127),
            ("1", 255),
        ] {
            dictionary(&fixture, 64, &format!("{entry} sway {sway}"));
            let bsp = compiled(&fixture);
            let bytes = bsp.to_bytes();
            if encoded == 0 {
                assert_eq!(bytes, baseline.to_bytes());
            }
            let mut rows = bsp.detail_props().unwrap().unwrap();
            assert!(!rows.props.is_empty());
            for prop in &mut rows.props {
                assert_eq!(prop.sway, encoded);
                prop.sway = 0;
            }
            assert_eq!(rows, expected);
            for index in 0..source_formats::bsp::LUMP_COUNT {
                assert_eq!(bsp.lump(index), baseline.lump(index));
            }
            assert_eq!(compiled(&fixture).to_bytes(), bytes);
            let reopened = Vmf::parse(&source).unwrap();
            assert_eq!(
                compiler::compile(&reopened, &fixture.game)
                    .bsp
                    .unwrap()
                    .to_bytes(),
                bytes
            );
        }
        for sway in ["NaN", "inf", "-0.1", "1.1"] {
            dictionary(&fixture, 64, &format!("{entry} sway {sway}"));
            rejected(&fixture, "sway");
        }
    }
    dictionary(&fixture, 64, "model models/props/test.mdl sway 0.1");
    rejected(&fixture, "sway");
    assert_eq!(fixture.source.write(&Default::default()), source);
    dictionary(&fixture, 64, SPRITE);
    assert_eq!(compiled(&fixture).to_bytes(), fixture.previous);
    detail_model::write(fixture.directory.path(), "test");
    let mut explicit = Entity::new(900_000, "prop_detail");
    explicit.properties.set("model", "models/props/test.mdl");
    explicit.properties.set("origin", "0 0 32");
    explicit.properties.set("sway", "0.1");
    fixture.source.entities.push(explicit);
    rejected(&fixture, "sway=0");
}

#[test]
fn native_half_sprite_paths_keep_raw_gaussian_scales_and_sampling() {
    let fixture = fixture();
    dictionary(&fixture, 512, &format!("{SPRITE} spriterandomscale 0.2"));
    let vertical = details(&fixture);
    for (orientation, shape, kind) in [
        (0, "", 1),
        (1, "", 1),
        (0, "sprite_shape cross", 2),
        (0, "sprite_shape tri shape_angle 35 shape_size 0.5", 3),
    ] {
        let entry = format!(
            "{} {shape} spriterandomscale 0.2",
            SPRITE.replace(
                "detailOrientation 2",
                &format!("detailOrientation {orientation}")
            )
        );
        dictionary(&fixture, 512, &entry);
        let scaled = details(&fixture);
        assert_eq!(scaled, details(&fixture));
        assert_eq!(scaled.sprites, vertical.sprites);
        assert_eq!(scaled.props.len(), vertical.props.len());
        assert!(scaled.props.iter().any(|prop| prop.scale < 1.0));
        assert!(scaled.props.iter().any(|prop| prop.scale > 1.0));
        for (mut prop, expected) in scaled.props.into_iter().zip(&vertical.props) {
            assert_eq!(prop.kind, kind);
            assert_eq!(prop.orientation, orientation);
            assert!(prop.scale >= f32::from_bits(103 << 23) && prop.scale <= 6.0);
            prop.kind = 1;
            prop.orientation = 2;
            prop.shape_angle = 0;
            prop.shape_size = 0;
            assert_eq!(&prop, expected);
        }
    }
    dictionary(&fixture, 64, SPRITE);
    assert_eq!(compiled(&fixture).to_bytes(), fixture.previous);
}

#[test]
fn gaussian_scaling_preserves_sampling_and_sprite_dictionary_deduplication() {
    let fixture = fixture();
    dictionary(&fixture, 512, SPRITE);
    let plain = details(&fixture);
    dictionary(&fixture, 512, &format!("{SPRITE} spriterandomscale 0.2"));
    let scaled = details(&fixture);
    assert_eq!(scaled, details(&fixture));
    assert_eq!(plain.sprites, scaled.sprites);
    assert_eq!(plain.props.len(), 134);
    assert_eq!(plain.props.len(), scaled.props.len());
    assert!(scaled.props.iter().any(|prop| prop.scale < 1.0));
    assert!(scaled.props.iter().any(|prop| prop.scale > 1.0));
    for (plain, scaled) in plain.props.iter().zip(&scaled.props) {
        assert!(scaled.scale > 0.0 && scaled.scale <= 6.0);
        assert_eq!(
            (plain.origin, plain.angles, plain.leaf),
            (scaled.origin, scaled.angles, scaled.leaf)
        );
    }
    std::fs::write(fixture.directory.path().join("detail.vbsp"), format!("detail {{ tufts {{ density 512 group {{ first {{ {SPRITE} amount 1 }} second {{ {SPRITE} amount 1 spriterandomscale 0.1 }} }} }} }}")).unwrap();
    let mixed = details(&fixture);
    assert_eq!(mixed.sprites.len(), 1);
    assert_eq!(mixed.props.len(), 134);
}

fn explicit_sprite(id: u32, orientation: u8) -> Entity {
    let mut entity = Entity::new(id, "prop_detail_sprite");
    for (key, value) in [
        ("origin", "0 0 32"),
        ("angles", "15 35 5"),
        ("position_ul", "-4 16"),
        ("position_lr", "12 -16"),
        ("tex_ul", "0 0"),
        ("tex_size", "4 4"),
        ("tex_total_size", "8"),
    ] {
        entity.properties.set(key, value);
    }
    entity
        .properties
        .set("detailOrientation", orientation.to_string());
    entity
}

#[test]
fn explicit_sprites_preserve_sdk_fields_deduplication_and_runtime_consumption() {
    let mut fixture = fixture();
    for orientation in 0..=2 {
        let mut entity = explicit_sprite(800_000 + u32::from(orientation), orientation);
        entity
            .properties
            .set("origin", format!("{} 0 32", 32 * i32::from(orientation)));
        fixture.source.entities.push(entity);
    }
    let authored = fixture.source.write(&Default::default());
    let bsp = compiled(&fixture);
    let rows = bsp.detail_props().unwrap().unwrap();
    assert_eq!(
        (rows.models.len(), rows.sprites.len(), rows.props.len()),
        (0, 1, 19)
    );
    let explicit = rows
        .props
        .iter()
        .filter(|prop| prop.origin[2] == 32.0)
        .collect::<Vec<_>>();
    assert_eq!(explicit.len(), 3);
    for orientation in 0..=2 {
        let prop = explicit
            .iter()
            .find(|prop| prop.origin[0] == 32.0 * f32::from(orientation))
            .unwrap();
        assert_eq!(
            (prop.kind, prop.orientation, prop.scale, prop.model),
            (1, orientation, 1.0, 0)
        );
        assert_eq!(prop.angles, [15.0, 35.0, 5.0]);
    }
    let text = std::str::from_utf8(&bsp.lump(source_formats::bsp::lump::ENTITIES).data).unwrap();
    assert!(!text.contains("prop_detail_sprite"));
    let reopened = Vmf::parse(&authored).unwrap();
    assert_eq!(
        compiler::compile(&reopened, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        bsp.to_bytes()
    );
    assert_eq!(fixture.source.write(&Default::default()), authored);
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    std::fs::write(fixture.directory.path().join("detail.vbsp"), "malformed").unwrap();
    let standalone = details(&fixture);
    assert_eq!(
        (
            standalone.models.len(),
            standalone.sprites.len(),
            standalone.props.len()
        ),
        (0, 1, 3)
    );
    dictionary(&fixture, 64, SPRITE);
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    fixture
        .source
        .entities
        .retain(|entity| entity.classname() != Some("prop_detail_sprite"));
    assert_eq!(compiled(&fixture).to_bytes(), fixture.previous);
}

#[test]
fn explicit_sprite_controls_share_native_records_and_preserve_zero_defaults() {
    let mut fixture = fixture();
    for (index, (orientation, shape)) in [
        (0, None),
        (1, None),
        (2, None),
        (0, Some("cross")),
        (0, Some("tri")),
    ]
    .into_iter()
    .enumerate()
    {
        let mut entity = explicit_sprite(800_000 + index as u32, orientation);
        entity
            .properties
            .set("origin", format!("{} 0 32", index as i32 * 32 - 64));
        entity.properties.set("scale", "1.001");
        if let Some(shape) = shape {
            entity.properties.set("sprite_shape", shape);
            if shape == "tri" {
                entity.properties.set("shape_angle", "35");
                entity.properties.set("shape_size", "0.5");
            }
        }
        fixture.source.entities.push(entity);
    }
    let options = compiler::Options {
        lighting: compiler::LightingMode::DirectLdr,
        bounces: 0,
        ..Default::default()
    };
    let baseline = compiler::compile_with_options(&fixture.source, &fixture.game, options);
    assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
    let baseline = baseline.bsp.unwrap();
    let expected = baseline.detail_props().unwrap().unwrap();
    for (sway, encoded) in [
        ("0", 0),
        ("1e-100", 0),
        ("0.003921568627", 1),
        ("0.5", 127),
        ("1", 255),
    ] {
        for entity in &mut fixture.source.entities {
            if entity.classname() == Some("prop_detail_sprite") {
                entity.properties.set("sway", sway);
            }
        }
        let source = fixture.source.write(&Default::default());
        let output = compiler::compile_with_options(&fixture.source, &fixture.game, options);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let mut rows = bsp.detail_props().unwrap().unwrap();
        assert_eq!(rows.sprites, expected.sprites);
        let explicit = rows
            .props
            .iter()
            .filter(|prop| prop.origin[2] == 32.0)
            .collect::<Vec<_>>();
        assert_eq!(explicit.len(), 5);
        for prop in explicit {
            assert_eq!(prop.scale, 1.001);
            assert_eq!(prop.sway, encoded);
            match prop.origin[0] as i32 {
                -64 => assert_eq!((prop.kind, prop.orientation), (1, 0)),
                -32 => assert_eq!((prop.kind, prop.orientation), (1, 1)),
                0 => assert_eq!((prop.kind, prop.orientation), (1, 2)),
                32 => assert_eq!((prop.kind, prop.shape_angle, prop.shape_size), (2, 0, 0)),
                64 => assert_eq!((prop.kind, prop.shape_angle, prop.shape_size), (3, 35, 127)),
                _ => unreachable!(),
            }
        }
        for prop in &mut rows.props {
            prop.sway = 0;
        }
        assert_eq!(rows, expected);
        if encoded == 0 {
            assert_eq!(bsp.to_bytes(), baseline.to_bytes());
        }
        for index in 0..source_formats::bsp::LUMP_COUNT {
            assert_eq!(baseline.lump(index), bsp.lump(index));
        }
        let reopened = Vmf::parse(&source).unwrap();
        assert_eq!(
            compiler::compile_with_options(&reopened, &fixture.game, options)
                .bsp
                .unwrap()
                .to_bytes(),
            bsp.to_bytes()
        );
        assert_eq!(fixture.source.write(&Default::default()), source);
    }
    let mut fixture = self::fixture();
    fixture.source.entities.push(explicit_sprite(800_000, 2));
    let baseline = compiled(&fixture).to_bytes();
    for (key, value) in [
        ("scale", "1"),
        ("sway", "0"),
        ("sprite_shape", ""),
        ("shape_angle", "0"),
        ("shape_size", "0"),
        ("spriterandomscale", "0"),
    ] {
        fixture
            .source
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set(key, value);
    }
    assert_eq!(compiled(&fixture).to_bytes(), baseline);
}

#[test]
fn explicit_sprite_scale_precision_matches_native_fast_and_slow_paths() {
    let mut fixture = fixture();
    let mut entity = explicit_sprite(800_000, 2);
    entity.properties.set("position_ul", "-0.0001 0.0001");
    entity.properties.set("position_lr", "0.0001 -0.0001");
    fixture.source.entities.push(entity);
    let base = fixture.source.clone();
    for (scale, accepted) in [
        ("0.0000000298023223876953125", false),
        ("0.000000059604644775390625", true),
        ("65504", true),
        ("65505", false),
    ] {
        for (orientation, shape, sway) in [
            (0, None, "0"),
            (1, None, "0"),
            (2, None, "0.5"),
            (0, Some("cross"), "0"),
            (0, Some("tri"), "0"),
        ] {
            fixture.source = base.clone();
            let entity = fixture.source.entities.last_mut().unwrap();
            entity
                .properties
                .set("detailOrientation", orientation.to_string());
            entity.properties.set("scale", scale);
            entity.properties.set("sway", sway);
            if let Some(shape) = shape {
                entity.properties.set("sprite_shape", shape);
            }
            if accepted {
                let rows = details(&fixture);
                assert!(
                    rows.props.iter().any(|prop| prop.origin[2] == 32.0
                        && prop.scale == scale.parse::<f32>().unwrap())
                );
            } else {
                rejected(&fixture, "half precision");
            }
        }
        fixture.source = base.clone();
        let entity = fixture.source.entities.last_mut().unwrap();
        entity.properties.set("scale", scale);
        let baseline = compiled(&fixture).to_bytes();
        fixture
            .source
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("sway", "1e-100");
        assert_eq!(compiled(&fixture).to_bytes(), baseline);
    }
}

#[test]
fn explicit_fast_sprite_bounds_reject_native_width_and_height_overflow_before_scaling() {
    let mut fixture = fixture();
    let mut entity = explicit_sprite(800_000, 2);
    entity.properties.set("position_ul", "-1.5e38 1.5e38");
    entity.properties.set("position_lr", "1.5e38 -1.5e38");
    entity.properties.set("scale", "1e-38");
    fixture.source.entities.push(entity);
    let baseline = compiled(&fixture);
    let rows = baseline.detail_props().unwrap().unwrap();
    assert!(
        rows.props
            .iter()
            .any(|prop| prop.origin[2] == 32.0 && prop.scale == 1e-38)
    );
    let base = fixture.source.clone();
    for (upper_left, lower_right) in [
        ("-3e38 1.5e38", "3e38 -1.5e38"),
        ("-1.5e38 3e38", "1.5e38 -3e38"),
        ("-3e38 3e38", "3e38 -3e38"),
    ] {
        fixture.source = base.clone();
        let entity = fixture.source.entities.last_mut().unwrap();
        entity.properties.set("position_ul", upper_left);
        entity.properties.set("position_lr", lower_right);
        rejected(&fixture, "geometry");
    }
}

#[test]
fn explicit_shape_controls_reject_unrepresentable_parameters_atomically() {
    let mut fixture = fixture();
    for changes in [
        vec![("sprite_shape", "unknown")],
        vec![("sprite_shape", "cross"), ("shape_angle", "1")],
        vec![("sprite_shape", "cross"), ("shape_size", "0.5")],
        vec![("sprite_shape", "tri"), ("shape_angle", "0.5")],
        vec![("sprite_shape", "tri"), ("shape_angle", "256")],
        vec![("sprite_shape", "tri"), ("shape_size", "NaN")],
        vec![("sprite_shape", "tri"), ("shape_size", "1.1")],
        vec![("sprite_shape", "tri"), ("detailOrientation", "2")],
        vec![("shape_angle", "1")],
        vec![("shape_size", "0.5")],
        vec![("scale", "NaN")],
        vec![("scale", "inf")],
        vec![("scale", "1e-100")],
        vec![("scale", "-1")],
        vec![("sway", "1.1")],
        vec![("sway", "-0.1")],
        vec![("modelscale", "2")],
        vec![
            ("scale", "2"),
            ("angles", "0 0 0"),
            ("position_ul", "-9000 16"),
        ],
    ] {
        let mut entity = explicit_sprite(800_000, 0);
        let expected = if changes.last().unwrap().0 == "position_ul" {
            "bounds"
        } else {
            changes.last().unwrap().0
        };
        for (key, value) in changes {
            entity.properties.set(key, value);
        }
        fixture.source.entities.push(entity);
        rejected(&fixture, expected);
        fixture.source.entities.pop();
    }
}

#[test]
fn explicit_sprite_failures_are_atomic_without_model_assets() {
    let mut fixture = fixture();
    for (key, value, expected) in [
        ("position_ul", "bad", "position_ul"),
        ("position_ul", "-40000 16", "bounds"),
        ("angles", "0 inf 0", "angles"),
        ("position_lr", "-4 -16", "geometry"),
        ("tex_ul", "NaN 0", "tex_ul"),
        ("tex_size", "0 4", "texture rectangle"),
        ("tex_total_size", "0", "texture rectangle"),
        ("detailOrientation", "0.5", "integer"),
        ("scale", "0", "scale"),
        ("rendercolor", "128 255 255", "rendercolor"),
        ("lightingorigin", "missing", "model-only"),
        ("parentname", "moving", "parentname"),
        ("sway", "NaN", "sway"),
        ("spriterandomscale", "0.2", "spriterandomscale=0"),
    ] {
        let mut entity = explicit_sprite(800_000, 2);
        entity.properties.set(key, value);
        fixture.source.entities.push(entity);
        rejected(&fixture, expected);
        fixture.source.entities.pop();
    }
    for key in [
        "position_ul",
        "position_lr",
        "tex_ul",
        "tex_size",
        "tex_total_size",
    ] {
        let mut missing = explicit_sprite(800_000, 2);
        missing.properties.remove(key);
        fixture.source.entities.push(missing);
        rejected(&fixture, &format!("requires {key}"));
        fixture.source.entities.pop();
    }
    fixture.source.entities.push(explicit_sprite(800_000, 2));
    std::fs::write(
        fixture.directory.path().join("materials/test/atlas.vmt"),
        "Water { }",
    )
    .unwrap();
    rejected(&fixture, "UnlitGeneric");
}

#[test]
fn regions_filter_explicit_sprites_by_origin_and_keep_normal_validation() {
    let mut fixture = fixture();
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    std::fs::create_dir_all(fixture.directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        fixture
            .directory
            .path()
            .join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut document = Document::new(fixture.source);
    document
        .edit("sprite region", |tx| {
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
    fixture.source = document.into_vmf();
    for (id, origin) in [(800_000, "0 0 32"), (800_001, "127.75 127.75 32")] {
        let mut entity = explicit_sprite(id, 2);
        entity.properties.set("origin", origin);
        entity.properties.set("angles", "0 0 0");
        entity.properties.set("position_ul", "-8 16");
        entity.properties.set("position_lr", "8 -16");
        fixture.source.entities.push(entity);
    }
    let mut outside = explicit_sprite(800_002, 2);
    outside.properties.set("origin", "128.001 0 32");
    outside.properties.remove("tex_size");
    outside.properties.set("scale", "2");
    fixture.source.entities.push(outside);
    let mut hidden = explicit_sprite(800_003, 2);
    hidden.hidden = true;
    hidden.properties.set("origin", "NaN 0 0");
    fixture.source.entities.push(hidden);
    let before = fixture.source.clone();
    let output = compiler::compile_with_options(
        &fixture.source,
        &fixture.game,
        compiler::Options {
            lighting: compiler::LightingMode::DirectLdr,
            bounces: 0,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let scope = output.stats.compile_region.as_ref().unwrap();
    for id in [800_000, 800_001] {
        assert!(scope.included.contains(&format!("entity:{id}")));
    }
    for id in [800_002, 800_003] {
        assert!(scope.omitted.contains(&format!("entity:{id}")));
    }
    let bsp = output.bsp.unwrap();
    let rows = bsp.detail_props().unwrap().unwrap();
    assert_eq!(
        (rows.models.len(), rows.sprites.len(), rows.props.len()),
        (0, 1, 2)
    );
    assert_eq!(rows.sprites[0].upper_left, [-8.0, 16.0]);
    assert_eq!(rows.sprites[0].lower_right, [8.0, -16.0]);
    assert!(
        rows.props
            .iter()
            .any(|prop| prop.origin == [127.75, 127.75, 32.0])
    );
    assert_eq!(fixture.source, before);
    fixture.previous = bsp.to_bytes();
    let base = fixture.source.clone();
    for (origin, key, value, expected) in [
        ("0 0 32", "tex_size", "0 4", "texture rectangle"),
        ("0 0 32", "position_ul", "-40000 16", "bounds"),
        ("0 0 32", "lightingorigin", "missing", "model-only"),
        ("128 0 32", "scale", "1", "open visible world leaf"),
    ] {
        fixture.source = base.clone();
        let mut entity = explicit_sprite(800_004, 2);
        entity.properties.set("origin", origin);
        entity.properties.set(key, value);
        fixture.source.entities.push(entity);
        rejected(&fixture, expected);
    }
}

#[test]
fn default_sprite_geometry_and_mixed_model_dictionary_references_remain_distinct() {
    let mut fixture = fixture();
    dictionary(&fixture, 64, "sprite \"4 4 1 1 8\" detailOrientation 2");
    detail_model::write(fixture.directory.path(), "test");
    let mut model = Entity::new(999_999, "prop_detail");
    model.properties.set("model", "models/props/test.mdl");
    model.properties.set("origin", "0 0 32");
    fixture.source.entities.push(model);
    let rows = details(&fixture);
    assert_eq!(
        (rows.models.len(), rows.sprites.len(), rows.props.len()),
        (1, 1, 17)
    );
    assert_eq!(rows.sprites[0].upper_left, [-10.0, 20.0]);
    assert_eq!(rows.sprites[0].lower_right, [10.0, 0.0]);
    assert_eq!(
        rows.sprites[0].texture_upper_left,
        rows.sprites[0].texture_lower_right
    );
    assert_eq!(rows.props.iter().filter(|prop| prop.kind == 0).count(), 1);
    assert_eq!(rows.props.iter().filter(|prop| prop.kind == 1).count(), 16);
    assert!(rows.props.iter().all(|prop| prop.model == 0));
    dictionary(&fixture, 62_500, SPRITE);
    rejected(&fixture, "16,384-object");
}

#[test]
fn terrain_and_flat_sprites_share_native_records_and_follow_final_surface_height() {
    let mut fixture = fixture();
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.material == "test/ground")
        .unwrap()
        .material = "dev/dev_measuregeneric01".into();
    let mut document = Document::new(fixture.source.clone());
    document
        .edit("sprite terrain", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-128.0, 32.0, 0.0),
                DVec3::new(128.0, 160.0, 24.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let side = tx
                .solid_mut(solid)?
                .sides
                .iter_mut()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap();
            side.material = "test/ground".into();
            let side = side.id;
            map_core::displacement::edit::create(tx, &[FaceId { solid, side }], 2, 0)?;
            Ok(())
        })
        .unwrap();
    fixture.source = document.vmf().clone();
    dictionary(&fixture, 512, &format!("{SPRITE} spriterandomscale 0.2"));
    let source = fixture.source.write(&Default::default());
    let rows = details(&fixture);
    assert_eq!(rows.props.len(), 16);
    assert_eq!(rows.sprites.len(), 1);
    assert!(rows.models.is_empty());
    assert!(rows.props.iter().any(|prop| prop.scale != 1.0));
    assert!(rows.props.iter().all(|prop| prop.kind == 1
        && prop.orientation == 2
        && prop.model == 0
        && prop.origin[2] == 24.0));
    assert!(
        rows.props
            .iter()
            .all(|prop| (-128.0..=128.0).contains(&prop.origin[0])
                && (32.0..=160.0).contains(&prop.origin[1]))
    );
    assert_eq!(rows, details(&fixture));
    assert_eq!(fixture.source.write(&Default::default()), source);
}

#[test]
fn unsupported_sprite_parameters_and_camera_facing_overflow_reject_atomically() {
    let fixture = fixture();
    for (fields, message) in [
        ("sprite \"0 0 4 4 8\" detailOrientation 1.5", "integer"),
        (
            "sprite \"0 0 4 4 8\" detailOrientation 3",
            "detailOrientation",
        ),
        ("sprite \"7 0 4 4 8\" detailOrientation 2", "rectangle"),
        (
            "sprite \"NaN 0 4 4 8\" detailOrientation 2",
            "finite numbers",
        ),
        (
            "sprite \"0 0 4 4 8\" spritesize \"2 0 16 32\" detailOrientation 2",
            "anchors",
        ),
        (
            "sprite \"0 0 4 4 8\" spritesize \"0.5 0 65536 32\" detailOrientation 2",
            "camera-facing bounds",
        ),
        (
            "sprite \"0 0 4 4 8\" spritesize \"0.5 0 1e-100 32\" detailOrientation 2",
            "anchors",
        ),
    ] {
        dictionary(&fixture, 64, fields);
        rejected(&fixture, message);
    }
    for extra in [
        "spriterandomscale NaN",
        "spriterandomscale 1.1",
        "sway 1.1",
        "spriteshape 1",
        "shape_angle 20",
        "model models/props/test.mdl",
    ] {
        dictionary(&fixture, 64, &format!("{SPRITE} {extra}"));
        rejected(&fixture, "Detail");
    }
    dictionary(
        &fixture,
        64,
        "sprite \"0 0 4 4 8\" spritesize \"0.5 0 32768 32\" detailOrientation 2",
    );
    rejected(&fixture, "camera-facing bounds");
    dictionary(&fixture, 100_000, SPRITE);
    rejected(&fixture, "16,384-object");
    dictionary(&fixture, 1_000_000, &format!("{SPRITE} amount 0"));
    rejected(&fixture, "sample build budget");
}

#[test]
fn atlas_assets_require_real_static_supported_material_and_complete_texture_data() {
    let mut fixture = fixture();
    let material = fixture.directory.path().join("materials/test/atlas.vmt");
    let atlas = fixture.directory.path().join("materials/test/atlas.vtf");
    fixture
        .source
        .world
        .properties
        .set("detailmaterial", " test/atlas ");
    rejected(&fixture, "surrounding whitespace");
    fixture.source.world.properties.set("detailmaterial", " ");
    rejected(&fixture, "surrounding whitespace");
    fixture.source.world.properties.set("detailmaterial", "");
    rejected(&fixture, "detail/detailsprites");
    fixture
        .source
        .world
        .properties
        .set("detailmaterial", "test/atlas");
    for text in [
        "VertexLitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 }",
        "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 }",
        "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 $additive 1 }",
        "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 Proxies { AnimatedTexture { } } }",
    ] {
        std::fs::write(&material, text).unwrap();
        rejected(&fixture, "static UnlitGeneric");
    }
    std::fs::write(&material, ATLAS).unwrap();
    let mut missing_mips = texture(16, 16, 1);
    missing_mips[20..24].copy_from_slice(&0u32.to_le_bytes());
    std::fs::write(&atlas, missing_mips).unwrap();
    rejected(&fixture, "native sampling requires");
    for bytes in [
        texture(16, 8, 1),
        texture(16, 16, 2),
        texture(8192, 1, 1),
        b"VTF\0".to_vec(),
        texture(16, 16, 1)[..90].to_vec(),
    ] {
        std::fs::write(&atlas, bytes).unwrap();
        rejected(&fixture, "Detail sprite atlas");
    }
    std::fs::remove_file(&atlas).unwrap();
    rejected(&fixture, "was not found");
    std::fs::remove_file(&material).unwrap();
    rejected(&fixture, "was not found");
}
