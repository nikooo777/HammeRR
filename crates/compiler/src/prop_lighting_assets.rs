use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Write};
use std::marker::PhantomData;

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use source_formats::bsp::{self, Bsp, Lump, PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::{AssetSourcePaths, GameFs, asset_relative};
use source_formats::{mdl, vhv};

use crate::Control;
use crate::asset_sources::{AssetSourceGuard, SourceGuardPolicy, capture_asset_source_guard, hash};

pub const MANIFEST_PATH: &str = "hammerr/generated-prop-lighting.json";
pub const LDR_MAP_FLAG: u32 = 1;
pub const HDR_MAP_FLAG: u32 = 2;
pub const MAX_RECEIVER_MESHES: usize = 256;
const MAP_FLAGS: u32 = LDR_MAP_FLAG | HDR_MAP_FLAG;
const DISABLE_VERTEX_LIGHTING: u32 = 0x40;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_RECEIVERS: usize = 4095;
const MAX_SOURCES: usize = 4096;
const MAX_MISSING: usize = 4096;
const MAX_MESHES: usize = 65_536;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PHYSICAL_PATHS: usize = 8192;
const MAX_CHANNELS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Ldr,
    Hdr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAsset {
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshLayout {
    pub lod: u32,
    pub vertex_count: u32,
}

impl From<vhv::MeshLayout> for MeshLayout {
    fn from(layout: vhv::MeshLayout) -> Self {
        Self {
            lod: layout.lod,
            vertex_count: layout.vertex_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receiver {
    pub ordinal: u32,
    pub model: String,
    pub checksum: u32,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_MESHES>")]
    pub meshes: Vec<MeshLayout>,
}

#[derive(Debug)]
pub struct GeneratedAsset {
    pub ordinal: u32,
    pub channel: Channel,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedAsset {
    pub ordinal: u32,
    pub channel: Channel,
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropLightingManifest {
    pub version: u32,
    pub static_props_version: u16,
    pub static_props_sha256: String,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_CHANNELS>")]
    pub channels: Vec<Channel>,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_RECEIVERS>")]
    pub receivers: Vec<Receiver>,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_RECEIVERS>")]
    pub assets: Vec<PackedAsset>,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_SOURCES>")]
    pub sources: Vec<SourceAsset>,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_MISSING>")]
    pub missing_sources: Vec<String>,
}

#[derive(Debug)]
struct Lookup {
    path: String,
    paths: AssetSourcePaths,
    missing: bool,
}

#[derive(Debug)]
pub struct PropLightingPlan {
    manifest: PropLightingManifest,
    entries: Vec<PakEntry>,
    lookups: Vec<Lookup>,
    guard: AssetSourceGuard,
}

impl PropLightingPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bsp: &Bsp,
        channels: Vec<Channel>,
        receivers: Vec<Receiver>,
        generated: Vec<GeneratedAsset>,
        sources: Vec<SourceAsset>,
        missing_sources: Vec<String>,
        game: &GameFs,
        control: &Control,
    ) -> io::Result<Self> {
        checkpoint(control)?;
        if generated.len() > MAX_RECEIVERS {
            return Err(invalid("Generated prop lighting exceeds its entry budget"));
        }
        let sprp = sprp(bsp)?;
        let mut manifest = PropLightingManifest {
            version: 1,
            static_props_version: sprp.version,
            static_props_sha256: hash(&sprp.data),
            channels,
            receivers,
            assets: Vec::new(),
            sources,
            missing_sources,
        };
        validate_metadata(&manifest, false)?;
        let mut entries = Vec::new();
        let mut total = 0_usize;
        for asset in generated {
            checkpoint(control)?;
            if asset.data.len() > PakLimits::default().max_entry_bytes {
                return Err(invalid("Generated VHV exceeds 16 MiB"));
            }
            total = total
                .checked_add(asset.data.len())
                .filter(|&bytes| bytes <= PakLimits::default().max_total_bytes)
                .ok_or_else(|| invalid("Generated prop lighting exceeds 64 MiB"))?;
            let path = asset_path(asset.ordinal, asset.channel);
            manifest.assets.push(PackedAsset {
                ordinal: asset.ordinal,
                channel: asset.channel,
                path: path.clone(),
                sha256: hash(&asset.data),
                bytes: asset.data.len(),
            });
            entries.push(PakEntry {
                name: path,
                data: asset.data,
            });
        }
        manifest
            .assets
            .sort_by_key(|asset| (asset.ordinal, asset.channel));
        validate_metadata(&manifest, true)?;
        let encoded_manifest = manifest_bytes(&manifest)?;
        let mut check = || checkpoint(control);
        let mut archive = bsp.pak_data_controlled(PakLimits::default(), &mut check)?;
        append(&mut archive, &entries, encoded_manifest, control)?;
        let flags = (map_flags(bsp)? & !MAP_FLAGS) | channel_flags(&manifest.channels);
        verify_archive(bsp, &archive, &manifest, flags, control)?;
        let encoded = archive.to_bytes_controlled(PakLimits::default(), &mut check)?;
        verify_bsp_size(bsp, encoded.len(), control)?;
        let lookups = capture_lookups(&manifest, game, control)?;
        let guard = source_guard(&manifest, game, control)?;
        verify_lookups(&lookups, game, control)?;
        guard.verify_metadata()?;
        Ok(Self {
            manifest,
            entries,
            lookups,
            guard,
        })
    }

    pub fn manifest(&self) -> &PropLightingManifest {
        &self.manifest
    }

    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }

    pub fn verify_source_metadata(&self) -> io::Result<()> {
        self.guard.verify_metadata()
    }

    pub(crate) fn source_guard(&self) -> AssetSourceGuard {
        self.guard.clone()
    }

    pub fn capture_source_guard(
        &self,
        game: &GameFs,
        control: &Control,
    ) -> io::Result<AssetSourceGuard> {
        checkpoint(control)?;
        self.guard.verify_metadata()?;
        verify_lookups(&self.lookups, game, control)?;
        let guard = source_guard(&self.manifest, game, control)?;
        verify_lookups(&self.lookups, game, control)?;
        self.guard.verify_metadata()?;
        guard.verify_metadata()?;
        Ok(guard)
    }

    pub fn verify_sources(&self, game: &GameFs, control: &Control) -> io::Result<()> {
        self.capture_source_guard(game, control).map(|_| ())
    }

    pub fn attach(&self, bsp: &mut Bsp, control: &Control) -> io::Result<()> {
        checkpoint(control)?;
        self.verify_source_metadata()?;
        let mut check = || checkpoint(control);
        let mut archive = bsp.pak_data_controlled(PakLimits::default(), &mut check)?;
        append(
            &mut archive,
            &self.entries,
            manifest_bytes(&self.manifest)?,
            control,
        )?;
        let flags = (map_flags(bsp)? & !MAP_FLAGS) | channel_flags(&self.manifest.channels);
        verify_archive(bsp, &archive, &self.manifest, flags, control)?;
        let encoded = archive.to_bytes_controlled(PakLimits::default(), &mut check)?;
        verify_bsp_size(bsp, encoded.len(), control)?;
        self.verify_source_metadata()?;
        let flag_data = flags.to_le_bytes().to_vec();
        checkpoint(control)?;
        bsp.lumps[bsp::lump::PAKFILE] = Lump {
            data: encoded,
            ..Lump::default()
        };
        bsp.lumps[bsp::lump::MAP_FLAGS] = Lump {
            data: flag_data,
            ..Lump::default()
        };
        Ok(())
    }
}

pub fn read_generated_prop_lighting(
    bsp: &Bsp,
    control: &Control,
) -> io::Result<Option<PropLightingManifest>> {
    checkpoint(control)?;
    check_lumps(bsp)?;
    let mut check = || checkpoint(control);
    let archive = bsp.pak_data_controlled(PakLimits::default(), &mut check)?;
    let Some(entry) = archive
        .entries
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(MANIFEST_PATH))
    else {
        if archive
            .entries
            .iter()
            .any(|entry| generated_name(&entry.name))
            || map_flags(bsp)? & MAP_FLAGS != 0
        {
            return Err(invalid("Generated prop lighting has no manifest"));
        }
        return Ok(None);
    };
    if entry.name != MANIFEST_PATH || entry.data.len() > MAX_MANIFEST_BYTES {
        return Err(invalid(
            "Invalid generated prop lighting manifest path or size",
        ));
    }
    let manifest: PropLightingManifest = serde_json::from_slice(&entry.data)
        .map_err(|error| invalid(format!("Invalid generated prop lighting manifest: {error}")))?;
    verify_archive(bsp, &archive, &manifest, map_flags(bsp)?, control)?;
    verify_bsp_size(bsp, bsp.lumps[bsp::lump::PAKFILE].data.len(), control)?;
    Ok(Some(manifest))
}

pub fn verify_generated_prop_lighting(
    bsp: &Bsp,
    expected: &PropLightingManifest,
    control: &Control,
) -> io::Result<()> {
    let actual = read_generated_prop_lighting(bsp, control)?
        .ok_or_else(|| invalid("Generated prop lighting manifest is missing"))?;
    if &actual != expected {
        return Err(invalid(
            "Generated prop lighting differs from its retained plan",
        ));
    }
    Ok(())
}

fn validate_metadata(manifest: &PropLightingManifest, assets: bool) -> io::Result<()> {
    if manifest.version != 1
        || !matches!(
            manifest.channels.as_slice(),
            [Channel::Ldr] | [Channel::Ldr, Channel::Hdr]
        )
    {
        return Err(invalid(
            "Unsupported generated prop lighting version or channels",
        ));
    }
    if manifest.receivers.is_empty()
        || manifest.receivers.len() > MAX_RECEIVERS
        || manifest.sources.is_empty()
        || manifest.sources.len() > MAX_SOURCES
        || manifest.missing_sources.len() > MAX_MISSING
        || manifest.assets.len() > MAX_RECEIVERS
    {
        return Err(invalid(
            "Generated prop lighting metadata exceeds its count budgets",
        ));
    }
    digest(&manifest.static_props_sha256)?;
    let mut sources = BTreeSet::new();
    let mut source_bytes = 0_usize;
    for source in &manifest.sources {
        path(&source.path)?;
        digest(&source.sha256)?;
        if source.bytes == 0 || !sources.insert(source.path.as_str()) {
            return Err(invalid("Prop lighting sources must be nonempty and unique"));
        }
        source_bytes = source_bytes
            .checked_add(source.bytes)
            .filter(|&bytes| bytes <= MAX_SOURCE_BYTES)
            .ok_or_else(|| invalid("Prop lighting source bytes exceed 64 MiB"))?;
    }
    let mut missing = BTreeSet::new();
    for candidate in &manifest.missing_sources {
        path(candidate)?;
        if sources.contains(candidate.as_str()) || !missing.insert(candidate) {
            return Err(invalid(
                "Prop lighting missing candidates conflict or repeat",
            ));
        }
    }
    let mut previous = None;
    let mut mesh_count = 0_usize;
    let mut models = BTreeMap::new();
    for receiver in &manifest.receivers {
        path(&receiver.model)?;
        if previous.is_some_and(|ordinal| ordinal >= receiver.ordinal)
            || receiver.ordinal as usize >= bsp::MAX_STATIC_PROPS
            || !receiver.model.starts_with("models/")
            || !receiver.model.ends_with(".mdl")
            || !sources.contains(receiver.model.as_str())
            || receiver.meshes.is_empty()
            || receiver.meshes.len() > MAX_RECEIVER_MESHES
        {
            return Err(invalid(
                "Invalid ordered prop lighting receiver or model source",
            ));
        }
        if models
            .insert(receiver.model.as_str(), receiver.checksum)
            .is_some_and(|old| old != receiver.checksum)
        {
            return Err(invalid("Prop lighting model has conflicting checksums"));
        }
        previous = Some(receiver.ordinal);
        mesh_count = mesh_count
            .checked_add(receiver.meshes.len())
            .filter(|&count| count <= MAX_MESHES)
            .ok_or_else(|| invalid("Prop lighting descriptors exceed their aggregate budget"))?;
        let mut vertices = 0_usize;
        for mesh in &receiver.meshes {
            if mesh.lod >= vhv::MAX_LODS {
                return Err(invalid("Unsupported prop lighting mesh LOD"));
            }
            vertices = vertices
                .checked_add(mesh.vertex_count as usize)
                .filter(|&count| count <= vhv::Limits::default().max_vertices)
                .ok_or_else(|| invalid("Prop lighting receiver vertices exceed their budget"))?;
        }
        if vertices == 0 {
            return Err(invalid("Prop lighting receiver has no color vertices"));
        }
    }
    if !assets {
        return Ok(());
    }
    let count = manifest
        .receivers
        .len()
        .checked_mul(manifest.channels.len())
        .ok_or_else(|| invalid("Prop lighting asset count overflow"))?;
    if manifest.assets.len() != count {
        return Err(invalid(
            "Prop lighting assets do not cover every receiver and channel",
        ));
    }
    let mut bytes = 0_usize;
    for (asset, (receiver, channel)) in
        manifest
            .assets
            .iter()
            .zip(manifest.receivers.iter().flat_map(|receiver| {
                manifest
                    .channels
                    .iter()
                    .map(move |channel| (receiver, channel))
            }))
    {
        digest(&asset.sha256)?;
        if asset.ordinal != receiver.ordinal
            || asset.channel != *channel
            || asset.path != asset_path(receiver.ordinal, *channel)
            || asset.bytes == 0
            || asset.bytes > PakLimits::default().max_entry_bytes
        {
            return Err(invalid(
                "Invalid prop lighting asset identity, channel or byte count",
            ));
        }
        bytes = bytes
            .checked_add(asset.bytes)
            .filter(|&count| count <= PakLimits::default().max_total_bytes)
            .ok_or_else(|| invalid("Prop lighting generated bytes exceed 64 MiB"))?;
    }
    Ok(())
}

fn verify_archive(
    bsp: &Bsp,
    archive: &PakArchive,
    manifest: &PropLightingManifest,
    flags: u32,
    control: &Control,
) -> io::Result<()> {
    checkpoint(control)?;
    check_lumps(bsp)?;
    verify_bsp_size(bsp, bsp.lumps[bsp::lump::PAKFILE].data.len(), control)?;
    validate_metadata(manifest, true)?;
    if flags & MAP_FLAGS != channel_flags(&manifest.channels) {
        return Err(invalid(
            "Baked prop lighting map flags differ from its channels",
        ));
    }
    let sprp = sprp(bsp)?;
    if sprp.version != manifest.static_props_version
        || hash(&sprp.data) != manifest.static_props_sha256
    {
        return Err(invalid(
            "Final static-prop table differs from the baked lighting plan",
        ));
    }
    let props = bsp
        .static_props()?
        .ok_or_else(|| invalid("Prop lighting requires the final static-prop table"))?;
    let eligible = props
        .props
        .iter()
        .enumerate()
        .filter(|(_, prop)| prop.flags & DISABLE_VERTEX_LIGHTING == 0);
    let mut receivers = manifest.receivers.iter();
    for (ordinal, prop) in eligible {
        checkpoint(control)?;
        let receiver = receivers
            .next()
            .ok_or_else(|| invalid("Prop lighting omits an enabled final static prop"))?;
        if receiver.ordinal as usize != ordinal
            || receiver.model != asset_relative(props.model_name(usize::from(prop.model))?)?
        {
            return Err(invalid(
                "Prop lighting ordinal or model differs from the final static prop",
            ));
        }
    }
    if receivers.next().is_some() {
        return Err(invalid(
            "Prop lighting contains an extra or disabled receiver",
        ));
    }
    let entries: BTreeMap<_, _> = archive
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect();
    let expected: BTreeSet<_> = manifest
        .assets
        .iter()
        .map(|asset| asset.path.as_str())
        .collect();
    for entry in &archive.entries {
        if generated_name(&entry.name) && !expected.contains(entry.name.as_str()) {
            return Err(invalid(
                "PAK contains an unregistered prop lighting sibling",
            ));
        }
    }
    for asset in &manifest.assets {
        checkpoint(control)?;
        let data = entries
            .get(asset.path.as_str())
            .ok_or_else(|| invalid("Generated prop lighting file is missing"))?;
        if data.len() != asset.bytes || hash(data) != asset.sha256 {
            return Err(invalid(
                "Generated prop lighting file length or hash differs",
            ));
        }
        let receiver = manifest
            .receivers
            .binary_search_by_key(&asset.ordinal, |receiver| receiver.ordinal)
            .map(|index| &manifest.receivers[index])
            .map_err(|_| invalid("Generated lighting has no receiver"))?;
        let metadata = vhv::inspect(data, vhv::Limits::default())?;
        if metadata.checksum != receiver.checksum || metadata.mesh_count != receiver.meshes.len() {
            return Err(invalid(
                "Generated VHV checksum or descriptor count differs",
            ));
        }
        for (index, mesh) in receiver.meshes.iter().enumerate() {
            let base = vhv::HEADER_BYTES + index * vhv::MESH_HEADER_BYTES;
            let lod = u32::from_le_bytes(data[base..base + 4].try_into().unwrap());
            let count = u32::from_le_bytes(data[base + 4..base + 8].try_into().unwrap());
            if lod != mesh.lod || count != mesh.vertex_count {
                return Err(invalid("Generated VHV ordered hardware descriptors differ"));
            }
        }
    }
    checkpoint(control)
}

fn append(
    archive: &mut PakArchive,
    entries: &[PakEntry],
    manifest: Vec<u8>,
    control: &Control,
) -> io::Result<()> {
    let limits = PakLimits::default();
    if archive
        .entries
        .len()
        .checked_add(entries.len())
        .and_then(|count| count.checked_add(1))
        .is_none_or(|count| count > limits.max_entries)
    {
        return Err(invalid("Combined prop lighting PAK exceeds 4096 entries"));
    }
    let mut total = manifest.len();
    for entry in archive.entries.iter().chain(entries) {
        checkpoint(control)?;
        total = total
            .checked_add(entry.data.len())
            .filter(|&bytes| {
                bytes <= limits.max_total_bytes && entry.data.len() <= limits.max_entry_bytes
            })
            .ok_or_else(|| invalid("Combined prop lighting PAK exceeds its byte budgets"))?;
    }
    if archive
        .entries
        .iter()
        .any(|entry| entry.name.eq_ignore_ascii_case(MANIFEST_PATH) || generated_name(&entry.name))
    {
        return Err(invalid(
            "Prop lighting assets or manifest collide with existing packed content",
        ));
    }
    archive.entries.extend(entries.iter().cloned());
    archive.entries.push(PakEntry {
        name: MANIFEST_PATH.into(),
        data: manifest,
    });
    Ok(())
}

fn source_guard(
    manifest: &PropLightingManifest,
    game: &GameFs,
    control: &Control,
) -> io::Result<AssetSourceGuard> {
    for source in &manifest.sources {
        checkpoint(control)?;
        let bytes = game
            .read_limited(&source.path, source.bytes)?
            .ok_or_else(|| invalid(format!("Prop lighting source disappeared: {}", source.path)))?;
        if bytes.len() != source.bytes {
            return Err(invalid(
                "Prop lighting source length differs from its snapshot",
            ));
        }
        if hash(&bytes) != source.sha256 {
            return Err(invalid(
                "Prop lighting source hash differs from its snapshot",
            ));
        }
        if let Some(receiver) = manifest
            .receivers
            .iter()
            .find(|receiver| receiver.model == source.path)
        {
            let header = mdl::ModelHeader::parse(&bytes)?;
            if header.checksum as u32 != receiver.checksum || !header.is_static_prop() {
                return Err(invalid(
                    "Prop lighting model checksum or static-prop flag differs",
                ));
            }
        }
    }
    capture_asset_source_guard(
        manifest
            .sources
            .iter()
            .map(|source| (source.path.as_str(), source.sha256.as_str())),
        manifest.missing_sources.iter().map(String::as_str),
        game,
        control,
        SourceGuardPolicy {
            label: "Prop lighting",
            missing_label: "Prop lighting material candidate",
            asset_bytes: MAX_SOURCE_BYTES,
            physical_paths: MAX_PHYSICAL_PATHS,
        },
    )
}

fn capture_lookups(
    manifest: &PropLightingManifest,
    game: &GameFs,
    control: &Control,
) -> io::Result<Vec<Lookup>> {
    let mut lookups = Vec::new();
    let mut physical_paths = 0_usize;
    for (path, missing) in manifest
        .sources
        .iter()
        .map(|source| (source.path.as_str(), false))
        .chain(
            manifest
                .missing_sources
                .iter()
                .map(|path| (path.as_str(), true)),
        )
    {
        checkpoint(control)?;
        let paths = if missing {
            game.asset_lookup_paths(path)?
        } else {
            game.asset_source_paths(path)?
                .ok_or_else(|| invalid("Prop lighting source disappeared"))?
        };
        physical_paths = physical_paths
            .checked_add(paths.files.len())
            .and_then(|count| count.checked_add(paths.lookup_directories.len()))
            .and_then(|count| count.checked_add(paths.lookup_symlinks.len()))
            .filter(|&count| count <= MAX_PHYSICAL_PATHS)
            .ok_or_else(|| {
                invalid("Prop lighting lookup snapshots exceed 8192 physical path references")
            })?;
        lookups.push(Lookup {
            path: path.into(),
            paths,
            missing,
        });
    }
    Ok(lookups)
}

fn verify_lookups(lookups: &[Lookup], game: &GameFs, control: &Control) -> io::Result<()> {
    for lookup in lookups {
        checkpoint(control)?;
        let current = if lookup.missing {
            Some(game.asset_lookup_paths(&lookup.path)?)
        } else {
            game.asset_source_paths(&lookup.path)?
        };
        if current.as_ref() != Some(&lookup.paths) {
            return Err(invalid(format!(
                "Prop lighting source resolution changed: {}",
                lookup.path
            )));
        }
    }
    Ok(())
}

fn sprp(bsp: &Bsp) -> io::Result<&bsp::GameLump> {
    check_lumps(bsp)?;
    let mut entries = bsp.game_lumps.iter().filter(|lump| lump.id == *b"sprp");
    let entry = entries
        .next()
        .ok_or_else(|| invalid("Prop lighting requires a static-prop game lump"))?;
    if entries.next().is_some() || entry.flags != 0 || entry.data.len() > bsp::MAX_STATIC_PROP_BYTES
    {
        return Err(invalid(
            "Prop lighting static-prop lump is duplicated, compressed or oversized",
        ));
    }
    Ok(entry)
}

fn map_flags(bsp: &Bsp) -> io::Result<u32> {
    check_lumps(bsp)?;
    let lump = &bsp.lumps[bsp::lump::MAP_FLAGS];
    if lump.version != 0 || lump.fourcc != [0; 4] || !matches!(lump.data.len(), 0 | 4) {
        return Err(invalid("Unsupported baked prop lighting map-flag lump"));
    }
    Ok(if lump.data.is_empty() {
        0
    } else {
        u32::from_le_bytes(lump.data[..4].try_into().unwrap())
    })
}

fn check_lumps(bsp: &Bsp) -> io::Result<()> {
    if bsp.lumps.len() != bsp::LUMP_COUNT {
        return Err(invalid("BSP must have exactly 64 lump slots"));
    }
    Ok(())
}

fn channel_flags(channels: &[Channel]) -> u32 {
    channels.iter().fold(0, |flags, channel| {
        flags
            | match channel {
                Channel::Ldr => LDR_MAP_FLAG,
                Channel::Hdr => HDR_MAP_FLAG,
            }
    })
}

fn asset_path(ordinal: u32, channel: Channel) -> String {
    match channel {
        Channel::Ldr => format!("sp_{ordinal}.vhv"),
        Channel::Hdr => format!("sp_hdr_{ordinal}.vhv"),
    }
}

fn generated_name(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let base = path.rsplit('/').next().unwrap_or(&path);
    base.starts_with("sp_") && base.ends_with(".vhv")
}

fn path(path: &str) -> io::Result<()> {
    if path.is_empty()
        || path.len() > PakLimits::default().max_name_bytes
        || asset_relative(path)? != path
        || path.chars().any(char::is_control)
    {
        return Err(invalid(
            "Prop lighting source path is not canonical or bounded",
        ));
    }
    Ok(())
}

fn digest(value: &str) -> io::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "Prop lighting SHA-256 must contain 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

struct ManifestWriter(Vec<u8>);

impl Write for ManifestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let required = self
            .0
            .len()
            .checked_add(bytes.len())
            .filter(|&size| size <= MAX_MANIFEST_BYTES)
            .ok_or_else(|| invalid("Prop lighting manifest exceeds 1 MiB"))?;
        if required > self.0.capacity() {
            let capacity = required
                .max(self.0.capacity().saturating_mul(2))
                .min(MAX_MANIFEST_BYTES);
            self.0
                .try_reserve_exact(capacity - self.0.len())
                .map_err(io::Error::other)?;
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn manifest_bytes(manifest: &PropLightingManifest) -> io::Result<Vec<u8>> {
    let mut writer = ManifestWriter(Vec::new());
    serde_json::to_writer(&mut writer, manifest).map_err(io::Error::other)?;
    Ok(writer.0)
}

fn bounded_vec<'de, D, T, const MAXIMUM: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const LIMIT: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const LIMIT: usize> Visitor<'de> for Bounded<T, LIMIT> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "at most {LIMIT} elements")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element()? {
                if values.len() == LIMIT {
                    return Err(serde::de::Error::custom(
                        "Prop lighting sequence exceeds its count budget",
                    ));
                }
                if values.len() == values.capacity() {
                    values
                        .try_reserve_exact((LIMIT - values.len()).min(32))
                        .map_err(serde::de::Error::custom)?;
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, MAXIMUM>(PhantomData))
}

fn verify_bsp_size(bsp: &Bsp, pak_bytes: usize, control: &Control) -> io::Result<()> {
    check_lumps(bsp)?;
    let mut size = 8_usize + bsp::LUMP_COUNT * 16 + 4;
    for (index, lump) in bsp.lumps.iter().enumerate() {
        checkpoint(control)?;
        size = size
            .checked_add(3)
            .map(|value| value & !3)
            .ok_or_else(|| invalid("Prop lighting BSP size overflow"))?;
        let bytes = if index == bsp::lump::PAKFILE {
            pak_bytes
        } else if index == bsp::lump::MAP_FLAGS {
            4
        } else if index == bsp::lump::GAME_LUMP {
            let directory = bsp
                .game_lumps
                .len()
                .checked_mul(16)
                .and_then(|value| value.checked_add(4))
                .ok_or_else(|| invalid("Prop lighting game-lump size overflow"))?;
            bsp.game_lumps
                .iter()
                .try_fold(directory, |size, lump| size.checked_add(lump.data.len()))
                .ok_or_else(|| invalid("Prop lighting game-lump size overflow"))?
        } else {
            lump.data.len()
        };
        size = size
            .checked_add(bytes)
            .filter(|&size| size <= PakLimits::default().max_bsp_bytes)
            .ok_or_else(|| invalid("Prop lighting BSP exceeds its byte budget"))?;
    }
    Ok(())
}

fn checkpoint(control: &Control) -> io::Result<()> {
    control.checkpoint().map_err(io::Error::other)
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests;
