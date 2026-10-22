use std::fs;
use std::io;

use source_formats::bsp::{self, Bsp, PakArchive, PakEntry, PakLimits, StaticProp, StaticProps};
use source_formats::gamefs::{GameFs, Mount, MountedPath};

use super::*;
use crate::Control;
use crate::asset_sources::{BuildMaterialSources, hash};

pub(super) const MODEL: &str = "models/props/portable.mdl";
pub(super) const VVD: &str = "models/props/portable.vvd";
pub(super) const VTX: &str = "models/props/portable.dx90.vtx";
pub(super) const PHY: &str = "models/props/portable.phy";
pub(super) const FIRST: &str = "materials/models/test/one.vmt";
pub(super) const SECOND: &str = "materials/models/test/two.vmt";
pub(super) const FIRST_TEXTURE: &str = "materials/test/one.vtf";
pub(super) const SECOND_TEXTURE: &str = "materials/test/two.vtf";

pub(super) struct Fixture {
    pub(super) directory: tempfile::TempDir,
    pub(super) game: GameFs,
    pub(super) sources: BuildMaterialSources,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        for path in [
            "models/props",
            "materials/models/test",
            "materials/test",
            "published",
        ] {
            fs::create_dir_all(directory.path().join(path)).unwrap();
        }
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
        let [mdl, vvd, vtx] = model();
        for (path, bytes) in [(MODEL, mdl), (VVD, vvd), (VTX, vtx), (PHY, physics())] {
            fixture.write(path, bytes);
        }
        fixture.write(FIRST, "VertexLitGeneric { $basetexture test/one }");
        fixture.write(SECOND, "VertexLitGeneric { $basetexture test/two }");
        fixture.write(FIRST_TEXTURE, texture());
        fixture.write(SECOND_TEXTURE, texture());
        fixture
    }

    pub(super) fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.directory.path().join(path), bytes).unwrap();
    }

    pub(super) fn seed(&self, path: &str) {
        self.sources
            .read_asset(&self.game, path, PakLimits::default().max_entry_bytes)
            .unwrap()
            .unwrap();
    }

    pub(super) fn seed_roots(&self, physics: bool) {
        self.seed(MODEL);
        if physics {
            self.seed(PHY);
        }
    }

    pub(super) fn plan(&self, bsp: &Bsp) -> io::Result<StaticPropAssetPlan> {
        collect_static_prop_asset_plan(
            bsp,
            &self.game,
            &self.sources,
            &PakArchive::default(),
            PakLimits::default(),
            &Control::default(),
        )
    }

    pub(super) fn mutate(&self, path: &str) {
        let mut bytes = fs::read(self.directory.path().join(path)).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        self.write(path, bytes);
    }
}

fn put_int(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn model() -> [Vec<u8>; 3] {
    let [mut mdl, mut vvd, mut vtx] = crate::test_static_prop_model::model();
    let textures = mdl[256..384].to_vec();
    let body = mdl[400..416].to_vec();
    mdl.resize(1280, 0);
    mdl[248..408].fill(0);
    mdl[1024..1152].copy_from_slice(&textures);
    mdl[1160..1176].copy_from_slice(&body);
    for (offset, value) in [
        (76, 1280),
        (208, 1024),
        (216, 1152),
        (228, 1156),
        (236, 1160),
        (308, 1200),
        (628, 4),
        (632, 4),
        (1024, 720 - 1024),
        (1088, 736 - 1088),
        (1152, 752),
        (1172, 416 - 1160),
    ] {
        put_int(&mut mdl, offset, value);
    }
    mdl[1158..1160].copy_from_slice(&1_u16.to_le_bytes());
    mdl[12..31].copy_from_slice(b"props/portable.mdl\0");
    mdl[1200..1208].copy_from_slice(b"default\0");
    vvd.resize(320, 0);
    for (offset, value) in [(12, 2), (20, 4), (60, 256)] {
        put_int(&mut vvd, offset, value);
    }
    for index in 0..4 {
        put_float(&mut vvd, 256 + index * 16, 1.0);
        put_float(&mut vvd, 268 + index * 16, 1.0);
    }
    vtx.resize(392, 0);
    for (offset, value) in [
        (20, 2),
        (24, 376),
        (44, 2),
        (48, 352 - 44),
        (352, 1),
        (356, 64 - 352),
        (364, 1),
        (368, 64 - 364),
    ] {
        put_int(&mut vtx, offset, value);
    }
    [mdl, vvd, vtx]
}

pub(super) fn physics() -> Vec<u8> {
    let mut bytes = [16_i32, 0, 1, 7].map(i32::to_le_bytes).concat();
    bytes.extend_from_slice(&32_i32.to_le_bytes());
    bytes.extend_from_slice(b"VPHY");
    bytes.extend_from_slice(&0x100_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 20]);
    bytes.extend_from_slice(b"solid { index 0 surfaceprop metalvent }\0");
    bytes
}

pub(super) fn texture() -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_i32), (8, 2), (12, 80), (52, 0), (57, -1)] {
        put_int(&mut bytes, offset, value);
    }
    for (offset, value) in [(16, 2_u16), (18, 2), (24, 1), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 2;
    bytes.extend_from_slice(&[31, 63, 127, 255]);
    for _ in 0..4 {
        bytes.extend_from_slice(&[31, 63, 127, 255]);
    }
    bytes
}

pub(super) fn prop_bsp(solids: &[u8]) -> Bsp {
    let mut bsp = Bsp::new();
    bsp.set_records(&[bsp::Leaf::default()]);
    bsp.lumps[bsp::lump::LEAFS].version = 1;
    bsp.set_static_props(&StaticProps {
        version: 6,
        models: vec![StaticProps::model_record(MODEL).unwrap()],
        leaves: vec![0],
        props: solids
            .iter()
            .map(|solid| StaticProp {
                solid: *solid,
                leaf_count: 1,
                ..Default::default()
            })
            .collect(),
    })
    .unwrap();
    bsp
}

fn archive(bsp: &Bsp) -> PakArchive {
    bsp.pak_data(PakLimits::default()).unwrap()
}

fn persist(bsp: &mut Bsp, archive: &mut PakArchive, manifest: &StaticPropAssetManifest) {
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_STATIC_PROPS_MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(manifest).unwrap();
    bsp.set_pak_data(archive, PakLimits::default()).unwrap();
}

#[test]
fn all_declared_skin_and_lod_materials_are_original_and_offline_complete() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let mut bsp = prop_bsp(&[0, 2]);
    let before = bsp.clone();
    let plan = fixture.plan(&bsp).unwrap();
    assert_eq!(plan.roots(), &[MODEL]);
    assert_eq!(plan.entries().len(), 7);
    assert!(plan.entries().iter().any(|entry| entry.name == SECOND));
    assert!(
        plan.entries()
            .iter()
            .any(|entry| entry.name == SECOND_TEXTURE)
    );
    assert!(!fixture.sources.is_observed(PHY).unwrap());
    for entry in plan.entries() {
        assert_eq!(
            entry.data,
            fs::read(fixture.directory.path().join(&entry.name)).unwrap()
        );
    }
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    let mut restored = bsp.clone();
    restored.lumps[bsp::lump::PAKFILE] = before.lumps[bsp::lump::PAKFILE].clone();
    assert_eq!(restored, before);
    fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
    fs::remove_dir_all(fixture.directory.path().join("models")).unwrap();
    assert_eq!(
        read_packed_static_props(&bsp, &Control::default()).unwrap(),
        Some(manifest.clone())
    );
    verify_packed_static_props(&bsp, &manifest, &Control::default()).unwrap();
}

#[test]
fn required_physics_aggregates_every_emitted_use_without_reading_unused_physics() {
    for solids in [&[0, 2][..], &[0, 6][..], &[6, 0][..]] {
        let fixture = Fixture::new();
        let required = solids.contains(&6);
        fixture.seed_roots(required);
        if !required {
            fixture.write(PHY, b"unused invalid physics");
        }
        let plan = fixture.plan(&prop_bsp(solids)).unwrap();
        assert_eq!(
            plan.entries().iter().any(|entry| entry.name == PHY),
            required
        );
        assert_eq!(fixture.sources.is_observed(PHY).unwrap(), required);
    }
}

#[test]
fn original_mdl_and_required_phy_must_already_belong_to_the_same_owner() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .plan(&prop_bsp(&[0]))
            .unwrap_err()
            .to_string()
            .contains("original compiler read")
    );
    fixture.seed_roots(false);
    assert!(
        fixture
            .plan(&prop_bsp(&[6]))
            .unwrap_err()
            .to_string()
            .contains(PHY)
    );
}

#[test]
fn every_original_first_read_is_preserved_and_drift_refuses_a_plan() {
    for path in [
        MODEL,
        VVD,
        VTX,
        FIRST,
        SECOND,
        FIRST_TEXTURE,
        SECOND_TEXTURE,
        PHY,
    ] {
        let fixture = Fixture::new();
        fixture.seed_roots(true);
        fixture.seed(path);
        fixture.mutate(path);
        assert!(fixture.plan(&prop_bsp(&[6])).is_err(), "rebaselined {path}");
    }
}

#[test]
fn missing_material_candidates_remain_guarded_before_the_original_winner() {
    let fixture = Fixture::new();
    let original = fs::read(fixture.directory.path().join(FIRST)).unwrap();
    fs::remove_file(fixture.directory.path().join(FIRST)).unwrap();
    fixture.write("materials/one.vmt", &original);
    fixture.seed_roots(false);
    let plan = fixture.plan(&prop_bsp(&[0])).unwrap();
    assert!(fixture.sources.is_observed(FIRST).unwrap());
    assert!(
        plan.entries()
            .iter()
            .any(|entry| entry.name == "materials/one.vmt")
    );
    fixture.write(FIRST, original);
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
    assert!(
        plan.capture_source_guard(&fixture.game, &Control::default())
            .is_err()
    );
}

#[test]
fn malformed_present_winner_never_falls_back_and_unselected_materials_are_required() {
    for path in [FIRST, SECOND] {
        let fixture = Fixture::new();
        fixture.write(path, "Patch { include test/fallback }");
        fixture.write(
            "materials/one.vmt",
            "VertexLitGeneric { $basetexture test/one }",
        );
        fixture.seed_roots(false);
        assert!(fixture.plan(&prop_bsp(&[0])).is_err());
    }
    let fixture = Fixture::new();
    fs::remove_file(fixture.directory.path().join(SECOND)).unwrap();
    fixture.seed_roots(false);
    assert!(
        fixture
            .plan(&prop_bsp(&[0]))
            .unwrap_err()
            .to_string()
            .contains("Missing declared")
    );
}

#[test]
fn resource_bearing_or_lenient_material_forms_are_refused() {
    for text in [
        "Patch { include test/one }",
        "VertexLitGeneric { $basetexture test/one Proxies {} }",
        "VertexLitGeneric { $basetexture test/one $bumpmap test/two }",
        "VertexLitGeneric { $basetexture test/one $unknown 1 }",
        "VertexLitGeneric { $basetexture test/one $basetexture test/two }",
        "VertexLitGeneric { $basetexture test/one } UnlitGeneric {}",
        "VertexLitGeneric { $basetexture test/one [$WIN32] }",
        "VertexLitGeneric { $basetexture test/one",
        "VertexLitGeneric { $basetexture _rt_water }",
        "VertexLitGeneric { $basetexture Test/One }",
        "VertexLitGeneric { $basetexture test/one.vtf }",
    ] {
        assert!(
            material::dependency(FIRST, text.as_bytes()).is_err(),
            "accepted {text}"
        );
    }
    assert_eq!(
        material::dependency(FIRST, b"VertexLitGeneric { $basetexture test/one }").unwrap(),
        material::Dependency::Texture(FIRST_TEXTURE.into())
    );
}

#[test]
fn incomplete_animated_cubemap_depth_and_extension_textures_are_refused() {
    let mut cases = Vec::new();
    let mut bytes = texture();
    bytes.pop();
    cases.push(bytes);
    for (offset, value) in [
        (8, 5),
        (20, 0x800),
        (20, 0x4000),
        (20, 0x8000),
        (20, 0x10000),
        (20, 0x4000000),
        (52, 24),
    ] {
        let mut bytes = texture();
        put_int(&mut bytes, offset, value);
        cases.push(bytes);
    }
    for (offset, value) in [(24, 0_u16), (24, 2), (63, 0), (63, 2)] {
        let mut bytes = texture();
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        cases.push(bytes);
    }
    for bytes in cases {
        assert!(material::texture(FIRST_TEXTURE, bytes, &Control::default()).is_err());
    }
}

#[test]
fn source_guard_remains_original_through_later_source_verification() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let plan = fixture.plan(&prop_bsp(&[0])).unwrap();
    fixture.mutate(FIRST_TEXTURE);
    assert!(plan.verify_source_metadata().is_err());
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
    assert!(
        plan.capture_source_guard(&fixture.game, &Control::default())
            .is_err()
    );
}

#[test]
fn combined_limits_count_existing_content_manifest_and_encoded_overhead() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let bsp = prop_bsp(&[0]);
    let plan = fixture.plan(&bsp).unwrap();
    for limits in [
        PakLimits {
            max_entries: plan.entries().len(),
            ..PakLimits::default()
        },
        PakLimits {
            max_archive_bytes: plan.asset_bytes(),
            ..PakLimits::default()
        },
        PakLimits {
            max_total_bytes: plan.asset_bytes(),
            ..PakLimits::default()
        },
        PakLimits {
            max_bsp_bytes: plan.combined_pak_bytes(),
            ..PakLimits::default()
        },
    ] {
        assert!(
            collect_static_prop_asset_plan(
                &bsp,
                &fixture.game,
                &fixture.sources,
                &PakArchive::default(),
                limits,
                &Control::default()
            )
            .is_err()
        );
    }
    let existing = PakArchive {
        entries: vec![PakEntry {
            name: "another/asset.bin".into(),
            data: vec![0; 100],
        }],
    };
    let limits = PakLimits {
        max_entries: plan.entries().len() + 1,
        ..PakLimits::default()
    };
    assert!(
        collect_static_prop_asset_plan(
            &bsp,
            &fixture.game,
            &fixture.sources,
            &existing,
            limits,
            &Control::default()
        )
        .is_err()
    );
}

#[test]
fn cross_producer_case_collisions_fail_even_for_identical_bytes() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let existing = PakArchive {
        entries: vec![PakEntry {
            name: FIRST_TEXTURE.to_ascii_uppercase(),
            data: texture(),
        }],
    };
    assert!(
        collect_static_prop_asset_plan(
            &prop_bsp(&[0]),
            &fixture.game,
            &fixture.sources,
            &existing,
            PakLimits::default(),
            &Control::default()
        )
        .unwrap_err()
        .to_string()
        .contains("collides")
    );
}

#[test]
fn zero_roots_are_explicit_and_do_not_read_assets() {
    let fixture = Fixture::new();
    let mut bsp = Bsp::new();
    let plan = fixture.plan(&bsp).unwrap();
    assert!(plan.roots().is_empty() && plan.entries().is_empty());
    assert!(!fixture.sources.is_observed(MODEL).unwrap());
    let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    assert!(manifest.models.is_empty() && manifest.uses.is_empty() && manifest.assets.is_empty());
    assert_eq!(
        read_packed_static_props(&bsp, &Control::default()).unwrap(),
        Some(manifest)
    );
}

#[test]
fn offline_verification_rechecks_exact_sprp_and_declared_closure() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let mut original = prop_bsp(&[0]);
    let plan = fixture.plan(&original).unwrap();
    let manifest = plan.attach(&mut original, &Control::default()).unwrap();
    for field in [0, 1, 2] {
        let mut bsp = original.clone();
        let mut props = bsp.static_props().unwrap().unwrap();
        match field {
            0 => props.props[0].skin = 1,
            1 => props.props[0].solid = 2,
            _ => props.props[0].origin[0] = 3.0,
        }
        bsp.set_static_props(&props).unwrap();
        assert!(read_packed_static_props(&bsp, &Control::default()).is_err());
    }
    for path in [VVD, VTX, SECOND, SECOND_TEXTURE] {
        let mut bsp = original.clone();
        let mut archive = archive(&bsp);
        let mut manifest = manifest.clone();
        archive.entries.retain(|entry| entry.name != path);
        manifest.assets.retain(|asset| asset.path != path);
        persist(&mut bsp, &mut archive, &manifest);
        assert!(
            read_packed_static_props(&bsp, &Control::default()).is_err(),
            "omitted {path}"
        );
    }
}

#[test]
fn offline_verification_rechecks_admission_after_asset_hashes_are_rewritten() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let mut bsp = prop_bsp(&[0]);
    let plan = fixture.plan(&bsp).unwrap();
    let mut manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
    let mut archive = archive(&bsp);
    let entry = archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == SECOND)
        .unwrap();
    entry.data = b"VertexLitGeneric { $basetexture test/two $bumpmap test/one }".to_vec();
    let asset = manifest
        .assets
        .iter_mut()
        .find(|asset| asset.path == SECOND)
        .unwrap();
    asset.sha256 = hash(&entry.data);
    asset.bytes = entry.data.len();
    persist(&mut bsp, &mut archive, &manifest);
    assert!(read_packed_static_props(&bsp, &Control::default()).is_err());
}

#[test]
fn late_attach_collision_or_sprp_change_does_not_mutate_bsp() {
    let fixture = Fixture::new();
    fixture.seed_roots(false);
    let mut bsp = prop_bsp(&[0]);
    let plan = fixture.plan(&bsp).unwrap();
    bsp.set_pak_data(
        &PakArchive {
            entries: vec![PakEntry {
                name: FIRST.into(),
                data: Vec::new(),
            }],
        },
        PakLimits::default(),
    )
    .unwrap();
    let before = bsp.clone();
    assert!(plan.attach(&mut bsp, &Control::default()).is_err());
    assert_eq!(bsp, before);
    let mut bsp = prop_bsp(&[2]);
    let before = bsp.clone();
    assert!(plan.attach(&mut bsp, &Control::default()).is_err());
    assert_eq!(bsp, before);
}
