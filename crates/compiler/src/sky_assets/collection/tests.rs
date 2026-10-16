use std::fs;

use super::*;
use crate::sky_assets::tests::{Fixture, vtf};

fn vmf() -> Vmf {
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", "test");
    vmf
}

fn collect(fixture: &Fixture) -> io::Result<SkyAssetPlan> {
    collect_sky_asset_plan(
        &vmf(),
        &fixture.game,
        &PakArchive::default(),
        PakLimits::default(),
        &Control::default(),
    )
}

#[test]
fn complete_dual_plan_retains_original_hdr_bytes_hashes_and_sources() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let mut hdr = vtf(24, 2, 2, 2);
    hdr[80..82].copy_from_slice(&0x4000u16.to_le_bytes());
    fixture.write("materials/skybox/shared_hdr.vtf", &hdr);
    let plan = collect(&fixture).unwrap();
    assert_eq!(plan.family(), "test");
    assert!(plan.has_hdr());
    assert_eq!(plan.entries().len(), 8);
    assert_eq!(plan.sources().len(), 8);
    let packed_hdr = plan
        .entries()
        .iter()
        .find(|entry| entry.name.ends_with("shared_hdr.vtf"))
        .unwrap();
    assert_eq!(packed_hdr.data, hdr);
    for source in plan.sources() {
        let original = fs::read(fixture.directory.path().join(source.path())).unwrap();
        let entry = plan
            .entries()
            .iter()
            .find(|entry| entry.name == source.path())
            .unwrap();
        assert_eq!(entry.data, original);
        assert_eq!(source.sha256(), hash(&original));
        assert!(matches!(source.origin(), SkyAssetSource::Directory { .. }));
        assert!(!source.lookup_paths().files.is_empty());
    }
    assert_eq!(
        plan.asset_bytes(),
        plan.entries()
            .iter()
            .map(|entry| entry.data.len())
            .sum::<usize>()
    );
    assert!(plan.combined_pak_bytes() > plan.asset_bytes());
    plan.verify_sources(&fixture.game, &Control::default())
        .unwrap();
}

#[test]
fn patch_plan_keeps_overridden_declared_textures_and_rejects_missing_ones() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/base.vmt",
        "Sky { $basetexture skybox/base_only $hdrbasetexture skybox/shared_hdr }",
    );
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Patch { include materials/skybox/base.vmt replace { $basetexture skybox/shared_ldr } }",
    );
    fixture.write("materials/skybox/base_only.vtf", vtf(0, 1, 1, 1));
    let plan = collect(&fixture).unwrap();
    assert!(
        plan.entries()
            .iter()
            .any(|entry| entry.name == "materials/skybox/base.vmt")
    );
    assert!(
        plan.entries()
            .iter()
            .any(|entry| entry.name == "materials/skybox/base_only.vtf")
    );
    fs::remove_file(
        fixture
            .directory
            .path()
            .join("materials/skybox/base_only.vtf"),
    )
    .unwrap();
    assert!(
        collect(&fixture)
            .unwrap_err()
            .to_string()
            .contains("Missing sky source")
    );
}

#[test]
fn raw_ambiguity_and_hidden_patch_nodes_are_rejected() {
    let fixture = Fixture::new();
    let base = "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr }";
    let cases = [
        "Sky { $basetexture a $BaseTexture b }",
        "Sky {} Sky {}",
        "#base materials/other.vmt Sky {}",
        "Patch { include materials/skybox/base.vmt Include materials/skybox/base.vmt }",
        "Patch { include materials/skybox/base.vmt unknown { $basetexture bad } }",
        "Patch { include materials/skybox/base.vmt replace { $missing 1 } }",
        "Patch { include materials/skybox/base.vmt replace { $basetexture a $BaseTexture b } }",
        "Patch { include materials/skybox/base.vmt replace { Proxies {} } }",
        "Patch { include materials/skybox/base.vmt insert { $basetexture skybox/different } }",
        "Sky { $basetexture skybox/shared_ldr",
        "/* comment */ Sky { $basetexture skybox/shared_ldr }",
    ];
    for source in cases {
        fixture.family("test", "");
        fixture.write("materials/skybox/base.vmt", base);
        fixture.write("materials/skybox/testrt.vmt", source);
        assert!(collect(&fixture).is_err(), "accepted {source}");
    }
    fixture.write("materials/skybox/testrt.vmt", [0xff, 0xfe]);
    assert!(collect(&fixture).unwrap_err().to_string().contains("UTF-8"));
    for edit in [
        "replace { $nofog 1 }",
        "insert { $basetexture skybox/different }",
    ] {
        fixture.family("test", "");
        fixture.write("materials/skybox/base.vmt", base);
        fixture.write(
            "materials/skybox/testrt.vmt",
            format!("Patch {{ include materials/skybox/base.vmt {edit} }}"),
        );
        let error = collect(&fixture).unwrap_err().to_string();
        assert!(error.contains("ambiguous Patch edit"), "{error}");
        assert!(
            error.contains("replace requires an existing key"),
            "{error}"
        );
    }
}

#[test]
fn raw_include_paths_fail_before_normalized_recursive_reads() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/base.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr }",
    );
    for path in [
        "/materials/skybox/base.vmt".to_owned(),
        "\\materials\\skybox\\base.vmt".into(),
        " materials/skybox/base.vmt ".into(),
        "a".repeat(600),
        "../base.vmt".into(),
    ] {
        fixture.write(
            "materials/skybox/testrt.vmt",
            format!("Patch {{ include \"{path}\" }}"),
        );
        let control = Control::default();
        let mut collector = Collector {
            reader: Reader::new(&fixture.game),
            control: &control,
            limits: PakLimits::default(),
            existing_bytes: 0,
            sources: BTreeMap::new(),
            stamps: BTreeMap::new(),
            raw: BTreeMap::new(),
        };
        assert!(collector.resolve("materials/skybox/testrt.vmt").is_err());
        assert!(
            !collector
                .reader
                .assets
                .contains_key("materials/skybox/base.vmt")
        );
    }
}

#[test]
fn known_scalar_rendering_values_are_retained_without_interpretation() {
    let fixture = Fixture::new();
    fixture.family(
        "test",
        "$basetexturetransform \"uninterpreted transform\" $nofog arbitrary $ignorez unusual",
    );
    let plan = collect(&fixture).unwrap();
    let original = fs::read(fixture.directory.path().join("materials/skybox/testrt.vmt")).unwrap();
    assert_eq!(
        plan.entries()
            .iter()
            .find(|entry| entry.name.ends_with("testrt.vmt"))
            .unwrap()
            .data,
        original
    );
}

#[test]
fn family_admission_requires_six_ldr_or_six_complete_dual_faces() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    for suffix in SUFFIXES {
        fixture.write(
            &format!("materials/skybox/test{suffix}.vmt"),
            "UnlitGeneric { $basetexture skybox/shared_ldr %tooltexture skybox/missing }",
        );
    }
    let ldr = collect(&fixture).unwrap();
    assert!(!ldr.has_hdr());
    assert_eq!(ldr.entries().len(), 7);
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr }",
    );
    assert!(collect(&fixture).is_err());
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Sky { $basetexture skybox/shared_ldr }",
    );
    assert!(collect(&fixture).is_err());
    fixture.family("test", "$hdrcompressedtexture skybox/shared_ldr");
    assert!(collect(&fixture).is_err());
}

#[test]
fn collisions_and_combined_pak_limits_preserve_existing_archive() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let existing = PakArchive {
        entries: vec![PakEntry {
            name: "MATERIALS/SKYBOX/SHARED_LDR.VTF".into(),
            data: vec![1, 2, 3],
        }],
    };
    let before = existing.clone();
    assert!(
        collect_sky_asset_plan(
            &vmf(),
            &fixture.game,
            &existing,
            PakLimits::default(),
            &Control::default()
        )
        .unwrap_err()
        .to_string()
        .contains("collides")
    );
    assert_eq!(existing, before);
    for limits in [
        PakLimits {
            max_name_bytes: 10,
            ..PakLimits::default()
        },
        PakLimits {
            max_entry_bytes: 16,
            ..PakLimits::default()
        },
        PakLimits {
            max_total_bytes: 100,
            ..PakLimits::default()
        },
        PakLimits {
            max_archive_bytes: 22,
            ..PakLimits::default()
        },
        PakLimits {
            max_entries: 2,
            ..PakLimits::default()
        },
    ] {
        assert!(
            collect_sky_asset_plan(
                &vmf(),
                &fixture.game,
                &PakArchive::default(),
                limits,
                &Control::default()
            )
            .is_err()
        );
    }
}

#[test]
fn original_metadata_content_and_cancellation_are_revalidated() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let plan = collect(&fixture).unwrap();
    fixture.write("materials/skybox/shared_ldr.vtf", vtf(0, 1, 1, 1));
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        collect_sky_asset_plan(
            &vmf(),
            &fixture.game,
            &PakArchive::default(),
            PakLimits::default(),
            &control
        )
        .unwrap_err()
        .to_string()
        .contains("cancelled")
    );
    assert!(
        plan.verify_sources(&fixture.game, &control)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}

#[test]
fn same_bytes_in_a_new_higher_priority_source_do_not_replace_original_identity() {
    use source_formats::gamefs::{Mount, MountedPath};
    let mut fixture = Fixture::new();
    fixture.family("test", "");
    let higher = tempfile::tempdir().unwrap();
    fs::create_dir_all(higher.path().join("materials/skybox")).unwrap();
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher.path().into()),
        },
    );
    let plan = collect(&fixture).unwrap();
    let original = fs::read(
        fixture
            .directory
            .path()
            .join("materials/skybox/shared_hdr.vtf"),
    )
    .unwrap();
    fs::write(
        higher.path().join("materials/skybox/shared_hdr.vtf"),
        &original,
    )
    .unwrap();
    assert_eq!(
        plan.entries()
            .iter()
            .find(|entry| entry.name.ends_with("shared_hdr.vtf"))
            .unwrap()
            .data,
        original
    );
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
}

#[test]
fn vpk_original_bytes_and_index_identity_are_retained() {
    use source_formats::gamefs::{Mount, MountedPath};
    use source_formats::vpk::Vpk;
    let mut fixture = Fixture::new();
    fixture.family("test", "");
    let path = crate::sky_assets::tests::vpk_material(fixture.directory.path());
    fixture.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Pak(Vpk::open(&path).unwrap()),
        },
    );
    let plan = collect(&fixture).unwrap();
    let source = plan
        .sources()
        .iter()
        .find(|source| source.path().ends_with("testrt.vmt"))
        .unwrap();
    assert!(matches!(source.origin(), SkyAssetSource::Vpk { index, .. } if index == &path));
    assert!(!source.lookup_paths().files.is_empty());
    fs::remove_file(path).unwrap();
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn symlink_retargeting_with_identical_bytes_invalidates_the_plan() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.family("test", "");
    let folder = fixture.directory.path().join("materials/skybox");
    fs::rename(folder.join("shared_hdr.vtf"), folder.join("first.vtf")).unwrap();
    symlink("first.vtf", folder.join("shared_hdr.vtf")).unwrap();
    let plan = collect(&fixture).unwrap();
    fs::copy(folder.join("first.vtf"), folder.join("second.vtf")).unwrap();
    fs::remove_file(folder.join("shared_hdr.vtf")).unwrap();
    symlink("second.vtf", folder.join("shared_hdr.vtf")).unwrap();
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
}

#[test]
fn oversized_raw_texture_references_are_not_normalized_into_short_dependencies() {
    let fixture = Fixture::new();
    let reference = "./".repeat(300) + "skybox/shared_ldr";
    fixture.family("test", "");
    fixture.write(
        "materials/skybox/testrt.vmt",
        format!("Sky {{ $basetexture \"{reference}\" $hdrbasetexture skybox/shared_hdr }}"),
    );
    assert!(
        collect(&fixture)
            .unwrap_err()
            .to_string()
            .contains("exact bounded")
    );
}
