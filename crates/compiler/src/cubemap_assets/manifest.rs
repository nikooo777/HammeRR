use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::bsp::{self, Bsp, PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::asset_relative;
use source_formats::keyvalues::{self, Node, Value};
use source_formats::vmt::{self, Material};
use source_formats::vtf::{ImageFormat, Vtf};

use super::material::{
    alias_name, bound_text, ensure_writable, material_name, material_nodes, sample_texture,
    self_bottom, strings,
};
use super::*;
use crate::Control;

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn checkpoint(control: &Control) -> io::Result<()> {
    control.checkpoint().map_err(io::Error::other)
}

pub(crate) fn bootstrap_cube(edge: u16) -> Result<Vec<u8>, String> {
    if !edge.is_power_of_two() || edge > MAX_CUBE_EDGE {
        return Err("Unsupported reflection cube edge".into());
    }
    let mips = edge.ilog2() as u8 + 1;
    let mut bytes = vec![0u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7u32), (8, 2), (12, 80), (20, 0x400c)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, edge), (18, edge), (24, 1), (26, 0), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[48..52].copy_from_slice(&1f32.to_le_bytes());
    bytes[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    bytes[56] = mips;
    for mip in (0..mips).rev() {
        let size = usize::from((edge >> mip).max(1));
        for _ in 0..size * size * 7 {
            bytes.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
    Ok(bytes)
}

pub(super) fn cube(bytes: &[u8], edge: u16) -> io::Result<Vtf> {
    if bytes.len() < 80
        || bytes.len() > MAX_VTF_BYTES
        || !edge.is_power_of_two()
        || edge > MAX_CUBE_EDGE
    {
        return Err(invalid("Reflection VTF exceeds 4 MiB"));
    }
    let vtf = Vtf::parse(bytes.to_vec()).map_err(|error| invalid(error.to_string()))?;
    let integer = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if !matches!(vtf.version, (7, 2) | (7, 4))
        || vtf.authored_frames != 1
        || vtf.first_frame != 0
        || vtf.authored_depth != Some(1)
        || vtf.faces != 7
    {
        return Err(invalid(
            "Reflection VTF requires the proven seven-face native header with raw frame/depth 1 and firstFrame 0",
        ));
    }
    let header = integer(12) as usize;
    let start = if vtf.version == (7, 2) {
        if header != 80 || integer(57) != u32::MAX || bytes[61..63] != [0, 0] {
            return Err(invalid("Unsupported legacy reflection thumbnail/header"));
        }
        header
    } else {
        let count = integer(68) as usize;
        if !(1..=2).contains(&count) || header != 80 + count * 8 || header > bytes.len() {
            return Err(invalid("Invalid native reflection resource header"));
        }
        let mut high = None;
        let mut crc = false;
        for index in 0..count {
            let resource = &bytes[80 + index * 8..88 + index * 8];
            if resource[..3] == [0x30, 0, 0] && resource[3] == 0 && high.is_none() {
                high = Some(u32::from_le_bytes(resource[4..8].try_into().unwrap()) as usize);
            } else if resource[..3] == *b"CRC" && resource[3] == 2 && !crc {
                crc = true;
            } else {
                return Err(invalid(
                    "Duplicate, inline or unsupported native reflection resource",
                ));
            }
        }
        high.filter(|&offset| offset == header).ok_or_else(|| invalid("Native reflection requires one external high-resolution resource immediately after its header"))?
    };
    if vtf.width != u32::from(edge)
        || vtf.height != u32::from(edge)
        || vtf.depth != 1
        || vtf.frames != 1
        || !matches!(vtf.faces, 6 | 7)
        || vtf.flags & 0x4000 == 0
        || vtf.mip_count != edge.ilog2() as u8 + 1
        || vtf.reflectivity.iter().any(|value| !value.is_finite())
    {
        return Err(invalid(
            "Reflection VTF dimensions, frames, faces, mip chain or metadata do not match its sample",
        ));
    }
    let mut image_bytes = 0usize;
    for mip in 0..vtf.mip_count {
        for face in 0..vtf.faces {
            image_bytes = image_bytes
                .checked_add(
                    vtf.image_data(mip, 0, face)
                        .map_err(|error| invalid(error.to_string()))?
                        .len(),
                )
                .ok_or_else(|| invalid("Reflection VTF image size overflow"))?;
        }
    }
    if start.checked_add(image_bytes) != Some(bytes.len()) {
        return Err(invalid(
            "Reflection VTF image spans do not exactly fill the file",
        ));
    }
    Ok(vtf)
}

pub(super) fn validate_cube(bytes: &[u8], edge: u16) -> io::Result<()> {
    let vtf = cube(bytes, edge)?;
    if !matches!(
        vtf.format,
        ImageFormat::Rgba8888
            | ImageFormat::Abgr8888
            | ImageFormat::Rgb888
            | ImageFormat::Bgr888
            | ImageFormat::Argb8888
            | ImageFormat::Bgra8888
            | ImageFormat::Bgrx8888
            | ImageFormat::Dxt1
            | ImageFormat::Dxt3
            | ImageFormat::Dxt5
            | ImageFormat::Dxt1OneBitAlpha
    ) {
        return Err(invalid(
            "Reflection capture is not a supported LDR pixel format",
        ));
    }
    Ok(())
}

pub(super) fn validate_sample(
    bytes: &[u8],
    sample: &CubemapSampleAsset,
    phase: CubemapAssetPhase,
) -> io::Result<()> {
    validate_cube(bytes, sample.edge)?;
    let bootstrap = bootstrap_cube(sample.edge).map_err(invalid)?;
    if hash(&bootstrap) != sample.bootstrap_sha256 {
        return Err(invalid(
            "Reflection bootstrap provenance does not match its registered sample",
        ));
    }
    if phase == CubemapAssetPhase::Pending {
        if bytes != bootstrap {
            return Err(invalid(
                "Pending reflection sample differs from its registered bootstrap",
            ));
        }
    } else {
        let captured = Vtf::parse(bytes.to_vec()).map_err(|error| invalid(error.to_string()))?;
        let original = Vtf::parse(bootstrap).map_err(|error| invalid(error.to_string()))?;
        let mut changed = false;
        for face in 0..6 {
            let image = captured
                .decode_rgba8(0, 0, face)
                .map_err(|error| invalid(error.to_string()))?;
            let placeholder = original
                .decode_rgba8(0, 0, face)
                .map_err(|error| invalid(error.to_string()))?;
            changed |= image.pixels != placeholder.pixels;
        }
        if !changed {
            return Err(invalid(
                "Native reflection capture still contains bootstrap pixels",
            ));
        }
    }
    Ok(())
}

pub(crate) fn pack(
    bsp: &mut Bsp,
    manifest: &mut CubemapAssetManifest,
    entries: Vec<PakEntry>,
    control: &Control,
) -> io::Result<()> {
    let (texinfo_bindings, face_bindings) = actual_bindings(bsp, manifest)?;
    manifest.texinfo_bindings = texinfo_bindings;
    manifest.face_bindings = face_bindings;
    let limits = PakLimits::default();
    let mut check = || checkpoint(control);
    let mut archive = bsp.pak_data_controlled(limits, &mut check)?;
    let existing: BTreeSet<_> = archive
        .entries
        .iter()
        .map(|entry| entry.name.to_ascii_lowercase())
        .collect();
    if existing.contains(MANIFEST_PATH)
        || entries
            .iter()
            .any(|entry| existing.contains(&entry.name.to_ascii_lowercase()))
    {
        return Err(invalid(
            "Generated reflection assets collide with existing packed content",
        ));
    }
    archive.entries.extend(entries);
    persist_manifest(&mut archive, manifest)?;
    bsp.set_pak_data_controlled(&archive, limits, &mut check)?;
    verify_generated_cubemaps(bsp, manifest, control)
}

pub(super) fn persist_manifest(
    archive: &mut PakArchive,
    manifest: &CubemapAssetManifest,
) -> io::Result<()> {
    let data = serde_json::to_vec(manifest).map_err(io::Error::other)?;
    if data.len() > MAX_MATERIAL_BYTES {
        return Err(invalid("Reflection manifest exceeds its 4 MiB limit"));
    }
    archive.entries.retain(|entry| entry.name != MANIFEST_PATH);
    archive.entries.push(PakEntry {
        name: MANIFEST_PATH.into(),
        data,
    });
    Ok(())
}

pub fn read_generated_cubemaps(
    bsp: &Bsp,
    control: &Control,
) -> io::Result<Option<CubemapAssetManifest>> {
    let mut check = || checkpoint(control);
    let archive = bsp.pak_data_controlled(PakLimits::default(), &mut check)?;
    let Some(entry) = archive
        .entries
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(MANIFEST_PATH))
    else {
        return Ok(None);
    };
    if entry.name != MANIFEST_PATH || entry.data.len() > MAX_MATERIAL_BYTES {
        return Err(invalid("Invalid reflection manifest path or size"));
    }
    let manifest: CubemapAssetManifest =
        serde_json::from_slice(&entry.data).map_err(invalid_json)?;
    verify_generated_cubemaps(bsp, &manifest, control)?;
    Ok(Some(manifest))
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    invalid(format!("Invalid reflection manifest: {error}"))
}

fn validate_manifest(manifest: &CubemapAssetManifest) -> io::Result<()> {
    CompileContext::new(&manifest.namespace).map_err(invalid)?;
    CompileContext::new(&manifest.logical_namespace).map_err(invalid)?;
    let legacy = manifest.version == 1
        && manifest.channel == "ldr"
        && manifest.hdr_samples.is_empty()
        && manifest.hdr_default_texture.is_none();
    let dual = manifest.version == 2
        && manifest.channel == "ldr_hdr"
        && manifest.hdr_samples.len() == manifest.samples.len();
    if (!legacy && !dual)
        || manifest.samples.is_empty()
        || manifest.samples.len() > MAX_SAMPLES
        || manifest.aliases.len() > MAX_ALIASES
        || manifest.assignments.len() > MAX_ASSIGNMENT_WORK
        || manifest.sources.len() > 4096
        || manifest.missing_sources.len() > 4096
        || manifest.sources.len() + manifest.missing_sources.len() > 4096
        || manifest.texinfo_bindings.len() > i16::MAX as usize + 1
        || manifest.face_bindings.len() > 196_608
    {
        return Err(invalid("Unsupported or oversized reflection manifest"));
    }
    let mut samples = BTreeSet::new();
    let mut helpers = BTreeSet::new();
    for sample in &manifest.samples {
        let edge = if sample.size_enum == 0 {
            128
        } else {
            1u16.checked_shl(u32::from(sample.size_enum - 1))
                .unwrap_or(0)
        };
        if sample.helper_id == 0
            || !samples.insert(sample.origin)
            || !helpers.insert(sample.helper_id)
            || sample.edge != edge
            || !edge.is_power_of_two()
            || edge > MAX_CUBE_EDGE
            || sample.texture != sample_texture(&manifest.namespace, sample.origin)
            || !is_hash(&sample.sha256)
            || !is_hash(&sample.bootstrap_sha256)
        {
            return Err(invalid(
                "Reflection sample manifest is invalid or ambiguous",
            ));
        }
        if manifest.phase == CubemapAssetPhase::Verified && sample.sha256 == sample.bootstrap_sha256
        {
            return Err(invalid("Verified reflections still contain a bootstrap"));
        }
    }
    for (sample, hdr_sample) in manifest.samples.iter().zip(&manifest.hdr_samples) {
        if sample.helper_id != hdr_sample.helper_id
            || sample.origin != hdr_sample.origin
            || sample.size_enum != hdr_sample.size_enum
            || sample.edge != hdr_sample.edge
            || hdr_sample.texture
                != super::material::hdr_sample_texture(&manifest.namespace, sample.origin)
            || !is_hash(&hdr_sample.sha256)
            || !is_hash(&hdr_sample.bootstrap_sha256)
            || (manifest.phase == CubemapAssetPhase::Verified
                && hdr_sample.sha256 == hdr_sample.bootstrap_sha256)
        {
            return Err(invalid(
                "HDR reflection manifest requires complete matching sample pairs with separate capture provenance",
            ));
        }
    }
    let mut aliases = BTreeSet::new();
    for alias in &manifest.aliases {
        let sample = manifest
            .samples
            .get(alias.sample)
            .ok_or_else(|| invalid("Reflection alias references a missing sample"))?;
        if material_name(&alias.material).map_err(invalid)? != alias.material
            || alias.alias
                != alias_name(&manifest.namespace, &alias.material, sample.origin)
                    .map_err(invalid)?
            || !aliases.insert(&alias.alias)
            || !is_hash(&alias.sha256)
            || alias.dependencies.len() > MAX_ALIASES
            || alias.envmaps.len() > MAX_ALIASES
            || alias
                .envmaps
                .iter()
                .any(|name| name.eq_ignore_ascii_case("env_cubemap"))
        {
            return Err(invalid("Invalid or duplicate reflection material alias"));
        }
    }
    if manifest.default_texture.as_ref().is_some_and(|asset| {
        asset.path != format!("materials/maps/{}/cubemapdefault.vtf", manifest.namespace)
            || !is_hash(&asset.sha256)
    }) {
        return Err(invalid("Invalid native default reflection asset"));
    }
    if manifest.hdr_default_texture.as_ref().is_some_and(|asset| {
        asset.path
            != format!(
                "materials/maps/{}/cubemapdefault.hdr.vtf",
                manifest.namespace
            )
            || !is_hash(&asset.sha256)
    }) {
        return Err(invalid("Invalid native HDR default reflection asset"));
    }
    let mut faces = BTreeSet::new();
    let by_alias: BTreeMap<_, _> = manifest
        .aliases
        .iter()
        .map(|alias| (alias.alias.as_str(), alias))
        .collect();
    for assignment in &manifest.assignments {
        if assignment.face_id == 0
            || !faces.insert(assignment.face_id)
            || !helpers.contains(&assignment.helper_id)
            || material_name(&assignment.material).map_err(invalid)? != assignment.material
            || (assignment.alias != assignment.material && !aliases.contains(&assignment.alias))
        {
            return Err(invalid("Invalid reflection source-face assignment"));
        }
        if assignment.alias != assignment.material {
            let alias = by_alias[assignment.alias.as_str()];
            if alias.material != assignment.material
                || manifest.samples[alias.sample].helper_id != assignment.helper_id
            {
                return Err(invalid(
                    "Reflection assignment does not match its material/sample provenance",
                ));
            }
        }
    }
    let mut sources = BTreeSet::new();
    for source in &manifest.sources {
        if asset_relative(&source.path).map_err(|error| invalid(error.to_string()))? != source.path
            || !sources.insert(&source.path)
            || !is_hash(&source.sha256)
        {
            return Err(invalid("Invalid reflection source-asset receipt"));
        }
    }
    for path in &manifest.missing_sources {
        if path.is_empty()
            || asset_relative(path).map_err(|error| invalid(error.to_string()))? != *path
            || !sources.insert(path)
        {
            return Err(invalid("Invalid missing reflection source-asset receipt"));
        }
    }
    Ok(())
}

fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn actual_bindings(
    bsp: &Bsp,
    manifest: &CubemapAssetManifest,
) -> io::Result<(Vec<CubemapTexInfoBinding>, Vec<CubemapFaceBinding>)> {
    for (lump, maximum) in [
        (bsp::lump::TEXINFO, 32_768 * 72),
        (bsp::lump::TEXDATA, 2048 * 32),
        (bsp::lump::FACES, 65_536 * 56),
        (bsp::lump::ORIGINALFACES, 65_536 * 56),
        (bsp::lump::BRUSHSIDES, 65_536 * 8),
        (bsp::lump::TEXDATA_STRING_TABLE, 65_536 * 4),
        (bsp::lump::TEXDATA_STRING_DATA, MAX_MATERIAL_BYTES),
    ] {
        if bsp.lump(lump).data.len() > maximum {
            return Err(invalid(
                "Reflection BSP texture/face record budget exceeded",
            ));
        }
    }
    let names = bsp.texdata_strings()?;
    let data = bsp.records::<bsp::TexData>()?;
    let infos = bsp.records::<bsp::TexInfo>()?;
    let aliases: BTreeSet<_> = manifest
        .aliases
        .iter()
        .map(|alias| alias.alias.as_str())
        .collect();
    let mut texinfo_bindings = Vec::new();
    let mut bound = BTreeSet::new();
    for (index, info) in infos.iter().enumerate() {
        let data = data
            .get(
                usize::try_from(info.texdata)
                    .map_err(|_| invalid("Invalid reflection texdata index"))?,
            )
            .ok_or_else(|| invalid("Missing reflection texdata record"))?;
        let name = names
            .get(
                usize::try_from(data.name_index)
                    .map_err(|_| invalid("Invalid reflection texture-name index"))?,
            )
            .ok_or_else(|| invalid("Missing reflection texture-name record"))?
            .to_ascii_lowercase();
        if aliases.contains(name.as_str()) {
            let mut bytes = Vec::with_capacity(72);
            for value in info
                .texture_vecs
                .iter()
                .chain(&info.lightmap_vecs)
                .flatten()
            {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            bytes.extend_from_slice(&info.flags.to_le_bytes());
            bytes.extend_from_slice(&info.texdata.to_le_bytes());
            texinfo_bindings.push(CubemapTexInfoBinding {
                index,
                alias: name,
                sha256: hash(&bytes),
            });
            bound.insert(
                i16::try_from(index)
                    .map_err(|_| invalid("Reflection texinfo index exceeds i16"))?,
            );
        }
    }
    let mut faces = Vec::new();
    for lump in [bsp::lump::FACES, bsp::lump::ORIGINALFACES] {
        for (index, face) in bsp.values::<bsp::Face>(lump)?.iter().enumerate() {
            if bound.contains(&face.texinfo) {
                faces.push(CubemapFaceBinding {
                    lump,
                    index,
                    texinfo: face.texinfo,
                });
            }
        }
    }
    for (index, side) in bsp.records::<bsp::BrushSide>()?.iter().enumerate() {
        if bound.contains(&side.texinfo) {
            faces.push(CubemapFaceBinding {
                lump: bsp::lump::BRUSHSIDES,
                index,
                texinfo: side.texinfo,
            });
        }
    }
    Ok((texinfo_bindings, faces))
}

pub fn verify_generated_cubemaps(
    bsp: &Bsp,
    manifest: &CubemapAssetManifest,
    control: &Control,
) -> io::Result<()> {
    checkpoint(control)?;
    validate_manifest(manifest)?;
    let mut check = || checkpoint(control);
    let archive = bsp.pak_data_controlled(PakLimits::default(), &mut check)?;
    let entries: BTreeMap<_, _> = archive
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect();
    let encoded = entries
        .get(MANIFEST_PATH)
        .ok_or_else(|| invalid("Reflection manifest is absent from the PAK"))?;
    if encoded.len() > MAX_MATERIAL_BYTES
        || serde_json::from_slice::<CubemapAssetManifest>(encoded).map_err(invalid_json)?
            != *manifest
    {
        return Err(invalid(
            "Reflection manifest disagrees with the BSP artifact",
        ));
    }
    let cubes = bsp.records::<bsp::Cubemap>()?;
    if cubes.len() != manifest.samples.len()
        || cubes
            .iter()
            .zip(&manifest.samples)
            .any(|(cube, sample)| cube.origin != sample.origin || cube.size != sample.size_enum)
    {
        return Err(invalid(
            "BSP cubemap samples do not match the generated manifest",
        ));
    }
    let (texinfo_bindings, face_bindings) = actual_bindings(bsp, manifest)?;
    if texinfo_bindings != manifest.texinfo_bindings || face_bindings != manifest.face_bindings {
        return Err(invalid(
            "Generated reflection face/brush-side/texinfo references disagree with the manifest",
        ));
    }
    if bsp.lump(bsp::lump::TEXDATA_STRING_TABLE).data.len() > 65_536 * 4
        || bsp.lump(bsp::lump::TEXDATA_STRING_DATA).data.len() > MAX_MATERIAL_BYTES
    {
        return Err(invalid("Reflection BSP texture string budget exceeded"));
    }
    let names: BTreeSet<_> = bsp
        .texdata_strings()?
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let aliases: BTreeMap<_, _> = manifest
        .aliases
        .iter()
        .map(|alias| (alias.alias.as_str(), alias))
        .collect();
    let sample_targets: BTreeSet<_> = manifest
        .samples
        .iter()
        .map(|sample| {
            sample
                .texture
                .strip_prefix("materials/")
                .unwrap()
                .strip_suffix(".vtf")
                .unwrap()
        })
        .collect();
    let source_paths: BTreeSet<_> = manifest
        .sources
        .iter()
        .map(|source| source.path.as_str())
        .collect();
    let mut bytes = 0usize;
    for sample in &manifest.samples {
        checkpoint(control)?;
        let data = entries.get(sample.texture.as_str()).ok_or_else(|| {
            invalid(format!(
                "Missing generated reflection sample: {}",
                sample.texture
            ))
        })?;
        if hash(data) != sample.sha256 {
            return Err(invalid(format!(
                "Reflection sample hash mismatch: {}",
                sample.texture
            )));
        }
        validate_sample(data, sample, manifest.phase)?;
        bytes = bytes
            .checked_add(data.len())
            .ok_or_else(|| invalid("Reflection byte count overflow"))?;
    }
    for sample in &manifest.hdr_samples {
        checkpoint(control)?;
        let data = entries.get(sample.texture.as_str()).ok_or_else(|| {
            invalid(format!(
                "Missing generated HDR reflection sample: {}",
                sample.texture
            ))
        })?;
        if hash(data) != sample.sha256 {
            return Err(invalid(format!(
                "HDR reflection sample hash mismatch: {}",
                sample.texture
            )));
        }
        super::hdr::validate_sample(data, sample, manifest.phase)?;
        bytes = bytes
            .checked_add(data.len())
            .ok_or_else(|| invalid("Reflection byte count overflow"))?;
    }
    for alias in &manifest.aliases {
        checkpoint(control)?;
        let path = vmt::material_path(&alias.alias);
        let data = entries
            .get(path.as_str())
            .ok_or_else(|| invalid(format!("Missing generated reflection material: {path}")))?;
        if data.len() > MAX_MATERIAL_BYTES || hash(data) != alias.sha256 {
            return Err(invalid(format!(
                "Reflection material hash mismatch: {path}"
            )));
        }
        let text = std::str::from_utf8(data).map_err(|error| invalid(error.to_string()))?;
        bound_text(text).map_err(invalid)?;
        let material = Material::parse(text).map_err(|error| invalid(error.to_string()))?;
        let nodes = material_nodes(&material);
        let target = manifest.samples[alias.sample]
            .texture
            .strip_prefix("materials/")
            .unwrap()
            .strip_suffix(".vtf")
            .unwrap();
        let mut dependencies = BTreeSet::new();
        let mut envmaps = BTreeSet::new();
        for (key, value) in strings(&nodes) {
            if key.eq_ignore_ascii_case("$envmap") {
                if value.eq_ignore_ascii_case("env_cubemap")
                    || (sample_targets.contains(value) && value != target)
                {
                    return Err(invalid(format!(
                        "Generated reflection material has an unresolved/mismatched envmap: {path}"
                    )));
                }
                envmaps.insert(value.to_owned());
            }
            if ["$bottommaterial", "$crackmaterial", "$fallbackmaterial"]
                .iter()
                .any(|dependency| key.eq_ignore_ascii_case(dependency))
                && !value.trim().is_empty()
            {
                let name = material_name(value).map_err(invalid)?;
                if name == alias.alias && !self_bottom(&alias.alias, key, value) {
                    return Err(invalid(
                        "Only an exact below-view self reference may name its own reflection alias",
                    ));
                }
                if let Some(dependency) = aliases.get(name.as_str()) {
                    if dependency.sample != alias.sample {
                        return Err(invalid("Reflection dependency uses a different sample"));
                    }
                } else if !source_paths.contains(vmt::material_path(&name).as_str()) {
                    return Err(invalid(
                        "Reflection dependency has no registered source-material receipt",
                    ));
                }
                dependencies.insert(name);
            }
        }
        if dependencies.into_iter().collect::<Vec<_>>() != alias.dependencies {
            return Err(invalid(
                "Reflection dependency references disagree with the manifest",
            ));
        }
        if envmaps.into_iter().collect::<Vec<_>>() != alias.envmaps {
            return Err(invalid(
                "Reflection envmap references disagree with the manifest",
            ));
        }
        bytes = bytes
            .checked_add(data.len())
            .ok_or_else(|| invalid("Reflection byte count overflow"))?;
    }
    for default in manifest
        .default_texture
        .iter()
        .chain(&manifest.hdr_default_texture)
    {
        let data = entries
            .get(default.path.as_str())
            .ok_or_else(|| invalid("Missing registered native default cubemap"))?;
        if hash(data) != default.sha256 {
            return Err(invalid("Native default cubemap hash mismatch"));
        }
        let vtf = Vtf::parse(data.to_vec()).map_err(|error| invalid(error.to_string()))?;
        let edge = u16::try_from(vtf.width)
            .map_err(|_| invalid("Native default cubemap edge overflow"))?;
        if !edge.is_power_of_two() || edge > MAX_CUBE_EDGE {
            return Err(invalid("Unsupported native default cubemap edge"));
        }
        if default.path.ends_with(".hdr.vtf") {
            super::hdr::validate_cube(data, edge)?;
        } else {
            validate_cube(data, edge)?;
        }
        bytes = bytes
            .checked_add(data.len())
            .ok_or_else(|| invalid("Reflection byte count overflow"))?;
    }
    if bytes > MAX_GENERATED_BYTES {
        return Err(invalid("Generated reflection assets exceed 64 MiB"));
    }
    for assignment in &manifest.assignments {
        if assignment.alias != assignment.material && !names.contains(&assignment.alias) {
            return Err(invalid(
                "Assigned reflection alias is absent from BSP texture data",
            ));
        }
    }
    Ok(())
}

pub fn rebase_generated_cubemaps(
    bytes: &[u8],
    new_namespace: &str,
    control: &Control,
) -> io::Result<Vec<u8>> {
    let context = CompileContext::new(new_namespace).map_err(invalid)?;
    let bsp = Bsp::parse(bytes)?;
    let Some(manifest) = read_generated_cubemaps(&bsp, control)? else {
        return Ok(bytes.to_vec());
    };
    if manifest.phase != CubemapAssetPhase::Verified {
        return Err(invalid(
            "Pending reflection assets cannot be staged for ordinary Run",
        ));
    }
    rebase(bytes, &manifest, &context, control).map(|(bytes, _)| bytes)
}

pub(super) fn rebase(
    bytes: &[u8],
    manifest: &CubemapAssetManifest,
    context: &CompileContext,
    control: &Control,
) -> io::Result<(Vec<u8>, CubemapAssetManifest)> {
    let bsp = Bsp::parse(bytes)?;
    verify_generated_cubemaps(&bsp, manifest, control)?;
    if context.namespace() == manifest.namespace {
        return Ok((bytes.to_vec(), manifest.clone()));
    }
    let mut check = || checkpoint(control);
    let mut archive = bsp.pak_data_controlled(PakLimits::default(), &mut check)?;
    let mut next = manifest.clone();
    next.namespace = context.namespace().into();
    let mut references = BTreeMap::new();
    let mut alias_references = BTreeMap::new();
    for (old, sample) in manifest
        .samples
        .iter()
        .chain(&manifest.hdr_samples)
        .zip(next.samples.iter_mut().chain(&mut next.hdr_samples))
    {
        sample.texture = if old.texture.ends_with(".hdr.vtf") {
            super::material::hdr_sample_texture(context.namespace(), sample.origin)
        } else {
            sample_texture(context.namespace(), sample.origin)
        };
        references.insert(
            old.texture
                .strip_prefix("materials/")
                .unwrap()
                .strip_suffix(".vtf")
                .unwrap()
                .to_owned(),
            sample
                .texture
                .strip_prefix("materials/")
                .unwrap()
                .strip_suffix(".vtf")
                .unwrap()
                .to_owned(),
        );
    }
    for alias in &mut next.aliases {
        let old = alias.alias.clone();
        alias.alias = alias_name(
            context.namespace(),
            &alias.material,
            next.samples[alias.sample].origin,
        )
        .map_err(invalid)?;
        alias_references.insert(old, alias.alias.clone());
    }
    let mut renamed = BTreeMap::new();
    for (old, sample) in manifest
        .samples
        .iter()
        .chain(&manifest.hdr_samples)
        .zip(next.samples.iter().chain(&next.hdr_samples))
    {
        renamed.insert(old.texture.clone(), sample.texture.clone());
    }
    for (old, alias) in manifest.aliases.iter().zip(&mut next.aliases) {
        let path = vmt::material_path(&old.alias);
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == path)
            .ok_or_else(|| invalid("Missing registered alias during rebase"))?;
        let text = std::str::from_utf8(&entry.data).map_err(|error| invalid(error.to_string()))?;
        bound_text(text).map_err(invalid)?;
        let material = Material::parse(text).map_err(|error| invalid(error.to_string()))?;
        let mut nodes = material_nodes(&material);
        rebase_nodes(&mut nodes, &references, &alias_references);
        ensure_writable(&nodes).map_err(invalid)?;
        entry.data = keyvalues::try_write(
            &[Node::block(material.shader, nodes)],
            &keyvalues::WriteOptions::default(),
        )
        .map_err(|error| invalid(error.to_string()))?
        .into_bytes();
        alias.sha256 = hash(&entry.data);
        alias.dependencies = alias
            .dependencies
            .iter()
            .map(|dependency| {
                alias_references
                    .get(dependency)
                    .unwrap_or(dependency)
                    .clone()
            })
            .collect();
        alias.envmaps = alias
            .envmaps
            .iter()
            .map(|texture| references.get(texture).unwrap_or(texture).clone())
            .collect();
        alias.dependencies.sort_unstable();
        alias.dependencies.dedup();
        alias.envmaps.sort_unstable();
        alias.envmaps.dedup();
        renamed.insert(path, vmt::material_path(&alias.alias));
    }
    for assignment in &mut next.assignments {
        if let Some(alias) = alias_references.get(&assignment.alias) {
            assignment.alias = alias.clone();
        }
    }
    for binding in &mut next.texinfo_bindings {
        binding.alias = alias_references
            .get(&binding.alias)
            .ok_or_else(|| invalid("Registered reflection texinfo alias is missing during rebase"))?
            .clone();
    }
    for entry in &mut archive.entries {
        if let Some(name) = renamed.get(&entry.name) {
            entry.name = name.clone();
        }
    }
    for default in manifest
        .default_texture
        .iter()
        .chain(&manifest.hdr_default_texture)
    {
        for entry in &mut archive.entries {
            if entry.name == default.path {
                let suffix = if default.path.ends_with(".hdr.vtf") {
                    ".hdr.vtf"
                } else {
                    ".vtf"
                };
                entry.name = format!("materials/maps/{}/cubemapdefault{suffix}", next.namespace);
            }
        }
    }
    for default in next
        .default_texture
        .iter_mut()
        .chain(&mut next.hdr_default_texture)
    {
        let suffix = if default.path.ends_with(".hdr.vtf") {
            ".hdr.vtf"
        } else {
            ".vtf"
        };
        default.path = format!("materials/maps/{}/cubemapdefault{suffix}", next.namespace);
    }
    persist_manifest(&mut archive, &next)?;
    let mut names = bsp.texdata_strings()?;
    for name in &mut names {
        if let Some((_, alias)) = alias_references
            .iter()
            .find(|(old, _)| old.eq_ignore_ascii_case(name))
        {
            *name = alias.to_ascii_uppercase();
        }
    }
    let mut patched = bytes.to_vec();
    let mut strings = Vec::new();
    let mut table = Vec::new();
    for name in names {
        table.extend_from_slice(
            &(i32::try_from(strings.len())
                .map_err(|_| invalid("Texture string offset overflow"))?)
            .to_le_bytes(),
        );
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
    }
    append_lump(&mut patched, bsp::lump::TEXDATA_STRING_DATA, &strings)?;
    append_lump(&mut patched, bsp::lump::TEXDATA_STRING_TABLE, &table)?;
    let result =
        bsp::replace_bsp_pak_controlled(&patched, &archive, PakLimits::default(), &mut check)?;
    verify_generated_cubemaps(&Bsp::parse(&result)?, &next, control)?;
    Ok((result, next))
}

fn append_lump(bytes: &mut Vec<u8>, index: usize, payload: &[u8]) -> io::Result<()> {
    let offset = bytes
        .len()
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or_else(|| invalid("BSP append offset overflow"))?;
    let end = offset
        .checked_add(payload.len())
        .ok_or_else(|| invalid("BSP append size overflow"))?;
    if end > PakLimits::default().max_bsp_bytes {
        return Err(invalid("Rebased BSP exceeds its byte limit"));
    }
    let offset = i32::try_from(offset).map_err(|_| invalid("BSP append offset overflow"))?;
    let length = i32::try_from(payload.len()).map_err(|_| invalid("BSP append length overflow"))?;
    bytes.resize(offset as usize, 0);
    bytes.extend_from_slice(payload);
    let header = 8 + index * 16;
    bytes[header..header + 4].copy_from_slice(&offset.to_le_bytes());
    bytes[header + 4..header + 8].copy_from_slice(&length.to_le_bytes());
    Ok(())
}

fn rebase_nodes(
    nodes: &mut [Node],
    textures: &BTreeMap<String, String>,
    materials: &BTreeMap<String, String>,
) {
    for node in nodes {
        match &mut node.value {
            Value::String(value) if node.key.eq_ignore_ascii_case("$envmap") => {
                if let Some(next) = textures.get(value) {
                    *value = next.clone();
                }
            }
            Value::String(value)
                if ["$bottommaterial", "$crackmaterial", "$fallbackmaterial"]
                    .iter()
                    .any(|key| node.key.eq_ignore_ascii_case(key)) =>
            {
                if let Some(next) = materials.get(value) {
                    *value = next.clone();
                }
            }
            Value::Block(children) => rebase_nodes(children, textures, materials),
            _ => {}
        }
    }
}
