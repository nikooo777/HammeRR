mod common;

use std::path::{Path, PathBuf};

use compiler::{Options, PhysicsMode, WATER_CURRENT_VELOCITY_KEY};
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

struct Fixture {
    directory: PathBuf,
    game: GameFs,
    gameinfo: PathBuf,
}

fn fixture() -> Fixture {
    let retained = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/637");
    std::fs::create_dir_all(&retained).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("current-")
        .tempdir_in(retained)
        .unwrap()
        .keep();
    let game_dir = directory.join("game");
    std::fs::create_dir_all(game_dir.join("materials/dev")).unwrap();
    for (name, text) in [
        ("dev/dev_measuregeneric01", "LightmappedGeneric {}"),
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
        ("water", "Water { $abovewater 1 }"),
        ("slime", "LightmappedGeneric { %compileslime 1 }"),
    ] {
        std::fs::write(game_dir.join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let gameinfo = game_dir.join("gameinfo.txt");
    std::fs::write(
        &gameinfo,
        "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.base_dir = directory.to_owned();
    game.game_dir = game_dir.clone();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(game_dir),
    });
    Fixture {
        directory,
        game,
        gameinfo,
    }
}

fn pool(vmf: Vmf, material: &str, role: Option<&str>, hidden: bool) -> Vmf {
    let mut document = Document::new(vmf);
    document
        .edit("current fixture", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            let solid = tx.solid_mut(id)?;
            solid.hidden = hidden;
            for side in &mut solid.sides {
                if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = material.into();
                }
            }
            if let Some(class) = role {
                edit::tie_to_entity(tx, &[id], class, &[])?;
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn options(physics: PhysicsMode) -> Options {
    Options {
        physics,
        ..Options::default()
    }
}

fn rejected(
    vmf: &Vmf,
    fixture: &Fixture,
    physics: PhysicsMode,
    fragment: &str,
) -> compiler::Output {
    let source = vmf.write(&Default::default());
    let result = compiler::compile_with_options(vmf, &fixture.game, options(physics));
    assert!(result.report.has_errors(), "{:?}", result.report);
    assert!(result.bsp.is_none());
    assert!(
        result.report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains(WATER_CURRENT_VELOCITY_KEY)
            && diagnostic.message.contains(fragment)),
        "{:?}",
        result.report
    );
    assert_eq!(vmf.write(&Default::default()), source);
    let previous = fixture.directory.join("previous.bsp");
    std::fs::write(&previous, b"previous checked output").unwrap();
    assert!(!result.write(&previous).unwrap());
    assert_eq!(std::fs::read(previous).unwrap(), b"previous checked output");
    result
}

#[test]
fn public_compiler_rejects_malformed_duplicate_and_inactive_controls_without_publication() {
    let fixture = fixture();
    for raw in [
        "",
        "1 2",
        "1 2 3 4",
        "NaN 0 0",
        "inf 0 0",
        "128 1 0",
        "128.00001 0 0",
        "127.999999 0.01 0",
        "1e-50 0 0",
        "1e-45 0 0",
    ] {
        let mut vmf = common::room();
        vmf.world.properties.set(WATER_CURRENT_VELOCITY_KEY, raw);
        rejected(&vmf, &fixture, PhysicsMode::Engine, "needs exactly");
    }
    let mut vmf = pool(common::room(), "water", None, false);
    vmf.world
        .properties
        .set(WATER_CURRENT_VELOCITY_KEY, "0 0 0");
    rejected(&vmf, &fixture, PhysicsMode::None, "engine physics");
    vmf.world.properties.0.push((
        WATER_CURRENT_VELOCITY_KEY.to_ascii_uppercase(),
        "1 0 0".into(),
    ));
    rejected(&vmf, &fixture, PhysicsMode::Engine, "repeats");
}

#[test]
fn retained_slime_detail_and_model_liquids_cannot_receive_partial_uniform_current() {
    let fixture = fixture();
    for (material, role, hidden, keep_water, fragment) in [
        ("water", None, true, false, "retained structural"),
        ("slime", None, false, false, "unsupported"),
        ("water", Some("func_detail"), false, false, "unsupported"),
        ("water", Some("func_brush"), false, false, "unsupported"),
        ("slime", None, false, true, "unsupported"),
        ("water", Some("func_detail"), false, true, "unsupported"),
        ("water", Some("func_brush"), false, true, "unsupported"),
    ] {
        let base = if keep_water {
            pool(common::room(), "water", None, false)
        } else {
            common::room()
        };
        let mut vmf = pool(base, material, role, hidden);
        vmf.world
            .properties
            .set(WATER_CURRENT_VELOCITY_KEY, "32 -16 8");
        let result = rejected(&vmf, &fixture, PhysicsMode::Engine, fragment);
        if role == Some("func_detail") {
            let solid = vmf
                .entities
                .iter()
                .find(|entity| entity.classname() == Some("func_detail"))
                .unwrap()
                .solids[0]
                .id;
            let diagnostic = result
                .report
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.contains(WATER_CURRENT_VELOCITY_KEY))
                .unwrap();
            assert!(
                diagnostic
                    .message
                    .contains(&format!("solid {solid} is unsupported")),
                "{}",
                diagnostic.message
            );
            assert!(
                !diagnostic.message.contains(" in entity "),
                "{}",
                diagnostic.message
            );
        }
    }
    let mut vmf = common::room();
    vmf.world
        .properties
        .set(WATER_CURRENT_VELOCITY_KEY, "0 0 0");
    rejected(&vmf, &fixture, PhysicsMode::Engine, "retained structural");
}

#[test]
fn retained_water_without_a_wet_leaf_cannot_claim_an_applied_current() {
    let fixture = fixture();
    let mut vmf = pool(common::room(), "water", None, false);
    for entity in &mut vmf.entities {
        entity.properties.set("origin", "200 -200 8");
    }
    let mut document = Document::new(vmf);
    document
        .edit("cover complete pool", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-160.0, -160.0, 0.0),
                DVec3::new(160.0, 160.0, 96.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    vmf.world
        .properties
        .set(WATER_CURRENT_VELOCITY_KEY, "0 0 0");
    rejected(&vmf, &fixture, PhysicsMode::Engine, "requires emitted");
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
mod native {
    use super::*;
    use source_formats::bsp::{Bsp, PhysicsCollision, lump};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::{Command, Output};

    const PROXY: &str = r#"#!/usr/bin/env python3
import os, re, struct, subprocess, sys
from pathlib import Path
subprocess.run([os.environ['HAMMERR_REAL_PHYSICS_WORKER'], *sys.argv[1:]], check=True)
output = Path(sys.argv[3])
original = output.read_bytes()
index, body_bytes, key_bytes, count = struct.unpack_from('<4i', original)
assert index == 0 and len(original) == 16 + body_bytes + key_bytes + 16
assert struct.unpack_from('<4i', original, len(original)-16) == (-1,-1,0,0)
key_start = 16 + body_bytes
keys = original[key_start:key_start+key_bytes]
assert keys.endswith(b'\0') and b'\0' not in keys[:-1]
receipt = Path(os.environ['HAMMERR_CURRENT_RECEIPT'])
receipt.mkdir()
(receipt/'request.json').write_bytes(Path(sys.argv[2]).read_bytes())
(receipt/'before.bin').write_bytes(original)
if os.environ['HAMMERR_CURRENT_MODE'] == 'raw_hu':
    keys, changed = re.subn(rb'"currentvelocity" "[^"]*"', b'"currentvelocity" "64 -32 16"', keys)
    assert changed == 1
changed = struct.pack('<4i', index, body_bytes, len(keys), count) + original[16:key_start] + keys + original[-16:]
assert changed[16:key_start] == original[16:key_start]
(receipt/'after.bin').write_bytes(changed)
output.write_bytes(changed)
"#;

    fn installed_game() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("CSS_DIR") {
            let path = PathBuf::from(path);
            assert!(
                path.join("bin/linux64/vphysics.so").is_file(),
                "CSS_DIR has no native physics library"
            );
            return Some(path);
        }
        let path = PathBuf::from(std::env::var_os("HOME")?)
            .join(".local/share/Steam/steamapps/common/Counter-Strike Source");
        path.join("bin/linux64/vphysics.so")
            .is_file()
            .then_some(path)
    }

    fn compile(
        vmf: &Vmf,
        fixture: &Fixture,
        destination: &Path,
        proxy: &Path,
        name: &str,
        mode: &str,
    ) -> Output {
        let source = fixture.directory.join(format!("{name}.vmf"));
        std::fs::write(&source, vmf.write(&Default::default())).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_hammerr-compile"))
            .arg(&source)
            .arg("--game")
            .arg(&fixture.gameinfo)
            .arg("--output")
            .arg(destination)
            .arg("--physics")
            .arg("engine")
            .env("HAMMERR_PHYSICS_WORKER", proxy)
            .env(
                "HAMMERR_REAL_PHYSICS_WORKER",
                env!("CARGO_BIN_EXE_hammerr-physics"),
            )
            .env("HAMMERR_CURRENT_RECEIPT", fixture.directory.join(name))
            .env("HAMMERR_CURRENT_MODE", mode)
            .output()
            .unwrap();
        std::fs::write(
            fixture.directory.join(format!("{name}.stderr")),
            &result.stderr,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(source).unwrap(),
            vmf.write(&Default::default())
        );
        result
    }

    fn native_vector(bytes: &[u8]) -> [f32; 3] {
        let physics = PhysicsCollision::parse(bytes, 1).unwrap();
        let nodes = source_formats::keyvalues::parse_bounded(
            std::str::from_utf8(physics.models[0].key_data)
                .unwrap()
                .trim_end_matches('\0'),
            1,
            64,
        )
        .unwrap();
        let fluids: Vec<_> = nodes.iter().filter(|node| node.key == "fluid").collect();
        assert_eq!(fluids.len(), 1);
        let fields = fluids[0].children().unwrap();
        assert_eq!(
            fields
                .iter()
                .find(|field| field.key == "damping")
                .unwrap()
                .as_str(),
            Some("0.01")
        );
        let numbers: Vec<f32> = fields
            .iter()
            .find(|field| field.key == "currentvelocity")
            .unwrap()
            .as_str()
            .unwrap()
            .split_ascii_whitespace()
            .map(|number| number.parse().unwrap())
            .collect();
        numbers.try_into().unwrap()
    }

    #[test]
    fn real_cli_preserves_default_bytes_units_geometry_and_previous_output_on_worker_tamper() {
        let Some(installed) = installed_game() else {
            eprintln!("CS:S not installed; skipping native water-current control");
            return;
        };
        let fixture = fixture();
        symlink(installed.join("bin"), fixture.directory.join("bin")).unwrap();
        let proxy = fixture.directory.join("worker.py");
        std::fs::write(&proxy, PROXY).unwrap();
        std::fs::set_permissions(&proxy, std::fs::Permissions::from_mode(0o700)).unwrap();
        let destination = fixture.directory.join("current.bsp");
        let mut vmf = pool(common::room(), "water", None, false);
        let absent = compile(&vmf, &fixture, &destination, &proxy, "absent", "pass");
        assert!(
            absent.status.success(),
            "{}",
            String::from_utf8_lossy(&absent.stderr)
        );
        let baseline = std::fs::read(&destination).unwrap();
        vmf.world
            .properties
            .set(WATER_CURRENT_VELOCITY_KEY, "-0 0 +0");
        let zero = compile(&vmf, &fixture, &destination, &proxy, "zero", "pass");
        assert!(
            zero.status.success(),
            "{}",
            String::from_utf8_lossy(&zero.stderr)
        );
        assert_eq!(std::fs::read(&destination).unwrap(), baseline);
        vmf.world
            .properties
            .set(WATER_CURRENT_VELOCITY_KEY, "64 -32 16");
        let flow = compile(&vmf, &fixture, &destination, &proxy, "flow", "pass");
        assert!(
            flow.status.success(),
            "{}",
            String::from_utf8_lossy(&flow.stderr)
        );
        let accepted = std::fs::read(&destination).unwrap();
        let before = Bsp::parse(&baseline).unwrap();
        let after = Bsp::parse(&accepted).unwrap();
        assert_eq!(
            native_vector(&after.lump(lump::PHYSCOLLIDE).data),
            [1.6256, -0.8128, 0.4064]
        );
        assert!(
            !after
                .entities()
                .unwrap()
                .to_ascii_lowercase()
                .contains(WATER_CURRENT_VELOCITY_KEY)
        );
        let dry = before.physics().unwrap().unwrap();
        let wet = after.physics().unwrap().unwrap();
        assert_eq!(dry.models[0].solids, wet.models[0].solids);
        for index in 0..source_formats::bsp::LUMP_COUNT {
            if index != lump::PHYSCOLLIDE {
                assert_eq!(
                    before.lump(index).data,
                    after.lump(index).data,
                    "lump {index}"
                );
            }
        }
        let request: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.directory.join("flow/request.json")).unwrap(),
        )
        .unwrap();
        let bodies = request["models"][0]["bodies"].as_array().unwrap();
        assert_eq!(
            bodies
                .iter()
                .find(|body| body["surface"].is_number())
                .unwrap()["current_hu"],
            serde_json::json!([64.0, -32.0, 16.0])
        );
        let bad = compile(&vmf, &fixture, &destination, &proxy, "raw-hu", "raw_hu");
        assert!(!bad.status.success());
        assert!(String::from_utf8_lossy(&bad.stderr).contains("Physics worker world keys"));
        assert_eq!(std::fs::read(&destination).unwrap(), accepted);
        let original = std::fs::read(fixture.directory.join("raw-hu/before.bin")).unwrap();
        let changed = std::fs::read(fixture.directory.join("raw-hu/after.bin")).unwrap();
        assert_eq!(
            PhysicsCollision::parse(&original, 1).unwrap().models[0].solids,
            PhysicsCollision::parse(&changed, 1).unwrap().models[0].solids
        );
        eprintln!(
            "Water-current control artifacts: {}",
            fixture.directory.display()
        );
    }
}
