mod common;

use compiler::{LightingMode, Options, SkyQuality};
use glam::{DMat3, DVec3};
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, Model, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const PIVOT: DVec3 = DVec3::new(5.5, -7.25, 64.125);
const OFF_AXIS: [f64; 9] = [
    0.7697511313200572,
    0.5389855446957562,
    -0.3420201433256687,
    -0.5162121193659353,
    0.8407726623973335,
    0.16317591116653482,
    0.37551064385876115,
    0.050950100827363065,
    0.9254165783983234,
];

fn rotations() -> [(&'static str, DMat3); 6] {
    [
        (
            "0 90 0",
            DMat3::from_cols_array(&[0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
        ),
        (
            "90 0 0",
            DMat3::from_cols_array(&[0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]),
        ),
        (
            "-90 0 0",
            DMat3::from_cols_array(&[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0]),
        ),
        (
            "0 0 90",
            DMat3::from_cols_array(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0]),
        ),
        ("20 35 10", DMat3::from_cols_array(&OFF_AXIS)),
        ("360 -360 360", DMat3::IDENTITY),
    ]
}

fn built(source: &Vmf, game: &GameFs, bounces: u8) -> compiler::Output {
    let output = compiler::compile_with_options(
        source,
        game,
        Options {
            lighting: LightingMode::DirectLdrHdr,
            bounces,
            sky_quality: SkyQuality::Fast,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn receiver(smooth: bool, mirror: bool) -> Vmf {
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    let mut document = Document::new(source);
    document
        .edit("rotation receiver", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 32.0),
                DVec3::new(32.0, 32.0, 96.0),
                "test/receiver",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[solid],
                "func_brush",
                &[("origin".into(), source_formats::vmf::format_vec3(PIVOT))],
            )?;
            Ok(())
        })
        .unwrap();
    let mut source = document.vmf().clone();
    for side in source
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .flat_map(|solid| &mut solid.sides)
    {
        side.smoothing_groups = smooth.then_some(1);
        if Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z {
            side.u_axis.axis = DVec3::X;
            side.v_axis.axis = if mirror { -DVec3::Y } else { DVec3::Y };
        }
    }
    source
}

fn pose(source: &mut Vmf, angles: &str) {
    source
        .entities
        .iter_mut()
        .find(|entity| !entity.solids.is_empty())
        .unwrap()
        .properties
        .set("angles", angles);
}

fn add_light(source: Vmf, origin: DVec3, named: bool, ldr: &str, hdr: &str) -> Vmf {
    let mut document = Document::new(source);
    document
        .edit("pose light", |tx| {
            let mut properties = vec![
                ("_light".into(), ldr.into()),
                ("_lightHDR".into(), hdr.into()),
                ("_constant_attn".into(), "1".into()),
                ("_linear_attn".into(), "0".into()),
                ("_quadratic_attn".into(), "0".into()),
            ];
            if named {
                properties.push(("targetname".into(), "rotated_style".into()));
            }
            edit::create_entity(tx, "light", Some(origin), &properties, Vec::new())?;
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn face_sample(
    bsp: &Bsp,
    model: usize,
    target: DVec3,
    normal: DVec3,
    kind: LightingKind,
    style: u8,
    map: usize,
) -> DVec3 {
    let models = bsp.records::<Model>().unwrap();
    let model = models[model];
    let faces = bsp.lighting_faces(kind).unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let plane = planes[face.plane as usize];
        let actual_normal = DVec3::from_array(plane.normal.map(f64::from));
        if actual_normal.distance(normal) > 1e-6
            || (actual_normal.dot(target) - f64::from(plane.dist)).abs() > 1e-5
        {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let rows = info
            .lightmap_vecs
            .map(|row| DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])));
        let coordinates = info.lightmap_vecs.map(|row| {
            DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])).dot(target)
                + f64::from(row[3])
        });
        let s = coordinates[0].round() as i32 - face.lightmap_mins[0];
        let t = coordinates[1].round() as i32 - face.lightmap_mins[1];
        if s < 0 || t < 0 || s > face.lightmap_size[0] || t > face.lightmap_size[1] {
            continue;
        }
        let rhs = DVec3::new(
            f64::from(s + face.lightmap_mins[0]) - f64::from(info.lightmap_vecs[0][3]),
            f64::from(t + face.lightmap_mins[1]) - f64::from(info.lightmap_vecs[1][3]),
            f64::from(plane.dist),
        );
        let decoded = DMat3::from_cols(rows[0], rows[1], actual_normal)
            .transpose()
            .inverse()
            * rhs;
        if decoded.distance(target) > 1e-5 {
            continue;
        }
        let data = lighting.face(face, info).unwrap().unwrap();
        let style = data
            .styles()
            .iter()
            .position(|candidate| *candidate == style)
            .unwrap_or_else(|| panic!("Missing native style {style} at local receiver {target:?}"));
        let width = face.lightmap_size[0] as usize + 1;
        return DVec3::from_array(
            data.samples(style, map).unwrap()[t as usize * width + s as usize].linear(),
        );
    }
    panic!("No native local receiver at {target:?} with normal {normal:?}");
}

fn local_payloads_equal(before: &Bsp, after: &Bsp) {
    for lump in [
        bsp::lump::PLANES,
        bsp::lump::VERTEXES,
        bsp::lump::EDGES,
        bsp::lump::SURFEDGES,
        bsp::lump::MODELS,
        bsp::lump::BRUSHES,
        bsp::lump::BRUSHSIDES,
        bsp::lump::TEXINFO,
        bsp::lump::VERTNORMALS,
        bsp::lump::VERTNORMALINDICES,
    ] {
        assert_eq!(
            before.lumps[lump], after.lumps[lump],
            "Changed local geometry/UV/normal lump {lump}"
        );
    }
}

#[derive(Debug, PartialEq)]
struct FaceColors {
    maps: usize,
    styles: Vec<u8>,
    payload: Vec<bsp::RgbExp32>,
}

fn model_colors(bsp: &Bsp, model: usize, kind: LightingKind) -> Vec<FaceColors> {
    let models = bsp.records::<Model>().unwrap();
    let model = models[model];
    let faces = bsp.lighting_faces(kind).unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let mut result = Vec::new();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let Some(maps) = lighting.face(face, &infos[face.texinfo as usize]).unwrap() else {
            continue;
        };
        let mut payload = Vec::new();
        for style in 0..maps.styles().len() {
            payload.push(*maps.average(style).unwrap());
            for map in 0..maps.maps() {
                payload.extend_from_slice(maps.samples(style, map).unwrap());
            }
        }
        result.push(FaceColors {
            maps: maps.maps(),
            styles: maps.styles().to_vec(),
            payload,
        });
    }
    result
}

#[test]
fn literal_source_matrices_move_points_and_normals_without_rotating_native_geometry() {
    let original = DVec3::new(0.0, 0.0, 96.0) - PIVOT;
    let lights = [
        DVec3::new(160.0, 80.0, 224.0),
        DVec3::new(-160.0, -120.0, 112.0),
    ];
    for (angles, matrix) in rotations() {
        let mut source = add_light(
            receiver(false, false),
            lights[0],
            false,
            "255 0 0 160",
            "0 255 0 320",
        );
        source = add_light(source, lights[1], false, "0 0 255 120", "255 0 0 240");
        let baseline = built(&source, &common::empty_game(), 0).bsp.unwrap();
        pose(&mut source, angles);
        let before = source.clone();
        let output = built(&source, &common::empty_game(), 0);
        assert_eq!(output.stats.lit_brush_models, 1);
        let bsp = output.bsp.unwrap();
        let point = PIVOT + matrix * original;
        let normal = matrix * DVec3::Z;
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            let factors = lights.map(|light| normal.dot((light - point).normalize()).max(0.0));
            let expected = if kind == LightingKind::Ldr {
                DVec3::new(160.0 * factors[0], 0.0, 120.0 * factors[1])
            } else {
                DVec3::new(240.0 * factors[1], 320.0 * factors[0], 0.0)
            };
            let actual = face_sample(&bsp, 1, original, DVec3::Z, kind, 0, 0);
            assert!(
                (actual - expected).abs().max_element() < 2.1,
                "{angles} {kind:?}: {actual:?} != {expected:?}"
            );
            assert_eq!(
                model_colors(&baseline, 0, kind),
                model_colors(&bsp, 0, kind)
            );
        }
        local_payloads_equal(&baseline, &bsp);
        let angle_line = format!("\"angles\" \"{angles}\"\n");
        assert!(bsp.entities().unwrap().contains(&angle_line));
        assert_eq!(
            bsp.entities().unwrap().replace(&angle_line, ""),
            baseline.entities().unwrap()
        );
        assert_eq!(source, before);
    }
}

#[test]
fn rotated_compound_hulls_self_shadow_and_leave_world_and_other_models_unchanged() {
    let pivot = DVec3::new(5.5, -7.25, 88.125);
    let matrix = rotations()[1].1;
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    let mut document = Document::new(source);
    document
        .edit("compound rotated hulls", |tx| {
            let mut solids = Vec::new();
            for (low, high) in [(32.0, 48.0), (128.0, 144.0)] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-48.0, -48.0, low),
                    DVec3::new(48.0, 48.0, high),
                    "test/receiver",
                )?
                else {
                    unreachable!()
                };
                solids.push(id);
            }
            edit::tie_to_entity(
                tx,
                &solids,
                "func_brush",
                &[("origin".into(), source_formats::vmf::format_vec3(pivot))],
            )?;
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-176.0, -16.0, 64.0),
                DVec3::new(-144.0, 16.0, 96.0),
                "test/receiver",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[id],
                "func_brush",
                &[("origin".into(), "-160 0 80".into())],
            )?;
            Ok(())
        })
        .unwrap();
    let source = add_light(
        document.vmf().clone(),
        pivot + matrix * (DVec3::new(0.0, 0.0, 224.0) - pivot),
        false,
        "255 255 255 150",
        "255 255 255 300",
    );
    let baseline = built(&source, &common::empty_game(), 0).bsp.unwrap();
    let mut source = source;
    pose(&mut source, "90 0 0");
    let before = source.clone();
    let bsp = built(&source, &common::empty_game(), 0).bsp.unwrap();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        assert_eq!(
            face_sample(
                &bsp,
                1,
                DVec3::new(0.0, 0.0, 48.0) - pivot,
                DVec3::Z,
                kind,
                0,
                0
            ),
            DVec3::ZERO
        );
        let bright = face_sample(
            &bsp,
            1,
            DVec3::new(0.0, 0.0, 144.0) - pivot,
            DVec3::Z,
            kind,
            0,
            0,
        );
        assert_eq!(
            bright,
            DVec3::splat(if kind == LightingKind::Ldr {
                150.0
            } else {
                300.0
            })
        );
        assert_eq!(
            model_colors(&baseline, 0, kind),
            model_colors(&bsp, 0, kind)
        );
        assert_eq!(
            model_colors(&baseline, 2, kind),
            model_colors(&bsp, 2, kind)
        );
    }
    local_payloads_equal(&baseline, &bsp);
    assert_eq!(source, before);
}

fn material_game(bump: bool) -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/test")).unwrap();
    std::fs::write(
        directory.path().join("materials/test/receiver.vmt"),
        if bump {
            "LightmappedGeneric { $bumpmap test/unused_normal }"
        } else {
            "LightmappedGeneric {}"
        },
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn base_maps_equal(plain: &Bsp, bumped: &Bsp, kind: LightingKind) {
    let plain_faces = plain.lighting_faces(kind).unwrap();
    let bumped_faces = bumped.lighting_faces(kind).unwrap();
    let plain_infos = plain.records::<TexInfo>().unwrap();
    let bumped_infos = bumped.records::<TexInfo>().unwrap();
    let plain_light = plain.lighting(kind).unwrap().unwrap();
    let bumped_light = bumped.lighting(kind).unwrap().unwrap();
    let model = plain.records::<Model>().unwrap()[1];
    let mut checked = 0;
    for index in model.first_face as usize..(model.first_face + model.face_count) as usize {
        let plain_face = &plain_faces[index];
        let bumped_face = &bumped_faces[index];
        let plain = plain_light
            .face(plain_face, &plain_infos[plain_face.texinfo as usize])
            .unwrap()
            .unwrap();
        let bumped = bumped_light
            .face(bumped_face, &bumped_infos[bumped_face.texinfo as usize])
            .unwrap()
            .unwrap();
        assert_eq!(plain.maps(), 1);
        assert_eq!(bumped.maps(), 4);
        assert_eq!(plain.styles(), bumped.styles());
        for style in 0..plain.styles().len() {
            assert_eq!(plain.samples(style, 0), bumped.samples(style, 0));
            assert_eq!(plain.average(style), bumped.average(style));
        }
        checked += 1;
    }
    assert_eq!(checked, 6);
}

#[test]
fn off_axis_smoothed_directional_maps_keep_handedness_styles_and_both_bounce_channels() {
    let (_plain_directory, plain_game) = material_game(false);
    let (_bump_directory, bump_game) = material_game(true);
    let matrix = DMat3::from_cols_array(&OFF_AXIS);
    let target = DVec3::new(16.0, 0.0, 96.0) - PIVOT;
    let point = PIVOT + matrix * target;
    let lights = [
        point + matrix * DVec3::new(64.0, 48.0, 128.0),
        point + matrix * DVec3::new(-64.0, 32.0, 112.0),
    ];
    let smooth = DVec3::new(0.5 / 3.0_f64.sqrt(), 0.0, 0.5 + 0.5 / 3.0_f64.sqrt()).normalize();
    let tangent = (DVec3::X - smooth * smooth.x).normalize();
    let bitangent = smooth.cross(tangent);
    let directional = [
        smooth,
        tangent * (2.0_f64 / 3.0).sqrt() + smooth / 3.0_f64.sqrt(),
        -tangent / 6.0_f64.sqrt() + bitangent / 2.0_f64.sqrt() + smooth / 3.0_f64.sqrt(),
        -tangent / 6.0_f64.sqrt() - bitangent / 2.0_f64.sqrt() + smooth / 3.0_f64.sqrt(),
    ];
    let mut mirrored = Vec::new();
    for mirror in [false, true] {
        let mut source = add_light(
            receiver(true, mirror),
            lights[0],
            false,
            "255 0 0 160",
            "0 255 0 320",
        );
        source = add_light(source, lights[1], true, "0 0 255 120", "255 0 0 240");
        let baseline = built(&source, &bump_game, 0).bsp.unwrap();
        pose(&mut source, "20 35 10");
        let before = source.clone();
        let mut direct_colors: Option<Vec<DVec3>> = None;
        let mut channels = Vec::new();
        for bounces in [0, 1, 4] {
            let output = built(&source, &bump_game, bounces);
            assert_eq!(output.stats.smoothed_faces, 6);
            assert_eq!(output.stats.lit_brush_models, 1);
            if bounces > 0 {
                assert!(output.stats.model_bounce_samples > 0);
            }
            let bsp = output.bsp.unwrap();
            let plain = built(&source, &plain_game, bounces).bsp.unwrap();
            local_payloads_equal(&baseline, &bsp);
            let mut colors = Vec::new();
            for kind in [LightingKind::Ldr, LightingKind::Hdr] {
                base_maps_equal(&plain, &bsp, kind);
                for (style, light) in [(0, lights[0]), (32, lights[1])] {
                    for map in 0..4 {
                        let actual = face_sample(&bsp, 1, target, DVec3::Z, kind, style, map);
                        if bounces == 0 {
                            let local_map = if mirror && map >= 2 { 5 - map } else { map };
                            let factor = (matrix * directional[local_map])
                                .dot((light - point).normalize())
                                .max(0.0);
                            let expected = match (kind, style) {
                                (LightingKind::Ldr, 0) => DVec3::X * (160.0 * factor),
                                (LightingKind::Ldr, _) => DVec3::Z * (120.0 * factor),
                                (LightingKind::Hdr, 0) => DVec3::Y * (320.0 * factor),
                                (LightingKind::Hdr, _) => DVec3::X * (240.0 * factor),
                            };
                            assert!(
                                (actual - expected).abs().max_element() < 2.1,
                                "mirror={mirror} {kind:?} style={style} map={map}: {actual:?} != {expected:?}"
                            );
                        }
                        colors.push(actual);
                    }
                }
            }
            if let Some(direct) = &direct_colors {
                assert!(
                    colors
                        .iter()
                        .zip(direct)
                        .all(
                            |(color, direct): (&DVec3, &DVec3)| (*color - *direct).min_element()
                                >= -2.0
                        )
                );
                assert!(
                    colors
                        .iter()
                        .zip(direct)
                        .any(|(color, direct)| (*color - *direct).max_element() > 0.5)
                );
            } else {
                direct_colors = Some(colors.clone());
            }
            channels.push(colors);
        }
        assert_eq!(source, before);
        mirrored.push(channels);
    }
    for (normal, mirror) in mirrored[0].iter().zip(&mirrored[1]) {
        assert_eq!(normal.len(), mirror.len());
        let (normal_groups, normal_tail) = normal.as_chunks::<4>();
        let (mirror_groups, mirror_tail) = mirror.as_chunks::<4>();
        assert!(normal_tail.is_empty() && mirror_tail.is_empty());
        for (normal, mirror) in normal_groups.iter().zip(mirror_groups) {
            assert_eq!(normal[0], mirror[0]);
            assert_eq!(normal[1], mirror[1]);
            assert_eq!(normal[2], mirror[3]);
            assert_eq!(normal[3], mirror[2]);
        }
    }
}

#[test]
fn rotated_smoothed_receiver_keeps_ray_bias_on_the_flat_geometric_plane() {
    let matrix = rotations()[2].1;
    let authored = DVec3::new(31.1875, 0.0, 96.0);
    let target = authored - PIVOT;
    let mut source = receiver(true, false);
    let top = source
        .entities
        .iter_mut()
        .find(|entity| !entity.solids.is_empty())
        .unwrap()
        .solids[0]
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap();
    top.lightmap_scale = 1;
    top.u_axis.offset = 3.25;
    let point = PIVOT + matrix * target;
    let source = add_light(
        source,
        point + matrix * DVec3::new(96.0, 0.0, -64.0),
        false,
        "255 255 255 200",
        "255 255 255 400",
    );
    let mut source = source;
    pose(&mut source, "-90 0 0");
    let before = source.clone();
    let bsp = built(&source, &common::empty_game(), 0).bsp.unwrap();
    let weight = authored.x / 32.0;
    let smooth = DVec3::new(
        weight / 3.0_f64.sqrt(),
        0.0,
        1.0 - weight + weight / 3.0_f64.sqrt(),
    )
    .normalize();
    assert!(smooth.dot(DVec3::new(96.0, 0.0, -64.0).normalize()) * 200.0 > 20.0);
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        assert_eq!(
            face_sample(&bsp, 1, target, DVec3::Z, kind, 0, 0),
            DVec3::ZERO
        );
        let edge = DVec3::new(32.1875, 0.0, 96.0) - PIVOT;
        assert!(face_sample(&bsp, 1, edge, DVec3::Z, kind, 0, 0).min_element() > 20.0);
    }
    assert_eq!(source, before);
}

fn pretransform_receiver(source: &mut Vmf, matrix: DMat3) {
    let model = source
        .entities
        .iter_mut()
        .find(|entity| !entity.solids.is_empty())
        .unwrap();
    for side in model.solids.iter_mut().flat_map(|solid| &mut solid.sides) {
        for point in &mut side.plane.0 {
            *point = PIVOT + matrix * (*point - PIVOT);
        }
        for axis in [&mut side.u_axis, &mut side.v_axis] {
            let rotated = matrix * axis.axis;
            axis.offset += (axis.axis - rotated).dot(PIVOT) / axis.scale;
            axis.axis = rotated;
        }
    }
}

#[test]
fn rotated_bounce_samples_match_an_independently_pretransformed_model_and_uv_reference() {
    let (_directory, game) = material_game(true);
    let matrix = rotations()[0].1;
    let target = DVec3::new(16.0, 0.0, 96.0) - PIVOT;
    let mut posed = add_light(
        receiver(true, true),
        DVec3::new(128.0, -64.0, 224.0),
        false,
        "255 0 0 160",
        "0 255 0 320",
    );
    posed = add_light(
        posed,
        DVec3::new(-96.0, 96.0, 192.0),
        true,
        "0 0 255 120",
        "255 0 0 240",
    );
    let mut reference = posed.clone();
    pretransform_receiver(&mut reference, matrix);
    pose(&mut posed, "0 90 0");
    let original = (posed.clone(), reference.clone());
    let mut direct = None;
    for bounces in [0, 1, 4] {
        let posed = built(&posed, &game, bounces).bsp.unwrap();
        let reference = built(&reference, &game, bounces).bsp.unwrap();
        let mut colors = Vec::new();
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            for style in [0, 32] {
                for map in 0..4 {
                    let actual = face_sample(&posed, 1, target, DVec3::Z, kind, style, map);
                    let expected = face_sample(
                        &reference,
                        1,
                        matrix * target,
                        matrix * DVec3::Z,
                        kind,
                        style,
                        map,
                    );
                    assert!(
                        (actual - expected).abs().max_element() < 2.1,
                        "bounces={bounces} {kind:?} style={style} map={map}: {actual:?} != {expected:?}"
                    );
                    colors.push(actual);
                }
            }
            assert_eq!(
                model_colors(&posed, 0, kind),
                model_colors(&reference, 0, kind)
            );
        }
        if let Some(direct) = &direct {
            assert!(
                colors
                    .iter()
                    .zip(direct)
                    .any(
                        |(color, direct): (&DVec3, &DVec3)| (*color - *direct).max_element() > 0.5
                    )
            );
        } else {
            direct = Some(colors);
        }
    }
    assert_eq!((posed, reference), original);
}
