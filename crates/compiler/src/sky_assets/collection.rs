use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

use source_formats::bsp::{PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::{AssetSourcePaths, GameFs, asset_relative};
use source_formats::keyvalues::{self, Node};
use source_formats::vmf::Vmf;
use source_formats::vmt::{self, Material};

use super::{
    MAX_ASSETS, MAX_FAMILY_BYTES, MAX_MATERIAL_BYTES, MAX_TOTAL_BYTES, Reader, SUFFIXES,
    SkyAssetSource, supported_parameter,
};
use crate::asset_sources::{AssetSourceGuard, SourceGuardPolicy, capture_asset_source_guard, hash};
use crate::{Control, FileStamp};

const MAX_PHYSICAL_PATHS: usize = 8192;
const MAX_RAW_DEPTH: usize = 64;
const MAX_RAW_NODES: usize = 4096;

#[derive(Debug)]
pub struct SkyAssetSourceSnapshot {
    path: String,
    sha256: String,
    origin: SkyAssetSource,
    lookup: AssetSourcePaths,
}

impl SkyAssetSourceSnapshot {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn origin(&self) -> &SkyAssetSource {
        &self.origin
    }
    pub fn lookup_paths(&self) -> &AssetSourcePaths {
        &self.lookup
    }
}

#[derive(Debug)]
pub struct SkyAssetPlan {
    family: String,
    hdr: bool,
    entries: Vec<PakEntry>,
    sources: Vec<SkyAssetSourceSnapshot>,
    stamps: Vec<FileStamp>,
    guard: AssetSourceGuard,
    combined_pak_bytes: usize,
}

impl SkyAssetPlan {
    pub fn family(&self) -> &str {
        &self.family
    }
    pub fn has_hdr(&self) -> bool {
        self.hdr
    }
    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }
    pub fn sources(&self) -> &[SkyAssetSourceSnapshot] {
        &self.sources
    }
    pub fn asset_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.data.len()).sum()
    }
    pub fn combined_pak_bytes(&self) -> usize {
        self.combined_pak_bytes
    }

    pub fn verify_sources(&self, game: &GameFs, control: &Control) -> io::Result<()> {
        control.checkpoint().map_err(io::Error::other)?;
        verify_stamps(&self.stamps, control)?;
        self.guard.verify_metadata()?;
        verify_lookups(&self.sources, game, control)?;
        capture_asset_source_guard(
            self.sources
                .iter()
                .map(|source| (source.path.as_str(), source.sha256.as_str())),
            [],
            game,
            control,
            guard_policy(),
        )?
        .verify_metadata()?;
        verify_lookups(&self.sources, game, control)?;
        verify_stamps(&self.stamps, control)?;
        self.guard.verify_metadata()
    }
}

#[derive(Clone)]
struct RawMaterial {
    include: Option<String>,
    edits: Vec<(bool, Vec<(String, String)>)>,
    textures: Vec<(String, String)>,
}

struct Collector<'a> {
    reader: Reader<'a>,
    control: &'a Control,
    limits: PakLimits,
    existing_bytes: usize,
    sources: BTreeMap<String, SkyAssetSourceSnapshot>,
    stamps: BTreeMap<PathBuf, FileStamp>,
    raw: BTreeMap<String, RawMaterial>,
}

impl Collector<'_> {
    fn read(&mut self, path: &str, material: bool) -> io::Result<Vec<u8>> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let path = pak_path(path, if material { "vmt" } else { "vtf" }, self.limits)?;
        if let Some(source) = self.sources.get(&path) {
            let current = self.reader.game.asset_source_paths(&path)?;
            if current.as_ref() != Some(&source.lookup) {
                return Err(invalid(format!("Sky source resolution changed: {path}")));
            }
            return self
                .reader
                .assets
                .get(&path)
                .and_then(|asset| asset.as_ref().ok())
                .and_then(Option::as_ref)
                .map(|asset| asset.data.clone())
                .ok_or_else(|| invalid("Cached sky source is unavailable"));
        }
        if self.sources.len() >= MAX_ASSETS {
            return Err(invalid("Sky closure exceeds 128 source lookups"));
        }
        let before = self
            .reader
            .game
            .asset_source_paths(&path)?
            .ok_or_else(|| invalid(format!("Missing sky source: {path}")))?;
        capture_paths(&before, &mut self.stamps, self.control)?;
        let remaining = self
            .limits
            .max_total_bytes
            .saturating_sub(self.existing_bytes)
            .saturating_sub(self.reader.bytes);
        let mut maximum = self
            .limits
            .max_entry_bytes
            .min(MAX_TOTAL_BYTES)
            .min(remaining);
        if material {
            maximum = maximum.min(MAX_MATERIAL_BYTES);
        }
        let asset = self
            .reader
            .read(&path, maximum)
            .map_err(invalid)?
            .ok_or_else(|| invalid(format!("Missing sky source: {path}")))?;
        let bytes = asset.data.clone();
        let origin = asset.source.clone();
        let after = self
            .reader
            .game
            .asset_source_paths(&path)?
            .ok_or_else(|| invalid(format!("Sky source disappeared: {path}")))?;
        if before != after {
            return Err(invalid(format!("Sky source resolution changed: {path}")));
        }
        capture_paths(&after, &mut self.stamps, self.control)?;
        if material {
            let mut raw = raw_material(&path, &bytes)?;
            if let Some(include) = &raw.include {
                raw.include = Some(pak_path(include, "vmt", self.limits)?);
            }
            self.raw.insert(path.clone(), raw);
        }
        self.sources.insert(
            path.clone(),
            SkyAssetSourceSnapshot {
                path,
                sha256: hash(&bytes),
                origin,
                lookup: before,
            },
        );
        Ok(bytes)
    }

    fn resolve(&mut self, path: &str) -> io::Result<Material> {
        Material::load_with_reader(path, &mut |path| self.read(path, true).map(Some))
            .map_err(|error| invalid(format!("{path}: {error}")))?
            .ok_or_else(|| invalid(format!("Missing sky material: {path}")))
    }

    fn validate_edits(&mut self, path: &str, active: &mut BTreeSet<String>) -> io::Result<()> {
        self.control.checkpoint().map_err(io::Error::other)?;
        if !active.insert(path.into()) {
            return Err(invalid(format!("Sky Patch cycle: {path}")));
        }
        let raw = self
            .raw
            .get(path)
            .cloned()
            .ok_or_else(|| invalid("Sky raw material is missing"))?;
        if let Some(include) = raw.include {
            let include = pak_path(&include, "vmt", self.limits)?;
            self.validate_edits(&include, active)?;
            let base = self.resolve(&include)?;
            let mut values: BTreeMap<_, _> = base.params.into_iter().collect();
            for (replace, edits) in raw.edits {
                for (key, value) in edits {
                    self.control.checkpoint().map_err(io::Error::other)?;
                    if replace && !values.contains_key(&key)
                        || !replace && values.get(&key).is_some_and(|old| old != &value)
                    {
                        return Err(invalid(format!(
                            "{path}: ambiguous Patch edit for {key}; replace requires an existing key and insert cannot change an existing value"
                        )));
                    }
                    values.insert(key, value);
                }
            }
        }
        active.remove(path);
        Ok(())
    }
}

/// Collects the supported six-face texture closure with VTF layout, source
/// identity and PAK budget checks. Scalar rendering behavior and HDR radiance
/// remain outside this validation.
pub fn collect_sky_asset_plan(
    vmf: &Vmf,
    game: &GameFs,
    existing: &PakArchive,
    limits: PakLimits,
    control: &Control,
) -> io::Result<SkyAssetPlan> {
    control.checkpoint().map_err(io::Error::other)?;
    let defaults = PakLimits::default();
    let limits = PakLimits {
        max_archive_bytes: limits.max_archive_bytes.min(defaults.max_archive_bytes),
        max_entries: limits.max_entries.min(defaults.max_entries),
        max_entry_bytes: limits.max_entry_bytes.min(defaults.max_entry_bytes),
        max_total_bytes: limits.max_total_bytes.min(defaults.max_total_bytes),
        max_name_bytes: limits.max_name_bytes.min(defaults.max_name_bytes),
        max_bsp_bytes: limits.max_bsp_bytes.min(defaults.max_bsp_bytes),
    };
    let mut check = || control.checkpoint().map_err(io::Error::other);
    existing.to_bytes_controlled(limits, &mut check)?;
    let mut names = vmf.world.properties.get_all("skyname");
    let family = names
        .next()
        .ok_or_else(|| invalid("Set world skyname before collecting sky assets"))?;
    if names.next().is_some() {
        return Err(invalid("World has duplicate skyname keys"));
    }
    if family.is_empty()
        || family.len() > MAX_FAMILY_BYTES
        || family.trim() != family
        || family.chars().any(char::is_control)
        || asset_relative(family).is_err()
    {
        return Err(invalid(
            "Sky family must be a bounded exact relative name without surrounding whitespace",
        ));
    }
    let mut collector = Collector {
        reader: Reader::new(game),
        control,
        limits,
        existing_bytes: existing.entries.iter().map(|entry| entry.data.len()).sum(),
        sources: BTreeMap::new(),
        stamps: BTreeMap::new(),
        raw: BTreeMap::new(),
    };
    let mut materials = Vec::new();
    for suffix in SUFFIXES {
        let path = vmt::material_path(&format!("skybox/{family}{suffix}"));
        materials.push(collector.resolve(&path)?);
        collector.validate_edits(&path, &mut BTreeSet::new())?;
    }
    let declarations = collector
        .raw
        .values()
        .flat_map(|raw| raw.textures.clone())
        .collect::<Vec<_>>();
    for (parameter, reference) in declarations {
        control.checkpoint().map_err(io::Error::other)?;
        if reference.len() > collector.limits.max_name_bytes
            || reference.trim() != reference
            || reference.is_empty()
            || reference.starts_with('_')
            || reference.eq_ignore_ascii_case("env_cubemap")
        {
            return Err(invalid(
                "Sky textures must name exact bounded static mounted files",
            ));
        }
        asset_relative(&reference)?;
        let path = vmt::texture_path(&reference);
        collector.read(&path, false)?;
        let texture =
            collector
                .reader
                .texture(&parameter, &reference, parameter == "$hdrbasetexture");
        if !texture.ready {
            return Err(invalid(format!("{path}: {}", texture.issues.join("; "))));
        }
    }
    let hdr = materials
        .iter()
        .any(|material| material.get("$hdrbasetexture").is_some());
    for (suffix, material) in SUFFIXES.into_iter().zip(&materials) {
        control.checkpoint().map_err(io::Error::other)?;
        let face = collector.reader.face(family, suffix);
        if !face.ldr.ready
            || hdr && (material.shader != "sky" || !face.hdr.ready)
            || !hdr && material.shader != "unlitgeneric"
        {
            return Err(invalid(format!(
                "skybox/{family}{suffix}: packing requires six UnlitGeneric LDR faces or six Sky faces with complete LDR and supported HDR bases"
            )));
        }
    }
    let mut entries = Vec::new();
    let existing_names = existing
        .entries
        .iter()
        .map(|entry| entry.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for (path, asset) in &collector.reader.assets {
        control.checkpoint().map_err(io::Error::other)?;
        let asset = asset
            .as_ref()
            .map_err(|error| invalid(error.clone()))?
            .as_ref()
            .ok_or_else(|| invalid("Sky closure contains a missing source"))?;
        if existing_names.contains(&path.to_ascii_lowercase()) {
            return Err(invalid(format!(
                "Sky asset collides with existing packed content: {path}"
            )));
        }
        entries.push(PakEntry {
            name: path.clone(),
            data: asset.data.clone(),
        });
    }
    if entries.len() > MAX_ASSETS {
        return Err(invalid("Sky closure exceeds its asset budget"));
    }
    let mut combined = existing.clone();
    combined.entries.extend(entries.iter().cloned());
    let combined_pak_bytes = combined.to_bytes_controlled(limits, &mut check)?.len();
    let sources = collector.sources.into_values().collect::<Vec<_>>();
    let stamps = collector.stamps.into_values().collect::<Vec<_>>();
    verify_stamps(&stamps, control)?;
    verify_lookups(&sources, game, control)?;
    let guard = capture_asset_source_guard(
        sources
            .iter()
            .map(|source| (source.path.as_str(), source.sha256.as_str())),
        [],
        game,
        control,
        guard_policy(),
    )?;
    verify_lookups(&sources, game, control)?;
    verify_stamps(&stamps, control)?;
    Ok(SkyAssetPlan {
        family: family.into(),
        hdr,
        entries,
        sources,
        stamps,
        guard,
        combined_pak_bytes,
    })
}

fn raw_material(path: &str, bytes: &[u8]) -> io::Result<RawMaterial> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        invalid(format!(
            "{path}: sky packing requires UTF-8 VMT source: {error}"
        ))
    })?;
    let nodes = keyvalues::parse_bounded(
        text.trim_start_matches('\u{feff}'),
        MAX_RAW_DEPTH,
        MAX_RAW_NODES,
    )
    .map_err(|error| {
        invalid(format!(
            "{path}: sky packing requires complete strict VMT syntax: {error}"
        ))
    })?;
    if nodes.len() != 1 {
        return Err(invalid(format!(
            "{path}: sky packing requires exactly one material root"
        )));
    }
    let root = &nodes[0];
    let children = root
        .children()
        .ok_or_else(|| invalid(format!("{path}: material root must be a block")))?;
    let shader = root.key.to_ascii_lowercase();
    if !matches!(shader.as_str(), "patch" | "sky" | "unlitgeneric") {
        return Err(invalid(format!("{path}: unsupported sky shader")));
    }
    let mut result = RawMaterial {
        include: None,
        edits: Vec::new(),
        textures: Vec::new(),
    };
    let mut keys = BTreeSet::new();
    for node in children {
        let key = node.key.to_ascii_lowercase();
        if !keys.insert(key.clone()) {
            return Err(invalid(format!("{path}: duplicate material key {key}")));
        }
        if shader == "patch" {
            match key.as_str() {
                "include" => {
                    result.include = Some(
                        node.as_str()
                            .ok_or_else(|| invalid("Patch include must be scalar"))?
                            .into(),
                    )
                }
                "replace" | "insert" => {
                    let edits = node
                        .children()
                        .ok_or_else(|| invalid("Patch edits must be scalar blocks"))?;
                    let values = raw_parameters(path, edits, &mut result.textures)?;
                    result.edits.push((key == "replace", values));
                }
                _ => return Err(invalid(format!("{path}: unsupported Patch key {key}"))),
            }
        } else {
            raw_parameter(path, node, &mut result.textures)?;
        }
    }
    if shader == "patch" && result.include.is_none() {
        return Err(invalid(format!(
            "{path}: Patch requires exactly one include"
        )));
    }
    Ok(result)
}

fn raw_parameters(
    path: &str,
    nodes: &[Node],
    textures: &mut Vec<(String, String)>,
) -> io::Result<Vec<(String, String)>> {
    let mut keys = BTreeSet::new();
    let mut values = Vec::new();
    for node in nodes {
        let (key, value) = raw_parameter(path, node, textures)?;
        if !keys.insert(key.clone()) {
            return Err(invalid(format!("{path}: duplicate Patch edit key {key}")));
        }
        values.push((key, value));
    }
    Ok(values)
}

fn raw_parameter(
    path: &str,
    node: &Node,
    textures: &mut Vec<(String, String)>,
) -> io::Result<(String, String)> {
    let key = node.key.to_ascii_lowercase();
    let value = node
        .as_str()
        .ok_or_else(|| invalid(format!("{path}: nested sky material nodes are unsupported")))?;
    if value.starts_with('[') && value.ends_with(']') {
        return Err(invalid(format!(
            "{path}: bracket conditional values are unsupported in sky packing"
        )));
    }
    if !supported_parameter(&key) || key.starts_with("$hdrcompressedtexture") {
        return Err(invalid(format!("{path}: unsupported sky parameter {key}")));
    }
    if matches!(key.as_str(), "$basetexture" | "$hdrbasetexture") {
        textures.push((key.clone(), value.into()));
    }
    Ok((key, value.into()))
}

fn pak_path(path: &str, extension: &str, limits: PakLimits) -> io::Result<String> {
    if path.len() > limits.max_name_bytes
        || path.trim() != path
        || path.chars().any(char::is_control)
    {
        return Err(invalid(
            "Sky dependency exceeds the PAK name limit or contains unsupported whitespace",
        ));
    }
    let path = asset_relative(path)?;
    if !path.starts_with("materials/")
        || !path.ends_with(&format!(".{extension}"))
        || path.len() > limits.max_name_bytes
    {
        return Err(invalid(
            "Sky dependencies must use bounded materials/*.vmt or materials/*.vtf PAK paths",
        ));
    }
    Ok(path)
}

fn capture_paths(
    paths: &AssetSourcePaths,
    stamps: &mut BTreeMap<PathBuf, FileStamp>,
    control: &Control,
) -> io::Result<()> {
    for path in paths
        .files
        .iter()
        .chain(&paths.lookup_directories)
        .chain(&paths.lookup_symlinks)
    {
        control.checkpoint().map_err(io::Error::other)?;
        let stamp = FileStamp::capture_container(path)?;
        if let Some(old) = stamps.get(path) {
            if old != &stamp {
                return Err(invalid(format!(
                    "Sky backing file or lookup path changed: {}",
                    path.display()
                )));
            }
        } else {
            if stamps.len() >= MAX_PHYSICAL_PATHS {
                return Err(invalid(
                    "Sky source guard exceeds 8192 physical lookup paths",
                ));
            }
            stamps.insert(path.clone(), stamp);
        }
    }
    Ok(())
}

fn verify_stamps(stamps: &[FileStamp], control: &Control) -> io::Result<()> {
    for stamp in stamps {
        control.checkpoint().map_err(io::Error::other)?;
        stamp.verify_metadata()?;
    }
    Ok(())
}

fn verify_lookups(
    sources: &[SkyAssetSourceSnapshot],
    game: &GameFs,
    control: &Control,
) -> io::Result<()> {
    for source in sources {
        control.checkpoint().map_err(io::Error::other)?;
        if game.asset_source_paths(&source.path)?.as_ref() != Some(&source.lookup) {
            return Err(invalid(format!(
                "Sky source resolution changed: {}",
                source.path
            )));
        }
    }
    Ok(())
}

fn guard_policy() -> SourceGuardPolicy {
    SourceGuardPolicy {
        label: "Sky",
        missing_label: "Sky dependency",
        asset_bytes: MAX_TOTAL_BYTES,
        physical_paths: MAX_PHYSICAL_PATHS,
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    let message = message.into();
    let (mut message, truncated) = super::bounded(&message, super::MAX_ISSUE_CHARACTERS);
    if truncated {
        message.push_str(" [truncated]");
    }
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
