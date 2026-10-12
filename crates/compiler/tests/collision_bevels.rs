mod common;

use compiler::map::{Map, MapBrush};
use compiler::planes::PlaneSet;
use glam::{DQuat, DVec3};
use map_core::brush::BrushGeometry;
use map_core::{Document, edit};
use source_formats::bsp::{self, Brush, BrushSide, Bsp, Face, TexInfo};

#[derive(Clone, Copy, Debug)]
enum Shape {
    Box(DVec3),
    Tetrahedron,
    Ramp,
}

fn fixture(shape: Shape, rotation: DVec3, reflection: [bool; 3], room: bool) -> (Document, u32) {
    let mut document = if room {
        Document::new(common::room())
    } else {
        Document::empty()
    };
    let size = match shape {
        Shape::Box(size) => size,
        _ => DVec3::splat(64.0),
    };
    let (id, owner) = document
        .edit("collision fixture", |tx| {
            let mut solid = edit::block_solid(tx, DVec3::ZERO, size, "collision/face0")?;
            if !matches!(shape, Shape::Box(_)) {
                let planes = match shape {
                    Shape::Tetrahedron => vec![
                        (DVec3::NEG_X, 0.0),
                        (DVec3::NEG_Y, 0.0),
                        (DVec3::NEG_Z, 0.0),
                        (DVec3::ONE.normalize(), 64.0 / 3.0f64.sqrt()),
                    ],
                    Shape::Ramp => vec![
                        (DVec3::X, 64.0),
                        (DVec3::Y, 64.0),
                        (DVec3::NEG_Y, 0.0),
                        (DVec3::NEG_Z, 0.0),
                        (DVec3::new(-1.0, 0.0, 1.0).normalize(), 0.0),
                    ],
                    Shape::Box(_) => unreachable!(),
                };
                solid.sides.clear();
                for (normal, dist) in planes {
                    let tangent = normal
                        .cross(if normal.z.abs() < 0.9 {
                            DVec3::Z
                        } else {
                            DVec3::X
                        })
                        .normalize();
                    let bitangent = normal.cross(tangent);
                    let center = normal * dist;
                    solid.sides.push(edit::new_side(
                        tx,
                        [center + tangent * 64.0, center, center + bitangent * 64.0],
                        "collision/face0",
                    )?);
                }
            }
            for (index, side) in solid.sides.iter_mut().enumerate() {
                side.material = format!("collision/face{index}");
                side.smoothing_groups = Some(1 << index);
                side.u_axis.offset = index as f64 * 7.25;
            }
            let id = solid.id;
            let owner = if room {
                edit::create_entity(tx, "func_brush", None, &[], vec![solid])?
            } else {
                tx.add_solid(solid)?
            };
            Ok((id, owner))
        })
        .unwrap();
    document
        .edit("transform collision fixture", |tx| {
            edit::transform(
                tx,
                &[owner],
                map_core::transform::Transform {
                    rotation,
                    reflection,
                    translation: if room { DVec3::Z * 64.0 } else { DVec3::ZERO },
                    pivot: size * 0.5,
                    ..Default::default()
                },
            )
        })
        .unwrap();
    (document, id)
}

fn source_solid(document: &Document, id: u32) -> &source_formats::vmf::Solid {
    document
        .vmf()
        .world
        .solids
        .iter()
        .chain(
            document
                .vmf()
                .entities
                .iter()
                .flat_map(|entity| &entity.solids),
        )
        .find(|solid| solid.id == id)
        .unwrap()
}

fn brush_planes(brush: &MapBrush, planes: &PlaneSet, axial_only: bool) -> Vec<(DVec3, f64)> {
    brush
        .sides
        .iter()
        .filter(|side| !axial_only || !side.bevel || planes.get(side.plane).kind < 3)
        .map(|side| {
            let plane = planes.get(side.plane);
            (plane.normal, plane.dist)
        })
        .collect()
}

fn plane_trace(
    planes: &[(DVec3, f64)],
    start: DVec3,
    end: DVec3,
    mins: DVec3,
    maxs: DVec3,
) -> Option<f64> {
    let offset = (mins + maxs) * 0.5;
    let extents = (maxs - mins) * 0.5;
    let mut enter: f64 = 0.0;
    let mut leave: f64 = 1.0;
    for &(normal, dist) in planes {
        let expanded = dist + normal.abs().dot(extents);
        let from = normal.dot(start + offset) - expanded;
        let to = normal.dot(end + offset) - expanded;
        if from > 0.0 && to > 0.0 {
            return None;
        }
        if from <= 0.0 && to <= 0.0 {
            continue;
        }
        let fraction = from / (from - to);
        if from > to {
            enter = enter.max(fraction);
        } else {
            leave = leave.min(fraction);
        }
        if enter > leave {
            return None;
        }
    }
    Some(enter)
}

fn sat_trace(
    geometry: &BrushGeometry,
    start: DVec3,
    end: DVec3,
    mins: DVec3,
    maxs: DVec3,
) -> Option<f64> {
    let points = geometry
        .faces
        .iter()
        .flat_map(|face| face.vertices.iter().copied())
        .collect::<Vec<_>>();
    let mut axes = vec![DVec3::X, DVec3::Y, DVec3::Z];
    for face in &geometry.faces {
        axes.push(face.plane.normal);
        for (index, &point) in face.vertices.iter().enumerate() {
            let edge = face.vertices[(index + 1) % face.vertices.len()] - point;
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                if let Some(normal) = edge.cross(axis).try_normalize() {
                    axes.push(normal);
                }
            }
        }
    }
    let offset = (mins + maxs) * 0.5;
    let extents = (maxs - mins) * 0.5;
    let mut enter: f64 = 0.0;
    let mut leave: f64 = 1.0;
    for axis in axes {
        let (low, high) = points.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &p| {
            let projection = axis.dot(p);
            (lo.min(projection), hi.max(projection))
        });
        let radius = axis.abs().dot(extents);
        let from = axis.dot(start + offset);
        let movement = axis.dot(end - start);
        if movement.abs() < 1e-12 {
            if from < low - radius - 1e-9 || from > high + radius + 1e-9 {
                return None;
            }
            continue;
        }
        let first = (low - radius - from) / movement;
        let last = (high + radius - from) / movement;
        enter = enter.max(first.min(last));
        leave = leave.min(first.max(last));
        if enter > leave + 1e-9 {
            return None;
        }
    }
    Some(enter)
}

fn assert_trace(actual: Option<f64>, expected: Option<f64>, context: &str) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => {
            assert!(
                (actual - expected).abs() < 0.001,
                "{context}: {actual} != {expected}"
            );
        }
        (None, None) => {}
        _ => panic!("{context}: {actual:?} != {expected:?}"),
    }
}

fn assert_authored_planes(map: &Map<'_>, brush: &MapBrush, geometry: &BrushGeometry) {
    assert_eq!(brush.mins, geometry.mins);
    assert_eq!(brush.maxs, geometry.maxs);
    assert_eq!(
        brush.sides.iter().filter(|side| !side.bevel).count(),
        geometry.faces.len()
    );
    for (side, face) in brush.sides.iter().zip(&geometry.faces) {
        assert!(!side.bevel);
        assert_eq!(side.winding.as_ref().unwrap().0, face.vertices);
        let plane = map.planes.get(side.plane);
        for emitted in [false, true] {
            let (normal, dist) = if emitted {
                (
                    plane.normal.as_vec3().as_dvec3(),
                    f64::from(plane.dist as f32),
                )
            } else {
                (plane.normal, plane.dist)
            };
            for &point in &face.vertices {
                assert!((normal.dot(point) - dist).abs() <= compiler::winding::ON_EPSILON);
            }
            let support = brush
                .points()
                .map(|point| normal.dot(point))
                .fold(f64::MIN, f64::max);
            assert!((support - dist).abs() <= compiler::winding::ON_EPSILON);
        }
    }
}

#[test]
fn axial_boxes_and_unrotated_ramps_retain_their_existing_axial_sides() {
    let fs = common::empty_game();
    for (shape, expected_sides) in [(Shape::Box(DVec3::splat(64.0)), 6), (Shape::Ramp, 7)] {
        let (document, _) = fixture(shape, DVec3::ZERO, [false; 3], false);
        let mut report = compiler::Report::default();
        let map = Map::load(document.vmf(), &fs, &mut report);
        assert!(!report.has_errors(), "{report:?}");
        let brush = &map.brushes[0];
        assert_eq!(
            brush.sides.len(),
            expected_sides,
            "{:?}",
            brush
                .sides
                .iter()
                .map(|side| (side.bevel, map.planes.get(side.plane)))
                .collect::<Vec<_>>()
        );
        assert!(
            brush
                .sides
                .iter()
                .all(|side| !side.bevel || map.planes.get(side.plane).kind < 3)
        );
        for normal in [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ] {
            assert!(
                brush
                    .sides
                    .iter()
                    .any(|side| map.planes.get(side.plane).normal == normal)
            );
        }
    }
}

#[test]
fn sloped_edge_bevels_remove_axial_only_false_hits_and_correct_entry_fractions() {
    let (document, id) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], false);
    let source = document.solid(id).unwrap();
    let geometry = BrushGeometry::from_solid(source).unwrap();
    let fs = common::empty_game();
    let map = Map::load(document.vmf(), &fs, &mut compiler::Report::default());
    let brush = &map.brushes[0];
    assert_eq!(brush.sides.len(), 10);
    assert_eq!(
        brush
            .sides
            .iter()
            .filter(|side| side.bevel && map.planes.get(side.plane).kind >= 3)
            .count(),
        3
    );
    let complete = brush_planes(brush, &map.planes, false);
    let axial = brush_planes(brush, &map.planes, true);
    let mins = DVec3::splat(-16.0);
    let maxs = DVec3::splat(16.0);
    let start = DVec3::new(96.0, 60.0, -15.0);
    let end = DVec3::new(60.0, 60.0, -15.0);
    assert!((plane_trace(&axial, start, end, mins, maxs).unwrap() - 29.0 / 36.0).abs() < 1e-9);
    assert_eq!(sat_trace(&geometry, start, end, mins, maxs), None);
    assert_eq!(plane_trace(&complete, start, end, mins, maxs), None);

    let start = DVec3::new(96.0, 20.0, -15.0);
    let end = DVec3::new(20.0, 20.0, -15.0);
    assert!((plane_trace(&axial, start, end, mins, maxs).unwrap() - 16.0 / 76.0).abs() < 1e-9);
    assert_trace(
        plane_trace(&complete, start, end, mins, maxs),
        Some(20.0 / 76.0),
        "sloped edge entry",
    );
    assert_trace(
        sat_trace(&geometry, start, end, mins, maxs),
        Some(20.0 / 76.0),
        "SAT entry",
    );
}

#[test]
fn rotated_reflected_thin_and_sloped_hull_sweeps_match_separating_axes() {
    let fs = common::empty_game();
    let fixtures = [
        (
            Shape::Box(DVec3::new(64.0, 48.0, 32.0)),
            DVec3::new(17.0, -23.0, 31.0),
            [false; 3],
        ),
        (
            Shape::Box(DVec3::new(0.25, 31.0, 48.0)),
            DVec3::new(-32.0, 14.0, -47.0),
            [true, false, false],
        ),
        (Shape::Tetrahedron, DVec3::ZERO, [false; 3]),
        (
            Shape::Tetrahedron,
            DVec3::new(13.0, 27.0, -19.0),
            [false, true, true],
        ),
        (Shape::Ramp, DVec3::new(21.0, -13.0, 37.0), [false; 3]),
        (
            Shape::Box(DVec3::splat(64.0)),
            DVec3::new(0.0, 0.0, 0.1),
            [false; 3],
        ),
        (
            Shape::Box(DVec3::splat(64.0)),
            DVec3::new(-0.1, 0.0, 0.0),
            [true, false, false],
        ),
        (
            Shape::Box(DVec3::new(0.25, 31.0, 48.0)),
            DVec3::new(0.2, 0.1, -0.2),
            [false, true, false],
        ),
    ];
    let hulls = [
        (DVec3::ZERO, DVec3::ZERO),
        (DVec3::splat(-16.0), DVec3::splat(16.0)),
        (DVec3::new(-16.0, -16.0, 0.0), DVec3::new(16.0, 16.0, 72.0)),
        (DVec3::new(-3.0, -7.0, -1.0), DVec3::new(11.0, 2.0, 5.0)),
    ];
    let mut corrected = 0;
    for (shape, rotation, reflection) in fixtures {
        let (document, id) = fixture(shape, rotation, reflection, false);
        let geometry = BrushGeometry::from_solid(source_solid(&document, id)).unwrap();
        let mut report = compiler::Report::default();
        let map = Map::load(document.vmf(), &fs, &mut report);
        assert!(!report.has_errors(), "{report:?}");
        let complete = brush_planes(&map.brushes[0], &map.planes, false);
        let axial = brush_planes(&map.brushes[0], &map.planes, true);
        for (mins, maxs) in hulls {
            for sample in 0..128 {
                let start = DVec3::new(
                    (sample * 37 % 241) as f64 - 80.0,
                    (sample * 53 % 229) as f64 - 72.0,
                    (sample * 71 % 223) as f64 - 64.0,
                );
                let end = DVec3::new(
                    (sample * 83 % 239) as f64 - 80.0,
                    (sample * 97 % 227) as f64 - 72.0,
                    (sample * 107 % 211) as f64 - 64.0,
                );
                let expected = sat_trace(&geometry, start, end, mins, maxs);
                let context = format!(
                    "{shape:?} {rotation:?} {reflection:?} {mins:?} {maxs:?} sample {sample}"
                );
                assert_trace(
                    plane_trace(&complete, start, end, mins, maxs),
                    expected,
                    &context,
                );
                if plane_trace(&axial, start, end, mins, maxs).is_some() && expected.is_none() {
                    corrected += 1;
                }
            }
        }
    }
    assert!(
        corrected > 20,
        "only {corrected} axial-only counterexamples"
    );
}

#[test]
fn bevels_preserve_authored_sides_materials_and_support_every_vertex() {
    let fs = common::empty_game();
    for rotation in [DVec3::new(17.0, -23.0, 31.0), DVec3::new(0.2, 0.1, -0.2)] {
        let (document, id) = fixture(
            Shape::Box(DVec3::new(64.0, 48.0, 32.0)),
            rotation,
            [false; 3],
            false,
        );
        let before = document.vmf().clone();
        let source = document.solid(id).unwrap();
        let geometry = BrushGeometry::from_solid(source).unwrap();
        let mut report = compiler::Report::default();
        let map = Map::load(document.vmf(), &fs, &mut report);
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(document.vmf(), &before);
        let brush = &map.brushes[0];
        assert_eq!(brush.id, id);
        for (compiled, face) in brush.sides.iter().zip(&geometry.faces) {
            let authored = &source.sides[face.side];
            assert!(!compiled.bevel);
            assert!(compiled.visible);
            assert_eq!(compiled.winding.as_ref().unwrap().0, face.vertices);
            assert_eq!(
                compiled.smoothing_groups,
                authored.smoothing_groups.unwrap()
            );
            assert_eq!(
                map.materials.get(compiled.material).name,
                authored.material.to_ascii_uppercase()
            );
        }
        let mut indices = std::collections::HashSet::new();
        for side in &brush.sides {
            assert!(indices.insert(side.plane), "duplicate brush plane");
            if side.bevel {
                assert!(!side.visible && side.winding.is_none());
                assert_eq!(side.smoothing_groups, 0);
                assert_eq!(side.material, brush.sides[0].material);
                assert_eq!(side.texinfo, brush.sides[0].texinfo);
                if map.planes.get(side.plane).kind >= 3 {
                    for point in brush.points() {
                        assert!(
                            map.planes.get(side.plane).distance(point)
                                <= compiler::winding::ON_EPSILON
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn distant_near_parallel_edge_bevels_reject_unsafe_global_plane_reuse() {
    let (first, _) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], false);
    let angle = 8e-6f64;
    let (second, _) = fixture(
        Shape::Tetrahedron,
        DVec3::new(0.0, 0.0, angle.to_degrees()),
        [false; 3],
        false,
    );
    let normal = DVec3::new(1.0, 1.0, 0.0).normalize();
    let rotated = DQuat::from_rotation_z(angle) * normal;
    let first_geometry = BrushGeometry::from_solid(&first.vmf().world.solids[0]).unwrap();
    let second_geometry = BrushGeometry::from_solid(&second.vmf().world.solids[0]).unwrap();
    let target_dist = first_geometry
        .faces
        .iter()
        .flat_map(|face| &face.vertices)
        .map(|point| normal.dot(*point))
        .fold(f64::MIN, f64::max);
    let current_dist = second_geometry
        .faces
        .iter()
        .flat_map(|face| &face.vertices)
        .map(|point| rotated.dot(*point))
        .fold(f64::MIN, f64::max);
    for sign in [1.0, -1.0] {
        let distant = DVec3::new(16_000.0, -16_000.0, 0.0) * sign;
        let translation = distant + rotated * (target_dist - current_dist - rotated.dot(distant));
        let mut vmf = first.vmf().clone();
        let mut solid = second.vmf().world.solids[0].clone();
        solid.id = 1_000;
        for (index, side) in solid.sides.iter_mut().enumerate() {
            side.id = 1_001 + index as u32;
            for point in &mut side.plane.0 {
                *point += translation;
            }
        }
        let geometry = BrushGeometry::from_solid(&solid).unwrap();
        vmf.world.solids.push(solid);
        let fs = common::empty_game();
        let mut report = compiler::Report::default();
        let map = Map::load(&vmf, &fs, &mut report);
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(map.brushes.len(), 2);
        let first_plane = map.brushes[0]
            .sides
            .iter()
            .find(|side| side.bevel && map.planes.get(side.plane).normal.abs_diff_eq(normal, 1e-10))
            .unwrap()
            .plane;
        let second_plane = map.brushes[1]
            .sides
            .iter()
            .find(|side| {
                side.bevel
                    && map
                        .planes
                        .get(side.plane)
                        .normal
                        .abs_diff_eq(rotated, 1e-10)
            })
            .unwrap()
            .plane;
        assert_ne!(first_plane, second_plane);
        let old_gap = map.brushes[1]
            .points()
            .map(|point| map.planes.get(first_plane).distance(point))
            .fold(f64::MIN, f64::max);
        assert!(old_gap.abs() > 0.1);
        let actual_gap = map.brushes[1]
            .points()
            .map(|point| map.planes.get(second_plane).distance(point))
            .fold(f64::MIN, f64::max);
        assert!(actual_gap.abs() <= compiler::winding::ON_EPSILON);
        if sign < 0.0 {
            let start = DVec3::new(-15_951.845_542_528, 16_048.155_227_950, -15.0);
            let end = DVec3::new(-15_952.057_672_865, 16_047.943_094_218, -15.0);
            let hull = (DVec3::splat(-16.0), DVec3::splat(16.0));
            assert_eq!(sat_trace(&geometry, start, end, hull.0, hull.1), None);
            let actual = brush_planes(&map.brushes[1], &map.planes, false);
            assert_eq!(plane_trace(&actual, start, end, hull.0, hull.1), None);
            let old = map.planes.get(first_plane);
            let unsafe_reuse = map.brushes[1]
                .sides
                .iter()
                .zip(actual)
                .map(|(side, plane)| {
                    if side.plane == second_plane {
                        (old.normal, old.dist)
                    } else {
                        plane
                    }
                })
                .collect::<Vec<_>>();
            let fraction = plane_trace(&unsafe_reuse, start, end, hull.0, hull.1).unwrap();
            assert!((fraction - 0.729_935_54).abs() < 0.001);
        }
    }
}

#[test]
fn near_axial_edge_bevels_preserve_required_hull_facets() {
    let (document, _) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], false);
    let mut vmf = document.vmf().clone();
    let solid = &mut vmf.world.solids[0];
    for side in &mut solid.sides {
        for point in &mut side.plane.0 {
            point.x += point.y * 0.999;
        }
    }
    let geometry = BrushGeometry::from_solid(solid).unwrap();
    let fs = common::empty_game();
    let mut report = compiler::Report::default();
    let map = Map::load(&vmf, &fs, &mut report);
    assert!(!report.has_errors(), "{report:?}");
    let normal = DVec3::new(1.0, 0.001, 0.0).normalize();
    let side = map.brushes[0]
        .sides
        .iter()
        .find(|side| side.bevel && map.planes.get(side.plane).normal.abs_diff_eq(normal, 1e-10))
        .unwrap();
    assert_eq!(map.planes.get(side.plane).kind, 3);
    let start = DVec3::new(80.2, 79.0, -15.0);
    let end = DVec3::new(79.97, 79.0, -15.0);
    let hull = (DVec3::splat(-16.0), DVec3::splat(16.0));
    assert_eq!(sat_trace(&geometry, start, end, hull.0, hull.1), None);
    let complete = brush_planes(&map.brushes[0], &map.planes, false);
    assert_eq!(plane_trace(&complete, start, end, hull.0, hull.1), None);
    let omitted = map.brushes[0]
        .sides
        .iter()
        .zip(complete)
        .filter(|(candidate, _)| candidate.plane != side.plane)
        .map(|(_, plane)| plane)
        .collect::<Vec<_>>();
    let fraction = plane_trace(&omitted, start, end, hull.0, hull.1).unwrap();
    assert!((fraction - 0.869_565_217_391_3).abs() < 1e-8);
}

#[test]
fn slightly_rotated_boxes_keep_authored_planes_on_their_faces() {
    let (document, _) = fixture(
        Shape::Box(DVec3::splat(64.0)),
        DVec3::new(0.0, 0.0, 0.1),
        [false; 3],
        false,
    );
    let fs = common::empty_game();
    let mut report = compiler::Report::default();
    let map = Map::load(document.vmf(), &fs, &mut report);
    assert!(!report.has_errors(), "{report:?}");
    let brush = &map.brushes[0];
    let geometry = BrushGeometry::from_solid(&document.vmf().world.solids[0]).unwrap();
    assert_authored_planes(&map, brush, &geometry);
    for (side, face) in brush.sides.iter().zip(&geometry.faces) {
        assert!(!side.bevel && side.visible);
        let plane = map.planes.get(side.plane);
        for &point in &face.vertices {
            assert!(
                plane.distance(point).abs() <= compiler::winding::ON_EPSILON,
                "authored plane clips its face by {} units",
                plane.distance(point)
            );
        }
        let center = face.vertices.iter().copied().sum::<DVec3>() / face.vertices.len() as f64;
        let start = center - face.plane.normal * 0.02;
        let end = center + face.plane.normal * 0.02;
        assert_trace(
            plane_trace(
                &brush_planes(brush, &map.planes, false),
                start,
                end,
                DVec3::ZERO,
                DVec3::ZERO,
            ),
            sat_trace(&geometry, start, end, DVec3::ZERO, DVec3::ZERO),
            "near-axis face crossing",
        );
    }
}

#[test]
fn authored_plane_checks_the_whole_face_when_snapped_support_is_safe() {
    let (document, _) = fixture(
        Shape::Box(DVec3::splat(64.0)),
        DVec3::ZERO,
        [false; 3],
        false,
    );
    let mut vmf = document.vmf().clone();
    for side in &mut vmf.world.solids[0].sides {
        for point in &mut side.plane.0 {
            point.x -= point.y * 0.001;
        }
    }
    let geometry = BrushGeometry::from_solid(&vmf.world.solids[0]).unwrap();
    let normal = DVec3::new(1.0, 0.001, 0.0).normalize();
    let face_index = geometry
        .faces
        .iter()
        .position(|face| face.plane.normal.abs_diff_eq(normal, 1e-10))
        .unwrap();
    let dist = geometry.faces[face_index].plane.dist.round();
    assert!((geometry.maxs.x - dist).abs() <= compiler::winding::ON_EPSILON);
    assert!(
        geometry.faces[face_index]
            .vertices
            .iter()
            .any(|point| (point.x - dist).abs() > 0.06)
    );
    let fs = common::empty_game();
    let mut report = compiler::Report::default();
    let map = Map::load(&vmf, &fs, &mut report);
    assert!(!report.has_errors(), "{report:?}");
    assert_authored_planes(&map, &map.brushes[0], &geometry);
    assert_eq!(
        map.planes.get(map.brushes[0].sides[face_index].plane).kind,
        3
    );
}

#[test]
fn distant_authored_faces_reject_unsafe_near_normal_reuse() {
    let (first, _) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], false);
    let angle = 8e-6f64;
    let (second, _) = fixture(
        Shape::Tetrahedron,
        DVec3::new(0.0, 0.0, angle.to_degrees()),
        [false; 3],
        false,
    );
    let first_geometry = BrushGeometry::from_solid(&first.vmf().world.solids[0]).unwrap();
    let second_geometry = BrushGeometry::from_solid(&second.vmf().world.solids[0]).unwrap();
    let first_face = first_geometry
        .faces
        .iter()
        .find(|face| face.plane.normal.min_element() > 0.0)
        .unwrap();
    let second_face = second_geometry
        .faces
        .iter()
        .find(|face| face.plane.normal.min_element() > 0.0)
        .unwrap();
    for sign in [1.0, -1.0] {
        let distant = DVec3::new(16_000.0, -16_000.0, 0.0) * sign;
        let translation = distant
            + second_face.plane.normal
                * (first_face.plane.dist
                    - second_face.plane.dist
                    - second_face.plane.normal.dot(distant));
        let mut vmf = first.vmf().clone();
        let mut solid = second.vmf().world.solids[0].clone();
        solid.id = 1_000;
        for (index, side) in solid.sides.iter_mut().enumerate() {
            side.id = 1_001 + index as u32;
            for point in &mut side.plane.0 {
                *point += translation;
            }
        }
        let geometry = BrushGeometry::from_solid(&solid).unwrap();
        vmf.world.solids.push(solid);
        let fs = common::empty_game();
        let mut report = compiler::Report::default();
        let map = Map::load(&vmf, &fs, &mut report);
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(map.brushes.len(), 2);
        assert_authored_planes(&map, &map.brushes[1], &geometry);
        let first_index = map.brushes[0].sides[3].plane;
        let second_index = map.brushes[1].sides[3].plane;
        assert_ne!(first_index, second_index);
        assert!(
            geometry
                .faces
                .iter()
                .flat_map(|face| &face.vertices)
                .map(|&point| map.planes.get(first_index).distance(point))
                .fold(f64::MIN, f64::max)
                .abs()
                > 0.1
        );
    }
}

#[test]
fn authored_distance_snapping_checks_native_f32_support() {
    let (document, id) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], true);
    let mut vmf = document.vmf().clone();
    let solid = vmf
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .find(|solid| solid.id == id)
        .unwrap();
    let geometry = BrushGeometry::from_solid(solid).unwrap();
    let face = geometry
        .faces
        .iter()
        .find(|face| face.plane.normal.min_element() > 0.0)
        .unwrap();
    let translation = face.plane.normal * (-22_581.990_1 - face.plane.dist);
    for side in &mut solid.sides {
        for point in &mut side.plane.0 {
            *point += translation;
        }
    }
    let geometry = BrushGeometry::from_solid(solid).unwrap();
    let face = &geometry.faces[3];
    let unsafe_dist = face.plane.dist.round();
    assert!((face.plane.dist - unsafe_dist).abs() <= compiler::winding::ON_EPSILON);
    let emitted_normal = face.plane.normal.as_vec3().as_dvec3();
    assert!(
        face.vertices
            .iter()
            .map(|&point| emitted_normal.dot(point) - unsafe_dist)
            .fold(f64::MIN, f64::max)
            > compiler::winding::ON_EPSILON
    );
    let fs = common::empty_game();
    let mut report = compiler::Report::default();
    let map = Map::load(&vmf, &fs, &mut report);
    assert!(!report.has_errors(), "{report:?}");
    let brush_index = map.brushes.iter().position(|brush| brush.id == id).unwrap();
    let brush = &map.brushes[brush_index];
    assert_authored_planes(&map, brush, &geometry);
    assert_ne!(map.planes.get(brush.sides[3].plane).dist, unsafe_dist);
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    let original = output.bsp.unwrap();
    let bsp = Bsp::parse(&original.to_bytes()).unwrap();
    assert_eq!(bsp, original);
    let planes = bsp.records::<bsp::Plane>().unwrap();
    for (side, face) in brush.sides.iter().zip(&geometry.faces) {
        let native = &planes[side.plane];
        let normal = DVec3::from_array(native.normal.map(f64::from));
        for &point in &face.vertices {
            assert!(
                (normal.dot(point) - f64::from(native.dist)).abs() <= compiler::winding::ON_EPSILON
            );
        }
    }
}

#[test]
fn distant_bevel_distance_supports_the_emitted_f32_plane() {
    let (document, id) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], true);
    let mut vmf = document.vmf().clone();
    let solid = vmf
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .find(|solid| solid.id == id)
        .unwrap();
    for side in &mut solid.sides {
        for point in &mut side.plane.0 {
            *point += DVec3::new(-16_000.0, -15_999.756_664_794_968, 0.0);
        }
    }
    let geometry = BrushGeometry::from_solid(solid).unwrap();
    let points = geometry
        .faces
        .iter()
        .flat_map(|face| &face.vertices)
        .copied()
        .collect::<Vec<_>>();
    let normal = DVec3::new(1.0, 1.0, 0.0).normalize();
    let support = points
        .iter()
        .map(|point| normal.dot(*point))
        .fold(f64::MIN, f64::max);
    let unsafe_dist = -22_582.0;
    assert!((support - unsafe_dist).abs() <= compiler::winding::ON_EPSILON);
    let emitted_support = points
        .iter()
        .map(|point| normal.as_vec3().as_dvec3().dot(*point))
        .fold(f64::MIN, f64::max);
    assert!(emitted_support - unsafe_dist > compiler::winding::ON_EPSILON);
    let fs = common::empty_game();
    let mut report = compiler::Report::default();
    let map = Map::load(&vmf, &fs, &mut report);
    assert!(!report.has_errors(), "{report:?}");
    let target_index = map.brushes.iter().position(|brush| brush.id == id).unwrap();
    let side = map.brushes[target_index]
        .sides
        .iter()
        .find(|side| side.bevel && map.planes.get(side.plane).normal.abs_diff_eq(normal, 1e-10))
        .unwrap();
    assert_ne!(map.planes.get(side.plane).dist, unsafe_dist);
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    let bsp = output.bsp.unwrap();
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    let planes = parsed.records::<bsp::Plane>().unwrap();
    let brushes = parsed.records::<Brush>().unwrap();
    let sides = parsed.records::<BrushSide>().unwrap();
    let target = &brushes[target_index];
    let native_side = sides
        [target.first_side as usize..(target.first_side + target.side_count) as usize]
        .iter()
        .find(|native| native.plane as usize == side.plane)
        .unwrap();
    assert_eq!(native_side.bevel, 1);
    let native_plane = &planes[native_side.plane as usize];
    let native_normal = DVec3::from_array(native_plane.normal.map(f64::from));
    for stored_vertices in [false, true] {
        let boundary = points
            .iter()
            .map(|point| {
                let point = if stored_vertices {
                    point.as_vec3().as_dvec3()
                } else {
                    *point
                };
                native_normal.dot(point)
            })
            .fold(f64::MIN, f64::max);
        assert!((boundary - f64::from(native_plane.dist)).abs() <= compiler::winding::ON_EPSILON);
    }
}

#[test]
fn native_bsp_edge_bevels_round_trip_without_visible_faces() {
    let (document, id) = fixture(Shape::Tetrahedron, DVec3::ZERO, [false; 3], true);
    let fs = common::empty_game();
    let mut report = compiler::Report::default();
    let map = Map::load(document.vmf(), &fs, &mut report);
    let brush_index = map.brushes.iter().position(|brush| brush.id == id).unwrap();
    let output = compiler::compile(document.vmf(), &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    let bsp = output.bsp.unwrap();
    let bytes = bsp.to_bytes();
    let parsed = Bsp::parse(&bytes).unwrap();
    assert_eq!(parsed, bsp);
    assert_eq!(parsed.to_bytes(), bytes);
    let planes = parsed.records::<bsp::Plane>().unwrap();
    let sides = parsed.records::<BrushSide>().unwrap();
    let brushes = parsed.records::<Brush>().unwrap();
    let faces = parsed.records::<Face>().unwrap();
    let texinfos = parsed.records::<TexInfo>().unwrap();
    let target = &brushes[brush_index];
    assert_eq!(target.contents, bsp::contents::SOLID);
    assert_eq!(target.side_count, 10);
    let target_sides =
        &sides[target.first_side as usize..(target.first_side + target.side_count) as usize];
    for (emitted, compiled) in target_sides.iter().zip(&map.brushes[brush_index].sides) {
        assert_eq!(emitted.plane as usize, compiled.plane);
        assert_eq!(emitted.texinfo as usize, compiled.texinfo);
        assert_eq!(emitted.dispinfo, -1);
        assert_eq!(emitted.bevel, u8::from(compiled.bevel));
        assert_eq!(emitted.thin, 0);
        assert!((emitted.plane as usize) < planes.len());
        assert!((emitted.texinfo as usize) < texinfos.len());
        if emitted.bevel != 0 {
            assert!(!faces.iter().any(|face| face.plane == emitted.plane));
        }
    }
    for group in [1, 2, 4, 8] {
        assert_eq!(
            faces
                .iter()
                .filter(|face| face.smoothing_groups == group)
                .count(),
            1
        );
    }
    let hull = (DVec3::splat(-16.0), DVec3::splat(16.0));
    let start = DVec3::new(96.0, 60.0, 49.0);
    let end = DVec3::new(60.0, 60.0, 49.0);
    let native_planes = target_sides
        .iter()
        .map(|side| {
            let plane = &planes[side.plane as usize];
            (
                DVec3::from_array(plane.normal.map(f64::from)),
                f64::from(plane.dist),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        plane_trace(&native_planes, start, end, hull.0, hull.1),
        None
    );
    let axial_planes = target_sides
        .iter()
        .filter(|side| side.bevel == 0 || planes[side.plane as usize].kind < 3)
        .map(|side| {
            let plane = &planes[side.plane as usize];
            (
                DVec3::from_array(plane.normal.map(f64::from)),
                f64::from(plane.dist),
            )
        })
        .collect::<Vec<_>>();
    assert!(plane_trace(&axial_planes, start, end, hull.0, hull.1).is_some());
}

#[test]
fn rotated_native_brush_models_keep_authored_surface_geometry_and_supporting_bevels() {
    let fs = common::empty_game();
    for (shape, rotation) in [
        (
            Shape::Box(DVec3::new(64.0, 48.0, 32.0)),
            DVec3::new(17.0, -23.0, 31.0),
        ),
        (Shape::Tetrahedron, DVec3::new(17.0, -23.0, 31.0)),
        (Shape::Ramp, DVec3::new(17.0, -23.0, 31.0)),
        (Shape::Box(DVec3::splat(64.0)), DVec3::new(0.0, 0.0, 0.1)),
        (Shape::Box(DVec3::splat(64.0)), DVec3::new(0.1, 0.0, 0.0)),
        (Shape::Box(DVec3::splat(64.0)), DVec3::new(0.0, -0.1, 0.0)),
        (
            Shape::Box(DVec3::new(0.25, 31.0, 48.0)),
            DVec3::new(0.2, 0.1, -0.2),
        ),
    ] {
        let (document, id) = fixture(shape, rotation, [true, false, false], true);
        let geometry = BrushGeometry::from_solid(source_solid(&document, id)).unwrap();
        let mut report = compiler::Report::default();
        let map = Map::load(document.vmf(), &fs, &mut report);
        let brush_index = map.brushes.iter().position(|brush| brush.id == id).unwrap();
        let output = compiler::compile(document.vmf(), &fs);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
        let planes = bsp.records::<bsp::Plane>().unwrap();
        let brushes = bsp.records::<Brush>().unwrap();
        let sides = bsp.records::<BrushSide>().unwrap();
        let all_faces = bsp.records::<Face>().unwrap();
        let models = bsp.records::<bsp::Model>().unwrap();
        let vertices = bsp.vertices().unwrap();
        let edges = bsp.records::<bsp::Edge>().unwrap();
        let surfedges = bsp.surfedges().unwrap();
        let model = &models[1];
        let faces =
            &all_faces[model.first_face as usize..(model.first_face + model.face_count) as usize];
        assert_eq!(faces.len(), geometry.faces.len());
        for face in &geometry.faces {
            let authored = &source_solid(&document, id).sides[face.side];
            let emitted = faces
                .iter()
                .find(|emitted| emitted.smoothing_groups == authored.smoothing_groups.unwrap())
                .unwrap();
            let points = surfedges[emitted.first_edge as usize
                ..emitted.first_edge as usize + emitted.edge_count as usize]
                .iter()
                .map(|&surfedge| {
                    let edge = edges[surfedge.unsigned_abs() as usize].0;
                    DVec3::from_array(
                        vertices[edge[usize::from(surfedge < 0)] as usize].map(f64::from),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(points.len(), face.vertices.len());
            for point in points {
                assert!(
                    face.vertices
                        .iter()
                        .any(|authored| authored.abs_diff_eq(point, 0.0002)),
                    "{shape:?}: altered face vertex {point:?}"
                );
            }
        }
        let brush = &brushes[brush_index];
        let emitted =
            &sides[brush.first_side as usize..(brush.first_side + brush.side_count) as usize];
        assert!(
            emitted
                .iter()
                .any(|side| planes[side.plane as usize].kind >= 3)
        );
        assert_eq!(emitted.len(), map.brushes[brush_index].sides.len());
        for side in emitted {
            if side.bevel != 0 {
                assert!(!faces.iter().any(|face| face.plane == side.plane));
            }
            let plane = &planes[side.plane as usize];
            let normal = DVec3::from_array(plane.normal.map(f64::from));
            assert!(
                geometry
                    .faces
                    .iter()
                    .flat_map(|face| &face.vertices)
                    .all(|point| normal.dot(*point) - f64::from(plane.dist) <= 0.0101)
            );
        }
    }
}

#[test]
#[ignore = "loading cost probe"]
fn collision_bevel_loading_cost() {
    for rotation in [DVec3::ZERO, DVec3::new(17.0, -23.0, 31.0)] {
        let (document, _) = fixture(
            Shape::Box(DVec3::new(64.0, 48.0, 32.0)),
            rotation,
            [false; 3],
            false,
        );
        let mut vmf = document.vmf().clone();
        let prototype = vmf.world.solids.pop().unwrap();
        for index in 0..1_000u32 {
            let mut solid = prototype.clone();
            solid.id = 1_000 + index * 7;
            let translation = DVec3::new(
                (index % 25) as f64 * 160.0 - 2_000.0,
                (index / 25) as f64 * 160.0 - 3_200.0,
                0.0,
            );
            for (side_index, side) in solid.sides.iter_mut().enumerate() {
                side.id = solid.id + side_index as u32 + 1;
                for point in &mut side.plane.0 {
                    *point += translation;
                }
            }
            vmf.world.solids.push(solid);
        }
        let fs = common::empty_game();
        let mut report = compiler::Report::default();
        let start = std::time::Instant::now();
        let map = Map::load(&vmf, &fs, &mut report);
        let elapsed = start.elapsed();
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(map.brushes.len(), 1_000);
        let sides = map
            .brushes
            .iter()
            .map(|brush| brush.sides.len())
            .sum::<usize>();
        eprintln!(
            "rotation {rotation:?}: 1,000 brushes, {sides} sides, {} planes, {elapsed:?}",
            map.planes.len()
        );
    }
}
