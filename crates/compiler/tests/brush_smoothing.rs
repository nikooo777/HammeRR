mod common;

use compiler::{LightingMode, Options};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{Bsp, Edge, Face, LightingKind, TexInfo, lump};
use source_formats::vmf::Vmf;

fn fixture() -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    let ObjectId::Solid(id) = document
        .edit("cube", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 32.0),
                DVec3::new(64.0, 64.0, 160.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap()
    else {
        unreachable!();
    };
    document
        .edit("light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(-128.0, -128.0, 200.0)),
                &[
                    ("_light".into(), "255 255 255 200".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    (document.vmf().clone(), id)
}

fn paint(vmf: &mut Vmf, id: u32, masks: &[u32; 6], scale: Option<u32>) {
    let solid = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap();
    for (side, &mask) in solid.sides.iter_mut().zip(masks) {
        side.smoothing_groups = Some(mask);
        if let Some(scale) = scale {
            side.lightmap_scale = scale;
        }
    }
}

fn output(vmf: &Vmf) -> compiler::Output {
    let output = compiler::compile_with_options(
        vmf,
        &common::empty_game(),
        Options {
            lighting: LightingMode::DirectLdr,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    output
}

fn corner_normals(bsp: &Bsp, point: DVec3) -> Vec<DVec3> {
    let vertices = bsp.vertices().unwrap();
    let edges = bsp.records::<Edge>().unwrap();
    let surfedges = bsp.surfedges().unwrap();
    let indices = bsp.values::<u16>(lump::VERTNORMALINDICES).unwrap();
    let normals = bsp.values::<[f32; 3]>(lump::VERTNORMALS).unwrap();
    let mut result = Vec::new();
    for face in bsp.records::<Face>().unwrap() {
        if face.smoothing_groups == 0 {
            continue;
        }
        for index in face.first_edge as usize..face.first_edge as usize + face.edge_count as usize {
            let edge = surfedges[index];
            let vertex = edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)] as usize;
            if DVec3::from_array(vertices[vertex].map(f64::from)).distance(point) < 1e-9 {
                result.push(DVec3::from_array(
                    normals[indices[index] as usize].map(f64::from),
                ));
            }
        }
    }
    result
}

fn top_sample(bsp: &Bsp, point: DVec3) -> DVec3 {
    let planes = bsp.records::<source_formats::bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let light = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in bsp.records::<Face>().unwrap() {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || (f64::from(plane.dist) - 160.0).abs() > 0.001 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            map_core::Plane {
                normal: DVec3::Z,
                dist: 160.0,
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(point) < 1e-6 {
                    return DVec3::from_array(
                        light
                            .face(&face, info)
                            .unwrap()
                            .unwrap()
                            .samples(0, 0)
                            .unwrap()[t * width + s]
                            .linear(),
                    );
                }
            }
        }
    }
    panic!("Missing top sample at {point}")
}

#[test]
fn shared_masks_bake_checked_corner_normals_and_preserve_geometry() {
    let (mut vmf, id) = fixture();
    let baseline = output(&vmf);
    assert_eq!(baseline.stats.smoothed_faces, 0);
    let baseline = baseline.bsp.unwrap();
    paint(&mut vmf, id, &[1; 6], None);
    let before = vmf.clone();
    let softened = output(&vmf);
    assert!(
        softened.stats.smoothed_faces >= 6
            && softened.stats.smoothed_corners >= 24
            && softened.stats.normal_work > 0
    );
    let softened = softened.bsp.unwrap();
    let corners = corner_normals(&softened, DVec3::new(64.0, 64.0, 160.0));
    assert_eq!(corners.len(), 3);
    assert!(
        corners
            .iter()
            .all(|normal| normal.distance(DVec3::ONE.normalize()) < 2e-7),
        "{corners:?}"
    );
    for (index, (before, after)) in baseline.lumps.iter().zip(&softened.lumps).enumerate() {
        if ![
            lump::FACES,
            lump::ORIGINALFACES,
            lump::LIGHTING,
            lump::LEAF_AMBIENT_LIGHTING,
            lump::LEAF_AMBIENT_INDEX,
            lump::VERTNORMALS,
            lump::VERTNORMALINDICES,
        ]
        .contains(&index)
        {
            assert!(before == after, "Unrelated lump {index} changed");
        }
    }
    let expected = baseline.records::<Face>().unwrap();
    let mut actual = softened.records::<Face>().unwrap();
    for face in &mut actual {
        face.smoothing_groups = 0;
    }
    assert_eq!(expected, actual);
    assert_ne!(
        baseline.lumps[lump::LIGHTING],
        softened.lumps[lump::LIGHTING]
    );
    assert!(softened.to_bytes() == output(&vmf).bsp.unwrap().to_bytes());
    assert_eq!(vmf, before);
    paint(&mut vmf, id, &[1 << 23; 6], None);
    let high = output(&vmf).bsp.unwrap();
    assert_eq!(high.lumps[lump::LIGHTING], softened.lumps[lump::LIGHTING]);
    assert_eq!(
        high.lumps[lump::VERTNORMALS],
        softened.lumps[lump::VERTNORMALS]
    );
}

#[test]
fn disjoint_or_shared_hard_masks_keep_flat_lighting() {
    let (mut vmf, id) = fixture();
    let baseline = output(&vmf).bsp.unwrap();
    for masks in [
        [1, 2, 4, 8, 16, 32],
        [1 | (1 << 24); 6],
        [1 | (1 << 31); 6],
        [1 << 24; 6],
        [1 << 31; 6],
    ] {
        paint(&mut vmf, id, &masks, None);
        let flat = output(&vmf);
        assert_eq!(flat.stats.smoothed_faces, 0);
        assert_eq!(flat.stats.smoothed_corners, 0);
        let flat = flat.bsp.unwrap();
        for lump in [lump::LIGHTING, lump::VERTNORMALS, lump::VERTNORMALINDICES] {
            assert!(
                baseline.lumps[lump] == flat.lumps[lump],
                "Flat lump {lump} changed"
            );
        }
    }
}

#[test]
fn subdivision_and_hidden_sources_keep_checked_native_normals() {
    let (mut vmf, id) = fixture();
    paint(&mut vmf, id, &[1; 6], None);
    let baseline = output(&vmf).bsp.unwrap();
    let expected = corner_normals(&baseline, DVec3::new(64.0, 64.0, 160.0));
    paint(&mut vmf, id, &[1; 6], Some(1));
    let split = output(&vmf);
    assert!(split.stats.smoothed_faces > 6);
    let split = split.bsp.unwrap();
    let point = DVec3::new(-32.0, 0.0, 160.0);
    assert_eq!(
        top_sample(&split, point),
        top_sample(&baseline, point),
        "Subdivision changed the same receiver's shading"
    );
    let indices = split.values::<u16>(lump::VERTNORMALINDICES).unwrap();
    let normals = split.values::<[f32; 3]>(lump::VERTNORMALS).unwrap();
    assert!(
        indices
            .iter()
            .all(|&index| usize::from(index) < normals.len())
            && normals.len() <= 65_536
    );
    let actual = corner_normals(&split, DVec3::new(64.0, 64.0, 160.0));
    assert_eq!(actual.len(), expected.len());
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.distance(expected) < 2e-7)
    );
    vmf.world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap()
        .hidden = true;
    let hidden = output(&vmf);
    assert_eq!(hidden.stats.smoothed_faces, 0);
    assert_eq!(hidden.stats.smoothed_corners, 0);
}
