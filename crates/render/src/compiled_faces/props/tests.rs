use super::*;
use source_formats::{
    bsp::PakArchive,
    mdl::{ModelHeader, STATIC_PROP_FLAG},
    model::{BodyGroup, BodyVariant, HardwareSection, Part},
};

struct Fixture {
    prop: StaticProp,
    model: Model,
    layout: HardwareLayout,
    ldr: Vec<u8>,
    hdr: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let prop = StaticProp {
            origin: [100.0, 200.0, 300.0],
            angles: [0.0, 90.0, 0.0],
            lighting_origin: [-900.0; 3],
            skin: 1,
            ..Default::default()
        };
        let model = Model {
            header: ModelHeader {
                checksum: 0x1234_5678,
                flags: STATIC_PROP_FLAG,
                hull_mins: [0.0; 3],
                hull_maxs: [32.0, 16.0, 8.0],
                render_mins: [0.0; 3],
                render_maxs: [32.0, 16.0, 8.0],
                illumination: [0.0; 3],
                skins: 2,
            },
            vertices: [
                [0.0; 3],
                [32.0, 0.0, 0.0],
                [0.0, 16.0, 0.0],
                [0.0, 0.0, 8.0],
            ]
            .into_iter()
            .map(|position| source_formats::model::Vertex {
                position,
                normal: [0.0, 0.0, 1.0],
                uv: [0.25, 0.75],
            })
            .collect(),
            parts: vec![Part {
                material: 0,
                indices: vec![0, 1, 2],
            }],
            materials: vec!["unselected".into(), "selected".into()],
            material_directories: vec!["test".into()],
            skins: vec![vec![0], vec![1]],
            bounds: [[0.0; 3], [32.0, 16.0, 8.0]],
            body_parts: 1,
            reference_bones: 0,
            body_groups: vec![BodyGroup {
                name: "body".into(),
                base: 1,
                variants: vec![BodyVariant {
                    name: "static".into(),
                    parts: 0..1,
                    bounds: None,
                }],
            }],
            attachments: Vec::new(),
        };
        let layout = HardwareLayout {
            checksum: model.header.checksum,
            root_lod: 0,
            lods: 2,
            vertices: vec![0, 2, 1, 3, 1, 2, 0, 3, 0, 1, 2, 0, 3, 2, 1, 0, 3],
            sections: [(0, 0, 8..13), (0, 1, 13..17), (1, 0, 0..4), (1, 1, 4..8)]
                .into_iter()
                .enumerate()
                .map(|(index, (lod, group, vertices))| HardwareSection {
                    body_part: 0,
                    model: 0,
                    lod,
                    mesh: 0,
                    group,
                    part: 0,
                    vertices,
                    triangle_indices: if index == 0 {
                        vec![0, 1, 2, 3, 2, 1]
                    } else {
                        vec![0, 1, 2]
                    },
                })
                .collect(),
        };
        let meshes = layout
            .sections
            .iter()
            .enumerate()
            .map(|(section, hardware)| {
                let colors = (0..hardware.vertices.len())
                    .map(|vertex| Bgra8 {
                        blue: [255, 128, 64, 0, 36][vertex],
                        green: (section * 32) as u8,
                        red: [0, 64, 128, 255, 239][vertex],
                        alpha: (7 + vertex) as u8,
                    })
                    .collect();
                vhv::Mesh {
                    layout: vhv::MeshLayout {
                        lod: hardware.lod as u32,
                        vertex_count: hardware.vertices.len() as u32,
                    },
                    colors,
                }
            })
            .collect();
        let mut file = vhv::File {
            checksum: model.header.checksum as u32,
            meshes,
        };
        let ldr = file.to_bytes(vhv::Limits::default()).unwrap();
        for mesh in &mut file.meshes {
            for color in &mut mesh.colors {
                std::mem::swap(&mut color.red, &mut color.green);
            }
        }
        let hdr = file.to_bytes(vhv::Limits::default()).unwrap();
        Self {
            prop,
            model,
            layout,
            ldr,
            hdr,
        }
    }

    fn input(&self, ordinal: u32) -> Input<'_> {
        Input {
            ordinal,
            model_name: "models/test/occurrences.mdl",
            prop: &self.prop,
            model: &self.model,
            layout: &self.layout,
            materials: &[0; 4],
            channels: [Some(&self.ldr), Some(&self.hdr)],
        }
    }
}

fn mesh(settings: &LightingSettings) -> SceneMesh {
    let game = source_formats::gamefs::GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let mut mesh = SceneMesh::build(
        map_core::Document::empty().vmf(),
        &game,
        &mut crate::MaterialLibrary::default(),
        &Default::default(),
    );
    mesh.baked_faces = Some(Arc::new(super::super::LightingData {
        samples: Vec::new(),
        faces: Vec::new(),
        exposure_ev: settings.exposure_ev as f32,
        channel: settings.channel,
        effective_style_gains: settings.style_gains.clone(),
    }));
    mesh
}

fn material() -> Arc<Material> {
    Arc::new(
        super::super::materials::load_static_prop_resolved(
            "test/selected",
            &source_formats::vmt::Material::parse("VertexLitGeneric {}").unwrap(),
            &PakArchive::default(),
            &mut |_| Ok(None),
            &mut || Ok(()),
            &mut super::super::materials::Budget {
                max_asset_bytes: 1024,
                max_texture_axis: 16,
                max_decoded_bytes: 1024,
                decoded_bytes: 0,
            },
        )
        .unwrap(),
    )
}

#[test]
fn all_lod_geometry_joins_each_literal_hardware_colour_before_interpolation() {
    let fixture = Fixture::new();
    let mut scene = Scene::new(vec![LightingChannel::Ldr, LightingChannel::Hdr]).unwrap();
    scene
        .push(fixture.input(7), MAX_CPU_BYTES, &mut || Ok(()))
        .unwrap();
    assert_eq!(scene.props()[0].skin, 1);
    assert_eq!(scene.props()[0].lods, [0, 1]);
    assert_eq!(scene.props()[0].vertices, 17);
    assert_eq!(scene.props()[0].triangles, 5);
    assert_eq!(scene.vertices[0].position, [100.0, 200.0, 300.0]);
    assert_eq!(scene.vertices[0].position, scene.vertices[3].position);
    assert_eq!(scene.vertices[4].position, [100.0, 200.0, 308.0]);
    assert_eq!(scene.vertices[0].uv, [0.25, 0.75]);
    assert_eq!(
        scene.bounds(0, Some(7), |_| true).unwrap(),
        Some([[84.0, 200.0, 300.0], [100.0, 232.0, 308.0]])
    );
    let settings = LightingSettings {
        exposure_ev: 1.0,
        style_gains: vec![super::super::StyleGain {
            style: 0,
            gain: 0.0,
        }],
        ..Default::default()
    };
    let black = scene.vertex_sample(7, 0, 0, &settings).unwrap();
    let duplicate = scene.vertex_sample(7, 0, 3, &settings).unwrap();
    let midpoint = scene.vertex_sample(7, 0, 2, &settings).unwrap();
    assert_eq!(black.bgra, [255, 0, 0, 7]);
    assert_eq!(duplicate.bgra, [0, 0, 255, 10]);
    assert_eq!(black.decoded_linear[0], 0.0);
    assert!((duplicate.decoded_linear[0] - 4.59479357).abs() < 1e-7);
    assert!((midpoint.decoded_linear[0] - 1.00864776).abs() < 1e-7);
    let interpolated = (black.decoded_linear[0] + duplicate.decoded_linear[0]) / 2.0;
    assert!((interpolated - midpoint.decoded_linear[0]).abs() > 1.28);
    assert_eq!(duplicate.pre_ev, duplicate.decoded_linear);
    assert_eq!(duplicate.exposed, duplicate.pre_ev.map(|value| value * 2.0));
    let hdr = scene
        .vertex_sample(
            7,
            1,
            0,
            &LightingSettings {
                channel: LightingChannel::Hdr,
                ..settings.clone()
            },
        )
        .unwrap();
    assert_eq!(hdr.bgra, [255, 0, 32, 7]);
    assert_ne!(
        hdr.decoded_linear,
        scene
            .vertex_sample(7, 1, 0, &settings)
            .unwrap()
            .decoded_linear
    );
    let materials = [Some(material())];
    let mut output = mesh(&settings);
    output.baked_face_materials.push(None);
    scene
        .append_prepared_filtered(&mut output, &materials, &settings, 0, 0, |_| true)
        .unwrap();
    assert_eq!(output.vertices.len(), 9);
    assert_eq!(output.indices, [0, 1, 2, 3, 2, 1, 5, 6, 7]);
    assert_eq!(output.wire_lines.len(), 18);
    assert_eq!(output.batches[0].material, 1);
    assert_eq!(output.baked_face_batches, 0..2);
    assert_eq!(output.vertices[3].lighting_receiver, BAKED_PROP_RECEIVER);
    assert_eq!(
        output.vertices[3].baked_irradiance,
        duplicate.decoded_linear.map(|value| value as f32)
    );
    assert_eq!(output.vertices[3].opacity, 1.0);
    let mut lower = mesh(&settings);
    scene
        .append_prepared_filtered(&mut lower, &materials, &settings, 1, 0, |_| true)
        .unwrap();
    assert_eq!(lower.vertices.len(), 8);
    assert_eq!(lower.indices, [0, 1, 2, 4, 5, 6]);
    assert_eq!(lower.vertices[1].position, [84.0, 200.0, 300.0]);
    let mut excluded = mesh(&settings);
    scene
        .append_prepared_filtered(&mut excluded, &materials, &settings, 0, 0, |_| false)
        .unwrap();
    assert!(
        excluded.vertices.is_empty()
            && excluded.indices.is_empty()
            && excluded.wire_lines.is_empty()
    );
    assert_eq!(
        scene.draw_lod_sample(7, 1, &settings).unwrap().bgra,
        [255, 64, 0, 7]
    );
    scene.sections[0].indices.end = scene.sections[0].indices.start;
    let mut sparse = mesh(&settings);
    scene
        .append_prepared_filtered(&mut sparse, &materials, &settings, 0, 0, |_| true)
        .unwrap();
    assert_eq!(sparse.vertices.len(), output.vertices.len());
    assert_eq!(
        sparse.vertices[3].baked_irradiance,
        output.vertices[3].baked_irradiance
    );
    assert_eq!(sparse.indices, [5, 6, 7]);
    assert_eq!(sparse.baked_face_batches, 0..1);
    assert!(sparse.batches.iter().all(|batch| batch.index_count > 0));
}

#[test]
fn absent_channel_lod_identity_and_combined_budget_refuse_without_geometry_substitution() {
    let mut fixture = Fixture::new();
    let mut scene = Scene::new(vec![LightingChannel::Ldr, LightingChannel::Hdr]).unwrap();
    assert!(scene.push(fixture.input(7), 1, &mut || Ok(())).is_err());
    assert!(scene.props().is_empty() && scene.vertices.is_empty());
    fixture.hdr[4] ^= 1;
    assert!(
        scene
            .push(fixture.input(7), MAX_CPU_BYTES, &mut || Ok(()))
            .is_err()
    );
    assert!(scene.props().is_empty() && scene.vertices.is_empty());
    fixture.hdr[4] ^= 1;
    scene
        .push(fixture.input(7), MAX_CPU_BYTES, &mut || Ok(()))
        .unwrap();
    assert!(
        scene
            .validate_settings(&LightingSettings::default(), 2)
            .is_err()
    );
    assert!(scene.bounds(2, Some(7), |_| true).is_err());
    assert!(
        scene
            .push(fixture.input(7), MAX_CPU_BYTES, &mut || Ok(()))
            .is_err()
    );
    let materials = [Some(material())];
    let mut output = mesh(&LightingSettings::default());
    assert!(
        scene
            .append_prepared_filtered(
                &mut output,
                &materials,
                &LightingSettings::default(),
                0,
                MAX_CPU_BYTES,
                |_| true
            )
            .is_err()
    );
    assert!(output.vertices.is_empty() && output.indices.is_empty());
    let mut ldr_only = Scene::new(vec![LightingChannel::Ldr]).unwrap();
    let mut input = fixture.input(7);
    input.channels[1] = None;
    ldr_only.push(input, MAX_CPU_BYTES, &mut || Ok(())).unwrap();
    assert!(
        ldr_only
            .validate_settings(
                &LightingSettings {
                    channel: LightingChannel::Hdr,
                    ..Default::default()
                },
                0
            )
            .is_err()
    );
}
