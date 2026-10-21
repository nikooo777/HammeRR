mod common;

use glam::DVec3;
use map_core::{Document, edit};
use source_formats::bsp::{self, Bsp, Leaf};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Entity, Vmf};

const PHYSICS_ASSET: &str = "models/props/test.phy";

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("models/props")).unwrap();
    let mut model = vec![0; 240];
    model[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 2)] {
        model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, values) in [
        (104, [-16.0_f32, -8.0, 0.0]),
        (116, [16.0, 8.0, 32.0]),
        (128, [-16.0, -8.0, 0.0]),
        (140, [16.0, 8.0, 32.0]),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            model[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(directory.path().join("models/props/test.mdl"), model).unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn add_prop(vmf: &mut Vmf, origin: &str) -> usize {
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap_or(0) + 1;
    let mut prop = Entity::new(id, "prop_static");
    for (key, value) in [
        ("model", "models/props/test.mdl"),
        ("origin", origin),
        ("solid", "2"),
    ] {
        prop.properties.set(key, value);
    }
    vmf.entities.push(prop);
    vmf.entities.len() - 1
}

fn physics_bytes(checksum: i32) -> Vec<u8> {
    let mut bytes = [16_i32, 0, 1, checksum].map(i32::to_le_bytes).concat();
    bytes.extend_from_slice(&32_i32.to_le_bytes());
    bytes.extend_from_slice(b"VPHY");
    bytes.extend_from_slice(&0x100_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 20]);
    bytes.extend_from_slice(b"solid {}\0");
    bytes
}

fn physics_directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/validation/694/compiler-tests");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("static-prop-phy-")
        .tempdir_in(root)
        .unwrap();
    std::fs::create_dir_all(directory.path().join("models/props")).unwrap();
    directory
}

fn physics_vpk(path: &std::path::Path, preload: &[u8], length: u32) -> Mount {
    let mut tree = b"phy\0models/props\0test\0".to_vec();
    tree.extend(0_u32.to_le_bytes());
    tree.extend(u16::try_from(preload.len()).unwrap().to_le_bytes());
    tree.extend(42_u16.to_le_bytes());
    tree.extend(0_u32.to_le_bytes());
    tree.extend(length.to_le_bytes());
    tree.extend(0xffff_u16.to_le_bytes());
    tree.extend(preload);
    tree.extend(b"\0\0\0");
    let mut bytes = 0x55aa_1234_u32.to_le_bytes().to_vec();
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(u32::try_from(tree.len()).unwrap().to_le_bytes());
    bytes.extend(tree);
    std::fs::write(path, bytes).unwrap();
    Mount::Pak(source_formats::vpk::Vpk::open(path).unwrap())
}

fn assert_physics_refusal(
    source: &Vmf,
    game: &GameFs,
    prop: usize,
    destination: &std::path::Path,
    previous: &[u8],
    expected: &str,
) {
    let before = source.clone();
    let output = compiler::compile(source, game);
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(
        output.report.diagnostics.iter().any(|row| {
            row.object == Some(format!("entity:{}", source.entities[prop].id))
                && row.message.contains(expected)
        }),
        "{:?}",
        output.report
    );
    assert!(!output.write(destination).unwrap());
    assert_eq!(std::fs::read(destination).unwrap(), previous);
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous pointfile"
    );
    assert_eq!(*source, before);
}

#[test]
fn static_prop_physics_directory_reads_are_bounded_before_parser_validation() {
    let (_models, mut game) = game();
    let assets = physics_directory();
    let physics = assets.path().join(PHYSICS_ASSET);
    std::fs::write(&physics, physics_bytes(0)).unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(assets.path().to_owned()),
        },
    );
    let mut source = common::room();
    let prop = add_prop(&mut source, "32 32 8");
    source.entities[prop].properties.set("solid", "6");
    let second = add_prop(&mut source, "-32 32 8");
    source.entities[second].properties.set("solid", "6");
    let baseline = compiler::compile(&source, &game);
    assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
    let bsp = baseline.bsp.unwrap();
    let props = bsp.static_props().unwrap().unwrap();
    assert_eq!(props.models.len(), 1);
    assert_eq!(props.props.len(), 2);
    assert!(props.props.iter().all(|prop| prop.solid == 6));
    assert_eq!(props.props[0].origin, [32.0, 32.0, 8.0]);
    assert_eq!(props.props[1].origin, [-32.0, 32.0, 8.0]);
    let previous = bsp.to_bytes();
    let destination = assets.path().join("previous.bsp");
    std::fs::write(&destination, &previous).unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous pointfile").unwrap();

    let file = std::fs::File::create(&physics).unwrap();
    file.set_len(bsp::MAX_PHYSICS_BYTES as u64).unwrap();
    assert_physics_refusal(
        &source,
        &game,
        prop,
        &destination,
        &previous,
        "models/props/test.phy: model physics header/count/checksum is invalid",
    );
    file.set_len(bsp::MAX_PHYSICS_BYTES as u64 + 1).unwrap();
    assert_physics_refusal(
        &source,
        &game,
        prop,
        &destination,
        &previous,
        "models/props/test.phy: asset exceeds the caller's byte read limit",
    );
    assert_eq!(
        std::fs::metadata(&physics).unwrap().len(),
        bsp::MAX_PHYSICS_BYTES as u64 + 1
    );
    drop(file);

    for (bytes, expected) in [
        (
            physics_bytes(17),
            "model physics header/count/checksum is invalid",
        ),
        (
            physics_bytes(0)[..20].to_vec(),
            "model physics solid is truncated",
        ),
        (b"bad".to_vec(), "model physics file size is invalid"),
    ] {
        std::fs::write(&physics, &bytes).unwrap();
        assert_physics_refusal(
            &source,
            &game,
            prop,
            &destination,
            &previous,
            &format!("{PHYSICS_ASSET}: {expected}"),
        );
        assert_eq!(std::fs::read(&physics).unwrap(), bytes);
    }
    std::fs::remove_file(&physics).unwrap();
    assert_physics_refusal(
        &source,
        &game,
        prop,
        &destination,
        &previous,
        "models/props/test.mdl needs its .phy collision asset for VPhysics",
    );
}

#[test]
fn static_prop_physics_vpk_limit_includes_preload_and_keeps_the_winning_source() {
    let (models, mut game) = game();
    let assets = physics_directory();
    let bytes = physics_bytes(0);
    std::fs::write(models.path().join(PHYSICS_ASSET), &bytes).unwrap();
    let mut source = common::room();
    let prop = add_prop(&mut source, "32 32 8");
    source.entities[prop].properties.set("solid", "6");
    let baseline = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    let (preload, payload) = bytes.split_at(16);
    std::fs::write(assets.path().join("valid_042.vpk"), payload).unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: physics_vpk(
                &assets.path().join("valid_dir.vpk"),
                preload,
                u32::try_from(payload.len()).unwrap(),
            ),
        },
    );
    assert_eq!(
        compiler::compile(&source, &game).bsp.unwrap().to_bytes(),
        baseline
    );
    let destination = assets.path().join("previous.bsp");
    std::fs::write(&destination, &baseline).unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous pointfile").unwrap();
    game.mounts[0].mount = physics_vpk(
        &assets.path().join("oversized_dir.vpk"),
        b"x",
        u32::try_from(bsp::MAX_PHYSICS_BYTES).unwrap(),
    );
    assert_physics_refusal(
        &source,
        &game,
        prop,
        &destination,
        &baseline,
        "models/props/test.phy: VPK entry exceeds the caller's byte read limit",
    );
    assert!(!assets.path().join("oversized_042.vpk").exists());
    let corrupt = physics_bytes(17);
    game.mounts[0].mount = physics_vpk(&assets.path().join("corrupt_dir.vpk"), &corrupt, 0);
    assert_physics_refusal(
        &source,
        &game,
        prop,
        &destination,
        &baseline,
        "models/props/test.phy: model physics header/count/checksum is invalid",
    );
    assert_eq!(
        std::fs::read(models.path().join(PHYSICS_ASSET)).unwrap(),
        bytes
    );
}

#[test]
fn static_prop_physics_files_remain_unread_for_unused_collision_sources() {
    let (models, mut game) = game();
    let assets = physics_directory();
    let physics = assets.path().join(PHYSICS_ASSET);
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(assets.path().to_owned()),
        },
    );
    let mut source = common::room();
    let prop = add_prop(&mut source, "32 32 8");
    let mut cases = Vec::new();
    for solid in ["0", "2"] {
        source.entities[prop].properties.set("solid", solid);
        cases.push(source.clone());
    }
    source.entities[prop].properties.set("solid", "6");
    source.entities[prop].hidden = true;
    cases.push(source.clone());
    source.entities[prop].hidden = false;
    source.entities[prop].properties.set("origin", "192 32 8");
    let tools = models.path().join("materials/tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("toolsblack.vmt"), "UnlitGeneric { }").unwrap();
    let mut document = Document::new(source);
    document
        .edit("region", |tx| {
            map_core::compile_region::apply(
                tx,
                Some(map_core::compile_region::Definition {
                    enabled: true,
                    bounds: map_core::compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    cases.push(document.vmf().clone());
    for source in cases {
        let absent = compiler::compile(&source, &game);
        assert!(!absent.report.has_errors(), "{:?}", absent.report);
        let file = std::fs::File::create(&physics).unwrap();
        file.set_len(bsp::MAX_PHYSICS_BYTES as u64 + 1).unwrap();
        let before = source.clone();
        let unused = compiler::compile(&source, &game);
        assert!(!unused.report.has_errors(), "{:?}", unused.report);
        assert_eq!(
            unused.bsp.unwrap().to_bytes(),
            absent.bsp.unwrap().to_bytes()
        );
        assert_eq!(source, before);
        drop(file);
        std::fs::remove_file(&physics).unwrap();
    }
}

#[test]
fn model_scale_runtime_tokens_survive_and_static_writer_rejects_without_replacing_outputs() {
    let (directory, game) = game();
    let mut vmf = common::room();
    let prop = add_prop(&mut vmf, "32 32 8");
    let baseline = compiler::compile(&vmf, &game).bsp.unwrap().to_bytes();
    let path = directory.path().join("scale.bsp");
    let lin = path.with_extension("lin");
    std::fs::write(&path, &baseline).unwrap();
    std::fs::write(&lin, b"previous pointfile").unwrap();
    for raw in ["1", "1.00000001"] {
        let mut unit = vmf.clone();
        unit.entities[prop].properties.set("modelscale", raw);
        assert_eq!(
            compiler::compile(&unit, &game).bsp.unwrap().to_bytes(),
            baseline
        );
    }
    for raw in ["0.5", "2", "64"] {
        vmf.entities[prop].properties.set("modelscale", raw);
        let original = vmf.clone();
        let failed = compiler::compile(&vmf, &game);
        assert!(failed.report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("Static-prop output does not support modelscale other than 1")
        }));
        assert!(!failed.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), baseline);
        assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
        assert_eq!(vmf, original);
    }
    for class in ["prop_dynamic", "prop_physics", "custom_model_entity"] {
        vmf.entities[prop].properties.set("classname", class);
        vmf.entities[prop]
            .properties
            .set("modelscale", "0.5000000001");
        let original = vmf.clone();
        let output = compiler::compile(&vmf, &game);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.static_props, 0);
        let bsp = output.bsp.unwrap();
        let text = bsp.entities().unwrap();
        assert!(text.contains("\"modelscale\" \"0.5000000001\""));
        assert!(text.contains(&format!("\"classname\" \"{class}\"")));
        assert_eq!(vmf, original);
    }
}

#[test]
fn static_props_emit_native_dictionary_transforms_leaves_and_lighting_origins() {
    let (_directory, game) = game();
    let mut document = Document::new(common::room());
    document
        .edit("wall with an upper passage", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-8.0, -256.0, 0.0),
                DVec3::new(8.0, 256.0, 160.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    let first = add_prop(&mut vmf, "8.125 0.25 144.375");
    for (key, value) in [
        ("angles", "0 90 -15"),
        ("skin", "1"),
        ("fademindist", "-1"),
        ("fademaxdist", "500"),
        ("fadescale", "0.5"),
        ("lightingorigin", "sample"),
        ("disableshadows", "1"),
        ("mindxlevel", "80"),
        ("maxdxlevel", "95"),
    ] {
        vmf.entities[first].properties.set(key, value);
    }
    add_prop(&mut vmf, "-64 32 8");
    let hidden = add_prop(&mut vmf, "0 0 16");
    vmf.entities[hidden].hidden = true;
    vmf.entities[hidden].properties.set("model", "missing.mdl");
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
    let mut helper = Entity::new(id, "info_lighting");
    helper.properties.set("targetname", "sample");
    helper.properties.set("origin", "32.125 64.25 80.375");
    vmf.entities.push(helper);
    let before = vmf.clone();
    let output = compiler::compile(&vmf, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.static_props, 2);
    assert_eq!(output.stats.static_prop_models, 1);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let props = bsp.static_props().unwrap().unwrap();
    assert_eq!(props.version, 6);
    assert_eq!(props.model_name(0).unwrap(), "models/props/test.mdl");
    assert_eq!(props.props[0].origin, [8.125, 0.25, 144.375]);
    assert_eq!(props.props[0].angles, [0.0, 90.0, -15.0]);
    assert_eq!(props.props[0].lighting_origin, [32.125, 64.25, 80.375]);
    assert_eq!(props.props[0].flags, 1 | 2 | 16);
    assert_eq!(props.props[0].skin, 1);
    assert_eq!(props.props[0].fade_min, 500.0);
    assert_eq!(props.props[0].fade_max, 500.0);
    assert_eq!(props.props[0].fade_scale, 0.5);
    assert_eq!(props.props[0].dx_levels, [80, 95]);
    assert!(props.props[0].leaf_count > 1);
    assert_eq!(props.leaves.len(), output.stats.static_prop_leaf_refs);
    let leaves = bsp.records::<Leaf>().unwrap();
    assert!(
        props
            .leaves
            .iter()
            .all(|&index| leaves[usize::from(index)].contents & bsp::contents::SOLID == 0)
    );
    let entities = bsp.entities().unwrap();
    assert!(!entities.contains("prop_static") && !entities.contains("info_lighting"));
    assert_eq!(vmf, before);
}

#[test]
fn regions_preserve_external_lighting_references_and_ambiguity() {
    let (directory, game) = game();
    let tools = directory.path().join("materials/tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("toolsblack.vmt"), "UnlitGeneric { }").unwrap();
    let mut source = common::room();
    let prop = add_prop(&mut source, "-64 32 8");
    source.entities[prop]
        .properties
        .set("lightingorigin", "sample");
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut helper = Entity::new(id, "info_lighting");
    helper.properties.set("targetname", "sample");
    helper.properties.set("origin", "400.125 64.25 80.375");
    source.entities.push(helper);
    let baseline = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    let mut document = Document::new(source);
    document
        .edit("region", |tx| {
            map_core::compile_region::apply(
                tx,
                Some(map_core::compile_region::Definition {
                    enabled: true,
                    bounds: map_core::compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    let source = document.vmf().clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(source, *document.vmf());
    assert!(
        output
            .stats
            .compile_region
            .as_ref()
            .unwrap()
            .included
            .contains(&format!("entity:{id}"))
    );
    let data = output.bsp.unwrap().to_bytes();
    let props = Bsp::parse(&data).unwrap().static_props().unwrap().unwrap();
    assert_eq!(props.props[0].lighting_origin, [400.125, 64.25, 80.375]);
    assert_eq!(props.props[0].flags & 2, 2);
    let controlled = compiler::compile_controlled(&source, &game, &Default::default());
    assert_eq!(controlled.bsp.unwrap().to_bytes(), data);
    let mut omitted = source.clone();
    let outside = add_prop(&mut omitted, "400 0 8");
    omitted.entities[outside]
        .properties
        .set("lightingorigin", "unavailable");
    let filtered = compiler::compile(&omitted, &game);
    assert!(!filtered.report.has_errors(), "{:?}", filtered.report);
    assert_eq!(filtered.stats.static_props, 1);
    assert!(
        filtered
            .stats
            .compile_region
            .unwrap()
            .omitted
            .contains(&format!("entity:{}", omitted.entities[outside].id))
    );
    let path = directory.path().join("region.bsp");
    std::fs::write(&path, &data).unwrap();
    for duplicate in [false, true] {
        let mut invalid = source.clone();
        if duplicate {
            let mut helper = invalid.entities.last().unwrap().clone();
            helper.id += 100;
            helper.properties.set("origin", "32 32 64");
            invalid.entities.push(helper);
        } else {
            invalid.entities.last_mut().unwrap().hidden = true;
        }
        let before = invalid.clone();
        let failed = compiler::compile(&invalid, &game);
        let expected = if duplicate {
            "multiple info_lighting"
        } else {
            "one visible info_lighting"
        };
        assert!(
            failed
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains(expected)),
            "{:?}",
            failed.report
        );
        assert!(!failed.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(invalid, before);
    }
    document.undo().unwrap();
    assert_eq!(
        compiler::compile(document.vmf(), &game)
            .bsp
            .unwrap()
            .to_bytes(),
        baseline
    );
}

#[test]
fn invalid_static_props_preserve_previous_output_and_source() {
    let (directory, game) = game();
    let mut vmf = common::room();
    let prop = add_prop(&mut vmf, "32 32 8");
    let outputs = directory.path().join("outputs");
    std::fs::create_dir(&outputs).unwrap();
    let path = outputs.join("existing.bsp");
    let output = compiler::compile(&vmf, &game);
    assert!(output.write(&path).unwrap());
    let old = std::fs::read(&path).unwrap();
    for amount in ["0", "128", "255"] {
        let mut normal = vmf.clone();
        normal.entities[prop].properties.set("rendermode", "0");
        normal.entities[prop].properties.set("renderamt", amount);
        let output = compiler::compile(&normal, &game);
        assert_eq!(output.bsp.unwrap().to_bytes(), old);
    }
    for (key, value) in [
        ("model", "models/absent.mdl"),
        ("model", "models/../escape.mdl"),
        ("angles", "0 NaN 0"),
        ("skin", "2"),
        ("solid", "5"),
        ("solid", "6"),
        ("fadescale", "-1"),
        ("lightingorigin", "missing"),
        ("generatelightmaps", "1"),
        ("modelscale", "2"),
        ("rendermode", "2"),
        ("renderfx", "1"),
        ("renderamt", "256"),
        ("origin", "16380 0 0"),
        ("origin", "1024 0 0"),
    ] {
        let mut invalid = vmf.clone();
        invalid.entities[prop].properties.set(key, value);
        let source = invalid.clone();
        let output = compiler::compile(&invalid, &game);
        assert!(
            output.report.has_errors(),
            "{key}={value}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), old);
        assert_eq!(invalid, source);
    }
    let control = compiler::Control::default();
    control.cancel().unwrap();
    assert!(
        compiler::compile_controlled(&vmf, &game, &control)
            .bsp
            .is_none()
    );
}

#[test]
fn invalid_model_bounds_static_flags_and_ambiguous_lighting_helpers_reject() {
    let (directory, game) = game();
    let mut vmf = common::room();
    let prop = add_prop(&mut vmf, "32 32 8");
    let path = directory.path().join("models/props/test.mdl");
    let old = std::fs::read(&path).unwrap();
    for (offset, value) in [
        (152, 0_i32),
        (104, f32::NAN.to_bits() as i32),
        (140, (-64.0_f32).to_bits() as i32),
    ] {
        let mut bytes = old.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(compiler::compile(&vmf, &game).report.has_errors());
    }
    std::fs::write(&path, old).unwrap();
    vmf.entities[prop]
        .properties
        .set("lightingorigin", "sample");
    for id in [900, 901] {
        let mut helper = Entity::new(id, "info_lighting");
        helper.properties.set("targetname", "sample");
        helper.properties.set("origin", "0 0 64");
        vmf.entities.push(helper);
    }
    assert!(compiler::compile(&vmf, &game).report.has_errors());
    vmf.entities.last_mut().unwrap().hidden = true;
    assert!(!compiler::compile(&vmf, &game).report.has_errors());
}

#[test]
fn unused_duplicate_and_originless_helpers_preserve_no_reference_static_prop_bytes() {
    let (_directory, game) = game();
    let mut source = common::room();
    add_prop(&mut source, "32 32 8");
    let baseline = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    for hidden in [false, false, true] {
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut helper = Entity::new(id, "info_lighting");
        helper.properties.set("targetname", "unused");
        helper.hidden = hidden;
        if hidden {
            helper.properties.set("origin", "0 NaN 64");
        }
        source.entities.push(helper);
    }
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
}
