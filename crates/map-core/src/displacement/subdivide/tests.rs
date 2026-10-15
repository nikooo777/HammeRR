use super::*;
use crate::{ObjectId, edit as map_edit, surface::SurfaceEdit};

fn add(
    document: &mut Document,
    mins: DVec3,
    maxs: DVec3,
    normal: DVec3,
    power: u32,
    corner: u32,
) -> FaceId {
    let ObjectId::Solid(solid) = document
        .edit("block", |tx| {
            map_edit::create_block(tx, mins, maxs, "brick/test")
        })
        .unwrap()
    else {
        unreachable!()
    };
    let side = document
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| {
            crate::Plane::from_points(side.plane.0)
                .unwrap()
                .normal
                .dot(normal)
                > 0.99
        })
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    document
        .edit("terrain", |tx| edit::create(tx, &[face], power, corner))
        .unwrap();
    face
}

fn fixture() -> (Document, Vec<FaceId>) {
    let mut document = Document::empty();
    let faces = vec![
        add(
            &mut document,
            DVec3::new(0.0, 0.0, 48.0),
            DVec3::splat(64.0),
            DVec3::Z,
            2,
            0,
        ),
        add(
            &mut document,
            DVec3::ZERO,
            DVec3::new(64.0, 16.0, 64.0),
            DVec3::NEG_Y,
            3,
            1,
        ),
        add(
            &mut document,
            DVec3::ZERO,
            DVec3::new(16.0, 64.0, 64.0),
            DVec3::NEG_X,
            4,
            3,
        ),
    ];
    (document, faces)
}

fn apply(document: &mut Document, plan: Plan) -> usize {
    document
        .edit("subdivide", |tx| {
            plan.edits.into_iter().try_fold(0, |count, (face, info)| {
                surface::edit_faces(
                    tx,
                    &[face],
                    &SurfaceEdit {
                        terrain: Some(super::super::Operation::Subdivided(Box::new(info))),
                        ..Default::default()
                    },
                )
                .map(|changed| count + changed)
            })
        })
        .unwrap()
}

#[test]
fn catmull_clark_stencils_move_interior_corners_and_edges_and_pin_boundaries() {
    let mesh = Mesh {
        points: vec![
            DVec3::ZERO,
            DVec3::new(64.0, 0.0, 0.0),
            DVec3::new(0.0, 64.0, 0.0),
            DVec3::new(0.0, 0.0, 64.0),
            DVec3::new(64.0, 0.0, 64.0),
            DVec3::splat(64.0),
            DVec3::new(0.0, 64.0, 64.0),
        ],
        faces: [[3, 4, 5, 6], [0, 1, 4, 3], [0, 3, 6, 2]]
            .into_iter()
            .enumerate()
            .map(|(source, vertices)| Quad {
                vertices,
                source,
                row: 0,
                column: 0,
            })
            .collect(),
    };
    let topology = Topology::build(&mesh).unwrap();
    let edge = topology
        .edges
        .iter()
        .position(|edge| edge.ends == [3, 4])
        .unwrap();
    let original = mesh.points.clone();
    let refined = mesh.refine().unwrap();
    assert_eq!(refined.faces.len(), 12);
    assert!(
        (refined.points[3] - DVec3::new(128.0 / 9.0, 128.0 / 9.0, 448.0 / 9.0)).length() < 1e-12
    );
    assert_eq!(
        refined.points[original.len() + edge],
        DVec3::new(32.0, 8.0, 56.0)
    );
    for id in [0, 1, 2, 4, 5, 6] {
        assert_eq!(refined.points[id], original[id]);
    }
}

#[test]
fn subdivision_plans_share_mixed_power_samples_boundaries_metadata_and_idempotence() {
    let (mut document, faces) = fixture();
    document
        .edit("sculpt and metadata", |tx| {
            for face in &faces {
                let info = tx
                    .solid_mut(face.solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .displacement
                    .as_mut()
                    .unwrap();
                let size = (1 << info.power) + 1;
                info.offsets = Some(vec![vec![DVec3::ZERO; size]; size]);
                info.offsets.as_mut().unwrap()[1][1] = DVec3::splat(3.0);
                for (id, value) in info
                    .alphas
                    .as_mut()
                    .unwrap()
                    .iter_mut()
                    .flatten()
                    .enumerate()
                {
                    *value = (id % 256) as f64;
                }
                info.extra_properties.set("test_metadata", "retained");
            }
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    let before = edit::inspect(&document, &faces).unwrap();
    let planned = plan(&document, &faces).unwrap();
    assert_eq!(planned.shared_edges, 3);
    assert_eq!(planned.vertices, 25 + 81 + 289);
    for (face, info) in &planned.edits {
        assert!(info.subdivided);
        let original = surface::side(&document, *face)
            .unwrap()
            .displacement
            .as_ref()
            .unwrap();
        let mut metadata = info.clone();
        metadata.subdivided = original.subdivided;
        metadata.offsets = original.offsets.clone();
        assert_eq!(&metadata, original);
    }
    assert_eq!(apply(&mut document, planned), 3);
    let after = edit::inspect(&document, &faces).unwrap();
    let views = before
        .iter()
        .map(|grid| {
            let n = grid.mesh.size;
            neighbors::SurfaceGrid {
                power: grid.power,
                corners: [0, n * (n - 1), n * n - 1, n - 1].map(|id| grid.mesh.base_positions[id]),
                positions: &grid.mesh.base_positions,
            }
        })
        .collect::<Vec<_>>();
    let links = neighbors::find_spans(&views, &mut |_| Ok(())).unwrap();
    for pair in &links.pairs {
        for (a, b) in neighbors::common_span_indices(
            (before[pair.first].power, pair.first_edge, pair.first_span),
            (
                before[pair.second].power,
                pair.second_edge,
                pair.second_span,
            ),
        )
        .unwrap()
        {
            assert!(
                (after[pair.first].mesh.positions[a] - after[pair.second].mesh.positions[b])
                    .length()
                    < 1e-10
            );
        }
    }
    for (index, grid) in after.iter().enumerate() {
        for edge in 0..4 {
            if links.pairs.iter().any(|pair| {
                (pair.first == index && pair.first_edge == edge)
                    || (pair.second == index && pair.second_edge == edge)
            }) {
                continue;
            }
            for vertex in neighbors::edge_indices(grid.mesh.size, edge).unwrap() {
                assert!(
                    (grid.mesh.positions[vertex] - grid.mesh.base_positions[vertex]).length()
                        < 1e-10
                );
            }
        }
        assert!(grid.subdivided);
        assert_eq!(grid.mesh.alphas, before[index].mesh.alphas);
    }
    let rounded = document.vmf().clone();
    let repeated = plan(&document, &faces).unwrap();
    assert_eq!(apply(&mut document, repeated), 0);
    assert_eq!(document.vmf(), &rounded);
    let text = document
        .vmf()
        .write(&source_formats::keyvalues::WriteOptions::default());
    let reopened = Document::new(source_formats::vmf::Vmf::parse(&text).unwrap());
    assert_eq!(
        edit::inspect(&reopened, &faces).unwrap()[0].mesh,
        after[0].mesh
    );
    document.undo().unwrap();
    assert_eq!(document.vmf(), &source);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &rounded);
}

#[test]
fn generated_grids_remain_editable_and_real_offset_normal_caches_stay_guarded() {
    let (mut document, faces) = fixture();
    let planned = plan(&document, &faces).unwrap();
    apply(&mut document, planned);
    assert!(super::super::sew::plan(&document, &faces, true).is_ok());
    let face = faces[0];
    let side = surface::side(&document, face).unwrap();
    let geometry = Face {
        side: 0,
        plane: crate::Plane::from_points(side.plane.0).unwrap(),
        vertices: surface::face_vertices(&document, face).unwrap(),
    };
    let info = side.displacement.as_ref().unwrap();
    assert!(
        super::super::brush::move_vertices(&geometry, info, &[12], DVec3::Z)
            .unwrap()
            .subdivided
    );
    assert!(
        super::super::resample::change(&geometry, info, 3)
            .unwrap()
            .subdivided
    );
    let brush = super::super::brush::Brush {
        center: DVec3::splat(32.0),
        radius: 8192.0,
        axis: super::super::brush::Axis::WorldZ,
    };
    assert!(
        super::super::brush::sculpt(
            &geometry,
            info,
            brush,
            super::super::brush::Sculpt::Raise(4.0)
        )
        .unwrap()
        .subdivided
    );
    assert!(
        super::super::brush::blend(
            &geometry,
            info,
            brush,
            super::super::brush::Blend::Fill(180.0)
        )
        .unwrap()
        .subdivided
    );
    document
        .edit("cache", |tx| {
            let info = tx
                .solid_mut(face.solid)?
                .sides
                .iter_mut()
                .find(|side| side.id == face.side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap();
            info.offset_normals = Some(vec![vec![DVec3::X; 5]; 5]);
            Ok(())
        })
        .unwrap();
    assert!(plan(&document, &faces).is_err());
    let info = surface::side(&document, face)
        .unwrap()
        .displacement
        .as_ref()
        .unwrap();
    assert!(super::super::brush::move_vertices(&geometry, info, &[12], DVec3::Z).is_err());
    assert!(super::super::resample::change(&geometry, info, 3).is_err());
    assert!(super::super::sew::plan(&document, &faces, false).is_err());
}

#[test]
fn invalid_fractional_edges_corner_contacts_and_duplicate_cages_reject_before_edits() {
    for corner_only in [false, true] {
        let mut document = Document::empty();
        let first = add(
            &mut document,
            DVec3::ZERO,
            DVec3::new(128.0, 64.0, 16.0),
            DVec3::Z,
            2,
            0,
        );
        let second = add(
            &mut document,
            if corner_only {
                DVec3::new(128.0, 64.0, 0.0)
            } else {
                DVec3::new(128.0, 0.0, 0.0)
            },
            DVec3::new(192.0, if corner_only { 96.0 } else { 16.0 }, 16.0),
            DVec3::Z,
            3,
            1,
        );
        let source = document.vmf().clone();
        assert!(plan(&document, &[first, second]).is_err());
        assert_eq!(document.vmf(), &source);
    }
    let (mut document, faces) = fixture();
    document
        .edit("cancelling geometry rows", |tx| {
            let info = tx
                .solid_mut(faces[0].solid)?
                .sides
                .iter_mut()
                .find(|side| side.id == faces[0].side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap();
            info.normals.as_mut().unwrap()[2][2] = DVec3::X * 1e20;
            info.distances.as_mut().unwrap()[2][2] = 1.0;
            info.offsets = Some(vec![vec![DVec3::ZERO; 5]; 5]);
            info.offsets.as_mut().unwrap()[2][2] = DVec3::X * -1e20;
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    assert!(
        plan(&document, &faces)
            .err()
            .unwrap()
            .to_string()
            .contains("precisely")
    );
    assert_eq!(document.vmf(), &source);
    let points = vec![DVec3::ZERO, DVec3::X, DVec3::ONE, DVec3::Y];
    let face = Quad {
        vertices: [0, 1, 2, 3],
        source: 0,
        row: 0,
        column: 0,
    };
    assert!(
        Topology::build(&Mesh {
            points: points.clone(),
            faces: vec![face, face]
        })
        .is_err()
    );
    assert!(
        Topology::build(&Mesh {
            points,
            faces: vec![Quad {
                vertices: [0, 1, 1, 3],
                ..face
            }]
        })
        .is_err()
    );
}

fn half_fixture(powers: [u32; 3], rotation: u32) -> (Document, Vec<FaceId>) {
    let mut document = Document::empty();
    let faces = vec![
        add(
            &mut document,
            DVec3::new(0.0, 0.0, 48.0),
            DVec3::new(64.0, 128.0, 64.0),
            DVec3::Z,
            powers[0],
            rotation,
        ),
        add(
            &mut document,
            DVec3::ZERO,
            DVec3::new(16.0, 64.0, 64.0),
            DVec3::NEG_X,
            powers[1],
            (rotation + 1) % 4,
        ),
        add(
            &mut document,
            DVec3::new(0.0, 64.0, 0.0),
            DVec3::new(16.0, 128.0, 64.0),
            DVec3::NEG_X,
            powers[2],
            (rotation + 3) % 4,
        ),
    ];
    (document, faces)
}

fn assert_border_samples(before: &[edit::TerrainGrid], after: &[edit::TerrainGrid]) {
    let views = before
        .iter()
        .map(|grid| {
            let n = grid.mesh.size;
            neighbors::SurfaceGrid {
                power: grid.power,
                corners: [0, n * (n - 1), n * n - 1, n - 1].map(|id| grid.mesh.base_positions[id]),
                positions: &grid.mesh.base_positions,
            }
        })
        .collect::<Vec<_>>();
    let links = neighbors::find_spans(&views, &mut |_| Ok(())).unwrap();
    let generated = views
        .iter()
        .zip(after)
        .map(|(view, grid)| neighbors::SurfaceGrid {
            power: view.power,
            corners: view.corners,
            positions: &grid.mesh.positions,
        })
        .collect::<Vec<_>>();
    let normals = neighbors::vertex_normals(&generated, &links.pairs, &mut |_| Ok(())).unwrap();
    let mut covered = before
        .iter()
        .map(|grid| vec![false; grid.mesh.positions.len()])
        .collect::<Vec<_>>();
    for pair in &links.pairs {
        for (source, edge, span) in [
            (pair.first, pair.first_edge, pair.first_span),
            (pair.second, pair.second_edge, pair.second_span),
        ] {
            for vertex in span.indices(before[source].power, edge).unwrap() {
                covered[source][vertex] = true;
            }
        }
        for (a, b) in neighbors::common_span_indices(
            (before[pair.first].power, pair.first_edge, pair.first_span),
            (
                before[pair.second].power,
                pair.second_edge,
                pair.second_span,
            ),
        )
        .unwrap()
        {
            assert!(
                after[pair.first].mesh.positions[a].distance(after[pair.second].mesh.positions[b])
                    < 1e-10
            );
            assert_eq!(normals[pair.first][a], normals[pair.second][b]);
            assert!(DVec3::from_array(normals[pair.first][a]).is_finite());
        }
    }
    for (source, grid) in after.iter().enumerate() {
        for edge in 0..4 {
            for vertex in neighbors::edge_indices(grid.mesh.size, edge).unwrap() {
                if !covered[source][vertex] {
                    assert!(
                        grid.mesh.positions[vertex].distance(grid.mesh.base_positions[vertex])
                            < 1e-10
                    );
                }
            }
        }
    }
}

#[test]
fn half_edge_cages_share_stencils_across_powers_rotated_starts_and_partial_coverage() {
    for powers in [[2, 3, 4], [4, 2, 3], [3, 4, 2], [2; 3], [3; 3], [4; 3]] {
        for rotation in 0..4 {
            for count in [2, 3] {
                let (mut document, mut faces) = half_fixture(powers, rotation);
                faces.truncate(count);
                let before = edit::inspect(&document, &faces).unwrap();
                let planned = plan(&document, &faces).unwrap();
                assert_eq!(planned.shared_edges, if count == 2 { 1 } else { 3 });
                assert_eq!(
                    planned.vertices,
                    before
                        .iter()
                        .map(|grid| grid.mesh.positions.len())
                        .sum::<usize>()
                );
                assert_eq!(apply(&mut document, planned), count);
                let after = edit::inspect(&document, &faces).unwrap();
                assert_border_samples(&before, &after);
                assert!(
                    before
                        .iter()
                        .zip(&after)
                        .all(|(a, b)| a.mesh.positions != b.mesh.positions)
                );
                assert!(super::super::sew::plan(&document, &faces, false).is_ok());
                let repeated = plan(&document, &faces).unwrap();
                assert_eq!(apply(&mut document, repeated), 0);
            }
        }
    }
}

#[test]
fn half_and_full_edge_components_preserve_rows_metadata_and_reopen_exactly() {
    let (mut document, mut faces) = half_fixture([2, 3, 4], 1);
    faces.push(add(
        &mut document,
        DVec3::new(64.0, 0.0, 48.0),
        DVec3::new(128.0, 128.0, 64.0),
        DVec3::Z,
        3,
        2,
    ));
    document
        .edit("terrain metadata", |tx| {
            for (index, face) in faces.iter().enumerate() {
                let side = tx
                    .solid_mut(face.solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap();
                let info = side.displacement.as_mut().unwrap();
                let size = (1 << info.power) + 1;
                info.normals = Some(vec![vec![DVec3::new(0.25, 0.5, 1.0); size]; size]);
                info.distances = Some(vec![vec![2.0; size]; size]);
                info.elevation = 7.0;
                info.offsets = Some(vec![vec![DVec3::splat(-2.0); size]; size]);
                info.offset_normals = Some(vec![vec![DVec3::ZERO; size]; size]);
                info.alphas = Some(vec![vec![index as f64 * 63.0; size]; size]);
                info.flags = Some(5);
                info.extra_properties.set("test_metadata", "half-edge");
            }
            Ok(())
        })
        .unwrap();
    let original = document.vmf().clone();
    let before = edit::inspect(&document, &faces).unwrap();
    let planned = plan(&document, &faces).unwrap();
    assert_eq!(planned.shared_edges, 4);
    for (face, generated) in &planned.edits {
        let source = surface::side(&document, *face)
            .unwrap()
            .displacement
            .as_ref()
            .unwrap();
        let mut restored = generated.clone();
        restored.offsets = source.offsets.clone();
        restored.subdivided = source.subdivided;
        assert_eq!(&restored, source);
    }
    apply(&mut document, planned);
    let rounded = document.vmf().clone();
    let after = edit::inspect(&document, &faces).unwrap();
    assert_border_samples(&before, &after);
    let reopened = Document::new(
        source_formats::vmf::Vmf::parse(&rounded.write(&Default::default())).unwrap(),
    );
    for (stored, expected) in edit::inspect(&reopened, &faces).unwrap().iter().zip(&after) {
        assert_eq!(stored.mesh, expected.mesh);
    }
    document.undo().unwrap();
    assert_eq!(document.vmf(), &original);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &rounded);
}

#[test]
fn ambiguous_half_junctions_and_nested_density_chains_reject_before_edits() {
    let (mut document, mut faces) = half_fixture([2, 3, 4], 0);
    faces.push(add(
        &mut document,
        DVec3::ZERO,
        DVec3::new(16.0, 128.0, 64.0),
        DVec3::NEG_X,
        2,
        0,
    ));
    let source = document.vmf().clone();
    assert!(
        plan(&document, &faces)
            .err()
            .unwrap()
            .to_string()
            .contains("unambiguous")
    );
    assert_eq!(document.vmf(), &source);
    let (mut document, mut faces) = half_fixture([2, 3, 4], 0);
    faces.truncate(2);
    faces.push(add(
        &mut document,
        DVec3::ZERO,
        DVec3::new(64.0, 32.0, 16.0),
        DVec3::NEG_Z,
        2,
        1,
    ));
    let source = document.vmf().clone();
    assert!(
        plan(&document, &faces)
            .err()
            .unwrap()
            .to_string()
            .contains("nested")
    );
    assert_eq!(document.vmf(), &source);
}

#[test]
fn half_junction_uses_one_conforming_catmull_clark_stencil() {
    let (document, faces) = half_fixture([2, 3, 4], 0);
    let grids = edit::inspect(&document, &faces).unwrap();
    let corners = grids
        .iter()
        .map(|grid| {
            let n = grid.mesh.size;
            [0, n * (n - 1), n * n - 1, n - 1].map(|id| grid.mesh.base_positions[id])
        })
        .collect::<Vec<_>>();
    let views = grids
        .iter()
        .zip(&corners)
        .map(|(grid, &corners)| neighbors::SurfaceGrid {
            power: grid.power,
            corners,
            positions: &grid.mesh.base_positions,
        })
        .collect::<Vec<_>>();
    let links = neighbors::find_spans(&views, &mut |_| Ok(())).unwrap();
    let (mesh, cells) = cage::build(&corners, &links.pairs).unwrap();
    assert_eq!(cells, [2, 1, 1]);
    assert_eq!(mesh.faces.len(), 6);
    let junction = mesh
        .points
        .iter()
        .position(|&point| point == DVec3::new(0.0, 64.0, 64.0))
        .unwrap();
    let refined = mesh.refine().unwrap();
    assert_eq!(refined.points[junction], DVec3::new(4.0, 64.0, 56.0));
}
