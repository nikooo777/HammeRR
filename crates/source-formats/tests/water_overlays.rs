use source_formats::bsp::{self, Bsp, Face, TexInfo, WaterOverlay, WaterOverlayData};

fn fixture() -> (Bsp, WaterOverlayData) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Face::default(); bsp::MAX_WATER_OVERLAY_FACES]);
    bsp.set_records(&[TexInfo::default()]);
    let mut record = WaterOverlay {
        id: 7,
        u: [1.0, -1.0],
        v: [0.0, 2.0],
        origin: [3.0, 5.0, 7.0],
        basis_normal: [0.0, 0.0, 1.0],
        ..WaterOverlay::default()
    }
    .with_face_count(bsp::MAX_WATER_OVERLAY_FACES)
    .unwrap()
    .with_render_order(3)
    .unwrap();
    for (index, face) in record.faces.iter_mut().enumerate() {
        *face = index as i32;
    }
    let data = WaterOverlayData {
        overlays: vec![record],
    };
    bsp.set_water_overlay_data(&data).unwrap();
    (bsp, data)
}

#[test]
fn native_water_layout_preserves_all_256_faces_packed_order_and_empty_inactive_slots() {
    assert_eq!(size_of::<WaterOverlay>(), 1120);
    assert!(Bsp::new().water_overlay_data().unwrap().is_none());
    let (mut bsp, data) = fixture();
    let bytes = &bsp.lumps[bsp::lump::WATEROVERLAYS].data;
    assert_eq!(&bytes[0x408..0x40c], &1.0f32.to_le_bytes());
    assert_eq!(&bytes[0x448..0x44c], &3.0f32.to_le_bytes());
    assert_eq!(&bytes[0x45c..0x460], &1.0f32.to_le_bytes());
    assert_eq!(
        Bsp::parse(&bsp.to_bytes())
            .unwrap()
            .water_overlay_data()
            .unwrap(),
        Some(data.clone())
    );
    assert_eq!(data.overlays[0].face_count(), 256);
    assert_eq!(data.overlays[0].render_order(), 3);
    assert!(data.overlays[0].with_face_count(257).is_err());
    assert!(data.overlays[0].with_render_order(4).is_err());
    let mut empty = data;
    empty.overlays[0] = empty.overlays[0].with_face_count(0).unwrap();
    empty.overlays[0].faces[255] = -99;
    bsp.set_water_overlay_data(&empty).unwrap();
    assert_eq!(bsp.water_overlay_data().unwrap(), Some(empty));
}

#[test]
fn water_projection_references_metadata_and_strides_reject_without_mutating_previous_bytes() {
    let (bsp, valid) = fixture();
    for case in 0..10 {
        let mut data = valid.clone();
        match case {
            0 => data.overlays[0].id = -1,
            1 => data.overlays.push(data.overlays[0]),
            2 => data.overlays[0].texinfo = -1,
            3 => data.overlays[0].texinfo = 1,
            4 => data.overlays[0].face_count_order = 257,
            5 => data.overlays[0].faces[255] = 256,
            6 => data.overlays[0].faces[255] = -1,
            7 => data.overlays[0].u[0] = f32::NAN,
            8 => data.overlays[0].uv_points[0][2] = f32::INFINITY,
            9 => data.overlays[0].basis_normal = [0.0; 3],
            _ => unreachable!(),
        }
        let mut copy = bsp.clone();
        assert!(copy.set_water_overlay_data(&data).is_err(), "case {case}");
        assert_eq!(copy.to_bytes(), bsp.to_bytes());
        copy.set_records(&data.overlays);
        assert!(copy.water_overlay_data().is_err(), "case {case}");
    }
    for slot in [
        bsp::lump::WATEROVERLAYS,
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
            assert!(copy.water_overlay_data().is_err());
            let previous = copy.to_bytes();
            if slot == bsp::lump::WATEROVERLAYS {
                copy.set_water_overlay_data(&valid).unwrap();
                assert_eq!(copy.water_overlay_data().unwrap(), Some(valid.clone()));
            } else {
                assert!(copy.set_water_overlay_data(&valid).is_err());
                assert_eq!(copy.to_bytes(), previous);
            }
        }
    }
    let mut compressed = bsp;
    compressed.lumps[bsp::lump::WATEROVERLAYS].data = b"LZMAinvalid".to_vec();
    assert!(compressed.water_overlay_data().is_err());
}

#[test]
fn water_record_allocation_budget_is_checked_before_decoding() {
    let (mut bsp, valid) = fixture();
    let records = (0..bsp::MAX_WATER_OVERLAYS)
        .map(|id| WaterOverlay {
            id: id as i32,
            ..valid.overlays[0]
        })
        .collect();
    let mut data = WaterOverlayData { overlays: records };
    bsp.set_water_overlay_data(&data).unwrap();
    assert_eq!(bsp.water_overlay_data().unwrap(), Some(data.clone()));
    let previous = bsp.to_bytes();
    data.overlays.push(WaterOverlay {
        id: i32::MAX,
        ..valid.overlays[0]
    });
    assert!(bsp.set_water_overlay_data(&data).is_err());
    assert_eq!(bsp.to_bytes(), previous);
    bsp.set_records(&data.overlays);
    assert!(bsp.water_overlay_data().is_err());
}
