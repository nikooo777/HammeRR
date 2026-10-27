mod common;

use std::path::Path;

use compiler::cubemap_assets::finalize_reflections_with_capture;
use compiler::instances::{self, SourceContext};
use compiler::{CompileContext, Control, CubemapAssetPhase, LightingMode, Options};
use glam::DVec3;
use map_core::{Document, edit};
use native_runtime::CaptureEnvironment;
use source_formats::bsp::{self, Cubemap};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Vmf, format_vec3};

fn next_id(source: &Vmf) -> u32 {
    source.object_ids().chain(source.side_ids()).max().unwrap() + 1
}

fn sample(source: &mut Vmf, origin: &str, size: Option<&str>) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "env_cubemap");
    entity.properties.set("origin", origin);
    if let Some(size) = size {
        entity.properties.set("cubemapsize", size);
    }
    source.entities.push(entity);
    id
}

fn child_sample(origin: &str, size: Option<&str>) -> Vmf {
    let mut source = Document::empty().into_vmf();
    sample(&mut source, origin, size);
    source
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "func_instance");
    entity.properties.set("file", file);
    entity.properties.set("origin", origin);
    entity.properties.set("angles", angles);
    source.entities.push(entity);
    id
}

fn write_source(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn saved_parent(directory: &Path, source: &Vmf) -> SourceContext {
    write_source(directory, "parent.vmf", source);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn game(directory: &Path) -> GameFs {
    for folder in ["dev", "tools", "skybox"] {
        std::fs::create_dir_all(directory.join("materials").join(folder)).unwrap();
    }
    std::fs::write(
        directory.join("materials/dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $envmap env_cubemap }",
    )
    .unwrap();
    std::fs::write(
        directory.join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    for face in ["rt", "lf", "bk", "ft", "up", "dn"] {
        std::fs::write(
            directory.join(format!("materials/skybox/sky_day01_01{face}.vmt")),
            "UnlitGeneric { }",
        )
        .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn room() -> Vmf {
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
    }
    source
}

fn region(source: &mut Vmf) {
    source.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-254 -254 4"),
            Node::string("maxs", "254 254 252"),
        ],
    ));
}

fn capture_options(lighting: LightingMode) -> Options {
    Options {
        lighting,
        generate_reflections: true,
        ..Default::default()
    }
}

fn context() -> CompileContext {
    CompileContext::new("instanced_cubemaps").unwrap()
}

fn compile_saved(
    source: &Vmf,
    game: &GameFs,
    options: Options,
    source_context: &SourceContext,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        source,
        game,
        options,
        Some(&context()),
        Some(source_context),
        &Control::default(),
    )
}

fn pending(output: &compiler::Output) {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some() && output.leak.is_none());
    assert_eq!(
        output.cubemap_assets.as_ref().unwrap().phase,
        CubemapAssetPhase::Pending
    );
}

fn child_error(output: &compiler::Output, owner: u32, file: &str, original: u32, reason: &str) {
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(output.cubemap_assets.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains(file)
                && diagnostic.message.contains(reason)
                && (diagnostic
                    .message
                    .contains(&format!("Child cubemap {original}"))
                    || diagnostic
                        .message
                        .contains(&format!("child object {original}")))
        }),
        "{:?}",
        output.report
    );
}

#[test]
fn nested_right_angles_preserve_source_float_integer_sample_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = child_sample("72 -80 -120", Some("6"));
    sample(&mut child, "72 -80 72", Some("6"));
    let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "8 16 64", "90 0 0");
    let middle_bytes = write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room();
    instance(&mut parent, "middle.vmf", "16 -8 128", "0 0 90");
    let source_context = saved_parent(directory.path(), &parent);
    let expected = [[-96, 0, 64], [96, 0, 64]];
    let mut flat = room();
    sample(&mut flat, "-96 0 64", Some("6"));
    sample(&mut flat, "96 0 64", Some("6"));
    let output = compile_saved(&parent, &game, Options::default(), &source_context);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    assert_eq!(
        bsp.records::<Cubemap>()
            .unwrap()
            .iter()
            .map(|cube| cube.origin)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        bsp.to_bytes(),
        compiler::compile(&flat, &game).bsp.unwrap().to_bytes()
    );
    for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
        let output = compile_saved(&parent, &game, capture_options(lighting), &source_context);
        pending(&output);
        let plan = output.cubemap_assets.unwrap();
        assert_eq!(
            plan.samples
                .iter()
                .map(|sample| sample.origin)
                .collect::<Vec<_>>(),
            expected
        );
        if lighting == LightingMode::DirectLdrHdr {
            assert_eq!(
                plan.hdr_samples
                    .iter()
                    .map(|sample| sample.origin)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        child_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("middle.vmf")).unwrap(),
        middle_bytes
    );
}

#[test]
fn translated_and_noncommuting_samples_match_literal_positions_and_capture_assignments() {
    for nested in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = child_sample("-92.875 18.5 48.125", None);
        sample(&mut child, "0.25 -0.75 72.875", Some(" 1 "));
        child.entities[1].properties.set("angle", "0");
        child.entities[1].properties.set("sides", " \t ");
        let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room();
        let owner = if nested {
            let mut middle = Document::empty().into_vmf();
            instance(&mut middle, "leaf.vmf", "16 32 0", "0 90 0");
            write_source(directory.path(), "middle.vmf", &middle);
            instance(&mut parent, "middle.vmf", "80 16 128", "90 0 0")
        } else {
            instance(&mut parent, "leaf.vmf", "80 16 16", "0 0 0")
        };
        let expected = if nested {
            [[128.125, -44.875, 130.5], [152.875, 48.25, 111.25]]
        } else {
            [[-12.875, 34.5, 64.125], [80.25, 15.25, 88.875]]
        };
        let native_positions = if nested {
            [[128, -44, 130], [152, 48, 111]]
        } else {
            [[-12, 34, 64], [80, 15, 88]]
        };
        let before = parent.clone();
        let source_context = saved_parent(directory.path(), &parent);
        let mut report = compiler::Report::default();
        let prepared = instances::resolve(
            &parent,
            Some(&source_context),
            &Control::default(),
            &mut report,
        )
        .unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (stats.cubemaps, stats.world_brushes, stats.detail_brushes),
            (2, 0, 0)
        );
        assert_eq!(stats.max_expansion_depth, if nested { 2 } else { 1 });
        assert_eq!(prepared.generated_brushes().count(), 0);
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| entity.classname() == Some("env_cubemap"))
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 2);
        for (entity, position) in generated.iter().zip(expected) {
            assert!(
                entity
                    .origin()
                    .unwrap()
                    .abs_diff_eq(DVec3::from_array(position), 1e-8)
            );
            assert_eq!(prepared.root_entity(entity.id), Some(owner));
            assert_eq!(entity.properties.get("angles"), None);
        }
        assert_eq!(generated[0].properties.get("angle"), None);
        assert_eq!(generated[1].properties.get("angle"), Some("0"));
        let mut flat = room();
        for (index, position) in expected.into_iter().enumerate() {
            sample(
                &mut flat,
                &format_vec3(DVec3::from_array(position)),
                if index == 0 { None } else { Some("1") },
            );
        }
        let ordinary = compile_saved(&parent, &game, Options::default(), &source_context);
        assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
        let ordinary_flat = compiler::compile(&flat, &game);
        assert!(
            !ordinary_flat.report.has_errors(),
            "{:?}",
            ordinary_flat.report
        );
        let bsp = ordinary.bsp.unwrap();
        assert_eq!(bsp.to_bytes(), ordinary_flat.bsp.unwrap().to_bytes());
        assert_eq!(
            bsp.records::<Cubemap>().unwrap(),
            native_positions
                .into_iter()
                .enumerate()
                .map(|(index, origin)| Cubemap {
                    origin,
                    size: index as u8,
                    padding: [0; 3]
                })
                .collect::<Vec<_>>()
        );
        let baseline = compiler::compile(&room(), &game).bsp.unwrap();
        for (index, lump) in baseline.lumps.iter().enumerate() {
            if index != bsp::lump::CUBEMAPS {
                assert_eq!(lump, &bsp.lumps[index], "physical lump {index}");
            }
        }
        assert!(!bsp.entities().unwrap().contains("env_cubemap"));
        for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
            let output = compile_saved(&parent, &game, capture_options(lighting), &source_context);
            pending(&output);
            let expected_output = compiler::compile_with_context_controlled(
                &flat,
                &game,
                capture_options(lighting),
                Some(&context()),
                &Control::default(),
            );
            pending(&expected_output);
            let actual_bsp = output.bsp.as_ref().unwrap();
            let expected_bsp = expected_output.bsp.as_ref().unwrap();
            for (index, lump) in expected_bsp.lumps.iter().enumerate() {
                if index != bsp::lump::PAKFILE {
                    assert_eq!(
                        lump, &actual_bsp.lumps[index],
                        "prepared capture lump {index}"
                    );
                }
            }
            let actual = output.cubemap_assets.as_ref().unwrap();
            let literal = expected_output.cubemap_assets.as_ref().unwrap();
            assert_eq!(actual.samples.len(), 2);
            assert_eq!(
                actual.hdr_samples.len(),
                if lighting == LightingMode::DirectLdrHdr {
                    2
                } else {
                    0
                }
            );
            assert_eq!(actual.aliases, literal.aliases);
            assert_eq!(actual.texinfo_bindings, literal.texinfo_bindings);
            assert_eq!(actual.face_bindings, literal.face_bindings);
            for (actual_samples, literal_samples) in [
                (&actual.samples, &literal.samples),
                (&actual.hdr_samples, &literal.hdr_samples),
            ] {
                assert_eq!(actual_samples.len(), literal_samples.len());
                for (actual, expected) in actual_samples.iter().zip(literal_samples) {
                    let mut normalized = actual.clone();
                    normalized.helper_id = expected.helper_id;
                    assert_eq!(&normalized, expected);
                }
            }
            assert!(!actual.assignments.is_empty());
            assert_eq!(actual.assignments.len(), literal.assignments.len());
            for (actual_assignment, literal_assignment) in
                actual.assignments.iter().zip(&literal.assignments)
            {
                let actual_sample = actual
                    .samples
                    .iter()
                    .find(|sample| sample.helper_id == actual_assignment.helper_id)
                    .unwrap();
                let literal_sample = literal
                    .samples
                    .iter()
                    .find(|sample| sample.helper_id == literal_assignment.helper_id)
                    .unwrap();
                assert_eq!(actual_sample.origin, literal_sample.origin);
                let mut normalized = actual_assignment.clone();
                normalized.helper_id = literal_assignment.helper_id;
                assert_eq!(&normalized, literal_assignment);
                assert!(!actual_assignment.manual);
            }
        }
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
            child_bytes
        );
    }
}

#[test]
fn quantized_duplicate_samples_remain_ordinary_records_but_capture_reports_both_owners() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = child_sample("1.25 2.25 64.25", Some("1"));
    let original = child.entities[0].id;
    write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0.6 0 0", "0 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room();
    let first = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
    let second = instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0");
    let source_context = saved_parent(directory.path(), &parent);
    let ordinary = compile_saved(&parent, &game, Options::default(), &source_context);
    assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
    assert_eq!(
        ordinary.bsp.unwrap().records::<Cubemap>().unwrap(),
        vec![
            Cubemap {
                origin: [1, 2, 64],
                size: 1,
                padding: [0; 3]
            };
            2
        ]
    );
    let rejected = compile_saved(
        &parent,
        &game,
        capture_options(LightingMode::DirectLdr),
        &source_context,
    );
    for owner in [first, second] {
        child_error(
            &rejected,
            owner,
            "leaf.vmf",
            original,
            "integer origin quantization",
        );
    }
}

#[test]
fn capture_limit_counts_root_repeated_and_nested_samples_after_region_omission() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut leaf = child_sample("0 0 64", Some("1"));
    sample(&mut leaf, "2 0 64", Some("1"));
    let mut hidden = Entity::new(100, "env_cubemap");
    hidden.hidden = true;
    hidden.properties.set("origin", "NaN 0 0");
    hidden.properties.set("cubemapsize", "invalid");
    leaf.entities.push(hidden);
    write_source(directory.path(), "leaf.vmf", &leaf);
    let single = child_sample("0 0 64", Some("1"));
    let original = single.entities[0].id;
    write_source(directory.path(), "single.vmf", &single);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut exterior = Document::empty().into_vmf();
    for index in 0..33 {
        sample(&mut exterior, &format!("{} 0 64", 1000 + index), Some("1"));
    }
    write_source(directory.path(), "exterior.vmf", &exterior);
    let mut parent = room();
    region(&mut parent);
    sample(&mut parent, "-240 0 64", Some("1"));
    for index in 0..15 {
        instance(
            &mut parent,
            "middle.vmf",
            &format!("{} 0 0", -140 + index * 20),
            "0 0 0",
        );
    }
    instance(&mut parent, "single.vmf", "200 0 0", "0 0 0");
    instance(&mut parent, "exterior.vmf", "0 0 0", "0 0 0");
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &game,
        capture_options(LightingMode::DirectLdr),
        &source_context,
    );
    pending(&output);
    assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 64);
    assert_eq!(output.stats.cubemap_samples, 32);
    assert_eq!(output.cubemap_assets.as_ref().unwrap().samples.len(), 32);
    assert_eq!(output.bsp.unwrap().records::<Cubemap>().unwrap().len(), 32);
    let owner = instance(&mut parent, "single.vmf", "220 0 0", "0 0 0");
    let before = parent.clone();
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &game,
        capture_options(LightingMode::DirectLdr),
        &source_context,
    );
    child_error(&output, owner, "single.vmf", original, "1–32");
    assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 65);
    assert_eq!(parent, before);
}

#[test]
fn ordinary_global_sample_capacity_is_independent_of_the_capture_limit() {
    let directory = tempfile::tempdir().unwrap();
    let mut leaf = Document::empty().into_vmf();
    for index in 0..31 {
        sample(&mut leaf, &format!("{index} 0 64"), Some("1"));
    }
    write_source(directory.path(), "leaf.vmf", &leaf);
    let single = child_sample("96 0 64", Some("1"));
    let original = single.entities[0].id;
    write_source(directory.path(), "single.vmf", &single);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room();
    sample(&mut parent, "-96 0 64", Some("1"));
    for _ in 0..33 {
        instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0");
    }
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &common::empty_game(),
        Options::default(),
        &source_context,
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 1023);
    assert_eq!(output.stats.cubemap_samples, bsp::MAX_CUBEMAP_SAMPLES);
    assert_eq!(
        output.bsp.unwrap().records::<Cubemap>().unwrap().len(),
        bsp::MAX_CUBEMAP_SAMPLES
    );
    let owner = instance(&mut parent, "single.vmf", "0 0 0", "0 0 0");
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &common::empty_game(),
        Options::default(),
        &source_context,
    );
    child_error(
        &output,
        owner,
        "single.vmf",
        original,
        "sample limit is 1024",
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 1024);
}

#[test]
fn ordinary_large_child_samples_share_the_generated_capture_size() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = child_sample("-12.875 34.5 64.125", Some("9"));
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room();
    instance(&mut parent, "leaf.vmf", "80 16 16", "0 0 0");
    let source_context = saved_parent(directory.path(), &parent);
    let ordinary = compile_saved(&parent, &game, Options::default(), &source_context);
    assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
    assert_eq!(
        ordinary.bsp.unwrap().records::<Cubemap>().unwrap(),
        [Cubemap {
            origin: [67, 50, 80],
            size: 9,
            padding: [0; 3]
        }]
    );
    let output = compile_saved(
        &parent,
        &game,
        capture_options(LightingMode::DirectLdrHdr),
        &source_context,
    );
    pending(&output);
    let generated = output.cubemap_assets.as_ref().unwrap();
    assert_eq!(generated.samples[0].edge, 256);
    assert_eq!(generated.hdr_samples[0].edge, 256);
    assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 1);
}

#[test]
fn hidden_children_are_omitted_but_invalid_exterior_sources_and_intermediate_poses_fail() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    for (key, value, reason) in [
        ("origin", "NaN 0 0", "origin"),
        ("origin", "16384.5 0 64", "bounds"),
        ("angles", "0 90 0", "angles"),
        ("sides", "1", "sides"),
        ("cubemapsize", "10", "Cubemap Size"),
    ] {
        for hidden in [false, true] {
            let mut child = child_sample("1024 1024 64", Some("1"));
            child.entities[0].hidden = hidden;
            child.entities[0].properties.set(key, value);
            let original = child.entities[0].id;
            write_source(directory.path(), "leaf.vmf", &child);
            let mut parent = room();
            region(&mut parent);
            let owner = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
            let source_context = saved_parent(directory.path(), &parent);
            let output = compile_saved(&parent, &game, Options::default(), &source_context);
            if hidden {
                assert!(!output.report.has_errors(), "{:?}", output.report);
                assert_eq!(output.stats.cubemap_samples, 0);
                assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 0);
                let stats = serde_json::to_value(output.stats.instances.as_ref().unwrap()).unwrap();
                assert!(stats.get("cubemaps").is_none());
                assert!(output.bsp.unwrap().records::<Cubemap>().unwrap().is_empty());
            } else {
                child_error(&output, owner, "leaf.vmf", original, reason);
                assert!(output.stats.compile_region.is_none());
            }
        }
    }
    let child = child_sample("600 0 64", Some("1"));
    let original = child.entities[0].id;
    write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16360 0 0", "0 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room();
    region(&mut parent);
    let owner = instance(&mut parent, "middle.vmf", "-16360 0 0", "0 0 0");
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, Options::default(), &source_context);
    child_error(&output, owner, "leaf.vmf", original, "bounds");
    assert!(output.stats.compile_region.is_none());
}

#[test]
fn root_manual_bindings_keep_original_side_custody_with_child_automatic_samples() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut document = Document::empty();
    document
        .edit("child geometry", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 16.0),
                DVec3::new(16.0, 16.0, 48.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let mut child = document.into_vmf();
    sample(&mut child, "-64 0 64", Some("1"));
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room();
    let root_helper = sample(&mut parent, "-96 0 64", Some("1"));
    let side = parent.world.solids[0]
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.9)
        .unwrap()
        .id;
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("sides", side.to_string());
    instance(&mut parent, "leaf.vmf", "64 0 0", "0 0 0");
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &game,
        capture_options(LightingMode::DirectLdr),
        &source_context,
    );
    pending(&output);
    let assignments = &output.cubemap_assets.as_ref().unwrap().assignments;
    assert!(
        assignments
            .iter()
            .any(|assignment| assignment.face_id == side
                && assignment.helper_id == root_helper
                && assignment.manual)
    );
    let generated = assignments
        .iter()
        .find(|assignment| !parent.side_ids().any(|id| id == assignment.face_id))
        .unwrap()
        .face_id;
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.id == root_helper)
        .unwrap()
        .properties
        .set("sides", generated.to_string());
    let before = parent.clone();
    let source_context = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &game,
        capture_options(LightingMode::DirectLdr),
        &source_context,
    );
    assert!(output.bsp.is_none() && output.cubemap_assets.is_none());
    assert!(output.stats.instances.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("original root map")
            && diagnostic.message.contains(&format!("side:{generated}"))),
        "{:?}",
        output.report
    );
    assert_eq!(parent, before);
}

#[test]
fn transitive_sample_dependencies_and_cancellation_stop_capture_and_publication() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = child_sample("0 0 64", Some("1"));
    let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    let middle_bytes = write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room();
    let owner = instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0");
    let mut document = Document::new(parent);
    document
        .edit("authored instance name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "assembly");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let output_directory = tempfile::tempdir().unwrap();
    let path = output_directory.path().join("instanced_cubemaps.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    let source_context = saved_parent(directory.path(), document.vmf());
    let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let ordinary = compile_saved(document.vmf(), &game, Options::default(), &source_context);
    assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
    let output = compile_saved(
        document.vmf(),
        &game,
        capture_options(LightingMode::DirectLdr),
        &source_context,
    );
    pending(&output);
    assert_eq!(
        output
            .stats
            .instances
            .as_ref()
            .unwrap()
            .dependencies
            .iter()
            .map(|dependency| dependency.path.as_str())
            .collect::<Vec<_>>(),
        ["leaf.vmf", "middle.vmf"]
    );
    assert!(
        output
            .write(&path)
            .unwrap_err()
            .to_string()
            .contains("pending native capture")
    );
    let staged = ordinary
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    let mut changed = child_bytes.clone();
    changed.push(b'\n');
    std::fs::write(directory.path().join("leaf.vmf"), changed).unwrap();
    assert!(staged.commit(&Control::default()).is_err());
    assert!(ordinary.write(&path).is_err());
    let environment = CaptureEnvironment {
        engine_root: "/unused/engine".into(),
        steam_root: "/unused/steam".into(),
        mounts: Vec::new(),
    };
    let result = finalize_reflections_with_capture(
        output,
        &context(),
        &game,
        &environment,
        &Control::default(),
        |_request, _observer| panic!("stale child reached capture"),
    );
    assert!(result.is_err());
    std::fs::write(directory.path().join("leaf.vmf"), &child_bytes).unwrap();
    let output = compile_saved(
        document.vmf(),
        &game,
        capture_options(LightingMode::DirectLdrHdr),
        &source_context,
    );
    pending(&output);
    let cancelled = Control::default();
    cancelled.cancel().unwrap();
    let result = finalize_reflections_with_capture(
        output,
        &context(),
        &game,
        &environment,
        &cancelled,
        |_request, _observer| panic!("cancelled assembly reached capture"),
    );
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(document.redo_labels().count(), 0);
    for (name, bytes) in [
        ("parent.vmf", parent_bytes),
        ("middle.vmf", middle_bytes),
        ("leaf.vmf", child_bytes),
    ] {
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
    }
}
