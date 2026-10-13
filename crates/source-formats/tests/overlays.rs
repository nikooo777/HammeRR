use source_formats::bsp::{self, Bsp, Face, Overlay, OverlayData, OverlayFade, TexInfo};

fn fixture() -> (Bsp, OverlayData) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Face::default(); 2]);
    bsp.set_records(&[TexInfo::default()]);
    let mut overlay = Overlay {
        id: 9,
        basis_normal: [0.0, 0.0, 1.0],
        u: [0.0, 1.0],
        v: [0.0, 1.0],
        uv_points: [
            [-16.0, -16.0, 1.0],
            [-16.0, 16.0, 0.0],
            [16.0, 16.0, 0.0],
            [16.0, -16.0, 1.0],
        ],
        ..Default::default()
    }
    .with_face_count(1)
    .unwrap()
    .with_render_order(3)
    .unwrap();
    overlay.faces[0] = 1;
    overlay.faces[63] = -77;
    let data = OverlayData {
        overlays: vec![overlay],
        fades: vec![OverlayFade {
            min_distance_squared: -1.0,
            max_distance_squared: 0.0,
        }],
    };
    bsp.set_overlay_data(&data).unwrap();
    (bsp, data)
}

#[test]
fn overlay_records_preserve_packed_fields_empty_faces_unused_slots_and_fade_sentinels() {
    assert_eq!(size_of::<Overlay>(), 352);
    assert_eq!(size_of::<OverlayFade>(), 8);
    assert!(Bsp::new().overlay_data().unwrap().is_none());
    let (mut bsp, mut data) = fixture();
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(parsed.overlay_data().unwrap(), Some(data.clone()));
    let overlay = data.overlays[0];
    assert_eq!(overlay.face_count(), 1);
    assert_eq!(overlay.render_order(), 3);
    let changed = overlay.with_face_count(64).unwrap();
    assert_eq!(changed.render_order(), 3);
    assert_eq!(changed.face_count(), 64);
    assert_eq!(changed.with_render_order(1).unwrap().face_count(), 64);
    assert!(overlay.with_face_count(65).is_err());
    assert!(overlay.with_render_order(4).is_err());
    data.overlays[0] = overlay.with_face_count(0).unwrap();
    data.fades.clear();
    bsp.set_overlay_data(&data).unwrap();
    assert_eq!(bsp.overlay_data().unwrap(), Some(data));
}

#[test]
fn modern_overlay_capacity_round_trips_a_live_final_record_above_the_old_sdk_limit() {
    let (mut bsp, good) = fixture();
    let mut overlays = (0..bsp::MAX_OVERLAYS)
        .map(|id| Overlay {
            id: id as i32,
            ..good.overlays[0].with_face_count(0).unwrap()
        })
        .collect::<Vec<_>>();
    overlays.last_mut().unwrap().face_count_order = 1;
    let data = OverlayData {
        overlays,
        fades: vec![good.fades[0]; bsp::MAX_OVERLAYS],
    };
    bsp.set_overlay_data(&data).unwrap();
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    let decoded = parsed.overlay_data().unwrap().unwrap();
    assert!(decoded.overlays.len() > 512);
    assert_eq!(decoded, data);
}

#[test]
fn malformed_overlay_data_and_reference_arrays_reject_atomically() {
    let (bsp, good) = fixture();
    let original = bsp.to_bytes();
    for case in 0..13 {
        let mut data = good.clone();
        match case {
            0 => data.overlays[0].id = -1,
            1 => {
                data.overlays.push(data.overlays[0]);
                data.fades.clear();
            }
            2 => data.overlays[0].texinfo = -1,
            3 => data.overlays[0].texinfo = 1,
            4 => data.overlays[0].face_count_order |= 65,
            5 => data.overlays[0].faces[0] = -1,
            6 => data.overlays[0].faces[0] = 2,
            7 => data.overlays[0].u[1] = f32::NAN,
            8 => data.overlays[0].uv_points[3][2] = f32::INFINITY,
            9 => data.overlays[0].basis_normal = [0.0; 3],
            10 => data.overlays[0].basis_normal = [0.0, 0.0, 2.0],
            11 => data.fades.push(data.fades[0]),
            12 => data.fades[0].max_distance_squared = f32::NAN,
            _ => unreachable!(),
        }
        let mut copy = bsp.clone();
        assert!(copy.set_overlay_data(&data).is_err(), "case {case}");
        assert_eq!(copy.to_bytes(), original);
        copy.set_records(&data.overlays);
        copy.set_records(&data.fades);
        assert!(copy.overlay_data().is_err(), "case {case}");
    }
    let oversized = OverlayData {
        overlays: vec![good.overlays[0]; bsp::MAX_OVERLAYS + 1],
        fades: Vec::new(),
    };
    let mut copy = bsp.clone();
    assert!(copy.set_overlay_data(&oversized).is_err());
    assert_eq!(copy.to_bytes(), original);
    copy.set_records(&oversized.overlays);
    assert!(copy.overlay_data().is_err());
    for slot in [bsp::lump::FACES, bsp::lump::TEXINFO] {
        let mut copy = bsp.clone();
        copy.lumps[slot].data.push(0);
        let original = copy.to_bytes();
        assert!(copy.set_overlay_data(&good).is_err());
        assert_eq!(copy.to_bytes(), original);
        assert!(copy.overlay_data().is_err());
    }
}

#[test]
fn unsupported_overlay_metadata_strides_and_bounded_counts_reject_before_decoding() {
    let (bsp, _) = fixture();
    for slot in [
        bsp::lump::OVERLAYS,
        bsp::lump::OVERLAY_FADES,
        bsp::lump::FACES,
        bsp::lump::TEXINFO,
    ] {
        for case in 0..3 {
            let mut copy = bsp.clone();
            match case {
                0 => copy.lumps[slot].version = 99,
                1 => copy.lumps[slot].fourcc = [1, 0, 0, 0],
                2 => copy.lumps[slot].data.push(0),
                _ => unreachable!(),
            }
            assert!(copy.overlay_data().is_err(), "slot {slot}, case {case}");
        }
    }
    let mut empty = Bsp::new();
    empty.lumps[bsp::lump::OVERLAYS].version = 1;
    assert!(empty.overlay_data().is_err());
    let mut compressed = bsp.clone();
    compressed.lumps[bsp::lump::OVERLAYS].data = b"LZMAinvalid".to_vec();
    assert!(compressed.overlay_data().is_err());
    let mut copy = bsp.clone();
    copy.lumps[bsp::lump::FACES].data = vec![0; size_of::<Face>() * 65_537];
    assert!(copy.overlay_data().is_err());
    copy = bsp.clone();
    copy.lumps[bsp::lump::TEXINFO].data = vec![0; size_of::<TexInfo>() * 32_769];
    assert!(copy.overlay_data().is_err());
}
