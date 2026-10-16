mod common;

use std::path::{Path, PathBuf};

use compiler::{Options, PhysicsMode};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::{
    bsp::{Bsp, lump},
    gamefs::{GameFs, Mount, MountedPath},
    vmf::Vmf,
};

fn assets(directory: &Path) -> GameFs {
    std::fs::create_dir_all(directory.join("materials/dev")).unwrap();
    std::fs::create_dir(directory.join("scripts")).unwrap();
    std::fs::write(
        directory.join("materials/dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $surfaceprop default }",
    )
    .unwrap();
    std::fs::write(
        directory.join("scripts/surfaceproperties_manifest.txt"),
        "surfaceproperties_manifest { file scripts/blend_surfaces.txt }",
    )
    .unwrap();
    std::fs::write(
        directory.join("scripts/blend_surfaces.txt"),
        "default { density 2000 } ice { density 917 friction 0.1 } rubber { density 1500 friction 0.8 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.base_dir = directory.into();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.into()),
    });
    game
}

fn material(directory: &Path, secondary: Option<&str>) {
    let secondary = secondary.map_or(String::new(), |value| format!(" $surfaceprop2 \"{value}\""));
    std::fs::write(
        directory.join("materials/blend.vmt"),
        format!("WorldVertexTransition {{ $surfaceprop ice{secondary} }}"),
    )
    .unwrap();
}

fn fixture(power: u32, alpha: f64) -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("terrain blend physics", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-112.0, -112.0, 16.0),
                DVec3::new(112.0, 112.0, 32.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let solid = tx.document().solid(id).unwrap();
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| face.plane.normal.z > 0.99)
                .unwrap();
            let target = map_core::surface::FaceId {
                solid: id,
                side: solid.sides[face.side].id,
            };
            map_core::displacement::edit::create(tx, &[target], power, power % 4)?;
            let side = tx
                .solid_mut(id)?
                .sides
                .iter_mut()
                .find(|side| side.id == target.side)
                .unwrap();
            side.material = "blend".into();
            let size = (1 << power) + 1;
            side.displacement.as_mut().unwrap().alphas = Some(vec![vec![alpha; size]; size]);
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn physics_options() -> Options {
    Options {
        physics: PhysicsMode::Engine,
        ..Options::default()
    }
}

#[test]
fn explicit_invalid_secondary_properties_fail_before_native_work_and_preserve_output() {
    let directory = tempfile::tempdir().unwrap();
    let game = assets(directory.path());
    let source = fixture(2, 0.0);
    let before = source.write(&Default::default());
    let guarded = directory.path().join("previous.bsp");
    std::fs::write(&guarded, b"previous BSP").unwrap();
    material(directory.path(), None);
    let unchanged = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    for (secondary, expected) in [
        ("", "1–127 ASCII"),
        ("bad/name", "1–127 ASCII"),
        ("bad\nname", "1–127 ASCII"),
        (
            "unknown_secondary",
            "not defined by the mounted surface manifest",
        ),
    ] {
        material(directory.path(), Some(secondary));
        let output = compiler::compile_with_options(&source, &game, physics_options());
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("Terrain secondary physics surface property")
                    && diagnostic.message.contains(expected)
            }),
            "{:?}",
            output.report
        );
        assert!(!output.write(&guarded).unwrap());
        assert_eq!(std::fs::read(&guarded).unwrap(), b"previous BSP");
        assert_eq!(source.write(&Default::default()), before);
        assert_eq!(
            compiler::compile(&source, &game).bsp.unwrap().to_bytes(),
            unchanged
        );
    }
    material(directory.path(), Some(&"a".repeat(128)));
    let output = compiler::compile_with_options(&source, &game, physics_options());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("secondary physics surface property")
                && diagnostic.message.contains("1–127 ASCII")
        }),
        "{:?}",
        output.report
    );
    material(directory.path(), Some("rubber"));
    std::fs::remove_file(
        directory
            .path()
            .join("scripts/surfaceproperties_manifest.txt"),
    )
    .unwrap();
    let output = compiler::compile_with_options(&source, &game, physics_options());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains(
                "Terrain secondary physics surface properties require mounted surface definitions",
            )
        }),
        "{:?}",
        output.report
    );
    assert!(output.bsp.is_none() && !output.write(&guarded).unwrap());
    assert_eq!(std::fs::read(&guarded).unwrap(), b"previous BSP");
    material(directory.path(), None);
    let output = compiler::compile_with_options(&source, &game, physics_options());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("requires the installed Linux64 CS:S vphysics.so")
        }),
        "{:?}",
        output.report
    );
}

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

fn compile(source: &Vmf, game: &GameFs) -> Bsp {
    let output = compiler::compile_with_options(source, game, physics_options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.physics_models, 1);
    assert_eq!(output.stats.physics_solids, 2);
    assert_eq!(
        output.stats.physics_mesh_triangles,
        output.stats.displacement_triangles
    );
    output.bsp.unwrap()
}

#[test]
fn engine_blended_terrain_reloads_both_materials_and_preserves_geometry_and_saved_alpha() {
    let Some(root) = installed_game() else {
        eprintln!("CS:S not installed; skipping native terrain blend fixture");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let mut game = assets(directory.path());
    game.base_dir = root;
    for power in 2..=4 {
        material(directory.path(), Some("rubber"));
        let primary_source = fixture(power, 0.0);
        let primary = compile(&primary_source, &game);
        for alpha in [-255.0, 127.5, 127.500_000_1] {
            let boundary = compile(&fixture(power, alpha), &game);
            assert_eq!(
                boundary.lump(lump::PHYSCOLLIDE).data,
                primary.lump(lump::PHYSCOLLIDE).data
            );
        }
        let secondary_source = fixture(power, 255.0);
        let before = secondary_source.write(&Default::default());
        let secondary = compile(&secondary_source, &game);
        let above = compile(&fixture(power, 127.500_01), &game);
        assert_eq!(
            above.lump(lump::PHYSCOLLIDE).data,
            secondary.lump(lump::PHYSCOLLIDE).data
        );
        assert_ne!(
            primary.lump(lump::PHYSCOLLIDE).data,
            secondary.lump(lump::PHYSCOLLIDE).data
        );
        assert_eq!(
            compile(&fixture(power, 300.0), &game)
                .lump(lump::PHYSCOLLIDE)
                .data,
            secondary.lump(lump::PHYSCOLLIDE).data
        );
        let mut mixed_source = secondary_source.clone();
        let alphas = mixed_source
            .world
            .solids
            .iter_mut()
            .flat_map(|solid| &mut solid.sides)
            .find_map(|side| side.displacement.as_mut())
            .unwrap()
            .alphas
            .as_mut()
            .unwrap();
        for row in alphas {
            let middle = row.len() / 2;
            row[middle..].fill(0.0);
        }
        let mixed_before = mixed_source.write(&Default::default());
        let mixed = compile(&mixed_source, &game);
        assert_ne!(
            mixed.lump(lump::PHYSCOLLIDE).data,
            primary.lump(lump::PHYSCOLLIDE).data
        );
        assert_ne!(
            mixed.lump(lump::PHYSCOLLIDE).data,
            secondary.lump(lump::PHYSCOLLIDE).data
        );
        let reopened = Vmf::parse(&mixed_before).unwrap();
        assert_eq!(compile(&reopened, &game).to_bytes(), mixed.to_bytes());
        assert_eq!(mixed_source.write(&Default::default()), mixed_before);
        let physics = secondary.physics().unwrap().unwrap();
        let keys = std::str::from_utf8(physics.models[0].key_data).unwrap();
        assert!(keys.contains("\"ice\" \"2\""));
        assert!(keys.contains("\"rubber\" \"3\""));
        for index in 0..64 {
            if [lump::DISP_VERTS, lump::PHYSCOLLIDE].contains(&index) {
                continue;
            }
            assert_eq!(
                secondary.lump(index).data,
                primary.lump(index).data,
                "Unrelated lump {index} changed"
            );
            assert_eq!(
                mixed.lump(index).data,
                primary.lump(index).data,
                "Unrelated mixed lump {index} changed"
            );
        }
        let reopened = Vmf::parse(&before).unwrap();
        assert_eq!(compile(&reopened, &game).to_bytes(), secondary.to_bytes());
        assert_eq!(secondary_source.write(&Default::default()), before);
        material(directory.path(), None);
        assert_eq!(
            compile(&secondary_source, &game)
                .lump(lump::PHYSCOLLIDE)
                .data,
            primary.lump(lump::PHYSCOLLIDE).data
        );
    }
}
