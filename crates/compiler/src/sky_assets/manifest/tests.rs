use std::fs;

use source_formats::bsp::{GameLump, Lump};
use source_formats::vmf::Vmf;

use super::*;
use crate::sky_assets::SUFFIXES;
use crate::sky_assets::collect_sky_asset_plan;
use crate::sky_assets::tests::{Fixture, vtf};

fn vmf(family: &str) -> Vmf {
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", family);
    vmf
}

fn bsp(family: &str) -> Bsp {
    let mut bsp = Bsp::new();
    bsp.set_entities(&format!(
        "{{ \"classname\" \"worldspawn\" \"skyname\" \"{family}\" \"message\" \"quoted {{ braces }}\" }}\n{{ \"classname\" \"info_player_start\" }}"
    ));
    bsp
}

fn plan(fixture: &Fixture, family: &str, existing: &PakArchive, limits: PakLimits) -> SkyAssetPlan {
    collect_sky_asset_plan(
        &vmf(family),
        &fixture.game,
        existing,
        limits,
        &Control::default(),
    )
    .unwrap()
}

fn packed(fixture: &Fixture) -> (Bsp, SkyAssetManifest) {
    let plan = plan(
        fixture,
        "test",
        &PakArchive::default(),
        PakLimits::default(),
    );
    let mut bsp = bsp("test");
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    (bsp, manifest)
}

fn archive(bsp: &Bsp) -> PakArchive {
    bsp.pak_data(PakLimits::default()).unwrap()
}

fn persist(bsp: &mut Bsp, archive: &mut PakArchive, manifest: &SkyAssetManifest) {
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_SKY_MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(manifest).unwrap();
    bsp.set_pak_data(archive, PakLimits::default()).unwrap();
}

fn replace_asset(bsp: &mut Bsp, manifest: &mut SkyAssetManifest, path: &str, data: Vec<u8>) {
    let mut archive = archive(bsp);
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == path)
        .unwrap()
        .data = data.clone();
    manifest
        .assets
        .iter_mut()
        .find(|asset| asset.path == path)
        .unwrap()
        .sha256 = hash(&data);
    persist(bsp, &mut archive, manifest);
}

fn read_error(bsp: &Bsp) -> String {
    read_packed_sky(bsp, &Control::default())
        .unwrap_err()
        .to_string()
}

#[test]
fn deterministic_dual_attachment_preserves_original_bytes_and_all_non_pak_data() {
    let fixture = Fixture::new();
    fixture.family(
        "test",
        "%tooltexture skybox/does_not_exist $nofog uninterpreted",
    );
    fixture.write(
        "materials/skybox/base.vmt",
        "Sky { $basetexture skybox/base_only $hdrbasetexture skybox/shared_hdr }",
    );
    fixture.write(
        "materials/skybox/testrt.vmt",
        "Patch { include materials/skybox/base.vmt replace { $basetexture skybox/shared_ldr } }",
    );
    fixture.write("materials/skybox/base_only.vtf", vtf(0, 1, 1, 1));
    let mut hdr = vtf(24, 2, 2, 2);
    hdr[80..82].copy_from_slice(&0x4000u16.to_le_bytes());
    hdr[88..90].copy_from_slice(&0x7c00u16.to_le_bytes());
    fixture.write("materials/skybox/shared_hdr.vtf", &hdr);
    let existing = PakArchive {
        entries: vec![PakEntry {
            name: "unrelated/keep.txt".into(),
            data: vec![1, 2, 3],
        }],
    };
    let plan = plan(&fixture, "test", &existing, PakLimits::default());
    let mut bsp = bsp("test");
    bsp.version = 21;
    bsp.map_revision = 29;
    bsp.lumps[bsp::lump::LIGHTING] = Lump {
        version: 7,
        fourcc: [1, 2, 3, 4],
        data: vec![9, 8, 7],
    };
    bsp.game_lumps.push(GameLump {
        id: *b"test",
        flags: 1,
        version: 3,
        data: vec![6, 5, 4],
    });
    bsp.set_pak_data(&existing, PakLimits::default()).unwrap();
    let before = bsp.clone();
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    assert_eq!(manifest, plan.manifest());
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.family, "test");
    assert_eq!(manifest.channels, "ldr_hdr");
    assert_eq!(manifest.assets.len(), 10);
    assert!(
        manifest
            .assets
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    );
    let archive = archive(&bsp);
    for original in plan.entries() {
        assert_eq!(
            archive
                .entries
                .iter()
                .find(|entry| entry.name == original.name)
                .unwrap(),
            original
        );
    }
    assert_eq!(
        archive
            .entries
            .iter()
            .find(|entry| entry.name == "materials/skybox/shared_hdr.vtf")
            .unwrap()
            .data,
        hdr
    );
    assert!(
        archive
            .entries
            .iter()
            .any(|entry| entry == &existing.entries[0])
    );
    let mut restored = bsp.clone();
    restored.lumps[bsp::lump::PAKFILE] = before.lumps[bsp::lump::PAKFILE].clone();
    assert_eq!(restored, before);
    let mut second = before;
    plan.attach(&mut second, &Control::default()).unwrap();
    assert_eq!(second, bsp);
    fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
    assert_eq!(
        read_packed_sky(&bsp, &Control::default()).unwrap(),
        Some(manifest.clone())
    );
    verify_packed_sky(&bsp, &manifest, &Control::default()).unwrap();
}

#[test]
fn ldr_manifest_preserves_exact_authored_case_without_adding_an_hdr_family() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    for suffix in SUFFIXES {
        fixture.write(
            &format!("materials/skybox/test{suffix}.vmt"),
            "UnlitGeneric { $basetexture skybox/shared_ldr }",
        );
    }
    let plan = plan(
        &fixture,
        "TeSt",
        &PakArchive::default(),
        PakLimits::default(),
    );
    let mut bsp = bsp("TeSt");
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    assert_eq!(manifest.family, "TeSt");
    assert_eq!(manifest.channels, "ldr");
    assert_eq!(manifest.assets.len(), 7);
    let archive = archive(&bsp);
    let json = &archive
        .entries
        .iter()
        .find(|entry| entry.name == PACKED_SKY_MANIFEST_PATH)
        .unwrap()
        .data;
    assert!(json.starts_with(br#"{"version":1,"family":"TeSt","channels":"ldr","assets":["#));
    assert!(!String::from_utf8_lossy(json).contains(fixture.directory.path().to_str().unwrap()));
    assert_eq!(
        read_packed_sky(&bsp, &Control::default()).unwrap(),
        Some(manifest)
    );
    bsp.set_entities("{ classname worldspawn skyname test }");
    assert!(read_error(&bsp).contains("differs from the BSP world skyname"));
}

#[test]
fn offline_verification_rejects_omitted_includes_and_overridden_texture_records() {
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
    let (original, manifest) = packed(&fixture);
    for path in [
        "materials/skybox/base.vmt",
        "materials/skybox/base_only.vtf",
        "materials/skybox/shared_hdr.vtf",
    ] {
        for remove_bytes in [false, true] {
            let mut bsp = original.clone();
            let mut archive = archive(&bsp);
            let mut changed = manifest.clone();
            changed.assets.retain(|asset| asset.path != path);
            if remove_bytes {
                archive.entries.retain(|entry| entry.name != path);
            }
            persist(&mut bsp, &mut archive, &changed);
            assert!(
                read_error(&bsp).contains("absent from its manifest"),
                "{path}, remove={remove_bytes}"
            );
        }
    }
    assert!(
        fixture
            .game
            .contains("materials/skybox/base_only.vtf")
            .unwrap()
    );
    let mut bsp = original;
    let mut archive = archive(&bsp);
    let extra = PakEntry {
        name: "materials/skybox/unreferenced.vtf".into(),
        data: vtf(0, 1, 1, 1),
    };
    let mut changed = manifest;
    changed.assets.push(SkyPackedAsset {
        path: extra.name.clone(),
        sha256: hash(&extra.data),
    });
    changed.assets.sort_by(|a, b| a.path.cmp(&b.path));
    archive.entries.push(extra);
    persist(&mut bsp, &mut archive, &changed);
    assert!(read_error(&bsp).contains("does not equal its complete raw asset closure"));
}

#[test]
fn missing_or_changed_registered_bytes_fail_even_when_installed_sources_are_valid() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let (original, _) = packed(&fixture);
    for delete in [false, true] {
        let mut bsp = original.clone();
        let mut archive = archive(&bsp);
        if delete {
            archive
                .entries
                .retain(|entry| entry.name != "materials/skybox/shared_ldr.vtf");
        } else {
            archive
                .entries
                .iter_mut()
                .find(|entry| entry.name == "materials/skybox/shared_ldr.vtf")
                .unwrap()
                .data[80] ^= 1;
        }
        bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
        let error = read_error(&bsp);
        assert!(
            error.contains(if delete {
                "asset is missing"
            } else {
                "asset hash differs"
            }),
            "{error}"
        );
    }
}

#[test]
fn manifest_schema_paths_order_channels_and_hashes_are_strict() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let (original, manifest) = packed(&fixture);
    for kind in 0..10 {
        let mut bsp = original.clone();
        let mut archive = archive(&bsp);
        let mut changed = manifest.clone();
        match kind {
            0 => changed.version = 2,
            1 => changed.channels = "hdr".into(),
            2 => changed.channels = "ldr".into(),
            3 => changed.assets[0].sha256 = "A".repeat(64),
            4 => changed.assets[0].path = changed.assets[0].path.to_ascii_uppercase(),
            5 => changed.assets[0].path = format!("./{}", changed.assets[0].path),
            6 => changed.assets.swap(0, 1),
            7 => changed.assets.insert(0, changed.assets[0].clone()),
            8 => changed.assets.clear(),
            9 => changed.assets[0].path = "materials/../escape.vmt".into(),
            _ => unreachable!(),
        }
        persist(&mut bsp, &mut archive, &changed);
        assert!(
            read_packed_sky(&bsp, &Control::default()).is_err(),
            "accepted manifest case {kind}"
        );
    }
    for data in [
        b"{}".to_vec(),
        br#"{"version":1,"version":1,"family":"test","channels":"ldr","assets":[]}"#.to_vec(),
        vec![b' '; MAX_MANIFEST_BYTES + 1],
    ] {
        let mut bsp = original.clone();
        let mut archive = archive(&bsp);
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == PACKED_SKY_MANIFEST_PATH)
            .unwrap()
            .data = data;
        bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
        assert!(read_packed_sky(&bsp, &Control::default()).is_err());
    }
    let mut bsp = original.clone();
    let mut archive = archive(&bsp);
    let mut value = serde_json::to_value(&manifest).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("origin".into(), serde_json::json!("/machine/path"));
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_SKY_MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(&value).unwrap();
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    assert!(read_error(&bsp).contains("unknown field"));
    let mut case_archive = original.pak_data(PakLimits::default()).unwrap();
    case_archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_SKY_MANIFEST_PATH)
        .unwrap()
        .name = PACKED_SKY_MANIFEST_PATH.to_ascii_uppercase();
    bsp.set_pak_data(&case_archive, PakLimits::default())
        .unwrap();
    assert!(read_error(&bsp).contains("manifest path or size"));
    assert!(
        read_packed_sky(&Bsp::new(), &Control::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn offline_raw_validation_rejects_hidden_ambiguity_even_after_rehashing() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let (original, manifest) = packed(&fixture);
    for source in [
        b"Sky { $basetexture skybox/shared_ldr $BaseTexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr }".as_slice(),
        b"Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr $nofog [$WINDOWS] }",
        b"Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr Proxies {} }",
        b"Sky { $basetexture skybox/shared_ldr $hdrbasetexture skybox/shared_hdr",
        &[0xff, 0xfe],
    ] {
        let mut bsp = original.clone();
        let mut changed = manifest.clone();
        replace_asset(&mut bsp, &mut changed, "materials/skybox/testrt.vmt", source.to_vec());
        assert!(read_packed_sky(&bsp, &Control::default()).is_err());
    }
    let mut bsp = original;
    let mut changed = manifest;
    replace_asset(
        &mut bsp,
        &mut changed,
        "materials/skybox/shared_hdr.vtf",
        vtf(0, 2, 2, 2),
    );
    assert!(read_error(&bsp).contains("supports HDR base textures only"));
}

#[test]
fn world_family_admission_uses_complete_flat_blocks_and_preserves_duplicates() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let (original, _) = packed(&fixture);
    for text in [
        "{ classname worldspawn skyname other }",
        "{ classname worldspawn skyname test SkyName test }",
        "{ classname worldspawn }",
        "{ classname worldspawn classname worldspawn skyname test }",
        "{ classname info_player_start } { classname worldspawn skyname test }",
        "{ classname worldspawn skyname test } { ClassName WorldSpawn skyname test }",
        "{ classname worldspawn skyname test } trailing",
        "{ classname worldspawn skyname test nested { key value } }",
        "{ classname worldspawn skyname test",
    ] {
        let mut bsp = original.clone();
        bsp.set_entities(text);
        assert!(
            read_packed_sky(&bsp, &Control::default()).is_err(),
            "accepted {text}"
        );
    }
    for kind in 0..4 {
        let mut bsp = original.clone();
        let entity = &mut bsp.lumps[bsp::lump::ENTITIES];
        match kind {
            0 => entity.fourcc = [1, 0, 0, 0],
            1 => entity.data = vec![b' '; ENTITY_LIMITS.max_bytes + 2],
            2 => entity.data.push(b'x'),
            3 => entity.data = vec![0xff, 0],
            _ => unreachable!(),
        }
        assert!(read_packed_sky(&bsp, &Control::default()).is_err());
    }
    let mut bsp = original;
    bsp.set_entities(
        "{ classname WORLDSPAWN SKYNAME test message \"braces { and } backslash \\ path\" }",
    );
    assert!(
        read_packed_sky(&bsp, &Control::default())
            .unwrap()
            .is_some()
    );
}

#[test]
fn attachment_collisions_and_family_errors_leave_the_entire_bsp_unchanged() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let plan = plan(
        &fixture,
        "test",
        &PakArchive::default(),
        PakLimits::default(),
    );
    for name in [
        PACKED_SKY_MANIFEST_PATH.to_owned(),
        PACKED_SKY_MANIFEST_PATH.to_ascii_uppercase(),
        "MATERIALS/SKYBOX/SHARED_LDR.VTF".into(),
    ] {
        let mut bsp = bsp("test");
        bsp.set_pak_data(
            &PakArchive {
                entries: vec![PakEntry {
                    name,
                    data: vec![3, 2, 1],
                }],
            },
            PakLimits::default(),
        )
        .unwrap();
        let before = bsp.clone();
        assert!(
            plan.attach(&mut bsp, &Control::default())
                .unwrap_err()
                .to_string()
                .contains("collide")
        );
        assert_eq!(bsp, before);
    }
    let mut bsp = bsp("other");
    let before = bsp.clone();
    assert!(plan.attach(&mut bsp, &Control::default()).is_err());
    assert_eq!(bsp, before);
}

#[test]
fn manifest_consumes_combined_pak_and_bsp_budgets_atomically() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let complete = plan(
        &fixture,
        "test",
        &PakArchive::default(),
        PakLimits::default(),
    );
    for limits in [
        PakLimits {
            max_entries: complete.entries().len(),
            ..PakLimits::default()
        },
        PakLimits {
            max_total_bytes: complete.asset_bytes(),
            ..PakLimits::default()
        },
        PakLimits {
            max_archive_bytes: complete.combined_pak_bytes(),
            ..PakLimits::default()
        },
        PakLimits {
            max_bsp_bytes: 1,
            ..PakLimits::default()
        },
    ] {
        let plan = plan(&fixture, "test", &PakArchive::default(), limits);
        let mut bsp = bsp("test");
        let before = bsp.clone();
        assert!(
            plan.attach(&mut bsp, &Control::default()).is_err(),
            "accepted {limits:?}"
        );
        assert_eq!(bsp, before);
    }
}

#[test]
fn cancellation_and_expected_manifest_mismatch_preserve_bsp_and_sources() {
    let fixture = Fixture::new();
    fixture.family("test", "");
    let plan = plan(
        &fixture,
        "test",
        &PakArchive::default(),
        PakLimits::default(),
    );
    let guard = plan
        .capture_source_guard(&fixture.game, &Control::default())
        .unwrap();
    plan.verify_source_metadata().unwrap();
    guard.verify_metadata().unwrap();
    let mut bsp = bsp("test");
    let before = bsp.clone();
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        plan.attach(&mut bsp, &control)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(bsp, before);
    assert!(
        read_packed_sky(&bsp, &control)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    plan.attach(&mut bsp, &Control::default()).unwrap();
    let mut changed = plan.manifest();
    changed.family = "other".into();
    assert!(
        verify_packed_sky(&bsp, &changed, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("expected sky plan")
    );
    fixture.write("materials/skybox/shared_ldr.vtf", vtf(0, 1, 1, 1));
    assert!(plan.verify_source_metadata().is_err());
    assert!(guard.verify_metadata().is_err());
}

#[test]
fn reflection_rebasing_preserves_every_registered_sky_byte_and_manifest() {
    use crate::cubemap_assets::{self, CompileContext, CubemapAssetPhase};
    use crate::{LightingMode, Options, compile_with_context_controlled};
    use glam::DVec3;
    use map_core::{Document, edit};

    let fixture = Fixture::new();
    fixture.family("test", "");
    fixture.write(
        "materials/dev/dev_measuregeneric01.vmt",
        "LightmappedGeneric { $envmap env_cubemap }",
    );
    let mut document = Document::new(crate::test_common::room());
    document
        .edit("reflection sample", |tx| {
            edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[("cubemapsize".into(), "1".into())],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    vmf.world.properties.set("skyname", "test");
    let control = Control::default();
    let output = compile_with_context_controlled(
        &vmf,
        &fixture.game,
        Options {
            generate_reflections: true,
            lighting: LightingMode::DirectLdr,
            ..Options::default()
        },
        Some(&CompileContext::new("published").unwrap()),
        &control,
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let mut bsp = output.bsp.unwrap();
    let mut reflections = output.cubemap_assets.unwrap();
    let mut archive = archive(&bsp);
    for sample in &mut reflections.samples {
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .unwrap();
        entry.data[80..].fill(1);
        sample.sha256 = hash(&entry.data);
    }
    reflections.phase = CubemapAssetPhase::Verified;
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == cubemap_assets::MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(&reflections).unwrap();
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    cubemap_assets::verify_generated_cubemaps(&bsp, &reflections, &control).unwrap();
    let plan = collect_sky_asset_plan(
        &vmf,
        &fixture.game,
        &archive,
        PakLimits::default(),
        &control,
    )
    .unwrap();
    let sky = plan.attach(&mut bsp, &control).unwrap();
    let original = bsp.pak_data(PakLimits::default()).unwrap();
    let staged =
        cubemap_assets::rebase_generated_cubemaps(&bsp.to_bytes(), "private_run", &control)
            .unwrap();
    let staged = Bsp::parse(&staged).unwrap();
    assert_eq!(
        read_packed_sky(&staged, &control).unwrap(),
        Some(sky.clone())
    );
    let staged_archive = staged.pak_data(PakLimits::default()).unwrap();
    for path in sky
        .assets
        .iter()
        .map(|asset| asset.path.as_str())
        .chain([PACKED_SKY_MANIFEST_PATH])
    {
        assert_eq!(
            original.entries.iter().find(|entry| entry.name == path),
            staged_archive
                .entries
                .iter()
                .find(|entry| entry.name == path)
        );
    }
    assert_eq!(
        cubemap_assets::read_generated_cubemaps(&staged, &control)
            .unwrap()
            .unwrap()
            .namespace,
        "private_run"
    );
}

#[test]
fn world_identity_rejects_trailing_space_aliases_without_normalizing_authored_keys() {
    for text in [
        "{ classname worldspawn \"skyname \" test }",
        "{ classname worldspawn \"SkYnAmE   \" test }",
        "{ \"classname \" worldspawn skyname test }",
        "{ \"ClAsSnAmE   \" worldspawn skyname test }",
        "{ classname worldspawn skyname test } { \"classname \" worldspawn }",
    ] {
        let mut bsp = Bsp::new();
        bsp.set_entities(text);
        let before = bsp.clone();
        let error = world_family(&bsp, &Control::default())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("identity keys cannot contain trailing ASCII spaces"),
            "{error}"
        );
        assert_eq!(bsp, before);
    }
    let mut bsp = Bsp::new();
    bsp.set_entities("{ classname worldspawn skyname test \"message \" preserved }");
    assert_eq!(world_family(&bsp, &Control::default()).unwrap(), "test");
}

#[test]
fn world_identity_rejects_trailing_space_alias_duplicates_in_either_order() {
    for text in [
        "{ classname worldspawn skyname test \"skyname \" other }",
        "{ classname worldspawn \"skyname \" other skyname test }",
        "{ classname worldspawn skyname test \"classname \" info_player_start }",
        "{ \"classname \" info_player_start classname worldspawn skyname test }",
        "{ classname worldspawn skyname test } { classname info_player_start \"classname \" worldspawn }",
    ] {
        let mut bsp = Bsp::new();
        bsp.set_entities(text);
        let error = world_family(&bsp, &Control::default())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("identity keys cannot contain trailing ASCII spaces"),
            "{error}"
        );
    }
}

#[test]
fn world_identity_entity_key_admission_counts_bytes_at_the_fixed_boundary() {
    for key in ["x".repeat(2047), format!("{}x", "é".repeat(1023))] {
        assert_eq!(key.len(), 2047);
        let mut bsp = Bsp::new();
        bsp.set_entities(&format!(
            "{{ classname worldspawn skyname test \"{key}\" retained }}"
        ));
        assert_eq!(world_family(&bsp, &Control::default()).unwrap(), "test");
    }
    for key in ["x".repeat(2048), "é".repeat(1024)] {
        assert_eq!(key.len(), 2048);
        let mut bsp = Bsp::new();
        bsp.set_entities(&format!(
            "{{ classname worldspawn skyname test \"{key}\" retained }}"
        ));
        assert!(
            world_family(&bsp, &Control::default())
                .unwrap_err()
                .to_string()
                .contains("2047-byte admission limit")
        );
    }
}

#[test]
fn world_identity_rejects_overlong_identity_aliases_with_late_nonspace_suffixes() {
    for prefix in ["skyname", "classname"] {
        let key = format!("{prefix}{}x", " ".repeat(2047 - prefix.len()));
        assert_eq!(key.len(), 2048);
        assert_eq!(key.trim_end_matches(' '), key);
        let mut bsp = Bsp::new();
        bsp.set_entities(&format!(
            "{{ classname worldspawn skyname test \"{key}\" other }}"
        ));
        assert!(
            world_family(&bsp, &Control::default())
                .unwrap_err()
                .to_string()
                .contains("2047-byte admission limit")
        );
    }
}

#[test]
fn world_identity_requires_canonical_classname_keys_and_retains_sky_key_case() {
    for text in [
        "{ CLASSNAME worldspawn skyname test }",
        "{ ClassName worldspawn skyname test }",
        "{ classname worldspawn CLASSNAME info_player_start skyname test }",
        "{ CLASSNAME info_player_start classname worldspawn skyname test }",
        "{ classname worldspawn skyname test } { CLASSNAME worldspawn }",
    ] {
        let mut bsp = Bsp::new();
        bsp.set_entities(text);
        assert!(
            world_family(&bsp, &Control::default())
                .unwrap_err()
                .to_string()
                .contains("classname keys must use canonical lowercase spelling")
        );
    }
    let mut bsp = Bsp::new();
    bsp.set_entities("{ classname WORLDSPAWN SKYNAME TeSt }");
    assert_eq!(world_family(&bsp, &Control::default()).unwrap(), "TeSt");
}
