use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::ops::Range;

use native_runtime::{
    CaptureChannel, CaptureEnvironment, CaptureObserver, CaptureReceipt, CaptureRequest,
    CaptureSample,
};
use source_formats::bsp::{self, Bsp, PakLimits};
use source_formats::gamefs::GameFs;
use source_formats::vtf::Vtf;

use super::manifest::{invalid, persist_manifest, rebase, validate_cube, validate_sample};
use super::material::ensure_default_unmounted;
use super::*;
use crate::{Control, Diagnostic, Output, Stage};

struct Observer<'a> {
    control: &'a Control,
    offset: usize,
    total: usize,
}

impl CaptureObserver for Observer<'_> {
    fn checkpoint(&mut self) -> io::Result<()> {
        self.control.checkpoint().map_err(io::Error::other)
    }
    fn sample_progress(&mut self, current: usize, _total: usize) {
        self.control
            .reflection_sample_progress(self.offset + current, self.total);
    }
}

pub fn finalize_reflections(
    output: Output,
    context: &CompileContext,
    game: &GameFs,
    environment: &CaptureEnvironment,
    control: &Control,
) -> io::Result<Output> {
    finalize_reflections_with_capture(
        output,
        context,
        game,
        environment,
        control,
        native_runtime::capture,
    )
}

pub fn finalize_reflections_with_capture(
    output: Output,
    context: &CompileContext,
    game: &GameFs,
    environment: &CaptureEnvironment,
    control: &Control,
    mut capture: impl FnMut(CaptureRequest, &mut dyn CaptureObserver) -> io::Result<CaptureReceipt>,
) -> io::Result<Output> {
    control.checkpoint().map_err(io::Error::other)?;
    let Some(manifest) = output.cubemap_assets.clone() else {
        return Ok(output);
    };
    if let Some(dependencies) = &output.instance_dependencies {
        dependencies.verify(control)?;
    }
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
    if let Some(plan) = &output.water_assets {
        crate::water_assets::verify_packed_water(original, &plan.manifest(), control)?;
        plan.verify_sources(game, control)?;
    }
    if let Some(plan) = &output.static_prop_assets {
        crate::static_prop_assets::verify_packed_static_props(original, &plan.manifest(), control)?;
        plan.verify_sources(game, control)?;
    }
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
    let channels = if manifest.hdr_samples.is_empty() {
        vec![CaptureChannel::Ldr]
    } else {
        super::material::ensure_hdr_default_unmounted(game, context.namespace())
            .map_err(invalid)?;
        super::material::ensure_hdr_default_unmounted(game, private.namespace())
            .map_err(invalid)?;
        vec![CaptureChannel::Ldr, CaptureChannel::Hdr]
    };
    control
        .advance(Stage::CaptureReflections)
        .map_err(io::Error::other)?;
    let mut captures = Vec::with_capacity(channels.len());
    let total = manifest.samples.len() * channels.len();
    for (index, channel) in channels.into_iter().enumerate() {
        let expected_staging =
            native_runtime::capture_staging_bsp_for_channel(&request_bsp, channel)?;
        let samples = if channel == CaptureChannel::Hdr {
            &private_manifest.hdr_samples
        } else {
            &private_manifest.samples
        };
        let request = CaptureRequest {
            channel,
            environment: environment.clone(),
            bsp: request_bsp.clone(),
            map_name: private.namespace().into(),
            samples: samples
                .iter()
                .map(|sample| CaptureSample {
                    origin: sample.origin,
                    dimension: sample.edge,
                    texture: sample.texture.clone(),
                    bootstrap_sha256: sample.bootstrap_sha256.clone(),
                })
                .collect(),
        };
        if let Some(dependencies) = &output.instance_dependencies {
            dependencies.verify(control)?;
        }
        let receipt = capture(
            request,
            &mut Observer {
                control,
                offset: index * manifest.samples.len(),
                total,
            },
        )?;
        if let Some(dependencies) = &output.instance_dependencies {
            dependencies.verify(control)?;
        }
        if receipt.channel != channel {
            return Err(invalid(
                "Native reflection receipt has the wrong capture channel",
            ));
        }
        source_guard.verify_metadata()?;
        captures.push(CapturedChannel {
            expected_staging,
            receipt,
        });
    }
    merge_captures(
        output,
        &manifest,
        &private_manifest,
        captures,
        game,
        control,
    )
}

pub(super) struct CapturedChannel {
    pub expected_staging: Vec<u8>,
    pub receipt: CaptureReceipt,
}

#[cfg(test)]
pub(super) fn merge_capture(
    output: Output,
    manifest: &CubemapAssetManifest,
    private_manifest: &CubemapAssetManifest,
    expected_staging: &[u8],
    receipt: CaptureReceipt,
    game: &GameFs,
    control: &Control,
) -> io::Result<Output> {
    merge_captures(
        output,
        manifest,
        private_manifest,
        vec![CapturedChannel {
            expected_staging: expected_staging.to_vec(),
            receipt,
        }],
        game,
        control,
    )
}

pub(super) fn merge_captures(
    mut output: Output,
    manifest: &CubemapAssetManifest,
    private_manifest: &CubemapAssetManifest,
    captures: Vec<CapturedChannel>,
    game: &GameFs,
    control: &Control,
) -> io::Result<Output> {
    let expected = if manifest.hdr_samples.is_empty() {
        1
    } else {
        2
    };
    verify_generated_cubemaps(
        output
            .bsp
            .as_ref()
            .ok_or_else(|| invalid("Reflection merge has no original compiler BSP"))?,
        manifest,
        control,
    )?;
    if captures.len() != expected
        || captures
            .first()
            .is_none_or(|capture| capture.receipt.channel != CaptureChannel::Ldr)
        || captures
            .iter()
            .filter(|capture| capture.receipt.channel == CaptureChannel::Ldr)
            .count()
            != 1
        || captures
            .iter()
            .filter(|capture| capture.receipt.channel == CaptureChannel::Hdr)
            .count()
            != expected - 1
    {
        return Err(invalid(
            "Reflection finalization requires exactly one verified capture for every registered channel",
        ));
    }
    control
        .advance(Stage::VerifyReflections)
        .map_err(io::Error::other)?;
    let mut check = || control.checkpoint().map_err(io::Error::other);
    let mut archive = output
        .bsp
        .as_ref()
        .ok_or_else(|| invalid("Reflection merge has no original compiler BSP"))?
        .pak_data_controlled(PakLimits::default(), &mut check)?;
    let mut verified = manifest.clone();
    let mut elapsed_ms = 0u64;
    let mut artifacts = std::path::PathBuf::new();
    let mut capture_artifacts = Vec::new();
    for capture in captures {
        elapsed_ms = elapsed_ms.saturating_add(capture.receipt.elapsed_ms);
        if artifacts.as_os_str().is_empty() {
            artifacts = capture.receipt.artifacts.clone();
        }
        if expected > 1 {
            capture_artifacts.push(capture.receipt.artifacts.clone());
        }
        let side_effect = merge_channel(
            &mut archive,
            &mut verified,
            private_manifest,
            capture,
            game,
            control,
        )?;
        if let Some(asset) = side_effect {
            output.report.diagnostics.push(Diagnostic::info(format!("HDR capture emitted an additional validated LDR default {} (SHA-256 {}); its raw bytes remain in the HDR capture receipt and the published default comes from the LDR capture", asset.path, asset.sha256), None, None));
        }
    }
    verified.phase = CubemapAssetPhase::Verified;
    persist_manifest(&mut archive, &verified)?;
    let bsp = output.bsp.as_mut().unwrap();
    bsp.set_pak_data_controlled(&archive, PakLimits::default(), &mut check)?;
    verify_generated_cubemaps(bsp, &verified, control)?;
    if let Some(plan) = &output.water_assets {
        crate::water_assets::verify_packed_water(bsp, &plan.manifest(), control)?;
        plan.verify_sources(game, control)?;
    }
    if let Some(plan) = &output.static_prop_assets {
        crate::static_prop_assets::verify_packed_static_props(bsp, &plan.manifest(), control)?;
        plan.verify_sources(game, control)?;
    }
    verify_sources(&verified, game, control)?;
    let channel = if verified.hdr_samples.is_empty() {
        "ldr"
    } else {
        "ldr_hdr"
    };
    let channel_label = if expected == 1 { "LDR" } else { "LDR + HDR" };
    output.report.diagnostics.push(Diagnostic::info(format!("Verified {} native {channel_label} reflection samples, {} material aliases and {} packed bytes in {elapsed_ms} ms; capture evidence: {}", verified.samples.len(), verified.aliases.len(), bsp.lump(bsp::lump::PAKFILE).data.len(), artifacts.display()), None, None));
    output.stats.reflections = Some(crate::ReflectionStats {
        channel,
        verified_samples: verified.samples.len(),
        material_aliases: verified.aliases.len(),
        packed_bytes: bsp.lump(bsp::lump::PAKFILE).data.len(),
        capture_ms: elapsed_ms,
        artifacts,
        capture_artifacts,
    });
    output.cubemap_assets = Some(verified);
    Ok(output)
}

fn merge_channel(
    archive: &mut bsp::PakArchive,
    verified: &mut CubemapAssetManifest,
    private_manifest: &CubemapAssetManifest,
    capture: CapturedChannel,
    game: &GameFs,
    control: &Control,
) -> io::Result<Option<CubemapSourceAsset>> {
    let receipt = capture.receipt;
    if !receipt.verified_controls || !receipt.preservation_passed {
        return Err(invalid(
            "Native reflection capture did not verify its controls and settings preservation",
        ));
    }
    if receipt.staged_bsp != capture.expected_staging {
        return Err(invalid(
            "Native capture staged a BSP outside the registered compatibility/controller adapter",
        ));
    }
    let baseline = Bsp::parse(&receipt.staged_bsp)?;
    verify_generated_cubemaps(&baseline, private_manifest, control)?;
    let captured_pak = verify_non_pak(&receipt.staged_bsp, &receipt.bsp, &baseline, control)?;
    let mut check = || control.checkpoint().map_err(io::Error::other);
    let staged_archive = baseline.pak_data_controlled(PakLimits::default(), &mut check)?;
    let captured_archive =
        bsp::PakArchive::parse_controlled(captured_pak, PakLimits::default(), &mut check)?;
    let hdr = receipt.channel == CaptureChannel::Hdr;
    let private_samples = if hdr {
        &private_manifest.hdr_samples
    } else {
        &private_manifest.samples
    };
    let samples: BTreeSet<_> = private_samples
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
        "materials/maps/{}/cubemapdefault{}",
        private_manifest.namespace,
        receipt.channel.texture_suffix()
    );
    let ldr_default_path = format!(
        "materials/maps/{}/cubemapdefault.vtf",
        private_manifest.namespace
    );
    for name in captured_entries.keys() {
        if !staged_entries.contains_key(name)
            && *name != default_path
            && !(hdr && *name == ldr_default_path)
        {
            return Err(invalid(format!(
                "Native capture added an unregistered packed asset: {name}"
            )));
        }
    }
    let verified_samples = if hdr {
        &mut verified.hdr_samples
    } else {
        &mut verified.samples
    };
    if verified_samples.len() != private_samples.len() {
        return Err(invalid(
            "Reflection private and output sample identities differ",
        ));
    }
    for (sample, private_sample) in verified_samples.iter_mut().zip(private_samples) {
        control.checkpoint().map_err(io::Error::other)?;
        let data = captured_entries
            .get(private_sample.texture.as_str())
            .ok_or_else(|| {
                invalid(format!(
                    "Native capture omitted required reflection sample: {}",
                    private_sample.texture
                ))
            })?;
        if hash(data) == sample.bootstrap_sha256 {
            return Err(invalid(format!(
                "Native capture left an unchanged bootstrap: {}",
                sample.texture
            )));
        }
        let data = if hdr {
            super::hdr::canonicalize(data, sample.edge)?
        } else {
            data.to_vec()
        };
        if hdr {
            super::hdr::validate_sample(&data, sample, CubemapAssetPhase::Verified)?;
        } else {
            validate_sample(&data, sample, CubemapAssetPhase::Verified)?;
        }
        sample.sha256 = hash(&data);
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .ok_or_else(|| invalid("Original compiler sample is missing"))?
            .data = data;
    }
    if let Some(&data) = captured_entries.get(default_path.as_str()) {
        ensure_default_unmounted(game, &verified.namespace).map_err(invalid)?;
        ensure_default_unmounted(game, &private_manifest.namespace).map_err(invalid)?;
        if hdr {
            super::material::ensure_hdr_default_unmounted(game, &verified.namespace)
                .map_err(invalid)?;
            super::material::ensure_hdr_default_unmounted(game, &private_manifest.namespace)
                .map_err(invalid)?;
        }
        let default = Vtf::parse(data.to_vec()).map_err(|error| invalid(error.to_string()))?;
        let edge = u16::try_from(default.width)
            .map_err(|_| invalid("Native default cubemap width overflow"))?;
        if !edge.is_power_of_two() || edge > MAX_CUBE_EDGE {
            return Err(invalid(
                "Native default cubemap exceeds the supported dimensions",
            ));
        }
        let data = if hdr {
            super::hdr::canonicalize(data, edge)?
        } else {
            validate_cube(data, edge)?;
            data.to_vec()
        };
        let name = format!(
            "materials/maps/{}/cubemapdefault{}",
            verified.namespace,
            receipt.channel.texture_suffix()
        );
        if archive
            .entries
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(&name))
        {
            return Err(invalid(
                "Native default cubemap collides with an existing packed asset",
            ));
        }
        let asset = CubemapSourceAsset {
            path: name.clone(),
            sha256: hash(&data),
        };
        archive.entries.push(bsp::PakEntry { name, data });
        if hdr {
            verified.hdr_default_texture = Some(asset);
        } else {
            verified.default_texture = Some(asset);
        }
    }
    if hdr && let Some(&data) = captured_entries.get(ldr_default_path.as_str()) {
        let vtf = Vtf::parse(data.to_vec()).map_err(|error| invalid(error.to_string()))?;
        let edge = u16::try_from(vtf.width)
            .map_err(|_| invalid("Native side-effect default cubemap edge overflow"))?;
        validate_cube(data, edge)?;
        if verified.default_texture.is_none() {
            return Err(invalid(
                "HDR capture emitted an LDR default without the required LDR capture default",
            ));
        }
        return Ok(Some(CubemapSourceAsset {
            path: ldr_default_path,
            sha256: hash(data),
        }));
    }
    Ok(None)
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

fn integer(bytes: &[u8], offset: usize) -> io::Result<usize> {
    let value = i32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("Truncated captured BSP header"))?
            .try_into()
            .unwrap(),
    );
    usize::try_from(value).map_err(|_| invalid("Negative captured BSP offset/count/size"))
}

fn lump_range(bytes: &[u8], index: usize) -> io::Result<Range<usize>> {
    let header = 8 + index * 16;
    let start = integer(bytes, header)?;
    let size = integer(bytes, header + 4)?;
    let end = start
        .checked_add(size)
        .ok_or_else(|| invalid("Captured BSP lump span overflow"))?;
    if end > bytes.len() || size != 0 && start < 8 + bsp::LUMP_COUNT * 16 + 4 {
        return Err(invalid(
            "Captured BSP lump exceeds file bounds or overlaps its header",
        ));
    }
    Ok(start..end)
}

fn verify_non_pak<'a>(
    before_bytes: &[u8],
    after_bytes: &'a [u8],
    before: &Bsp,
    control: &Control,
) -> io::Result<&'a [u8]> {
    control.checkpoint().map_err(io::Error::other)?;
    let revision = 8 + bsp::LUMP_COUNT * 16;
    if before_bytes.get(..8) != after_bytes.get(..8)
        || before_bytes.get(revision..revision + 4) != after_bytes.get(revision..revision + 4)
    {
        return Err(invalid(
            "Native capture changed non-PAK BSP metadata, geometry, lighting, entities or game records",
        ));
    }
    let mut pak = &[][..];
    let mut spans = Vec::with_capacity(bsp::LUMP_COUNT);
    for index in 0..bsp::LUMP_COUNT {
        control.checkpoint().map_err(io::Error::other)?;
        let span = lump_range(after_bytes, index)?;
        let captured = &after_bytes[span.clone()];
        if !span.is_empty() {
            spans.push(span);
        }
        let header = 8 + index * 16;
        if index == bsp::lump::PAKFILE {
            if after_bytes[header + 8..header + 16] != [0; 8] {
                return Err(invalid("Captured BSP PAK must be uncompressed version 0"));
            }
            pak = captured;
            continue;
        }
        let original = &before_bytes[lump_range(before_bytes, index)?];
        if before_bytes[header + 8..header + 16] != after_bytes[header + 8..header + 16]
            || original.len() != captured.len()
            || index != bsp::lump::GAME_LUMP && original != captured
        {
            return Err(invalid(
                "Native capture changed non-PAK BSP metadata, geometry, lighting, entities or game records",
            ));
        }
    }
    spans.sort_unstable_by_key(|span| span.start);
    if spans.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(invalid("Captured BSP lump spans overlap"));
    }
    let before_game = normalized_game_lump(before_bytes, &before.game_lumps, control)?;
    let after_game = normalized_game_lump(after_bytes, &before.game_lumps, control)?;
    if before_game != after_game {
        return Err(invalid(
            "Native capture changed game-lump bytes outside absolute entry offsets",
        ));
    }
    Ok(pak)
}

fn normalized_game_lump(
    bytes: &[u8],
    expected: &[bsp::GameLump],
    control: &Control,
) -> io::Result<Vec<u8>> {
    let span = lump_range(bytes, bsp::lump::GAME_LUMP)?;
    let (start, end) = (span.start, span.end);
    let data = &bytes[span];
    let count = integer(data, 0)?;
    let directory_end = 4usize
        .checked_add(
            count
                .checked_mul(16)
                .ok_or_else(|| invalid("Game-lump count overflow"))?,
        )
        .ok_or_else(|| invalid("Game-lump directory overflow"))?;
    if count > 4096 || directory_end > data.len() || count != expected.len() {
        return Err(invalid("Invalid game-lump entry count"));
    }
    let mut spans = Vec::with_capacity(count);
    for (index, entry) in expected.iter().enumerate() {
        control.checkpoint().map_err(io::Error::other)?;
        let field = 4 + index * 16 + 8;
        let offset = integer(data, field)?;
        let length = integer(data, field + 4)?;
        let limit = offset
            .checked_add(length)
            .ok_or_else(|| invalid("Game-lump entry span overflow"))?;
        if offset < start + directory_end || limit > end {
            return Err(invalid("Game-lump entry escapes its payload/BSP"));
        }
        if bytes[offset..limit] != entry.data {
            return Err(invalid("Native capture changed game-lump payloads"));
        }
        spans.push((offset, limit));
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(invalid("Overlapping game-lump entry payloads"));
    }
    let mut normalized = data.to_vec();
    for index in 0..count {
        let field = 4 + index * 16 + 8;
        normalized[field..field + 4].fill(0);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(bytes: &mut [u8], offset: usize, value: usize) {
        bytes[offset..offset + 4].copy_from_slice(&i32::try_from(value).unwrap().to_le_bytes());
    }

    #[test]
    fn captured_layout_checks_borrowed_ranges_and_preserves_relocated_game_payloads() {
        let mut baseline = Bsp::new();
        baseline.lumps[bsp::lump::ENTITIES].data = vec![1, 2, 3, 4];
        baseline.lumps[bsp::lump::VERTEXES].data = vec![1, 2, 3, 4];
        baseline.game_lumps = [*b"test", *b"next"]
            .into_iter()
            .map(|id| bsp::GameLump {
                id,
                flags: 0,
                version: 1,
                data: vec![5, 6, 7, 8],
            })
            .collect();
        let limits = PakLimits::default();
        let mut archive = bsp::PakArchive {
            entries: vec![bsp::PakEntry {
                name: "capture.bin".into(),
                data: vec![0],
            }],
        };
        baseline.set_pak_data(&archive, limits).unwrap();
        let before = baseline.to_bytes();
        let control = Control::default();
        verify_non_pak(&before, &before, &baseline, &control).unwrap();
        archive.entries[0].data = vec![42, 43];
        let mut captured = bsp::replace_bsp_pak(&before, &archive, limits).unwrap();
        let original_game = lump_range(&captured, bsp::lump::GAME_LUMP).unwrap();
        let mut relocated = captured[original_game.clone()].to_vec();
        let new_start = captured.len().next_multiple_of(4);
        for index in 0..2 {
            let field = 4 + index * 16 + 8;
            let offset = integer(&relocated, field).unwrap();
            word(
                &mut relocated,
                field,
                new_start + offset - original_game.start,
            );
        }
        captured.resize(new_start, 0);
        captured.extend_from_slice(&relocated);
        word(&mut captured, 8 + bsp::lump::GAME_LUMP * 16, new_start);
        let pak = verify_non_pak(&before, &captured, &baseline, &control).unwrap();
        let decoded = bsp::PakArchive::parse(pak, limits).unwrap();
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].name, "capture.bin");
        assert_eq!(decoded.entries[0].data, [42, 43]);

        let game = lump_range(&captured, bsp::lump::GAME_LUMP).unwrap();
        let vertices = lump_range(&captured, bsp::lump::VERTEXES).unwrap();
        for (case, expected) in [
            ("truncated", "non-PAK"),
            ("negative", "Negative"),
            ("header_overlap", "overlaps its header"),
            ("ordinary_overlap", "lump spans overlap"),
            ("game_count", "entry count"),
            ("game_overlap", "Overlapping game-lump"),
            ("game_escape", "escapes its payload"),
            ("game_payload", "changed game-lump payloads"),
            ("game_metadata", "outside absolute entry offsets"),
            ("pak_metadata", "uncompressed version 0"),
        ] {
            let mut invalid = captured.clone();
            match case {
                "truncated" => invalid.truncate(100),
                "negative" => invalid[8..12].copy_from_slice(&(-1_i32).to_le_bytes()),
                "header_overlap" => word(&mut invalid, 8, 0),
                "ordinary_overlap" => word(&mut invalid, 8, vertices.start),
                "game_count" => word(&mut invalid, game.start, 4097),
                "game_overlap" => {
                    let first = integer(&invalid, game.start + 12).unwrap();
                    word(&mut invalid, game.start + 28, first);
                }
                "game_escape" => word(&mut invalid, game.start + 12, vertices.start),
                "game_payload" => invalid[game.end - 1] ^= 1,
                "game_metadata" => invalid[game.start + 4] ^= 1,
                "pak_metadata" => word(&mut invalid, 8 + bsp::lump::PAKFILE * 16 + 8, 1),
                _ => unreachable!(),
            }
            let error = verify_non_pak(&before, &invalid, &baseline, &control)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{case}: {error}");
        }
    }
}
