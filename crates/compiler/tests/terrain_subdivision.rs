mod common;

use glam::DVec3;
use map_core::{
    Document, ObjectId, displacement, edit,
    surface::{self, FaceId},
};
use source_formats::{bsp, vmf::Vmf};

fn fixture(half: bool) -> (Document, Vec<FaceId>) {
    let mut document = Document::new(common::room());
    let mut faces = Vec::new();
    let mut patches = vec![
        ([0.0, 0.0, 48.0], [64.0, 128.0, 64.0], DVec3::Z, 2, 1),
        (
            [0.0, 0.0, 0.0],
            [16.0, if half { 64.0 } else { 128.0 }, 64.0],
            DVec3::NEG_X,
            3,
            3,
        ),
    ];
    if half {
        patches.push(([0.0, 64.0, 0.0], [16.0, 128.0, 64.0], DVec3::NEG_X, 4, 2));
    }
    for (mins, maxs, normal, power, corner) in patches {
        let face = document
            .edit("terrain cage", |tx| {
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let side = tx
                    .document()
                    .solid(solid)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0)
                            .unwrap()
                            .normal
                            .dot(normal)
                            > 0.99
                    })
                    .unwrap()
                    .id;
                let face = FaceId { solid, side };
                displacement::edit::create(tx, &[face], power, corner)?;
                Ok(face)
            })
            .unwrap();
        faces.push(face);
    }
    let plan = displacement::subdivide::plan(&document, &faces).unwrap();
    document
        .edit("subdivide cage", |tx| {
            for (face, info) in plan.edits {
                tx.solid_mut(face.solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .displacement = Some(info);
            }
            Ok(())
        })
        .unwrap();
    (document, faces)
}

#[test]
fn full_and_half_subdivision_keep_native_samples_masks_normals_and_saved_bytes() {
    use displacement::neighbors::{Span, common_span_indices};

    for half in [false, true] {
        let (document, faces) = fixture(half);
        let source = document.vmf().clone();
        let options = compiler::Options {
            physics: compiler::PhysicsMode::Engine,
            ..Default::default()
        };
        let result = compiler::compile_with_options(&source, &common::empty_game(), options);
        assert!(!result.report.has_errors(), "{:?}", result.report);
        assert_eq!(result.stats.displacements, if half { 3 } else { 2 });
        assert_eq!(
            result.stats.displacement_neighbor_pairs,
            if half { 3 } else { 1 }
        );
        assert_eq!(
            result.stats.displacement_half_edge_pairs,
            if half { 2 } else { 0 }
        );
        assert!(result.stats.displacement_lod_snapped_vertices > 0);
        let native = result.bsp.unwrap();
        let data = native.displacements().unwrap().unwrap();
        let grids = displacement::edit::inspect(&document, &faces).unwrap();
        let positions = data
            .infos
            .iter()
            .enumerate()
            .map(|(index, _)| {
                data.positions(index, native.displacement_quad(index).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut active = 0;
        for (index, (info, grid)) in data.infos.iter().zip(&grids).enumerate() {
            for (vertex, point) in positions[index].iter().enumerate() {
                if info.vertex_allowed(vertex) {
                    assert!(
                        DVec3::from_array(point.map(f64::from))
                            .distance(grid.mesh.positions[vertex])
                            < 2e-5
                    );
                    active += 1;
                }
            }
            for (edge, slots) in info.edges.iter().enumerate() {
                for neighbor in slots {
                    if neighbor.index == u16::MAX {
                        continue;
                    }
                    let other = usize::from(neighbor.index);
                    let other_edge = (edge + 2 + usize::from(neighbor.orientation)) % 4;
                    for (a, b) in common_span_indices(
                        (
                            info.power as u32,
                            edge,
                            Span::from_native(neighbor.span, edge).unwrap(),
                        ),
                        (
                            data.infos[other].power as u32,
                            other_edge,
                            Span::from_native(neighbor.neighbor_span, other_edge).unwrap(),
                        ),
                    )
                    .unwrap()
                    {
                        assert!(
                            DVec3::from_array(positions[index][a].map(f64::from))
                                .distance(DVec3::from_array(positions[other][b].map(f64::from)))
                                < 2e-5
                        );
                        assert_eq!(info.vertex_allowed(a), data.infos[other].vertex_allowed(b));
                    }
                }
            }
        }
        assert!(active > 50);
        let lit = compiler::compile_with_options(
            &source,
            &common::empty_game(),
            compiler::Options {
                lighting: compiler::LightingMode::DirectLdr,
                ..options
            },
        );
        assert!(!lit.report.has_errors(), "{:?}", lit.report);
        let lit = lit.bsp.unwrap();
        for lump in [
            bsp::lump::DISP_VERTS,
            bsp::lump::DISP_TRIS,
            bsp::lump::PHYSCOLLIDE,
        ] {
            assert_eq!(lit.lumps[lump].data, native.lumps[lump].data);
        }
        let reopened = Vmf::parse(&source.write(&Default::default())).unwrap();
        let saved = compiler::compile_with_options(&reopened, &common::empty_game(), options);
        assert!(!saved.report.has_errors(), "{:?}", saved.report);
        assert_eq!(saved.bsp.unwrap().to_bytes(), native.to_bytes());
        let mut unmarked = source.clone();
        for info in unmarked
            .world
            .solids
            .iter_mut()
            .flat_map(|solid| &mut solid.sides)
            .filter_map(|side| side.displacement.as_mut())
        {
            info.subdivided = false;
        }
        let plain = compiler::compile_with_options(&unmarked, &common::empty_game(), options);
        assert!(!plain.report.has_errors(), "{:?}", plain.report);
        assert_eq!(plain.bsp.unwrap().to_bytes(), native.to_bytes());
        assert_eq!(document.vmf(), &source);
        assert!(faces.iter().all(|face| {
            surface::side(&document, *face)
                .unwrap()
                .displacement
                .as_ref()
                .unwrap()
                .subdivided
        }));
        let restored = bsp::Bsp::parse(&native.to_bytes()).unwrap();
        assert_eq!(restored.to_bytes(), native.to_bytes());
    }
}
