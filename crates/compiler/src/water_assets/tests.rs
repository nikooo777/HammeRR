use std::fs;
use std::io;

use source_formats::bsp::{self, Bsp, PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::{GameFs, Mount, MountedPath};

use super::*;
use crate::Control;
use crate::asset_sources::{BuildMaterialSources, hash};
use crate::water_dependencies::MAX_MATERIAL_BYTES;

pub(super) const ROOT: &str = "materials/test/root.vmt";
pub(super) const BOTTOM: &str = "materials/test/bottom.vmt";
pub(super) const NORMAL: &str = "materials/test/normal.vtf";
pub(super) const CUBE: &str = "materials/test/cube.vtf";
pub(super) const HDR_CUBE: &str = "materials/test/cube.hdr.vtf";
pub(super) const WATER: &str = "Water { $normalmap test/normal $basetexture test/color $envmap test/cube $bottommaterial test/bottom $underwateroverlay 0 }";
pub(super) const BOTTOM_WATER: &str =
    "Water { $normalmap test/normal $envmap test/cube $underwateroverlay 0 }";

pub(super) struct Fixture {
    pub(super) directory: tempfile::TempDir,
    pub(super) game: GameFs,
    pub(super) sources: BuildMaterialSources,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("materials/test")).unwrap();
        let game = GameFs {
            game_dir: directory.path().into(),
            base_dir: directory.path().into(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(directory.path().into()),
            }],
        };
        let fixture = Self {
            directory,
            game,
            sources: BuildMaterialSources::default(),
        };
        fixture.write(ROOT, WATER);
        fixture.write(BOTTOM, BOTTOM_WATER);
        fixture.write(NORMAL, texture(false, false));
        fixture.write("materials/test/color.vtf", texture(false, false));
        fixture.write(CUBE, texture(true, false));
        fixture.write(HDR_CUBE, texture(true, true));
        fixture
    }

    pub(super) fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.directory.path().join(path), bytes).unwrap();
    }

    pub(super) fn seed(&self, path: &str) {
        self.sources
            .read_asset(&self.game, path, MAX_MATERIAL_BYTES)
            .unwrap()
            .unwrap();
    }

    pub(super) fn plan(&self, roots: &[&str]) -> io::Result<WaterAssetPlan> {
        collect_water_asset_plan(
            roots.iter().copied(),
            &self.game,
            &self.sources,
            &PakArchive::default(),
            PakLimits::default(),
            &Control::default(),
        )
    }
}

pub(super) fn texture(cube: bool, hdr: bool) -> Vec<u8> {
    let header = if cube { 88u32 } else { 80 };
    let mut bytes = vec![0; header as usize];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [
        (4, 7u32),
        (8, if cube { 4 } else { 2 }),
        (12, header),
        (20, if cube { 0x4000 } else { 0 }),
        (52, if hdr { 24 } else { 0 }),
        (57, u32::MAX),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, 1u16), (18, 1), (24, 1), (26, 0), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    if cube {
        bytes[68..72].copy_from_slice(&1u32.to_le_bytes());
        bytes[80..84].copy_from_slice(&[0x30, 0, 0, 0]);
        bytes[84..88].copy_from_slice(&header.to_le_bytes());
    }
    for _ in 0..if cube { 7 } else { 1 } {
        if hdr {
            for value in [0x3c00u16, 0, 0x4000, 0x3c00] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        } else {
            bytes.extend_from_slice(&[127, 63, 31, 255]);
        }
    }
    bytes
}

pub(super) fn water_bsp(material: &str) -> Bsp {
    let mut bsp = Bsp::new();
    bsp.set_texdata_strings(&[material]);
    bsp.set_records(&[bsp::TexData {
        name_index: 0,
        ..Default::default()
    }]);
    bsp.set_records(&[bsp::TexInfo {
        texdata: 0,
        ..Default::default()
    }]);
    bsp.set_records(&[bsp::LeafWaterData {
        surface_texinfo: 0,
        ..Default::default()
    }]);
    bsp
}

fn archive(bsp: &Bsp) -> PakArchive {
    bsp.pak_data(PakLimits::default()).unwrap()
}

fn persist(bsp: &mut Bsp, archive: &mut PakArchive, manifest: &WaterAssetManifest) {
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_WATER_MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(manifest).unwrap();
    bsp.set_pak_data(archive, PakLimits::default()).unwrap();
}

#[test]
fn complete_static_plan_preserves_original_bytes_and_verifies_without_mounts() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    assert_eq!(plan.roots(), &[ROOT]);
    assert_eq!(plan.entries().len(), 6);
    assert!(plan.combined_pak_bytes() > plan.asset_bytes());
    for entry in plan.entries() {
        assert_eq!(
            entry.data,
            fs::read(fixture.directory.path().join(&entry.name)).unwrap()
        );
        let source = plan
            .sources()
            .iter()
            .find(|source| source.path() == entry.name)
            .unwrap();
        assert_eq!(source.sha256(), hash(&entry.data));
    }
    let mut bsp = water_bsp("TEST/ROOT");
    let before = bsp.clone();
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    assert_eq!(manifest.channels, "ldr_hdr");
    let mut restored = bsp.clone();
    restored.lumps[bsp::lump::PAKFILE] = before.lumps[bsp::lump::PAKFILE].clone();
    assert_eq!(restored, before);
    fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
    assert_eq!(
        read_packed_water(&bsp, &Control::default()).unwrap(),
        Some(manifest.clone())
    );
    verify_packed_water(&bsp, &manifest, &Control::default()).unwrap();
    assert!(plan.verify_source_metadata().is_err());
}

#[test]
fn shared_roots_deduplicate_only_original_assets_inside_water_producer() {
    let fixture = Fixture::new();
    fixture.write("materials/test/second.vmt", BOTTOM_WATER);
    fixture.seed(ROOT);
    fixture.seed(BOTTOM);
    fixture.seed("materials/test/second.vmt");
    let plan = fixture
        .plan(&[ROOT, BOTTOM, ROOT, "materials/test/second.vmt"])
        .unwrap();
    assert_eq!(plan.roots(), &[BOTTOM, ROOT, "materials/test/second.vmt"]);
    assert_eq!(plan.entries().len(), 7);
    assert_eq!(
        plan.entries()
            .iter()
            .filter(|entry| entry.name == CUBE)
            .count(),
        1
    );
    assert_eq!(
        plan.entries()
            .iter()
            .filter(|entry| entry.name == HDR_CUBE)
            .count(),
        1
    );
}

#[test]
fn unobserved_root_is_not_late_read_as_original() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .plan(&[ROOT])
            .unwrap_err()
            .to_string()
            .contains("original compiler material read")
    );
}

#[test]
fn mutation_after_original_material_or_metric_texture_read_is_refused() {
    for path in [ROOT, NORMAL] {
        let fixture = Fixture::new();
        fixture.seed(ROOT);
        fixture.seed(path);
        let mut bytes = fs::read(fixture.directory.path().join(path)).unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 1;
        fixture.write(path, bytes);
        assert!(fixture.plan(&[ROOT]).is_err(), "rebaselined {path}");
    }
}

#[test]
fn missing_or_ldr_formatted_hdr_sibling_is_never_fallback_complete() {
    for ldr_in_hdr in [false, true] {
        let fixture = Fixture::new();
        if ldr_in_hdr {
            fixture.write(HDR_CUBE, texture(true, false));
        } else {
            fs::remove_file(fixture.directory.path().join(HDR_CUBE)).unwrap();
        }
        fixture.seed(ROOT);
        let error = fixture.plan(&[ROOT]).unwrap_err().to_string();
        assert!(
            error.contains(if ldr_in_hdr {
                "HDR color format"
            } else {
                "Missing Water source"
            }),
            "{error}"
        );
    }
}

#[test]
fn generic_vtf_layouts_outside_the_installed_loader_subset_are_refused() {
    for (path, hdr) in [(CUBE, false), (HDR_CUBE, true)] {
        for minor in [4u32, 5] {
            let fixture = Fixture::new();
            let mut bytes = texture(true, hdr);
            bytes[8..12].copy_from_slice(&minor.to_le_bytes());
            bytes[26..28].copy_from_slice(&u16::MAX.to_le_bytes());
            bytes.truncate(88 + 6 * if hdr { 8 } else { 4 });
            let parsed = source_formats::vtf::Vtf::parse(bytes.clone()).unwrap();
            assert_eq!(parsed.faces, 6);
            fixture.write(path, bytes);
            fixture.seed(ROOT);
            let error = fixture.plan(&[ROOT]).unwrap_err().to_string();
            assert!(
                error.contains(path)
                    && error.contains(if minor == 4 {
                        "seven stored square faces"
                    } else {
                        "unsupported by CS:S"
                    }),
                "{error}"
            );
        }
    }
}

#[test]
fn runtime_default_and_reachable_child_envmaps_are_refused() {
    for source in [
        "Water { $normalmap test/normal }",
        "Water { $normalmap test/normal $envmap env_cubemap }",
        "Water { $normalmap test/normal $envmap test/cube $bottommaterial test/bottom }",
        "Water { $normalmap test/normal $envmap test/cube $underwateroverlay test/bottom }",
    ] {
        let fixture = Fixture::new();
        fixture.write(ROOT, source);
        fixture.write(BOTTOM, "Water { $normalmap test/normal }");
        fixture.seed(ROOT);
        let error = fixture.plan(&[ROOT]).unwrap_err().to_string();
        assert!(
            error.contains("explicit static $envmap"),
            "{source}: {error}"
        );
    }
}

#[test]
fn wider_native_reference_spellings_and_misplaced_runtime_targets_are_refused() {
    for reference in [
        "test/cube.vtf",
        "materials/test/cube",
        "Test/Cube",
        "test\\cube",
        "_rt_WaterReflection",
        "../test/cube",
        "test//cube",
    ] {
        let fixture = Fixture::new();
        fixture.write(
            ROOT,
            format!("Water {{ $normalmap test/normal $envmap \"{reference}\" }}"),
        );
        fixture.seed(ROOT);
        assert!(fixture.plan(&[ROOT]).is_err(), "accepted {reference}");
    }
}

#[test]
fn patch_originals_are_packed_with_strict_effective_dependencies() {
    let fixture = Fixture::new();
    let patch = "Patch { include materials/test/bottom.vmt insert { $basetexture test/color } }";
    fixture.write(ROOT, patch);
    fixture.seed(ROOT);
    fixture.seed(BOTTOM);
    let plan = fixture.plan(&[ROOT]).unwrap();
    assert_eq!(
        plan.entries()
            .iter()
            .find(|entry| entry.name == ROOT)
            .unwrap()
            .data,
        patch.as_bytes()
    );
    assert_eq!(
        plan.entries()
            .iter()
            .find(|entry| entry.name == BOTTOM)
            .unwrap()
            .data,
        BOTTOM_WATER.as_bytes()
    );
    assert_eq!(plan.entries().len(), 6);
}

#[test]
fn dynamic_unknown_cyclic_and_non_water_graphs_are_refused() {
    for source in [
        "Water { $normalmap test/normal $envmap test/cube Proxies {} }",
        "Water { $normalmap test/normal $envmap test/cube $unknown 1 }",
        "Water { $normalmap test/normal $envmap test/cube $bottommaterial test/root }",
        "Patch { include materials/test/root.vmt }",
        "LightmappedGeneric { $basetexture test/color }",
        "Water { $normalmap test/normal $envmap test/cube $envmap test/cube }",
    ] {
        let fixture = Fixture::new();
        fixture.write(ROOT, source);
        fixture.seed(ROOT);
        assert!(fixture.plan(&[ROOT]).is_err(), "accepted {source}");
    }
}

#[test]
fn duplicate_bytes_from_another_pak_producer_still_collide() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let existing = PakArchive {
        entries: vec![PakEntry {
            name: CUBE.to_ascii_uppercase(),
            data: texture(true, false),
        }],
    };
    let error = collect_water_asset_plan(
        [ROOT],
        &fixture.game,
        &fixture.sources,
        &existing,
        PakLimits::default(),
        &Control::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("collides"), "{error}");
}

#[test]
fn combined_archive_budget_counts_manifest_and_encoded_overhead() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    let limits = PakLimits {
        max_archive_bytes: plan.asset_bytes(),
        ..PakLimits::default()
    };
    assert!(
        collect_water_asset_plan(
            [ROOT],
            &fixture.game,
            &fixture.sources,
            &PakArchive::default(),
            limits,
            &Control::default()
        )
        .is_err()
    );
    let limits = PakLimits {
        max_entries: plan.entries().len(),
        ..PakLimits::default()
    };
    assert!(
        collect_water_asset_plan(
            [ROOT],
            &fixture.game,
            &fixture.sources,
            &PakArchive::default(),
            limits,
            &Control::default()
        )
        .is_err()
    );
}

#[test]
fn empty_selection_has_an_explicit_empty_manifest() {
    let fixture = Fixture::new();
    let plan = fixture.plan(&[]).unwrap();
    let mut bsp = Bsp::new();
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    assert!(manifest.roots.is_empty());
    assert!(manifest.assets.is_empty());
    assert_eq!(
        read_packed_water(&bsp, &Control::default()).unwrap(),
        Some(manifest)
    );
    assert!(plan.attach(&mut bsp, &Control::default()).is_err());
}

#[test]
fn offline_verification_requires_every_dependency_and_water_surface_coverage() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    let mut original = water_bsp("TEST/ROOT");
    let manifest = plan.attach(&mut original, &Control::default()).unwrap();
    for path in [BOTTOM, HDR_CUBE] {
        let mut bsp = original.clone();
        let mut archive = archive(&bsp);
        let mut changed = manifest.clone();
        changed.assets.retain(|asset| asset.path != path);
        archive.entries.retain(|entry| entry.name != path);
        persist(&mut bsp, &mut archive, &changed);
        assert!(
            read_packed_water(&bsp, &Control::default())
                .unwrap_err()
                .to_string()
                .contains("absent from its manifest")
        );
    }
    for name in ["materials/test/root", "test/root.vmt", "test\\root"] {
        let mut bsp = original.clone();
        bsp.set_texdata_strings(&[name]);
        assert!(
            read_packed_water(&bsp, &Control::default()).is_err(),
            "{name}"
        );
    }
    let mut bsp = original;
    bsp.set_texdata_strings(&["TEST/UNREGISTERED"]);
    assert!(
        read_packed_water(&bsp, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("absent from its manifest")
    );
}

#[test]
fn offline_verification_rechecks_hdr_semantics_after_hashes_are_updated() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    let mut bsp = water_bsp("TEST/ROOT");
    let mut manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    let mut archive = archive(&bsp);
    let entry = archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == HDR_CUBE)
        .unwrap();
    entry.data[88..90].copy_from_slice(&0x7c00u16.to_le_bytes());
    manifest
        .assets
        .iter_mut()
        .find(|asset| asset.path == HDR_CUBE)
        .unwrap()
        .sha256 = hash(&entry.data);
    persist(&mut bsp, &mut archive, &manifest);
    assert!(
        read_packed_water(&bsp, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("finite and nonnegative")
    );
}

#[test]
fn sdk_normal_default_is_mounted_and_role_targets_are_runtime_only() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.directory.path().join("materials/dev")).unwrap();
    fixture.write("materials/dev/water_normal.vtf", texture(false, false));
    fixture.write(ROOT, "Water { $envmap test/cube }");
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    assert_eq!(plan.entries().len(), 4);
    assert!(
        plan.entries()
            .iter()
            .any(|entry| entry.name == "materials/dev/water_normal.vtf")
    );
    assert!(
        !plan
            .entries()
            .iter()
            .any(|entry| entry.name.contains("_rt_"))
    );
}

#[test]
fn canonical_cube_limit_accounts_for_actual_hdr_suffix_and_native_buffer() {
    for basename_bytes in [236, 237] {
        let fixture = Fixture::new();
        let reference = format!("test/{}", "a".repeat(basename_bytes));
        fixture.write(
            ROOT,
            format!("Water {{ $normalmap test/normal $envmap {reference} }}"),
        );
        fixture.write(&format!("materials/{reference}.vtf"), texture(true, false));
        fixture.write(
            &format!("materials/{reference}.hdr.vtf"),
            texture(true, true),
        );
        fixture.seed(ROOT);
        let result = fixture.plan(&[ROOT]);
        if basename_bytes == 236 {
            let plan = result.unwrap();
            assert!(plan.entries().iter().any(|entry| entry.name.len() == 259));
        } else {
            assert!(result.unwrap_err().to_string().contains("259 bytes"));
        }
    }
}

#[test]
fn attach_refuses_late_collision_or_bsp_budget_without_mutation() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    let mut bsp = water_bsp("TEST/ROOT");
    let existing = PakArchive {
        entries: vec![PakEntry {
            name: NORMAL.into(),
            data: texture(false, false),
        }],
    };
    bsp.set_pak_data(&existing, PakLimits::default()).unwrap();
    let before = bsp.clone();
    assert!(
        plan.attach(&mut bsp, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("collide")
    );
    assert_eq!(bsp, before);

    let mut plan = plan;
    plan.limits.max_bsp_bytes = plan.combined_pak_bytes();
    let mut bsp = water_bsp("TEST/ROOT");
    let before = bsp.clone();
    assert!(
        plan.attach(&mut bsp, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("BSP exceeds")
    );
    assert_eq!(bsp, before);
}

#[test]
fn later_source_verification_keeps_the_original_guard() {
    let fixture = Fixture::new();
    fixture.seed(ROOT);
    let plan = fixture.plan(&[ROOT]).unwrap();
    let mut changed = texture(false, false);
    *changed.last_mut().unwrap() ^= 1;
    fixture.write(NORMAL, changed);
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
    assert!(
        plan.capture_source_guard(&fixture.game, &Control::default())
            .is_err()
    );
    assert!(plan.verify_source_metadata().is_err());
}

#[test]
fn included_base_used_as_a_water_surface_requires_its_own_static_closure() {
    let fixture = Fixture::new();
    fixture.write(
        BOTTOM,
        "Water { $normalmap test/normal $envmap env_cubemap }",
    );
    fixture.write(
        ROOT,
        "Patch { include materials/test/bottom.vmt replace { $envmap test/cube } }",
    );
    fixture.seed(ROOT);
    fixture.seed(BOTTOM);
    let plan = fixture.plan(&[ROOT]).unwrap();
    let mut bsp = water_bsp("TEST/ROOT");
    plan.attach(&mut bsp, &Control::default()).unwrap();
    bsp.set_texdata_strings(&["TEST/BOTTOM"]);
    assert!(
        read_packed_water(&bsp, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("explicit static $envmap")
    );
}
