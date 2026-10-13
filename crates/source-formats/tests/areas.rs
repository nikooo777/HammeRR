use source_formats::bsp::{self, Area, AreaData, AreaPortal, Bsp, Leaf, Plane};

fn fixture() -> (Bsp, AreaData) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[
        Plane {
            normal: [1.0, 0.0, 0.0],
            dist: 0.0,
            kind: 0,
        },
        Plane {
            normal: [-1.0, 0.0, 0.0],
            dist: 0.0,
            kind: 0,
        },
    ]);
    bsp.set_records(&[
        Leaf {
            area_flags: (0x53_u16 << 9 | 1) as i16,
            ..Default::default()
        },
        Leaf {
            area_flags: 2,
            ..Default::default()
        },
    ]);
    let data = AreaData {
        areas: vec![
            Area::default(),
            Area {
                portal_count: 1,
                first_portal: 1,
            },
            Area {
                portal_count: 1,
                first_portal: 2,
            },
        ],
        portals: vec![
            AreaPortal::default(),
            AreaPortal {
                portal_key: 7,
                other_area: 2,
                first_clip_vert: 0,
                clip_vert_count: 4,
                plane: 0,
            },
            AreaPortal {
                portal_key: 7,
                other_area: 1,
                first_clip_vert: 0,
                clip_vert_count: 4,
                plane: 1,
            },
        ],
        clip_vertices: vec![
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 64.0],
            [0.0, 64.0, 64.0],
            [0.0, 64.0, 0.0],
        ],
    };
    bsp.set_area_data(&data).unwrap();
    (bsp, data)
}

#[test]
fn checked_area_graph_round_trips_and_leaf_area_edits_preserve_flags() {
    let (bsp, data) = fixture();
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(parsed.area_data().unwrap(), Some(data));
    let leaf = parsed.records::<Leaf>().unwrap()[0];
    let changed = leaf.with_area(2).unwrap();
    assert_eq!(changed.area(), 2);
    assert_eq!(changed.flags(), 0x53);
    assert!(leaf.with_area(-1).is_err());
    assert!(leaf.with_area(256).is_err());
    assert_eq!(Bsp::new().area_data().unwrap(), None);
}

#[test]
fn malformed_ranges_keys_reciprocals_and_clip_vertices_reject_atomically() {
    let (mut bsp, data) = fixture();
    let original = bsp.clone();
    for case in 0..12 {
        let mut bad = data.clone();
        match case {
            0 => bad.areas[1].first_portal = -1,
            1 => bad.areas[1].portal_count = i32::MAX,
            2 => bad.areas[2].first_portal = 1,
            3 => bad.portals[1].other_area = 3,
            4 => bad.portals[1].other_area = 1,
            5 => bad.portals[1].plane = -1,
            6 => bad.portals[1].plane = 2,
            7 => bad.portals[1].clip_vert_count = 2,
            8 => bad.portals[1].first_clip_vert = u16::MAX,
            9 => bad.portals[2].portal_key = 8,
            10 => bad.portals[2].plane = 0,
            11 => bad.clip_vertices[0][0] = f32::NAN,
            _ => unreachable!(),
        }
        assert!(bsp.set_area_data(&bad).is_err(), "case {case}");
        assert_eq!(bsp, original);
        let mut raw = original.clone();
        raw.set_records(&bad.areas);
        raw.set_records(&bad.portals);
        raw.set_values(bsp::lump::CLIPPORTALVERTS, &bad.clip_vertices);
        assert!(raw.area_data().is_err(), "case {case}");
    }
}

#[test]
fn unsupported_metadata_counts_and_leaf_references_fail_before_publication() {
    let (original, data) = fixture();
    for slot in [
        bsp::lump::AREAS,
        bsp::lump::AREAPORTALS,
        bsp::lump::CLIPPORTALVERTS,
        bsp::lump::PLANES,
    ] {
        let mut bad = original.clone();
        bad.lumps[slot].version = 1;
        assert!(bad.area_data().is_err());
        bad.lumps[slot].version = 0;
        bad.lumps[slot].fourcc = [1, 0, 0, 0];
        assert!(bad.area_data().is_err());
        bad.lumps[slot].fourcc = [0; 4];
        bad.lumps[slot].data = b"LZMA".to_vec();
        assert!(bad.area_data().is_err());
    }
    let mut bad = original.clone();
    bad.lumps[bsp::lump::AREAS]
        .data
        .resize(257 * size_of::<Area>(), 0);
    assert!(bad.area_data().is_err());
    let leaf = bad.records::<Leaf>().unwrap()[0];
    bad = original.clone();
    bad.set_records(&[leaf.with_area(3).unwrap()]);
    let before = bad.clone();
    assert!(bad.set_area_data(&data).is_err());
    assert_eq!(bad, before);
    assert!(bad.area_data().is_err());
}
