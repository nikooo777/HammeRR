mod common;

use glam::{DQuat, DVec3};
use map_core::{Document, ObjectId, Plane, edit, overlay, surface::FaceId};
use source_formats::vmf::Vmf;

#[test]
fn multiple_planar_terrain_and_flat_targets_match_native_samples_and_exact_bindings() {
    let mut document = Document::new(common::room());
    let sources = document
        .edit("isolated displaced targets and gap controls", |tx| {
            let mut sources = Vec::new();
            for (index, [min, max]) in [
                [-208.0, -112.0],
                [-48.0, 48.0],
                [112.0, 208.0],
                [-96.0, -64.0],
                [64.0, 96.0],
            ]
            .into_iter()
            .enumerate()
            {
                let terrain = matches!(index, 0 | 1 | 3);
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(min, if terrain { -64.0 } else { -60.0 }, 16.0),
                    DVec3::new(
                        max,
                        if terrain { 64.0 } else { 68.0 },
                        if terrain { 32.0 } else { 44.0 },
                    ),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let source = FaceId {
                    solid,
                    side: tx.solid_mut(solid)?.sides[0].id,
                };
                if terrain {
                    let power = if index == 1 { 3 } else { 2 };
                    map_core::displacement::edit::create(
                        tx,
                        &[source],
                        power,
                        if index == 1 { 2 } else { 0 },
                    )?;
                    let size = (1 << power) + 1;
                    tx.solid_mut(solid)?.sides[0]
                        .displacement
                        .as_mut()
                        .unwrap()
                        .offsets = Some(vec![vec![DVec3::new(3.0, 4.0, 12.0); size]; size]);
                }
                if index == 1 {
                    edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                }
                if index < 3 {
                    sources.push(source);
                }
            }
            Ok(sources)
        })
        .unwrap();
    let base = document.vmf().clone();
    let prepared = map_core::displacement::surface::prepare(&base);
    assert!(prepared.issues.is_empty(), "{:?}", prepared.issues);
    assert_eq!(prepared.meshes.len(), 3);
    for anchor in [0, 2] {
        let mut document = Document::new(base.clone());
        let mut ordered = sources.clone();
        let first = ordered.remove(anchor);
        ordered.insert(0, first);
        let ObjectId::Entity(id) = document
            .edit("one terrain and flat overlay", |tx| {
                overlay::create_faces(
                    tx,
                    &ordered,
                    overlay::Options {
                        material: "decals/decalgraffiti001a".into(),
                        targetname: Some("multiple_terrain_mark".into()),
                        ..Default::default()
                    },
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            overlay::parse(document.entity(id).unwrap()).unwrap().sides,
            sources.iter().map(|source| source.side).collect::<Vec<_>>()
        );
        let authored = document.vmf().clone();
        let output = compiler::compile(&authored, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let data = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(data.overlays.len(), 1);
        let record = data.overlays[0];
        let (origin, min, max) = if anchor == 0 {
            ([-157.0, 4.0, 44.0], -48.0, 365.0)
        } else {
            ([160.0, 4.0, 44.0], -365.0, 48.0)
        };
        assert_eq!(record.origin, origin);
        assert_eq!(record.basis_normal, [0.0, 0.0, 1.0]);
        assert_eq!(
            record.uv_points,
            [
                [min, -64.0, 1.0],
                [min, 64.0, 0.0],
                [max, 64.0, 0.0],
                [max, -64.0, 0.0]
            ]
        );
        let bound = record.faces[..record.face_count()]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let terrain = bsp.displacements().unwrap().unwrap();
        assert_eq!(terrain.infos.len(), 3);
        let mut selected_terrain = 0;
        for (index, info) in terrain.infos.iter().enumerate() {
            let quad = bsp.displacement_quad(index).unwrap();
            assert!(quad.iter().all(|point| point[2] == 32.0));
            let points = terrain.positions(index, quad).unwrap();
            assert!(points.iter().all(|point| point[2] == 44.0));
            assert!(
                info.edges
                    .iter()
                    .flatten()
                    .all(|edge| edge.index == u16::MAX)
            );
            let center = points[points.len() / 2];
            let source = if center[0] < -100.0 {
                Some(sources[0])
            } else if center[0] > -50.0 {
                Some(sources[1])
            } else {
                None
            };
            assert_eq!(
                bound.contains(&i32::from(info.face)),
                source.is_some(),
                "native terrain center {center:?}"
            );
            if let Some(source) = source {
                selected_terrain += 1;
                let expected = prepared.get(&source).unwrap();
                assert_eq!(points.len(), expected.positions.len());
                for (native, actual) in points.iter().zip(&expected.positions) {
                    assert!(DVec3::from_array(native.map(f64::from)).abs_diff_eq(*actual, 0.00001));
                    assert!((native[0] - origin[0]) >= min && (native[0] - origin[0]) <= max);
                    assert!(native[1] >= -60.0 && native[1] <= 68.0);
                }
                assert_eq!(
                    center,
                    if source == sources[0] {
                        [-157.0, 4.0, 44.0]
                    } else {
                        [3.0, 4.0, 44.0]
                    }
                );
            }
        }
        assert_eq!(selected_terrain, 2);
        let faces = bsp.records::<source_formats::bsp::Face>().unwrap();
        let vertices = bsp
            .values::<[f32; 3]>(source_formats::bsp::lump::VERTEXES)
            .unwrap();
        let edges = bsp.records::<source_formats::bsp::Edge>().unwrap();
        let surfedges = bsp
            .values::<i32>(source_formats::bsp::lump::SURFEDGES)
            .unwrap();
        let mut flat_faces = 0;
        for index in &bound {
            if terrain
                .infos
                .iter()
                .any(|info| i32::from(info.face) == *index)
            {
                continue;
            }
            flat_faces += 1;
            let face = faces[*index as usize];
            for &edge in &surfedges
                [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
            {
                let point = vertices
                    [usize::from(edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)])];
                assert!(
                    point[0] >= 112.0
                        && point[0] <= 208.0
                        && point[1] >= -60.0
                        && point[1] <= 68.0
                        && point[2] == 44.0,
                    "unselected native flat binding: {point:?}"
                );
            }
        }
        assert!(flat_faces > 0);
        let mut literal = Document::new(base.clone());
        literal
            .edit("literal imported multi-terrain projection", |tx| {
                edit::create_entity(
                    tx,
                    "info_overlay",
                    Some(DVec3::from_array(origin.map(f64::from))),
                    &[
                        ("material".into(), "decals/decalgraffiti001a".into()),
                        ("targetname".into(), "multiple_terrain_mark".into()),
                        (
                            "sides".into(),
                            sources
                                .iter()
                                .map(|source| source.side.to_string())
                                .collect::<Vec<_>>()
                                .join(" "),
                        ),
                        (
                            "BasisOrigin".into(),
                            if anchor == 0 { "-157 4 44" } else { "160 4 44" }.into(),
                        ),
                        ("BasisNormal".into(), "0 0 1".into()),
                        ("BasisU".into(), "1 0 0".into()),
                        ("BasisV".into(), "0 1 0".into()),
                        ("uv0".into(), format!("{min} -64 0")),
                        ("uv1".into(), format!("{min} 64 0")),
                        ("uv2".into(), format!("{max} 64 0")),
                        ("uv3".into(), format!("{max} -64 0")),
                    ],
                    Vec::new(),
                )
            })
            .unwrap();
        let expected = compiler::compile(literal.vmf(), &common::empty_game());
        assert!(!expected.report.has_errors(), "{:?}", expected.report);
        assert_eq!(expected.bsp.unwrap().to_bytes(), bsp.to_bytes());
        let reopened = Vmf::parse(&authored.write(&Default::default())).unwrap();
        let repeated = compiler::compile(&reopened, &common::empty_game());
        assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
        assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
        assert_eq!(document.vmf(), &authored);
    }
}

#[test]
fn fitted_planar_terrain_matches_literal_native_projection_and_original_displacement_binding() {
    let mut document = Document::new(common::room());
    let source = document
        .edit("isolated displaced target", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 0.0),
                DVec3::new(64.0, 64.0, 32.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let source = FaceId {
                solid,
                side: tx.solid_mut(solid)?.sides[0].id,
            };
            map_core::displacement::edit::create(tx, &[source], 2, 0)?;
            let info = tx.solid_mut(solid)?.sides[0].displacement.as_mut().unwrap();
            info.start_position = DVec3::new(-64.0, -64.0, 32.0);
            info.offsets = Some(vec![vec![DVec3::new(3.0, 4.0, 12.0); 5]; 5]);
            Ok(source)
        })
        .unwrap();
    let base = document.vmf().clone();
    document
        .edit("fitted terrain overlay", |tx| {
            overlay::create(
                tx,
                source,
                overlay::Options {
                    material: "decals/decalgraffiti001a".into(),
                    targetname: Some("terrain_mark".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let authored = document.vmf().clone();
    let output = compiler::compile(&authored, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    let data = bsp.overlay_data().unwrap().unwrap();
    assert_eq!(data.overlays.len(), 1);
    let record = data.overlays[0];
    assert_eq!(record.origin, [3.0, 4.0, 44.0]);
    assert_eq!(record.basis_normal, [0.0, 0.0, 1.0]);
    assert_eq!(
        record.uv_points,
        [
            [-64.0, -64.0, 1.0],
            [-64.0, 64.0, 0.0],
            [64.0, 64.0, 0.0],
            [64.0, -64.0, 0.0]
        ]
    );
    let terrain = bsp.displacements().unwrap().unwrap();
    assert_eq!(terrain.infos.len(), 1);
    assert_eq!(record.face_count(), 1);
    assert_eq!(record.faces[0], i32::from(terrain.infos[0].face));
    let points = terrain
        .positions(0, bsp.displacement_quad(0).unwrap())
        .unwrap();
    assert_eq!(points[0], [-61.0, -60.0, 44.0]);
    assert_eq!(points[12], [3.0, 4.0, 44.0]);
    assert_eq!(points[24], [67.0, 68.0, 44.0]);
    let mut literal = Document::new(base);
    literal
        .edit("literal imported projection", |tx| {
            edit::create_entity(
                tx,
                "info_overlay",
                Some(DVec3::new(3.0, 4.0, 44.0)),
                &[
                    ("material".into(), "decals/decalgraffiti001a".into()),
                    ("targetname".into(), "terrain_mark".into()),
                    ("sides".into(), source.side.to_string()),
                    ("BasisOrigin".into(), "3 4 44".into()),
                    ("BasisNormal".into(), "0 0 1".into()),
                    ("BasisU".into(), "1 0 0".into()),
                    ("BasisV".into(), "0 1 0".into()),
                    ("uv0".into(), "-64 -64 0".into()),
                    ("uv1".into(), "-64 64 0".into()),
                    ("uv2".into(), "64 64 0".into()),
                    ("uv3".into(), "64 -64 0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let expected = compiler::compile(literal.vmf(), &common::empty_game());
    assert!(!expected.report.has_errors(), "{:?}", expected.report);
    assert_eq!(expected.bsp.unwrap().to_bytes(), bsp.to_bytes());
    assert_eq!(document.vmf(), &authored);
    let reopened = Vmf::parse(&authored.write(&Default::default())).unwrap();
    let repeated = compiler::compile(&reopened, &common::empty_game());
    assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
    assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
}

#[test]
fn coplanar_multiface_authoring_binds_only_selected_native_faces_across_a_gap() {
    for gap in [0.0, 64.0] {
        let mut document = Document::new(common::room());
        let sources = document
            .edit("selected plates and unbound control", |tx| {
                let mut sources = Vec::new();
                let mut ranges = vec![[-128.0, -gap * 0.5], [gap * 0.5, 128.0]];
                if gap > 0.0 {
                    ranges.push([-gap * 0.5, gap * 0.5]);
                }
                for (index, [min, max]) in ranges.into_iter().enumerate() {
                    let ObjectId::Solid(solid) = edit::create_block(
                        tx,
                        DVec3::new(min, -64.0, 16.0),
                        DVec3::new(max, 64.0, 32.0),
                        "dev/dev_measuregeneric01",
                    )?
                    else {
                        unreachable!()
                    };
                    let brush = tx.solid_mut(solid)?;
                    let side = brush
                        .sides
                        .iter()
                        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                        .unwrap()
                        .id;
                    if index < 2 {
                        sources.push(FaceId { solid, side });
                    }
                    if index == 1 {
                        edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                    }
                }
                Ok(sources)
            })
            .unwrap();
        document
            .edit("one overlay", |tx| {
                overlay::create_faces(
                    tx,
                    &sources,
                    overlay::Options {
                        material: "decals/decalgraffiti001a".into(),
                        targetname: Some("multi_mark".into()),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        let output = compiler::compile(&before, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let data = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(data.overlays.len(), 1);
        let record = data.overlays[0];
        assert!(record.face_count() > 0);
        let faces = bsp.records::<source_formats::bsp::Face>().unwrap();
        let vertices = bsp
            .values::<[f32; 3]>(source_formats::bsp::lump::VERTEXES)
            .unwrap();
        let edges = bsp.records::<source_formats::bsp::Edge>().unwrap();
        let surfedges = bsp
            .values::<i32>(source_formats::bsp::lump::SURFEDGES)
            .unwrap();
        let mut coverage = [false; 2];
        for &index in &record.faces[..record.face_count()] {
            let face = faces[index as usize];
            let points = surfedges
                [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
                .iter()
                .map(|&edge| {
                    vertices
                        [usize::from(edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)])]
                })
                .collect::<Vec<_>>();
            assert!(points.iter().all(|point| point[2] == 32.0
                && point[0] >= -128.0
                && point[0] <= 128.0
                && point[1] >= -64.0
                && point[1] <= 64.0));
            let min = points
                .iter()
                .map(|point| point[0])
                .fold(f32::INFINITY, f32::min);
            let max = points
                .iter()
                .map(|point| point[0])
                .fold(f32::NEG_INFINITY, f32::max);
            if gap > 0.0 {
                assert!(
                    max <= -gap as f32 * 0.5 || min >= gap as f32 * 0.5,
                    "native binding spans unselected gap: {points:?}"
                );
            }
            coverage[0] |= min < -gap as f32 * 0.5;
            coverage[1] |= max > gap as f32 * 0.5;
        }
        assert_eq!(coverage, [true, true]);
        assert!(
            bsp.entities()
                .unwrap()
                .contains("\"targetname\" \"multi_mark\"")
        );
        let reopened = Vmf::parse(&before.write(&Default::default())).unwrap();
        let repeated = compiler::compile(&reopened, &common::empty_game());
        assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
        assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
        assert_eq!(document.vmf(), &before);
    }
}

#[test]
fn core_fitted_world_and_detail_quads_use_the_native_overlay_pipeline() {
    for detail in [false, true] {
        let mut document = Document::new(common::room());
        let face = document
            .edit("tilted overlay target", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-48.0, 32.0, 32.0),
                    DVec3::new(48.0, 128.0, 48.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let turn = DQuat::from_rotation_x(0.2);
                let solid = tx.solid_mut(id)?;
                for side in &mut solid.sides {
                    for point in &mut side.plane.0 {
                        point.x *= 1.0 + point.y / 512.0;
                        *point = turn * *point;
                    }
                }
                let side = solid
                    .sides
                    .iter()
                    .find(|side| {
                        Plane::from_points(side.plane.0)
                            .unwrap()
                            .normal
                            .dot(turn * DVec3::Z)
                            > 0.99
                    })
                    .unwrap()
                    .id;
                if detail {
                    edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
                }
                Ok(FaceId { solid: id, side })
            })
            .unwrap();
        let options = overlay::Options {
            material: "Materials/Decals/decalgraffiti001a.VMT".into(),
            targetname: Some("  fitted marking  ".into()),
            render_order: 2,
            u: [1.0, 0.0],
            fade_min: 16.0,
            fade_max: 64.0,
            ..Default::default()
        };
        let definition = overlay::fit(document.vmf(), face, options.clone()).unwrap();
        document
            .edit("Fit overlay to face", |tx| {
                overlay::create(tx, face, options)
            })
            .unwrap();
        let source = document.into_vmf();
        let before = source.clone();
        let output = compiler::compile(&source, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let data = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(data.overlays.len(), 1);
        let record = data.overlays[0];
        assert!(record.face_count() > 0);
        assert_eq!(record.render_order(), 2);
        assert_eq!(record.u, definition.options.u);
        assert_eq!(record.v, definition.options.v);
        assert_eq!(
            record.origin,
            definition.basis_origin.map(|value| value as f32)
        );
        assert_eq!(
            record.basis_normal,
            definition.basis_normal.map(|value| value as f32)
        );
        for (index, point) in record.uv_points.iter().enumerate() {
            assert_eq!(
                [point[0], point[1]],
                [
                    definition.uv_points[index][0] as f32,
                    definition.uv_points[index][1] as f32
                ]
            );
        }
        assert_eq!(data.fades[0].min_distance_squared, 256.0);
        assert_eq!(data.fades[0].max_distance_squared, 4096.0);
        assert!(
            bsp.entities()
                .unwrap()
                .contains("\"targetname\" \"  fitted marking  \"")
        );
        assert_eq!(source, before);
        let reopened = Vmf::parse(&source.write(&Default::default())).unwrap();
        let repeated =
            compiler::compile_controlled(&reopened, &common::empty_game(), &Default::default());
        assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
        assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
    }
}

#[test]
fn edited_local_corners_preserve_native_binding_frame_ranges_and_fades() {
    for detail in [false, true] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(solid) = document
            .edit("target", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-64.0, -64.0, 32.0),
                    DVec3::new(64.0, 64.0, 48.0),
                    "dev/dev_measuregeneric01",
                )
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
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let source = FaceId { solid, side };
        if detail {
            document
                .edit("detail", |tx| {
                    edit::tie_to_entity(tx, &[solid], "func_detail", &[])
                })
                .unwrap();
        }
        let ObjectId::Entity(id) = document
            .edit("overlay", |tx| {
                overlay::create(
                    tx,
                    source,
                    overlay::Options {
                        material: "decals/decalgraffiti001a".into(),
                        targetname: Some("corner_mark".into()),
                        render_order: 2,
                        u: [1.0, 0.0],
                        fade_min: 16.0,
                        fade_max: 64.0,
                        ..Default::default()
                    },
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let baseline = compiler::compile(document.vmf(), &common::empty_game());
        assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
        let baseline = baseline.bsp.unwrap().overlay_data().unwrap().unwrap();
        let corners = [
            [-24.5, -16.25],
            [-20.0, 24.75],
            [28.5, 16.5],
            [32.25, -20.125],
        ];
        document
            .edit("corners", |tx| overlay::edit_corners(tx, id, corners))
            .unwrap();
        let source = document.into_vmf();
        let output = compiler::compile(&source, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let data = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(data.overlays.len(), 1);
        assert_eq!(data.fades, baseline.fades);
        let mut expected = baseline.overlays[0];
        for (point, corner) in expected.uv_points.iter_mut().zip(corners) {
            point[0] = corner[0] as f32;
            point[1] = corner[1] as f32;
        }
        assert_eq!(data.overlays[0], expected);
        let reopened = Vmf::parse(&source.write(&Default::default())).unwrap();
        let repeated =
            compiler::compile_controlled(&reopened, &common::empty_game(), &Default::default());
        assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
        assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
    }
}
