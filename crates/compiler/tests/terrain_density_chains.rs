mod common;

use std::collections::BTreeMap;

use glam::{DVec2, DVec3};
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::{
    bsp::{self, Bsp, DisplacementInfo, LightingKind, TexInfo},
    gamefs::GameFs,
    vmf::{DispInfo, Vmf},
};

#[derive(Clone, Copy)]
struct Patch {
    mins: [f64; 3],
    maxs: [f64; 3],
    start: [f64; 3],
    step: [f64; 2],
    power: u32,
}

const PATCHES: [Patch; 4] = [
    Patch {
        mins: [0.0, 0.0, 0.0],
        maxs: [64.0, 128.0, 16.0],
        start: [0.0, 0.0, 16.0],
        step: [16.0, 32.0],
        power: 2,
    },
    Patch {
        mins: [64.0, 0.0, 0.0],
        maxs: [128.0, 64.0, 16.0],
        start: [64.0, 0.0, 16.0],
        step: [16.0, 16.0],
        power: 2,
    },
    Patch {
        mins: [128.0, 0.0, 0.0],
        maxs: [160.0, 32.0, 16.0],
        start: [128.0, 0.0, 16.0],
        step: [4.0, 4.0],
        power: 3,
    },
    Patch {
        mins: [160.0, 0.0, 0.0],
        maxs: [192.0, 32.0, 16.0],
        start: [192.0, 32.0, 16.0],
        step: [-4.0, -4.0],
        power: 3,
    },
];

const MASKS: [[u32; 10]; 4] = [
    [u32::MAX; 10],
    [
        0xffff_7fdf,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
    ],
    [
        0xc7f3_f1fd,
        0x7f3f_1fdf,
        0xffff_fdfc,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
    ],
    [u32::MAX; 10],
];

const HEIGHTS: [&[f32]; 4] = [
    &[0.0, 1.0, 4.0, 9.0, 16.0],
    &[0.0, 0.25, 1.0, 2.25, 4.0],
    &[
        0.0, 0.015625, 0.0625, 0.140625, 0.25, 0.390625, 0.5625, 0.765625, 1.0,
    ],
    &[
        1.0, 0.765625, 0.5625, 0.390625, 0.25, 0.140625, 0.0625, 0.015625, 0.0,
    ],
];

const VERTEX_BYTES: usize = std::mem::size_of::<bsp::DisplacementVertex>();

const C_CORRECTIONS: [[f32; 3]; 9] = [
    [0.0; 3],
    [3.0 / 64.0, 1.0 / 64.0, 1.0 / 64.0],
    [4.0 / 64.0, 2.0 / 64.0, 0.0],
    [3.0 / 64.0, 1.0 / 64.0, 1.0 / 64.0],
    [0.0; 3],
    [3.0 / 64.0, 1.0 / 64.0, 1.0 / 64.0],
    [4.0 / 64.0, 2.0 / 64.0, 0.0],
    [3.0 / 64.0, 1.0 / 64.0, 1.0 / 64.0],
    [0.0; 3],
];

fn fixture(patches: &[usize], curved: bool) -> Vmf {
    let mut document = Document::new(common::room());
    for &index in patches {
        let patch = PATCHES[index];
        document
            .edit("explicit terrain grid", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(patch.mins),
                    DVec3::from_array(patch.maxs),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.solid_mut(id)?;
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let face = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                let size = (1 << patch.power) + 1;
                solid.sides[face.side].displacement = Some(DispInfo {
                    power: patch.power,
                    start_position: DVec3::from_array(patch.start),
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: None,
                    distances: None,
                    offsets: Some(
                        (0..size)
                            .map(|row| {
                                let y = patch.start[1] + patch.step[1] * row as f64;
                                let height = if curved { y * y / 1024.0 } else { 0.0 };
                                vec![DVec3::Z * height; size]
                            })
                            .collect(),
                    ),
                    offset_normals: None,
                    alphas: Some(
                        (0..size)
                            .map(|row| vec![(index * 32 + row) as f64; size])
                            .collect(),
                    ),
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                Ok(())
            })
            .unwrap();
    }
    document
        .edit("density-chain light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(100.0, 48.0, 180.0)),
                &[
                    ("_light".into(), "255 64 0 200".into()),
                    ("_lightHDR".into(), "64 255 0 200".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_linear_attn".into(), "0".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    document.vmf().clone()
}

fn compile(vmf: &Vmf, game: &GameFs, options: compiler::Options) -> compiler::Output {
    let output = compiler::compile_with_options(vmf, game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn expected_point(patch: usize, row: usize, col: usize, curved: bool, lod: bool) -> [f32; 3] {
    let spec = PATCHES[patch];
    let correction = match (patch, row, col) {
        (1, 1 | 3, 0) => 0.25,
        (2, _, 0..=2) => C_CORRECTIONS[row][col],
        _ => 0.0,
    };
    [
        (spec.start[0] + spec.step[0] * col as f64) as f32,
        (spec.start[1] + spec.step[1] * row as f64) as f32,
        16.0 + if curved {
            HEIGHTS[patch][row] + if lod { correction } else { 0.0 }
        } else {
            0.0
        },
    ]
}

fn canonical(mut triangles: Vec<[usize; 3]>) -> Vec<[usize; 3]> {
    for triangle in &mut triangles {
        let first = (0..3).min_by_key(|&index| triangle[index]).unwrap();
        triangle.rotate_left(first);
    }
    triangles.sort_unstable();
    triangles
}

fn expected_triangles(patch: usize, lod: bool) -> Vec<[usize; 3]> {
    let size = (1 << PATCHES[patch].power) + 1;
    let mut triangles = Vec::new();
    for row in 0..size - 1 {
        for col in 0..size - 1 {
            if lod && patch == 2 && col < 2 {
                continue;
            }
            let a = row * size + col;
            let [b, c, d] = [a + 1, a + size, a + size + 1];
            triangles.extend(if (row + col) % 2 == 0 {
                [[a, c, d], [a, d, b]]
            } else {
                [[a, c, b], [c, d, b]]
            });
        }
    }
    if lod && patch == 1 {
        triangles.retain(|triangle| !triangle.iter().any(|index| [5, 15].contains(index)));
        triangles.extend([[0, 10, 6], [10, 20, 16]]);
    }
    if lod && patch == 2 {
        triangles.retain(|triangle| {
            !triangle
                .iter()
                .any(|index| [11, 29, 47, 65].contains(index))
        });
        triangles.extend([
            [2, 20, 12],
            [20, 38, 30],
            [38, 56, 48],
            [56, 74, 66],
            [2, 0, 20],
            [0, 36, 20],
            [36, 38, 20],
            [38, 36, 56],
            [36, 72, 56],
            [72, 74, 56],
        ]);
    }
    canonical(triangles)
}

fn assert_coverage(info: DisplacementInfo, triangles: &[[usize; 3]]) {
    let size = info.grid_size().unwrap();
    let uv = |index: usize| DVec2::new((index % size) as f64, (index / size) as f64);
    let mut edges = BTreeMap::<[usize; 2], usize>::new();
    let mut area = 0.0;
    for &[a, b, c] in triangles {
        let signed = (uv(b) - uv(a)).perp_dot(uv(c) - uv(a));
        assert!(signed < 0.0);
        area += signed;
        for [a, b] in [[a, b], [b, c], [c, a]] {
            *edges.entry([a.min(b), a.max(b)]).or_default() += 1;
        }
    }
    assert_eq!(area, -2.0 * ((size - 1) * (size - 1)) as f64);
    for ([a, b], count) in edges {
        let border = [uv(a), uv(b)];
        let boundary = (0..2).any(|axis| {
            border.iter().all(|p| p[axis] == 0.0)
                || border.iter().all(|p| p[axis] == (size - 1) as f64)
        });
        assert_eq!(count, if boundary { 1 } else { 2 });
    }
    for row in 0..size - 1 {
        for col in 0..size - 1 {
            for offset in [DVec2::new(0.2, 1.0 / 3.0), DVec2::new(0.8, 2.0 / 3.0)] {
                let point = DVec2::new(col as f64, row as f64) + offset;
                let covering = triangles
                    .iter()
                    .filter(|triangle| {
                        let [a, b, c] = triangle.map(uv);
                        [[a, b], [b, c], [c, a]]
                            .into_iter()
                            .all(|[a, b]| (b - a).perp_dot(point - a) < 0.0)
                    })
                    .count();
                assert_eq!(covering, 1, "uncovered or overlapping point {point}");
            }
        }
    }
}

fn assert_geometry(bsp: &Bsp, patches: &[usize], curved: bool, lod: bool) {
    let data = bsp.displacements().unwrap().unwrap();
    assert_eq!(data.infos.len(), patches.len());
    for (index, &patch) in patches.iter().enumerate() {
        let info = data.infos[index];
        assert_eq!(info.power as u32, PATCHES[patch].power);
        assert_eq!(info.start, PATCHES[patch].start.map(|v| v as f32));
        assert_eq!(
            info.allowed_vertices,
            if lod { MASKS[patch] } else { [u32::MAX; 10] }
        );
        let size = info.grid_size().unwrap();
        let positions = data
            .positions(index, bsp.displacement_quad(index).unwrap())
            .unwrap();
        for (vertex, &position) in positions.iter().enumerate() {
            let [row, col] = [vertex / size, vertex % size];
            assert_eq!(position, expected_point(patch, row, col, curved, lod));
            assert_eq!(
                data.vertices[info.vertex_start as usize + vertex].alpha,
                (patch * 32 + row) as f32
            );
        }
        let triangles = info.allowed_triangle_indices().unwrap();
        assert_eq!(canonical(triangles.clone()), expected_triangles(patch, lod));
        assert_eq!(
            triangles.len(),
            if lod {
                [32, 30, 98, 128][patch]
            } else {
                [32, 32, 128, 128][patch]
            }
        );
        assert!(
            triangles
                .iter()
                .flatten()
                .all(|&vertex| info.vertex_allowed(vertex))
        );
        assert_coverage(info, &triangles);
    }
}

fn assert_chain(output: &compiler::Output, curved: bool) {
    assert_eq!(output.stats.displacements, 4);
    assert_eq!(output.stats.displacement_vertices, 212);
    assert_eq!(output.stats.displacement_triangles, 320);
    assert_eq!(output.stats.displacement_neighbor_pairs, 3);
    assert_eq!(output.stats.displacement_half_edge_pairs, 2);
    assert_eq!(output.stats.displacement_mixed_power_pairs, 1);
    assert_eq!(output.stats.displacement_corner_neighbor_pairs, 0);
    assert_eq!(output.stats.displacement_lod_disabled_vertices, 21);
    assert_eq!(
        output.stats.displacement_lod_snapped_vertices,
        if curved { 18 } else { 0 }
    );
    let bsp = output.bsp.as_ref().unwrap();
    assert_geometry(bsp, &[0, 1, 2, 3], curved, true);
    let data = bsp.displacements().unwrap().unwrap();
    let mut expected = [[[(u16::MAX, 0, 0, 0); 2]; 4]; 4];
    expected[0][2][0] = (1, 0, 1, 0);
    expected[1][0][0] = (0, 0, 0, 1);
    expected[1][2][0] = (2, 0, 1, 0);
    expected[2][0][0] = (1, 0, 0, 1);
    expected[2][2][0] = (3, 2, 0, 0);
    expected[3][2][0] = (2, 2, 0, 0);
    for (index, info) in data.infos.iter().enumerate() {
        for (edge, slots) in info.edges.iter().enumerate() {
            for (slot, neighbor) in slots.iter().enumerate() {
                assert_eq!(
                    (
                        neighbor.index,
                        neighbor.orientation,
                        neighbor.span,
                        neighbor.neighbor_span
                    ),
                    expected[index][edge][slot]
                );
            }
        }
        assert!(info.corners.iter().all(|corner| corner.count == 0));
    }
}

#[test]
fn explicit_three_density_chain_has_literal_native_geometry_and_stable_source() {
    let game = common::empty_game();
    let options = compiler::Options::default();
    let source = fixture(&[0, 1, 2, 3], true);
    let original = source.clone();
    let output = compile(&source, &game, options);
    assert_chain(&output, true);
    let native = output.bsp.as_ref().unwrap();

    let half_source = fixture(&[0, 1], true);
    let half = compile(&half_source, &game, options);
    assert_eq!(half.stats.displacement_neighbor_pairs, 1);
    assert_eq!(half.stats.displacement_half_edge_pairs, 1);
    assert_eq!(half.stats.displacement_lod_disabled_vertices, 2);
    assert_eq!(half.stats.displacement_lod_snapped_vertices, 2);
    assert_geometry(half.bsp.as_ref().unwrap(), &[0, 1], true, true);
    assert_eq!(
        &native.lumps[bsp::lump::DISP_VERTS].data[..50 * VERTEX_BYTES],
        &half.bsp.as_ref().unwrap().lumps[bsp::lump::DISP_VERTS].data
    );

    let full_source = fixture(&[2, 3], true);
    let full = compile(&full_source, &game, options);
    assert_eq!(full.stats.displacement_neighbor_pairs, 1);
    assert_eq!(full.stats.displacement_half_edge_pairs, 0);
    assert_eq!(full.stats.displacement_lod_disabled_vertices, 0);
    assert_eq!(full.stats.displacement_lod_snapped_vertices, 0);
    assert_geometry(full.bsp.as_ref().unwrap(), &[2, 3], true, false);
    assert_eq!(
        &native.lumps[bsp::lump::DISP_VERTS].data[131 * VERTEX_BYTES..],
        &full.bsp.as_ref().unwrap().lumps[bsp::lump::DISP_VERTS].data[81 * VERTEX_BYTES..]
    );

    let reopened = Vmf::parse(&source.write(&Default::default())).unwrap();
    let saved = compile(&reopened, &game, options);
    assert_eq!(saved.bsp.unwrap().to_bytes(), native.to_bytes());
    let mut marked = source.clone();
    for info in marked
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
        .filter_map(|side| side.displacement.as_mut())
    {
        info.subdivided = true;
    }
    let marked = compile(&marked, &game, options);
    assert_eq!(marked.bsp.unwrap().to_bytes(), native.to_bytes());
    assert_eq!(source, original);

    let flat = fixture(&[0, 1, 2, 3], false);
    assert_chain(&compile(&flat, &game, options), false);
}

#[test]
fn dual_lighting_preserves_the_chain_surface_and_uses_both_authored_channels() {
    let source = fixture(&[0, 1, 2, 3], true);
    let original = source.clone();
    let game = common::empty_game();
    let fullbright = compile(&source, &game, compiler::Options::default());
    let output = compile(
        &source,
        &game,
        compiler::Options {
            lighting: compiler::LightingMode::DirectLdrHdr,
            ..Default::default()
        },
    );
    assert_chain(&output, true);
    let bsp = output.bsp.as_ref().unwrap();
    for lump in [bsp::lump::DISP_VERTS, bsp::lump::DISP_TRIS] {
        assert_eq!(
            bsp.lumps[lump].data,
            fullbright.bsp.as_ref().unwrap().lumps[lump].data
        );
    }
    let data = bsp.displacements().unwrap().unwrap();
    let texinfos = bsp.records::<TexInfo>().unwrap();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let faces = bsp.lighting_faces(kind).unwrap();
        let lighting = bsp.lighting(kind).unwrap().unwrap();
        for info in &data.infos {
            let face = &faces[usize::from(info.face)];
            assert_eq!(face.styles, [0, 255, 255, 255]);
            let colors = lighting
                .face(face, &texinfos[face.texinfo as usize])
                .unwrap()
                .unwrap();
            let samples = colors.samples(0, 0).unwrap();
            assert!(samples.iter().any(|sample| {
                let [red, green, blue] = sample.linear();
                blue == 0.0
                    && match kind {
                        LightingKind::Ldr => red > green && green > 0.0,
                        LightingKind::Hdr => green > red && red > 0.0,
                    }
            }));
        }
    }
    assert_eq!(source, original);
}

#[test]
#[ignore = "requires CSS_DIR and the matching native physics worker"]
fn native_physics_worker_receives_all_288_active_chain_triangles() {
    let root = std::env::var_os("CSS_DIR")
        .map(std::path::PathBuf::from)
        .expect("set CSS_DIR to the installed CS:S root");
    assert!(
        root.join("bin/linux64/vphysics.so").is_file(),
        "CSS_DIR needs the native Linux64 physics module"
    );
    let source = fixture(&[0, 1, 2, 3], true);
    let original = source.clone();
    let mut game = common::empty_game();
    game.base_dir = root;
    let output = compile(
        &source,
        &game,
        compiler::Options {
            physics: compiler::PhysicsMode::Engine,
            ..Default::default()
        },
    );
    assert_chain(&output, true);
    assert_eq!(output.stats.physics_mesh_triangles, 288);
    assert_eq!(output.stats.physics_convexes, 6);
    assert_eq!(output.stats.physics_solids, 2);
    let physics = output.bsp.as_ref().unwrap().physics().unwrap().unwrap();
    assert_eq!(physics.models[0].solids.len(), 2);
    assert_eq!(source, original);
}
