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
