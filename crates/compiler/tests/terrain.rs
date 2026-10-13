mod common;

use glam::DVec3;
use map_core::displacement::DisplacementMesh;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, Leaf, Model, TexInfo};
use source_formats::vmf::{DispInfo, Vmf};

#[test]
fn direct_terrain_lighting_uses_native_surface_samples_and_sloped_normals() {
    for power in [2, 3, 4] {
        let (vmf, _) = fixture(power);
        let mut document = Document::new(vmf);
        document
            .edit("terrain light", |tx| {
                edit::create_entity(
                    tx,
                    "light",
                    Some(DVec3::new(64.0, 64.0, 180.0)),
                    &[
                        ("_light".into(), "255 0 0 200".into()),
                        ("_constant_attn".into(), "1".into()),
                        ("_quadratic_attn".into(), "0".into()),
                    ],
                    Vec::new(),
                )
            })
            .unwrap();
        let source = document.vmf().clone();
        let output = compiler::compile_with_options(
            document.vmf(),
            &common::empty_game(),
            compiler::Options {
                lighting: compiler::LightingMode::DirectLdr,
                ..Default::default()
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.stats.ambient_samples > 0);
        assert_eq!(output.stats.lighting_faces, 7);
        let bsp = output.bsp.unwrap();
        let data = bsp.displacements().unwrap().unwrap();
        let face = bsp.records::<bsp::Face>().unwrap()[usize::from(data.infos[0].face)];
        assert_eq!(face.lightmap_size, [9, 9]);
        let info = bsp.records::<TexInfo>().unwrap()[face.texinfo as usize];
        assert_eq!(info.flags & bsp::surf::NOLIGHT, 0);
        let positions = data
            .positions(0, bsp.displacement_quad(0).unwrap())
            .unwrap();
        let sampler = bsp::DisplacementSurfaceSampler::new(data.infos[0], &positions).unwrap();
        let samples = bsp.displacement_lightmap_samples(0).unwrap().unwrap();
        assert_eq!(samples.len(), 100);
        let lighting = bsp.lighting(bsp::LightingKind::Ldr).unwrap().unwrap();
        let colors = lighting.face(&face, &info).unwrap().unwrap();
        let light = compiler::map::MapEntity {
            id: 0,
            classname: "light".into(),
            origin: Some(DVec3::new(64.0, 64.0, 180.0)),
            brushes: Vec::new(),
            properties: vec![
                ("_light".into(), "255 0 0 200".into()),
                ("_constant_attn".into(), "1".into()),
                ("_quadratic_attn".into(), "0".into()),
            ],
        };
        let light = compiler::lighting::PointLight::from_entity(&light, 0).unwrap();
        for (index, (sample, color)) in samples
            .iter()
            .zip(colors.samples(0, 0).unwrap())
            .enumerate()
        {
            let geometric = sampler
                .sample([(index % 10) as f64 / 9.0, (index / 10) as f64 / 9.0])
                .unwrap();
            assert_eq!(
                *sample,
                geometric
                    .lightmap_record()
                    .unwrap_or(bsp::DisplacementLightmapSample {
                        triangle: 0,
                        weights: [0; 3]
                    })
            );
            let point = DVec3::from_array(geometric.surface.position);
            let normal = DVec3::from_array(geometric.surface.normal);
            assert!(point.z > 19.0 && normal.x.abs() + normal.y.abs() > 0.01);
            let expected =
                compiler::lighting::encode_color(light.contribution(point, normal).unwrap())
                    .unwrap();
            assert_eq!(*color, expected);
        }
        let planes = bsp.records::<bsp::Plane>().unwrap();
        let infos = bsp.records::<TexInfo>().unwrap();
        let floor = bsp
            .records::<bsp::Face>()
            .unwrap()
            .into_iter()
            .find(|face| {
                let plane = planes[face.plane as usize];
                plane.normal[2] > 0.99 && plane.dist.abs() < 0.01
            })
            .unwrap();
        let info = &infos[floor.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            map_core::Plane {
                normal: DVec3::Z,
                dist: 0.0,
            },
            info,
            floor.lightmap_mins,
            floor.lightmap_size,
        )
        .unwrap();
        let colors = lighting.face(&floor, info).unwrap().unwrap();
        let [width, height] = frame.dimensions();
        for (point, shadowed) in [
            (DVec3::new(64.0, 64.0, 0.0), true),
            (DVec3::new(-64.0, -64.0, 0.0), false),
        ] {
            let index = (0..height)
                .flat_map(|t| (0..width).map(move |s| (s, t)))
                .find(|&(s, t)| frame.position(s, t).unwrap().distance(point) < 0.001)
                .unwrap();
            let color = colors.samples(0, 0).unwrap()[index.1 * width + index.0];
            assert_eq!(color.rgb == [0; 3], shadowed);
        }
        assert_eq!(*document.vmf(), source);
    }
}

#[test]
fn mixed_power_terrain_keeps_sample_offsets_and_explicit_missing_records() {
    let mut vmf = common::room();
    for (index, power) in [2, 4, 3].into_iter().enumerate() {
        let (patch, id) = fixture(power);
        let mut solid = patch
            .world
            .solids
            .into_iter()
            .find(|solid| solid.id == id)
            .unwrap();
        solid.id = 1000 + index as u32;
        let offset = DVec3::new(
            if index == 0 { -160.0 } else { 0.0 },
            if index == 2 { 0.0 } else { -160.0 },
            0.0,
        );
        for (side, face) in solid.sides.iter_mut().enumerate() {
            face.id = 2000 + (index * 6 + side) as u32;
            face.plane.0 = face.plane.0.map(|point| point + offset);
            if let Some(disp) = &mut face.displacement {
                disp.start_position += offset;
            }
            face.lightmap_scale = 4;
        }
        vmf.world.solids.push(solid);
    }
    let source = vmf.clone();
    let output = compiler::compile_with_options(
        &vmf,
        &common::empty_game(),
        compiler::Options {
            lighting: compiler::LightingMode::DirectLdr,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.displacements, 3);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let data = bsp.displacements().unwrap().unwrap();
    let faces = bsp.records::<bsp::Face>().unwrap();
    let mut cursor = 0;
    let mut missing = 0;
    for (index, info) in data.infos.iter().enumerate() {
        assert_eq!(info.lightmap_sample_start as usize, cursor);
        let samples = bsp.displacement_lightmap_samples(index).unwrap().unwrap();
        let face = faces[usize::from(info.face)];
        let width = face.lightmap_size[0] as usize + 1;
        let height = face.lightmap_size[1] as usize + 1;
        assert_eq!(samples.len(), width * height);
        let positions = data
            .positions(index, bsp.displacement_quad(index).unwrap())
            .unwrap();
        let sampler = bsp::DisplacementSurfaceSampler::new(*info, &positions).unwrap();
        for (i, sample) in samples.iter().enumerate() {
            let geometry = sampler
                .sample([
                    (i % width) as f64 / (width - 1) as f64,
                    (i / width) as f64 / (height - 1) as f64,
                ])
                .unwrap();
            if geometry.triangle == 511 {
                missing += 1;
                assert_eq!(
                    *sample,
                    bsp::DisplacementLightmapSample {
                        triangle: 0,
                        weights: [0; 3]
                    }
                );
                assert!(
                    sample
                        .surface(&info.lightmap_triangle_indices().unwrap(), &positions)
                        .unwrap()
                        .is_none()
                );
            } else {
                assert_eq!(*sample, geometry.lightmap_record().unwrap());
            }
        }
        cursor += info.encode_lightmap_samples(&samples).unwrap().len();
    }
    assert!(missing > 0);
    assert_eq!(
        cursor,
        bsp.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS]
            .data
            .len()
    );
    assert_eq!(vmf, source);
}

#[test]
fn terrain_ridge_receives_interpolated_lighting_without_changing_contact_geometry() {
    let (mut vmf, id) = fixture(2);
    let side = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap()
        .sides
        .iter_mut()
        .find(|side| side.displacement.is_some())
        .unwrap();
    side.lightmap_scale = 18;
    let disp = side.displacement.as_mut().unwrap();
    disp.elevation = 0.0;
    disp.offsets = None;
    disp.distances = Some(vec![vec![0.0, 0.0, 16.0, 0.0, 0.0]; 5]);
    let fullbright = compiler::compile(&vmf, &common::empty_game()).bsp.unwrap();
    let mut document = Document::new(vmf);
    document
        .edit("ridge light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(64.125, 64.25, 180.0)),
                &[
                    ("_light".into(), "0 255 0 200".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let source = document.vmf().clone();
    let output = compiler::compile_with_options(
        document.vmf(),
        &common::empty_game(),
        compiler::Options {
            lighting: compiler::LightingMode::DirectLdr,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    for lump in [bsp::lump::DISP_VERTS, bsp::lump::DISP_TRIS] {
        assert_eq!(bsp.lumps[lump].data, fullbright.lumps[lump].data);
    }
    let data = bsp.displacements().unwrap().unwrap();
    let face = bsp.records::<bsp::Face>().unwrap()[usize::from(data.infos[0].face)];
    assert_eq!(face.lightmap_size, [8, 8]);
    let info = bsp.records::<TexInfo>().unwrap()[face.texinfo as usize];
    let lighting = bsp.lighting(bsp::LightingKind::Ldr).unwrap().unwrap();
    let colors = lighting.face(&face, &info).unwrap().unwrap();
    assert_eq!(
        colors.samples(0, 0).unwrap()[4 * 9 + 4],
        compiler::lighting::encode_color(DVec3::new(0.0, 200.0, 0.0)).unwrap()
    );
    assert_eq!(*document.vmf(), source);
}

fn adjacent_fixture(powers: [u32; 2], rotations: [usize; 2]) -> Vmf {
    let mut document = Document::new(common::room());
    for (index, power) in powers.into_iter().enumerate() {
        document
            .edit("adjacent terrain", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(index as f64 * 112.0, 0.0, 0.0),
                    DVec3::new((index + 1) as f64 * 112.0, 224.0, 16.0),
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
                solid.sides[face.side].displacement = Some(DispInfo {
                    power,
                    start_position: face.vertices[rotations[index]],
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: None,
                    distances: None,
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                Ok(())
            })
            .unwrap();
    }
    document.vmf().clone()
}

#[test]
fn native_terrain_full_edges_keep_sdk_orientations_for_rotated_start_corners() {
    let orientations = [[2, 3, 0, 1], [1, 2, 3, 0], [0, 1, 2, 3], [3, 0, 1, 2]];
    for power in [2, 3, 4] {
        for first in 0..4 {
            for second in 0..4 {
                let vmf = adjacent_fixture([power; 2], [first, second]);
                let source = vmf.clone();
                let output = compiler::compile(&vmf, &common::empty_game());
                assert!(!output.report.has_errors(), "{:?}", output.report);
                assert_eq!(output.stats.displacement_neighbor_pairs, 1);
                let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
                let data = bsp.displacements().unwrap().unwrap();
                let edges = data
                    .infos
                    .iter()
                    .map(|info| {
                        info.edges
                            .iter()
                            .enumerate()
                            .find(|(_, edge)| edge[0].index != u16::MAX)
                            .unwrap()
                            .0
                    })
                    .collect::<Vec<_>>();
                for index in 0..2 {
                    let edge = data.infos[index].edges[edges[index]];
                    assert_eq!(edge[0].index as usize, 1 - index);
                    assert_eq!(
                        edge[0].orientation,
                        orientations[edges[index]][edges[1 - index]]
                    );
                    assert_eq!((edge[0].span, edge[0].neighbor_span), (0, 0));
                    assert_eq!(edge[1].index, u16::MAX);
                    assert_eq!(
                        data.infos[index]
                            .edges
                            .iter()
                            .flatten()
                            .filter(|edge| edge.index != u16::MAX)
                            .count(),
                        1
                    );
                    assert_eq!(data.infos[index].allowed_vertices, [u32::MAX; 10]);
                }
                assert_eq!(vmf, source);
            }
        }
    }
}

#[test]
fn unequal_power_mismatched_and_ambiguous_terrain_edges_stay_unlinked() {
    for case in 0..3 {
        let mut vmf = adjacent_fixture(if case == 0 { [2, 3] } else { [3, 3] }, [0, 0]);
        if case == 1 {
            vmf.world
                .solids
                .iter_mut()
                .flat_map(|solid| &mut solid.sides)
                .find_map(|side| side.displacement.as_mut())
                .unwrap()
                .elevation = 1.0;
        }
        if case == 2 {
            let mut duplicate = vmf
                .world
                .solids
                .iter()
                .find(|solid| solid.sides.iter().any(|side| side.displacement.is_some()))
                .unwrap()
                .clone();
            duplicate.id = 10000;
            for (index, side) in duplicate.sides.iter_mut().enumerate() {
                side.id = 10001 + index as u32;
            }
            vmf.world.solids.push(duplicate);
        }
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.displacement_neighbor_pairs, 0);
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|row| row.message.contains("Terrain shared edges left unlinked"))
        );
        let data = output.bsp.unwrap().displacements().unwrap().unwrap();
        assert!(
            data.infos
                .iter()
                .flat_map(|info| info.edges)
                .flatten()
                .all(|neighbor| neighbor.index == u16::MAX)
        );
    }
}

fn fixture(power: u32) -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    let id = document
        .edit("terrain", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(0.125, 0.25, 0.375),
                DVec3::new(128.125, 128.25, 16.375),
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
            let size = (1 << power) + 1;
            solid.sides[face.side].displacement = Some(DispInfo {
                power,
                start_position: face.vertices[0],
                flags: Some(0),
                elevation: 2.5,
                subdivided: false,
                normals: Some(vec![vec![DVec3::Z; size]; size]),
                distances: Some(
                    (0..size)
                        .map(|row| (0..size).map(|col| (row + 3 * col) as f64).collect())
                        .collect(),
                ),
                offsets: Some(vec![vec![DVec3::new(0.125, -0.25, 0.375); size]; size]),
                offset_normals: None,
                alphas: Some(vec![vec![300.0; size]; size]),
                triangle_tags: None,
                allowed_verts: None,
                extra_properties: Default::default(),
                extra: Vec::new(),
            });
            Ok(id)
        })
        .unwrap();
    (document.vmf().clone(), id)
}

#[test]
fn world_terrain_emits_native_grids_and_leaf_faces_without_solid_parent_volumes() {
    for power in 2..=4 {
        let (vmf, id) = fixture(power);
        let before = vmf.clone();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.brushes, 6);
        assert_eq!(output.stats.displacements, 1);
        let size = (1 << power) + 1;
        assert_eq!(output.stats.displacement_vertices, size * size);
        assert_eq!(output.stats.displacement_triangles, 2 * (size - 1).pow(2));
        let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
        let data = bsp.displacements().unwrap().unwrap();
        assert_eq!(data.infos[0].flags(), Some(0));
        let solid = vmf
            .world
            .solids
            .iter()
            .find(|solid| solid.id == id)
            .unwrap();
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap();
        let mesh =
            DisplacementMesh::build(face, solid.sides[face.side].displacement.as_ref().unwrap())
                .unwrap();
        let points = data
            .positions(0, bsp.displacement_quad(0).unwrap())
            .unwrap();
        assert!(points.iter().zip(&mesh.positions).all(|(point, &source)| {
            DVec3::from_array(point.map(f64::from)).distance(source) < 0.02
        }));
        assert!(data.vertices.iter().all(|vertex| vertex.alpha == 300.0));
        assert!(data.triangles.iter().all(|triangle| triangle.tags == 7));
        let faces = bsp.records::<bsp::Face>().unwrap();
        let native_face = faces[usize::from(data.infos[0].face)];
        let info = bsp.records::<TexInfo>().unwrap()[native_face.texinfo as usize];
        assert_ne!(info.flags & bsp::surf::NOLIGHT, 0);
        let refs = bsp.values::<u16>(bsp::lump::LEAFFACES).unwrap();
        let leaves = bsp.records::<Leaf>().unwrap();
        assert!(leaves.iter().any(|leaf| {
            leaf.contents & bsp::contents::SOLID == 0
                && refs[usize::from(leaf.first_leaf_face)
                    ..usize::from(leaf.first_leaf_face) + usize::from(leaf.leaf_face_count)]
                    .contains(&data.infos[0].face)
        }));
        assert_eq!(vmf, before);
    }
}

#[test]
fn invalid_or_unsupported_terrain_preserves_previous_output() {
    let (vmf, id) = fixture(2);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("existing.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    for case in 0..6 {
        let mut invalid = vmf.clone();
        let solid = invalid
            .world
            .solids
            .iter_mut()
            .find(|solid| solid.id == id)
            .unwrap();
        let side = solid
            .sides
            .iter_mut()
            .find(|side| side.displacement.is_some())
            .unwrap();
        let disp = side.displacement.as_mut().unwrap();
        match case {
            0 => disp.power = 31,
            1 => disp.distances.as_mut().unwrap()[0][0] = f64::NAN,
            2 => disp.start_position = DVec3::ZERO,
            3 => disp.flags = Some(16),
            4 => disp.subdivided = true,
            5 => disp.triangle_tags = Some(vec![vec![0x20; 8]; 4]),
            _ => unreachable!(),
        }
        let source = invalid.write(&Default::default());
        let output = compiler::compile(&invalid, &common::empty_game());
        assert!(
            output.report.has_errors(),
            "case {case}: {:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(invalid.write(&Default::default()), source);
    }
}

#[test]
fn terrain_parent_cannot_seal_a_leak_and_brush_entities_are_rejected() {
    let (mut vmf, id) = fixture(2);
    let floor = vmf
        .world
        .solids
        .iter()
        .position(|solid| solid.sides.iter().all(|side| side.plane.0[0].z <= 0.0))
        .unwrap();
    vmf.world.solids[floor].hidden = true;
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.leak.is_some() && output.bsp.is_none());
    vmf.world.solids[floor].hidden = false;
    let mut document = Document::new(vmf);
    document
        .edit("terrain entity", |tx| {
            edit::tie_to_entity(tx, &[id], "func_brush", &[])?;
            Ok(())
        })
        .unwrap();
    assert!(
        compiler::compile(document.vmf(), &common::empty_game())
            .report
            .has_errors()
    );
}

#[test]
fn world_bounds_include_displaced_peaks() {
    let (mut vmf, id) = fixture(2);
    let disp = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap()
        .sides
        .iter_mut()
        .find_map(|side| side.displacement.as_mut())
        .unwrap();
    disp.distances.as_mut().unwrap()[2][2] = 300.0;
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    assert!(bsp.records::<Model>().unwrap()[0].maxs[2] > 300.0);
}

#[test]
fn engine_terrain_soup_preserves_native_grids_and_honors_no_physics_flags() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let gameinfo = std::path::PathBuf::from(home)
        .join(".local/share/Steam/steamapps/common/Counter-Strike Source/cstrike/gameinfo.txt");
    if !gameinfo.is_file() {
        return;
    }
    let game = source_formats::gamefs::GameFs::from_gameinfo(&gameinfo).unwrap();
    let options = compiler::Options {
        physics: compiler::PhysicsMode::Engine,
        ..Default::default()
    };
    let (mut vmf, id) = fixture(2);
    let source = vmf.clone();
    let output = compiler::compile_with_options(&vmf, &game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_mesh_triangles, 32);
    assert_eq!(output.stats.physics_convexes, 6);
    assert_eq!(output.stats.physics_solids, 2);
    let bsp = output.bsp.unwrap();
    let physics = bsp.physics().unwrap().unwrap();
    assert_eq!(physics.models[0].solids.len(), 2);
    assert_eq!(
        compiler::compile_with_options(&vmf, &game, options)
            .bsp
            .unwrap()
            .to_bytes(),
        bsp.to_bytes()
    );
    assert_eq!(vmf, source);
    let disp = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap()
        .sides
        .iter_mut()
        .find_map(|side| side.displacement.as_mut())
        .unwrap();
    disp.flags = Some(2);
    let output = compiler::compile_with_options(&vmf, &game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_mesh_triangles, 0);
    assert_eq!(output.stats.physics_solids, 1);
    assert_eq!(
        output.bsp.unwrap().displacements().unwrap().unwrap().infos[0].flags(),
        Some(2)
    );
}
