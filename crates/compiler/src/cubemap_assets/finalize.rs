use std::collections::{BTreeMap, BTreeSet};
use std::io;

use native_runtime::{
    CaptureEnvironment, CaptureObserver, CaptureReceipt, CaptureRequest, CaptureSample,
};
use source_formats::bsp::{self, Bsp, PakLimits};
use source_formats::gamefs::GameFs;
use source_formats::vtf::Vtf;

use super::manifest::{invalid, persist_manifest, rebase, validate_cube, validate_sample};
use super::material::ensure_default_unmounted;
use super::*;
use crate::{Control, Diagnostic, Output, Stage};

struct Observer<'a>(&'a Control);

impl CaptureObserver for Observer<'_> {
    fn checkpoint(&mut self) -> io::Result<()> {
        self.0.checkpoint().map_err(io::Error::other)
    }
    fn sample_progress(&mut self, current: usize, total: usize) {
        self.0.reflection_sample_progress(current, total);
    }
}

pub fn finalize_reflections(
    output: Output,
    context: &CompileContext,
    game: &GameFs,
    environment: &CaptureEnvironment,
    control: &Control,
) -> io::Result<Output> {
    control.checkpoint().map_err(io::Error::other)?;
    let Some(manifest) = output.cubemap_assets.clone() else {
        return Ok(output);
    };
    if manifest.logical_namespace != context.namespace()
        || manifest.namespace != context.namespace()
    {
        return Err(invalid(
            "Reflection finalizer output context does not match the prepared namespace",
        ));
    }
    let original = output
        .bsp
        .as_ref()
        .ok_or_else(|| invalid("Reflection build has no BSP to finalize"))?;
    verify_generated_cubemaps(original, &manifest, control)?;
    let source_guard = capture_reflection_source_guard(&manifest, game, control)?;
    if manifest.phase == CubemapAssetPhase::Verified {
        return Ok(output);
    }
    ensure_default_unmounted(game, context.namespace()).map_err(invalid)?;
    let private_name = format!(
        "hammerr_capture_{}",
        tempfile::Builder::new()
            .prefix("c")
            .tempfile()?
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace('.', "_")
    );
    let private = CompileContext::new(private_name).map_err(invalid)?;
    ensure_default_unmounted(game, private.namespace()).map_err(invalid)?;
    let (request_bsp, private_manifest) =
        rebase(&original.to_bytes(), &manifest, &private, control)?;
    let expected_staging = native_runtime::capture_staging_bsp(&request_bsp)?;
    control
        .advance(Stage::CaptureReflections)
        .map_err(io::Error::other)?;
    let request = CaptureRequest {
        environment: environment.clone(),
        bsp: request_bsp,
        map_name: private.namespace().into(),
        samples: private_manifest
            .samples
            .iter()
            .map(|sample| CaptureSample {
                origin: sample.origin,
                dimension: sample.edge,
                texture: sample.texture.clone(),
                bootstrap_sha256: sample.bootstrap_sha256.clone(),
            })
            .collect(),
    };
    let receipt = native_runtime::capture(request, &mut Observer(control))?;
    source_guard.verify_metadata()?;
    merge_capture(
        output,
        &manifest,
        &private_manifest,
        &expected_staging,
        receipt,
        game,
        control,
    )
}

pub(super) fn merge_capture(
    mut output: Output,
    manifest: &CubemapAssetManifest,
    private_manifest: &CubemapAssetManifest,
    expected_staging: &[u8],
    receipt: CaptureReceipt,
    game: &GameFs,
    control: &Control,
) -> io::Result<Output> {
    if !receipt.verified_controls || !receipt.preservation_passed {
        return Err(invalid(
            "Native reflection capture did not verify its controls and settings preservation",
        ));
    }
    control
        .advance(Stage::VerifyReflections)
        .map_err(io::Error::other)?;
    if receipt.staged_bsp != expected_staging {
        return Err(invalid(
            "Native capture staged a BSP outside the registered compatibility/controller adapter",
        ));
    }
    let baseline = Bsp::parse(&receipt.staged_bsp)?;
    let captured = Bsp::parse(&receipt.bsp)?;
    verify_non_pak(&receipt.staged_bsp, &receipt.bsp, &baseline, &captured)?;
    let mut check = || control.checkpoint().map_err(io::Error::other);
    let staged_archive = baseline.pak_data_controlled(PakLimits::default(), &mut check)?;
    let captured_archive = captured.pak_data_controlled(PakLimits::default(), &mut check)?;
    let samples: BTreeSet<_> = private_manifest
        .samples
        .iter()
        .map(|sample| sample.texture.as_str())
        .collect();
    let staged_entries: BTreeMap<_, _> = staged_archive
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect();
    let captured_entries: BTreeMap<_, _> = captured_archive
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect();
    for (&name, &data) in &staged_entries {
        if !samples.contains(name) && captured_entries.get(name).copied() != Some(data) {
            return Err(invalid(format!(
                "Native capture changed or removed unrelated packed asset: {name}"
            )));
        }
    }
    let default_path = format!(
        "materials/maps/{}/cubemapdefault.vtf",
        private_manifest.namespace
    );
    for name in captured_entries.keys() {
        if !staged_entries.contains_key(name) && *name != default_path {
            return Err(invalid(format!(
                "Native capture added an unregistered packed asset: {name}"
            )));
        }
    }
    let mut archive = output
        .bsp
        .as_ref()
        .ok_or_else(|| invalid("Reflection merge has no original compiler BSP"))?
        .pak_data_controlled(PakLimits::default(), &mut check)?;
    let mut verified = manifest.clone();
    verified.phase = CubemapAssetPhase::Verified;
    for (sample, private_sample) in verified.samples.iter_mut().zip(&private_manifest.samples) {
        control.checkpoint().map_err(io::Error::other)?;
        let data = captured_entries
            .get(private_sample.texture.as_str())
            .ok_or_else(|| {
                invalid(format!(
                    "Native capture omitted required reflection sample: {}",
                    private_sample.texture
                ))
            })?;
        validate_sample(data, sample, CubemapAssetPhase::Verified)?;
        let sha256 = hash(data);
        if sha256 == sample.bootstrap_sha256 {
            return Err(invalid(format!(
                "Native capture left an unchanged bootstrap: {}",
                sample.texture
            )));
        }
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .ok_or_else(|| invalid("Original compiler sample is missing"))?;
        entry.data = data.to_vec();
        sample.sha256 = sha256;
    }
    if let Some(&data) = captured_entries.get(default_path.as_str()) {
        ensure_default_unmounted(game, &manifest.namespace).map_err(invalid)?;
        ensure_default_unmounted(game, &private_manifest.namespace).map_err(invalid)?;
        let default = Vtf::parse(data.to_vec()).map_err(|error| invalid(error.to_string()))?;
        let edge = u16::try_from(default.width)
            .map_err(|_| invalid("Native default cubemap width overflow"))?;
        if !edge.is_power_of_two() || edge > MAX_CUBE_EDGE {
            return Err(invalid(
                "Native default cubemap exceeds the supported dimensions",
            ));
        }
        validate_cube(data, edge)?;
        let name = format!("materials/maps/{}/cubemapdefault.vtf", manifest.namespace);
        if archive
            .entries
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(&name))
        {
            return Err(invalid(
                "Native default cubemap collides with an existing packed asset",
            ));
        }
        archive.entries.push(bsp::PakEntry {
            name,
            data: data.to_vec(),
        });
        verified.default_texture = Some(CubemapSourceAsset {
            path: format!("materials/maps/{}/cubemapdefault.vtf", manifest.namespace),
            sha256: hash(data),
        });
    }
    persist_manifest(&mut archive, &verified)?;
    let bsp = output.bsp.as_mut().unwrap();
    bsp.set_pak_data_controlled(&archive, PakLimits::default(), &mut check)?;
    verify_generated_cubemaps(bsp, &verified, control)?;
    verify_sources(&verified, game, control)?;
    output.report.diagnostics.push(Diagnostic::info(format!("Verified {} native LDR reflection samples, {} material aliases and {} packed bytes in {} ms; capture evidence: {}", verified.samples.len(), verified.aliases.len(), bsp.lump(bsp::lump::PAKFILE).data.len(), receipt.elapsed_ms, receipt.artifacts.display()), None, None));
    output.stats.reflections = Some(crate::ReflectionStats {
        channel: "ldr",
        verified_samples: verified.samples.len(),
        material_aliases: verified.aliases.len(),
        packed_bytes: bsp.lump(bsp::lump::PAKFILE).data.len(),
        capture_ms: receipt.elapsed_ms,
        artifacts: receipt.artifacts,
    });
    output.cubemap_assets = Some(verified);
    Ok(output)
}

pub fn verify_sources(
    manifest: &CubemapAssetManifest,
    game: &GameFs,
    control: &Control,
) -> io::Result<()> {
    for source in &manifest.sources {
        control.checkpoint().map_err(io::Error::other)?;
        let bytes = game
            .read_limited(&source.path, MAX_GENERATED_BYTES)?
            .ok_or_else(|| {
                invalid(format!(
                    "Reflection source disappeared before publication: {}",
                    source.path
                ))
            })?;
        if hash(&bytes) != source.sha256 {
            return Err(invalid(format!(
                "Reflection source changed before publication: {}",
                source.path
            )));
        }
    }
    for path in &manifest.missing_sources {
        control.checkpoint().map_err(io::Error::other)?;
        if game.contains(path)? {
            return Err(invalid(format!(
                "Reflection model material candidate appeared before publication: {path}"
            )));
        }
    }
    Ok(())
}

fn verify_non_pak(
    before_bytes: &[u8],
    after_bytes: &[u8],
    before: &Bsp,
    after: &Bsp,
) -> io::Result<()> {
    if before.version != after.version
        || before.map_revision != after.map_revision
        || before.game_lumps != after.game_lumps
        || before
            .lumps
            .iter()
            .zip(&after.lumps)
            .enumerate()
            .any(|(index, (old, new))| index != bsp::lump::PAKFILE && old != new)
    {
        return Err(invalid(
            "Native capture changed non-PAK BSP metadata, geometry, lighting, entities or game records",
        ));
    }
    let before_game = normalized_game_lump(before_bytes)?;
    let after_game = normalized_game_lump(after_bytes)?;
    if before_game != after_game {
        return Err(invalid(
            "Native capture changed game-lump bytes outside absolute entry offsets",
        ));
    }
    Ok(())
}

fn normalized_game_lump(bytes: &[u8]) -> io::Result<Vec<u8>> {
    fn integer(bytes: &[u8], offset: usize) -> io::Result<usize> {
        let value = i32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or_else(|| invalid("Truncated game-lump header"))?
                .try_into()
                .unwrap(),
        );
        usize::try_from(value).map_err(|_| invalid("Negative game-lump offset/count/size"))
    }
    let header = 8 + bsp::lump::GAME_LUMP * 16;
    let start = integer(bytes, header)?;
    let size = integer(bytes, header + 4)?;
    let end = start
        .checked_add(size)
        .ok_or_else(|| invalid("Game-lump span overflow"))?;
    let mut normalized = bytes
        .get(start..end)
        .ok_or_else(|| invalid("Game lump exceeds BSP bounds"))?
        .to_vec();
    let count = integer(&normalized, 0)?;
    let directory_end = 4usize
        .checked_add(
            count
                .checked_mul(16)
                .ok_or_else(|| invalid("Game-lump count overflow"))?,
        )
        .ok_or_else(|| invalid("Game-lump directory overflow"))?;
    if count > 4096 || directory_end > size {
        return Err(invalid("Invalid game-lump entry count"));
    }
    let mut spans = Vec::with_capacity(count);
    for index in 0..count {
        let field = 4 + index * 16 + 8;
        let offset = integer(&normalized, field)?;
        let length = integer(&normalized, field + 4)?;
        let limit = offset
            .checked_add(length)
            .ok_or_else(|| invalid("Game-lump entry span overflow"))?;
        if offset < start + directory_end || limit > end {
            return Err(invalid("Game-lump entry escapes its payload/BSP"));
        }
        spans.push((offset, limit));
        normalized[field..field + 4].fill(0);
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(invalid("Overlapping game-lump entry payloads"));
    }
    Ok(normalized)
}
