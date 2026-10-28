use super::*;
use source_formats::bsp::{Edge, Face, Model, Plane};

fn face_fixture(model: bool) -> (Bsp, Vec<DMat4>) {
    let mut bsp = Bsp::new();
    bsp.set_records(&[Plane {
        normal: [0.0, 0.0, 1.0],
        dist: 0.0,
        kind: 2,
    }]);
    bsp.set_values(
        lump::VERTEXES,
        &[
            [0.0_f32, 0.0, 0.0],
            [0.0, 16.0, 0.0],
            [32.0, 16.0, 0.0],
            [32.0, 0.0, 0.0],
        ],
    );
    bsp.set_records(&[Edge([0, 1]), Edge([1, 2]), Edge([2, 3]), Edge([3, 0])]);
    bsp.set_values(lump::SURFEDGES, &[0_i32, 1, 2, 3]);
    bsp.set_values(lump::VERTNORMALS, &[[0.0_f32, 0.0, 1.0]]);
    bsp.set_values(lump::VERTNORMALINDICES, &[0_u16; 4]);
    bsp.set_records(&[TexInfo {
        texture_vecs: [[2.0, 0.0, 0.0, 8.0], [0.0, -4.0, 0.0, 20.0]],
        lightmap_vecs: [[1.0 / 16.0, 0.0, 0.0, 3.0], [0.0, 1.0 / 16.0, 0.0, -2.0]],
        flags: surf::BUMPLIGHT,
        texdata: 0,
    }]);
    bsp.set_records(&[TexData {
        width: 64,
        height: 128,
        ..Default::default()
    }]);
    bsp.set_texdata_strings(&["test/compiled"]);
    let face = Face {
        edge_count: 4,
        dispinfo: -1,
        fog_volume: -1,
        styles: [0, 7, 255, 255],
        light_offset: 8,
        area: 512.0,
        lightmap_mins: [3, -2],
        lightmap_size: [2, 1],
        original_face: -1,
        ..Default::default()
    };
    for (kind, exponent) in [(LightingKind::Ldr, -1), (LightingKind::Hdr, 2)] {
        bsp.set_lighting_faces(kind, &[face]).unwrap();
        let mut samples = vec![RgbExp32::default(); 2];
        for style in 0..2 {
            for map in 0..4 {
                for index in 0..6 {
                    samples.push(RgbExp32 {
                        rgb: [32 + style * 64 + map * 8 + index, 16 + index, 8],
                        exponent,
                    });
                }
            }
        }
        bsp.set_lighting(kind, &samples).unwrap();
    }
    let mut models = vec![Model {
        face_count: if model { 0 } else { 1 },
        ..Default::default()
    }];
    let mut transforms = vec![DMat4::IDENTITY];
    if model {
        models.push(Model {
            face_count: 1,
            ..Default::default()
        });
        transforms.push(DMat4::from_rotation_translation(
            glam::DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
            DVec3::new(100.0, 200.0, 300.0),
        ));
    }
    bsp.set_records(&models);
    (bsp, transforms)
}

#[test]
fn final_faces_preserve_local_uv_luxels_and_directional_frame_through_model_placement() {
    for model in [false, true] {
        let (bsp, transforms) = face_fixture(model);
        let scene = Scene::decode(&bsp, &transforms).unwrap();
        let channel = scene.channel(LightingChannel::Hdr).unwrap();
        assert_eq!(channel.indices, [0, 1, 2, 0, 2, 3]);
        assert_eq!(channel.vertices[0].uv, [0.125, 0.15625]);
        assert_eq!(channel.vertices[1].uv, [0.125, -0.34375]);
        assert_eq!(channel.vertices[2].lightmap, [2.0, 1.0, 16.0]);
        assert_eq!(channel.vertices[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(channel.vertices[0].lighting_receiver, BAKED_FACE_RECEIVER);
        assert_eq!(channel.vertices[0].tangent[3], -1.0);
        let expected = if model {
            [100.0, 200.0, 300.0]
        } else {
            [0.0; 3]
        };
        assert_eq!(channel.vertices[0].position, expected);
        let tangent = Vec3::from_slice(&channel.vertices[0].tangent[..3]);
        assert!(tangent.distance(if model { Vec3::Y } else { Vec3::X }) < 1e-6);
        assert_eq!(channel.surfaces[0].model, usize::from(model));
        assert_eq!(scene.materials()[0].dimensions, [64, 128]);
        assert_eq!(channel.wires.len(), 8);
        assert_eq!(channel.surfaces[0].source, LightingSource::BakedLightmap);
    }
    let (mut bsp, transforms) = face_fixture(false);
    bsp.set_values(lump::VERTNORMALS, &[[1.0_f32, 0.0, 0.0]]);
    assert!(Scene::decode(&bsp, &transforms).is_err());
    let mut info = bsp.values::<TexInfo>(lump::TEXINFO).unwrap();
    info[0].flags &= !surf::BUMPLIGHT;
    bsp.set_records(&info);
    let scene = Scene::decode(&bsp, &transforms).unwrap();
    let channel = scene.channel(LightingChannel::Ldr).unwrap();
    assert_eq!(channel.vertices[0].normal, [1.0, 0.0, 0.0]);
    assert_eq!(channel.vertices[0].tangent, [0.0; 4]);
}

#[test]
fn independent_channels_composite_stored_ids_and_all_maps_before_exposure_without_clamping_hdr() {
    let (bsp, transforms) = face_fixture(false);
    let scene = Scene::decode(&bsp, &transforms).unwrap();
    let settings = LightingSettings {
        channel: LightingChannel::Hdr,
        exposure_ev: 1.0,
        style_gains: vec![
            StyleGain {
                style: 0,
                gain: 0.5,
            },
            StyleGain {
                style: 7,
                gain: 2.0,
            },
        ],
    };
    let hdr = scene.lighting(&settings).unwrap();
    assert_eq!(hdr.faces[0].map_offsets, [0, 6, 12, 18]);
    assert_eq!(hdr.exposure_ev, 1.0);
    assert!((hdr.samples[0][0] - 832.0 / 255.0).abs() < 1e-6);
    assert!((hdr.samples[23][0] - 1122.0 / 255.0).abs() < 1e-6);
    assert!(hdr.samples.iter().any(|sample| sample[0] > 1.0));
    let ldr = scene
        .lighting(&LightingSettings {
            channel: LightingChannel::Ldr,
            ..settings.clone()
        })
        .unwrap();
    assert!((hdr.samples[17][0] / ldr.samples[17][0] - 8.0).abs() < 1e-6);
    let zero = scene
        .lighting(&LightingSettings {
            style_gains: vec![StyleGain {
                style: 0,
                gain: 0.0,
            }],
            ..Default::default()
        })
        .unwrap();
    assert!(zero.samples.iter().all(|sample| *sample == [0.0; 4]));
    for gains in [
        vec![StyleGain {
            style: 3,
            gain: 1.0,
        }],
        vec![
            StyleGain {
                style: 0,
                gain: 1.0,
            },
            StyleGain {
                style: 0,
                gain: 2.0,
            },
        ],
        vec![StyleGain {
            style: 7,
            gain: f64::NAN,
        }],
    ] {
        assert!(
            scene
                .lighting(&LightingSettings {
                    style_gains: gains,
                    ..Default::default()
                })
                .is_err()
        );
    }
    let mut absent = bsp.clone();
    absent.set_lighting_faces(LightingKind::Hdr, &[]).unwrap();
    absent.set_lighting(LightingKind::Hdr, &[]).unwrap();
    let scene = Scene::decode(&absent, &transforms).unwrap();
    assert!(scene.lighting(&settings).is_err());
    assert!(scene.lighting(&LightingSettings::default()).is_ok());

    let mut unlit = bsp;
    let mut infos = unlit.records::<TexInfo>().unwrap();
    infos[0].flags = surf::NOLIGHT;
    unlit.set_records(&infos);
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let mut faces = unlit.lighting_faces(kind).unwrap();
        faces[0].light_offset = -1;
        faces[0].styles = [255; 4];
        unlit.set_lighting_faces(kind, &faces).unwrap();
        unlit.set_lighting(kind, &[]).unwrap();
    }
    let scene = Scene::decode(&unlit, &transforms).unwrap();
    assert!(!scene.materials()[0].lightmapped);
    let surface = &scene.surfaces(LightingChannel::Ldr).unwrap()[0];
    assert_eq!(surface.source, LightingSource::UnlitMaterial);
    let data = scene.lighting(&LightingSettings::default()).unwrap();
    assert!(data.samples.is_empty());
    assert_eq!(data.faces[0].map_count, 0);
    assert_eq!([data.faces[0].width, data.faces[0].height], [0; 2]);
    let channel = scene.channel(LightingChannel::Ldr).unwrap();
    assert_eq!(channel.indices, [0, 1, 2, 0, 2, 3]);
    assert_eq!(channel.wires.len(), 8);
    assert!(
        channel
            .vertices
            .iter()
            .all(|vertex| vertex.lightmap[2] == -1.0)
    );
}

#[test]
fn terrain_uses_start_oriented_grid_allowed_topology_and_retained_surface_sample_locations() {
    use source_formats::bsp::{
        DisplacementInfo, DisplacementLightmapSample, DisplacementSurfaceSampler,
        DisplacementTriangle, DisplacementVertex, Displacements,
    };

    for power in 2..=4 {
        let (mut bsp, transforms) = face_fixture(false);
        let mut info = DisplacementInfo {
            start: [32.0, 16.0, 0.0],
            power,
            ..Default::default()
        };
        info.allowed_vertices[0] &= !(1 << 1);
        let size = info.grid_size().unwrap();
        let center = size * (size / 2) + size / 2;
        let lateral = center + size;
        let vertices = (0..size * size)
            .map(|index| DisplacementVertex {
                direction: if index == lateral {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 0.0, 1.0]
                },
                distance: if index == center {
                    8.0
                } else if index == lateral {
                    1.0
                } else {
                    0.0
                },
                alpha: if index == center { 128.25 } else { 0.0 },
            })
            .collect();
        let data = Displacements {
            infos: vec![info],
            vertices,
            triangles: vec![DisplacementTriangle::default(); info.triangle_count().unwrap()],
        };
        let quad = bsp.vertices().unwrap().try_into().unwrap();
        let positions = data.positions(0, quad).unwrap();
        let sampler = DisplacementSurfaceSampler::new(info, &positions).unwrap();
        let sentinel = DisplacementLightmapSample {
            triangle: 0,
            weights: [0; 3],
        };
        let mut records = (0..25)
            .map(|index| {
                sampler
                    .sample([(index % 5) as f64 / 4.0, (index / 5) as f64 / 4.0])
                    .unwrap()
                    .lightmap_record()
                    .unwrap_or(sentinel)
            })
            .collect::<Vec<_>>();
        records[0] = sentinel;
        let encoded = info.encode_lightmap_samples(&records).unwrap();
        bsp.set_values(lump::DISP_LIGHTMAP_SAMPLE_POSITIONS, &encoded);
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            let mut faces = bsp.lighting_faces(kind).unwrap();
            faces[0].dispinfo = 0;
            faces[0].lightmap_size = [4; 2];
            bsp.set_lighting_faces(kind, &faces).unwrap();
            bsp.set_lighting(
                kind,
                &vec![
                    RgbExp32 {
                        rgb: [128, 64, 32],
                        exponent: 1
                    };
                    202
                ],
            )
            .unwrap();
        }
        bsp.set_displacements(&data).unwrap();
        let scene = Scene::decode(&bsp, &transforms).unwrap();
        let channel = scene.channel(LightingChannel::Hdr).unwrap();
        assert_eq!(channel.vertices[0].position, [32.0, 16.0, 0.0]);
        assert_eq!(channel.vertices[0].lightmap[..2], [0.0; 2]);
        assert_eq!(channel.vertices[center].position, [16.0, 8.0, 8.0]);
        assert_eq!(channel.vertices[lateral].position[0], 17.0);
        assert_eq!(
            channel.vertices[lateral].uv,
            [0.625, [0.03125, -0.03125, -0.0625][power as usize - 2]]
        );
        assert_eq!(channel.vertices[center].lightmap[..2], [2.0, 2.0]);
        assert!(channel.vertices[center].lightmap[2] > 0.0);
        assert_eq!(channel.vertices[center].color[3], 128);
        assert_eq!(channel.vertices[center].opacity, 128.25 / 255.0);
        assert!(!channel.indices.contains(&1));
        assert!(channel.surfaces[0].missing_surface_samples >= 1);
        assert_eq!(
            channel.surfaces[0].triangles,
            info.allowed_triangle_indices().unwrap().len()
        );
        records[0] = records[12];
        let encoded = info.encode_lightmap_samples(&records).unwrap();
        bsp.set_values(lump::DISP_LIGHTMAP_SAMPLE_POSITIONS, &encoded);
        assert!(Scene::decode(&bsp, &transforms).is_err());
    }
}

#[test]
fn invalid_material_or_sample_ranges_preserve_scene_and_region_filters_leave_full_maps_retained() {
    let (bsp, transforms) = face_fixture(false);
    let scene = Scene::decode(&bsp, &transforms).unwrap();
    let fs = source_formats::gamefs::GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let mut mesh = SceneMesh::build(
        map_core::Document::empty().vmf(),
        &fs,
        &mut crate::MaterialLibrary::default(),
        &Default::default(),
    );
    let lighting = scene.lighting(&LightingSettings::default()).unwrap();
    let original = (
        mesh.vertices.len(),
        mesh.indices.len(),
        mesh.batches.len(),
        mesh.mins,
        mesh.maxs,
    );
    assert!(
        scene
            .append_prepared(&mut mesh, &[None], lighting.clone())
            .is_err()
    );
    assert_eq!(
        (
            mesh.vertices.len(),
            mesh.indices.len(),
            mesh.batches.len(),
            mesh.mins,
            mesh.maxs
        ),
        original
    );
    let material = materials::Material {
        name: "test/compiled".into(),
        kind: materials::Kind::Single,
        diffuse: [None, None],
        normals: [None, None],
        diffuse_transform: [[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 2],
        normal_transform: [[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 2],
        color: [1.0; 3],
        detail_alpha_mask: false,
        lighting: materials::MaterialLighting::Lightmap { directional: true },
        coverage: materials::Coverage::Opaque,
        nocull: false,
    };
    let material = [Some(Arc::new(material))];
    let mut invalid = scene
        .channel(LightingChannel::Ldr)
        .unwrap()
        .compose(&LightingSettings::default(), 0)
        .unwrap();
    invalid.faces[0].map_offsets[3] = u32::MAX;
    assert!(
        scene
            .append_prepared(&mut mesh, &material, Arc::new(invalid))
            .is_err()
    );
    assert_eq!(
        (
            mesh.vertices.len(),
            mesh.indices.len(),
            mesh.batches.len(),
            mesh.mins,
            mesh.maxs
        ),
        original
    );
    scene
        .append_prepared_filtered(&mut mesh, &material, lighting.clone(), |surface| {
            surface.mins[0] >= 100.0
        })
        .unwrap();
    assert!(mesh.indices.is_empty());
    assert_eq!(mesh.baked_face_visible_surfaces, 0);
    assert_eq!(mesh.baked_faces.as_ref().unwrap().samples.len(), 24);
    assert!(Arc::ptr_eq(mesh.baked_faces.as_ref().unwrap(), &lighting));
}
