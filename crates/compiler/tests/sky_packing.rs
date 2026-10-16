mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use compiler::{Control, Options, OutputLease, WriteReservation};
use source_formats::bsp::{self, Bsp, PakLimits};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    vmf: Vmf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let materials = directory.path().join("materials/skybox");
        fs::create_dir_all(&materials).unwrap();
        for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
            fs::write(
                materials.join(format!("portable{suffix}.vmt")),
                "UnlitGeneric { $basetexture skybox/portable_shared }",
            )
            .unwrap();
        }
        let mut texture = vec![0; 84];
        texture[..4].copy_from_slice(b"VTF\0");
        for (offset, value) in [(4, 7u32), (8, 2), (12, 80), (57, u32::MAX)] {
            texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for offset in [16, 18, 24, 63] {
            texture[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
        }
        texture[56] = 1;
        texture[80..].copy_from_slice(&[31, 63, 127, 255]);
        fs::write(materials.join("portable_shared.vtf"), texture).unwrap();
        fs::write(
            directory.path().join("gameinfo.txt"),
            "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
        )
        .unwrap();
        let game = GameFs::from_gameinfo(directory.path().join("gameinfo.txt")).unwrap();
        let mut vmf = common::room();
        vmf.world.properties.set("skyname", "portable");
        Self {
            directory,
            game,
            vmf,
        }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.directory.path().join(name)
    }

    fn cli(&self, output: &Path) -> Command {
        let source = self.path("source.vmf");
        fs::write(&source, self.vmf.write(&Default::default())).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_hammerr-compile"));
        command
            .arg(source)
            .arg("--game")
            .arg(self.path("gameinfo.txt"))
            .arg("--output")
            .arg(output)
            .arg("--pack-sky-assets")
            .arg("--reflection-engine-root")
            .arg(self.path("missing-native-runtime"));
        command
    }
}

fn packed_options() -> Options {
    Options {
        pack_sky_assets: true,
        ..Default::default()
    }
}

#[test]
fn fullbright_packs_without_a_capture_context_and_only_changes_the_pak() {
    let fixture = Fixture::new();
    let source = fixture.vmf.clone();
    let baseline = compiler::compile(&source, &fixture.game);
    let output = compiler::compile_with_options(&source, &fixture.game, packed_options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.cubemap_assets.is_none());
    let plan = output.sky_assets.as_ref().unwrap();
    let stats = output.stats.sky_assets.as_ref().unwrap();
    assert_eq!(stats.family, "portable");
    assert_eq!(stats.channels, "ldr");
    assert_eq!(stats.files, 7);
    assert_eq!(stats.asset_bytes, plan.asset_bytes());
    let bsp = output.bsp.as_ref().unwrap();
    assert_eq!(
        compiler::read_packed_sky(bsp, &Control::default()).unwrap(),
        Some(plan.manifest())
    );
    for index in 0..bsp::LUMP_COUNT {
        if index != bsp::lump::PAKFILE {
            assert_eq!(
                bsp.lumps[index],
                baseline.bsp.as_ref().unwrap().lumps[index]
            );
        }
    }
    assert_eq!(fixture.vmf, source);
    assert!(output.write(&fixture.path("portable.bsp")).unwrap());
}

#[test]
fn default_off_keeps_missing_sky_output_and_statistics_unchanged() {
    let game = common::empty_game();
    let mut source = common::room();
    source.world.properties.set("skyname", "absent");
    let baseline = compiler::compile(&source, &game);
    let explicit_off = compiler::compile_with_options(&source, &game, Options::default());
    assert!(!baseline.report.has_errors());
    assert!(baseline.sky_assets.is_none() && baseline.stats.sky_assets.is_none());
    assert_eq!(
        baseline.bsp.unwrap().to_bytes(),
        explicit_off.bsp.unwrap().to_bytes()
    );
    assert!(
        serde_json::to_value(explicit_off.stats)
            .unwrap()
            .get("sky_assets")
            .is_none()
    );
    let failed = compiler::compile_with_options(&source, &game, packed_options());
    assert!(failed.report.has_errors() && failed.bsp.is_none());
    assert!(failed.sky_assets.is_none());
}

#[test]
fn publication_rechecks_packed_bytes_before_touching_previous_output() {
    let fixture = Fixture::new();
    let mut output = compiler::compile_with_options(&fixture.vmf, &fixture.game, packed_options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.as_mut().unwrap();
    let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name.ends_with("portable_shared.vtf"))
        .unwrap()
        .data[80] ^= 1;
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    let path = fixture.path("unchanged.bsp");
    fs::write(&path, b"previous output").unwrap();
    assert!(
        output
            .prepare_write_controlled(&path, &Control::default())
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"previous output");
}

#[test]
fn reserved_write_preserves_collection_stamps_when_output_is_inside_the_mount() {
    let fixture = Fixture::new();
    let path = fixture.path("guarded.bsp");
    fs::write(&path, b"previous output").unwrap();
    let control = Control::default();
    let lease = OutputLease::acquire(&path, &control).unwrap();
    let reservation = WriteReservation::new(lease.path(), &control).unwrap();
    let output = compiler::compile_with_options(&fixture.vmf, &fixture.game, packed_options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let prepared = output
        .prepare_write_reserved(reservation, &control)
        .unwrap()
        .unwrap();
    let sources = compiler::asset_sources::capture_output_asset_source_guard(
        &output,
        &fixture.game,
        &control,
    )
    .unwrap();
    assert!(
        prepared
            .commit_checked_with(&lease, &control, || sources.verify_metadata())
            .unwrap()
    );
    let parsed = Bsp::parse(&fs::read(path).unwrap()).unwrap();
    assert!(
        compiler::read_packed_sky(&parsed, &Control::default())
            .unwrap()
            .is_some()
    );
}

#[test]
fn guarded_publication_is_independent_of_native_capture_requirements() {
    assert!(!Options::default().requires_guarded_publication());
    assert!(packed_options().requires_guarded_publication());
    assert!(packed_options().validate().is_ok());
    let reflection_only = Options {
        generate_reflections: true,
        ..Default::default()
    };
    assert!(reflection_only.requires_guarded_publication());
    assert!(reflection_only.validate().is_err());
}

#[test]
fn cli_pack_only_accepts_fullbright_and_a_nonreflection_output_basename() {
    let fixture = Fixture::new();
    let output = fixture.path("packed sky ü.bsp");
    let result = fixture.cli(&output).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Packed sky portable: ldr"));
    let parsed = Bsp::parse(&fs::read(output).unwrap()).unwrap();
    assert!(
        compiler::read_packed_sky(&parsed, &Control::default())
            .unwrap()
            .is_some()
    );
    let reopened = Vmf::parse(&fs::read_to_string(fixture.path("source.vmf")).unwrap()).unwrap();
    assert_eq!(reopened, fixture.vmf);
}

#[test]
fn cli_pack_only_respects_existing_output_lease_and_preserves_failed_output() {
    let fixture = Fixture::new();
    let output = fixture.path("preserved.bsp");
    fs::write(&output, b"previous output").unwrap();
    let lease = OutputLease::acquire(&output, &Control::default()).unwrap();
    let result = fixture.cli(&output).output().unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    drop(lease);
    fs::remove_file(fixture.path("materials/skybox/portablert.vmt")).unwrap();
    let result = fixture.cli(&output).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Missing sky source"));
    assert_eq!(fs::read(output).unwrap(), b"previous output");
}
