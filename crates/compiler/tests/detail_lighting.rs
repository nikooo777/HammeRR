mod common;
#[path = "common/detail_model.rs"]
mod detail_model;
#[path = "detail_lighting/generated_tint.rs"]
mod generated_tint;
#[path = "detail_lighting/tint.rs"]
mod tint;

use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, edit};
use source_formats::bsp::{Bsp, DetailProps};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Entity, Vmf};

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    source: Vmf,
}

fn entity(source: &mut Vmf, classname: &str, origin: &str, fields: &[(&str, &str)]) -> usize {
    let id = source
        .object_ids()
        .chain(source.side_ids())
        .max()
        .unwrap_or(0)
        + 1;
    let mut item = Entity::new(id, classname);
    item.properties.set("origin", origin);
    for &(key, value) in fields {
        item.properties.set(key, value);
    }
    source.entities.push(item);
    source.entities.len() - 1
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    detail_model::write(directory.path(), "test");
    std::fs::create_dir_all(directory.path().join("materials/test")).unwrap();
    std::fs::write(
        directory.path().join("materials/test/black.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/black".into();
        side.lightmap_scale = 64;
    }
    entity(
        &mut source,
        "prop_detail",
        "0 0 16",
        &[("model", "models/props/test.mdl")],
    );
    Fixture {
        directory,
        game,
        source,
    }
}

fn options(mode: LightingMode, bounces: u8) -> Options {
    Options {
        lighting: mode,
        sky_quality: SkyQuality::Fast,
        bounces,
        ..Default::default()
    }
}

fn compiled(fixture: &Fixture, mode: LightingMode, bounces: u8) -> Bsp {
    let output =
        compiler::compile_with_options(&fixture.source, &fixture.game, options(mode, bounces));
    assert!(!output.report.has_errors(), "{:?}", output.report);
    output.bsp.unwrap()
}

fn color(bsp: &Bsp) -> DVec3 {
    DVec3::from_array(
        bsp.detail_props().unwrap().unwrap().props[0]
            .lighting
            .linear(),
    )
}

#[test]
fn duplicate_native_placements_keep_distinct_named_receivers_through_leaf_sort_and_hdr() {
    let mut fixture = fixture();
    let first = fixture.source.entities.len() - 1;
    let second = entity(
        &mut fixture.source,
        "detail_prop",
        "0 0 16",
        &[("model", "models/props/test.mdl")],
    );
    for (name, point) in [("red_sample", "-96 0 64"), ("blue_sample", "96 0 64")] {
        entity(
            &mut fixture.source,
            "info_lighting",
            point,
            &[("targetname", name)],
        );
    }
    for (point, color) in [("-96 0 128", "255 0 0 80"), ("96 0 128", "0 0 255 80")] {
        entity(
            &mut fixture.source,
            "light",
            point,
            &[
                ("_light", color),
                ("_constant_attn", "0"),
                ("_linear_attn", "0"),
                ("_quadratic_attn", "1"),
            ],
        );
    }
    let no_reference = compiled(&fixture, LightingMode::Fullbright, 0).to_bytes();
    fixture.source.entities[first]
        .properties
        .set("lightingorigin", "red_sample");
    fixture.source.entities[second]
        .properties
        .set("lightingorigin", "blue_sample");
    assert_eq!(
        compiled(&fixture, LightingMode::Fullbright, 0).to_bytes(),
        no_reference
    );
    let source = fixture.source.write(&Default::default());
    for mode in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
        let bsp = compiled(&fixture, mode, 0);
        let details = bsp.detail_props().unwrap().unwrap();
        assert_eq!(details.props.len(), 2);
        let a = DVec3::from_array(details.props[0].lighting.linear());
        let b = DVec3::from_array(details.props[1].lighting.linear());
        assert!(a.x > a.z * 4.0 && b.z > b.x * 4.0, "{a:?} {b:?}");
        let mut same = details.props[0];
        same.lighting = details.props[1].lighting;
        assert_eq!(same, details.props[1]);
        fixture.source.entities[first]
            .properties
            .set("lightingorigin", "blue_sample");
        fixture.source.entities[second]
            .properties
            .set("lightingorigin", "red_sample");
        let swapped = compiled(&fixture, mode, 0).detail_props().unwrap().unwrap();
        assert_eq!(swapped.props[0].lighting, details.props[1].lighting);
        assert_eq!(swapped.props[1].lighting, details.props[0].lighting);
        fixture.source.entities[first]
            .properties
            .set("lightingorigin", "red_sample");
        fixture.source.entities[second]
            .properties
            .set("lightingorigin", "blue_sample");
        let reopened = Vmf::parse(&source).unwrap();
        assert_eq!(
            compiler::compile_with_options(&reopened, &fixture.game, options(mode, 0))
                .bsp
                .unwrap()
                .to_bytes(),
            bsp.to_bytes()
        );
    }
    assert_eq!(fixture.source.write(&Default::default()), source);
}

#[test]
fn named_detail_origins_reject_missing_hidden_duplicate_and_invalid_helpers_in_fullbright() {
    let fixture = fixture();
    for case in ["missing", "hidden", "duplicate", "invalid"] {
        let mut source = fixture.source.clone();
        source
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("lightingorigin", "sample");
        if case != "missing" {
            let helper = entity(
                &mut source,
                "info_lighting",
                if case == "invalid" {
                    "0 NaN 64"
                } else {
                    "0 0 64"
                },
                &[("targetname", "sample")],
            );
            source.entities[helper].hidden = case == "hidden";
            if case == "duplicate" {
                entity(
                    &mut source,
                    "info_lighting",
                    "0 0 64",
                    &[("targetname", "sample")],
                );
            }
        }
        let before = source.clone();
        let path = fixture.directory.path().join("prior.bsp");
        std::fs::write(&path, b"previous").unwrap();
        let output = compiler::compile(&source, &fixture.game);
        assert!(output.report.has_errors(), "{case}: {:?}", output.report);
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous");
        assert_eq!(source, before);
    }
}

#[test]
fn named_receivers_remain_attached_when_native_leaf_sort_reverses_emission_order() {
    let mut fixture = fixture();
    let mut document = Document::new(fixture.source);
    document
        .edit("split room", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-8.0, -256.0, 0.0),
                DVec3::new(8.0, 256.0, 160.0),
                "test/black",
            )
        })
        .unwrap();
    fixture.source = document.into_vmf();
    let first = fixture.source.entities.len() - 1;
    fixture.source.entities[first]
        .properties
        .set("origin", "-64 32 16");
    fixture.source.entities[first]
        .properties
        .set("lightingorigin", "red_sample");
    let second = entity(
        &mut fixture.source,
        "prop_detail",
        "64 32 16",
        &[
            ("model", "models/props/test.mdl"),
            ("lightingorigin", "blue_sample"),
        ],
    );
    for (name, point) in [("red_sample", "-96 0 64"), ("blue_sample", "96 0 64")] {
        entity(
            &mut fixture.source,
            "info_lighting",
            point,
            &[("targetname", name)],
        );
    }
    for (point, color) in [("-96 0 128", "255 0 0 80"), ("96 0 128", "0 0 255 80")] {
        entity(
            &mut fixture.source,
            "light",
            point,
            &[
                ("_light", color),
                ("_constant_attn", "0"),
                ("_linear_attn", "0"),
                ("_quadratic_attn", "1"),
            ],
        );
    }
    let before = compiled(&fixture, LightingMode::Fullbright, 0)
        .detail_props()
        .unwrap()
        .unwrap();
    assert_ne!(before.props[0].leaf, before.props[1].leaf);
    if before.props[0].origin[0] < 0.0 {
        fixture.source.entities.swap(first, second);
    }
    let emission_origin = fixture.source.entities[first].origin().unwrap().x;
    let baked = compiled(&fixture, LightingMode::DirectLdr, 0)
        .detail_props()
        .unwrap()
        .unwrap();
    assert_ne!(f64::from(baked.props[0].origin[0]), emission_origin);
    for prop in baked.props {
        let color = DVec3::from_array(prop.lighting.linear());
        if prop.origin[0] < 0.0 {
            assert!(color.x > color.z * 4.0, "{color:?}");
        } else {
            assert!(color.z > color.x * 4.0, "{color:?}");
        }
    }
}

#[test]
fn detail_direct_color_uses_rotated_validated_hull_center_and_independent_static_channels() {
    let mut fixture = fixture();
    let path = fixture.directory.path().join("models/props/test.mdl");
    let mut mdl = std::fs::read(&path).unwrap();
    for (index, value) in [2.0_f32, 4.0, 6.0, 4.0, 8.0, 10.0].into_iter().enumerate() {
        mdl[104 + index * 4..108 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, mdl).unwrap();
    let prop = fixture.source.entities.last_mut().unwrap();
    prop.properties.set("origin", "10 20 30");
    prop.properties.set("angles", "0 90 0");
    let light = entity(
        &mut fixture.source,
        "light",
        "4 23 138",
        &[
            ("_light", "255 0 0 200"),
            ("_lightHDR", "0 0 255 80"),
            ("_constant_attn", "1"),
            ("_linear_attn", "0"),
            ("_quadratic_attn", "0"),
        ],
    );
    let source = fixture.source.write(&Default::default());
    let ldr = compiled(&fixture, LightingMode::DirectLdr, 0);
    let dual = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    assert!((color(&ldr) - DVec3::new(200.0, 0.0, 0.0)).length() < 0.01);
    assert_eq!(color(&dual), DVec3::ZERO);
    assert!(ldr.detail_prop_lighting(false).unwrap().is_none());
    assert!(ldr.detail_prop_lighting(true).unwrap().is_none());
    let ldr_static = dual.detail_prop_lighting(false).unwrap().unwrap();
    let hdr_static = dual.detail_prop_lighting(true).unwrap().unwrap();
    assert_eq!(ldr_static.len(), 1);
    assert_eq!(hdr_static.len(), 1);
    assert_eq!(ldr_static[0].style, 0);
    assert_eq!(hdr_static[0].style, 0);
    assert_eq!(
        ldr_static[0].lighting,
        ldr.detail_props().unwrap().unwrap().props[0].lighting
    );
    assert_eq!(hdr_static[0].lighting.linear(), [0.0, 0.0, 80.0]);
    let mut expected: DetailProps = ldr.detail_props().unwrap().unwrap();
    expected.props[0].lighting = Default::default();
    expected.props[0].light_style_count = 1;
    assert_eq!(expected, dual.detail_props().unwrap().unwrap());
    assert_eq!(
        compiled(&fixture, LightingMode::DirectLdrHdr, 0).to_bytes(),
        dual.to_bytes()
    );
    let reopened = Vmf::parse(&source).unwrap();
    assert_eq!(
        compiler::compile_with_options(
            &reopened,
            &fixture.game,
            options(LightingMode::DirectLdrHdr, 0)
        )
        .bsp
        .unwrap()
        .to_bytes(),
        dual.to_bytes()
    );
    assert_eq!(fixture.source.write(&Default::default()), source);

    fixture.source.entities[light]
        .properties
        .set("_lightHDR", "255 0 0 200");
    let equal = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    assert_eq!(equal.detail_props().unwrap(), ldr.detail_props().unwrap());
    assert_eq!(
        equal.game_lump(*b"dprp").unwrap().data,
        ldr.game_lump(*b"dprp").unwrap().data
    );
    assert!(equal.detail_prop_lighting(false).unwrap().is_none());
    assert!(equal.detail_prop_lighting(true).unwrap().is_none());
}

#[test]
fn detail_lighting_keeps_fullbright_and_geometry_and_handles_zero_and_shadowed_light() {
    let mut fixture = fixture();
    let fullbright = compiled(&fixture, LightingMode::Fullbright, 0);
    assert_eq!(color(&fullbright), DVec3::splat(255.0));
    let dark = compiled(&fixture, LightingMode::DirectLdr, 0);
    assert_eq!(color(&dark), DVec3::ZERO);
    entity(
        &mut fixture.source,
        "light",
        "0.5 0.5 180",
        &[
            ("_light", "255 255 255 180"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    let bright = compiled(&fixture, LightingMode::DirectLdr, 0);
    assert!((color(&bright) - DVec3::splat(180.0)).length() < 0.01);
    let mut expected = fullbright.detail_props().unwrap().unwrap();
    expected.props[0].lighting = bright.detail_props().unwrap().unwrap().props[0].lighting;
    assert_eq!(expected, bright.detail_props().unwrap().unwrap());
    let mut document = Document::new(fixture.source.clone());
    document
        .edit("shadow", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 80.0),
                DVec3::new(16.0, 16.0, 96.0),
                "test/black",
            )
        })
        .unwrap();
    fixture.source = document.vmf().clone();
    assert_eq!(
        color(&compiled(&fixture, LightingMode::DirectLdr, 0)),
        DVec3::ZERO
    );
    let with_details = compiled(&fixture, LightingMode::DirectLdr, 0);
    fixture
        .source
        .entities
        .retain(|item| item.classname() != Some("prop_detail"));
    let without_details = compiled(&fixture, LightingMode::DirectLdr, 0);
    for index in 0..source_formats::bsp::LUMP_COUNT {
        assert_eq!(
            with_details.lump(index),
            without_details.lump(index),
            "Unrelated lump {index} changed"
        );
    }
}

#[test]
fn only_actual_direct_styles_allocate_native_detail_records() {
    for (origin, extra) in [
        ("0.5 0.5 180", vec![("_distance", "32")]),
        ("0.5 0.5 180", vec![("_light", "0 0 0 0")]),
    ] {
        let mut fixture = fixture();
        let mut fields = vec![
            ("_light", "255 255 255 120"),
            ("style", "1"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ];
        fields.extend(extra);
        entity(&mut fixture.source, "light", origin, &fields);
        assert_eq!(
            color(&compiled(&fixture, LightingMode::DirectLdr, 0)),
            DVec3::ZERO
        );
    }
    let mut fixture = fixture();
    entity(
        &mut fixture.source,
        "light",
        "0.5 0.5 180",
        &[
            ("_light", "255 255 255 120"),
            ("style", "1"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    let before = fixture.source.write(&Default::default());
    let bsp = compiled(&fixture, LightingMode::DirectLdr, 0);
    let props = bsp.detail_props().unwrap().unwrap();
    assert_eq!(
        (
            props.props[0].light_styles,
            props.props[0].light_style_count
        ),
        (0, 1)
    );
    assert_eq!(color(&bsp), DVec3::ZERO);
    let styles = bsp.detail_prop_lighting(false).unwrap().unwrap();
    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].style, 1);
    assert_eq!(styles[0].lighting.linear(), [60.0; 3]);
    assert_eq!(fixture.source.write(&Default::default()), before);
}

#[test]
fn reflected_styles_reach_details_even_when_the_direct_normal_faces_away() {
    let mut fixture = fixture();
    std::fs::write(
        fixture.directory.path().join("materials/test/lit.vmt"),
        "LightmappedGeneric { }",
    )
    .unwrap();
    for side in fixture
        .source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/lit".into();
    }
    fixture
        .source
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("angles", "180 0 0");
    let light = entity(
        &mut fixture.source,
        "light",
        "0 0 180",
        &[
            ("_light", "255 64 0 120"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    let base = color(&compiled(&fixture, LightingMode::DirectLdr, 0));
    let bounced_bsp = compiled(&fixture, LightingMode::DirectLdr, 1);
    let bounced = color(&bounced_bsp);
    assert!(base.x > 0.0 && base.y > 0.0 && base.z == 0.0, "{base}");
    assert!(bounced.x > base.x, "{base} -> {bounced}");
    let prop = fixture
        .source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("prop_detail"))
        .unwrap();
    prop.properties.set("_detail_lighting_tint", "128 64 0");
    let tinted = compiled(&fixture, LightingMode::DirectLdr, 1);
    tint::assert_record(
        tinted.detail_props().unwrap().unwrap().props[0],
        bounced_bsp.detail_props().unwrap().unwrap().props[0],
        [128, 64, 0],
    );
    fixture
        .source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("prop_detail"))
        .unwrap()
        .properties
        .remove("_detail_lighting_tint");
    fixture.source.entities[light].properties.set("style", "1");
    let bsp = compiled(&fixture, LightingMode::DirectLdr, 0);
    let extra = bsp.detail_prop_lighting(false).unwrap().unwrap();
    assert_eq!(extra.len(), 1);
    assert_eq!(extra[0].style, 1);
    let reflected = DVec3::from_array(extra[0].lighting.linear());
    assert!((reflected * 2.0 - base).abs().max_element() <= 1.0);
    assert_eq!(color(&bsp), DVec3::ZERO);
    fixture
        .source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("prop_detail"))
        .unwrap()
        .properties
        .set("_detail_lighting_tint", "128 64 0");
    let tinted = compiled(&fixture, LightingMode::DirectLdr, 0);
    tint::assert_color(
        tinted.detail_prop_lighting(false).unwrap().unwrap()[0].lighting,
        extra[0].lighting,
        [128, 64, 0],
    );
}

#[test]
fn a_fully_blocked_style_does_not_reject_static_detail_lighting() {
    let mut fixture = fixture();
    entity(
        &mut fixture.source,
        "light",
        "0.5 0.5 180",
        &[
            ("_light", "255 255 255 120"),
            ("style", "1"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    let mut document = Document::new(fixture.source);
    document
        .edit("opaque shelf", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 80.0),
                DVec3::new(16.0, 16.0, 96.0),
                "test/black",
            )
        })
        .unwrap();
    fixture.source = document.vmf().clone();
    assert_eq!(
        color(&compiled(&fixture, LightingMode::DirectLdr, 0)),
        DVec3::ZERO
    );
}

#[test]
fn detail_sun_and_spherical_sky_ambient_require_visible_sky() {
    let mut fixture = fixture();
    std::fs::write(
        fixture.directory.path().join("materials/test/sky.vmt"),
        "UnlitGeneric { %compilesky 1 }",
    )
    .unwrap();
    fixture.source.world.solids[1]
        .sides
        .iter_mut()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
        .unwrap()
        .material = "test/sky".into();
    entity(
        &mut fixture.source,
        "light_environment",
        "0 0 128",
        &[
            ("_light", "255 0 0 200"),
            ("_ambient", "0 0 255 100"),
            ("pitch", "-90"),
        ],
    );
    let open = color(&compiled(&fixture, LightingMode::DirectLdr, 0));
    assert!(
        (open.x - 200.0).abs() < 1.0 && open.y == 0.0 && open.z > 0.0,
        "{open}"
    );
    fixture.source.world.solids[1]
        .sides
        .iter_mut()
        .find(|side| side.material == "test/sky")
        .unwrap()
        .material = "test/black".into();
    assert_eq!(
        color(&compiled(&fixture, LightingMode::DirectLdr, 0)),
        DVec3::ZERO
    );
}

#[test]
fn material_sprites_and_explicit_models_share_baked_color_without_reseeding() {
    let mut fixture = fixture();
    std::fs::write(
        fixture.directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    std::fs::write(fixture.directory.path().join("materials/test/atlas.vmt"), "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 }").unwrap();
    let mut texture = vec![255; 80 + 16 * 16 * 4];
    texture[..80].fill(0);
    texture[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18] {
        texture[offset..offset + 2].copy_from_slice(&16_u16.to_le_bytes());
    }
    for offset in [24, 63] {
        texture[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    texture[20..24].copy_from_slice(&source_formats::vtf::FLAG_NOMIP.to_le_bytes());
    texture[56] = 1;
    texture[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    std::fs::write(
        fixture.directory.path().join("materials/test/atlas.vtf"),
        texture,
    )
    .unwrap();
    std::fs::write(fixture.directory.path().join("detail.vbsp"), "detail { tufts { density 64 group { alpha 1 sprite { sprite \"0 0 8 8 16\" spritesize \"0.25 0 20 40\" detailOrientation 2 upright 1 spriterandomscale 0.2 } } } }").unwrap();
    fixture
        .source
        .world
        .properties
        .set("detailmaterial", "test/atlas");
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .material = "test/ground".into();
    entity(
        &mut fixture.source,
        "light",
        "0 0 220",
        &[
            ("_light", "64 255 0 100"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    let before = fixture.source.write(&Default::default());
    let white = compiled(&fixture, LightingMode::Fullbright, 0)
        .detail_props()
        .unwrap()
        .unwrap();
    let baked_bsp = compiled(&fixture, LightingMode::DirectLdr, 0);
    let mut baked = baked_bsp.detail_props().unwrap().unwrap();
    assert_eq!(baked.props.len(), 17);
    assert_eq!(baked.props.iter().filter(|prop| prop.kind == 1).count(), 16);
    assert!(
        baked
            .props
            .iter()
            .any(|prop| prop.kind == 1 && prop.scale != 1.0)
    );
    for (baked, white) in baked.props.iter_mut().zip(&white.props) {
        let color = DVec3::from_array(baked.lighting.linear());
        assert!(
            color.y > color.x && color.x > 0.0 && color.z == 0.0,
            "{color}"
        );
        baked.lighting = white.lighting;
    }
    assert_eq!(baked, white);
    assert_eq!(
        compiled(&fixture, LightingMode::DirectLdr, 0).to_bytes(),
        baked_bsp.to_bytes()
    );
    assert_eq!(fixture.source.write(&Default::default()), before);
    fixture
        .source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("prop_detail"))
        .unwrap()
        .properties
        .set("_detail_lighting_tint", "128 64 0");
    let tinted = compiled(&fixture, LightingMode::DirectLdr, 0)
        .detail_props()
        .unwrap()
        .unwrap();
    let original = baked_bsp.detail_props().unwrap().unwrap();
    for (&actual, &old) in tinted.props.iter().zip(&original.props) {
        if actual.kind == 0 {
            tint::assert_record(actual, old, [128, 64, 0]);
        } else {
            assert_eq!(actual, old);
        }
    }
}

#[test]
fn detail_style_channel_union_keeps_shared_offsets_and_zero_fills_missing_channels() {
    let mut fixture = fixture();
    let second = entity(
        &mut fixture.source,
        "prop_detail",
        "64 0 16",
        &[("model", "models/props/test.mdl")],
    );
    for (style, ldr, hdr) in [
        ("1", "255 0 0 120", "0 0 0 0"),
        ("2", "0 0 0 0", "0 0 255 80"),
    ] {
        entity(
            &mut fixture.source,
            "light",
            "0.5 0.5 180",
            &[
                ("style", style),
                ("_light", ldr),
                ("_lightHDR", hdr),
                ("_constant_attn", "1"),
                ("_quadratic_attn", "0"),
            ],
        );
    }
    let before = fixture.source.write(&Default::default());
    let bsp = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    let props = bsp.detail_props().unwrap().unwrap();
    let ldr = bsp.detail_prop_lighting(false).unwrap().unwrap();
    let hdr = bsp.detail_prop_lighting(true).unwrap().unwrap();
    assert_eq!(ldr.len(), 4);
    assert_eq!(
        ldr.iter().map(|value| value.style).collect::<Vec<_>>(),
        [1, 2, 1, 2]
    );
    assert_eq!(
        ldr.iter().map(|value| value.style).collect::<Vec<_>>(),
        hdr.iter().map(|value| value.style).collect::<Vec<_>>()
    );
    assert_eq!(
        props
            .props
            .iter()
            .map(|prop| (prop.light_styles, prop.light_style_count))
            .collect::<Vec<_>>(),
        [(0, 2), (2, 2)]
    );
    assert_eq!(ldr[0].lighting.linear(), [60.0, 0.0, 0.0]);
    assert_eq!(hdr[0].lighting.linear(), [0.0; 3]);
    assert_eq!(ldr[1].lighting.linear(), [0.0; 3]);
    assert_eq!(hdr[1].lighting.linear(), [0.0, 0.0, 40.0]);
    assert_eq!(color(&bsp), DVec3::ZERO);
    assert_eq!(
        compiled(&fixture, LightingMode::DirectLdrHdr, 0).to_bytes(),
        bsp.to_bytes()
    );
    assert_eq!(fixture.source.write(&Default::default()), before);
    let reopened = Vmf::parse(&before).unwrap();
    assert_eq!(
        compiler::compile_with_options(
            &reopened,
            &fixture.game,
            options(LightingMode::DirectLdrHdr, 0)
        )
        .bsp
        .unwrap()
        .to_bytes(),
        bsp.to_bytes()
    );
    let fullbright = compiled(&fixture, LightingMode::Fullbright, 0);
    assert!(fullbright.detail_prop_lighting(false).unwrap().is_none());
    assert!(fullbright.detail_prop_lighting(true).unwrap().is_none());

    entity(
        &mut fixture.source,
        "light",
        "0.5 0.5 180",
        &[
            ("_light", "255 0 0 200"),
            ("_lightHDR", "0 0 255 80"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    fixture.source.entities[second]
        .properties
        .set("_detail_lighting_tint", "0 0 0");
    let split = compiled(&fixture, LightingMode::DirectLdrHdr, 0);
    let props = split.detail_props().unwrap().unwrap();
    let split_ldr = split.detail_prop_lighting(false).unwrap().unwrap();
    let split_hdr = split.detail_prop_lighting(true).unwrap().unwrap();
    assert_eq!(
        props
            .props
            .iter()
            .map(|prop| (prop.light_styles, prop.light_style_count))
            .collect::<Vec<_>>(),
        [(0, 3), (3, 2)]
    );
    assert!(props.props.iter().all(|prop| prop.lighting.rgb == [0; 3]));
    assert_eq!(split_ldr[0].style, 0);
    assert_eq!(split_hdr[0].style, 0);
    assert_eq!(split_ldr[0].lighting.linear(), [200.0, 0.0, 0.0]);
    assert_eq!(split_hdr[0].lighting.linear(), [0.0, 0.0, 80.0]);
    assert_eq!(&split_ldr[1..3], &ldr[..2]);
    assert_eq!(&split_hdr[1..3], &hdr[..2]);
    assert_eq!(
        split_ldr
            .iter()
            .map(|record| record.style)
            .collect::<Vec<_>>(),
        [0, 1, 2, 1, 2]
    );
    assert_eq!(
        split_hdr
            .iter()
            .map(|record| record.style)
            .collect::<Vec<_>>(),
        [0, 1, 2, 1, 2]
    );
    assert!(
        split_ldr[3..]
            .iter()
            .chain(&split_hdr[3..])
            .all(|record| record.lighting.rgb == [0; 3])
    );
}
