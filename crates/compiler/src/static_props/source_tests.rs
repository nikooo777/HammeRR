use std::fs;

use source_formats::gamefs::{Mount, MountedPath};

use super::*;
use crate::asset_sources::{AssetSourceGuard, capture_output_asset_source_guard, hash};
use crate::{OutputLease, WriteReservation};

const MODEL: &str = "models/props/source.mdl";
const PHYSICS: &str = "models/props/source.phy";

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("models/props")).unwrap();
        let mut model = vec![0; 240];
        model[..4].copy_from_slice(b"IDST");
        for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
            model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, values) in [
            (104, [-16.0_f32, -8.0, 0.0]),
            (116, [16.0, 8.0, 32.0]),
            (128, [-16.0, -8.0, 0.0]),
            (140, [16.0, 8.0, 32.0]),
        ] {
            for (axis, value) in values.into_iter().enumerate() {
                model[offset + axis * 4..offset + axis * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        fs::write(directory.path().join(MODEL), model).unwrap();
        let mut physics = [16_i32, 0, 1, 0].map(i32::to_le_bytes).concat();
        physics.extend_from_slice(&32_i32.to_le_bytes());
        physics.extend_from_slice(b"VPHY");
        physics.extend_from_slice(&0x100_u16.to_le_bytes());
        physics.extend_from_slice(&0_u16.to_le_bytes());
        physics.extend_from_slice(&4_u32.to_le_bytes());
        physics.extend_from_slice(&[0; 20]);
        physics.extend_from_slice(b"solid {}\0");
        fs::write(directory.path().join(PHYSICS), physics).unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().into()),
        });
        Self { directory, game }
    }

    fn source(&self, solid: &str) -> Vmf {
        let mut source = crate::test_common::room();
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut prop = Entity::new(id, "prop_static");
        for (key, value) in [("model", MODEL), ("origin", "32 32 8"), ("solid", solid)] {
            prop.properties.set(key, value);
        }
        source.entities.push(prop);
        source
    }

    fn guard(&self, solid: &str) -> AssetSourceGuard {
        let mut bsp = crate::compile(&crate::test_common::room(), &self.game)
            .bsp
            .unwrap();
        let mut report = Report::default();
        let stats = bake(
            &self.source(solid),
            &self.game,
            &mut bsp,
            &mut report,
            &Control::default(),
            None,
        )
        .unwrap();
        assert!(!report.has_errors(), "{report:?}");
        stats.source_guard.unwrap()
    }

    fn mutate(&self, path: &str) {
        let file = self.directory.path().join(path);
        let mut data = fs::read(&file).unwrap();
        let index = if path == MODEL { 20 } else { data.len() - 2 };
        data[index] ^= 1;
        fs::write(file, data).unwrap();
    }
}

#[test]
fn actual_model_and_required_physics_consumers_return_original_guards() {
    for path in [MODEL, PHYSICS] {
        let fixture = Fixture::new();
        let before = crate::compile(&fixture.source("6"), &fixture.game)
            .bsp
            .unwrap()
            .to_bytes();
        let guard = fixture.guard("6");
        guard
            .verify_sources(&fixture.game, &Control::default())
            .unwrap();
        fixture.mutate(path);
        assert!(guard.verify_metadata().is_err(), "{path}");
        assert!(
            guard
                .verify_sources(&fixture.game, &Control::default())
                .is_err(),
            "{path}"
        );
        let after = crate::compile(&fixture.source("6"), &fixture.game)
            .bsp
            .unwrap()
            .to_bytes();
        assert_eq!(after, before);
    }
}

#[test]
fn unused_physics_payloads_do_not_enter_the_original_source_guard() {
    for solid in ["0", "2"] {
        let fixture = Fixture::new();
        let guard = fixture.guard(solid);
        fixture.mutate(PHYSICS);
        guard
            .verify_sources(&fixture.game, &Control::default())
            .unwrap();
    }
}

#[test]
fn required_physics_keeps_the_original_vpk_index_and_payload_guards() {
    for container in ["physics_dir.vpk", "physics_042.vpk"] {
        let mut fixture = Fixture::new();
        let physics = fs::read(fixture.directory.path().join(PHYSICS)).unwrap();
        let prefix = b"phy\0models/props\0source\0";
        let mut tree = prefix.to_vec();
        tree.extend_from_slice(&0_u32.to_le_bytes());
        tree.extend_from_slice(&0_u16.to_le_bytes());
        tree.extend_from_slice(&42_u16.to_le_bytes());
        tree.extend_from_slice(&0_u32.to_le_bytes());
        tree.extend_from_slice(&u32::try_from(physics.len()).unwrap().to_le_bytes());
        tree.extend_from_slice(&0xffff_u16.to_le_bytes());
        tree.extend_from_slice(b"\0\0\0");
        let mut index = 0x55aa_1234_u32.to_le_bytes().to_vec();
        index.extend_from_slice(&1_u32.to_le_bytes());
        index.extend_from_slice(&u32::try_from(tree.len()).unwrap().to_le_bytes());
        index.extend_from_slice(&tree);
        let index_path = fixture.directory.path().join("physics_dir.vpk");
        fs::write(&index_path, index).unwrap();
        fs::write(fixture.directory.path().join("physics_042.vpk"), physics).unwrap();
        fixture.game.mounts.insert(
            0,
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Pak(source_formats::vpk::Vpk::open(&index_path).unwrap()),
            },
        );
        let guard = fixture.guard("6");
        guard
            .verify_sources(&fixture.game, &Control::default())
            .unwrap();
        let path = fixture.directory.path().join(container);
        let mut data = fs::read(&path).unwrap();
        let position = if container == "physics_dir.vpk" {
            12 + prefix.len()
        } else {
            data.len() - 2
        };
        data[position] ^= 1;
        fs::write(path, data).unwrap();
        assert!(guard.verify_metadata().is_err(), "{container}");
        assert!(
            guard
                .verify_sources(&fixture.game, &Control::default())
                .is_err(),
            "{container}"
        );
    }
}

#[test]
fn original_model_winner_is_checked_even_when_both_payloads_are_identical() {
    let mut fixture = Fixture::new();
    let higher = fixture.directory.path().join("higher");
    fs::create_dir_all(higher.join("models/props")).unwrap();
    fs::copy(fixture.directory.path().join(MODEL), higher.join(MODEL)).unwrap();
    let guard = fixture.guard("2");
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher),
        },
    );
    guard.verify_metadata().unwrap();
    assert!(
        guard
            .verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
}

#[test]
fn guarded_static_props_preserve_prior_outputs_if_consumed_sources_change() {
    for path in [MODEL, PHYSICS] {
        for after_preparation in [false, true] {
            let fixture = Fixture::new();
            let source = fixture.source("6");
            let original = source.clone();
            let destination = fixture.directory.path().join("previous.bsp");
            fs::write(&destination, b"previous BSP").unwrap();
            fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
            let control = Control::default();
            let lease = OutputLease::acquire(&destination, &control).unwrap();
            let reservation = WriteReservation::new(&destination, &control).unwrap();
            let output = crate::compile_controlled(&source, &fixture.game, &control);
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert!(output.static_prop_sources.is_some());
            if after_preparation {
                let prepared = output
                    .prepare_write_reserved(reservation, &control)
                    .unwrap()
                    .unwrap();
                fixture.mutate(path);
                assert!(prepared.commit_checked(&lease, &control).is_err());
            } else {
                fixture.mutate(path);
                assert!(
                    output
                        .prepare_write_reserved(reservation, &control)
                        .is_err()
                );
            }
            assert_eq!(source, original);
            assert_eq!(fs::read(&destination).unwrap(), b"previous BSP");
            assert_eq!(
                fs::read(destination.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
        }
    }
}

#[test]
fn original_static_prop_guard_is_reused_by_the_output_guard() {
    let mut fixture = Fixture::new();
    let higher = fixture.directory.path().join("higher");
    fs::create_dir_all(higher.join("models/props")).unwrap();
    fs::copy(fixture.directory.path().join(MODEL), higher.join(MODEL)).unwrap();
    let destination = fixture.directory.path().join("outputs/missing-guard.bsp");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, b"previous BSP").unwrap();
    let mut output = crate::compile(&fixture.source("2"), &fixture.game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let guard =
        capture_output_asset_source_guard(&output, &fixture.game, &Control::default()).unwrap();
    guard
        .static_prop_guard()
        .unwrap()
        .verify_sources(&fixture.game, &Control::default())
        .unwrap();
    let original = output.static_prop_sources.take().unwrap();
    assert!(
        capture_output_asset_source_guard(&output, &fixture.game, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("missing its original")
    );
    let control = Control::default();
    let reservation = crate::WriteReservation::new(&destination, &control).unwrap();
    assert!(
        output
            .prepare_write_reserved(reservation, &control)
            .unwrap_err()
            .to_string()
            .contains("missing its original")
    );
    assert_eq!(fs::read(&destination).unwrap(), b"previous BSP");
    output.static_prop_sources = Some(original);
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher),
        },
    );
    guard.verify_metadata().unwrap();
    assert!(
        guard
            .static_prop_guard()
            .unwrap()
            .verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
    assert!(
        capture_output_asset_source_guard(&output, &fixture.game, &Control::default()).is_err()
    );
}

#[test]
fn optional_prop_lighting_keeps_its_original_model_read_receipt() {
    let fixture = Fixture::new();
    let source = fixture.source("6");
    let mut reads = crate::lighting::asset_tracking::SourceReads::default();
    let mut bsp = crate::compile(&crate::test_common::room(), &fixture.game)
        .bsp
        .unwrap();
    let mut report = Report::default();
    let stats = bake(
        &source,
        &fixture.game,
        &mut bsp,
        &mut report,
        &Control::default(),
        Some(&mut reads),
    )
    .unwrap();
    assert!(!report.has_errors(), "{report:?}");
    stats
        .source_guard
        .unwrap()
        .verify_sources(&fixture.game, &Control::default())
        .unwrap();
    let (sources, missing) = reads.into_parts();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, MODEL);
    let bytes = fs::read(fixture.directory.path().join(MODEL)).unwrap();
    assert_eq!(sources[0].bytes, bytes.len());
    assert_eq!(sources[0].sha256, hash(&bytes));
    assert!(missing.is_empty());

    let mut reads = crate::lighting::asset_tracking::SourceReads::default();
    reads.record(MODEL, Some(&bytes)).unwrap();
    fixture.mutate(MODEL);
    let mut report = Report::default();
    let stats = bake(
        &source,
        &fixture.game,
        &mut bsp,
        &mut report,
        &Control::default(),
        Some(&mut reads),
    )
    .unwrap();
    assert!(report.has_errors());
    assert!(stats.source_guard.is_none());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("changed during baking"))
    );
}

#[test]
fn hidden_and_region_omitted_models_remain_unread_without_a_source_guard() {
    for region in [false, true] {
        let fixture = Fixture::new();
        let mut source = fixture.source("6");
        if region {
            source
                .entities
                .last_mut()
                .unwrap()
                .properties
                .set("origin", "192 32 8");
            fs::create_dir_all(fixture.directory.path().join("materials/tools")).unwrap();
            fs::write(
                fixture
                    .directory
                    .path()
                    .join("materials/tools/toolsblack.vmt"),
                "LightmappedGeneric {}",
            )
            .unwrap();
            let mut document = map_core::Document::new(source);
            document
                .edit("region", |transaction| {
                    map_core::compile_region::apply(
                        transaction,
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
            source = document.into_vmf();
        } else {
            source.entities.last_mut().unwrap().hidden = true;
        }
        let baseline = crate::compile(&source, &fixture.game);
        assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
        assert!(baseline.static_prop_sources.is_none());
        fs::write(
            fixture.directory.path().join(MODEL),
            b"unused invalid model",
        )
        .unwrap();
        let output = crate::compile(&source, &fixture.game);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.static_prop_sources.is_none());
        assert_eq!(output.stats.static_props, 0);
        assert_eq!(output.bsp.unwrap(), baseline.bsp.unwrap());
    }
}
