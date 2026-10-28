use source_formats::bsp::{
    self, Bsp, DisplacementCorner, DisplacementInfo, DisplacementLightmapSample,
    DisplacementNeighbor, DisplacementTriangle, DisplacementVertex, Displacements, Edge, Face,
    FaceGeometry, LightingKind, Model, Plane,
};

fn fixture() -> (Bsp, Displacements) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Face {
        edge_count: 4,
        dispinfo: 0,
        ..Default::default()
    }]);
    bsp.set_values(
        bsp::lump::VERTEXES,
        &[
            [0.0_f32, 0.0, 0.0],
            [0.0, 16.0, 0.0],
            [16.0, 16.0, 0.0],
            [16.0, 0.0, 0.0],
        ],
    );
    bsp.set_records(&[
        Edge([0, 0]),
        Edge([0, 1]),
        Edge([1, 2]),
        Edge([2, 3]),
        Edge([3, 0]),
    ]);
    bsp.set_values(bsp::lump::SURFEDGES, &[1_i32, 2, 3, 4]);
    let mut info = DisplacementInfo {
        padding: 0xabcd,
        min_tessellation: i32::MIN,
        ..Default::default()
    };
    info.edges[0][0].padding = 0xa5;
    info.edges[0][0].orientation = 255;
    info.corners[0].neighbors = [u16::MAX; 4];
    info.corners[0].padding = 0x5a;
    let mut vertices = vec![DisplacementVertex::default(); 25];
    vertices[12] = DisplacementVertex {
        direction: [0.0, 0.0, 1.0],
        distance: 8.0,
        alpha: 300.0,
    };
    let data = Displacements {
        infos: vec![info],
        vertices,
        triangles: vec![DisplacementTriangle { tags: 0x8007 }; 32],
    };
    (bsp, data)
}

fn final_geometry_fixture(power: i32) -> Bsp {
    let (mut bsp, mut data) = fixture();
    let info = DisplacementInfo {
        power,
        start: [16.0, 16.0, 0.0],
        ..Default::default()
    };
    let size = info.grid_size().unwrap();
    data.infos = vec![info];
    data.vertices = vec![DisplacementVertex::default(); info.vertex_count().unwrap()];
    data.vertices[size * (size / 2) + size / 2] = DisplacementVertex {
        direction: [0.0, 0.0, 1.0],
        distance: 8.0,
        alpha: 127.5,
    };
    data.triangles = vec![DisplacementTriangle::default(); info.triangle_count().unwrap()];
    bsp.set_records(&[Face {
        edge_count: 4,
        dispinfo: 0,
        area: 256.0,
        light_offset: -1,
        ..Default::default()
    }]);
    bsp.set_records(&[Plane {
        normal: [0.0, 0.0, 1.0],
        ..Default::default()
    }]);
    bsp.set_records(&[
        Model::default(),
        Model {
            face_count: 1,
            ..Default::default()
        },
    ]);
    bsp.set_values(bsp::lump::VERTNORMALS, &[[0.0_f32, 0.0, 1.0]]);
    bsp.set_values(bsp::lump::VERTNORMALINDICES, &[0_u16; 4]);
    bsp.set_displacements(&data).unwrap();
    bsp
}

#[test]
fn native_displacement_layout_padding_inactive_neighbors_and_mesh_positions_round_trip() {
    assert_eq!(size_of::<DisplacementInfo>(), 176);
    assert_eq!(size_of::<DisplacementVertex>(), 20);
    assert_eq!(size_of::<DisplacementTriangle>(), 2);
    assert_eq!(size_of::<DisplacementNeighbor>(), 6);
    assert_eq!(size_of::<DisplacementCorner>(), 10);
    let (mut bsp, data) = fixture();
    assert_eq!(data.infos[0].flags(), Some(0));
    assert_eq!(
        DisplacementInfo {
            min_tessellation: i32::MIN | 14,
            ..data.infos[0]
        }
        .flags(),
        Some(14)
    );
    assert_eq!(
        DisplacementInfo {
            min_tessellation: 0,
            ..data.infos[0]
        }
        .flags(),
        None
    );
    bsp.set_displacements(&data).unwrap();
    let bsp = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(bsp.displacements().unwrap(), Some(data.clone()));
    assert_eq!(bsp.displacement_lightmap_samples(0).unwrap(), None);
    let quad = bsp.displacement_quad(0).unwrap();
    let positions = data.positions(0, quad).unwrap();
    assert_eq!(positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(positions[12], [8.0, 8.0, 8.0]);
    assert_eq!(positions[24], [16.0, 16.0, 0.0]);
    for power in [2, 3, 4] {
        let info = DisplacementInfo {
            power,
            ..Default::default()
        };
        assert_eq!(
            info.vertex_count().unwrap(),
            ((1 << power) + 1_usize).pow(2)
        );
        assert_eq!(
            info.triangle_count().unwrap(),
            2 * (1 << power) * (1 << power)
        );
        let mut bsp = final_geometry_fixture(power);
        let geometry = FaceGeometry::new(&bsp, LightingKind::Ldr).unwrap();
        let winding = geometry.face(0).unwrap();
        assert_eq!(winding.model, 1);
        assert_eq!(winding.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(winding.normals, vec![[0.0, 0.0, 1.0]; 4]);
        let terrain = geometry.terrain(0).unwrap().unwrap();
        let size = info.grid_size().unwrap();
        let middle = size * (size / 2) + size / 2;
        assert_eq!(terrain.positions[0], [16.0, 16.0, 0.0]);
        assert_eq!(terrain.positions[middle], [8.0, 8.0, 8.0]);
        assert_eq!(terrain.positions[size * size - 1], [0.0, 0.0, 0.0]);
        assert_eq!(terrain.grid_uvs[0], [0.0, 0.0]);
        assert_eq!(terrain.grid_uvs[middle], [0.5, 0.5]);
        assert_eq!(terrain.grid_uvs[size * size - 1], [1.0, 1.0]);
        assert_eq!(terrain.alphas[middle], 127.5);
        assert_eq!(terrain.triangles.len(), info.triangle_count().unwrap());
        assert_eq!(
            geometry.positions(0).unwrap(),
            (1, terrain.positions.clone())
        );
        for normal in &terrain.normals {
            assert!((normal.iter().map(|value| value * value).sum::<f32>() - 1.0).abs() < 1e-6);
        }
        let mut data = bsp.displacements().unwrap().unwrap();
        data.infos[0].allowed_vertices[0] &= !(1 << 1);
        bsp.set_displacements(&data).unwrap();
        let terrain = FaceGeometry::new(&bsp, LightingKind::Ldr)
            .unwrap()
            .terrain(0)
            .unwrap()
            .unwrap();
        assert!(terrain.triangles.len() < info.triangle_count().unwrap());
        assert!(
            terrain
                .triangles
                .iter()
                .all(|triangle| !triangle.contains(&1))
        );
    }
}

#[test]
fn malformed_displacement_ranges_fields_and_neighbors_preserve_existing_records() {
    let (mut bsp, data) = fixture();
    bsp.set_displacements(&data).unwrap();
    let old = bsp.clone();
    let mut invalids = Vec::new();
    for info in [
        DisplacementInfo {
            power: i32::MAX,
            ..data.infos[0]
        },
        DisplacementInfo {
            vertex_start: -1,
            ..data.infos[0]
        },
        DisplacementInfo {
            vertex_start: i32::MAX,
            ..data.infos[0]
        },
        DisplacementInfo {
            triangle_start: 1,
            ..data.infos[0]
        },
        DisplacementInfo {
            face: 1,
            ..data.infos[0]
        },
        DisplacementInfo {
            start: [f32::NAN; 3],
            ..data.infos[0]
        },
        DisplacementInfo {
            lightmap_alpha_start: 1,
            ..data.infos[0]
        },
    ] {
        invalids.push(Displacements {
            infos: vec![info],
            ..data.clone()
        });
    }
    for neighbor in [
        DisplacementNeighbor {
            index: 1,
            ..Default::default()
        },
        DisplacementNeighbor {
            index: 0,
            orientation: 4,
            ..Default::default()
        },
        DisplacementNeighbor {
            index: 0,
            span: 3,
            ..Default::default()
        },
    ] {
        let mut invalid = data.clone();
        invalid.infos[0].edges[0][0] = neighbor;
        invalids.push(invalid);
    }
    let mut invalid = data.clone();
    invalid.infos[0].corners[0].count = 5;
    invalids.push(invalid);
    let mut invalid = data.clone();
    invalid.vertices[0].distance = f32::INFINITY;
    invalids.push(invalid);
    for invalid in invalids {
        assert!(bsp.set_displacements(&invalid).is_err());
        assert_eq!(bsp, old);
    }
}

#[test]
fn unsupported_displacement_lumps_and_broken_base_quad_references_reject() {
    let (mut bsp, data) = fixture();
    bsp.set_displacements(&data).unwrap();
    for index in [
        bsp::lump::DISPINFO,
        bsp::lump::DISP_VERTS,
        bsp::lump::DISP_TRIS,
    ] {
        let mut invalid = bsp.clone();
        invalid.lumps[index].version = 1;
        assert!(invalid.displacements().is_err());
        invalid.lumps[index].version = 0;
        invalid.lumps[index].fourcc = [1, 0, 0, 0];
        assert!(invalid.displacements().is_err());
        invalid.lumps[index].fourcc = [0; 4];
        invalid.lumps[index].data.pop();
        assert!(invalid.displacements().is_err());
    }
    for edge in [i32::MIN, 99, -1] {
        let mut invalid = bsp.clone();
        invalid.set_values(bsp::lump::SURFEDGES, &[edge, 2, 3, 4]);
        assert!(invalid.displacement_quad(0).is_err());
    }
    let mut bsp = final_geometry_fixture(2);
    let ldr = bsp.lighting_faces(LightingKind::Ldr).unwrap();
    bsp.set_lighting_faces(LightingKind::Hdr, &ldr).unwrap();
    assert!(FaceGeometry::new(&bsp, LightingKind::Hdr).is_ok());
    for plane in [0, 1] {
        let mut oriented = bsp.clone();
        let mut faces = ldr.clone();
        faces[0].plane = plane;
        faces[0].side = 1;
        oriented
            .set_lighting_faces(LightingKind::Ldr, &faces)
            .unwrap();
        oriented.set_records(&[
            Plane {
                normal: [0.0, 0.0, 1.0],
                ..Default::default()
            },
            Plane {
                normal: [0.0, 0.0, -1.0],
                ..Default::default()
            },
        ]);
        oriented.set_values(bsp::lump::SURFEDGES, &[-4_i32, -3, -2, -1]);
        oriented.set_values(
            bsp::lump::VERTNORMALS,
            &[[0.0_f32, 0.0, 1.0], [0.0, 0.0, -1.0]],
        );
        oriented.set_values(bsp::lump::VERTNORMALINDICES, &[1_u16; 4]);
        let winding = FaceGeometry::new(&oriented, LightingKind::Ldr)
            .unwrap()
            .face(0)
            .unwrap();
        assert_eq!(winding.normal, [0.0, 0.0, -1.0]);
        assert_eq!(winding.normals, vec![[0.0, 0.0, -1.0]; 4]);
    }
    for change in [0, 1, 2] {
        let mut faces = ldr.clone();
        match change {
            0 => faces[0].lightmap_size[0] += 1,
            1 => faces[0].first_edge += 1,
            _ => faces[0].dispinfo = -1,
        }
        let mut invalid = bsp.clone();
        invalid
            .set_lighting_faces(LightingKind::Hdr, &faces)
            .unwrap();
        assert!(FaceGeometry::new(&invalid, LightingKind::Hdr).is_err());
    }
    bsp.lumps[bsp::lump::VERTNORMALS].data.clear();
    bsp.lumps[bsp::lump::VERTNORMALINDICES].data.clear();
    let geometry = FaceGeometry::new(&bsp, LightingKind::Ldr).unwrap();
    assert!(geometry.positions(0).is_ok());
    assert!(geometry.face(0).is_err());
    let mut faces = ldr;
    faces[0].dispinfo = -1;
    bsp.set_lighting_faces(LightingKind::Ldr, &faces).unwrap();
    for index in [
        bsp::lump::DISPINFO,
        bsp::lump::DISP_VERTS,
        bsp::lump::DISP_TRIS,
    ] {
        bsp.lumps[index].data.clear();
    }
    assert!(
        FaceGeometry::new(&bsp, LightingKind::Ldr)
            .unwrap()
            .positions(0)
            .is_ok()
    );
    for edges in [[1_i32, 2, 4, 3], [1, 2, 3, 1], [i32::MIN, 2, 3, 4]] {
        let mut invalid = bsp.clone();
        invalid.set_values(bsp::lump::SURFEDGES, &edges);
        assert!(
            FaceGeometry::new(&invalid, LightingKind::Ldr)
                .unwrap()
                .positions(0)
                .is_err()
        );
    }
    let mut invalid = bsp.clone();
    invalid.set_records(&[
        Model {
            face_count: 1,
            ..Default::default()
        },
        Model {
            face_count: 1,
            ..Default::default()
        },
    ]);
    assert!(FaceGeometry::new(&invalid, LightingKind::Ldr).is_err());
    let mut cancelled = || {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ))
    };
    assert_eq!(
        FaceGeometry::new_controlled(&bsp, LightingKind::Ldr, &mut cancelled)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
}

#[test]
fn native_triangle_order_keeps_clockwise_coverage_for_every_supported_power() {
    for power in 2..=4 {
        let info = DisplacementInfo {
            power,
            ..Default::default()
        };
        let size = info.grid_size().unwrap();
        let triangles = info.triangle_indices().unwrap();
        assert_eq!(triangles.len(), info.triangle_count().unwrap());
        assert_eq!(triangles[0], [0, size, size + 1]);
        assert_eq!(triangles[7], [size, 2 * size, size + 1]);
        let mut area = 0isize;
        for [a, b, c] in triangles {
            assert!([a, b, c].iter().all(|&i| i < size * size));
            let xy = |index: usize| [(index % size) as isize, (index / size) as isize];
            let [a, b, c] = [a, b, c].map(xy);
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert_eq!(cross, -1);
            area -= cross;
        }
        assert_eq!(area as usize, 2 * (size - 1).pow(2));
    }
}

#[test]
fn power_info_lightmap_order_is_distinct_from_contact_order() {
    for power in 2..=4 {
        let info = DisplacementInfo {
            power,
            ..Default::default()
        };
        let size = info.grid_size().unwrap();
        let triangles = info.lightmap_triangle_indices().unwrap();
        assert_eq!(
            triangles[0],
            [
                (size - 3) * size + size - 1,
                (size - 3) * size + size - 2,
                (size - 2) * size + size - 2
            ]
        );
        assert_ne!(triangles, info.triangle_indices().unwrap());
    }
    let info = DisplacementInfo {
        power: 4,
        ..Default::default()
    };
    assert_eq!(info.lightmap_triangle_indices().unwrap()[337][1], 0);
}

#[test]
fn stock_power_four_samples_match_power_info_uvs_except_native_triangle_511_aliases() {
    let root = std::env::var_os("CSS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".local/share/Steam/steamapps/common/Counter-Strike Source")
        });
    let maps = root.join("cstrike/maps");
    if !maps.is_dir() {
        return;
    }
    let mut checked = 0;
    let mut aliases = 0;
    for path in std::fs::read_dir(maps)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
    {
        if path.extension().is_none_or(|extension| extension != "bsp")
            || path.metadata().unwrap().len() == 0
        {
            continue;
        }
        let bsp = Bsp::read(&path).unwrap();
        let Some(data) = bsp.displacements().unwrap() else {
            continue;
        };
        let faces = if bsp.lumps[bsp::lump::FACES].data.is_empty() {
            bsp.values::<Face>(bsp::lump::FACES_HDR).unwrap()
        } else {
            bsp.records::<Face>().unwrap()
        };
        for (index, info) in data
            .infos
            .iter()
            .enumerate()
            .filter(|(_, info)| info.power == 4)
        {
            let triangles = info.lightmap_triangle_indices().unwrap();
            let positions = (0..289)
                .map(|i| [(i % 17) as f32, (i / 17) as f32, 0.0])
                .collect::<Vec<_>>();
            let face = faces[usize::from(info.face)];
            let Some(samples) = bsp.displacement_lightmap_samples(index).unwrap() else {
                continue;
            };
            let width = face.lightmap_size[0] as usize + 1;
            for (index, sample) in samples.into_iter().enumerate() {
                let Some(surface) = sample.surface(&triangles, &positions).unwrap() else {
                    continue;
                };
                let expected = [
                    (index % width) as f64 * 16.0 / f64::from(face.lightmap_size[0]),
                    (index / width) as f64 * 16.0 / f64::from(face.lightmap_size[1]),
                ];
                let difference = [
                    surface.position[0] - expected[0],
                    surface.position[1] - expected[1],
                ];
                checked += 1;
                if difference.iter().any(|value| value.abs() > 0.025) {
                    assert_eq!(sample.triangle, 255, "{}", path.display());
                    assert!(
                        (difference[0] + 8.0).abs() < 0.025 && (difference[1] - 8.0).abs() < 0.025,
                        "{difference:?} in {}",
                        path.display()
                    );
                    aliases += 1;
                }
            }
        }
    }
    eprintln!("{checked} power-4 samples checked; {aliases} native triangle-511 aliases");
}

#[test]
fn native_lightmap_samples_preserve_escapes_weights_and_missing_samples() {
    let (mut bsp, mut data) = fixture();
    data.infos[0].power = 4;
    data.vertices.resize(289, DisplacementVertex::default());
    data.triangles.resize(512, DisplacementTriangle::default());
    let mut face = bsp.records::<Face>().unwrap()[0];
    face.lightmap_size = [4, 0];
    bsp.set_records(&[face]);
    let samples = [0, 254, 255, 510, 0].map(|triangle| DisplacementLightmapSample {
        triangle,
        weights: if triangle == 0 { [0; 3] } else { [127, 64, 63] },
    });
    let bytes = data.infos[0].encode_lightmap_samples(&samples).unwrap();
    assert_eq!(bytes.len(), 22);
    assert_eq!(&bytes[8..10], &[255, 0]);
    assert_eq!(&bytes[13..15], &[255, 255]);
    bsp.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data = bytes.clone();
    bsp.set_displacements(&data).unwrap();
    assert_eq!(
        bsp.displacement_lightmap_samples(0).unwrap().unwrap(),
        samples
    );
    assert_eq!(bsp.displacements().unwrap().unwrap(), data);
    assert_eq!(
        data.infos[0]
            .encode_lightmap_samples(&bsp.displacement_lightmap_samples(0).unwrap().unwrap())
            .unwrap(),
        bytes
    );
    assert!(
        data.infos[0]
            .encode_lightmap_samples(&[DisplacementLightmapSample {
                triangle: 511,
                weights: [255, 0, 0]
            }])
            .is_err()
    );
}

#[test]
fn malformed_lightmap_streams_ranges_and_extents_fail_without_overwriting_records() {
    let (mut bsp, data) = fixture();
    bsp.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data = vec![0, 255, 0, 0];
    bsp.set_displacements(&data).unwrap();
    let mut cases = Vec::new();
    for bytes in [vec![0, 255, 0], vec![255, 0, 255, 0], vec![32, 255, 0, 0]] {
        let mut invalid = bsp.clone();
        invalid.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data = bytes;
        cases.push(invalid);
    }
    for size in [[-1, 0], [i32::MAX, i32::MAX], [1, 1]] {
        let mut invalid = bsp.clone();
        let mut face = invalid.records::<Face>().unwrap()[0];
        face.lightmap_size = size;
        invalid.set_records(&[face]);
        cases.push(invalid);
    }
    for mut invalid in cases {
        let before = invalid.clone();
        assert!(invalid.displacement_lightmap_samples(0).is_err());
        assert!(invalid.set_displacements(&data).is_err());
        assert_eq!(invalid, before);
    }
}

#[test]
fn compressed_sample_weights_recover_raised_sloped_surfaces_and_outward_normals() {
    let (mut bsp, mut data) = fixture();
    for (index, vertex) in data.vertices.iter_mut().enumerate() {
        vertex.direction = [0.0, 0.0, 1.0];
        vertex.distance = 8.0 + (index % 5) as f32 * 2.0;
    }
    bsp.set_displacements(&data).unwrap();
    let positions = data
        .positions(0, bsp.displacement_quad(0).unwrap())
        .unwrap();
    let triangles = data.infos[0].lightmap_triangle_indices().unwrap();
    let sample = DisplacementLightmapSample {
        triangle: 18,
        weights: [64, 63, 127],
    };
    let result = sample.surface(&triangles, &positions).unwrap().unwrap();
    let x = 4.0 * 127.0 / 254.0;
    assert!((result.position[0] - x).abs() < 1e-10);
    assert!((result.position[2] - (8.0 + x * 0.5)).abs() < 1e-10);
    let normal = glam::DVec3::from_array(result.normal);
    assert!(normal.distance(glam::DVec3::new(-0.5, 0.0, 1.0).normalize()) < 1e-10);
    assert!(
        DisplacementLightmapSample {
            triangle: 0,
            weights: [0; 3]
        }
        .surface(&triangles, &positions)
        .unwrap()
        .is_none()
    );
    assert!(sample.surface(&triangles, &[[f32::NAN; 3]; 25]).is_err());
    assert!(sample.surface(&triangles, &[[0.0; 3]; 25]).is_err());
}

#[test]
fn geometric_sampler_covers_all_triangles_without_aliasing_native_record_511() {
    use bsp::DisplacementSurfaceSampler;
    use glam::{DVec2, DVec3};
    for power in 2..=4 {
        let info = DisplacementInfo {
            power,
            ..Default::default()
        };
        let size = info.grid_size().unwrap();
        let positions = (0..size * size)
            .map(|i| {
                let x = (i % size) as f32;
                let y = (i / size) as f32;
                [x, y, 8.0 + 0.5 * x - 0.25 * y]
            })
            .collect::<Vec<_>>();
        let sampler = DisplacementSurfaceSampler::new(info, &positions).unwrap();
        let triangles = info.lightmap_triangle_indices().unwrap();
        for (index, triangle) in triangles.iter().enumerate() {
            let uv = triangle
                .iter()
                .map(|&i| DVec2::new((i % size) as f64, (i / size) as f64))
                .sum::<DVec2>()
                / (3.0 * (size - 1) as f64);
            let sample = sampler.sample(uv.to_array()).unwrap();
            assert_eq!(sample.triangle as usize, index);
            assert!(
                sample
                    .weights
                    .iter()
                    .all(|&w| (w - 1.0 / 3.0).abs() < 1e-12)
            );
            let point = DVec3::from_array(sample.surface.position);
            assert!((point.z - (8.0 + point.x * 0.5 - point.y * 0.25)).abs() < 1e-12);
            assert!(
                DVec3::from_array(sample.surface.normal)
                    .distance(DVec3::new(-0.5, 0.25, 1.0).normalize())
                    < 1e-12
            );
            if index == 511 {
                assert!(sample.lightmap_record().is_none());
            } else {
                assert_eq!(sample.lightmap_record().unwrap().triangle as usize, index);
            }
        }
        for uv in [[0.0, 0.0], [1.0, 1.0], [0.5, 0.5], [0.25, 0.75]] {
            let result = sampler.sample(uv).unwrap();
            let point = DVec3::from_array(result.surface.position);
            assert!((point.x - uv[0] * (size - 1) as f64).abs() < 1e-12);
            assert!((point.y - uv[1] * (size - 1) as f64).abs() < 1e-12);
        }
        for uv in [
            [f64::NAN, 0.0],
            [0.0, f64::INFINITY],
            [-0.1, 0.0],
            [0.0, 1.1],
        ] {
            assert!(sampler.sample(uv).is_err());
        }
        assert!(DisplacementSurfaceSampler::new(info, &positions[..positions.len() - 1]).is_err());
        assert!(DisplacementSurfaceSampler::new(info, &vec![[0.0; 3]; positions.len()]).is_err());
    }
}

#[test]
fn smoothed_terrain_normals_preserve_curvature_and_continuity_at_grid_boundaries() {
    use glam::DVec3;
    let info = DisplacementInfo::default();
    let positions = (0..25)
        .map(|i| {
            let x = (i % 5) as f32;
            let y = (i / 5) as f32;
            [x, y, (x - 2.0).powi(2) + (y - 2.0).powi(2)]
        })
        .collect::<Vec<_>>();
    let normals = info
        .vertex_normals(
            &positions
                .iter()
                .map(|p| p.map(f64::from))
                .collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(normals[12], [0.0, 0.0, 1.0]);
    assert!(DVec3::from_array(normals[11]).distance(DVec3::new(2.0, 0.0, 1.0).normalize()) < 1e-12);
    let sampler = bsp::DisplacementSurfaceSampler::new(info, &positions).unwrap();
    let center = sampler.sample_smoothed([0.5, 0.5]).unwrap();
    let flat = sampler.sample([0.5, 0.5]).unwrap();
    assert_eq!(center.surface.normal, [0.0, 0.0, 1.0]);
    assert_ne!(flat.surface.normal, center.surface.normal);
    assert_eq!(flat.surface.position, center.surface.position);
    assert_eq!(flat.lightmap_record(), center.lightmap_record());
    let left = sampler.sample_smoothed([0.5 - 1e-8, 0.5]).unwrap();
    let right = sampler.sample_smoothed([0.5 + 1e-8, 0.5]).unwrap();
    assert!(
        DVec3::from_array(left.surface.normal).distance(DVec3::from_array(right.surface.normal))
            < 1e-6
    );
    for uv in [[0.0, 0.0], [1.0, 1.0], [0.25, 0.75]] {
        let normal = DVec3::from_array(sampler.sample_smoothed(uv).unwrap().surface.normal);
        assert!((normal.length() - 1.0).abs() < 1e-12 && normal.z > 0.0);
    }
    assert!(info.vertex_normals(&[[f64::NAN; 3]; 25]).is_err());
    assert!(info.vertex_normals(&[[0.0; 3]; 24]).is_err());
}
