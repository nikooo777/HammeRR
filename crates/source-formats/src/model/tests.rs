use super::*;

pub(crate) fn put_int(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_string(data: &mut [u8], offset: usize, value: &str) {
    data[offset..offset + value.len()].copy_from_slice(value.as_bytes());
}

pub(crate) fn fixture() -> [Vec<u8>; 3] {
    let mut mdl = vec![0; 1024];
    mdl[..4].copy_from_slice(b"IDST");
    for (offset, value) in [
        (4, 48),
        (8, 7),
        (76, 1024),
        (152, 16),
        (204, 2),
        (208, 256),
        (212, 1),
        (216, 384),
        (220, 1),
        (224, 2),
        (228, 388),
        (232, 1),
        (236, 400),
        (256, 464),
        (320, 416),
        (384, 752),
        (404, 1),
        (408, 1),
        (412, 16),
        (488, 1),
        (492, 160),
        (496, 4),
        (500, 0),
        (576, 0),
        (580, -160),
        (584, 4),
        (588, 0),
    ] {
        put_int(&mut mdl, offset, value);
    }
    for offset in [116, 140] {
        put_float(&mut mdl, offset, 1.0);
        put_float(&mut mdl, offset + 4, 1.0);
    }
    mdl[390..392].copy_from_slice(&1u16.to_le_bytes());
    put_string(&mut mdl, 720, "one");
    put_string(&mut mdl, 736, "two");
    put_string(&mut mdl, 752, "models/test/");
    let mut vvd = vec![0; 256];
    vvd[..4].copy_from_slice(b"IDSV");
    for (offset, value) in [(4, 4), (8, 7), (12, 1), (16, 4), (56, 64)] {
        put_int(&mut vvd, offset, value);
    }
    for (index, point) in [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ]
    .into_iter()
    .enumerate()
    {
        let offset = 64 + index * 48;
        for (axis, value) in point.into_iter().enumerate() {
            put_float(&mut vvd, offset + 16 + axis * 4, value);
        }
        put_float(&mut vvd, offset + 36, 1.0);
        put_float(&mut vvd, offset + 40, point[0]);
        put_float(&mut vvd, offset + 44, point[1]);
    }
    let mut vtx = vec![0; 173];
    for (offset, value) in [
        (0, 7),
        (16, 7),
        (20, 1),
        (28, 1),
        (32, 36),
        (36, 1),
        (40, 8),
        (44, 1),
        (48, 8),
        (52, 1),
        (56, 12),
        (64, 1),
        (68, 9),
        (73, 4),
        (77, 25),
        (81, 6),
        (85, 61),
        (89, 1),
        (93, 73),
        (146, 6),
        (150, 0),
        (154, 4),
        (158, 0),
    ] {
        put_int(&mut vtx, offset, value);
    }
    for index in 0..4 {
        vtx[98 + index * 9 + 4..98 + index * 9 + 6].copy_from_slice(&(index as u16).to_le_bytes());
    }
    for (index, value) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
        vtx[134 + index * 2..136 + index * 2].copy_from_slice(&value.to_le_bytes());
    }
    vtx[164] = 1;
    [mdl, vvd, vtx]
}

#[test]
fn static_models_decode_mesh_ranges_material_families_and_relative_names() {
    for version in [44, 48] {
        let [mut mdl, vvd, vtx] = fixture();
        put_int(&mut mdl, 4, version);
        put_string(&mut mdl, 12, "earlier");
        put_int(&mut mdl, 256, 12 - 256);
        let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
        assert_eq!(model.vertices.len(), 4);
        assert_eq!(model.parts[0].indices, [0, 1, 2, 0, 2, 3]);
        assert_eq!(model.parts[0].material, 0);
        assert_eq!(model.materials, ["earlier", "two"]);
        assert_eq!(model.material_directories, ["models/test"]);
        assert_eq!(model.skins, [vec![0], vec![1]]);
        assert_eq!(model.bounds, [[0.0; 3], [1.0, 1.0, 0.0]]);
        assert!(model.memory_bytes() > 128);
    }
    let [mut mdl, vvd, mut vtx] = fixture();
    for (offset, value) in [(496, 3), (500, 48), (584, 3)] {
        put_int(&mut mdl, offset, value);
    }
    for (offset, value) in [(73, 3), (81, 3), (146, 3), (154, 3)] {
        put_int(&mut vtx, offset, value);
    }
    let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(model.parts[0].indices, [1, 2, 3]);
}

#[test]
fn vvd_fixups_restore_mesh_order_and_vtx_strips_alternate_winding() {
    let [mdl, mut vvd, mut vtx] = fixture();
    vvd.resize(280, 0);
    for (offset, value) in [
        (48, 2),
        (52, 256),
        (256, 0),
        (260, 2),
        (264, 2),
        (268, 0),
        (272, 0),
        (276, 2),
    ] {
        put_int(&mut vvd, offset, value);
    }
    let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(model.vertices[0].position, [1.0, 1.0, 0.0]);
    assert_eq!(model.vertices[2].position, [0.0, 0.0, 0.0]);
    vtx[164] = 2;
    put_int(&mut vtx, 81, 4);
    put_int(&mut vtx, 146, 4);
    for (index, value) in [0u16, 1, 3, 2].into_iter().enumerate() {
        vtx[134 + index * 2..136 + index * 2].copy_from_slice(&value.to_le_bytes());
    }
    let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(model.parts[0].indices, [0, 1, 3, 3, 1, 2]);
    put_int(&mut vvd, 264, 3);
    assert!(Model::parse(&mdl, &vvd, &vtx).is_err());
}

#[test]
fn repeated_degenerate_strips_cannot_bypass_the_traversal_budget() {
    let [mdl, vvd, mut vtx] = fixture();
    let index_count = 8190;
    let strip_count = 4096;
    let strips = 134 + index_count * 2;
    vtx.resize(strips + strip_count * 27, 0);
    vtx[134..].fill(0);
    put_int(&mut vtx, 81, index_count as i32);
    put_int(&mut vtx, 89, strip_count as i32);
    put_int(&mut vtx, 93, (strips - 73) as i32);
    for index in 0..strip_count {
        let base = strips + index * 27;
        put_int(&mut vtx, base, index_count as i32);
        put_int(&mut vtx, base + 8, 4);
        vtx[base + 18] = 1;
    }
    assert!(
        Model::parse(&mdl, &vvd, &vtx)
            .unwrap_err()
            .to_string()
            .contains("work limit")
    );
}

#[test]
fn malformed_model_ranges_checksums_paths_and_floats_reject_before_publication() {
    for (file, offset, value) in [
        (0, 208, i32::MAX),
        (0, 204, -1),
        (0, 232, 129),
        (0, 220, 4097),
        (0, 500, 1),
        (0, 152, 0),
        (1, 16, 1_000_001),
        (1, 56, i32::MAX),
        (1, 8, 8),
        (2, 32, i32::MIN),
        (2, 81, 6_000_001),
        (2, 150, 7),
        (2, 102, 65535),
        (2, 16, 8),
    ] {
        let mut files = fixture();
        put_int(&mut files[file], offset, value);
        assert!(
            Model::parse(&files[0], &files[1], &files[2]).is_err(),
            "file {file} offset {offset}"
        );
    }
    for (offset, value) in [
        (64 + 16, f32::NAN),
        (64 + 16, 65537.0),
        (64 + 28, f32::INFINITY),
        (64 + 28, 1e30),
        (64 + 40, f32::NAN),
    ] {
        let [mdl, mut vvd, vtx] = fixture();
        put_float(&mut vvd, offset, value);
        assert!(Model::parse(&mdl, &vvd, &vtx).is_err());
    }
    let [mut mdl, vvd, vtx] = fixture();
    put_string(&mut mdl, 720, "../bad");
    assert!(Model::parse(&mdl, &vvd, &vtx).is_err());
    for file in 0..3 {
        for length in [0, 3, 35, 63, 90, 172] {
            let mut files = fixture();
            files[file].truncate(length);
            assert!(
                Model::parse(&files[0], &files[1], &files[2]).is_err(),
                "truncated {file} to {length}"
            );
        }
    }
}
