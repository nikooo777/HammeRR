mod common;
#[path = "common/detail_model.rs"]
mod detail_model;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit, surface::FaceId};
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
        dictionary(
            &fixture,
            64,
            &format!(
                "{} spriterandomscale 0.2",
                SPRITE.replace(
                    "detailOrientation 2",
                    &format!("detailOrientation {orientation}")
                )
            ),
        );
        rejected(&fixture, "spriterandomscale=0");
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
        ("sprite_shape tri sway 0.1", "sway"),
        ("sprite_shape tri spriterandomscale 0.1", "unit scale"),
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
        "sway 1",
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
