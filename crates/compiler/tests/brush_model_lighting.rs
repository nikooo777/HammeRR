mod common;

use compiler::{LightingMode, Options};
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, LightingKind, Model, TexInfo};
use source_formats::vmf::Vmf;

fn fixture(pivot: Option<DVec3>) -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("model lighting", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 32.0),
                DVec3::new(32.0, 32.0, 96.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            if let Some(pivot) = pivot {
                edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[("origin".into(), source_formats::vmf::format_vec3(pivot))],
                )?;
            }
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(-96.0, -96.0, 224.0)),
                &[
                    ("_light".into(), "255 255 255 150".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn built(vmf: &Vmf) -> compiler::Output {
    let result = compiler::compile_with_options(
        vmf,
        &common::empty_game(),
        Options {
            lighting: LightingMode::DirectLdr,
            ..Default::default()
        },
    );
    assert!(!result.report.has_errors(), "{:?}", result.report);
    result
}

fn top_sample(bsp: &Bsp, model: usize, pivot: DVec3) -> DVec3 {
    sample_at(bsp, model, pivot, DVec3::new(0.0, 0.0, 96.0), 0)
}

fn sample_at(bsp: &Bsp, model: usize, pivot: DVec3, point: DVec3, style: usize) -> DVec3 {
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.records::<Face>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    let target = point - pivot;
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || (f64::from(plane.dist) - target.z).abs() > 1e-6 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::from_array(plane.normal.map(f64::from)),
                dist: f64::from(plane.dist),
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(target) > 1e-6 {
                    continue;
                }
                let colors = lighting
                    .face(face, info)
                    .unwrap()
                    .expect("Brush-model face has no lightmap");
                return DVec3::from_array(
                    colors.samples(style, 0).unwrap()[t * width + s].linear(),
                );
            }
        }
    }
    panic!("No top receiver for model")
}

#[test]
fn rendered_models_receive_world_space_lighting_at_translated_and_fractional_pivots() {
    let world = built(&fixture(None)).bsp.unwrap();
    let expected = top_sample(&world, 0, DVec3::ZERO);
    assert!(expected.min_element() > 0.0);
    for pivot in [
        DVec3::ZERO,
        DVec3::new(128.0, -64.0, 32.0),
        DVec3::new(101.5, -67.25, 32.125),
    ] {
        let source = fixture(Some(pivot));
        let before = source.clone();
        let output = built(&source);
        assert_eq!(output.stats.lit_brush_models, 1);
        assert_eq!(output.stats.model_lighting_faces, 6);
        let bsp = output.bsp.unwrap();
        assert_eq!(top_sample(&bsp, 1, pivot), expected);
        let unlit = compiler::compile(&source, &common::empty_game())
            .bsp
            .unwrap();
        for lump in [
            bsp::lump::PLANES,
            bsp::lump::VERTEXES,
            bsp::lump::EDGES,
            bsp::lump::SURFEDGES,
            bsp::lump::MODELS,
            bsp::lump::BRUSHES,
            bsp::lump::BRUSHSIDES,
            bsp::lump::NODES,
            bsp::lump::LEAFS,
            bsp::lump::LEAFBRUSHES,
        ] {
            assert!(
                bsp.lumps[lump] == unlit.lumps[lump],
                "Lighting changed geometry lump {lump}"
            );
        }
        let bounced = compiler::compile_with_options(
            &source,
            &common::empty_game(),
            Options {
                lighting: LightingMode::DirectLdr,
                bounces: 1,
                ..Default::default()
            },
        );
        assert!(!bounced.report.has_errors(), "{:?}", bounced.report);
        assert_eq!(top_sample(&bounced.bsp.unwrap(), 1, pivot), expected);
        assert_eq!(source, before);
    }
}

#[test]
fn compound_models_self_occlude_without_baking_permanent_world_shadows() {
    let pivot = DVec3::new(128.0, -64.0, 16.0);
    let mut document = Document::new(common::room());
    document
        .edit("compound", |tx| {
            let mut ids = Vec::new();
            for (low, high) in [(32.0, 48.0), (128.0, 144.0)] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-64.0, -64.0, low),
                    DVec3::new(64.0, 64.0, high),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                ids.push(id);
            }
            edit::tie_to_entity(
                tx,
                &ids,
                "func_brush",
                &[("origin".into(), source_formats::vmf::format_vec3(pivot))],
            )?;
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 224.0)),
                &[
                    ("_light".into(), "255 255 255 150".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    let bsp = built(&source).bsp.unwrap();
    assert_eq!(
        sample_at(&bsp, 1, pivot, DVec3::new(0.0, 0.0, 48.0), 0),
        DVec3::ZERO
    );
    assert_eq!(
        sample_at(&bsp, 1, pivot, DVec3::new(0.0, 0.0, 144.0), 0),
        DVec3::splat(150.0)
    );
    assert_eq!(
        sample_at(&bsp, 0, DVec3::ZERO, DVec3::ZERO, 0),
        DVec3::splat(150.0)
    );
    assert_eq!(*document.vmf(), source);
}

#[test]
fn named_style_channels_follow_model_receiver_coordinates() {
    let pivot = DVec3::new(101.5, -67.25, 32.125);
    let source = fixture(Some(pivot));
    let baseline = built(&source).bsp.unwrap();
    let expected = top_sample(&baseline, 1, pivot);
    let mut styled = source.clone();
    styled
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("targetname", "model_light");
    let styled = built(&styled).bsp.unwrap();
    assert_eq!(top_sample(&styled, 1, pivot), DVec3::ZERO);
    assert_eq!(
        sample_at(&styled, 1, pivot, DVec3::new(0.0, 0.0, 96.0), 1),
        expected
    );
}

#[test]
fn authored_model_groups_bake_checked_smooth_corner_normals() {
    let pivot = DVec3::new(101.5, -67.25, 32.125);
    let mut source = fixture(Some(pivot));
    for side in source
        .entities
        .iter_mut()
        .find(|entity| !entity.solids.is_empty())
        .unwrap()
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.smoothing_groups = Some(1);
    }
    let output = built(&source);
    assert_eq!(output.stats.smoothed_faces, 6);
    let bsp = output.bsp.unwrap();
    let normals = model_corner_normals(&bsp, 1, DVec3::new(32.0, 32.0, 96.0) - pivot);
    assert_eq!(normals.len(), 3);
    assert!(
        normals
            .iter()
            .all(|normal| normal.distance(DVec3::ONE.normalize()) < 2e-7)
    );
    let point = DVec3::new(16.0, 0.0, 96.0);
    let expected = sample_at(&bsp, 1, pivot, point, 0);
    for side in source
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 1;
    }
    let divided = built(&source).bsp.unwrap();
    assert_eq!(sample_at(&divided, 1, pivot, point, 0), expected);
}

fn model_corner_normals(bsp: &Bsp, model: usize, target: DVec3) -> Vec<DVec3> {
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.records::<Face>().unwrap();
    let vertices = bsp.vertices().unwrap();
    let edges = bsp.records::<bsp::Edge>().unwrap();
    let surfedges = bsp.surfedges().unwrap();
    let indices = bsp.values::<u16>(bsp::lump::VERTNORMALINDICES).unwrap();
    let normals = bsp.values::<[f32; 3]>(bsp::lump::VERTNORMALS).unwrap();
    let mut result = Vec::new();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        for index in face.first_edge as usize..face.first_edge as usize + face.edge_count as usize {
            let edge = surfedges[index];
            let vertex = edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)];
            if DVec3::from_array(vertices[vertex as usize].map(f64::from)).distance(target) < 1e-6 {
                let normal = DVec3::from_array(normals[indices[index] as usize].map(f64::from));
                result.push(normal);
            }
        }
    }
    result
}

#[test]
fn model_scopes_keep_coincident_local_vertices_separate_and_respect_hard_and_angle_rules() {
    let mut document = Document::new(common::room());
    let mut marked = Vec::new();
    for (x, normal) in [(-128.0, DVec3::Z), (128.0, DVec3::X)] {
        let pivot = DVec3::new(x, 0.0, 64.0);
        let id = document
            .edit("separate models", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    pivot - DVec3::splat(32.0),
                    pivot + DVec3::splat(32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[("origin".into(), source_formats::vmf::format_vec3(pivot))],
                )
            })
            .unwrap();
        marked.push((id, normal));
    }
    let mut source = document.vmf().clone();
    for (id, normal) in marked {
        let ObjectId::Entity(id) = id else {
            unreachable!()
        };
        let side = source
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap()
            .solids[0]
            .sides
            .iter_mut()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal == normal)
            .unwrap();
        side.smoothing_groups = Some(1);
    }
    let output = built(&source);
    assert_eq!(output.stats.smoothed_faces, 0);
    let bsp = output.bsp.unwrap();
    for model in [1, 2] {
        let normals = model_corner_normals(&bsp, model, DVec3::splat(32.0));
        assert_eq!(normals.len(), 3);
        assert!(normals.iter().all(|normal| {
            [DVec3::X, DVec3::Y, DVec3::Z]
                .iter()
                .any(|axis| normal.distance(*axis) < 2e-7)
        }));
    }
    let pivot = DVec3::new(101.5, -67.25, 32.125);
    let mut source = fixture(Some(pivot));
    for side in source
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .flat_map(|solid| &mut solid.sides)
    {
        side.smoothing_groups = Some(1 | (1 << 24));
    }
    assert_eq!(built(&source).stats.smoothed_faces, 0);
    let source = fixture(Some(pivot));
    for (angle, smooth) in [(89, false), (90, true)] {
        let result = compiler::compile_with_options(
            &source,
            &common::empty_game(),
            Options {
                lighting: LightingMode::DirectLdr,
                smoothing_angle: angle,
                ..Default::default()
            },
        );
        assert!(!result.report.has_errors(), "{:?}", result.report);
        let normals = model_corner_normals(
            &result.bsp.unwrap(),
            1,
            DVec3::new(32.0, 32.0, 96.0) - pivot,
        );
        assert_eq!(normals.len(), 3);
        assert_eq!(
            normals
                .iter()
                .all(|normal| normal.distance(DVec3::ONE.normalize()) < 2e-7),
            smooth
        );
    }
}
