use source_formats::bsp::{
    Bsp, Face, Lighting, LightingKind, MAX_LIGHTING_BYTES, MAX_LIGHTING_FACES, MAX_WORLD_LIGHTS,
    RgbExp32, TexInfo, WorldLight, lump, surf, world_light_kind,
};

fn color(value: u8) -> RgbExp32 {
    RgbExp32 {
        rgb: [value, value + 1, value + 2],
        exponent: -1,
    }
}

fn face() -> Face {
    Face {
        styles: [0, 32, 255, 255],
        light_offset: 8,
        lightmap_size: [1, 2],
        ..Face::default()
    }
}

#[test]
fn face_channels_keep_independent_styles_offsets_and_bump_maps() {
    let mut bsp = Bsp::new();
    let ldr_face = Face {
        styles: [0, 255, 255, 255],
        light_offset: 4,
        ..face()
    };
    let hdr_face = face();
    let ldr_colors: Vec<_> = (10..35).map(color).collect();
    let hdr_colors: Vec<_> = (100..150).map(color).collect();
    bsp.set_lighting_faces(LightingKind::Ldr, &[ldr_face])
        .unwrap();
    assert!(bsp.lighting_faces(LightingKind::Hdr).unwrap().is_empty());
    let ldr_lump = bsp.lump(lump::FACES).clone();
    bsp.set_lighting_faces(LightingKind::Hdr, &[hdr_face])
        .unwrap();
    assert_eq!(bsp.lump(lump::FACES), &ldr_lump);
    bsp.set_lighting(LightingKind::Ldr, &ldr_colors).unwrap();
    bsp.set_lighting(LightingKind::Hdr, &hdr_colors).unwrap();
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(parsed.lump(lump::FACES).version, 0);
    assert_eq!(parsed.lump(lump::FACES_HDR).version, 1);
    assert_eq!(
        &parsed.lump(lump::FACES).data[16..24],
        &[0, 255, 255, 255, 4, 0, 0, 0]
    );
    assert_eq!(
        &parsed.lump(lump::FACES_HDR).data[16..24],
        &[0, 32, 255, 255, 8, 0, 0, 0]
    );
    let info = TexInfo {
        flags: surf::BUMPLIGHT,
        ..TexInfo::default()
    };
    for (kind, expected_face, colors, styles, first) in [
        (
            LightingKind::Ldr,
            ldr_face,
            ldr_colors.as_slice(),
            &[0u8][..],
            1,
        ),
        (
            LightingKind::Hdr,
            hdr_face,
            hdr_colors.as_slice(),
            &[0u8, 32][..],
            2,
        ),
    ] {
        let faces = parsed.lighting_faces(kind).unwrap();
        assert_eq!(faces, [expected_face]);
        let lighting = parsed.lighting(kind).unwrap().unwrap();
        let view = lighting.face(&faces[0], &info).unwrap().unwrap();
        assert_eq!(view.styles(), styles);
        assert_eq!(view.dimensions(), [2, 3]);
        assert_eq!(view.maps(), 4);
        for style in 0..styles.len() {
            assert_eq!(view.average(style), Some(&colors[first - style - 1]));
            for map in 0..4 {
                let offset = first + (style * 4 + map) * 6;
                assert_eq!(view.samples(style, map), Some(&colors[offset..offset + 6]));
            }
        }
    }
    assert!(
        parsed
            .lighting(LightingKind::Ldr)
            .unwrap()
            .unwrap()
            .face(&hdr_face, &info)
            .is_err()
    );
}

#[test]
fn face_writes_preserve_legacy_ldr_bytes_and_other_channel_payloads() {
    let faces = [Face {
        plane: 17,
        side: 1,
        on_node: 1,
        first_edge: 205,
        edge_count: 4,
        texinfo: 8,
        dispinfo: -1,
        fog_volume: -1,
        area: 17.25,
        lightmap_mins: [-11, 23],
        original_face: 13,
        primitive_count: 0x8001,
        first_primitive: 31,
        smoothing_groups: 0x8100_0020,
        ..face()
    }];
    let mut legacy = Bsp::new();
    legacy.set_entities("{\n\"classname\" \"worldspawn\"\n}\n");
    legacy
        .set_lighting(LightingKind::Ldr, &(0..50).map(color).collect::<Vec<_>>())
        .unwrap();
    legacy.set_records(&faces);
    legacy.lumps[lump::FACES_HDR].data = b"opaque HDR payload".to_vec();
    legacy.lumps[lump::FACES_HDR].version = 7;
    let bytes = legacy.to_bytes();
    let mut typed = legacy.clone();
    typed.set_lighting_faces(LightingKind::Ldr, &faces).unwrap();
    assert_eq!(typed.to_bytes(), bytes);
    assert_eq!(typed.lighting_faces(LightingKind::Ldr).unwrap(), faces);
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let slot = if kind == LightingKind::Ldr {
            lump::FACES
        } else {
            lump::FACES_HDR
        };
        for version in [0, 1] {
            let mut fixture = Bsp::new();
            fixture.lumps[slot].data = bytemuck::cast_slice(&faces).to_vec();
            fixture.lumps[slot].version = version;
            let before = fixture.to_bytes();
            let parsed = Bsp::parse(&before).unwrap();
            assert_eq!(parsed.lighting_faces(kind).unwrap(), faces);
            assert_eq!(parsed.to_bytes(), before);
        }
    }
}

#[test]
fn face_channel_metadata_framing_and_allocation_bounds_are_checked() {
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let slot = if kind == LightingKind::Ldr {
            lump::FACES
        } else {
            lump::FACES_HDR
        };
        let mut bsp = Bsp::new();
        bsp.set_lighting_faces(kind, &[face()]).unwrap();
        for version in [-1, 2, i32::MAX] {
            let mut bad = bsp.clone();
            bad.lumps[slot].version = version;
            assert!(bad.lighting_faces(kind).is_err());
            bad.lumps[slot].data.clear();
            assert!(bad.lighting_faces(kind).is_err());
        }
        for length in [55, 57] {
            let mut bad = bsp.clone();
            bad.lumps[slot].data.resize(length, 0);
            assert!(bad.lighting_faces(kind).is_err());
        }
        let mut compressed = bsp.clone();
        compressed.lumps[slot].data[..4].copy_from_slice(b"LZMA");
        assert!(compressed.lighting_faces(kind).is_err());
        let mut fourcc = bsp.clone();
        fourcc.lumps[slot].fourcc = 56u32.to_le_bytes();
        assert!(fourcc.lighting_faces(kind).is_err());
        fourcc.lumps[slot].data.clear();
        assert!(fourcc.lighting_faces(kind).is_err());
        let at_limit = vec![face(); MAX_LIGHTING_FACES];
        bsp.set_lighting_faces(kind, &at_limit).unwrap();
        assert_eq!(bsp.lighting_faces(kind).unwrap().len(), MAX_LIGHTING_FACES);
        let previous = bsp.to_bytes();
        let over_limit = vec![face(); MAX_LIGHTING_FACES + 1];
        assert!(bsp.set_lighting_faces(kind, &over_limit).is_err());
        assert_eq!(bsp.to_bytes(), previous);
        bsp.set_values(slot, &over_limit);
        assert!(bsp.lighting_faces(kind).is_err());
    }
}

#[test]
fn native_records_keep_signed_exponents_and_world_light_fields() {
    assert_eq!(size_of::<RgbExp32>(), 4);
    assert_eq!(size_of::<WorldLight>(), 88);
    let mut bsp = Bsp::new();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let colors = [
            color(10),
            RgbExp32 {
                rgb: [128, 64, 32],
                exponent: 2,
            },
        ];
        bsp.set_lighting(kind, &colors).unwrap();
        let light = WorldLight {
            origin: [16.0, -32.0, 48.0],
            intensity: [128.0, 64.0, 32.0],
            cluster: 12,
            kind: world_light_kind::POINT,
            constant_attenuation: 1.0,
            quadratic_attenuation: 0.01,
            texinfo: -1,
            owner: -1,
            ..WorldLight::default()
        };
        bsp.set_world_lights(kind, &[light]).unwrap();
        let roundtrip = Bsp::parse(&bsp.to_bytes()).unwrap();
        let lighting = roundtrip.lighting(kind).unwrap().unwrap();
        assert_eq!(lighting.samples(), colors);
        assert_eq!(lighting.samples()[0].linear(), [5.0, 5.5, 6.0]);
        assert_eq!(lighting.samples()[1].linear(), [512.0, 256.0, 128.0]);
        assert_eq!(roundtrip.world_lights(kind).unwrap(), [light]);
    }
    let bytes = bsp.lump(lump::LIGHTING).data.as_slice();
    assert_eq!(&bytes[..4], &[10, 11, 12, 255]);
}

#[test]
fn styles_have_reversed_average_colors_and_independent_bump_maps() {
    let colors: Vec<_> = (0..50).map(color).collect();
    let lighting = Lighting::parse(bytemuck::cast_slice(&colors)).unwrap();
    let info = TexInfo {
        flags: surf::BUMPLIGHT,
        ..TexInfo::default()
    };
    let face = face();
    let layout = lighting.face(&face, &info).unwrap().unwrap();
    assert_eq!(layout.styles(), [0, 32]);
    assert_eq!(layout.dimensions(), [2, 3]);
    assert_eq!(layout.maps(), 4);
    assert_eq!(layout.average(0), Some(&colors[1]));
    assert_eq!(layout.average(1), Some(&colors[0]));
    for style in 0..2 {
        for map in 0..4 {
            let start = 2 + (style * 4 + map) * 6;
            assert_eq!(layout.samples(style, map), Some(&colors[start..start + 6]));
        }
    }
    assert!(layout.average(2).is_none());
    assert!(layout.average(usize::MAX).is_none());
    assert!(layout.samples(2, 0).is_none());
    assert!(layout.samples(0, 4).is_none());
}

#[test]
fn malformed_face_ranges_fail_before_sample_access() {
    let colors = vec![color(1); 50];
    let lighting = Lighting::parse(bytemuck::cast_slice(&colors)).unwrap();
    let info = TexInfo::default();
    for bad in [
        Face {
            light_offset: -2,
            ..face()
        },
        Face {
            light_offset: 7,
            ..face()
        },
        Face {
            light_offset: 4,
            ..face()
        },
        Face {
            light_offset: 200,
            ..face()
        },
        Face {
            light_offset: i32::MAX - 3,
            ..face()
        },
        Face {
            styles: [255; 4],
            ..face()
        },
        Face {
            styles: [0, 255, 32, 255],
            ..face()
        },
        Face {
            lightmap_size: [-1, 2],
            ..face()
        },
        Face {
            lightmap_size: [i32::MAX; 2],
            ..face()
        },
    ] {
        assert!(lighting.face(&bad, &info).is_err(), "{bad:?}");
    }
    let short = Lighting::parse(&[0; 56]).unwrap();
    assert!(
        short
            .face(
                &face(),
                &TexInfo {
                    flags: surf::BUMPLIGHT,
                    ..info
                }
            )
            .is_err()
    );
    let unlit = Face {
        light_offset: -1,
        styles: [255; 4],
        ..face()
    };
    assert!(lighting.face(&unlit, &info).unwrap().is_none());
}

#[test]
fn lighting_versions_compression_sizes_and_writes_are_checked() {
    let mut bsp = Bsp::new();
    assert!(bsp.lighting(LightingKind::Ldr).unwrap().is_none());
    bsp.set_lighting(LightingKind::Ldr, &[color(1)]).unwrap();
    bsp.lumps[lump::LIGHTING].version = 0;
    assert!(bsp.lighting(LightingKind::Ldr).is_err());
    bsp.lumps[lump::LIGHTING].version = 1;
    bsp.lumps[lump::LIGHTING].data = b"LZMA".to_vec();
    assert!(bsp.lighting(LightingKind::Ldr).is_err());
    assert!(Lighting::parse(&[0; 3]).is_err());
    assert!(Lighting::parse(&vec![0; MAX_LIGHTING_BYTES + 4]).is_err());
    bsp.set_lighting(LightingKind::Ldr, &[color(1)]).unwrap();
    let previous = bsp.lump(lump::LIGHTING).clone();
    assert!(
        bsp.set_lighting(
            LightingKind::Ldr,
            &vec![color(1); MAX_LIGHTING_BYTES / 4 + 1]
        )
        .is_err()
    );
    assert_eq!(bsp.lump(lump::LIGHTING), &previous);
    let lights = vec![WorldLight::default(); MAX_WORLD_LIGHTS + 1];
    assert!(bsp.set_world_lights(LightingKind::Hdr, &lights).is_err());
    bsp.set_values(lump::WORLDLIGHTS, &lights);
    assert!(bsp.world_lights(LightingKind::Ldr).is_err());
    bsp.set_world_lights(LightingKind::Ldr, &[]).unwrap();
    bsp.lumps[lump::WORLDLIGHTS].data = vec![0; 87];
    assert!(bsp.world_lights(LightingKind::Ldr).is_err());
}
