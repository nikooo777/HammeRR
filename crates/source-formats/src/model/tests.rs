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

pub(crate) fn body_fixture() -> [Vec<u8>; 3] {
    let [mut mdl, vvd, mut vtx] = fixture();
    let model = mdl[416..564].to_vec();
    let mesh = mdl[576..692].to_vec();
    mdl.resize(1600, 0);
    put_int(&mut mdl, 76, 1600);
    put_int(&mut mdl, 404, 2);
    put_int(&mut mdl, 412, 624);
    put_string(&mut mdl, 1024, "blank");
    mdl[1172..1320].copy_from_slice(&model);
    put_string(&mut mdl, 1172, "quad");
    put_int(&mut mdl, 1248, 148);
    mdl[1320..1436].copy_from_slice(&mesh);
    put_int(&mut mdl, 1324, -148);
    vtx.resize(201, 0);
    for (offset, value) in [
        (36, 2),
        (40, 137),
        (173, 1),
        (177, 16),
        (181, 1),
        (185, -129),
    ] {
        put_int(&mut vtx, offset, value);
    }
    [mdl, vvd, vtx]
}

#[test]
fn bodygroups_select_blank_and_visible_variants_with_native_integer_mapping() {
    let [mdl, vvd, vtx] = body_fixture();
    let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(model.body_groups.len(), 1);
    assert_eq!(model.body_groups[0].variants.len(), 2);
    assert_eq!(model.body_groups[0].variants[0].name, "blank");
    assert_eq!(model.body_groups[0].variants[1].name, "quad");
    assert_eq!(model.parts_for_body(0).count(), 0);
    assert_eq!(
        model.parts_for_body(1).next().unwrap().indices,
        [0, 1, 2, 0, 2, 3]
    );
    assert_eq!(model.bounds_for_body(0), None);
    assert_eq!(model.bounds_for_body(1), Some([[0.0; 3], [1.0, 1.0, 0.0]]));
    assert_eq!(model.set_bodygroup(6, 0, 1).unwrap(), 7);
    assert_eq!(model.set_bodygroup(7, 0, 0).unwrap(), 6);
    assert!(model.set_bodygroup(0, 1, 0).is_err());
    assert!(model.set_bodygroup(0, 0, 2).is_err());
    let mut bad = mdl.clone();
    put_int(&mut bad, 408, 0);
    assert!(Model::parse(&bad, &vvd, &vtx).is_err());
    let mut bad = vtx.clone();
    put_int(&mut bad, 36, 3);
    assert!(Model::parse(&mdl, &vvd, &bad).is_err());
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

fn reference_fixture() -> [Vec<u8>; 3] {
    let [mut mdl, mut vvd, vtx] = fixture();
    mdl.resize(1456, 0);
    put_int(&mut mdl, 76, 1456);
    put_int(&mut mdl, 152, 0);
    put_int(&mut mdl, 156, 2);
    put_int(&mut mdl, 160, 1024);
    put_int(&mut mdl, 1028, -1);
    put_int(&mut mdl, 1244, 0);
    put_float(&mut mdl, 1056, 2.0);
    put_float(&mut mdl, 1076, std::f32::consts::FRAC_1_SQRT_2);
    put_float(&mut mdl, 1080, std::f32::consts::FRAC_1_SQRT_2);
    put_float(&mut mdl, 1296, 1.0);
    put_float(&mut mdl, 1276, 3.0);
    for base in [1024, 1240] {
        for offset in [96, 116, 136] {
            put_float(&mut mdl, base + offset, 1.0);
        }
    }
    for vertex in 0..4 {
        let base = 64 + vertex * 48;
        put_float(&mut vvd, base, 0.25);
        put_float(&mut vvd, base + 4, 0.75);
        vvd[base + 12] = 0;
        vvd[base + 13] = 1;
        vvd[base + 15] = 2;
    }
    [mdl, vvd, vtx]
}

#[test]
fn attachments_use_reference_bone_poses_without_inverse_bind_transforms() {
    let [mut mdl, vvd, vtx] = reference_fixture();
    mdl.resize(1664, 0);
    put_int(&mut mdl, 76, 1664);
    put_int(&mut mdl, 240, 2);
    put_int(&mut mdl, 244, 1456);
    for (index, name) in ["muzzle", "world"].into_iter().enumerate() {
        let base = 1456 + index * 92;
        let name_offset = 1640 + index * 8;
        put_int(&mut mdl, base, (name_offset - base) as i32);
        put_string(&mut mdl, name_offset, name);
        put_int(&mut mdl, base + 8, 1);
        for offset in [12, 32, 52] {
            put_float(&mut mdl, base + offset, 1.0);
        }
        put_float(&mut mdl, base + 24, 4.0);
    }
    put_int(&mut mdl, 1552, 0x10000);
    let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(model.attachments.len(), 2);
    let first = &model.attachments[0];
    assert_eq!(first.name, "muzzle");
    assert_eq!(first.bone, 1);
    assert!(
        first
            .transform
            .translation
            .abs_diff_eq(glam::DVec3::new(-1.0, 4.0, 0.0), 1e-5)
    );
    assert!(
        first
            .transform
            .matrix3
            .x_axis
            .abs_diff_eq(glam::DVec3::Y, 1e-5)
    );
    assert!(!first.world_aligned());
    assert!(model.attachments[1].world_aligned());
    put_float(&mut mdl, 1136, 32.0);
    assert_eq!(
        Model::parse(&mdl, &vvd, &vtx).unwrap().attachments[0].transform,
        first.transform
    );
    for (offset, value) in [
        (240, 1025),
        (1464, 2),
        (1456, i32::MAX),
        (1480, f32::NAN.to_bits() as i32),
    ] {
        let mut broken = mdl.clone();
        put_int(&mut broken, offset, value);
        assert!(
            Model::parse(&broken, &vvd, &vtx).is_err(),
            "offset {offset}"
        );
    }
}

#[test]
fn reference_models_compose_parent_poses_inverse_binds_and_weighted_vertices() {
    let [mut mdl, vvd, vtx] = reference_fixture();
    let model = Model::parse(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(model.reference_bones, 2);
    assert!(!model.header.is_static_prop());
    for (vertex, expected) in model.vertices.iter().zip([
        [-0.25, 0.0, 0.0],
        [-0.25, 1.0, 0.0],
        [-1.25, 1.0, 0.0],
        [-1.25, 0.0, 0.0],
    ]) {
        assert!(
            glam::Vec3::from_array(vertex.position)
                .abs_diff_eq(glam::Vec3::from_array(expected), 1e-5)
        );
        assert_eq!(vertex.normal, [0.0, 0.0, 1.0]);
    }
    for (base, translation) in [(1024, 2.0), (1240, -1.0)] {
        for offset in 96..144 {
            mdl[base + offset] = 0;
        }
        put_float(&mut mdl, base + 100, 1.0);
        put_float(&mut mdl, base + 112, -1.0);
        put_float(&mut mdl, base + 136, 1.0);
        put_float(&mut mdl, base + 124, translation);
    }
    let restored = Model::parse(&mdl, &vvd, &vtx).unwrap();
    let original = fixture();
    let original = Model::parse(&original[0], &original[1], &original[2]).unwrap();
    for (a, b) in restored.vertices.iter().zip(original.vertices) {
        assert!(
            glam::Vec3::from_array(a.position)
                .abs_diff_eq(glam::Vec3::from_array(b.position), 1e-5)
        );
    }
}

#[test]
fn reference_models_reject_bad_hierarchies_transforms_and_vertex_weights() {
    let [mdl, mut vvd, vtx] = reference_fixture();
    vvd[79] = 1;
    put_float(&mut vvd, 64, 1.0 + f32::EPSILON);
    assert!(Model::parse(&mdl, &vvd, &vtx).is_ok());
    for (file, offset, value) in [
        (0, 156, 257),
        (0, 1028, 0),
        (0, 1028, 2),
        (1, 76, 255),
        (1, 79, 4),
    ] {
        let mut files = reference_fixture();
        if file == 1 {
            files[file][offset] = value as u8;
        } else {
            put_int(&mut files[file], offset, value);
        }
        assert!(
            Model::parse(&files[0], &files[1], &files[2]).is_err(),
            "file{file} offset{offset}"
        );
    }
    for (file, offset, value) in [
        (0, 1076, 0.0),
        (0, 1120, f32::NAN),
        (0, 1056, 1e20),
        (1, 64, -0.5),
        (1, 64, 0.0),
        (1, 64, 1.5),
    ] {
        let mut files = reference_fixture();
        put_float(&mut files[file], offset, value);
        assert!(
            Model::parse(&files[0], &files[1], &files[2]).is_err(),
            "file{file} offset{offset}"
        );
    }
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
