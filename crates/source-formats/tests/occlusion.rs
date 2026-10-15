use source_formats::bsp::{
    Area, Bsp, MAX_OCCLUDERS, OCCLUDER_INACTIVE, Occluder, OccluderPolygon, Occlusion, Plane, lump,
};

fn fixture() -> (Bsp, Occlusion) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Plane { normal: [0.0, 0.0, 1.0], dist: 16.0, kind: 2 }]);
    bsp.set_values(lump::VERTEXES, &[[0.0f32, 0.0, 16.0], [0.0, 8.0, 16.0], [8.0, 0.0, 16.0]]);
    bsp.set_records(&[Area::default(); 2]);
    let data = Occlusion {
        occluders: vec![Occluder {
            flags: OCCLUDER_INACTIVE, first_polygon: 0, polygon_count: 1,
            mins: [0.0, 0.0, 16.0], maxs: [8.0, 8.0, 16.0], area: 1,
        }],
        polygons: vec![OccluderPolygon { first_vertex_index: 0, vertex_count: 3, plane: 0 }],
        vertex_indices: vec![0, 1, 2],
    };
    bsp.set_occlusion(&data).unwrap();
    (bsp, data)
}

#[test]
fn native_v2_layout_round_trips_with_global_references_and_inactive_flags() {
    let (bsp, data) = fixture();
    assert_eq!(size_of::<Occluder>(), 40);
    assert_eq!(size_of::<OccluderPolygon>(), 12);
    let native = &bsp.lumps[lump::OCCLUSION];
    assert_eq!(native.version, 2);
    assert_eq!(native.data.len(), 4 + 40 + 4 + 12 + 4 + 12);
    assert_eq!(&native.data[40..44], &1i32.to_le_bytes());
    assert_eq!(&native.data[44..48], &1i32.to_le_bytes());
    assert_eq!(&native.data[60..64], &3i32.to_le_bytes());
    let reread = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(reread.occlusion().unwrap(), Some(data.clone()));
    let mut written = reread.clone();
    written.set_occlusion(&data).unwrap();
    assert_eq!(written.to_bytes(), reread.to_bytes());
}

#[test]
fn version_one_uses_the_engines_area_one_upgrade() {
    let (mut bsp, data) = fixture();
    bsp.lumps[lump::OCCLUSION].version = 1;
    bsp.lumps[lump::OCCLUSION].data.drain(40..44);
    assert_eq!(bsp.occlusion().unwrap(), Some(data.clone()));
    bsp.set_occlusion(&data).unwrap();
    assert_eq!(bsp.lumps[lump::OCCLUSION].version, 2);
}

#[test]
fn truncation_counts_versions_compression_and_trailing_bytes_are_rejected() {
    let (bsp, _) = fixture();
    for length in 1..bsp.lumps[lump::OCCLUSION].data.len() {
        let mut invalid = bsp.clone();
        invalid.lumps[lump::OCCLUSION].data.truncate(length);
        assert!(invalid.occlusion().is_err(), "accepted {length}-byte payload");
    }
    for count in [-1, i32::MAX, MAX_OCCLUDERS as i32 + 1] {
        let mut invalid = bsp.clone();
        invalid.lumps[lump::OCCLUSION].data[..4].copy_from_slice(&count.to_le_bytes());
        assert!(invalid.occlusion().is_err());
    }
    for version in [0, 3] {
        let mut invalid = bsp.clone();
        invalid.lumps[lump::OCCLUSION].version = version;
        assert!(invalid.occlusion().is_err());
    }
    let mut invalid = bsp.clone();
    invalid.lumps[lump::OCCLUSION].fourcc = [1, 0, 0, 0];
    assert!(invalid.occlusion().is_err());
    invalid = bsp.clone();
    invalid.lumps[lump::OCCLUSION].data[..4].copy_from_slice(b"LZMA");
    assert!(invalid.occlusion().is_err());
    invalid = bsp.clone();
    invalid.lumps[lump::OCCLUSION].data.push(0);
    assert!(invalid.occlusion().is_err());
}

#[test]
fn invalid_cross_references_and_bounds_never_replace_the_existing_lump() {
    let (mut bsp, data) = fixture();
    let original = bsp.lumps[lump::OCCLUSION].clone();
    for case in 0..11 {
        let mut invalid = data.clone();
        match case {
            0 => invalid.occluders[0].first_polygon = -1,
            1 => invalid.occluders[0].polygon_count = i32::MAX,
            2 => invalid.occluders[0].area = -1,
            3 => invalid.occluders[0].area = 2,
            4 => invalid.occluders[0].mins[0] = f32::NAN,
            5 => invalid.occluders[0].maxs[0] = -1.0,
            6 => invalid.polygons[0].first_vertex_index = i32::MAX,
            7 => invalid.polygons[0].vertex_count = 2,
            8 => invalid.polygons[0].plane = 1,
            9 => invalid.vertex_indices[0] = -1,
            10 => invalid.vertex_indices[0] = 3,
            _ => unreachable!(),
        }
        assert!(bsp.set_occlusion(&invalid).is_err(), "case {case}");
        assert_eq!(bsp.lumps[lump::OCCLUSION], original);
    }
    bsp.lumps[lump::PLANES].version = 2;
    assert!(bsp.occlusion().is_err());
}

#[test]
fn empty_occlusion_keeps_the_native_three_count_layout() {
    let mut bsp = Bsp::new();
    assert_eq!(bsp.occlusion().unwrap(), None);
    bsp.set_occlusion(&Occlusion::default()).unwrap();
    assert_eq!(bsp.lumps[lump::OCCLUSION].data, vec![0; 12]);
    assert_eq!(bsp.occlusion().unwrap(), Some(Occlusion::default()));
}

#[test]
fn empty_native_records_preserve_the_unassigned_area_and_empty_bounds_sentinels() {
    let mut bsp = Bsp::new();
    let data = Occlusion {
        occluders: vec![Occluder {
            area: -1, mins: [f32::MAX; 3], maxs: [-f32::MAX; 3], ..Occluder::default()
        }],
        ..Occlusion::default()
    };
    bsp.set_occlusion(&data).unwrap();
    assert_eq!(bsp.occlusion().unwrap(), Some(data));
}

#[test]
fn nonfinite_referenced_geometry_is_rejected_before_native_emission() {
    let (mut bsp, data) = fixture();
    bsp.lumps[lump::VERTEXES].data[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(bsp.occlusion().is_err());
    assert!(bsp.set_occlusion(&data).is_err());
    let (mut bsp, data) = fixture();
    bsp.lumps[lump::PLANES].data[..4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    assert!(bsp.occlusion().is_err());
    assert!(bsp.set_occlusion(&data).is_err());
}

#[test]
fn installed_css_occlusion_fixtures_round_trip_including_empty_sentinels() {
    let directory = std::env::var_os("CSS_DIR").map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share/Steam/steamapps/common/Counter-Strike Source"))
    });
    let Some(directory) = directory else { return; };
    for name in ["cs_assault", "cs_compound", "de_aztec", "de_port"] {
        let path = directory.join(format!("cstrike/maps/{name}.bsp"));
        if !path.exists() { continue; }
        let mut bsp = Bsp::read(&path).unwrap();
        let original = bsp.lumps[lump::OCCLUSION].clone();
        let data = bsp.occlusion().unwrap().unwrap();
        bsp.set_occlusion(&data).unwrap();
        assert_eq!(bsp.lumps[lump::OCCLUSION], original, "{name}");
    }
}
