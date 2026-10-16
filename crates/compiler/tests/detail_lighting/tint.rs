use super::*;
use source_formats::bsp::{DetailProp, RgbExp32};

const KEY: &str = "_detail_lighting_tint";
const MODES: [LightingMode; 3] = [
    LightingMode::Fullbright,
    LightingMode::DirectLdr,
    LightingMode::DirectLdrHdr,
];

pub(super) fn atlas(fixture: &mut Fixture) {
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
    texture[56] = 1;
    texture[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    std::fs::write(
        fixture.directory.path().join("materials/test/atlas.vtf"),
        texture,
    )
    .unwrap();
    fixture
        .source
        .world
        .properties
        .set("detailmaterial", "test/atlas");
}

fn sprite(fixture: &mut Fixture, origin: &str, orientation: u8, shape: Option<&str>) -> usize {
    let orientation = orientation.to_string();
    let mut fields = vec![
        ("detailOrientation", orientation.as_str()),
        ("position_ul", "-4 16"),
        ("position_lr", "4 0"),
        ("tex_ul", "0 0"),
        ("tex_size", "8 8"),
        ("tex_total_size", "16"),
    ];
    if let Some(shape) = shape {
        fields.push(("sprite_shape", shape));
    }
    entity(&mut fixture.source, "prop_detail_sprite", origin, &fields)
}

fn family() -> (Fixture, Vec<usize>) {
    let mut fixture = fixture();
    atlas(&mut fixture);
    let mut indices = vec![fixture.source.entities.len() - 1];
    indices.push(entity(
        &mut fixture.source,
        "detail_prop",
        "128 0 16",
        &[("model", "models/props/test.mdl")],
    ));
    for (orientation, origin) in [(0, "-128 64 16"), (1, "-64 64 16"), (2, "64 64 16")] {
        indices.push(sprite(&mut fixture, origin, orientation, None));
    }
    for (shape, origin) in [("cross", "-64 -64 16"), ("tri", "64 -64 16")] {
        indices.push(sprite(&mut fixture, origin, 0, Some(shape)));
    }
    for (style, ldr, hdr) in [
        ("0", "255 255 255 128", "255 255 255 64"),
        ("1", "255 255 255 120", "0 0 0 0"),
        ("2", "0 0 0 0", "255 255 255 80"),
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
    (fixture, indices)
}

pub(super) fn assert_color(actual: RgbExp32, untinted: RgbExp32, tint: [u8; 3]) {
    let quantization =
        2.0_f64.powi(i32::from(actual.exponent)) + 2.0_f64.powi(i32::from(untinted.exponent));
    let actual = DVec3::from_array(actual.linear());
    let expected = DVec3::from_array(untinted.linear())
        * DVec3::from_array(tint.map(|channel| f64::from(channel) / 255.0));
    assert!(
        (actual - expected).abs().max_element() <= quantization,
        "{actual} != {expected}"
    );
    for channel in 0..3 {
        if tint[channel] == 0 {
            assert_eq!(actual[channel], 0.0);
        }
    }
}

pub(super) fn assert_record(mut actual: DetailProp, untinted: DetailProp, tint: [u8; 3]) {
    assert_color(actual.lighting, untinted.lighting, tint);
    actual.lighting = untinted.lighting;
    assert_eq!(actual, untinted);
}

#[test]
fn white_tint_keeps_omitted_bsp_bytes_for_models_cards_shapes_and_both_channels() {
    let (mut fixture, indices) = family();
    for mode in MODES {
        for &index in &indices {
            fixture.source.entities[index].properties.remove(KEY);
        }
        let omitted = compiled(&fixture, mode, 0).to_bytes();
        for &index in &indices {
            fixture.source.entities[index]
                .properties
                .set(KEY, "255 255 255");
        }
        let before = fixture.source.write(&Default::default());
        assert_eq!(compiled(&fixture, mode, 0).to_bytes(), omitted);
        let reopened = Vmf::parse(&before).unwrap();
        assert_eq!(
            compiler::compile_with_options(&reopened, &fixture.game, options(mode, 0))
                .bsp
                .unwrap()
                .to_bytes(),
            omitted
        );
        assert_eq!(fixture.source.write(&Default::default()), before);
    }
}

#[test]
fn linear_tint_modulates_completed_base_and_half_styles_without_changing_native_paths() {
    let (mut fixture, indices) = family();
    for mode in MODES {
        for &index in &indices {
            fixture.source.entities[index].properties.remove(KEY);
        }
        let untinted = compiled(&fixture, mode, 0);
        let original = untinted.detail_props().unwrap().unwrap();
        assert_eq!(original.props.len(), 7);
        for (text, tint) in [("128 64 0", [128, 64, 0]), ("0 0 0", [0; 3])] {
            for &index in &indices {
                fixture.source.entities[index].properties.set(KEY, text);
            }
            let before = fixture.source.write(&Default::default());
            let tinted = compiled(&fixture, mode, 0);
            let details = tinted.detail_props().unwrap().unwrap();
            assert_eq!(details.models, original.models);
            assert_eq!(details.sprites, original.sprites);
            for (&actual, &old) in details.props.iter().zip(&original.props) {
                assert_record(actual, old, tint);
                if mode == LightingMode::Fullbright {
                    assert_eq!(actual.lighting.rgb, tint);
                    assert_eq!(actual.lighting.exponent, 0);
                }
            }
            for hdr in [false, true] {
                let old = untinted.detail_prop_lighting(hdr).unwrap();
                let new = tinted.detail_prop_lighting(hdr).unwrap();
                assert_eq!(new.is_some(), old.is_some());
                if let Some(old) = old {
                    let new = new.unwrap();
                    assert_eq!(new.len(), old.len());
                    for (actual, old) in new.iter().zip(old) {
                        assert_eq!(actual.style, old.style);
                        assert_color(actual.lighting, old.lighting, tint);
                    }
                }
            }
            for index in 0..source_formats::bsp::LUMP_COUNT {
                assert_eq!(tinted.lump(index), untinted.lump(index));
            }
            assert_eq!(fixture.source.write(&Default::default()), before);
        }
    }
}

#[test]
fn invalid_tint_keeps_source_and_previous_output_and_rendercolor_remains_distinct() {
    let (mut fixture, indices) = family();
    let previous = compiled(&fixture, LightingMode::Fullbright, 0).to_bytes();
    let path = fixture.directory.path().join("previous-tinted.bsp");
    std::fs::write(&path, &previous).unwrap();
    for &index in &[indices[0], indices[4], indices[6]] {
        for invalid in [
            "", "1 2", "1 2 3 4", "-1 0 0", "256 0 0", "0.5 0 0", "NaN 0 0",
        ] {
            fixture.source.entities[index].properties.set(KEY, invalid);
            let before = fixture.source.write(&Default::default());
            let output = compiler::compile(&fixture.source, &fixture.game);
            assert!(output.bsp.is_none(), "{invalid}");
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|row| row.message.contains(KEY))
            );
            assert!(!output.write(&path).unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), previous);
            assert_eq!(fixture.source.write(&Default::default()), before);
        }
        fixture.source.entities[index]
            .properties
            .set(KEY, "128 64 0");
        fixture.source.entities[index]
            .properties
            .set("rendercolor", "128 64 0");
        let output = compiler::compile(&fixture.source, &fixture.game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|row| row.message.contains("rendercolor"))
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        fixture.source.entities[index]
            .properties
            .remove("rendercolor");
        fixture.source.entities[index].properties.remove(KEY);
    }
    let mut duplicate = fixture.source.clone();
    duplicate.entities[indices[1]]
        .properties
        .set(KEY, "128 64 0");
    duplicate.entities[indices[1]]
        .properties
        .0
        .push(("_DETAIL_LIGHTING_TINT".into(), "255 255 255".into()));
    let before = duplicate.write(&Default::default());
    let imported = Vmf::parse(&before).unwrap();
    let output = compiler::compile(&imported, &fixture.game);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("repeats _detail_lighting_tint"))
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert_eq!(imported.write(&Default::default()), before);
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let before = fixture.source.write(&Default::default());
    let output = compiler::compile_controlled(&fixture.source, &fixture.game, &control);
    assert!(output.bsp.is_none() && !output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert_eq!(fixture.source.write(&Default::default()), before);
}

#[test]
fn distinct_tints_and_named_receivers_follow_duplicate_records_through_reversed_leaf_sort() {
    let mut fixture = fixture();
    let mut document = Document::new(fixture.source);
    document
        .edit("split tint fixture", |tx| {
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
        "detail_prop",
        "-64 32 16",
        &[
            ("model", "models/props/test.mdl"),
            ("lightingorigin", "blue_sample"),
        ],
    );
    let third = entity(
        &mut fixture.source,
        "prop_detail",
        "64 32 16",
        &[
            ("model", "models/props/test.mdl"),
            ("lightingorigin", "red_sample"),
        ],
    );
    let fourth = entity(
        &mut fixture.source,
        "detail_prop",
        "64 32 16",
        &[
            ("model", "models/props/test.mdl"),
            ("lightingorigin", "blue_sample"),
        ],
    );
    let tints = [
        (
            fixture.source.entities[first].id,
            "128 255 64",
            [128, 255, 64],
        ),
        (
            fixture.source.entities[second].id,
            "64 128 255",
            [64, 128, 255],
        ),
        (
            fixture.source.entities[third].id,
            "255 64 128",
            [255, 64, 128],
        ),
        (
            fixture.source.entities[fourth].id,
            "64 255 128",
            [64, 255, 128],
        ),
    ];
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
    let white = compiled(&fixture, LightingMode::Fullbright, 0)
        .detail_props()
        .unwrap()
        .unwrap();
    assert_ne!(white.props[0].leaf, white.props[3].leaf);
    if white.props[0].origin[0] < 0.0 {
        fixture.source.entities.swap(first, third);
        fixture.source.entities.swap(second, fourth);
    }
    let emission_first = fixture.source.entities[first].origin().unwrap().x;
    for mode in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
        for &(id, _, _) in &tints {
            fixture
                .source
                .entities
                .iter_mut()
                .find(|entity| entity.id == id)
                .unwrap()
                .properties
                .remove(KEY);
        }
        let baseline = compiled(&fixture, mode, 0).detail_props().unwrap().unwrap();
        assert_ne!(f64::from(baseline.props[0].origin[0]), emission_first);
        for &(id, text, _) in &tints {
            fixture
                .source
                .entities
                .iter_mut()
                .find(|entity| entity.id == id)
                .unwrap()
                .properties
                .set(KEY, text);
        }
        let before = fixture.source.write(&Default::default());
        let bsp = compiled(&fixture, mode, 0);
        let actual = bsp.detail_props().unwrap().unwrap();
        let mut counts = [0; 2];
        for (&prop, &untinted) in actual.props.iter().zip(&baseline.props) {
            let side = usize::from(prop.origin[0] > 0.0);
            let tint = tints[side * 2 + counts[side]].2;
            let color = DVec3::from_array(untinted.lighting.linear());
            if counts[side] == 0 {
                assert!(color.x > color.z * 4.0, "{color}");
            } else {
                assert!(color.z > color.x * 4.0, "{color}");
            }
            assert_record(prop, untinted, tint);
            counts[side] += 1;
        }
        assert_eq!(counts, [2, 2]);
        assert_eq!(fixture.source.write(&Default::default()), before);
        let reopened = Vmf::parse(&before).unwrap();
        assert_eq!(
            compiler::compile_with_options(&reopened, &fixture.game, options(mode, 0))
                .bsp
                .unwrap()
                .to_bytes(),
            bsp.to_bytes()
        );
    }
}

#[test]
fn black_tint_retains_style_membership_and_native_half_scale_validation() {
    let mut fixture = fixture();
    atlas(&mut fixture);
    let index = sprite(&mut fixture, "0 0 16", 2, None);
    fixture.source.entities[index].properties.set(KEY, "0 0 0");
    fixture.source.entities[index]
        .properties
        .set("scale", "0.00000001");
    let unstyled = compiled(&fixture, LightingMode::Fullbright, 0);
    assert!(
        unstyled
            .detail_props()
            .unwrap()
            .unwrap()
            .props
            .iter()
            .any(|prop| prop.kind == 1 && prop.lighting.rgb == [0; 3])
    );
    entity(
        &mut fixture.source,
        "light",
        "0 0 180",
        &[
            ("_light", "255 255 255 120"),
            ("style", "1"),
            ("_constant_attn", "1"),
            ("_quadratic_attn", "0"),
        ],
    );
    let before = fixture.source.write(&Default::default());
    let path = fixture.directory.path().join("previous-half.bsp");
    std::fs::write(&path, unstyled.to_bytes()).unwrap();
    let output = compiler::compile_with_options(
        &fixture.source,
        &fixture.game,
        options(LightingMode::DirectLdr, 0),
    );
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("half precision")),
        "{:?}",
        output.report
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), unstyled.to_bytes());
    assert_eq!(fixture.source.write(&Default::default()), before);
}
