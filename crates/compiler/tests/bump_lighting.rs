mod common;

use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, LightingKind, Model, TexInfo, surf};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::DispInfo;

fn game(bump: bool) -> (tempfile::TempDir, GameFs) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("materials/dev")).unwrap();
    std::fs::write(
        dir.path().join("materials/dev/dev_measuregeneric01.vmt"),
        if bump {
            "LightmappedGeneric { $bumpmap dev/normal }"
        } else {
            "LightmappedGeneric {}"
        },
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(dir.path().to_owned()),
    });
    (dir, game)
}

fn built(document: &Document, game: &GameFs, bounces: u8) -> compiler::Output {
    let result = compiler::compile_with_options(
        document.vmf(),
        game,
        Options {
            lighting: LightingMode::DirectLdr,
            bounces,
            sky_quality: SkyQuality::Fast,
            ..Default::default()
        },
    );
    assert!(!result.report.has_errors(), "{:?}", result.report);
    result
}

fn light(document: &mut Document, named: bool) {
    document
        .edit("bump lights", |tx| {
            let mut props = vec![
                (
                    "_light".into(),
                    if named { "0 0 255 200" } else { "255 0 0 200" }.into(),
                ),
                ("_constant_attn".into(), "1".into()),
            ];
            if named {
                props.push(("targetname".into(), "blue".into()));
            }
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 124.0)),
                &props,
                Vec::new(),
            )
        })
        .unwrap();
}

fn sample(
    bsp: &Bsp,
    model: usize,
    pivot: DVec3,
    target: DVec3,
    style: usize,
    direction: usize,
) -> DVec3 {
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || (f64::from(plane.dist) + pivot.z - target.z).abs() > 1e-6 {
            continue;
        }
        let info = infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::Z,
                dist: f64::from(plane.dist),
            },
            &info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if (frame.position(s, t).unwrap() + pivot).distance(target) < 1e-6 {
                    return DVec3::from_array(
                        lighting
                            .face(face, &info)
                            .unwrap()
                            .unwrap()
                            .samples(style, direction)
                            .unwrap()[t * width + s]
                            .linear(),
                    );
                }
            }
        }
    }
    panic!("Missing directional receiver");
}

fn check_base_maps(plain: &Bsp, bumped: &Bsp) {
    let normal = plain.lighting(LightingKind::Ldr).unwrap().unwrap();
    let directional = bumped.lighting(LightingKind::Ldr).unwrap().unwrap();
    let ordinary = plain.records::<Face>().unwrap();
    let faces = bumped.records::<Face>().unwrap();
    let normal_infos = plain.records::<TexInfo>().unwrap();
    let infos = bumped.records::<TexInfo>().unwrap();
    for (before, face) in ordinary.iter().zip(&faces) {
        let base = normal
            .face(before, &normal_infos[before.texinfo as usize])
            .unwrap();
        let maps = directional
            .face(face, &infos[face.texinfo as usize])
            .unwrap();
        if let (Some(base), Some(maps)) = (base, maps) {
            assert_eq!(maps.maps(), 4);
            assert_eq!(maps.styles(), base.styles());
            for style in 0..maps.styles().len() {
                assert_eq!(
                    maps.samples(style, 0).unwrap(),
                    base.samples(style, 0).unwrap()
                );
                assert_eq!(maps.average(style), base.average(style));
            }
        }
    }
}

#[test]
fn native_directional_style_maps_keep_base_samples_averages_and_order() {
    let (_plain_dir, plain_game) = game(false);
    let (_bump_dir, bump_game) = game(true);
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
    light(&mut document, false);
    light(&mut document, true);
    let source = document.vmf().clone();
    for bounces in [0, 1, 2] {
        let plain = built(&document, &plain_game, bounces).bsp.unwrap();
        let result = built(&document, &bump_game, bounces);
        assert!(result.stats.bump_faces > 0 && result.stats.bump_lightmaps > 0);
        assert_eq!(
            result.stats.bump_lightmaps,
            result.stats.light_style_maps * 3 / 4
        );
        let bumped = result.bsp.unwrap();
        check_base_maps(&plain, &bumped);
        for style in [0, 1] {
            let base = sample(&bumped, 0, DVec3::ZERO, DVec3::ZERO, style, 0);
            for direction in 1..4 {
                let actual = sample(&bumped, 0, DVec3::ZERO, DVec3::ZERO, style, direction);
                assert!(actual.y == 0.0 && actual[if style == 0 { 2 } else { 0 }] == 0.0);
                assert!(actual[if style == 0 { 0 } else { 2 }] > 0.0);
                if bounces == 0 {
                    assert!(
                        actual.distance(base * (1.0_f64 / 3.0).sqrt()) < 2.0,
                        "{actual}, {base}"
                    );
                }
            }
        }
    }
    assert_eq!(*document.vmf(), source);
}

#[test]
fn model_directional_maps_match_at_fractional_pivots_and_receive_bounce() {
    let (_plain_dir, plain_game) = game(false);
    let (_bump_dir, bump_game) = game(true);
    let mut reference = None;
    for pivot in [
        DVec3::ZERO,
        DVec3::new(128.0, -64.0, 16.0),
        DVec3::new(101.5, -67.25, 32.125),
    ] {
        let mut document = Document::new(common::room());
        document
            .edit("model", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-32.0, -32.0, 8.0),
                    DVec3::new(32.0, 32.0, 24.0),
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
        light(&mut document, false);
        let direct = built(&document, &bump_game, 0).bsp.unwrap();
        let plain = built(&document, &plain_game, 0).bsp.unwrap();
        check_base_maps(&plain, &direct);
        let colors = (0..4)
            .map(|map| sample(&direct, 1, pivot, DVec3::new(0.0, 0.0, 24.0), 0, map))
            .collect::<Vec<_>>();
        if let Some(previous) = &reference {
            assert_eq!(&colors, previous);
        } else {
            reference = Some(colors.clone());
        }
        let bounced = built(&document, &bump_game, 1);
        assert!(bounced.stats.model_bounce_samples > 0);
        let bsp = bounced.bsp.unwrap();
        for (map, direct) in colors.iter().enumerate() {
            assert!(sample(&bsp, 1, pivot, DVec3::new(0.0, 0.0, 24.0), 0, map).x > direct.x);
        }
    }
}

#[test]
fn terrain_keeps_material_bump_flags_and_four_checked_maps() {
    let (_dir, game) = game(true);
    let mut document = Document::new(common::room());
    document
        .edit("terrain", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(64.0, 64.0, 0.0),
                DVec3::new(192.0, 192.0, 16.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let solid = tx.solid_mut(id)?;
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            let top = geometry
                .faces
                .iter()
                .find(|face| face.plane.normal.z > 0.99)
                .unwrap();
            solid.sides[top.side].displacement = Some(DispInfo {
                power: 2,
                start_position: top.vertices[0],
                flags: Some(0),
                elevation: 0.0,
                subdivided: false,
                normals: Some(vec![vec![DVec3::Z; 5]; 5]),
                distances: Some(
                    (0..5)
                        .map(|row| (0..5).map(|col| (row + col * 2) as f64).collect())
                        .collect(),
                ),
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
    light(&mut document, false);
    let result = built(&document, &game, 0);
    let bsp = result.bsp.unwrap();
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let face = faces.iter().find(|face| face.dispinfo >= 0).unwrap();
    let info = infos[face.texinfo as usize];
    assert_ne!(info.flags & surf::BUMPLIGHT, 0);
    let light = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    let maps = light.face(face, &info).unwrap().unwrap();
    assert_eq!(maps.maps(), 4);
    assert!((1..4).any(|map| maps.samples(0, map).unwrap() != maps.samples(0, 0).unwrap()));
    let samples = bsp.displacement_lightmap_samples(0).unwrap().unwrap();
    assert_eq!(
        samples.len(),
        (face.lightmap_size[0] + 1) as usize * (face.lightmap_size[1] + 1) as usize
    );
}

#[test]
fn sky_directional_maps_keep_the_same_base_and_bake_sun_and_ambient() {
    let (plain_dir, plain_game) = game(false);
    let (bump_dir, bump_game) = game(true);
    for dir in [&plain_dir, &bump_dir] {
        std::fs::write(
            dir.path().join("materials/sky.vmt"),
            "UnlitGeneric { %compilesky 1 }",
        )
        .unwrap();
    }
    let mut vmf = common::room();
    let ceiling = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0))
        .unwrap();
    let side = ceiling
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
        .unwrap();
    side.material = "sky".into();
    let mut document = Document::new(vmf);
    document
        .edit("sky", |tx| {
            edit::create_entity(
                tx,
                "light_environment",
                Some(DVec3::new(0.0, 0.0, 128.0)),
                &[
                    ("_light".into(), "255 0 0 200".into()),
                    ("_ambient".into(), "0 0 255 100".into()),
                    ("pitch".into(), "-90".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let plain = built(&document, &plain_game, 0).bsp.unwrap();
    let bumped = built(&document, &bump_game, 0).bsp.unwrap();
    check_base_maps(&plain, &bumped);
    for map in 1..4 {
        let color = sample(&bumped, 0, DVec3::ZERO, DVec3::ZERO, 0, map);
        assert!(color.x > 0.0 && color.z > 0.0 && color.y == 0.0, "{color}");
    }
}

fn suppression_scene() -> Document {
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
        .edit("normal-map suppression receivers", |tx| {
            for (mins, maxs, classname, origin) in [
                (
                    [-192.0, -128.0, 8.0],
                    [-160.0, -96.0, 24.0],
                    "func_detail",
                    None,
                ),
                (
                    [-32.0, -32.0, 8.0],
                    [32.0, 32.0, 24.0],
                    "func_brush",
                    Some("101.5 -67.25 32.125"),
                ),
            ] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    side.lightmap_scale = 64;
                }
                let properties =
                    origin.map_or_else(Vec::new, |value| vec![("origin".into(), value.into())]);
                edit::tie_to_entity(tx, &[id], classname, &properties)?;
            }
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(64.0, 64.0, 0.0),
                DVec3::new(192.0, 192.0, 16.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let solid = tx.solid_mut(id)?;
            for side in &mut solid.sides {
                side.lightmap_scale = 64;
            }
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            let top = geometry
                .faces
                .iter()
                .find(|face| face.plane.normal.z > 0.99)
                .unwrap();
            solid.sides[top.side].displacement = Some(DispInfo {
                power: 2,
                start_position: top.vertices[0],
                flags: Some(0),
                elevation: 0.0,
                subdivided: false,
                normals: Some(vec![vec![DVec3::Z; 5]; 5]),
                distances: Some(
                    (0..5)
                        .map(|row| {
                            (0..5)
                                .map(|column| {
                                    ((row - 2_i32).pow(2) + (column - 2_i32).pow(2)) as f64
                                })
                                .collect()
                        })
                        .collect(),
                ),
                offsets: None,
                offset_normals: None,
                alphas: None,
                triangle_tags: None,
                allowed_verts: None,
                extra_properties: Default::default(),
                extra: Vec::new(),
            });
            edit::create_block(
                tx,
                DVec3::new(128.0, -128.0, 0.0),
                DVec3::new(160.0, -96.0, 16.0),
                "dev/control",
            )?;
            for (classname, origin, properties) in [
                (
                    "light",
                    [-96.0, 0.0, 180.0],
                    vec![
                        ("_light", "255 0 0 100"),
                        ("_lightHDR", "0 0 255 200"),
                        ("_constant_attn", "1"),
                        ("_quadratic_attn", "0"),
                    ],
                ),
                (
                    "light_spot",
                    [96.0, 0.0, 180.0],
                    vec![
                        ("targetname", "suppression_style"),
                        ("angles", "-90 0 0"),
                        ("_cone", "60"),
                        ("_cone2", "85"),
                        ("_light", "0 255 0 60"),
                        ("_lightHDR", "255 0 0 120"),
                        ("_constant_attn", "1"),
                        ("_quadratic_attn", "0"),
                    ],
                ),
            ] {
                let properties = properties
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect::<Vec<_>>();
                edit::create_entity(
                    tx,
                    classname,
                    Some(DVec3::from_array(origin)),
                    &properties,
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    document
}

fn suppression_build(
    document: &Document,
    shader: &str,
    profile: &str,
    bounces: u8,
) -> compiler::Output {
    let (directory, game) = game(false);
    let normal = if profile == "ordinary" {
        ""
    } else {
        "$bumpmap keep/primary $bumpmap2 keep/secondary"
    };
    let suppression = match profile {
        "zero" => "$nodiffusebumplighting 0",
        "suppressed" => "$nodiffusebumplighting 1",
        _ => "",
    };
    std::fs::write(
        directory
            .path()
            .join("materials/dev/dev_measuregeneric01.vmt"),
        format!("{shader} {{ {normal} {suppression} }}"),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/dev/control.vmt"),
        "LightmappedGeneric {}",
    )
    .unwrap();
    let output = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting: LightingMode::DirectLdrHdr,
            bounces,
            sky_quality: SkyQuality::Fast,
            ..Default::default()
        },
    );
    assert!(
        !output.report.has_errors(),
        "{shader}/{profile}: {:?}",
        output.report
    );
    output
}

#[test]
fn canonical_suppression_shares_world_detail_model_terrain_styles_and_hdr_plans() {
    let document = suppression_scene();
    let original = document.vmf().clone();
    for shader in [
        "LightmappedGeneric",
        "WorldVertexTransition",
        "WorldTwoTextureBlend",
    ] {
        for bounces in [0, 1] {
            let ordinary = suppression_build(&document, shader, "ordinary", bounces);
            let enabled = suppression_build(&document, shader, "enabled", bounces);
            let zero = suppression_build(&document, shader, "zero", bounces);
            let suppressed = suppression_build(&document, shader, "suppressed", bounces);
            let plain = ordinary.bsp.as_ref().unwrap();
            let bumped = enabled.bsp.as_ref().unwrap();
            let disabled = suppressed.bsp.as_ref().unwrap();
            assert_eq!(disabled.to_bytes(), plain.to_bytes(), "{shader}/{bounces}");
            assert_eq!(bumped.to_bytes(), zero.bsp.as_ref().unwrap().to_bytes());
            assert_eq!(suppressed.stats.bump_faces, 0);
            assert_eq!(suppressed.stats.bump_lightmaps, 0);
            assert!(enabled.stats.bump_faces > 0 && enabled.stats.bump_lightmaps > 0);
            assert_eq!(
                suppressed.stats.lightmap_samples,
                ordinary.stats.lightmap_samples
            );
            assert_eq!(suppressed.stats.lighting_work, ordinary.stats.lighting_work);
            assert!(suppressed.stats.lighting_work < enabled.stats.lighting_work);
            assert_eq!(enabled.stats.lighting_work, zero.stats.lighting_work);
            assert_eq!(suppressed.stats.lit_brush_models, 1);
            assert_eq!(suppressed.stats.displacements, 1);
            assert!(suppressed.stats.styled_faces > 0);
            for (kind, stats) in [
                (LightingKind::Ldr, None),
                (LightingKind::Hdr, suppressed.stats.hdr.as_ref()),
            ] {
                if let Some(stats) = stats {
                    assert_eq!(stats.bump_faces, 0);
                    assert_eq!(stats.bump_lightmaps, 0);
                    assert!(stats.styled_faces > 0);
                }
                let infos = disabled.records::<TexInfo>().unwrap();
                let bumped_infos = bumped.records::<TexInfo>().unwrap();
                let faces = disabled.lighting_faces(kind).unwrap();
                let bumped_faces = bumped.lighting_faces(kind).unwrap();
                assert_eq!(faces.len(), bumped_faces.len());
                let world = disabled.records::<Model>().unwrap()[0];
                let world_faces =
                    world.first_face as usize..(world.first_face + world.face_count) as usize;
                let lighting = disabled.lighting(kind).unwrap().unwrap();
                let bumped_lighting = bumped.lighting(kind).unwrap().unwrap();
                let mut style_maps = 0;
                let mut directional = false;
                let mut terrain = false;
                let mut detail = false;
                for (index, (face, bumped_face)) in faces.iter().zip(&bumped_faces).enumerate() {
                    let info = infos[face.texinfo as usize];
                    let Some(view) = lighting.face(face, &info).unwrap() else {
                        continue;
                    };
                    let bumped_view = bumped_lighting
                        .face(bumped_face, &bumped_infos[bumped_face.texinfo as usize])
                        .unwrap()
                        .unwrap();
                    assert_eq!(info.flags & surf::BUMPLIGHT, 0);
                    assert_eq!(view.maps(), 1);
                    assert_eq!(view.styles(), bumped_view.styles());
                    terrain |= face.dispinfo >= 0;
                    detail |=
                        world_faces.contains(&index) && face.on_node == 0 && face.dispinfo < 0;
                    style_maps += view.styles().len();
                    for style in 0..view.styles().len() {
                        assert_eq!(view.samples(style, 0), bumped_view.samples(style, 0));
                        assert_eq!(view.average(style), bumped_view.average(style));
                        if bumped_view.maps() == 4 {
                            directional |=
                                bumped_view.samples(style, 1) != bumped_view.samples(style, 0);
                        }
                    }
                }
                assert!(terrain && detail && directional);
                let expected_maps = stats.map_or(suppressed.stats.light_style_maps, |stats| {
                    stats.light_style_maps
                });
                assert_eq!(style_maps, expected_maps);
            }
            assert_eq!(document.vmf(), &original);
        }
    }
}

#[test]
fn unsupported_primary_normal_suppression_fails_atomically_with_material_context() {
    let (directory, game) = game(true);
    let mut document = Document::new(common::room());
    light(&mut document, false);
    let original = document.vmf().clone();
    let destination = directory.path().join("retained.bsp");
    built(&document, &game, 0).write(&destination).unwrap();
    let retained = std::fs::read(&destination).unwrap();
    let material = directory
        .path()
        .join("materials/dev/dev_measuregeneric01.vmt");
    for shader in [
        "LightmappedGeneric",
        "WorldVertexTransition",
        "WorldTwoTextureBlend",
    ] {
        for value in ["2", "true", "1.0", "", "01", "+1", "-1", "NaN"] {
            std::fs::write(
                &material,
                format!("{shader} {{ $bumpmap keep/primary $nodiffusebumplighting \"{value}\" }}"),
            )
            .unwrap();
            for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
                let failed = compiler::compile_with_options(
                    document.vmf(),
                    &game,
                    Options {
                        lighting,
                        ..Default::default()
                    },
                );
                assert!(failed.bsp.is_none());
                assert!(
                    failed
                        .report
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.level == compiler::Level::Error
                            && diagnostic
                                .message
                                .contains("materials/dev/dev_measuregeneric01.vmt")
                            && diagnostic.message.contains("$nodiffusebumplighting")
                            && diagnostic.message.contains("literal 0 or 1")),
                    "{:?}",
                    failed.report
                );
                assert!(!failed.write(&destination).unwrap());
                assert_eq!(std::fs::read(&destination).unwrap(), retained);
                assert_eq!(document.vmf(), &original);
            }
        }
    }
    std::fs::write(
        directory.path().join("materials/dev/base.vmt"),
        "WorldVertexTransition { $bumpmap keep/primary $nodiffusebumplighting 0 }",
    )
    .unwrap();
    std::fs::write(
        &material,
        "Patch { include materials/dev/base.vmt replace { $nodiffusebumplighting 2 } }",
    )
    .unwrap();
    let failed = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting: LightingMode::DirectLdrHdr,
            ..Default::default()
        },
    );
    assert!(failed.bsp.is_none());
    assert!(failed.report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("materials/dev/dev_measuregeneric01.vmt")
            && diagnostic.message.contains("literal 0 or 1")
    }));
    assert!(!failed.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), retained);
    assert_eq!(document.vmf(), &original);
}
