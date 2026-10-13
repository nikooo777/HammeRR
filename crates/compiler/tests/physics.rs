mod common;

use std::path::PathBuf;
use std::process::Command;

use compiler::{Options, PhysicsMode};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{Bsp, CollisionHeader, Model, lump};
use source_formats::gamefs::{Mount, MountedPath};

fn installed_game() -> Option<PathBuf> {
    let root = std::env::var_os("CSS_DIR").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home).join(".local/share/Steam/steamapps/common/Counter-Strike Source")
        })
    })?;
    root.join("bin/linux64/vphysics.so")
        .is_file()
        .then_some(root)
}

#[test]
fn engine_worker_emits_checked_world_clip_and_local_brush_bodies() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native worker fixture");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    std::fs::write(
        materials.join("clip.vmt"),
        "UnlitGeneric { %compileclip 1 }",
    )
    .unwrap();
    let mut fs = common::empty_game();
    fs.base_dir = root;
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut document = Document::new(common::room());
    document
        .edit("physics fixtures", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 0.0),
                DVec3::new(32.0, 32.0, 48.0),
                "clip",
            )?;
            let ObjectId::Solid(body) = edit::create_block(
                tx,
                DVec3::new(80.0, 80.0, 16.0),
                DVec3::new(112.0, 112.0, 48.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[body],
                "func_brush",
                &[("origin".into(), "96 96 32".into())],
            )?;
            Ok(())
        })
        .unwrap();
    let before = document.vmf().write(&Default::default());
    let options = Options {
        physics: PhysicsMode::Engine,
        ..Options::default()
    };
    let output = compiler::compile_with_options(document.vmf(), &fs, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_mode, PhysicsMode::Engine);
    assert_eq!(output.stats.physics_models, 2);
    assert_eq!(output.stats.physics_solids, 4);
    assert_eq!(output.stats.physics_convexes, 9);
    let bsp = output.bsp.unwrap();
    let physics = bsp.physics().unwrap().unwrap();
    assert_eq!(physics.models.len(), 2);
    assert_eq!(physics.models[0].solids.len(), 3);
    assert_eq!(physics.models[1].solids.len(), 1);
    assert!(
        physics
            .models
            .iter()
            .flat_map(|model| &model.solids)
            .all(|solid| matches!(solid.header(), CollisionHeader::Convex { .. }))
    );
    let world_keys = std::str::from_utf8(physics.models[0].key_data).unwrap();
    assert!(
        world_keys.contains("\"contents\" \"65536\"")
            && world_keys.contains("\"contents\" \"131072\"")
    );
    assert!(world_keys.contains("materialtable"));
    let body_keys = std::str::from_utf8(physics.models[1].key_data).unwrap();
    assert!(body_keys.contains("solid {") && body_keys.contains("\"mass\" \"1\""));
    let model = bsp.records::<Model>().unwrap()[1];
    assert_eq!(model.mins, [-16.0; 3]);
    assert_eq!(model.maxs, [16.0; 3]);
    assert_eq!(
        output.stats.physics_bytes,
        bsp.lump(lump::PHYSCOLLIDE).data.len()
    );
    assert_eq!(
        Bsp::parse(&bsp.to_bytes())
            .unwrap()
            .physics()
            .unwrap()
            .unwrap()
            .to_bytes(2)
            .unwrap(),
        bsp.lump(lump::PHYSCOLLIDE).data
    );
    assert_eq!(document.vmf().write(&Default::default()), before);
    let repeated = compiler::compile_with_options(document.vmf(), &fs, options)
        .bsp
        .unwrap();
    assert_eq!(
        repeated.lump(lump::PHYSCOLLIDE).data,
        bsp.lump(lump::PHYSCOLLIDE).data
    );
}

#[test]
fn absent_and_unverified_backends_preserve_previous_output() {
    let directory = tempfile::tempdir().unwrap();
    let mut fs = common::empty_game();
    fs.base_dir = directory.path().into();
    let before = compiler::compile(&common::room(), &fs)
        .bsp
        .unwrap()
        .to_bytes();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, &before).unwrap();
    let options = Options {
        physics: PhysicsMode::Engine,
        ..Options::default()
    };
    let output = compiler::compile_with_options(&common::room(), &fs, options);
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let libraries = directory.path().join("bin/linux64");
    std::fs::create_dir_all(&libraries).unwrap();
    std::fs::write(libraries.join("vphysics.so"), "unverified").unwrap();
    let output = compiler::compile_with_options(&common::room(), &fs, options);
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|message| message.message.contains("Unverified CS:S vphysics")),
        "{:?}",
        output.report
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let request = directory.path().join("bad.json");
    std::fs::write(&request, "{\"model_count\":1,\"models\":[{\"index\":0,\"bodies\":[{\"contents\":33570827,\"shapes\":[{\"brush\":0,\"corners\":[[0,0,0],[1,0,0],[0,1,0],[1,1,0]],\"planes\":[[0,0,1,0],[0,0,-1,0],[1,0,0,1],[-1,0,0,0]]}]}]}]}").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hammerr-physics"))
        .arg(&libraries)
        .arg(request)
        .arg(directory.path().join("rejected.bin"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("coplanar")
            || String::from_utf8_lossy(&result.stderr).contains("empty bounds")
    );
    assert!(!directory.path().join("rejected.bin").exists());
}
