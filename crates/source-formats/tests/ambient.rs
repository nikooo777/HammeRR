use source_formats::bsp::{
    Bsp, Leaf, LeafAmbient, LeafAmbientIndex, LeafAmbientSample, LightingKind, RgbExp32, lump,
};

fn indexed() -> LeafAmbient {
    LeafAmbient::Indexed {
        indices: vec![
            LeafAmbientIndex {
                sample_count: 0,
                first_sample: 1,
            },
            LeafAmbientIndex {
                sample_count: 1,
                first_sample: 0,
            },
        ],
        samples: vec![LeafAmbientSample {
            cube: [RgbExp32 {
                rgb: [128, 64, 32],
                exponent: -8,
            }; 6],
            position: [0, 128, 255],
            padding: 17,
        }],
    }
}

fn map() -> Bsp {
    let mut bsp = Bsp::new();
    bsp.set_records(
        &[Leaf {
            mins: [-16; 3],
            maxs: [48; 3],
            ..Leaf::default()
        }; 2],
    );
    bsp
}

#[test]
fn indexed_ambient_preserves_hdr_ldr_aliases_positions_and_padding() {
    assert_eq!(size_of::<LeafAmbientIndex>(), 4);
    assert_eq!(size_of::<LeafAmbientSample>(), 28);
    let data = indexed();
    let mut bsp = map();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        bsp.set_leaf_ambient(kind, &data).unwrap();
        let copy = Bsp::parse(&bsp.to_bytes()).unwrap();
        assert_eq!(copy.leaf_ambient(kind).unwrap(), Some(data.clone()));
        assert_eq!(data.sample_leaf(0).unwrap(), Some(1));
        assert!(data.sample_leaf(2).is_err());
    }
    let LeafAmbient::Indexed { samples, .. } = data else {
        unreachable!()
    };
    let leaf = bsp.records::<Leaf>().unwrap()[0];
    assert_eq!(
        samples[0].world_position(&leaf).unwrap(),
        [-16.0, -16.0 + 64.0 * 128.0 / 255.0, 48.0]
    );
    assert_eq!(samples[0].cube[0].linear(), [0.5, 0.25, 0.125]);
    let mut reversed = leaf;
    reversed.maxs[0] = -17;
    assert!(samples[0].world_position(&reversed).is_err());
}

#[test]
fn malformed_ambient_ranges_versions_compression_and_aliases_fail_atomically() {
    let mut bsp = map();
    bsp.set_leaf_ambient(LightingKind::Ldr, &indexed()).unwrap();
    let before = bsp.to_bytes();
    for indices in [
        vec![
            LeafAmbientIndex {
                sample_count: 1,
                first_sample: 1
            };
            2
        ],
        vec![
            LeafAmbientIndex {
                sample_count: 0,
                first_sample: 2
            };
            2
        ],
        vec![
            LeafAmbientIndex {
                sample_count: 0,
                first_sample: 1,
            },
            LeafAmbientIndex::default(),
        ],
        vec![LeafAmbientIndex::default()],
    ] {
        let data = LeafAmbient::Indexed {
            indices,
            samples: vec![LeafAmbientSample::default()],
        };
        assert!(bsp.set_leaf_ambient(LightingKind::Ldr, &data).is_err());
        assert_eq!(bsp.to_bytes(), before);
    }
    for slot in [lump::LEAF_AMBIENT_INDEX, lump::LEAF_AMBIENT_LIGHTING] {
        let mut copy = bsp.clone();
        copy.lumps[slot].data.pop();
        assert!(copy.leaf_ambient(LightingKind::Ldr).is_err());
        let mut copy = bsp.clone();
        copy.lumps[slot].data = b"LZMA".to_vec();
        assert!(copy.leaf_ambient(LightingKind::Ldr).is_err());
        let mut copy = bsp.clone();
        copy.lumps[slot].version = 3;
        assert!(copy.leaf_ambient(LightingKind::Ldr).is_err());
    }
}

#[test]
fn legacy_embedded_and_dense_cubes_preserve_water_and_layout_versions() {
    let cube = [RgbExp32 {
        rgb: [64, 32, 16],
        exponent: -7,
    }; 6];
    let mut bsp = Bsp::new();
    bsp.lumps[lump::LEAFS].data = vec![0; 56];
    bsp.lumps[lump::LEAFS].data[28..30].copy_from_slice(&(-1_i16).to_le_bytes());
    bsp.lumps[lump::LEAFS].data[30..54].copy_from_slice(bytemuck::bytes_of(&cube));
    bsp.lumps[lump::LEAFS].data[54..56].copy_from_slice(&[13, 29]);
    let original = bsp.to_bytes();
    let data = bsp.leaf_ambient(LightingKind::Ldr).unwrap().unwrap();
    assert_eq!(data, LeafAmbient::Embedded(vec![cube]));
    bsp.set_leaf_ambient(LightingKind::Ldr, &data).unwrap();
    assert_eq!(bsp.to_bytes(), original);
    assert_eq!(bsp.records::<Leaf>().unwrap()[0].water_data, -1);
    assert!(bsp.leaf_ambient(LightingKind::Hdr).unwrap().is_none());
    for version in [0, 48] {
        let mut bsp = map();
        bsp.lumps[lump::LEAF_AMBIENT_INDEX_HDR].version = 1;
        let data = LeafAmbient::Dense {
            cubes: vec![cube; 2],
            version,
        };
        bsp.set_leaf_ambient(LightingKind::Hdr, &data).unwrap();
        assert_eq!(bsp.leaf_ambient(LightingKind::Hdr).unwrap(), Some(data));
        assert_eq!(bsp.lumps[lump::LEAF_AMBIENT_LIGHTING_HDR].version, version);
        assert_eq!(bsp.lumps[lump::LEAF_AMBIENT_INDEX_HDR].version, 1);
    }
}
