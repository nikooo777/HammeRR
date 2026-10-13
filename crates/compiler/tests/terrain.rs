mod common;

use glam::DVec3;
use map_core::displacement::DisplacementMesh;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, Leaf, Model, TexInfo};
use source_formats::vmf::{DispInfo, Vmf};

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
