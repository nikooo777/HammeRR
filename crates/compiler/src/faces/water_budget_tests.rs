use super::water_budget::{Budget, Limits, TestConfig, with_config};
use crate::{Control, Output, Stage};
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Face, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

fn crossed_partitions() -> (tempfile::TempDir, GameFs, Vmf) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    for (name, text) in [
        ("water", "Water { $abovewater 1 $bottommaterial below }"),
        ("below", "Water { $abovewater 0 }"),
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
    ] {
        std::fs::write(materials.join(format!("{name}.vmt")), text).unwrap();
    }
    let mut game = crate::test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut document = Document::new(crate::test_common::room());
    document
        .edit("crossed water partitions", |tx| {
            let ObjectId::Solid(pool) = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(pool)?.sides {
                if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = "water".into();
                }
            }
            for start in [-96.0, -48.0, 0.0, 48.0] {
                edit::create_block(
                    tx,
                    DVec3::new(start, -128.0, 64.0),
                    DVec3::new(start + 8.0, 128.0, 96.0),
                    "dev/dev_measuregeneric01",
                )?;
                edit::create_block(
                    tx,
                    DVec3::new(-128.0, start, 0.0),
                    DVec3::new(128.0, start + 8.0, 64.0),
                    "dev/dev_measuregeneric01",
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut source = document.vmf().clone();
    for entity in &mut source.entities {
        entity.properties.set("origin", "-192 -192 8");
    }
    (directory, game, source)
}

fn successful(vmf: &Vmf, game: &GameFs) -> Output {
    let output = crate::compile(vmf, game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert!(output.bsp.is_some());
    output
}

#[test]
fn crossed_water_partitions_reject_each_injected_budget_and_preserve_publication() {
    let (directory, game, source) = crossed_partitions();
    let original = source.clone();
    let baseline = successful(&source, &game);
    let bsp = baseline.bsp.as_ref().unwrap();
    let names = bsp.texdata_strings().unwrap();
    let data = bsp.records::<bsp::TexData>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let bottoms = bsp
        .records::<Face>()
        .unwrap()
        .iter()
        .filter(|face| {
            names[data[infos[face.texinfo as usize].texdata as usize].name_index as usize]
                == "BELOW"
        })
        .count();
    assert!(bottoms >= 16);
    let path = directory.path().join("previous.bsp");
    assert!(baseline.write(&path).unwrap());
    let previous = std::fs::read(&path).unwrap();
    for (limits, expected) in [
        (
            Limits {
                work: 16,
                ..Default::default()
            },
            "traversal work",
        ),
        (
            Limits {
                fragments: 8,
                ..Default::default()
            },
            "generated fragment",
        ),
        (
            Limits {
                points: 24,
                ..Default::default()
            },
            "winding point allocation",
        ),
    ] {
        let output = with_config(
            TestConfig {
                limits,
                ..Default::default()
            },
            || crate::compile(&source, &game),
        );
        assert!(output.bsp.is_none());
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("Water undersides")
                    && diagnostic.message.contains(expected)
            }),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert_eq!(source, original);
    }
}

#[test]
fn generated_water_cancellation_interrupts_active_visits_and_preserves_previous_output() {
    let (directory, game, source) = crossed_partitions();
    let baseline = successful(&source, &game);
    let path = directory.path().join("previous.bsp");
    assert!(baseline.write(&path).unwrap());
    let previous = std::fs::read(&path).unwrap();
    let original = source.clone();
    let control = Control::default();
    let output = with_config(
        TestConfig {
            cancel_after_visits: Some(3),
            ..Default::default()
        },
        || crate::compile_controlled(&source, &game, &control),
    );
    assert!(control.cancelled());
    assert_eq!(control.progress().stage, Stage::Faces);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "Compilation cancelled")
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert_eq!(source, original);
}

#[test]
fn no_bottom_material_bypasses_zero_generated_water_limits_without_changing_bytes() {
    let (directory, game, source) = crossed_partitions();
    std::fs::write(
        directory.path().join("materials/water.vmt"),
        "Water { $abovewater 1 }",
    )
    .unwrap();
    let baseline = successful(&source, &game).bsp.unwrap().to_bytes();
    let output = with_config(
        TestConfig {
            limits: Limits {
                work: 0,
                fragments: 0,
                points: 0,
            },
            cancel_after_visits: Some(1),
        },
        || successful(&source, &game),
    );
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
}

#[test]
fn reservations_fail_before_mutating_the_water_ledger() {
    let control = Control::default();
    let mut budget = with_config(
        TestConfig {
            limits: Limits {
                points: 4,
                ..Default::default()
            },
            ..Default::default()
        },
        Budget::default,
    );
    budget.polygon(4, &control).unwrap();
    let before = budget.usage();
    assert!(
        budget
            .split(4, &control)
            .unwrap_err()
            .contains("winding point allocation")
    );
    assert_eq!(budget.usage(), before);
    control.cancel().unwrap();
    assert_eq!(
        budget.polygon(4, &control).unwrap_err(),
        "Compilation cancelled"
    );
    assert_eq!(budget.usage(), before);
}
