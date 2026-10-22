mod common;

use std::path::PathBuf;

use compiler::{Options, PhysicsMode};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{Brush, Leaf, Model, contents, lump};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    for (name, source) in [
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
        ("drawn", "LightmappedGeneric { }"),
        ("clip", "UnlitGeneric { %compileclip 1 }"),
        ("water", "Water { }"),
        ("trigger", "UnlitGeneric { %compiletrigger 1 }"),
        ("dev/dev_measuregeneric01", "LightmappedGeneric { }"),
        ("tools/toolsblack", "UnlitGeneric { $color \"[0 0 0]\" }"),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/{name}.vmt")),
            source,
        )
        .unwrap();
    }
    std::fs::create_dir(directory.path().join("scripts")).unwrap();
    std::fs::write(
        directory
            .path()
            .join("scripts/surfaceproperties_manifest.txt"),
        "surfaceproperties_manifest { file scripts/base.txt }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("scripts/base.txt"),
        "default { density 1000 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn fixture() -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    let id = document
        .edit("stationary water", |tx| {
            let ObjectId::Solid(brush) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 32.0),
                DVec3::new(32.0, 32.0, 96.0),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            let ObjectId::Entity(entity) = edit::tie_to_entity(
                tx,
                &[brush],
                compiler::WATER_ANALOG_CLASSNAME,
                &[("origin".into(), "0 0 64".into())],
            )?
            else {
                unreachable!()
            };
            Ok(entity)
        })
        .unwrap();
    (document.into_vmf(), id)
}

fn refuses(vmf: &Vmf, game: &GameFs, id: u32, physics: PhysicsMode, message: &str) {
    let before = vmf.clone();
    let output = compiler::compile_with_options(
        vmf,
        game,
        Options {
            physics,
            ..Default::default()
        },
    );
    assert!(
        output.bsp.is_none() && output.leak.is_none(),
        "{:?}",
        output.report
    );
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.level == compiler::Level::Error
                && diagnostic.object == Some(format!("entity:{id}"))
                && diagnostic.message.contains(message)
        }),
        "{:?}",
        output.report
    );
    assert!(
        !output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("vphysics.so")),
        "{:?}",
        output.report
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    let pointfile = path.with_extension("lin");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(&pointfile, b"previous LIN").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(pointfile).unwrap(), b"previous LIN");
    assert_eq!(vmf, &before);
}

#[test]
fn invisible_carrier_remains_a_dry_internal_brush_and_requires_engine_physics() {
    let (_directory, game) = game();
    let (vmf, id) = fixture();
    let mut report = compiler::Report::default();
    let map = compiler::map::Map::load(&vmf, &game, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let entity = map.entities.iter().find(|entity| entity.id == id).unwrap();
    assert_eq!(entity.brushes.len(), 1);
    let brush = &map.brushes[entity.brushes[0]];
    assert_eq!(brush.contents, contents::SOLID);
    assert!(brush.sides.iter().all(|side| !side.visible));
    assert_eq!(brush.mins, DVec3::splat(-32.0));
    assert_eq!(brush.maxs, DVec3::splat(32.0));
    refuses(
        &vmf,
        &game,
        id,
        PhysicsMode::None,
        "CS:S engine physics is required",
    );
}

#[test]
fn malformed_carriers_fail_with_source_diagnostics_before_native_work() {
    let (_directory, game) = game();
    for variant in [
        "empty",
        "hidden child",
        "two brushes",
        "redundant side",
        "sloped",
        "missing origin",
        "duplicate origin",
        "bracketed origin",
        "array origin",
        "long native origin",
        "parent",
        "angles",
        "start",
        "spawn",
        "speed",
        "duplicate class",
        "classname key",
        "drawn",
        "missing",
        "clip",
        "water",
        "trigger",
    ] {
        let (mut vmf, id) = fixture();
        let entity = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        match variant {
            "empty" => entity.solids.clear(),
            "hidden child" => entity.solids[0].hidden = true,
            "two brushes" => {
                let mut second = entity.solids[0].clone();
                second.id += 1000;
                for side in &mut second.sides {
                    side.id += 1000;
                }
                entity.solids.push(second);
            }
            "sloped" => {
                let side = entity.solids[0]
                    .sides
                    .iter_mut()
                    .find(|side| {
                        map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99
                    })
                    .unwrap();
                side.plane.0[0].z += 8.0;
            }
            "redundant side" => {
                let mut side = entity.solids[0].sides[0].clone();
                side.id += 1000;
                entity.solids[0].sides.push(side);
            }
            "missing origin" => entity.properties.0.retain(|(key, _)| key != "origin"),
            "duplicate origin" => entity.properties.0.push(("Origin".into(), "0 0 64".into())),
            "bracketed origin" => entity.properties.set("origin", "(0 0 64)"),
            "array origin" => entity.properties.set("origin", "[0 0 64]"),
            "long native origin" => entity
                .properties
                .set("origin", format!("0 0 {}64", "0".repeat(122))),
            "parent" => entity.properties.set("parentname", "parent"),
            "angles" => entity.properties.set("angles", "0 1 0"),
            "start" => entity.properties.set("StartPosition", "1"),
            "spawn" => entity.properties.set("spawnflags", "1"),
            "speed" => entity.properties.set("speed", "100"),
            "duplicate class" => entity
                .properties
                .0
                .push(("ClassName".into(), compiler::WATER_ANALOG_CLASSNAME.into())),
            "classname key" => {
                let key = &mut entity
                    .properties
                    .0
                    .iter_mut()
                    .find(|(key, _)| key == "classname")
                    .unwrap()
                    .0;
                *key = "ClassName".into();
            }
            material => {
                for side in &mut entity.solids[0].sides {
                    side.material = material.into();
                }
            }
        }
        for physics in [PhysicsMode::None, PhysicsMode::Engine] {
            refuses(&vmf, &game, id, physics, "Cannot compile func_water_analog");
        }
    }
}

#[test]
fn complete_box_and_other_initial_brush_bounds_are_checked() {
    let (_directory, game) = game();
    for (mins, maxs) in [
        ([-8.0, -8.0, 48.0], [8.0, 8.0, 80.0]),
        ([32.0, -8.0, 48.0], [48.0, 8.0, 80.0]),
    ] {
        let (vmf, id) = fixture();
        let mut document = Document::new(vmf);
        document
            .edit("obstruction", |tx| {
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "nodraw",
                )
            })
            .unwrap();
        refuses(
            document.vmf(),
            &game,
            id,
            PhysicsMode::Engine,
            "conservative bounds",
        );
    }
}

#[test]
fn nonzero_underflow_does_not_become_a_stationary_zero_default() {
    let (_directory, game) = game();
    for (key, value) in [
        ("angles", "0 1e-9999 0"),
        ("angles", "0 1e-50 0"),
        ("angle", "1e-9999"),
        ("StartPosition", "1e-9999"),
        ("spawnflags", "1e-9999"),
    ] {
        let (mut vmf, id) = fixture();
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap()
            .properties
            .set(key, value);
        refuses(
            &vmf,
            &game,
            id,
            PhysicsMode::Engine,
            "Cannot compile func_water_analog",
        );
    }
}

#[test]
fn rotated_other_brush_bounds_cannot_enter_the_carrier() {
    let (_directory, game) = game();
    let (vmf, id) = fixture();
    let mut document = Document::new(vmf);
    document
        .edit("rotated obstruction", |tx| {
            let ObjectId::Solid(brush) = edit::create_block(
                tx,
                DVec3::new(-96.0, 88.0, 56.0),
                DVec3::new(-64.0, 104.0, 72.0),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[brush],
                "func_brush",
                &[
                    ("origin".into(), "0 96 64".into()),
                    ("angles".into(), "0 90 0".into()),
                ],
            )
        })
        .unwrap();
    refuses(
        document.vmf(),
        &game,
        id,
        PhysicsMode::Engine,
        "conservative bounds",
    );
}

#[test]
fn tiny_authored_tilts_are_not_admitted_as_axis_aligned_planes() {
    let (_directory, game) = game();
    let (mut vmf, id) = fixture();
    let entity = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap();
    let side = entity.solids[0]
        .sides
        .iter_mut()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap();
    side.plane.0[0].z += 1e-7;
    refuses(
        &vmf,
        &game,
        id,
        PhysicsMode::Engine,
        "only axis-aligned cuboids",
    );
}

#[test]
fn hidden_carrier_preserves_ordinary_output() {
    let (_directory, game) = game();
    let (mut vmf, id) = fixture();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .hidden = true;
    let actual = compiler::compile(&vmf, &game);
    let expected = compiler::compile(&common::room(), &game);
    assert!(!actual.report.has_errors(), "{:?}", actual.report);
    assert_eq!(
        actual.bsp.unwrap().to_bytes(),
        expected.bsp.unwrap().to_bytes()
    );
}

#[test]
fn region_omitted_carrier_does_not_require_engine_physics() {
    use source_formats::keyvalues::Node;
    let (_directory, game) = game();
    let (mut vmf, id) = fixture();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("origin", "4096 0 64");
    vmf.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 192"),
        ],
    ));
    let before = vmf.clone();
    let output = compiler::compile(&vmf, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(
        !output
            .bsp
            .unwrap()
            .entities()
            .unwrap()
            .contains(compiler::WATER_ANALOG_CLASSNAME)
    );
    assert_eq!(vmf, before);
}

#[test]
fn third_retained_carrier_is_rejected_before_native_work() {
    let (_directory, game) = game();
    let (mut vmf, id) = fixture();
    let template = vmf
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap()
        .clone();
    for index in 1..=2 {
        let mut entity = template.clone();
        entity.id += index * 1000;
        for solid in &mut entity.solids {
            solid.id += index * 1000;
            for side in &mut solid.sides {
                side.id += index * 1000;
            }
        }
        edit::translate_entity(&mut entity, DVec3::X * f64::from(index * 80), true);
        vmf.entities.push(entity);
    }
    refuses(
        &vmf,
        &game,
        template.id + 2000,
        PhysicsMode::Engine,
        "at most 2 retained",
    );
}

#[test]
fn stationary_carriers_refuse_other_liquids_and_authored_world_water_controls() {
    let (_directory, game) = game();
    for (key, value) in [
        (compiler::WATER_CURRENT_VELOCITY_KEY, "0 0 0"),
        (compiler::WATER_DAMPING_KEY, "0"),
    ] {
        let (mut vmf, id) = fixture();
        vmf.world.properties.set(key, value);
        refuses(
            &vmf,
            &game,
            id,
            PhysicsMode::Engine,
            "authored world water current or damping",
        );
    }
    let (vmf, id) = fixture();
    let mut document = Document::new(vmf);
    document
        .edit("separate world water", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-160.0, -160.0, 16.0),
                DVec3::new(-96.0, -96.0, 48.0),
                "water",
            )
        })
        .unwrap();
    refuses(
        document.vmf(),
        &game,
        id,
        PhysicsMode::Engine,
        "other WATER or SLIME brush geometry",
    );
}

fn installed_game() -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("CSS_DIR")
            .expect("set CSS_DIR for the explicit native water-carrier test"),
    );
    assert!(
        root.join("bin/linux64/vphysics.so").is_file(),
        "CSS_DIR must contain bin/linux64/vphysics.so"
    );
    root
}

#[test]
#[ignore = "requires installed CS:S engine physics"]
fn native_carrier_matches_dry_nodraw_body_and_only_changes_owned_contents() {
    let root = installed_game();
    let (_directory, mut game) = game();
    game.base_dir = root;
    let (mut vmf, id) = fixture();
    let options = Options {
        physics: PhysicsMode::Engine,
        ..Default::default()
    };
    let output = compiler::compile_with_options(&vmf, &game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let actual = output.bsp.unwrap();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("classname", "func_movelinear");
    let output = compiler::compile_with_options(&vmf, &game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let dry = output.bsp.unwrap();
    for (index, (actual, expected)) in actual.lumps.iter().zip(&dry.lumps).enumerate() {
        if ![lump::ENTITIES, lump::BRUSHES, lump::LEAFS].contains(&index) {
            assert_eq!(actual.data, expected.data, "lump {index}");
        }
    }
    let models = actual.records::<Model>().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[1].face_count, 0);
    assert_eq!(models[1].mins, [-32.0; 3]);
    assert_eq!(models[1].maxs, [32.0; 3]);
    assert_eq!(
        actual.records::<Brush>().unwrap().len(),
        dry.records::<Brush>().unwrap().len()
    );
    assert_eq!(
        actual.records::<Leaf>().unwrap().len(),
        dry.records::<Leaf>().unwrap().len()
    );
    let mut changed_brushes = 0;
    for (actual, expected) in actual
        .records::<Brush>()
        .unwrap()
        .iter()
        .zip(dry.records::<Brush>().unwrap())
    {
        if actual != &expected {
            changed_brushes += 1;
            assert_eq!(actual.contents, contents::WATER);
            assert_eq!(expected.contents, contents::SOLID);
            assert_eq!(actual.first_side, expected.first_side);
            assert_eq!(actual.side_count, expected.side_count);
        }
    }
    assert_eq!(changed_brushes, 1);
    let mut changed_leaves = 0;
    for (index, (actual, expected)) in actual
        .records::<Leaf>()
        .unwrap()
        .iter()
        .zip(dry.records::<Leaf>().unwrap())
        .enumerate()
    {
        if actual != &expected {
            changed_leaves += 1;
            assert_ne!(index, 0);
            assert_eq!(actual.contents, contents::WATER);
            let mut restored = *actual;
            restored.contents = contents::SOLID;
            assert_eq!(restored, expected);
        }
    }
    assert_eq!(changed_leaves, 1);
    let physics = actual.physics().unwrap().unwrap();
    let inline = physics
        .models
        .iter()
        .find(|model| model.index == 1)
        .unwrap();
    assert_eq!(inline.solids.len(), 1);
    let text = std::str::from_utf8(inline.key_data)
        .unwrap()
        .trim_end_matches('\0');
    let nodes = source_formats::keyvalues::parse(text).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].key, "solid");
    assert!(
        nodes[0]
            .children()
            .unwrap()
            .iter()
            .any(|field| field.key == "index" && field.as_str() == Some("0"))
    );
}
