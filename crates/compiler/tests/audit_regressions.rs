mod common;

use compiler::map::{MAX_COMPILE_COORD, Map};
use glam::DVec3;
use map_core::{Document, edit};
use source_formats::bsp::{Brush, Face, TexInfo, contents, surf};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmt::Material;

fn material_game(parameters: &str) -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/audit.vmt"),
        format!("LightmappedGeneric {{ {parameters} }}"),
    )
    .unwrap();
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, fs)
}

#[test]
fn skip_on_any_floor_face_preserves_the_sealed_room() {
    let (_directory, fs) = material_game("%compileskip true");
    for side in 0..6 {
        let mut room = common::room();
        room.world.solids[0].sides[side].material = "audit".into();
        let output = compiler::compile(&room, &fs);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 6);
        let bsp = output.bsp.unwrap();
        assert!(
            bsp.records::<Brush>()
                .unwrap()
                .iter()
                .all(|brush| brush.contents == contents::SOLID)
        );
        let texinfos = bsp.records::<TexInfo>().unwrap();
        assert!(
            bsp.records::<Face>()
                .unwrap()
                .iter()
                .all(|face| texinfos[face.texinfo as usize].flags & surf::SKIP == 0)
        );
    }
}

#[test]
fn nonsolid_geometry_survives_with_opaque_contents_and_visible_faces() {
    let (_directory, fs) = material_game("%compilenonsolid 1");
    for mixed in [false, true] {
        let mut document = Document::new(common::room());
        document
            .edit("non-solid fixture", |tx| {
                let id = edit::create_block(tx, DVec3::ZERO, DVec3::splat(32.0), "audit")?;
                if mixed {
                    let map_core::ObjectId::Solid(id) = id else {
                        unreachable!()
                    };
                    tx.solid_mut(id)?.sides[0].material = "dev/dev_measuregeneric01".into();
                }
                Ok(())
            })
            .unwrap();
        let output = compiler::compile(document.vmf(), &fs);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(
            bsp.records::<Brush>()
                .unwrap()
                .iter()
                .any(|brush| brush.contents == contents::OPAQUE)
        );
        let texinfos = bsp.records::<TexInfo>().unwrap();
        let names = bsp.texdata_strings().unwrap();
        assert!(bsp.records::<Face>().unwrap().iter().any(|face| {
            let texinfo = &texinfos[face.texinfo as usize];
            names[texinfo.texdata as usize] == "AUDIT" && texinfo.flags & surf::NODRAW == 0
        }));
    }
}

#[test]
fn compile_booleans_agree_between_editor_and_compiler() {
    for (value, hidden) in [
        ("true", true),
        ("TRUE", true),
        ("1", true),
        ("false", false),
        ("0", false),
        ("2", false),
    ] {
        let params = format!("%compilenodraw {value}");
        let material = Material::parse(&format!("LightmappedGeneric {{ {params} }}")).unwrap();
        assert_eq!(material.is_tool(), hidden, "{value}");
        let (_directory, fs) = material_game(&params);
        let mut room = common::room();
        room.world.solids[0].sides[0].material = "audit".into();
        let mut report = compiler::Report::default();
        let map = Map::load(&room, &fs, &mut report).unwrap();
        let info = map
            .materials
            .iter()
            .find(|info| info.name == "AUDIT")
            .unwrap();
        assert_eq!(!info.draws, hidden, "{value}");
        assert_eq!(info.surface & surf::NODRAW != 0, hidden, "{value}");
        assert_eq!(info.contents, contents::SOLID);
    }
}

#[test]
fn mixed_detail_faces_keep_solid_contents_and_mixed_fog_faces_report_the_limit() {
    for (params, expected, warning) in [
        (
            "%compiledetail 1",
            contents::SOLID | contents::DETAIL,
            false,
        ),
        ("%compilefog true", contents::SOLID, true),
    ] {
        let (_directory, fs) = material_game(params);
        let mut room = common::room();
        room.world.solids[0].sides[0].material = "audit".into();
        let mut report = compiler::Report::default();
        let map = Map::load(&room, &fs, &mut report).unwrap();
        assert_eq!(map.brushes[0].contents, expected);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("%compilefog")),
            warning
        );
    }
}

#[test]
fn unsupported_flags_and_misplaced_areaportals_are_reported() {
    for (params, message) in [
        ("%compilefuture true", "%compilefuture"),
        (
            "%compileareaportal 1",
            "areaportal material requires func_areaportal",
        ),
        ("%compilefog 1", "skip/fog"),
    ] {
        let (_directory, fs) = material_game(params);
        let mut room = common::room();
        for side in &mut room.world.solids[0].sides {
            side.material = "audit".into();
        }
        let mut report = compiler::Report::default();
        Map::load(&room, &fs, &mut report).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(message)
                    && diagnostic.object == Some(format!("solid:{}", room.world.solids[0].id))),
            "{report:?}"
        );
    }
}

#[test]
fn compiler_rejects_out_of_range_geometry_and_origins_before_tree_building() {
    let fs = common::empty_game();
    for x in [
        70_000.0,
        65_536.0,
        65_520.0,
        MAX_COMPILE_COORD,
        -MAX_COMPILE_COORD - 16.0,
    ] {
        let mut document = Document::new(common::room());
        let id = document
            .edit("outside source bounds", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(x, 0.0, 0.0),
                    DVec3::new(x + 8.0, 8.0, 8.0),
                    "audit",
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        let output = compiler::compile(document.vmf(), &fs);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("±16384")
                    && diagnostic.object.as_deref()
                        == Some(id.to_string().replace(' ', ":").as_str())),
            "{:?}",
            output.report
        );
        assert_eq!(document.vmf(), &before);
    }
    for origin in ["NaN 0 0", "70000 0 0", "invalid"] {
        let mut room = common::room();
        room.entities[0].properties.set("origin", origin);
        let output = compiler::compile(&room, &fs);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("±16384")
                    && diagnostic.object == Some(format!("entity:{}", room.entities[0].id)))
        );
    }
    for x in [-MAX_COMPILE_COORD, MAX_COMPILE_COORD - 16.0] {
        let mut document = Document::empty();
        document
            .edit("at source bounds", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(x, 0.0, 0.0),
                    DVec3::new(x + 16.0, 16.0, 16.0),
                    "audit",
                )
            })
            .unwrap();
        let mut report = compiler::Report::default();
        let map = Map::load(document.vmf(), &fs, &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(map.brushes.len(), 1);
    }
}

#[test]
fn compiler_reports_duplicate_imported_ids_before_building() {
    let mut room = common::room();
    room.entities[0].id = room.world.solids[0].id;
    let output = compiler::compile(&room, &common::empty_game());
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("duplicate object ID"))
    );
}
