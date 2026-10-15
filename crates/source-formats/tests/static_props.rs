use source_formats::bsp::{Bsp, Leaf, StaticProp, StaticProps};
use source_formats::mdl::ModelHeader;

fn fixture(version: u16) -> StaticProps {
    StaticProps {
        version,
        models: vec![StaticProps::model_record("models/props/test.mdl").unwrap()],
        leaves: vec![1, 3],
        props: vec![StaticProp {
            origin: [1.125, 2.25, 3.375],
            angles: [12.0, 90.0, -15.0],
            leaf_count: 2,
            solid: 6,
            flags: if version == 10 { 0x123 } else { 0x23 },
            padding: if version == 10 { 0xa7 } else { 0 },
            ..Default::default()
        }],
    }
}

#[test]
fn native_static_prop_versions_preserve_records_names_and_padding() {
    for version in [4, 5, 6, 10] {
        let mut props = fixture(version);
        props.models[0][127] = 0xa5;
        let bytes = props.to_bytes(4).unwrap();
        assert_eq!(StaticProps::parse(version, &bytes, 4).unwrap(), props);
        assert_eq!(
            StaticProps::parse(version, &bytes, 4)
                .unwrap()
                .to_bytes(4)
                .unwrap(),
            bytes
        );
        let mut bsp = Bsp::new();
        bsp.set_records(&[Leaf::default(); 4]);
        bsp.set_static_props(&props).unwrap();
        let bsp = Bsp::parse(&bsp.to_bytes()).unwrap();
        assert_eq!(bsp.static_props().unwrap(), Some(props));
    }
}

#[test]
fn inactive_lighting_origin_bytes_remain_opaque_but_active_values_must_be_finite() {
    let mut props = fixture(6);
    props.props[0].flags &= !2;
    props.props[0].lighting_origin = [f32::from_bits(0x7fc01234), f32::INFINITY, 0.0];
    let bytes = props.to_bytes(4).unwrap();
    assert_eq!(
        StaticProps::parse(6, &bytes, 4)
            .unwrap()
            .to_bytes(4)
            .unwrap(),
        bytes
    );
    props.props[0].flags |= 2;
    assert!(props.to_bytes(4).is_err());
}

#[test]
fn malformed_static_prop_counts_references_and_values_reject_atomically() {
    let props = fixture(6);
    let bytes = props.to_bytes(4).unwrap();
    for end in 0..bytes.len() {
        assert!(StaticProps::parse(6, &bytes[..end], 4).is_err());
    }
    for offset in [0, 132, 140] {
        for value in [-1_i32, i32::MAX] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(StaticProps::parse(6, &invalid, 4).is_err());
        }
    }
    let mut bsp = Bsp::new();
    bsp.set_records(&[Leaf::default(); 4]);
    bsp.set_static_props(&props).unwrap();
    let old = bsp.game_lumps.clone();
    for name in [b"".as_slice(), b"\xff", b"line\nbreak", &[b'x'; 128]] {
        let mut invalid = props.clone();
        invalid.models[0].fill(0);
        invalid.models[0][..name.len()].copy_from_slice(name);
        assert!(bsp.set_static_props(&invalid).is_err());
        assert_eq!(bsp.game_lumps, old);
    }
    for invalid in [
        StaticProps {
            leaves: vec![4, 3],
            ..props.clone()
        },
        StaticProps {
            version: 7,
            ..props.clone()
        },
        StaticProps {
            props: vec![StaticProp {
                model: 1,
                ..props.props[0].clone()
            }],
            ..props.clone()
        },
        StaticProps {
            props: vec![StaticProp {
                first_leaf: u16::MAX,
                ..props.props[0].clone()
            }],
            ..props.clone()
        },
        StaticProps {
            props: vec![StaticProp {
                origin: [f32::NAN; 3],
                ..props.props[0].clone()
            }],
            ..props.clone()
        },
    ] {
        assert!(bsp.set_static_props(&invalid).is_err());
        assert_eq!(bsp.game_lumps, old);
    }
    bsp.game_lumps[0].flags = 1;
    assert!(bsp.static_props().is_err());
    bsp.game_lumps[0].flags = 0;
    bsp.game_lumps.push(bsp.game_lumps[0].clone());
    assert!(bsp.static_props().is_err());
}

#[test]
fn model_header_checks_version_length_bounds_and_static_prop_flag() {
    let mut bytes = vec![0; 240];
    bytes[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    assert!(ModelHeader::parse(&bytes).unwrap().is_static_prop());
    for (offset, value) in [
        (4, 49_i32),
        (76, 241),
        (224, -1),
        (128, f32::NAN.to_bits() as i32),
        (140, (-1.0_f32).to_bits() as i32),
    ] {
        let mut invalid = bytes.clone();
        invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(ModelHeader::parse(&invalid).is_err());
    }
}

#[test]
fn installed_stock_model_headers_expose_static_bounds_and_skins() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let gameinfo = std::path::PathBuf::from(home)
        .join(".local/share/Steam/steamapps/common/Counter-Strike Source/cstrike/gameinfo.txt");
    if !gameinfo.is_file() {
        return;
    }
    let fs = source_formats::gamefs::GameFs::from_gameinfo(&gameinfo).unwrap();
    for name in [
        "models/props_junk/wood_crate001a.mdl",
        "models/props/cs_office/file_cabinet1.mdl",
        "models/props_c17/oildrum001.mdl",
    ] {
        let data = fs.read(name).unwrap().unwrap();
        let header = ModelHeader::parse(&data).unwrap();
        eprintln!("{name}: {header:?}");
        assert!(header.is_static_prop());
        let phy = fs
            .read(&format!("{}.phy", &name[..name.len() - 4]))
            .unwrap()
            .unwrap();
        assert!(source_formats::mdl::validate_physics(&phy, header.checksum).unwrap() > 0);
    }
}

#[test]
fn model_physics_rejects_bad_checksums_lengths_counts_and_keys() {
    let mut bytes = [16_i32, 0, 1, 17].map(i32::to_le_bytes).concat();
    bytes.extend_from_slice(&32_i32.to_le_bytes());
    bytes.extend_from_slice(b"VPHY");
    bytes.extend_from_slice(&0x100_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 20]);
    bytes.extend_from_slice(b"solid {}\0");
    assert_eq!(
        source_formats::mdl::validate_physics(&bytes, 17).unwrap(),
        1
    );
    assert!(source_formats::mdl::validate_physics(&bytes, 18).is_err());
    for length in 0..bytes.len() {
        assert!(source_formats::mdl::validate_physics(&bytes[..length], 17).is_err());
    }
    for (offset, value) in [
        (0, -1_i32),
        (8, -1),
        (8, 1025),
        (16, -1),
        (16, i32::MAX),
        (28, 5),
    ] {
        let mut invalid = bytes.clone();
        invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(source_formats::mdl::validate_physics(&invalid, 17).is_err());
    }
    let key_start = bytes.len() - b"solid {}\0".len();
    for keys in [b"".as_slice(), b"unterminated", b"one\0two\0", b"one\0\0"] {
        let mut invalid = bytes[..key_start].to_vec();
        invalid.extend_from_slice(keys);
        assert!(source_formats::mdl::validate_physics(&invalid, 17).is_err());
    }
    for keys in [b"\0".as_slice(), b"\xff\0"] {
        let mut opaque = bytes[..key_start].to_vec();
        opaque.extend_from_slice(keys);
        assert_eq!(
            source_formats::mdl::validate_physics(&opaque, 17).unwrap(),
            1
        );
    }
}
