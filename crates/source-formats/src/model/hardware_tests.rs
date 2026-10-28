use super::tests::{fixture, put_int};
use super::*;

fn hardware_fixture() -> [Vec<u8>; 3] {
    let [mut mdl, vvd, mut vtx] = fixture();
    put_int(&mut mdl, 628, 4);
    vtx.resize(181, 0);
    put_int(&mut vtx, 24, 173);
    vtx[97] = 2;
    [mdl, vvd, vtx]
}

fn parse(files: &[Vec<u8>; 3]) -> io::Result<(Model, HardwareLayout)> {
    Model::parse_with_hardware(
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits::default(),
        &mut |_| Ok(()),
    )
}

fn reordered_fixture() -> [Vec<u8>; 3] {
    let [mdl, vvd, mut vtx] = hardware_fixture();
    vtx.resize(349, 0);
    for (offset, value) in [
        (73, 6),
        (77, 183),
        (81, 3),
        (85, 237),
        (93, 249),
        (322, 3),
        (330, 6),
    ] {
        put_int(&mut vtx, offset, value);
    }
    for (index, original) in [2u16, 0, 2, 1, 3, 3].into_iter().enumerate() {
        vtx[260 + index * 9..262 + index * 9].copy_from_slice(&original.to_le_bytes());
    }
    for (index, vertex) in [1u16, 3, 0].into_iter().enumerate() {
        vtx[310 + index * 2..312 + index * 2].copy_from_slice(&vertex.to_le_bytes());
    }
    vtx[340] = 1;
    [mdl, vvd, vtx]
}

#[test]
fn hardware_mapping_retains_declared_order_duplicates_and_unused_vertices() {
    let files = reordered_fixture();
    let (model, layout) = parse(&files).unwrap();
    assert_eq!(model.parts[0].indices, [0, 1, 2]);
    assert_eq!(layout.vertices, [2, 0, 2, 1, 3, 3]);
    assert_eq!(layout.checksum, 7);
    assert_eq!(layout.lods, 1);
    assert_eq!(layout.root_lod, 0);
    assert_eq!(
        layout.sections,
        [HardwareSection {
            body_part: 0,
            model: 0,
            lod: 0,
            mesh: 0,
            group: 0,
            part: 0,
            vertices: 0..6,
            triangle_indices: vec![1, 3, 0],
        }]
    );
    assert_eq!(model.skins, [vec![0], vec![1]]);
    assert_eq!(model.body_groups[0].variants[0].parts, 0..1);
    let ordinary = Model::parse(&files[0], &files[1], &files[2]).unwrap();
    assert_eq!(ordinary.parts[0].indices, model.parts[0].indices);
    assert_eq!(ordinary.vertices, model.vertices);

    let [mdl, vvd, mut vtx] = files;
    vtx.resize(392, 0);
    for (offset, value) in [(81, 8), (85, 276), (93, 292), (365, 8), (373, 6)] {
        put_int(&mut vtx, offset, value);
    }
    for (index, vertex) in [1u16, 3, 0, 2, 3, 4, 4, 5].into_iter().enumerate() {
        vtx[349 + index * 2..351 + index * 2].copy_from_slice(&vertex.to_le_bytes());
    }
    vtx[383] = 2;
    let (model, layout) = parse(&[mdl.clone(), vvd.clone(), vtx.clone()]).unwrap();
    assert_eq!(layout.vertices, [2, 0, 2, 1, 3, 3]);
    assert_eq!(layout.sections[0].triangle_indices, [1, 3, 0, 3, 2, 4]);
    assert_eq!(model.parts[0].indices, [0, 1, 2, 1, 2, 3]);
    assert_eq!(
        Model::parse(&mdl, &vvd, &vtx).unwrap().parts[0].indices,
        model.parts[0].indices
    );
}

fn two_model_two_lod_fixture() -> [Vec<u8>; 3] {
    let [mut mdl, mut vvd, mut vtx] = hardware_fixture();
    let model = mdl[416..564].to_vec();
    let mesh = mdl[576..692].to_vec();
    mdl.resize(1600, 0);
    put_int(&mut mdl, 76, 1600);
    put_int(&mut mdl, 404, 2);
    put_int(&mut mdl, 412, 624);
    mdl[1024..1172].copy_from_slice(&model);
    mdl[1172..1320].copy_from_slice(&model);
    mdl[1320..1436].copy_from_slice(&mesh);
    mdl[1436..1552].copy_from_slice(&mesh);
    for (offset, value) in [
        (1100, 296),
        (1248, 264),
        (1256, 192),
        (1372, 4),
        (1376, 3),
        (1468, 1),
        (1488, 4),
        (1492, 3),
    ] {
        put_int(&mut mdl, offset, value);
    }
    let vertex_bytes = vvd[64..256].to_vec();
    vvd.resize(448, 0);
    vvd[256..448].copy_from_slice(&vertex_bytes);
    for (offset, value) in [(12, 2), (16, 8), (20, 6)] {
        put_int(&mut vvd, offset, value);
    }
    vtx.resize(920, 0);
    for (offset, value) in [
        (20, 2),
        (24, 200),
        (36, 2),
        (40, 184),
        (220, 2),
        (224, 20),
        (228, 2),
        (232, 36),
        (240, 1),
        (244, 60),
        (252, 1),
        (256, 57),
        (264, 1),
        (268, 54),
        (276, 1),
        (280, 51),
        (300, 1),
        (304, 100),
        (309, 1),
        (313, 116),
        (318, 1),
        (322, 132),
        (327, 1),
        (331, 148),
    ] {
        put_int(&mut vtx, offset, value);
    }
    for (group, original, data) in [
        (400, 2u16, 600),
        (425, 1, 680),
        (450, 3, 760),
        (475, 0, 840),
    ] {
        for (offset, value) in [
            (group, 3),
            (group + 4, data - group),
            (group + 8, 3),
            (group + 12, data + 27 - group),
            (group + 16, 1),
            (group + 20, data + 33 - group),
            (data + 33, 3),
            (data + 41, 3),
        ] {
            put_int(&mut vtx, offset, value as i32);
        }
        vtx[group + 24] = 2;
        for vertex in 0..3 {
            let value = match original {
                0 => vertex as u16,
                1 => [1, 0, 2][vertex],
                _ if vertex == 0 => original,
                _ => (vertex - 1) as u16,
            };
            vtx[data + 4 + vertex * 9..data + 6 + vertex * 9].copy_from_slice(&value.to_le_bytes());
            let local = match group {
                400 => [0u16, 1, 2],
                425 => [2, 0, 1],
                450 => [1, 2, 0],
                _ => [2, 1, 0],
            };
            vtx[data + 27 + vertex * 2..data + 29 + vertex * 2]
                .copy_from_slice(&local[vertex].to_le_bytes());
        }
        vtx[data + 51] = 1;
    }
    [mdl, vvd, vtx]
}

#[test]
fn hardware_sections_keep_every_model_and_lod_in_lod_first_order() {
    let files = two_model_two_lod_fixture();
    let (model, layout) = parse(&files).unwrap();
    assert_eq!(model.body_groups[0].variants.len(), 2);
    assert_eq!(model.parts.len(), 2);
    assert_eq!(layout.lods, 2);
    let identity = layout
        .sections
        .iter()
        .map(|section| {
            (
                section.lod,
                section.body_part,
                section.model,
                section.mesh,
                section.group,
                section.part,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        identity,
        [
            (0, 0, 0, 0, 0, 0),
            (0, 0, 1, 0, 0, 1),
            (1, 0, 0, 0, 0, 0),
            (1, 0, 1, 0, 0, 1)
        ]
    );
    let first = layout
        .sections
        .iter()
        .map(|section| layout.vertices[section.vertices.start])
        .collect::<Vec<_>>();
    assert_eq!(first, [2, 7, 1, 4]);
    let triangles = layout
        .sections
        .iter()
        .map(|section| section.triangle_indices.as_slice())
        .collect::<Vec<_>>();
    let expected: [&[u32]; 4] = [&[0, 1, 2], &[1, 2, 0], &[2, 0, 1], &[2, 1, 0]];
    assert_eq!(triangles, expected);
    for section in &layout.sections {
        let roots = section
            .triangle_indices
            .iter()
            .map(|&local| layout.vertices[section.vertices.start + local as usize])
            .collect::<Vec<_>>();
        if section.lod == 0 {
            assert_eq!(roots, model.parts[section.part].indices);
        }
    }
    let mut files = files;
    files[0][377] = 1;
    let (_, layout) = parse(&files).unwrap();
    assert_eq!(layout.root_lod, 1);
    assert_eq!(layout.sections.len(), 4);
}

#[test]
fn hardware_mesh_slots_reject_reordered_and_aliased_ids() {
    for (first, second) in [(0, 0), (1, 0), (0, -1), (0, 2)] {
        let mut files = two_model_two_lod_fixture();
        put_int(&mut files[0], 1352, first);
        put_int(&mut files[0], 1468, second);
        assert!(parse(&files).is_err(), "mesh IDs {first}, {second}");
        assert_eq!(
            Model::parse(&files[0], &files[1], &files[2])
                .unwrap()
                .parts
                .len(),
            2
        );
    }
}

#[test]
fn hardware_mapping_addresses_fixed_up_vertices_and_checks_every_lod_fixup_total() {
    let mut files = reordered_fixture();
    files[1].resize(280, 0);
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
        put_int(&mut files[1], offset, value);
    }
    let (model, layout) = parse(&files).unwrap();
    assert_eq!(layout.vertices, [2, 0, 2, 1, 3, 3]);
    assert_eq!(
        model.vertices[layout.vertices[0] as usize].position,
        [0.0; 3]
    );
    assert_eq!(
        model.vertices[layout.vertices[1] as usize].position,
        [1.0, 1.0, 0.0]
    );
    let mut files = two_model_two_lod_fixture();
    files[1].resize(496, 0);
    for (offset, value) in [
        (48, 4),
        (52, 448),
        (448, 1),
        (452, 0),
        (456, 3),
        (460, 0),
        (464, 3),
        (468, 1),
        (472, 1),
        (476, 4),
        (480, 3),
        (484, 0),
        (488, 7),
        (492, 1),
    ] {
        put_int(&mut files[1], offset, value);
    }
    assert!(parse(&files).is_ok());
    put_int(&mut files[1], 448, 0);
    let failure = parse(&files).unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("fixups do not match declared LOD")
    );
}

#[test]
fn hardware_group_limits_are_global_even_when_tables_share_their_payload() {
    let [mdl, vvd, mut vtx] = hardware_fixture();
    let group = vtx[73..98].to_vec();
    vtx.resize(250, 0);
    put_int(&mut vtx, 64, 2);
    put_int(&mut vtx, 68, 136);
    for base in [200, 225] {
        vtx[base..base + 25].copy_from_slice(&group);
        for (offset, value) in [(4, 98), (12, 134), (20, 146)] {
            put_int(&mut vtx, base + offset, value - base as i32);
        }
    }
    let (_, layout) = parse(&[mdl.clone(), vvd.clone(), vtx.clone()]).unwrap();
    assert_eq!(layout.vertices, [0, 1, 2, 3, 0, 1, 2, 3]);
    assert_eq!(layout.sections[0].group, 0);
    assert_eq!(layout.sections[1].group, 1);
    for section in &layout.sections {
        assert_eq!(section.triangle_indices, [0, 1, 2, 0, 2, 3]);
    }
    for limits in [
        HardwareLimits {
            vertices: 7,
            ..HardwareLimits::default()
        },
        HardwareLimits {
            sections: 1,
            ..HardwareLimits::default()
        },
    ] {
        assert!(Model::parse_with_hardware(&mdl, &vvd, &vtx, limits, &mut |_| Ok(())).is_err());
    }
}

#[test]
fn hardware_rejects_bad_correspondence_offsets_and_unsupported_paths() {
    for (file, offset, value) in [
        (0, 500, 48),
        (0, 588, 1),
        (0, 628, 3),
        (0, 592, 1),
        (0, 600, 1),
        (0, 516, 1),
        (0, 480, 1),
        (0, 152, 0),
        (1, 8, 8),
        (2, 16, 8),
        (2, 44, 2),
        (2, 24, i32::MAX),
        (2, 173, 1),
        (2, 77, i32::MIN),
        (2, 89, -1),
        (2, 150, 7),
    ] {
        let mut files = hardware_fixture();
        put_int(&mut files[file], offset, value);
        assert!(parse(&files).is_err(), "file {file} offset {offset}");
    }
    for (offset, flags) in [(72, 1), (97, 0), (97, 3), (97, 6), (164, 5)] {
        let mut files = hardware_fixture();
        files[2][offset] = flags;
        assert!(parse(&files).is_err(), "offset {offset}, flags {flags}");
    }
    let mut files = two_model_two_lod_fixture();
    put_int(&mut files[2], 252, 0);
    assert!(parse(&files).is_err());
    let mut files = two_model_two_lod_fixture();
    files[2][684..686].copy_from_slice(&3u16.to_le_bytes());
    assert!(parse(&files).is_err());
}

#[test]
fn hardware_limits_and_caller_rejection_stop_before_growth() {
    let files = reordered_fixture();
    let mut charged = HardwareCharge::default();
    let (_, layout) = Model::parse_with_hardware(
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits::default(),
        &mut |charge| {
            charged.bytes += charge.bytes;
            charged.work += charge.work;
            Ok(())
        },
    )
    .unwrap();
    let topology_bytes =
        layout.sections[0].triangle_indices.capacity() * std::mem::size_of::<u32>();
    let layout_bytes = std::mem::size_of::<HardwareLayout>()
        + layout.sections.capacity() * std::mem::size_of::<HardwareSection>()
        + layout.vertices.capacity() * std::mem::size_of::<u32>()
        + topology_bytes;
    assert_eq!(layout.memory_bytes(), layout_bytes);
    for limits in [
        HardwareLimits {
            vertices: 5,
            ..HardwareLimits::default()
        },
        HardwareLimits {
            sections: 0,
            ..HardwareLimits::default()
        },
        HardwareLimits {
            work: 1,
            ..HardwareLimits::default()
        },
        HardwareLimits {
            bytes: 1,
            ..HardwareLimits::default()
        },
        HardwareLimits {
            bytes: charged.bytes - topology_bytes,
            ..HardwareLimits::default()
        },
        HardwareLimits {
            vertices: usize::MAX,
            ..HardwareLimits::default()
        },
    ] {
        assert!(
            Model::parse_with_hardware(&files[0], &files[1], &files[2], limits, &mut |_| Ok(()))
                .is_err()
        );
    }
    let mut calls = 0;
    let failure = Model::parse_with_hardware(
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits::default(),
        &mut |charge| {
            calls += 1;
            if charge.bytes > 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "cancel hardware parse",
                ));
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(failure.kind(), io::ErrorKind::Interrupted);
    assert_eq!(calls, 2);
}
