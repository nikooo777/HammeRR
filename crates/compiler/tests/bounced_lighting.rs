mod common;

use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, LightingKind, TexInfo};
use source_formats::gamefs::{Mount, MountedPath};

fn settings(bounces: u8) -> Options {
    Options {
        lighting: LightingMode::DirectLdr,
        sky_quality: SkyQuality::Balanced,
        bounces,
        ..Default::default()
    }
}

fn light(document: &mut Document) {
    document
        .edit("light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(-128.0, 0.0, 128.0)),
                &[
                    ("_light".into(), "255 255 255 200".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
}

fn sample(bsp: &Bsp, position: DVec3) -> DVec3 {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let light = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in bsp.records::<Face>().unwrap() {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::Z,
                dist: 0.0,
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(position) < 0.001 {
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
    panic!("No floor sample at {position}")
}

fn albedo_game(albedo: [f32; 3]) -> (tempfile::TempDir, source_formats::gamefs::GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $basetexture dev/reflector }",
    )
    .unwrap();
    let mut texture = vec![0_u8; 80];
    texture[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        texture[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    texture[56] = 1;
    texture[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for (axis, value) in albedo.into_iter().enumerate() {
        texture[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    texture.extend_from_slice(&[255; 4]);
    std::fs::write(materials.join("reflector.vtf"), texture).unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

#[test]
fn first_bounce_lights_a_shadow_with_reflected_color_and_bounded_energy() {
    let (_directory, game) = albedo_game([0.5, 0.0, 0.0]);
    let mut document = Document::new(common::room());
    light(&mut document);
    document
        .edit("shadow shelf", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-96.0, -96.0, 48.0),
                DVec3::new(96.0, 96.0, 64.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let source = document.vmf().clone();
    let direct = compiler::compile_with_options(document.vmf(), &game, settings(0));
    let bounced = compiler::compile_with_options(document.vmf(), &game, settings(1));
    assert!(
        !direct.report.has_errors() && !bounced.report.has_errors(),
        "{:?} {:?}",
        direct.report,
        bounced.report
    );
    assert_eq!(bounced.stats.bounces, 1);
    assert_eq!(
        bounced.stats.bounce_rays,
        bounced.stats.lightmap_samples * SkyQuality::Balanced.directions()
    );
    assert!(
        bounced.stats.bounce_samples > 0
            && bounced.stats.bounce_work > 0
            && bounced.stats.lighting_work <= 100_000_000
    );
    let direct = direct.bsp.unwrap();
    let bounced = bounced.bsp.unwrap();
    assert_eq!(sample(&direct, DVec3::ZERO), DVec3::ZERO);
    let reflected = sample(&bounced, DVec3::ZERO);
    assert!(reflected.x > 1.0 && reflected.x <= 100.0, "{reflected}");
    assert_eq!(reflected.y, 0.0);
    assert_eq!(reflected.z, 0.0);
    for (index, (before, after)) in direct.lumps.iter().zip(&bounced.lumps).enumerate() {
        if ![
            bsp::lump::LIGHTING,
            bsp::lump::LEAF_AMBIENT_LIGHTING,
            bsp::lump::LEAF_AMBIENT_INDEX,
        ]
        .contains(&index)
        {
            assert_eq!(before, after, "lump {index}");
        }
    }
    let again = compiler::compile_with_options(document.vmf(), &game, settings(1));
    assert!(
        bounced.to_bytes() == again.bsp.unwrap().to_bytes(),
        "Repeated bounce output differs"
    );
    assert_eq!(*document.vmf(), source);
}

#[test]
fn zero_albedo_unlit_rooms_and_sealed_receivers_do_not_gain_bounce_energy() {
    for emitting in [false, true] {
        let (_directory, game) = albedo_game([0.0; 3]);
        let mut document = Document::new(common::room());
        if emitting {
            light(&mut document);
        }
        let direct = compiler::compile_with_options(document.vmf(), &game, settings(0));
        let bounced = compiler::compile_with_options(document.vmf(), &game, settings(1));
        assert!(!direct.report.has_errors() && !bounced.report.has_errors());
        assert_eq!(bounced.stats.bounce_samples, 0);
        assert!(
            direct.bsp.unwrap().to_bytes() == bounced.bsp.unwrap().to_bytes(),
            "Zero reflected energy changed the BSP (emitting={emitting})"
        );
    }
    let mut document = Document::new(common::room());
    light(&mut document);
    document
        .edit("sealed divide", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-8.0, -256.0, 0.0),
                DVec3::new(8.0, 256.0, 256.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), settings(1));
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(
        sample(&output.bsp.unwrap(), DVec3::new(128.0, 0.0, 0.0)),
        DVec3::ZERO
    );
}

#[test]
fn unsupported_bounce_settings_and_albedo_preserve_previous_output() {
    let mut document = Document::new(common::room());
    light(&mut document);
    let (directory, game) = albedo_game([2.0, 0.5, 0.5]);
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    for options in [settings(2), settings(1)] {
        let output = compiler::compile_with_options(document.vmf(), &game, options);
        assert!(output.report.has_errors());
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    }
    std::fs::write(
        directory.path().join("materials/good.vmt"),
        "LightmappedGeneric {}",
    )
    .unwrap();
    let mut scoped = document.vmf().clone();
    for side in scoped
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "good".into();
    }
    let mut scoped = Document::new(scoped);
    scoped
        .edit("unlit moving model", |tx| {
            let map_core::ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(160.0, 160.0, 32.0),
                DVec3::new(192.0, 192.0, 64.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!();
            };
            edit::tie_to_entity(tx, &[id], "func_brush", &[])
        })
        .unwrap();
    let output = compiler::compile_with_options(scoped.vmf(), &game, settings(1));
    assert!(
        !output.report.has_errors(),
        "Unused brush-model albedo rejected the bounce: {:?}",
        output.report
    );
    let output = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            bounces: 1,
            ..Default::default()
        },
    );
    let baseline = compiler::compile(document.vmf(), &game);
    assert!(!output.report.has_errors());
    assert_eq!(output.stats.bounces, 0);
    assert_eq!(output.stats.bounce_rays, 0);
    assert_eq!(
        output.bsp.unwrap().to_bytes(),
        baseline.bsp.unwrap().to_bytes()
    );
}

fn model_receiver(pivot: DVec3, enclosure: bool, separate: bool) -> Document {
    use map_core::ObjectId;
    let mut document = Document::new(common::room());
    light(&mut document);
    document
        .edit("model bounce", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-96.0, -96.0, 48.0),
                DVec3::new(96.0, 96.0, 64.0),
                "dev/dev_measuregeneric01",
            )?;
            let ObjectId::Solid(receiver) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 8.0),
                DVec3::new(32.0, 32.0, 24.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let mut ids = vec![receiver];
            let mut walls = Vec::new();
            if enclosure {
                for (mins, maxs) in [
                    ([-48.0, -48.0, 24.0], [-32.0, 48.0, 96.0]),
                    ([32.0, -48.0, 24.0], [48.0, 48.0, 96.0]),
                    ([-32.0, -48.0, 24.0], [32.0, -32.0, 96.0]),
                    ([-32.0, 32.0, 24.0], [32.0, 48.0, 96.0]),
                    ([-48.0, -48.0, 96.0], [48.0, 48.0, 112.0]),
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
                    walls.push(id);
                }
            }
            if !separate {
                ids.append(&mut walls);
            }
            edit::tie_to_entity(
                tx,
                &ids,
                "func_brush",
                &[("origin".into(), source_formats::vmf::format_vec3(pivot))],
            )?;
            if !walls.is_empty() {
                edit::tie_to_entity(
                    tx,
                    &walls,
                    "func_brush",
                    &[("origin".into(), "-70.25 83.5 56.125".into())],
                )?;
            }
            Ok(())
        })
        .unwrap();
    document
}

fn model_top(bsp: &Bsp, pivot: DVec3) -> DVec3 {
    let model = bsp.records::<bsp::Model>().unwrap()[1];
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let faces = bsp.records::<Face>().unwrap();
    let light = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || (f64::from(plane.dist) + pivot.z - 24.0).abs() > 1e-6 {
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
                if (frame.position(s, t).unwrap() + pivot).distance(DVec3::new(0.0, 0.0, 24.0))
                    < 1e-6
                {
                    return DVec3::from_array(
                        light
                            .face(face, &info)
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
    panic!("Missing model bounce receiver");
}

fn world_lightmaps(bsp: &Bsp) -> Vec<Vec<bsp::RgbExp32>> {
    let model = bsp.records::<bsp::Model>().unwrap()[0];
    let infos = bsp.records::<TexInfo>().unwrap();
    let faces = bsp.records::<Face>().unwrap();
    let light = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    faces[model.first_face as usize..(model.first_face + model.face_count) as usize]
        .iter()
        .filter_map(|face| {
            light
                .face(face, &infos[face.texinfo as usize])
                .unwrap()
                .map(|samples| samples.samples(0, 0).unwrap().to_vec())
        })
        .collect()
}

#[test]
fn model_receivers_gain_world_bounce_at_integer_and_fractional_pivots_without_geometry_changes() {
    let (_directory, game) = albedo_game([0.5, 0.0, 0.0]);
    let mut reference = None;
    for pivot in [
        DVec3::ZERO,
        DVec3::new(128.0, -64.0, 16.0),
        DVec3::new(101.5, -67.25, 32.125),
    ] {
        let document = model_receiver(pivot, false, false);
        let source = document.vmf().clone();
        let direct = compiler::compile_with_options(&source, &game, settings(0));
        let bounced = compiler::compile_with_options(&source, &game, settings(1));
        assert!(
            !direct.report.has_errors() && !bounced.report.has_errors(),
            "{:?} {:?}",
            direct.report,
            bounced.report
        );
        assert_eq!(bounced.stats.model_bounce_faces, 6);
        assert!(bounced.stats.model_bounce_samples > 0);
        assert_eq!(
            bounced.stats.bounce_rays,
            bounced.stats.lightmap_samples * SkyQuality::Balanced.directions()
        );
        let direct = direct.bsp.unwrap();
        let bounced = bounced.bsp.unwrap();
        assert_eq!(model_top(&direct, pivot), DVec3::ZERO);
        let value = model_top(&bounced, pivot);
        assert!(
            value.x > 1.0 && value.x <= 100.0 && value.y == 0.0 && value.z == 0.0,
            "{value}"
        );
        if let Some((color, world)) = &reference {
            assert_eq!(&value, color);
            assert_eq!(&world_lightmaps(&bounced), world);
        } else {
            reference = Some((value, world_lightmaps(&bounced)));
        }
        for (index, (before, after)) in direct.lumps.iter().zip(&bounced.lumps).enumerate() {
            if ![
                bsp::lump::LIGHTING,
                bsp::lump::LEAF_AMBIENT_LIGHTING,
                bsp::lump::LEAF_AMBIENT_INDEX,
            ]
            .contains(&index)
            {
                assert!(before == after, "Model bounce changed lump {index}");
            }
        }
        assert_eq!(*document.vmf(), source);
    }
}

#[test]
fn model_bounce_uses_only_its_own_occluders_and_zero_albedo_preserves_bytes() {
    let pivot = DVec3::new(101.5, -67.25, 32.125);
    let (_directory, game) = albedo_game([0.5, 0.0, 0.0]);
    let mut results = Vec::new();
    for (enclosure, separate) in [(false, false), (true, false), (true, true)] {
        let document = model_receiver(pivot, enclosure, separate);
        let output = compiler::compile_with_options(document.vmf(), &game, settings(1));
        assert!(!output.report.has_errors(), "{:?}", output.report);
        results.push(output.bsp.unwrap());
    }
    let open = model_top(&results[0], pivot);
    assert!(open.x > 0.0);
    assert_eq!(model_top(&results[1], pivot), DVec3::ZERO);
    assert_eq!(model_top(&results[2], pivot), open);
    assert_eq!(world_lightmaps(&results[0]), world_lightmaps(&results[1]));
    assert_eq!(world_lightmaps(&results[0]), world_lightmaps(&results[2]));
    let (_directory, game) = albedo_game([0.0; 3]);
    let document = model_receiver(pivot, false, false);
    let direct = compiler::compile_with_options(document.vmf(), &game, settings(0));
    let bounced = compiler::compile_with_options(document.vmf(), &game, settings(1));
    assert!(!direct.report.has_errors() && !bounced.report.has_errors());
    assert_eq!(bounced.stats.model_bounce_samples, 0);
    assert!(
        direct.bsp.unwrap().to_bytes() == bounced.bsp.unwrap().to_bytes(),
        "Zero reflected model energy changed bytes"
    );
}
