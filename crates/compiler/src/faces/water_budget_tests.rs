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
        ("side", "Water { $abovewater 1 $bottommaterial below }"),
        (
            "slime",
            "LightmappedGeneric { %compileslime 1 $bottommaterial slime }",
        ),
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
fn slime_inward_faces_share_generated_geometry_limits_and_cancellation() {
    let (_directory, game, mut source) = crossed_partitions();
    for solid in &mut source.world.solids {
        for side in &mut solid.sides {
            if side.material == "water" {
                side.material = "slime".into();
            }
        }
    }
    let original = source.clone();
    assert!(successful(&source, &game).stats.water_regions > 0);
    for config in [
        TestConfig {
            limits: Limits {
                fragments: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        TestConfig {
            cancel_after_visits: Some(1),
            ..Default::default()
        },
    ] {
        let output = with_config(config, || crate::compile(&source, &game));
        assert!(output.bsp.is_none());
        assert!(output.report.has_errors());
        assert_eq!(source, original);
    }
}

#[test]
fn crossed_water_partitions_reject_each_injected_budget_and_preserve_publication() {
    let (directory, game, source) = crossed_partitions();
    let original = source.clone();
    let path = directory.path().join("previous.bsp");
    let publish_control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &publish_control).unwrap();
    let reservation = crate::WriteReservation::new(&path, &publish_control).unwrap();
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
    let origins = baseline.face_origins.as_ref().unwrap();
    for (index, face) in bsp.records::<Face>().unwrap().iter().enumerate() {
        if names[data[infos[face.texinfo as usize].texdata as usize].name_index as usize] == "BELOW"
        {
            assert_eq!(origins.contributors(index).unwrap().len(), 0);
        }
    }

    assert!(
        baseline
            .prepare_write_reserved(reservation, &publish_control)
            .unwrap()
            .unwrap()
            .commit_checked(&lease, &publish_control)
            .unwrap()
    );
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
                diagnostic.message.contains("Liquid inward faces")
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
    let path = directory.path().join("previous.bsp");
    let publish_control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &publish_control).unwrap();
    let reservation = crate::WriteReservation::new(&path, &publish_control).unwrap();
    let baseline = successful(&source, &game);
    assert!(
        baseline
            .prepare_write_reserved(reservation, &publish_control)
            .unwrap()
            .unwrap()
            .commit_checked(&lease, &publish_control)
            .unwrap()
    );
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

#[test]
fn vertical_only_generated_water_is_budgeted_and_cancellable_before_publication() {
    let (directory, game, mut source) = crossed_partitions();
    std::fs::write(
        directory.path().join("materials/water.vmt"),
        "Water { $abovewater 1 }",
    )
    .unwrap();
    let pool = source
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.sides.iter().any(|side| side.material == "water"))
        .unwrap();
    for side in &mut pool.sides {
        if Plane::from_points(side.plane.0).unwrap().normal == DVec3::NEG_Y {
            side.material = "side".into();
        }
    }
    let original = source.clone();
    let path = directory.path().join("vertical-previous.bsp");
    let publish_control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &publish_control).unwrap();
    let reservation = crate::WriteReservation::new(&path, &publish_control).unwrap();
    let baseline = successful(&source, &game);
    assert!(
        baseline
            .prepare_write_reserved(reservation, &publish_control)
            .unwrap()
            .unwrap()
            .commit_checked(&lease, &publish_control)
            .unwrap()
    );
    let previous = std::fs::read(&path).unwrap();
    for config in [
        TestConfig {
            limits: Limits {
                fragments: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        TestConfig {
            cancel_after_visits: Some(3),
            ..Default::default()
        },
    ] {
        let control = Control::default();
        let output = with_config(config, || {
            crate::compile_controlled(&source, &game, &control)
        });
        assert!(output.bsp.is_none(), "{:?}", output.report);
        assert_eq!(control.progress().stage, Stage::Faces);
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert_eq!(source, original);
    }
}

#[test]
fn authored_bottom_only_generation_obeys_all_budgets_and_cancellation_before_publication() {
    let (directory, game, source) = crossed_partitions();
    std::fs::write(
        directory.path().join("materials/water.vmt"),
        "Water { $abovewater 1 }",
    )
    .unwrap();
    let pool = source
        .world
        .solids
        .iter()
        .find(|solid| solid.sides.iter().any(|side| side.material == "water"))
        .unwrap()
        .id;
    let mut document = Document::new(source);
    document
        .edit("exposed bottom-only water", |tx| {
            edit::transform(
                tx,
                &[ObjectId::Solid(pool)],
                edit::Transform {
                    translation: DVec3::new(0.0, 0.0, 32.0),
                    ..Default::default()
                },
            )?;
            for side in &mut tx.solid_mut(pool)?.sides {
                if Plane::from_points(side.plane.0).unwrap().normal == DVec3::NEG_Z {
                    side.material = "side".into();
                }
            }
            Ok(())
        })
        .unwrap();
    let source = document.into_vmf();
    let original = source.clone();
    let path = directory.path().join("bottom-previous.bsp");
    let publish_control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &publish_control).unwrap();
    let reservation = crate::WriteReservation::new(&path, &publish_control).unwrap();
    let baseline = successful(&source, &game);
    let bsp = baseline.bsp.as_ref().unwrap();
    let names = bsp.texdata_strings().unwrap();
    let data = bsp.records::<bsp::TexData>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let generated = bsp
        .records::<Face>()
        .unwrap()
        .into_iter()
        .filter(|face| {
            names[data[infos[face.texinfo as usize].texdata as usize].name_index as usize]
                == "BELOW"
        })
        .collect::<Vec<_>>();
    assert!(!generated.is_empty());
    for face in generated {
        let plane = planes[face.plane as usize];
        assert_eq!(plane.normal, [0.0, 0.0, 1.0]);
        assert_eq!(plane.dist, 32.0);
        assert!(face.fog_volume >= 0);
    }
    assert!(
        baseline
            .prepare_write_reserved(reservation, &publish_control)
            .unwrap()
            .unwrap()
            .commit_checked(&lease, &publish_control)
            .unwrap()
    );
    let previous = std::fs::read(&path).unwrap();
    let pointfile = path.with_extension("lin");
    let previous_pointfile = b"retained pointfile\n";
    std::fs::write(&pointfile, previous_pointfile).unwrap();
    for (config, expected) in [
        (
            TestConfig {
                limits: Limits {
                    work: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            Some("traversal work"),
        ),
        (
            TestConfig {
                limits: Limits {
                    fragments: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            Some("generated fragment"),
        ),
        (
            TestConfig {
                limits: Limits {
                    points: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            Some("winding point allocation"),
        ),
        (
            TestConfig {
                cancel_after_visits: Some(3),
                ..Default::default()
            },
            None,
        ),
    ] {
        let control = Control::default();
        let output = with_config(config, || {
            crate::compile_controlled(&source, &game, &control)
        });
        assert!(output.bsp.is_none(), "{:?}", output.report);
        assert_eq!(control.progress().stage, Stage::Faces);
        assert_eq!(control.cancelled(), expected.is_none());
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                expected.map_or(diagnostic.message == "Compilation cancelled", |expected| {
                    diagnostic.message.contains("Liquid inward faces")
                        && diagnostic.message.contains(expected)
                })
            }),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert_eq!(std::fs::read(&pointfile).unwrap(), previous_pointfile);
        assert_eq!(source, original);
    }
}
