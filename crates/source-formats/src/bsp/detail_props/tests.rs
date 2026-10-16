use super::*;

#[test]
fn checked_detail_style_writer_is_atomic_and_preserves_native_five_byte_layout() {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Leaf::default(); 4]);
    let mut props = fixture();
    props.props[0].light_style_count = 2;
    let ldr = [
        DetailPropLighting {
            style: 1,
            lighting: RgbExp32 {
                rgb: [60, 0, 0],
                exponent: 0,
            },
        },
        DetailPropLighting {
            style: 32,
            ..Default::default()
        },
    ];
    let hdr = [
        DetailPropLighting {
            style: 1,
            ..Default::default()
        },
        DetailPropLighting {
            style: 32,
            lighting: RgbExp32 {
                rgb: [0, 0, 40],
                exponent: 0,
            },
        },
    ];
    bsp.set_detail_props_with_lighting(&props, &ldr, Some(&hdr))
        .unwrap();
    assert_eq!(bsp.game_lump(*b"dplt").unwrap().data.len(), 14);
    assert_eq!(bsp.game_lump(*b"dplh").unwrap().version, 0);
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(parsed.detail_props().unwrap().unwrap(), props);
    assert_eq!(parsed.detail_prop_lighting(false).unwrap().unwrap(), ldr);
    assert_eq!(parsed.detail_prop_lighting(true).unwrap().unwrap(), hdr);
    let before = bsp.to_bytes();
    let wrong = [hdr[1], hdr[0]];
    assert!(
        bsp.set_detail_props_with_lighting(&props, &ldr, Some(&wrong))
            .is_err()
    );
    assert!(
        bsp.set_detail_props_with_lighting(&props, &wrong, Some(&wrong))
            .is_err()
    );
    let mut bad = props.clone();
    bad.props[0].light_style_count = 64;
    assert!(
        bsp.set_detail_props_with_lighting(&bad, &ldr, Some(&hdr))
            .is_err()
    );
    bad = props.clone();
    bad.props[0].light_styles = 2;
    assert!(
        bsp.set_detail_props_with_lighting(&bad, &ldr, Some(&hdr))
            .is_err()
    );
    let too_many = vec![ldr[0]; MAX_DETAIL_PROP_LIGHTING / 2 + 1];
    for style in [0, 64] {
        let mut invalid = ldr;
        invalid[0].style = style;
        assert!(
            bsp.set_detail_props_with_lighting(&props, &invalid, None)
                .is_err()
        );
    }
    assert!(
        bsp.set_detail_props_with_lighting(&props, &too_many, Some(&too_many))
            .is_err()
    );
    assert_eq!(bsp.to_bytes(), before);
}

fn fixture() -> DetailProps {
    DetailProps {
        models: vec![DetailProps::model_record("models/props/test.mdl").unwrap()],
        sprites: vec![DetailSprite {
            upper_left: [-10.0, 20.0],
            lower_right: [10.0, 0.0],
            texture_upper_left: [0.1, 0.2],
            texture_lower_right: [0.9, 0.8],
        }],
        props: (0..=3)
            .map(|kind| DetailProp {
                kind,
                leaf: 1,
                origin: [1.125, 2.25, 3.375],
                angles: [15.0, 90.0, -20.0],
                padding: [1, 2, 3],
                padding2: [4, 5, 6],
                orientation: kind % 3,
                sway: 17,
                shape_size: 32,
                shape_angle: 45,
                ..Default::default()
            })
            .collect(),
    }
}

#[test]
fn native_v4_layout_preserves_models_sprites_shapes_padding_and_lighting_lumps() {
    assert_eq!(size_of::<DetailProp>(), 52);
    assert_eq!(size_of::<DetailSprite>(), 32);
    assert_eq!(size_of::<DetailPropLighting>(), 5);
    let mut props = fixture();
    props.models[0][127] = 0xa5;
    props.props[1].light_styles = 0;
    props.props[1].light_style_count = 1;
    let bytes = props.to_bytes(4).unwrap();
    assert_eq!(bytes.len(), 12 + 128 + 32 + 4 * 52);
    assert_eq!(DetailProps::parse(4, &bytes, 4).unwrap(), props);
    assert_eq!(
        DetailProps::parse(4, &bytes, 4)
            .unwrap()
            .to_bytes(4)
            .unwrap(),
        bytes
    );
    let mut bsp = Bsp::new();
    bsp.set_records(&[Leaf::default(); 4]);
    let style = DetailPropLighting {
        lighting: RgbExp32 {
            rgb: [32, 64, 128],
            exponent: -2,
        },
        style: 7,
    };
    let mut lighting = Vec::new();
    append(&mut lighting, &[style]);
    for id in [*b"dplt", *b"dplh"] {
        bsp.game_lumps.push(GameLump {
            id,
            flags: 0,
            version: 0,
            data: lighting.clone(),
        });
    }
    bsp.game_lumps.push(GameLump {
        id: *b"test",
        flags: 3,
        version: 77,
        data: vec![1, 2, 3],
    });
    bsp.set_detail_props(&props).unwrap();
    let bsp = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(bsp.detail_props().unwrap(), Some(props));
    assert_eq!(bsp.detail_prop_lighting(false).unwrap(), Some(vec![style]));
    assert_eq!(bsp.detail_prop_lighting(true).unwrap(), Some(vec![style]));
    assert_eq!(bsp.game_lump(*b"dplt").unwrap().data, lighting);
    assert_eq!(bsp.game_lump(*b"dplh").unwrap().data, lighting);
    assert_eq!(bsp.game_lump(*b"test").unwrap().data, [1, 2, 3]);
}

#[test]
fn ignored_model_scale_and_inactive_style_offsets_preserve_their_raw_bytes() {
    let mut props = fixture();
    props.props.truncate(1);
    props.props[0].scale = f32::from_bits(0x7fc01234);
    props.props[0].light_styles = u32::MAX;
    let bytes = props.to_bytes(4).unwrap();
    assert_eq!(
        DetailProps::parse(4, &bytes, 4)
            .unwrap()
            .to_bytes(4)
            .unwrap(),
        bytes
    );
    props.props[0].kind = 1;
    assert!(props.to_bytes(4).is_err());
}

#[test]
fn malformed_counts_references_versions_and_fields_never_mutate_the_bsp() {
    let props = fixture();
    let bytes = props.to_bytes(4).unwrap();
    for end in 0..bytes.len() {
        assert!(DetailProps::parse(4, &bytes[..end], 4).is_err());
    }
    for offset in [0, 132, 168] {
        for count in [-1_i32, i32::MAX] {
            let mut bad = bytes.clone();
            bad[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
            assert!(DetailProps::parse(4, &bad, 4).is_err());
        }
    }
    assert!(DetailProps::parse(3, &bytes, 4).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(DetailProps::parse(4, &trailing, 4).is_err());
    let mut bsp = Bsp::new();
    bsp.set_records(&[Leaf::default(); 4]);
    bsp.set_detail_props(&props).unwrap();
    let original = bsp.game_lumps.clone();
    for prop in [
        DetailProp {
            model: 1,
            ..props.props[0]
        },
        DetailProp {
            leaf: 4,
            ..props.props[0]
        },
        DetailProp {
            kind: 4,
            ..props.props[0]
        },
        DetailProp {
            orientation: 3,
            ..props.props[0]
        },
        DetailProp {
            origin: [f32::NAN, 0.0, 0.0],
            ..props.props[0]
        },
        DetailProp {
            light_styles: u32::MAX,
            light_style_count: 1,
            ..props.props[0]
        },
    ] {
        let mut invalid = props.clone();
        invalid.props[0] = prop;
        assert!(bsp.set_detail_props(&invalid).is_err());
        assert_eq!(bsp.game_lumps, original);
    }
    let mut unsorted = props;
    unsorted.props[0].leaf = 3;
    assert!(bsp.set_detail_props(&unsorted).is_err());
    assert_eq!(bsp.game_lumps, original);
}
