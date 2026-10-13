mod common;

use compiler::lighting::LuxelFrame;
use compiler::{LightingMode, Options};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, LightingKind, TexInfo};

fn options() -> Options {
    Options {
        lighting: LightingMode::DirectLdr,
        ..Options::default()
    }
}

fn room(origin: DVec3, properties: &[(&str, &str)]) -> Document {
    let mut document = Document::new(common::room());
    let properties = properties
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    document
        .edit("point light", |tx| {
            edit::create_entity(tx, "light", Some(origin), &properties, Vec::new())
        })
        .unwrap();
    document
}

fn floor_sample(bsp: &Bsp, point: DVec3) -> DVec3 {
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in &faces {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = LuxelFrame::new(
            Plane {
                normal: DVec3::from_array(plane.normal.map(f64::from)),
                dist: f64::from(plane.dist),
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let samples = lighting.face(face, info).unwrap().unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(point) < 0.001 {
                    return DVec3::from_array(
                        samples.samples(0, 0).unwrap()[t * width + s].linear(),
                    );
                }
            }
        }
    }
    panic!("No baked floor sample at {point}")
}

#[test]
fn direct_ldr_emits_checked_colored_samples_averages_and_world_lights() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[("_light", "255 0 0 200"), ("_quadratic_attn", "1")],
    );
    let before = document.vmf().write(&Default::default());
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let stats = &output.stats;
    assert_eq!(stats.lighting_mode, LightingMode::DirectLdr);
    assert_eq!(stats.world_lights, 1);
    assert!(stats.lighting_faces > 0 && stats.lightmap_samples > 0);
    assert_eq!(
        stats.lighting_bytes,
        (stats.lightmap_samples + stats.lighting_faces) * 4
    );
    let bsp = output.bsp.unwrap();
    assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::new(200.0, 0.0, 0.0));
    assert_eq!(bsp.world_lights(LightingKind::Ldr).unwrap().len(), 1);
    assert!(bsp.lighting(LightingKind::Hdr).unwrap().is_none());
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    for face in bsp.records::<Face>().unwrap() {
        if let Some(samples) = lighting.face(&face, &infos[face.texinfo as usize]).unwrap() {
            assert_eq!(samples.styles(), [0]);
            assert!(samples.average(0).is_some());
            assert!(
                samples
                    .samples(0, 0)
                    .unwrap()
                    .iter()
                    .all(|value| value.rgb[1] == 0 && value.rgb[2] == 0)
            );
        }
    }
    assert_eq!(document.vmf().write(&Default::default()), before);
    let again = compiler::compile_with_options(document.vmf(), &common::empty_game(), options())
        .bsp
        .unwrap();
    assert_eq!(bsp.to_bytes(), again.to_bytes());
}

#[test]
fn percentage_light_bakes_half_intensity_and_hard_cutoff_without_source_edits() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", "100"),
            ("_zero_percent_distance", "200"),
            ("_hardfalloff", "1"),
        ],
    );
    let before = document.vmf().clone();
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::new(100.0, 0.0, 0.0));
    assert_eq!(floor_sample(&bsp, DVec3::new(176.0, 0.0, 0.0)), DVec3::ZERO);
    assert_eq!(*document.vmf(), before);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    let invalid = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", "100"),
            ("_zero_percent_distance", "50"),
        ],
    );
    let output = compiler::compile_with_options(invalid.vmf(), &common::empty_game(), options());
    assert!(output.report.has_errors());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
}

fn sky_fixture(
    material: bool,
    shade: bool,
) -> (tempfile::TempDir, source_formats::gamefs::GameFs, Document) {
    use source_formats::gamefs::{Mount, MountedPath};
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/sky.vmt"),
        "UnlitGeneric { %compilesky 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut vmf = common::room();
    if material {
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
    }
    let mut document = Document::new(vmf);
    document
        .edit("sun", |tx| {
            edit::create_entity(
                tx,
                "light_environment",
                Some(DVec3::new(0.0, 0.0, 128.0)),
                &[
                    ("_light".into(), "255 0 0 200".into()),
                    ("_ambient".into(), "0".into()),
                    ("pitch".into(), "-90".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    if shade {
        document
            .edit("shade", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 32.0),
                    DVec3::new(16.0, 16.0, 48.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap();
    }
    (directory, game, document)
}

#[test]
fn native_direct_sun_requires_sky_and_preserves_opaque_shadows() {
    for (sky, shade, expected) in [(true, false, 200.0), (true, true, 0.0), (false, false, 0.0)] {
        let (_directory, game, document) = sky_fixture(sky, shade);
        let source = document.vmf().clone();
        let output = compiler::compile_with_options(document.vmf(), &game, options());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.world_lights, 1);
        let bsp = output.bsp.unwrap();
        assert_eq!(
            floor_sample(&bsp, DVec3::ZERO),
            DVec3::new(expected, 0.0, 0.0)
        );
        assert_eq!(
            floor_sample(&bsp, DVec3::new(128.0, 0.0, 0.0)),
            DVec3::new(if sky { 200.0 } else { 0.0 }, 0.0, 0.0)
        );
        let light = bsp.world_lights(LightingKind::Ldr).unwrap()[0];
        assert_eq!(light.kind, bsp::world_light_kind::SKY);
        assert!((DVec3::from_array(light.normal.map(f64::from)) + DVec3::Z).length() < 1e-6);
        assert!((light.intensity[0] as f64 - 200.0 / 255.0).abs() < 1e-7);
        assert_eq!(light.cluster, -1);
        assert_eq!(*document.vmf(), source);
    }
}

#[test]
fn rejected_sky_parameters_and_duplicate_emitters_preserve_previous_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    for (field, value) in [
        ("_ambient", "NaN"),
        ("SunSpreadAngle", "91"),
        ("SunSpreadAngle", "-0.1"),
        ("SunSpreadAngle", "NaN"),
        ("SunSpreadAngle", "inf"),
        ("duplicate", "1"),
    ] {
        let (_directory, game, mut document) = sky_fixture(true, false);
        document
            .edit("invalid sun", |tx| {
                if field == "duplicate" {
                    edit::create_entity(
                        tx,
                        "light_environment",
                        Some(DVec3::new(0.0, 0.0, 100.0)),
                        &[("_light".into(), "255".into())],
                        Vec::new(),
                    )?;
                } else {
                    let id = tx
                        .vmf()
                        .entities
                        .iter()
                        .find(|entity| entity.classname() == Some("light_environment"))
                        .unwrap()
                        .id;
                    tx.entity_mut(id)?.properties.set(field, value);
                }
                Ok(())
            })
            .unwrap();
        let output = compiler::compile_with_options(document.vmf(), &game, options());
        assert!(output.report.has_errors(), "{field}: {:?}", output.report);
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    }
}

#[test]
fn soft_sun_bakes_a_penumbra_without_changing_native_light_direction() {
    let (_directory, game, mut document) = sky_fixture(true, false);
    document
        .edit("high shadow caster", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -128.0, 96.0),
                DVec3::new(32.0, 128.0, 112.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let hard = compiler::compile_with_options(document.vmf(), &game, options());
    assert!(!hard.report.has_errors(), "{:?}", hard.report);
    let hard = hard.bsp.unwrap();
    assert_eq!(floor_sample(&hard, DVec3::ZERO), DVec3::ZERO);
    let id = document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("light_environment"))
        .unwrap()
        .id;
    document
        .edit("soft sun", |tx| {
            tx.entity_mut(id)?.properties.set("SunSpreadAngle", "20");
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    let mut previous_work = 0;
    for quality in [compiler::SkyQuality::Fast, compiler::SkyQuality::Detailed] {
        let soft = compiler::compile_with_options(
            document.vmf(),
            &game,
            Options {
                sky_quality: quality,
                ..options()
            },
        );
        assert!(!soft.report.has_errors(), "{:?}", soft.report);
        assert!(soft.stats.lighting_work > previous_work);
        previous_work = soft.stats.lighting_work;
        let soft = soft.bsp.unwrap();
        let center = floor_sample(&soft, DVec3::ZERO);
        assert!(center.x > 0.0 && center.x < 200.0, "{quality:?}: {center}");
        assert_eq!(
            floor_sample(&soft, DVec3::new(128.0, 0.0, 0.0)),
            DVec3::new(200.0, 0.0, 0.0)
        );
        assert_eq!(
            soft.world_lights(LightingKind::Ldr).unwrap(),
            hard.world_lights(LightingKind::Ldr).unwrap()
        );
        assert_eq!(*document.vmf(), source);
    }
    document
        .edit("hard sun", |tx| {
            tx.entity_mut(id)?.properties.set("SunSpreadAngle", "0");
            Ok(())
        })
        .unwrap();
    let zero = compiler::compile_with_options(document.vmf(), &game, options())
        .bsp
        .unwrap();
    for (index, lump) in hard.lumps.iter().enumerate() {
        if index != bsp::lump::ENTITIES {
            assert_eq!(zero.lumps[index].data, lump.data, "lump {index}");
        }
    }
}

#[test]
fn sampled_sky_ambient_matches_open_closed_and_shaded_hemispheres() {
    for (sky, shade, expected) in [
        (true, false, [200.0, 0.0, 60.0]),
        (true, true, [0.0, 0.0, 37.5]),
        (false, false, [0.0; 3]),
    ] {
        let (_directory, game, mut document) = sky_fixture(sky, shade);
        document
            .edit("blue sky ambient", |tx| {
                let id = tx
                    .vmf()
                    .entities
                    .iter()
                    .find(|entity| entity.classname() == Some("light_environment"))
                    .unwrap()
                    .id;
                tx.entity_mut(id)?.properties.set("_ambient", "0 0 255 100");
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        let output = compiler::compile_with_options(document.vmf(), &game, options());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.world_lights, 2);
        let bsp = output.bsp.unwrap();
        assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::from_array(expected));
        let lights = bsp.world_lights(LightingKind::Ldr).unwrap();
        assert_eq!(lights[1].kind, bsp::world_light_kind::SKY_AMBIENT);
        assert_eq!(lights[1].normal, [0.0; 3]);
        assert!((lights[1].intensity[2] as f64 - 100.0 / 255.0).abs() < 1e-7);
        assert_eq!(*document.vmf(), source);
    }
}

#[test]
fn native_spotlights_bake_inside_the_cone_and_keep_outside_samples_dark() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_constant_attn", "1"),
            ("pitch", "-90"),
            ("_inner_cone", "20"),
            ("_cone", "45"),
            ("_exponent", "2"),
        ],
    );
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap()
        .properties
        .set("classname", "light_spot");
    let source = vmf.clone();
    let output = compiler::compile_with_options(&vmf, &common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.world_lights, 1);
    let bsp = output.bsp.unwrap();
    assert_eq!(
        bsp.world_lights(LightingKind::Ldr).unwrap()[0].kind,
        bsp::world_light_kind::SPOT
    );
    assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::new(200.0, 0.0, 0.0));
    assert_eq!(floor_sample(&bsp, DVec3::new(128.0, 0.0, 0.0)), DVec3::ZERO);
    assert!(floor_sample(&bsp, DVec3::new(32.0, 0.0, 0.0)).x > 175.0);
    assert_eq!(vmf, source);
}

#[test]
fn opaque_world_hulls_cast_shadows_and_near_far_light_matches_reference() {
    let mut document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[("_light", "255 255 255 200"), ("_quadratic_attn", "1")],
    );
    let clear = compiler::compile_with_options(document.vmf(), &common::empty_game(), options())
        .bsp
        .unwrap();
    document
        .edit("shadow caster", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 32.0),
                DVec3::new(16.0, 16.0, 48.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let shadow = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(!shadow.report.has_errors(), "{:?}", shadow.report);
    assert_eq!(floor_sample(&shadow.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    assert_eq!(floor_sample(&clear, DVec3::ZERO), DVec3::splat(200.0));
    let distant = room(
        DVec3::new(0.0, 0.0, 200.0),
        &[("_light", "255 255 255 200"), ("_quadratic_attn", "1")],
    );
    let distant = compiler::compile_with_options(distant.vmf(), &common::empty_game(), options())
        .bsp
        .unwrap();
    assert_eq!(floor_sample(&distant, DVec3::ZERO), DVec3::splat(50.0));
}

#[test]
fn rejected_lights_and_cancelled_bakes_preserve_existing_output() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[("_light", "255 255 255 NaN")],
    );
    let previous = compiler::compile(document.vmf(), &common::empty_game())
        .bsp
        .unwrap()
        .to_bytes();
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == compiler::Level::Error
            && diagnostic
                .object
                .as_deref()
                .is_some_and(|object| object.starts_with("entity:"))
    }));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, &previous).unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let output = compiler::compile_with_options_controlled(
        document.vmf(),
        &common::empty_game(),
        options(),
        &control,
    );
    assert!(output.bsp.is_none());
    assert!(output.write_controlled(&path, &control).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
}

#[test]
fn fullbright_remains_default_and_zero_lights_still_produce_dark_lightmaps() {
    let document = common::room();
    let default = compiler::compile(&document, &common::empty_game());
    assert_eq!(default.stats.lighting_mode, LightingMode::Fullbright);
    assert!(
        default
            .bsp
            .as_ref()
            .unwrap()
            .lump(bsp::lump::LIGHTING)
            .data
            .is_empty()
    );
    let explicit =
        compiler::compile_with_options(&document, &common::empty_game(), Options::default());
    assert_eq!(
        default.bsp.unwrap().to_bytes(),
        explicit.bsp.unwrap().to_bytes()
    );
    let dark = compiler::compile_with_options(&document, &common::empty_game(), options());
    assert!(!dark.report.has_errors(), "{:?}", dark.report);
    assert_eq!(dark.stats.world_lights, 0);
    assert!(dark.stats.lighting_faces > 0);
    assert_eq!(floor_sample(&dark.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
}
