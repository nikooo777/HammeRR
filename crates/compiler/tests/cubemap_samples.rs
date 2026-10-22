mod common;

use compiler::map::MAX_COMPILE_COORD;
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, Cubemap};
use source_formats::vmf::{Vmf, format_vec3};

fn add_sample(vmf: Vmf, origin: Option<DVec3>, size: Option<&str>) -> (Vmf, u32) {
    let mut document = Document::new(vmf);
    let id = document
        .edit("cubemap", |tx| {
            let properties = size
                .map(|value| vec![("cubemapsize".into(), value.into())])
                .unwrap_or_default();
            let ObjectId::Entity(id) =
                edit::create_entity(tx, "env_cubemap", origin, &properties, Vec::new())?
            else {
                unreachable!()
            };
            Ok(id)
        })
        .unwrap();
    (document.vmf().clone(), id)
}

#[test]
fn native_samples_use_source_integer_positions_byte_sizes_and_no_runtime_entities() {
    let mut vmf = common::room();
    let cases = [
        (
            DVec3::new(-12.875, 34.5, 64.125),
            Some("9"),
            9,
            [-12, 34, 64],
        ),
        (DVec3::new(-0.75, 0.75, 72.875), Some("1"), 1, [0, 0, 72]),
        (
            DVec3::new(48.5, -48.5, 128.25),
            Some("5"),
            5,
            [48, -48, 128],
        ),
        (DVec3::new(96.0, 96.0, 128.0), None, 0, [96, 96, 128]),
        (
            DVec3::new(128.0, 96.0, 128.0),
            Some(" 0 "),
            0,
            [128, 96, 128],
        ),
        (
            DVec3::new(-95.99999999999999, 95.99999999999999, 64.0),
            Some("6"),
            6,
            [-96, 96, 64],
        ),
        (
            DVec3::new(-0.99999999, 0.99999999, 72.0),
            Some("6"),
            6,
            [-1, 1, 72],
        ),
        (
            DVec3::new(-16383.9999, 16383.9999, 128.0),
            Some("6"),
            6,
            [-16384, 16384, 128],
        ),
    ];
    for (origin, size, _, _) in cases {
        vmf = add_sample(vmf, Some(origin), size).0;
    }
    let source = vmf.clone();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert_eq!(output.stats.cubemap_samples, cases.len());
    let bsp = output.bsp.unwrap();
    let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
    let samples = parsed.records::<Cubemap>().unwrap();
    assert_eq!(samples.len(), cases.len());
    for (sample, (_, _, size, expected)) in samples.iter().zip(cases) {
        assert_eq!(sample.origin, expected);
        assert_eq!(sample.size, size);
        assert_eq!(sample.padding, [0; 3]);
    }
    assert!(!parsed.entities().unwrap().contains("env_cubemap"));
    let baseline = compiler::compile(&common::room(), &common::empty_game())
        .bsp
        .unwrap();
    for index in 0..bsp::LUMP_COUNT {
        if index != bsp::lump::CUBEMAPS {
            assert_eq!(parsed.lumps[index], baseline.lumps[index], "lump {index}");
        }
    }
    assert_eq!(vmf, source);
}

#[test]
fn missing_origin_defaults_to_zero_and_hidden_samples_are_not_validated_or_emitted() {
    let (mut vmf, id) = add_sample(common::room(), None, Some(""));
    let (next, hidden_id) = add_sample(vmf, Some(DVec3::ZERO), Some("invalid"));
    vmf = next;
    let hidden = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == hidden_id)
        .unwrap();
    hidden.hidden = true;
    hidden.properties.set("origin", "NaN 0 0");
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.cubemap_samples, 1);
    assert_eq!(
        output.bsp.unwrap().records::<Cubemap>().unwrap(),
        vec![Cubemap::default()]
    );
    assert!(vmf.entities.iter().any(|entity| entity.id == id));
}

#[test]
fn invalid_sizes_origins_and_helper_geometry_fail_with_object_diagnostics() {
    for value in ["-1", "10", "256", "1.5", "NaN", "default"] {
        let (vmf, id) = add_sample(common::room(), Some(DVec3::ZERO), Some(value));
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(output.bsp.is_none(), "size={value}");
        assert!(output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.level == compiler::Level::Error
                && diagnostic.object.as_deref() == Some(&format!("entity:{id}"))
                && diagnostic.message.contains("Cubemap Size")
        }));
    }
    for value in [
        "NaN 0 0".to_owned(),
        "0 infinity 0".to_owned(),
        "16384.00001 0 0".to_owned(),
        "-16384.00001 0 0".to_owned(),
        format_vec3(DVec3::X * (MAX_COMPILE_COORD + 0.5)),
        "1 2".to_owned(),
    ] {
        let (mut vmf, id) = add_sample(common::room(), None, None);
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap()
            .properties
            .set("origin", value);
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(output.report.has_errors());
        assert!(output.bsp.is_none());
    }
    let (mut vmf, id) = add_sample(common::room(), Some(DVec3::ZERO), None);
    let solid = vmf.world.solids.pop().unwrap();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .solids
        .push(solid);
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.bsp.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == compiler::Level::Error && diagnostic.message.contains("point helper")
    }));
}

#[test]
fn sample_limit_is_checked_before_emission_and_duplicate_positions_are_retained() {
    let (mut vmf, id) = add_sample(common::room(), Some(DVec3::ZERO), Some("9"));
    let template = vmf
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap()
        .clone();
    for index in 1..bsp::MAX_CUBEMAP_SAMPLES {
        let mut sample = template.clone();
        sample.id = id + index as u32;
        vmf.entities.push(sample);
    }
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.cubemap_samples, bsp::MAX_CUBEMAP_SAMPLES);
    assert_eq!(
        output.bsp.unwrap().records::<Cubemap>().unwrap().len(),
        bsp::MAX_CUBEMAP_SAMPLES
    );
    let mut excess = template;
    excess.id = id + bsp::MAX_CUBEMAP_SAMPLES as u32;
    vmf.entities.push(excess);
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.bsp.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == compiler::Level::Error
            && diagnostic.message.contains("sample limit is 1024")
    }));
}

#[test]
fn manual_face_bindings_are_diagnosed_and_samples_do_not_seed_flood_fill() {
    let (mut vmf, id) = add_sample(common::room(), Some(DVec3::ZERO), Some("7"));
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("sides", "1 2");
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.object.as_deref() == Some(&format!("entity:{id}"))
            && diagnostic.message.contains("sides list is unused")
    }));
    let outside = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap();
    outside.properties.set("origin", "1024 1024 1024");
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors());
    assert!(output.leak.is_none());
    assert!(output.bsp.is_some());
    vmf.entities.retain(|entity| entity.id == id);
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.report.has_errors());
    assert!(output.bsp.is_none());
}
