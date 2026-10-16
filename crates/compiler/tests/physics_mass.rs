mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use compiler::{Options, PhysicsMode};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::{
    bsp::Bsp,
    gamefs::{GameFs, Mount, MountedPath},
    vmf::Vmf,
};

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

fn assets(directory: &Path, root: PathBuf) -> GameFs {
    std::fs::create_dir(directory.join("scripts")).unwrap();
    std::fs::create_dir(directory.join("materials")).unwrap();
    std::fs::write(
        directory.join("scripts/surfaceproperties_manifest.txt"),
        "surfaceproperties_manifest { file scripts/base.txt file scripts/mod.txt }",
    )
    .unwrap();
    std::fs::write(
        directory.join("scripts/base.txt"),
        "default { density 1000 } bulk { density 800 } shell { base bulk thickness 0.25 }",
    )
    .unwrap();
    std::fs::write(directory.join("scripts/mod.txt"), "bulk { density 1000 }").unwrap();
    for property in ["bulk", "shell"] {
        std::fs::write(
            directory.join(format!("materials/{property}.vmt")),
            format!("LightmappedGeneric {{ $surfaceprop {property} }}"),
        )
        .unwrap();
    }
    let mut fs = common::empty_game();
    fs.base_dir = root;
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.into()),
    });
    fs
}

fn fixture() -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("mass fixture", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(80.0, 80.0, 16.0),
                DVec3::new(112.0, 112.0, 48.0),
                "bulk",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[id],
                "func_physbox",
                &[
                    ("origin".into(), "96 96 32".into()),
                    ("massScale".into(), "3".into()),
                    ("overridescript".into(), "mass,123".into()),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn compile(source: &Vmf, fs: &GameFs) -> Bsp {
    let result = compiler::compile_with_options(
        source,
        fs,
        Options {
            physics: PhysicsMode::Engine,
            ..Default::default()
        },
    );
    assert!(!result.report.has_errors(), "{:?}", result.report);
    assert!(
        result
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("base mass is"))
    );
    result.bsp.unwrap()
}

fn solid(bsp: &Bsp) -> BTreeMap<String, String> {
    let physics = bsp.physics().unwrap().unwrap();
    let text = std::str::from_utf8(physics.models[1].key_data)
        .unwrap()
        .trim_end_matches('\0');
    let nodes = source_formats::keyvalues::parse(text).unwrap();
    nodes[0]
        .children()
        .unwrap()
        .iter()
        .map(|node| (node.key.clone(), node.as_str().unwrap().to_owned()))
        .collect()
}

#[test]
fn native_base_masses_follow_inherited_density_shell_area_and_mixed_material_dominance() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native mass fixture");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let fs = assets(directory.path(), root);
    let source = fixture();
    let before = source.write(&Default::default());
    let bulk = compile(&source, &fs);
    let fields = solid(&bulk);
    assert_eq!(fields["surfaceprop"], "bulk");
    assert!((fields["volume"].parse::<f64>().unwrap() - 32768.0).abs() < 0.1);
    assert!((fields["mass"].parse::<f64>().unwrap() - 536.971313152).abs() < 0.001);
    let entities =
        std::str::from_utf8(&bulk.lumps[source_formats::bsp::lump::ENTITIES].data).unwrap();
    assert!(
        entities.contains("\"massScale\" \"3\"")
            && entities.contains("\"overridescript\" \"mass,123\"")
    );
    std::fs::write(
        directory.path().join("scripts/mod.txt"),
        "bulk { density 2000 }",
    )
    .unwrap();
    let heavier = compile(&source, &fs);
    assert!((solid(&heavier)["mass"].parse::<f64>().unwrap() - 1073.942626304).abs() < 0.001);
    let bulk_physics = bulk.physics().unwrap().unwrap();
    let heavier_physics = heavier.physics().unwrap().unwrap();
    assert_eq!(
        bulk_physics.models[0].key_data,
        heavier_physics.models[0].key_data
    );
    for (a, b) in bulk_physics.models.iter().zip(&heavier_physics.models) {
        assert_eq!(a.solids, b.solids);
    }
    let mut shell_source = source.clone();
    let model = shell_source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("func_physbox"))
        .unwrap();
    for side in &mut model.solids[0].sides {
        side.material = "shell".into();
    }
    let shell = compile(&shell_source, &fs);
    let fields = solid(&shell);
    assert_eq!(fields["surfaceprop"], "shell");
    assert!((fields["mass"].parse::<f64>().unwrap() - 20.139701656).abs() < 0.0001);
    shell_source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("func_physbox"))
        .unwrap()
        .solids[0]
        .sides[0]
        .material = "bulk".into();
    let mixed = compile(&shell_source, &fs);
    assert_eq!(solid(&mixed), solid(&shell));
    assert_ne!(
        mixed.physics().unwrap().unwrap().models[1].solids,
        shell.physics().unwrap().unwrap().models[1].solids
    );
    std::fs::write(
        directory.path().join("scripts/mod.txt"),
        "bulk { density 1e30 }",
    )
    .unwrap();
    assert_eq!(
        solid(&compile(&source, &fs))["mass"]
            .parse::<f32>()
            .unwrap(),
        50_000.0
    );
    assert_eq!(source.write(&Default::default()), before);
}

#[test]
fn hidden_model_mass_ignores_omitted_liquid_and_nonsolid_brushes() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native mass fixture");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let fs = assets(directory.path(), root);
    std::fs::write(
        directory.path().join("materials/hidden_bulk.vmt"),
        "LightmappedGeneric { $surfaceprop bulk %compilenodraw 1 }",
    )
    .unwrap();
    for flag in ["%compilewater", "%compilenonsolid"] {
        std::fs::write(
            directory.path().join("materials/omitted.vmt"),
            format!(
                "LightmappedGeneric {{ $surfaceprop undefined_omitted %compilenodraw 1 {flag} 1 }}"
            ),
        )
        .unwrap();
        let mut document = Document::new(common::room());
        document
            .edit("hidden model", |tx| {
                let mut ids = Vec::new();
                for (x, material) in [(16.0, "omitted"), (80.0, "hidden_bulk")] {
                    let ObjectId::Solid(id) = edit::create_block(
                        tx,
                        DVec3::new(x, 80.0, 16.0),
                        DVec3::new(x + 32.0, 112.0, 48.0),
                        material,
                    )?
                    else {
                        unreachable!()
                    };
                    ids.push(id);
                }
                edit::tie_to_entity(tx, &ids, "func_physbox", &[])?;
                Ok(())
            })
            .unwrap();
        let bsp = compile(document.vmf(), &fs);
        let models = bsp.records::<source_formats::bsp::Model>().unwrap();
        assert_eq!(models[1].face_count, 0);
        let fields = solid(&bsp);
        assert_eq!(fields["surfaceprop"], "bulk");
        assert!((fields["volume"].parse::<f64>().unwrap() - 32768.0).abs() < 0.1);
        assert!((fields["mass"].parse::<f64>().unwrap() - 536.971313152).abs() < 0.001);
    }
}

#[test]
fn invalid_mass_assets_and_native_surface_aliases_preserve_the_previous_bsp() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native mass fixture");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let fs = assets(directory.path(), root);
    let source = fixture();
    let baseline = compile(&source, &fs).to_bytes();
    let target = directory.path().join("guarded.bsp");
    std::fs::write(&target, &baseline).unwrap();
    let manifest = directory
        .path()
        .join("scripts/surfaceproperties_manifest.txt");
    for (text, expected) in [
        ("bulk { density invalid }", "density"),
        ("bulk { density 0 }", "positive density"),
        ("bulk { density -1 }", "density"),
        ("bulk { density NaN }", "density"),
        ("bulk { thickness -1 }", "thickness"),
        ("bulk { base undefined }", "undefined base"),
    ] {
        std::fs::write(directory.path().join("scripts/mod.txt"), text).unwrap();
        let result = compiler::compile_with_options(
            &source,
            &fs,
            Options {
                physics: PhysicsMode::Engine,
                ..Default::default()
            },
        );
        assert!(result.report.has_errors() && !result.write(&target).unwrap());
        assert!(
            result
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{:?}",
            result.report
        );
        assert_eq!(std::fs::read(&target).unwrap(), baseline);
    }
    std::fs::write(
        directory.path().join("scripts/mod.txt"),
        "bulk { density 1000 }",
    )
    .unwrap();
    let material = directory.path().join("materials/bulk.vmt");
    for (contents, expected) in [
        (
            Some("LightmappedGeneric { $surfaceprop undefined_property }"),
            "undefined_property",
        ),
        (None, "missing material"),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&material, contents).unwrap();
        } else {
            std::fs::remove_file(&material).unwrap();
        }
        let result = compiler::compile_with_options(
            &source,
            &fs,
            Options {
                physics: PhysicsMode::Engine,
                ..Default::default()
            },
        );
        assert!(result.report.has_errors() && !result.write(&target).unwrap());
        assert!(
            result
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{:?}",
            result.report
        );
        assert_eq!(std::fs::read(&target).unwrap(), baseline);
    }
    std::fs::write(&material, "LightmappedGeneric { $surfaceprop bulk }").unwrap();
    std::fs::remove_file(&manifest).unwrap();
    let result = compiler::compile_with_options(
        &source,
        &fs,
        Options {
            physics: PhysicsMode::Engine,
            ..Default::default()
        },
    );
    assert!(result.report.has_errors() && !result.write(&target).unwrap());
    assert!(
        result
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("surfaceproperties_manifest"))
    );
    std::fs::write(
        &manifest,
        "surfaceproperties_manifest { file scripts/overflow.txt }",
    )
    .unwrap();
    let mut definitions = "default { density 1000 }".to_owned();
    for index in 0..128 {
        definitions.push_str(&format!(" p{index} {{ density 1000 }}"));
    }
    definitions.push_str(" bulk { density 1000 }");
    std::fs::write(directory.path().join("scripts/overflow.txt"), definitions).unwrap();
    let result = compiler::compile_with_options(
        &source,
        &fs,
        Options {
            physics: PhysicsMode::Engine,
            ..Default::default()
        },
    );
    assert!(result.report.has_errors() && !result.write(&target).unwrap());
    assert!(
        result
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("native surface index")),
        "{:?}",
        result.report
    );
    assert_eq!(std::fs::read(&target).unwrap(), baseline);
}
