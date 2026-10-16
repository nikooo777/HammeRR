use native_runtime::{CaptureChannel, CaptureReceipt};
use source_formats::{
    bsp::{self, Bsp, PakEntry, PakLimits},
    gamefs::GameFs,
    vmf::Vmf,
    vtf::Vtf,
};

use super::*;
use crate::{Control, LightingMode, Options, Output, compile_with_context_controlled};

fn native_hdr_cube(edge: u16) -> Vec<u8> {
    let mut bytes = hdr::bootstrap_cube(edge).unwrap();
    let mut offset = 88;
    for mip in (0..bytes[56]).rev() {
        let size = usize::from((edge >> mip).max(1));
        for face in 0..7 {
            for _ in 0..size * size {
                if face == 6 {
                    bytes[offset..offset + 8].fill(0xff);
                } else {
                    for (channel, bits) in
                        [0x3c00u16 + face * 16 + u16::from(mip), 1, 0x8000, 0x7fff]
                            .into_iter()
                            .enumerate()
                    {
                        bytes[offset + channel * 2..offset + channel * 2 + 2]
                            .copy_from_slice(&bits.to_le_bytes());
                    }
                }
                offset += 8;
            }
        }
    }
    let mut native = bytes[..80].to_vec();
    native[12..16].copy_from_slice(&96u32.to_le_bytes());
    native[68..72].copy_from_slice(&2u32.to_le_bytes());
    native[20..24].copy_from_slice(&0x600cu32.to_le_bytes());
    native[28..32].fill(0xa5);
    native.extend_from_slice(&[0x30, 0, 0, 0, 96, 0, 0, 0, b'C', b'R', b'C', 2, 1, 2, 3, 4]);
    native.extend_from_slice(&bytes[88..]);
    native
}

fn pending(vmf: &Vmf, game: &GameFs) -> Output {
    let output = compile_with_context_controlled(
        vmf,
        game,
        Options {
            lighting: LightingMode::DirectLdrHdr,
            generate_reflections: true,
            ..Default::default()
        },
        Some(&CompileContext::new("reflections").unwrap()),
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn captures(output: &Output) -> (CubemapAssetManifest, Vec<finalize::CapturedChannel>) {
    let manifest = output.cubemap_assets.as_ref().unwrap();
    let (rebased, private) = manifest::rebase(
        &output.bsp.as_ref().unwrap().to_bytes(),
        manifest,
        &CompileContext::new("private_capture").unwrap(),
        &Control::default(),
    )
    .unwrap();
    let result = [CaptureChannel::Ldr, CaptureChannel::Hdr]
        .into_iter()
        .map(|channel| {
            let staged =
                native_runtime::capture_staging_bsp_for_channel(&rebased, channel).unwrap();
            let mut bsp = Bsp::parse(&staged).unwrap();
            let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
            let samples = if channel == CaptureChannel::Hdr {
                &private.hdr_samples
            } else {
                &private.samples
            };
            for sample in samples {
                archive
                    .entries
                    .iter_mut()
                    .find(|entry| entry.name == sample.texture)
                    .unwrap()
                    .data = if channel == CaptureChannel::Hdr {
                    native_hdr_cube(sample.edge)
                } else {
                    tests::native_black_cube(sample.edge)
                };
            }
            archive.entries.push(PakEntry {
                name: format!(
                    "materials/maps/{}/cubemapdefault{}",
                    private.namespace,
                    channel.texture_suffix()
                ),
                data: if channel == CaptureChannel::Hdr {
                    native_hdr_cube(1)
                } else {
                    tests::native_black_cube(1)
                },
            });
            if channel == CaptureChannel::Hdr {
                archive.entries.push(PakEntry {
                    name: format!("materials/maps/{}/cubemapdefault.vtf", private.namespace),
                    data: tests::native_black_cube(1),
                });
            }
            bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
            finalize::CapturedChannel {
                expected_staging: staged.clone(),
                receipt: CaptureReceipt {
                    channel,
                    staged_bsp: staged,
                    bsp: bsp.to_bytes(),
                    artifacts: format!("/tmp/fake-{channel:?}-receipt").into(),
                    elapsed_ms: 2,
                    verified_controls: true,
                    preservation_passed: true,
                },
            }
        })
        .collect();
    (private, result)
}

#[test]
fn hdr_canonicalization_preserves_all_six_face_rgb_bits_and_removes_unused_bytes() {
    for edge in [1, 32, 128] {
        let raw = native_hdr_cube(edge);
        let canonical = hdr::canonicalize(&raw, edge).unwrap();
        let before = Vtf::parse(raw).unwrap();
        let after = Vtf::parse(canonical.clone()).unwrap();
        assert_eq!(
            u32::from_le_bytes(canonical[12..16].try_into().unwrap()),
            88
        );
        assert_eq!(u32::from_le_bytes(canonical[68..72].try_into().unwrap()), 1);
        assert_eq!(&canonical[28..32], &[0; 4]);
        assert_eq!(after.reflectivity, before.reflectivity);
        for mip in 0..after.mip_count {
            for face in 0..6 {
                let original = before.image_data(mip, 0, face).unwrap();
                let normalized = after.image_data(mip, 0, face).unwrap();
                for (original, normalized) in
                    original.chunks_exact(8).zip(normalized.chunks_exact(8))
                {
                    assert_eq!(&original[..6], &normalized[..6]);
                    assert_eq!(&normalized[6..], &0x3c00u16.to_le_bytes());
                }
            }
            assert!(
                after
                    .image_data(mip, 0, 6)
                    .unwrap()
                    .iter()
                    .all(|byte| *byte == 0)
            );
        }
        hdr::validate_cube(&canonical, edge).unwrap();
        assert_eq!(hdr::canonicalize(&canonical, edge).unwrap(), canonical);
    }
}

#[test]
fn hdr_validation_rejects_invalid_rgb_in_every_mip_and_malformed_resources() {
    let raw = native_hdr_cube(32);
    let parsed = Vtf::parse(raw.clone()).unwrap();
    let mut offset = 96;
    for mip in (0..parsed.mip_count).rev() {
        let bytes = parsed.image_data(mip, 0, 0).unwrap().len();
        for face in 0..6 {
            for bits in [0x7c00u16, 0x7e00, 0xbc00] {
                let mut invalid = raw.clone();
                invalid[offset + face * bytes..offset + face * bytes + 2]
                    .copy_from_slice(&bits.to_le_bytes());
                assert!(
                    hdr::canonicalize(&invalid, 32).is_err(),
                    "mip {mip}, face {face}, bits {bits}"
                );
            }
        }
        offset += bytes * 7;
    }
    for case in 0..9 {
        let mut invalid = raw.clone();
        match case {
            0 => {
                invalid.pop();
            }
            1 => invalid.push(0),
            2 => invalid[83] = 2,
            3 => invalid[88..96].copy_from_slice(&raw[80..88]),
            4 => invalid[52..56].copy_from_slice(&25u32.to_le_bytes()),
            5 => invalid[20..24].copy_from_slice(&0x200cu32.to_le_bytes()),
            6 => invalid[32..36].copy_from_slice(&f32::NAN.to_le_bytes()),
            7 => invalid[20..24].copy_from_slice(&0xe00cu32.to_le_bytes()),
            8 => invalid[32..36].copy_from_slice(&(-0.25f32).to_le_bytes()),
            _ => unreachable!(),
        }
        assert!(hdr::canonicalize(&invalid, 32).is_err(), "case {case}");
    }
}

#[test]
fn dual_manifests_merge_complete_receipts_and_rebase_both_channels_for_run() {
    let (_directory, game) = tests::game();
    let (vmf, _) = tests::source();
    let output = pending(&vmf, &game);
    let original = output.bsp.as_ref().unwrap().clone();
    let manifest = output.cubemap_assets.clone().unwrap();
    assert_eq!(manifest.version, 2);
    assert_eq!(manifest.channel, "ldr_hdr");
    assert_eq!(manifest.samples.len(), manifest.hdr_samples.len());
    let previous = tempfile::tempdir().unwrap();
    let destination = previous.path().join("previous.bsp");
    std::fs::write(&destination, b"previous published BSP").unwrap();
    assert!(output.write(&destination).is_err());
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"previous published BSP"
    );
    let (private, captures) = captures(&output);
    let output = finalize::merge_captures(
        output,
        &manifest,
        &private,
        captures,
        &game,
        &Control::default(),
    )
    .unwrap();
    assert_eq!(
        output.stats.reflections.as_ref().unwrap().channel,
        "ldr_hdr"
    );
    assert_eq!(
        output
            .stats
            .reflections
            .as_ref()
            .unwrap()
            .capture_artifacts
            .len(),
        2
    );
    let bsp = output.bsp.unwrap();
    for index in 0..64 {
        if index != bsp::lump::PAKFILE {
            assert_eq!(bsp.lump(index), original.lump(index));
        }
    }
    let verified = read_generated_cubemaps(&bsp, &Control::default())
        .unwrap()
        .unwrap();
    assert_eq!(verified.phase, CubemapAssetPhase::Verified);
    assert!(verified.default_texture.is_some());
    assert!(verified.hdr_default_texture.is_some());
    let archive = bsp.pak_data(PakLimits::default()).unwrap();
    for sample in &verified.hdr_samples {
        let data = &archive
            .entries
            .iter()
            .find(|entry| entry.name == sample.texture)
            .unwrap()
            .data;
        hdr::validate_sample(data, sample, CubemapAssetPhase::Verified).unwrap();
        assert_eq!(hash(data), sample.sha256);
    }
    let rebased =
        rebase_generated_cubemaps(&bsp.to_bytes(), "random_run", &Control::default()).unwrap();
    let rebased = Bsp::parse(&rebased).unwrap();
    let next = read_generated_cubemaps(&rebased, &Control::default())
        .unwrap()
        .unwrap();
    assert_eq!(next.logical_namespace, "reflections");
    assert_eq!(next.namespace, "random_run");
    assert!(
        next.samples
            .iter()
            .chain(&next.hdr_samples)
            .all(|sample| sample.texture.starts_with("materials/maps/random_run/"))
    );
    assert!(
        next.default_texture
            .unwrap()
            .path
            .starts_with("materials/maps/random_run/")
    );
    assert!(
        next.hdr_default_texture
            .unwrap()
            .path
            .ends_with("/cubemapdefault.hdr.vtf")
    );
    let rebased_archive = rebased.pak_data(PakLimits::default()).unwrap();
    assert!(
        !rebased_archive
            .entries
            .iter()
            .any(|entry| entry.name.starts_with("materials/maps/reflections/"))
    );
}

#[test]
fn dual_capture_rejects_missing_swapped_modified_and_unchanged_channels() {
    let (_directory, game) = tests::game();
    let (vmf, _) = tests::source();
    for case in 0..10 {
        let output = pending(&vmf, &game);
        let manifest = output.cubemap_assets.clone().unwrap();
        let (private, mut captures) = captures(&output);
        match case {
            0 => {
                captures.pop();
            }
            1 => captures[1].receipt.channel = CaptureChannel::Ldr,
            2 => captures[0].receipt.channel = CaptureChannel::Hdr,
            3..=8 => {
                let target = if case == 6 { 0 } else { 1 };
                let mut bsp = Bsp::parse(&captures[target].receipt.bsp).unwrap();
                let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
                let hdr_path = &private.hdr_samples[0].texture;
                let ldr_path = &private.samples[0].texture;
                match case {
                    3 => archive.entries.retain(|entry| &entry.name != hdr_path),
                    4 => {
                        archive
                            .entries
                            .iter_mut()
                            .find(|entry| &entry.name == hdr_path)
                            .unwrap()
                            .data = hdr::bootstrap_cube(private.hdr_samples[0].edge).unwrap()
                    }
                    5 => archive
                        .entries
                        .iter_mut()
                        .find(|entry| &entry.name == hdr_path)
                        .unwrap()
                        .data[96..98]
                        .copy_from_slice(&0x7e00u16.to_le_bytes()),
                    6 => {
                        archive
                            .entries
                            .iter_mut()
                            .find(|entry| &entry.name == hdr_path)
                            .unwrap()
                            .data[88] ^= 1
                    }
                    7 => {
                        archive
                            .entries
                            .iter_mut()
                            .find(|entry| &entry.name == ldr_path)
                            .unwrap()
                            .data[80] ^= 1
                    }
                    8 => bsp.lumps[bsp::lump::LIGHTING_HDR].data[0] ^= 1,
                    _ => unreachable!(),
                }
                bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
                captures[target].receipt.bsp = bsp.to_bytes();
            }
            9 => captures[1].receipt.preservation_passed = false,
            _ => unreachable!(),
        }
        assert!(
            finalize::merge_captures(
                output,
                &manifest,
                &private,
                captures,
                &game,
                &Control::default()
            )
            .is_err(),
            "case {case}"
        );
    }
}

#[test]
fn hdr_manifest_pairs_are_strict_and_legacy_ldr_serialization_stays_unchanged() {
    let (_directory, game) = tests::game();
    let (vmf, _) = tests::source();
    let output = pending(&vmf, &game);
    let bsp = output.bsp.unwrap();
    let original = output.cubemap_assets.unwrap();
    for case in 0..6 {
        let mut manifest = original.clone();
        match case {
            0 => {
                let _ = manifest.hdr_samples.pop();
            }
            1 => manifest.version = 1,
            2 => manifest.hdr_samples[0].origin[0] += 1,
            3 => manifest.hdr_samples[0].helper_id += 1,
            4 => manifest.hdr_samples[0].texture = manifest.samples[0].texture.clone(),
            5 => manifest.hdr_samples[0].edge *= 2,
            _ => unreachable!(),
        }
        let mut invalid_bsp = bsp.clone();
        let mut archive = invalid_bsp.pak_data(PakLimits::default()).unwrap();
        manifest::persist_manifest(&mut archive, &manifest).unwrap();
        invalid_bsp
            .set_pak_data(&archive, PakLimits::default())
            .unwrap();
        assert!(verify_generated_cubemaps(&invalid_bsp, &manifest, &Control::default()).is_err());
    }
    let legacy = compile_with_context_controlled(
        &vmf,
        &game,
        Options {
            lighting: LightingMode::DirectLdr,
            generate_reflections: true,
            ..Default::default()
        },
        Some(&CompileContext::new("reflections").unwrap()),
        &Control::default(),
    )
    .cubemap_assets
    .unwrap();
    let value = serde_json::to_value(&legacy).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["channel"], "ldr");
    assert!(value.get("hdr_samples").is_none());
    assert!(value.get("hdr_default_texture").is_none());
    assert_eq!(
        serde_json::from_value::<CubemapAssetManifest>(value).unwrap(),
        legacy
    );
    let options = Options {
        lighting: LightingMode::DirectLdrHdr,
        ..Default::default()
    };
    let ordinary = crate::compile_with_options(&vmf, &game, options);
    let named = compile_with_context_controlled(
        &vmf,
        &game,
        options,
        Some(&CompileContext::new("irrelevant").unwrap()),
        &Control::default(),
    );
    assert_eq!(
        ordinary.bsp.unwrap().to_bytes(),
        named.bsp.unwrap().to_bytes()
    );
}
