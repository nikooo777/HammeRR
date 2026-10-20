use std::fs;
use std::path::PathBuf;

use source_formats::bsp::{StaticProp, StaticProps};
use source_formats::gamefs::{Mount, MountedPath};

use super::*;

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    bsp: Bsp,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/validation/prop-lighting-assets");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("case-")
            .tempdir_in(root)
            .unwrap();
        fs::create_dir_all(directory.path().join("models")).unwrap();
        fs::create_dir_all(directory.path().join("materials/models")).unwrap();
        fs::write(directory.path().join("models/test.mdl"), model(17)).unwrap();
        fs::write(
            directory.path().join("materials/models/test.vmt"),
            b"VertexLitGeneric {}",
        )
        .unwrap();
        let game = GameFs {
            game_dir: directory.path().to_owned(),
            base_dir: directory.path().to_owned(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(directory.path().to_owned()),
            }],
        };
        let mut bsp = Bsp::new();
        bsp.set_static_props(&StaticProps {
            version: 6,
            models: vec![StaticProps::model_record("models/test.mdl").unwrap()],
            leaves: Vec::new(),
            props: vec![
                StaticProp {
                    origin: [0.0, 0.0, 0.0],
                    ..StaticProp::default()
                },
                StaticProp {
                    origin: [10.0, 0.0, 0.0],
                    flags: DISABLE_VERTEX_LIGHTING,
                    ..StaticProp::default()
                },
                StaticProp {
                    origin: [20.0, 0.0, 0.0],
                    ..StaticProp::default()
                },
            ],
        })
        .unwrap();
        bsp.lumps[bsp::lump::MAP_FLAGS].data = 0x80_u32.to_le_bytes().to_vec();
        Self {
            directory,
            game,
            bsp,
        }
    }

    fn sources(&self) -> Vec<SourceAsset> {
        ["models/test.mdl", "materials/models/test.vmt"]
            .into_iter()
            .map(|path| {
                let bytes = self.game.read(path).unwrap().unwrap();
                SourceAsset {
                    path: path.into(),
                    sha256: hash(&bytes),
                    bytes: bytes.len(),
                }
            })
            .collect()
    }

    fn plan(&self, channels: Vec<Channel>) -> io::Result<PropLightingPlan> {
        PropLightingPlan::new(
            &self.bsp,
            channels.clone(),
            receivers(),
            generated(&channels),
            self.sources(),
            vec!["materials/models/missing.vmt".into()],
            &self.game,
            &Control::default(),
        )
    }
}

fn model(checksum: u32) -> Vec<u8> {
    let mut bytes = vec![0; 240];
    bytes[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48), (8, checksum), (76, 240), (152, 0x10), (224, 1)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn vhv() -> Vec<u8> {
    let mut bytes = vec![0; 1024];
    for (offset, value) in [
        (0, 2),
        (4, 17),
        (8, 4),
        (12, 4),
        (16, 1),
        (20, 1),
        (44, 1),
        (49, 2),
    ] {
        bytes[offset] = value;
    }
    bytes[512..516].copy_from_slice(&[0x12, 0x34, 0x56, 0xff]);
    bytes
}

fn receivers() -> Vec<Receiver> {
    [0, 2]
        .into_iter()
        .map(|ordinal| Receiver {
            ordinal,
            model: "models/test.mdl".into(),
            checksum: 17,
            meshes: vec![MeshLayout {
                lod: 0,
                vertex_count: 1,
            }],
        })
        .collect()
}

fn generated(channels: &[Channel]) -> Vec<GeneratedAsset> {
    [0, 2]
        .into_iter()
        .flat_map(|ordinal| {
            channels.iter().map(move |&channel| GeneratedAsset {
                ordinal,
                channel,
                data: vhv(),
            })
        })
        .collect()
}

fn archive(bsp: &Bsp) -> PakArchive {
    bsp.pak_data(PakLimits::default()).unwrap()
}

fn persist(bsp: &mut Bsp, archive: &mut PakArchive, manifest: &PropLightingManifest) {
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == MANIFEST_PATH)
        .unwrap()
        .data = manifest_bytes(manifest).unwrap();
    bsp.set_pak_data(archive, PakLimits::default()).unwrap();
}

fn attached(fixture: &Fixture, channels: Vec<Channel>) -> (Bsp, PropLightingManifest) {
    let plan = fixture.plan(channels).unwrap();
    let mut bsp = fixture.bsp.clone();
    plan.attach(&mut bsp, &Control::default()).unwrap();
    (bsp, plan.manifest().clone())
}

#[test]
fn native_receiver_scratch_admits_256_sections_and_rejects_257() {
    let fixture = Fixture::new();
    for (count, accepted) in [(256, true), (257, false)] {
        let meshes = vec![
            MeshLayout {
                lod: 0,
                vertex_count: 1
            };
            count
        ];
        let file = source_formats::vhv::File {
            checksum: 17,
            meshes: meshes
                .iter()
                .map(|mesh| source_formats::vhv::Mesh {
                    layout: source_formats::vhv::MeshLayout {
                        lod: mesh.lod,
                        vertex_count: mesh.vertex_count,
                    },
                    colors: vec![source_formats::vhv::Bgra8 {
                        blue: 12,
                        green: 34,
                        red: 56,
                        alpha: 255,
                    }],
                })
                .collect(),
        }
        .to_bytes(source_formats::vhv::Limits::default())
        .unwrap();
        let mut receivers = receivers();
        for receiver in &mut receivers {
            receiver.meshes = meshes.clone();
        }
        let generated = [0, 2]
            .into_iter()
            .map(|ordinal| GeneratedAsset {
                ordinal,
                channel: Channel::Ldr,
                data: file.clone(),
            })
            .collect();
        let plan = PropLightingPlan::new(
            &fixture.bsp,
            vec![Channel::Ldr],
            receivers,
            generated,
            fixture.sources(),
            Vec::new(),
            &fixture.game,
            &Control::default(),
        );
        assert_eq!(plan.is_ok(), accepted, "{count} sections: {plan:?}");
        if let Ok(plan) = plan {
            let mut bsp = fixture.bsp.clone();
            plan.attach(&mut bsp, &Control::default()).unwrap();
            verify_generated_prop_lighting(&bsp, plan.manifest(), &Control::default()).unwrap();
        }
    }
}

#[test]
fn literal_vhv_and_final_native_ordinals_survive_unrelated_pak_edits_offline() {
    let fixture = Fixture::new();
    let plan = fixture.plan(vec![Channel::Ldr]).unwrap();
    assert_eq!(
        plan.entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["sp_0.vhv", "sp_2.vhv"]
    );
    assert_eq!(plan.entries()[0].data, vhv());
    let before = fixture.bsp.clone();
    let mut bsp = before.clone();
    plan.attach(&mut bsp, &Control::default()).unwrap();
    assert_eq!(map_flags(&bsp).unwrap(), 0x81);
    assert_eq!(sprp(&bsp).unwrap(), sprp(&before).unwrap());
    let mut restored = bsp.clone();
    restored.lumps[bsp::lump::PAKFILE] = before.lumps[bsp::lump::PAKFILE].clone();
    restored.lumps[bsp::lump::MAP_FLAGS] = before.lumps[bsp::lump::MAP_FLAGS].clone();
    assert_eq!(restored, before);
    let mut archive = archive(&bsp);
    for name in [
        "materials/skybox/later.vtf",
        "hammerr/generated-cubemaps.json",
    ] {
        archive.entries.push(PakEntry {
            name: name.into(),
            data: b"later unrelated content".to_vec(),
        });
    }
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    fs::remove_dir_all(fixture.directory.path()).unwrap();
    assert_eq!(
        read_generated_prop_lighting(&bsp, &Control::default()).unwrap(),
        Some(plan.manifest().clone())
    );
    verify_generated_prop_lighting(&bsp, plan.manifest(), &Control::default()).unwrap();
}

#[test]
fn dual_channels_require_complete_siblings_and_exact_flag_bits() {
    let fixture = Fixture::new();
    assert!(fixture.plan(vec![Channel::Hdr]).is_err());
    let (mut bsp, manifest) = attached(&fixture, vec![Channel::Ldr, Channel::Hdr]);
    assert_eq!(map_flags(&bsp).unwrap(), 0x83);
    assert_eq!(
        manifest
            .assets
            .iter()
            .map(|asset| asset.path.as_str())
            .collect::<Vec<_>>(),
        ["sp_0.vhv", "sp_hdr_0.vhv", "sp_2.vhv", "sp_hdr_2.vhv"]
    );
    let original = bsp.clone();
    bsp.lumps[bsp::lump::MAP_FLAGS].data = 0x81_u32.to_le_bytes().to_vec();
    assert!(
        read_generated_prop_lighting(&bsp, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("map flags")
    );
    bsp = original;
    let mut archive = archive(&bsp);
    archive.entries.retain(|entry| entry.name != "sp_hdr_2.vhv");
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    assert!(read_generated_prop_lighting(&bsp, &Control::default()).is_err());
}

#[test]
fn changed_ordinals_disabled_flags_and_unregistered_siblings_are_rejected() {
    let fixture = Fixture::new();
    let (bsp, _) = attached(&fixture, vec![Channel::Ldr]);
    let mut changed = bsp.clone();
    let mut props = changed.static_props().unwrap().unwrap();
    props.props.swap(0, 2);
    changed.set_static_props(&props).unwrap();
    assert!(
        read_generated_prop_lighting(&changed, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("static-prop table")
    );
    props.props[1].flags = 0;
    changed.set_static_props(&props).unwrap();
    assert!(read_generated_prop_lighting(&changed, &Control::default()).is_err());
    for name in ["sp_hdr_0.vhv", "sp_1.vhv", "SP_99.VHV", "other/sp_0.vhv"] {
        let mut changed = bsp.clone();
        let mut archive = archive(&changed);
        archive.entries.push(PakEntry {
            name: name.into(),
            data: vhv(),
        });
        changed
            .set_pak_data(&archive, PakLimits::default())
            .unwrap();
        assert!(
            read_generated_prop_lighting(&changed, &Control::default())
                .unwrap_err()
                .to_string()
                .contains("unregistered")
        );
    }
}

#[test]
fn resealed_hashes_do_not_admit_wrong_vhv_checksum_stride_layout_or_offset() {
    let fixture = Fixture::new();
    let (bsp, manifest) = attached(&fixture, vec![Channel::Ldr]);
    for (offset, value) in [(4, 18), (8, 5), (12, 8), (40, 1), (48, 4)] {
        let mut changed = bsp.clone();
        let mut manifest = manifest.clone();
        let mut archive = archive(&changed);
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == "sp_0.vhv")
            .unwrap();
        entry.data[offset] = value;
        manifest.assets[0].sha256 = hash(&entry.data);
        persist(&mut changed, &mut archive, &manifest);
        assert!(read_generated_prop_lighting(&changed, &Control::default()).is_err());
    }
    let mut changed = bsp;
    let mut archive = archive(&changed);
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == "sp_0.vhv")
        .unwrap()
        .data[512] ^= 1;
    changed
        .set_pak_data(&archive, PakLimits::default())
        .unwrap();
    assert!(
        read_generated_prop_lighting(&changed, &Control::default())
            .unwrap_err()
            .to_string()
            .contains("hash")
    );
}

#[test]
fn constructor_rejects_incomplete_receiver_channel_and_model_provenance() {
    let fixture = Fixture::new();
    let mut incomplete = receivers();
    incomplete.pop();
    assert!(
        PropLightingPlan::new(
            &fixture.bsp,
            vec![Channel::Ldr],
            incomplete,
            generated(&[Channel::Ldr]),
            fixture.sources(),
            Vec::new(),
            &fixture.game,
            &Control::default()
        )
        .is_err()
    );
    let mut missing = generated(&[Channel::Ldr, Channel::Hdr]);
    missing.pop();
    assert!(
        PropLightingPlan::new(
            &fixture.bsp,
            vec![Channel::Ldr, Channel::Hdr],
            receivers(),
            missing,
            fixture.sources(),
            Vec::new(),
            &fixture.game,
            &Control::default()
        )
        .is_err()
    );
    let mut sources = fixture.sources();
    sources[0].sha256 = "0".repeat(64);
    assert!(
        PropLightingPlan::new(
            &fixture.bsp,
            vec![Channel::Ldr],
            receivers(),
            generated(&[Channel::Ldr]),
            sources,
            Vec::new(),
            &fixture.game,
            &Control::default()
        )
        .is_err()
    );
    let mut sources = fixture.sources();
    sources[0].bytes += 1;
    assert!(
        PropLightingPlan::new(
            &fixture.bsp,
            vec![Channel::Ldr],
            receivers(),
            generated(&[Channel::Ldr]),
            sources,
            Vec::new(),
            &fixture.game,
            &Control::default()
        )
        .is_err()
    );
    fs::write(fixture.directory.path().join("models/test.mdl"), model(18)).unwrap();
    assert!(fixture.plan(vec![Channel::Ldr]).is_err());
}

#[test]
fn source_replacement_and_new_failed_candidate_block_retained_publication() {
    let fixture = Fixture::new();
    let plan = fixture.plan(vec![Channel::Ldr]).unwrap();
    let guard = plan
        .capture_source_guard(&fixture.game, &Control::default())
        .unwrap();
    fs::write(
        fixture.directory.path().join("replacement"),
        b"changed material bytes",
    )
    .unwrap();
    fs::rename(
        fixture.directory.path().join("replacement"),
        fixture.directory.path().join("materials/models/test.vmt"),
    )
    .unwrap();
    assert!(guard.verify_metadata().is_err());
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
    let mut bsp = fixture.bsp.clone();
    let before = bsp.clone();
    assert!(plan.attach(&mut bsp, &Control::default()).is_err());
    assert_eq!(bsp, before);
    let fixture = Fixture::new();
    let plan = fixture.plan(vec![Channel::Ldr]).unwrap();
    fs::write(
        fixture
            .directory
            .path()
            .join("materials/models/missing.vmt"),
        b"appeared",
    )
    .unwrap();
    assert!(
        plan.verify_sources(&fixture.game, &Control::default())
            .is_err()
    );
}

#[test]
fn collisions_and_cancellation_leave_bsp_unchanged() {
    let fixture = Fixture::new();
    let plan = fixture.plan(vec![Channel::Ldr]).unwrap();
    for name in [MANIFEST_PATH, "SP_0.VHV", "sp_hdr_99.vhv"] {
        let mut bsp = fixture.bsp.clone();
        bsp.set_pak_data(
            &PakArchive {
                entries: vec![PakEntry {
                    name: name.into(),
                    data: vec![1],
                }],
            },
            PakLimits::default(),
        )
        .unwrap();
        let before = bsp.clone();
        assert!(plan.attach(&mut bsp, &Control::default()).is_err());
        assert_eq!(bsp, before);
    }
    let mut bsp = fixture.bsp.clone();
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
}

#[test]
fn combined_pak_entry_and_aggregate_byte_caps_apply_before_append_growth() {
    let mut archive = PakArchive {
        entries: (0..4095)
            .map(|index| PakEntry {
                name: format!("keep/{index}"),
                data: Vec::new(),
            })
            .collect(),
    };
    let generated = vec![PakEntry {
        name: "sp_0.vhv".into(),
        data: vhv(),
    }];
    assert!(append(&mut archive, &generated, vec![0], &Control::default()).is_err());
    assert_eq!(archive.entries.len(), 4095);
    let mut archive = PakArchive {
        entries: (0..4)
            .map(|index| PakEntry {
                name: format!("keep/{index}"),
                data: vec![0; 16 * 1024 * 1024],
            })
            .collect(),
    };
    assert!(append(&mut archive, &generated, vec![0], &Control::default()).is_err());
    assert_eq!(archive.entries.len(), 4);
}

#[test]
fn bounded_manifest_rejects_unknown_fields_oversized_sequences_and_bytes() {
    let fixture = Fixture::new();
    let plan = fixture.plan(vec![Channel::Ldr]).unwrap();
    let mut manifest = plan.manifest().clone();
    manifest.receivers[0].meshes = vec![
        MeshLayout {
            lod: 0,
            vertex_count: 1
        };
        MAX_MESHES
    ];
    assert!(manifest_bytes(&manifest).is_err());
    let mut value = serde_json::to_value(plan.manifest()).unwrap();
    value["unknown"] = serde_json::json!(1);
    assert!(serde_json::from_value::<PropLightingManifest>(value).is_err());
    let mut value = serde_json::to_value(plan.manifest()).unwrap();
    value["channels"] = serde_json::json!(["ldr", "hdr", "ldr"]);
    assert!(serde_json::from_value::<PropLightingManifest>(value).is_err());
    let (mut bsp, _) = attached(&fixture, vec![Channel::Ldr]);
    let mut archive = archive(&bsp);
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == MANIFEST_PATH)
        .unwrap()
        .data = vec![b' '; MAX_MANIFEST_BYTES + 1];
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    assert!(read_generated_prop_lighting(&bsp, &Control::default()).is_err());
}

#[test]
fn no_manifest_without_generated_flags_or_files_is_unchanged_feature_off() {
    let fixture = Fixture::new();
    let before = fixture.bsp.clone();
    assert!(
        read_generated_prop_lighting(&fixture.bsp, &Control::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.bsp, before);
}
