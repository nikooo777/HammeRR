#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

#[allow(dead_code)]
mod common;

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{Bsp, PhysicsCollision, lump};

const WORKER_PROXY: &str = r#"#!/usr/bin/env python3
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys

subprocess.run([os.environ['HAMMERR_REAL_PHYSICS_WORKER'], *sys.argv[1:]], check=True)
output = Path(sys.argv[3])
original = output.read_bytes()
index, body_bytes, key_bytes, count = struct.unpack_from('<4i', original)
assert index == 0 and len(original) == 16 + body_bytes + key_bytes + 16
assert struct.unpack_from('<4i', original, len(original) - 16) == (-1, -1, 0, 0)
key_start = 16 + body_bytes
keys = original[key_start:key_start + key_bytes]
assert keys.endswith(b'\0') and b'\0' not in keys[:-1]
mode = os.environ['HAMMERR_PHYSICS_KEY_MODE']
receipt = Path(os.environ['HAMMERR_PHYSICS_KEY_RECEIPT'])
receipt.mkdir()
(receipt / 'request.json').write_bytes(Path(sys.argv[2]).read_bytes())
(receipt / 'before.bin').write_bytes(original)
if mode == 'missing_fluid':
    keys, removed = re.subn(rb'(?m)^fluid \{\n[^{}]*\}\n', b'', keys, count=1)
    assert removed == 1
elif mode != 'pass':
    raise AssertionError(mode)
changed = (struct.pack('<4i', index, body_bytes, len(keys), count)
           + original[16:key_start] + keys + original[-16:])
assert changed[16:key_start] == original[16:key_start]
output.write_bytes(changed)
(receipt / 'after.bin').write_bytes(changed)
(receipt / 'receipt.json').write_text(json.dumps({
    'mode': mode, 'body_bytes': body_bytes, 'solid_count': count,
    'before_key_bytes': key_bytes, 'after_key_bytes': len(keys),
}) + '\n')
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

fn room_with_pool() -> source_formats::vmf::Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("world key fluid control", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = "water".into();
                }
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn compile(
    source: &Path,
    game: &Path,
    destination: &Path,
    proxy: &Path,
    receipt: &Path,
    mode: &str,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hammerr-compile"))
        .arg(source)
        .arg("--game")
        .arg(game)
        .arg("--output")
        .arg(destination)
        .arg("--physics")
        .arg("engine")
        .env("HAMMERR_PHYSICS_WORKER", proxy)
        .env(
            "HAMMERR_REAL_PHYSICS_WORKER",
            env!("CARGO_BIN_EXE_hammerr-physics"),
        )
        .env("HAMMERR_PHYSICS_KEY_MODE", mode)
        .env("HAMMERR_PHYSICS_KEY_RECEIPT", receipt)
        .output()
        .unwrap()
}

#[test]
fn parent_rejects_a_real_worker_fluid_block_removed_without_changing_geometry_or_framing() {
    let Some(installed) = installed_game() else {
        eprintln!("CS:S not installed; skipping real world-key worker control");
        return;
    };
    let retained = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/630");
    std::fs::create_dir_all(&retained).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("world-keys-")
        .tempdir_in(retained)
        .unwrap()
        .keep();
    let game = directory.join("game");
    std::fs::create_dir_all(game.join("materials/dev")).unwrap();
    symlink(installed.join("bin"), directory.join("bin")).unwrap();
    for (name, text) in [
        ("dev/dev_measuregeneric01", "LightmappedGeneric {}"),
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
        ("water", "Water { $abovewater 1 }"),
    ] {
        std::fs::write(game.join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let gameinfo = game.join("gameinfo.txt");
    std::fs::write(
        &gameinfo,
        "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
    )
    .unwrap();
    let source = directory.join("pool.vmf");
    std::fs::write(&source, room_with_pool().write(&Default::default())).unwrap();
    let proxy = directory.join("worker.py");
    std::fs::write(&proxy, WORKER_PROXY).unwrap();
    std::fs::set_permissions(&proxy, std::fs::Permissions::from_mode(0o700)).unwrap();
    let destination = directory.join("previous.bsp");
    let pass_receipt = directory.join("pass");
    let passed = compile(
        &source,
        &gameinfo,
        &destination,
        &proxy,
        &pass_receipt,
        "pass",
    );
    std::fs::write(directory.join("pass.stderr"), &passed.stderr).unwrap();
    assert!(
        passed.status.success(),
        "{}",
        String::from_utf8_lossy(&passed.stderr)
    );
    let previous = std::fs::read(&destination).unwrap();
    let bsp = Bsp::parse(&previous).unwrap();
    let generated = std::fs::read(pass_receipt.join("before.bin")).unwrap();
    assert_eq!(bsp.lump(lump::PHYSCOLLIDE).data, generated);
    let request: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pass_receipt.join("request.json")).unwrap()).unwrap();
    assert_eq!(request["model_count"], 1);
    let bodies = request["models"][0]["bodies"].as_array().unwrap();
    assert_eq!(
        bodies
            .iter()
            .filter(|body| body["surface"].is_number())
            .count(),
        1
    );

    let bad_receipt = directory.join("missing-fluid");
    let rejected = compile(
        &source,
        &gameinfo,
        &destination,
        &proxy,
        &bad_receipt,
        "missing_fluid",
    );
    std::fs::write(directory.join("missing-fluid.stderr"), &rejected.stderr).unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("Physics worker world keys"));
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    let before = std::fs::read(bad_receipt.join("before.bin")).unwrap();
    let after = std::fs::read(bad_receipt.join("after.bin")).unwrap();
    let original = PhysicsCollision::parse(&before, 1).unwrap();
    let changed = PhysicsCollision::parse(&after, 1).unwrap();
    assert_eq!(original.models.len(), changed.models.len());
    assert_eq!(original.models[0].index, changed.models[0].index);
    assert_eq!(original.models[0].solids, changed.models[0].solids);
    assert_ne!(original.models[0].key_data, changed.models[0].key_data);
    let nodes = source_formats::keyvalues::parse_bounded(
        std::str::from_utf8(changed.models[0].key_data)
            .unwrap()
            .trim_end_matches('\0'),
        1,
        128,
    )
    .unwrap();
    assert!(
        !nodes
            .iter()
            .any(|node| node.key.eq_ignore_ascii_case("fluid"))
    );
    eprintln!(
        "World physics key control artifacts: {}",
        directory.display()
    );
}
