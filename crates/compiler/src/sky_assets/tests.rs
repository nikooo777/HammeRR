use std::fs;
use std::path::Path;

use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vpk::Vpk;

use super::*;

pub(super) struct Fixture {
    pub(super) directory: tempfile::TempDir,
    pub(super) game: GameFs,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("materials/skybox")).unwrap();
        let game = GameFs {
            game_dir: directory.path().to_owned(),
            base_dir: directory.path().to_owned(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(directory.path().to_owned()),
            }],
        };
        Self { directory, game }
    }

    pub(super) fn write(&self, path: &str, data: impl AsRef<[u8]>) {
        let path = self.directory.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    pub(super) fn family(&self, name: &str, extra: &str) {
        for suffix in SUFFIXES {
            self.write(
                &format!("materials/skybox/{name}{suffix}.vmt"),
                format!("Sky {{ $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr {extra} }}"),
            );
        }
        self.write("materials/skybox/shared_ldr.vtf", vtf(0, 2, 2, 2));
        self.write("materials/skybox/shared_hdr.vtf", vtf(24, 2, 2, 2));
    }
}

pub(super) fn vtf(format: u32, width: u16, height: u16, mips: u8) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&80u32.to_le_bytes());
    bytes[16..18].copy_from_slice(&width.to_le_bytes());
    bytes[18..20].copy_from_slice(&height.to_le_bytes());
    bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
    bytes[52..56].copy_from_slice(&format.to_le_bytes());
    bytes[56] = mips;
    bytes[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    bytes[63..65].copy_from_slice(&1u16.to_le_bytes());
    let pixel_bytes = if matches!(format, 24 | 25) { 8 } else { 4 };
    let size: usize = (0..mips)
        .map(|mip| {
            usize::from((width >> mip).max(1)) * usize::from((height >> mip).max(1)) * pixel_bytes
        })
        .sum();
    bytes.resize(80 + size, 0);
    bytes
}

#[test]
fn exact_hdr_family_reports_both_channels_and_deduplicated_sources() {
    let fixture = Fixture::new();
    fixture.family("test_hdr", "");
    let report = inspect(&fixture.game, Some("TeSt_hdr"));
    assert_eq!(report.authored_name.as_deref(), Some("TeSt_hdr"));
    assert!(report.ldr_ready && report.hdr_ready);
    assert_eq!(report.faces.len(), 6);
    assert_eq!(report.assets_read, 8);
    assert_eq!(report.faces[0].material, "skybox/TeSt_hdrrt");
    assert_eq!(
        report.faces[0].hdr.textures[0].format.as_deref(),
        Some("Rgba16161616F")
    );
    assert!(matches!(
        report.faces[0].source,
        Some(SkyAssetSource::Directory { .. })
    ));
    assert!(report.readiness_scope.contains("VTF layout only"));
    assert_eq!(
        report.limits.hdr_base_formats,
        ["Rgba16161616F", "Rgba16161616"]
    );
    let wrong_family = inspect(&fixture.game, Some("test"));
    assert!(!wrong_family.ldr_ready && !wrong_family.hdr_ready);
    assert!(wrong_family.faces[0].issues[0].contains("Missing sky material"));
}

#[test]
fn ldr_family_never_implies_an_hdr_family_or_game_channel() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    for suffix in SUFFIXES {
        fixture.write(
            &format!("materials/skybox/test{suffix}.vmt"),
            "UnlitGeneric { $basetexture skybox/shared_ldr }",
        );
    }
    let report = inspect(&fixture.game, Some("test"));
    assert!(report.ldr_ready);
    assert!(!report.hdr_ready);
    assert!(report.faces.iter().all(|face| face.hdr.textures.is_empty()));
    assert!(
        report.faces[0]
            .hdr
            .issues
            .iter()
            .any(|issue| issue.contains("not selected automatically"))
    );
}

#[test]
fn channels_keep_missing_truncated_and_unsupported_assets_separate() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Sky { $basetexture skybox/missing $hdrbasetexture skybox/shared_hdr }",
    );
    fixture.write(
        "materials/skybox/testbk.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/truncated }",
    );
    let mut truncated = vtf(24, 2, 2, 2);
    truncated.pop();
    fixture.write("materials/skybox/truncated.vtf", truncated);
    fixture.write(
        "materials/skybox/testlf.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_ldr }",
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(!report.ldr_ready && !report.hdr_ready);
    assert!(!report.faces[0].ldr.ready && report.faces[0].hdr.ready);
    assert!(report.faces[1].ldr.ready && !report.faces[1].hdr.ready);
    assert!(report.faces[1].hdr.textures[0].issues[0].contains("truncated"));
    assert!(report.faces[2].hdr.textures[0].issues[0].contains("This inspection supports"));
    assert!(report.faces[3].ldr.ready && report.faces[3].hdr.ready);
    fixture.write("materials/skybox/shared_hdr.vtf", vtf(25, 2, 2, 2));
    let fresh = inspect(&fixture.game, Some("test"));
    assert_eq!(
        fresh.faces[3].hdr.textures[0].format.as_deref(),
        Some("Rgba16161616")
    );
    assert!(fresh.faces[3].hdr.ready);
}

#[test]
fn patch_provenance_tracks_every_material_and_respects_mount_priority() {
    let mut fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/base.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr }",
    );
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Patch { include materials/skybox/base.vmt }",
    );
    let override_dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(override_dir.path().join("materials/skybox")).unwrap();
    fs::write(
        override_dir.path().join("materials/skybox/shared_hdr.vtf"),
        vtf(25, 1, 1, 1),
    )
    .unwrap();
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(override_dir.path().to_owned()),
        },
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(report.ldr_ready && report.hdr_ready);
    assert_eq!(report.faces[0].material_sources.len(), 2);
    assert_eq!(
        report.faces[0].material_sources[0].path,
        "materials/skybox/testrt.vmt"
    );
    assert_eq!(
        report.faces[0].material_sources[1].path,
        "materials/skybox/base.vmt"
    );
    let Some(SkyAssetSource::Directory { mount, file }) = &report.faces[0].hdr.textures[0].source
    else {
        panic!("expected directory provenance")
    };
    assert_eq!(mount, override_dir.path());
    assert_eq!(
        file,
        &override_dir.path().join("materials/skybox/shared_hdr.vtf")
    );
}

#[test]
fn malformed_higher_priority_assets_never_fall_back_to_valid_lower_mounts() {
    let mut fixture = Fixture::new();
    fixture.family("test", "");
    let override_dir = tempfile::tempdir().unwrap();
    let materials = override_dir.path().join("materials/skybox");
    fs::create_dir_all(&materials).unwrap();
    fs::write(materials.join("testrt.vmt"), "not a material").unwrap();
    fs::write(materials.join("shared_hdr.vtf"), "invalid texture").unwrap();
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(override_dir.path().to_owned()),
        },
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(!report.ldr_ready && !report.hdr_ready);
    assert!(!report.faces[0].issues.is_empty());
    let Some(SkyAssetSource::Directory { mount, .. }) = &report.faces[0].source else {
        panic!("expected malformed material provenance")
    };
    assert_eq!(mount, override_dir.path());
    assert!(report.faces[1].ldr.ready);
    assert!(!report.faces[1].hdr.ready);
    let texture = &report.faces[1].hdr.textures[0];
    let Some(SkyAssetSource::Directory { mount, .. }) = &texture.source else {
        panic!("expected malformed texture provenance")
    };
    assert_eq!(mount, override_dir.path());
    assert!(texture.issues[0].contains("not a VTF"));
    fs::remove_file(materials.join("testrt.vmt")).unwrap();
    fs::remove_file(materials.join("shared_hdr.vtf")).unwrap();
    let fresh = inspect(&fixture.game, Some("test"));
    assert!(fresh.ldr_ready && fresh.hdr_ready);
}

pub(super) fn vpk_material(root: &Path) -> PathBuf {
    let body = b"Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr }";
    let mut tree = b"vmt\0materials/skybox\0testrt\0".to_vec();
    tree.extend(0u32.to_le_bytes());
    tree.extend(0u16.to_le_bytes());
    tree.extend(0x7fffu16.to_le_bytes());
    tree.extend(0u32.to_le_bytes());
    tree.extend((body.len() as u32).to_le_bytes());
    tree.extend(0xffffu16.to_le_bytes());
    tree.extend([0, 0, 0]);
    let mut bytes = 0x55aa1234u32.to_le_bytes().to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((tree.len() as u32).to_le_bytes());
    bytes.extend(tree);
    bytes.extend(body);
    let path = root.join("sky_dir.vpk");
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn vpk_winning_source_is_reported_instead_of_directory_fallback() {
    let mut fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write("materials/skybox/testrt.vmt", "UnsupportedShader {}");
    let path = vpk_material(fixture.directory.path());
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Pak(Vpk::open(&path).unwrap()),
        },
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(report.ldr_ready && report.hdr_ready);
    let Some(SkyAssetSource::Vpk { index, files }) = &report.faces[0].source else {
        panic!("expected VPK provenance")
    };
    assert_eq!(index, &path);
    assert!(files.contains(&path));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["faces"][0]["source"]["kind"], "vpk");
}

#[test]
fn unsupported_material_features_and_hdr_encodings_are_explicit() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrcompressedtexture skybox/shared_ldr }",
    );
    fixture.write("materials/skybox/testbk.vmt", "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr Proxies { TextureScroll {} } }");
    fixture.write(
        "materials/skybox/testlf.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr $unsupported 1 }",
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(report.faces[0].ldr.ready && !report.faces[0].hdr.ready);
    assert!(report.faces[0].hdr.issues[0].contains("unsupported by this inspection"));
    assert!(report.faces[0].hdr.textures[0].source.is_some());
    assert!(!report.faces[1].ldr.ready && !report.faces[1].hdr.ready);
    assert!(report.faces[1].issues[0].contains("conditional branches"));
    assert!(report.faces[2].issues[0].contains("Unsupported sky material parameter"));
}

#[test]
fn invalid_names_paths_and_patch_cycles_cannot_escape_mounts() {
    let fixture = Fixture::new();
    for name in [
        None,
        Some(""),
        Some(" test"),
        Some("../outside"),
        Some("/outside"),
        Some("bad\0name"),
    ] {
        let report = inspect(&fixture.game, name);
        assert!(!report.ldr_ready && !report.hdr_ready);
        assert_eq!(report.assets_read, 0);
        assert!(!report.issues.is_empty());
    }
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Patch { include materials/skybox/testrt.vmt }",
    );
    fixture.write(
        "materials/skybox/testbk.vmt",
        "Sky { $basetexture /outside $hdrbasetexture ../outside }",
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(report.faces[0].issues[0].contains("nest too deeply"));
    assert!(report.faces[1].ldr.textures[0].source.is_none());
    assert!(report.faces[1].hdr.textures[0].source.is_none());
}

#[test]
fn overlong_material_values_and_issue_lists_are_explicitly_bounded() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let long_reference = "r".repeat(MAX_MATERIAL_BYTES / 2);
    let long_shader = "s".repeat(4096);
    let parameters = (0..80)
        .map(|index| format!("$unknown{index} 1 "))
        .collect::<String>();
    fixture.write(
        "materials/skybox/testrt.vmt",
        format!("{long_shader} {{ $basetexture {long_reference} {parameters} }}"),
    );
    fixture.write(
        "materials/skybox/testbk.vmt",
        format!("Patch {{ include {long_reference} }}"),
    );
    let report = inspect(&fixture.game, Some("test"));
    assert!(!report.ldr_ready && !report.hdr_ready);
    assert!(report.report_truncated);
    assert!(report.faces[0].shader_truncated);
    assert!(report.faces[0].ldr.textures[0].reference_truncated);
    assert!(report.faces[0].ldr.textures[0].source.is_none());
    assert!(report.faces[0].ldr.textures[0].path.is_empty());
    assert_eq!(report.faces[0].issues.len(), MAX_ISSUES);
    assert!(
        report.faces[0]
            .issues
            .last()
            .unwrap()
            .contains("additional issues omitted")
    );
    assert!(report.faces[1].issues[0].contains("path exceeds 512 bytes"));
    assert!(serde_json::to_vec(&report).unwrap().len() < 64 * 1024);
    let long_name = inspect(&fixture.game, Some(&"n".repeat(4096)));
    assert!(long_name.authored_name_truncated && long_name.report_truncated);
    assert_eq!(long_name.assets_read, 0);
}

#[test]
fn material_texture_and_total_work_limits_are_bounded() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write("materials/skybox/shared_hdr.vtf", vtf(24, 4097, 1, 1));
    assert!(
        inspect(&fixture.game, Some("test")).faces[0].hdr.textures[0].issues[0].contains("4096")
    );
    let oversized = fixture.directory.path().join("materials/skybox/testrt.vmt");
    fs::File::create(oversized)
        .unwrap()
        .set_len((MAX_MATERIAL_BYTES + 1) as u64)
        .unwrap();
    assert!(inspect(&fixture.game, Some("test")).faces[0].issues[0].contains("byte read limit"));
    let mut reader = Reader::new(&fixture.game);
    for index in 0..MAX_ASSETS {
        assert!(
            reader
                .read(&format!("missing/{index}"), MAX_TOTAL_BYTES)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        reader
            .read("missing/overflow", MAX_TOTAL_BYTES)
            .unwrap_err()
            .contains("128")
    );
    for name in ["large_a", "large_b"] {
        fs::File::create(
            fixture
                .directory
                .path()
                .join(format!("materials/{name}.vtf")),
        )
        .unwrap()
        .set_len(33 * 1024 * 1024)
        .unwrap();
    }
    let mut reader = Reader::new(&fixture.game);
    assert!(
        reader
            .read("materials/large_a.vtf", MAX_TOTAL_BYTES)
            .unwrap()
            .is_some()
    );
    assert!(
        reader
            .read("materials/large_b.vtf", MAX_TOTAL_BYTES)
            .unwrap_err()
            .contains("64 MiB total")
    );
    assert!(reader.bytes <= MAX_TOTAL_BYTES);
}
