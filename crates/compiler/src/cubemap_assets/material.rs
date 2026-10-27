use std::collections::{BTreeMap, BTreeSet};

use source_formats::bsp::PakEntry;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::keyvalues::{self, Node, Value};
use source_formats::vmt::{self, Material};

use super::*;
use crate::Control;

const DEPENDENCIES: [&str; 3] = ["$bottommaterial", "$crackmaterial", "$fallbackmaterial"];

pub(super) fn self_bottom(name: &str, key: &str, value: &str) -> bool {
    key.eq_ignore_ascii_case("$bottommaterial") && value.eq_ignore_ascii_case(name)
}

pub(super) fn material_name(name: &str) -> Result<String, String> {
    let name = asset_relative(name.trim()).map_err(|error| error.to_string())?;
    let name = name.strip_prefix("materials/").unwrap_or(&name);
    let name = name.strip_suffix(".vmt").unwrap_or(name);
    if name.is_empty() || name.len() > 180 || name.contains(['"', '\n', '\r']) {
        return Err(format!("Unsupported reflection material path: {name}"));
    }
    Ok(name.to_owned())
}

pub(super) fn alias_name(
    namespace: &str,
    material: &str,
    origin: [i32; 3],
) -> Result<String, String> {
    let name = format!(
        "maps/{namespace}/{material}_{}_{}_{}",
        origin[0], origin[1], origin[2]
    );
    if name.len() >= 128 || vmt::material_path(&name).len() > 259 {
        return Err(format!(
            "Generated reflection material name exceeds the native 127-byte limit: {name}"
        ));
    }
    Ok(name)
}

pub(super) fn sample_texture(namespace: &str, origin: [i32; 3]) -> String {
    format!(
        "materials/maps/{namespace}/c{}_{}_{}.vtf",
        origin[0], origin[1], origin[2]
    )
}

pub(super) fn hdr_sample_texture(namespace: &str, origin: [i32; 3]) -> String {
    sample_texture(namespace, origin).replace(".vtf", ".hdr.vtf")
}

pub(super) fn ensure_default_unmounted(fs: &GameFs, namespace: &str) -> Result<(), String> {
    let path = format!("materials/maps/{namespace}/cubemapdefault.vtf");
    ensure_unmounted(fs, &path)
}

pub(super) fn ensure_hdr_default_unmounted(fs: &GameFs, namespace: &str) -> Result<(), String> {
    ensure_unmounted(
        fs,
        &format!("materials/maps/{namespace}/cubemapdefault.hdr.vtf"),
    )
}

fn ensure_unmounted(fs: &GameFs, path: &str) -> Result<(), String> {
    if fs
        .read_limited(path, 1)
        .map_err(|error| format!("Cannot reserve generated default cubemap {path}: {error}"))?
        .is_some()
    {
        return Err(format!(
            "Generated default cubemap collides with mounted content: {path}"
        ));
    }
    Ok(())
}

pub(super) struct Graph<'a> {
    fs: &'a GameFs,
    control: &'a Control,
    materials: BTreeMap<String, Material>,
    material_snapshots: BTreeMap<String, Vec<u8>>,
    reflective: BTreeMap<String, bool>,
    pub sources: BTreeMap<String, String>,
    pub missing_sources: BTreeSet<String>,
    source_bytes: usize,
    pub entries: BTreeMap<String, Vec<u8>>,
    pub aliases: BTreeMap<String, CubemapAliasAsset>,
}

impl<'a> Graph<'a> {
    pub fn new(fs: &'a GameFs, control: &'a Control) -> Self {
        Self {
            fs,
            control,
            materials: BTreeMap::new(),
            material_snapshots: BTreeMap::new(),
            reflective: BTreeMap::new(),
            sources: BTreeMap::new(),
            missing_sources: BTreeSet::new(),
            source_bytes: 0,
            entries: BTreeMap::new(),
            aliases: BTreeMap::new(),
        }
    }

    pub fn record_scene(&mut self, vmf: &source_formats::vmf::Vmf) -> Result<(), String> {
        for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
            self.control.checkpoint()?;
            let Some(model) = entity
                .properties
                .get("model")
                .filter(|value| !value.trim().is_empty() && !value.starts_with('*'))
            else {
                continue;
            };
            if !entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
            {
                return Err(format!(
                    "entity:{}: native reflection capture currently supports external models only on prop_static; remove this model from the capture scene",
                    entity.id
                ));
            }
            let path = asset_relative(model).map_err(|error| error.to_string())?;
            let stem = path
                .strip_suffix(".mdl")
                .filter(|_| path.starts_with("models/"))
                .ok_or_else(|| {
                    format!(
                        "entity:{}: reflection model must use a relative models/*.mdl path",
                        entity.id
                    )
                })?;
            let mdl = self.record(&path, source_formats::mdl::MAX_MODEL_BYTES)?;
            let vvd = self.record(&format!("{stem}.vvd"), source_formats::mdl::MAX_MODEL_BYTES)?;
            let vtx = self.record(
                &format!("{stem}.dx90.vtx"),
                source_formats::mdl::MAX_MODEL_BYTES,
            )?;
            let parsed =
                source_formats::model::Model::parse(&mdl, &vvd, &vtx).map_err(|error| {
                    format!("entity:{}: reflection model {path}: {error}", entity.id)
                })?;
            for name in &parsed.materials {
                if !self.record_model_material(name, &parsed.material_directories)? {
                    return Err(format!(
                        "entity:{}: reflection model material was not found: {name}",
                        entity.id
                    ));
                }
            }
        }
        if let Some(sky) = vmf
            .world
            .properties
            .get("skyname")
            .filter(|value| !value.trim().is_empty())
        {
            let sky = material_name(sky)?;
            for face in ["rt", "lf", "bk", "ft", "up", "dn"] {
                self.needs_alias(&format!("skybox/{sky}{face}"))?;
            }
        }
        Ok(())
    }

    fn record_model_material(
        &mut self,
        name: &str,
        directories: &[String],
    ) -> Result<bool, String> {
        for candidate in source_formats::model::material_candidates(name, directories) {
            let candidate = material_name(&candidate)?;
            let path = vmt::material_path(&candidate);
            if self.record_material(&path)?.is_some() {
                self.needs_alias(&candidate)?;
                return Ok(true);
            }
            self.record_missing(&path)?;
        }
        Ok(false)
    }

    fn record(&mut self, path: &str, maximum: usize) -> Result<Vec<u8>, String> {
        self.control.checkpoint()?;
        let bytes = self
            .fs
            .read_limited(path, maximum)
            .map_err(|error| format!("{path}: {error}"))?
            .ok_or_else(|| format!("Reflection dependency was not found: {path}"))?;
        self.record_bytes(path, &bytes)?;
        Ok(bytes)
    }

    fn record_material(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        if let Some(bytes) = self.material_snapshots.get(path) {
            return Ok(Some(bytes.clone()));
        }
        self.control.checkpoint()?;
        let Some(bytes) = self
            .fs
            .read_limited(path, MAX_MATERIAL_BYTES)
            .map_err(|error| format!("{path}: {error}"))?
        else {
            return Ok(None);
        };
        self.record_bytes(path, &bytes)?;
        self.material_snapshots
            .insert(path.to_owned(), bytes.clone());
        Ok(Some(bytes))
    }

    fn record_missing(&mut self, path: &str) -> Result<(), String> {
        if self.sources.contains_key(path) {
            return Err(format!(
                "Reflection source disappeared while preparing the build: {path}"
            ));
        }
        if !self.missing_sources.contains(path) {
            if self.sources.len() + self.missing_sources.len() >= 4096 {
                return Err("Reflection dependency source budget exceeded".into());
            }
            self.missing_sources.insert(path.to_owned());
        }
        Ok(())
    }

    fn record_bytes(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        if self.missing_sources.contains(path) {
            return Err(format!(
                "Reflection source appeared while preparing the build: {path}"
            ));
        }
        let digest = hash(bytes);
        if let Some(old) = self.sources.get(path) {
            if *old != digest {
                return Err(format!(
                    "Reflection source changed while preparing the build: {path}"
                ));
            }
        } else {
            self.source_bytes = self
                .source_bytes
                .checked_add(bytes.len())
                .ok_or("Reflection source byte count overflow")?;
            if self.source_bytes > MAX_GENERATED_BYTES
                || self.sources.len() + self.missing_sources.len() >= 4096
            {
                return Err("Reflection dependency source budget exceeded".into());
            }
            self.sources.insert(path.to_owned(), digest);
        }
        Ok(())
    }

    fn record_patch_chain(
        &mut self,
        path: &str,
        depth: usize,
        active: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        if depth > 8 || !active.insert(path.to_owned()) {
            return Err(format!(
                "Reflection material include cycle/depth exceeded: {path}"
            ));
        }
        let bytes = self
            .record_material(path)?
            .ok_or_else(|| format!("Reflection dependency was not found: {path}"))?;
        let text = std::str::from_utf8(&bytes).map_err(|error| format!("{path}: {error}"))?;
        bound_text(text)?;
        let nodes = keyvalues::parse_lenient(text.trim_start_matches('\u{feff}'))
            .map_err(|error| format!("{path}: {error}"))?;
        let shader = nodes
            .iter()
            .find(|node| node.children().is_some())
            .ok_or_else(|| format!("Material has no shader: {path}"))?;
        if shader.key.eq_ignore_ascii_case("patch") {
            let include = shader
                .children()
                .unwrap()
                .iter()
                .find(|node| node.key.eq_ignore_ascii_case("include"))
                .and_then(Node::as_str)
                .ok_or("Patch material has no include")?;
            let include = asset_relative(include).map_err(|error| error.to_string())?;
            self.record_patch_chain(&include, depth + 1, active)?;
            let base = Material::load_with_reader(&include, &mut |path| {
                Ok(self.material_snapshots.get(path).cloned())
            })
            .map_err(|error| format!("{path}: {error}"))?
            .ok_or_else(|| format!("Patch include was not found: {include}"))?;
            let mut values: BTreeMap<_, _> = base.params.into_iter().collect();
            for edit in shader.children().unwrap().iter().filter(|node| {
                node.key.eq_ignore_ascii_case("replace") || node.key.eq_ignore_ascii_case("insert")
            }) {
                let children = edit.children().ok_or_else(|| {
                    format!("{path}: reflection patch edits require scalar blocks")
                })?;
                for node in children {
                    let value = node.as_str().ok_or_else(|| {
                        format!(
                            "{path}: nested Patch edits are unsupported for reflection generation"
                        )
                    })?;
                    let key = node.key.to_ascii_lowercase();
                    let existing = values.get(&key);
                    if (edit.key.eq_ignore_ascii_case("replace") && existing.is_none())
                        || (edit.key.eq_ignore_ascii_case("insert")
                            && existing.is_some_and(|old| old != value))
                    {
                        return Err(format!(
                            "{path}: ambiguous Patch {} for {key}; replace must name an existing key and insert must not change an existing value",
                            edit.key
                        ));
                    }
                    values.insert(key, value.to_owned());
                }
            }
        }
        active.remove(path);
        Ok(())
    }

    fn load(&mut self, name: &str) -> Result<Material, String> {
        if let Some(material) = self.materials.get(name) {
            return Ok(material.clone());
        }
        self.record_patch_chain(&vmt::material_path(name), 0, &mut BTreeSet::new())?;
        let material = Material::load_with_reader(&vmt::material_path(name), &mut |path| {
            Ok(self.material_snapshots.get(path).cloned())
        })
        .map_err(|error| format!("{name}: {error}"))?
        .ok_or_else(|| format!("Reflection material was not found: {name}"))?;
        for (key, value) in strings(&material_nodes(&material)) {
            if texture_parameter(key)
                && !value.trim().is_empty()
                && !value.starts_with('_')
                && !value.eq_ignore_ascii_case("env_cubemap")
            {
                let path =
                    asset_relative(&vmt::texture_path(value)).map_err(|error| error.to_string())?;
                self.record(&path, MAX_GENERATED_BYTES)?;
            }
        }
        self.materials.insert(name.to_owned(), material.clone());
        Ok(material)
    }

    pub fn needs_alias(&mut self, name: &str) -> Result<bool, String> {
        self.needs_alias_inner(name, &mut BTreeSet::new(), 0)
    }

    fn needs_alias_inner(
        &mut self,
        name: &str,
        active: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<bool, String> {
        self.control.checkpoint()?;
        if let Some(value) = self.reflective.get(name) {
            return Ok(*value);
        }
        if depth > 8 || !active.insert(name.to_owned()) {
            return Err(format!(
                "Reflection dependency cycle/depth exceeded: {name}"
            ));
        }
        let material = self.load(name)?;
        let nodes = material_nodes(&material);
        let mut reflective = false;
        for (key, value) in strings(&nodes) {
            if key.eq_ignore_ascii_case("$envmap") && value.eq_ignore_ascii_case("env_cubemap") {
                reflective = true;
            }
            if DEPENDENCIES
                .iter()
                .any(|dependency| key.eq_ignore_ascii_case(dependency))
                && !value.trim().is_empty()
                && !self_bottom(name, key, value)
            {
                reflective |= self.needs_alias_inner(&material_name(value)?, active, depth + 1)?;
            }
        }
        active.remove(name);
        self.reflective.insert(name.to_owned(), reflective);
        Ok(reflective)
    }

    pub fn generate(
        &mut self,
        name: &str,
        sample: usize,
        samples: &[CubemapSampleAsset],
        namespace: &str,
    ) -> Result<String, String> {
        self.generate_inner(name, sample, samples, namespace, 0)
    }

    fn generate_inner(
        &mut self,
        name: &str,
        sample: usize,
        samples: &[CubemapSampleAsset],
        namespace: &str,
        depth: usize,
    ) -> Result<String, String> {
        self.control.checkpoint()?;
        if depth > 8 {
            return Err("Reflection generated dependency depth exceeded".into());
        }
        if !self.needs_alias(name)? {
            return Ok(name.to_owned());
        }
        let alias = alias_name(namespace, name, samples[sample].origin)?;
        if self.aliases.contains_key(&alias) {
            return Ok(alias);
        }
        if self.aliases.len() >= MAX_ALIASES {
            return Err("Reflection material alias limit exceeded (1024)".into());
        }
        if self
            .fs
            .read_limited(&vmt::material_path(&alias), 1)
            .map_err(|error| format!("Generated alias path collision: {alias}: {error}"))?
            .is_some()
        {
            return Err(format!(
                "Generated alias collides with mounted content: {alias}"
            ));
        }
        let material = self.load(name)?;
        let mut nodes = material_nodes(&material);
        let mut replacements = BTreeMap::new();
        let mut dependencies = BTreeSet::new();
        for (key, value) in strings(&nodes) {
            if DEPENDENCIES
                .iter()
                .any(|dependency| key.eq_ignore_ascii_case(dependency))
                && !value.trim().is_empty()
            {
                let dependency = material_name(value)?;
                let replacement = if self_bottom(name, key, value) {
                    alias.clone()
                } else {
                    self.generate_inner(&dependency, sample, samples, namespace, depth + 1)?
                };
                dependencies.insert(replacement.clone());
                replacements.insert(dependency, replacement);
            }
        }
        let texture = samples[sample]
            .texture
            .strip_prefix("materials/")
            .unwrap()
            .strip_suffix(".vtf")
            .unwrap();
        rewrite_nodes(&mut nodes, texture, &replacements)?;
        let envmaps = strings(&nodes)
            .into_iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("$envmap"))
            .map(|(_, value)| value.to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        ensure_writable(&nodes)?;
        let bytes = keyvalues::try_write(
            &[Node::block(material.shader, nodes)],
            &keyvalues::WriteOptions::default(),
        )
        .map_err(|error| error.to_string())?
        .into_bytes();
        if bytes.len() > MAX_MATERIAL_BYTES
            || self.entries.values().map(Vec::len).sum::<usize>() + bytes.len()
                > MAX_GENERATED_BYTES
        {
            return Err("Reflection generated material byte budget exceeded".into());
        }
        if self.aliases.len() >= MAX_ALIASES {
            return Err("Reflection material alias limit exceeded (1024)".into());
        }
        self.entries
            .insert(vmt::material_path(&alias), bytes.clone());
        self.aliases.insert(
            alias.clone(),
            CubemapAliasAsset {
                material: name.to_owned(),
                alias: alias.clone(),
                sample,
                sha256: hash(&bytes),
                dependencies: dependencies.into_iter().collect(),
                envmaps,
            },
        );
        Ok(alias)
    }

    pub fn into_entries(self) -> Vec<PakEntry> {
        self.entries
            .into_iter()
            .map(|(name, data)| PakEntry { name, data })
            .collect()
    }
}

fn texture_parameter(key: &str) -> bool {
    let Some(key) = key.strip_prefix('$') else {
        return false;
    };
    [
        "ALBEDO",
        "ALPHAMASKTEXTURE",
        "AMBIENTOCCLTEXTURE",
        "ATEXTURE",
        "BASETEXTURE",
        "BASETEXTURE2",
        "BASETEXTURE3",
        "BASETEXTURE4",
        "BLENDMODULATETEXTURE",
        "BLOOMTEXTURE",
        "BLURREDTEXTURE",
        "BLURTEXTURE",
        "BUMPCOMPRESS",
        "BUMPMAP",
        "BUMPMAP2",
        "BUMPMAP3",
        "BUMPMASK",
        "BUMPSTRETCH",
        "CANVAS",
        "CBTEXTURE",
        "CLOUDALPHATEXTURE",
        "COLORBAR",
        "COMPRESS",
        "CORECOLORTEXTURE",
        "CORNEATEXTURE",
        "CRTEXTURE",
        "DELTA",
        "DETAIL",
        "DETAIL2",
        "DISTORTMAP",
        "DUDVMAP",
        "DUST_TEXTURE",
        "EMISSIVEBLENDBASETEXTURE",
        "EMISSIVEBLENDFLOWTEXTURE",
        "EMISSIVEBLENDTEXTURE",
        "ENVMAP",
        "ENVMAPMASK",
        "EXPOSURE_TEXTURE",
        "FBTEXTURE",
        "FLESHBORDERTEXTURE1D",
        "FLESHCUBETEXTURE",
        "FLESHINTERIORNOISETEXTURE",
        "FLESHINTERIORTEXTURE",
        "FLESHNORMALTEXTURE",
        "FLESHSUBSURFACETEXTURE",
        "FLOWMAP",
        "FRAMETEXTURE",
        "FRAME_TEXTURE",
        "GLASSENVMAP",
        "GLINT",
        "GRADIENTTEXTURE",
        "GRAIN",
        "GRAIN_TEXTURE",
        "HDRBASETEXTURE",
        "HDRCOMPRESSEDTEXTURE",
        "HDRCOMPRESSEDTEXTURE0",
        "HDRCOMPRESSEDTEXTURE1",
        "HDRCOMPRESSEDTEXTURE2",
        "INPUT",
        "INPUT_TEXTURE",
        "IRIS",
        "LIGHTMAP",
        "LIGHTWARPTEXTURE",
        "NOISETEXTURE",
        "NORMALMAP",
        "NORMALMAP2",
        "ORIGINALTEXTURE",
        "PHONGEXPONENTTEXTURE",
        "PHONGWARPTEXTURE",
        "PORTALCOLORTEXTURE",
        "PORTALMASKTEXTURE",
        "RAMPTEXTURE",
        "REFLECTTEXTURE",
        "REFRACTTEXTURE",
        "REFRACTTINTEXTURE",
        "SELFILLUMMAP",
        "SELFILLUMMASK",
        "SELFILLUMTEXTURE",
        "SHEENMAP",
        "SHEENMAPMASK",
        "SIDESPEED",
        "SOURCEMRTRENDERTARGET",
        "SRCTEXTURE0",
        "SRCTEXTURE1",
        "SRCTEXTURE2",
        "SRCTEXTURE3",
        "STATICBLENDTEXTURE",
        "STRETCH",
        "STRIPETEXTURE",
        "TEXTURE0",
        "TEXTURE1",
        "TEXTURE2",
        "TEXTURE3",
        "TEXTURE4",
        "VIGNETTE_TEXTURE",
        "VIGNETTE_TILE",
        "WARPTEXTURE",
        "YTEXTURE",
    ]
    .iter()
    .any(|known| key.eq_ignore_ascii_case(known))
}

pub(super) fn bound_text(text: &str) -> Result<(), String> {
    let bytes = text.as_bytes();
    let (mut index, mut depth, mut tokens) = (0usize, 0usize, 0usize);
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index..].starts_with(b"/*") {
            index += 2;
            while index + 1 < bytes.len() && !bytes[index..].starts_with(b"*/") {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        match bytes[index] {
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += 1;
                }
                index = (index + 1).min(bytes.len());
                tokens += 1;
            }
            _ => {
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'{' | b'}' | b'"')
                {
                    index += 1;
                }
                tokens += 1;
            }
        }
        if depth > 32 || tokens > 131_072 {
            return Err("Reflection material text exceeds depth/node limits".into());
        }
    }
    Ok(())
}

pub(super) fn material_nodes(material: &Material) -> Vec<Node> {
    material
        .params
        .iter()
        .map(|(key, value)| Node::string(key, value))
        .chain(material.blocks.iter().cloned())
        .collect()
}

pub(super) fn strings(nodes: &[Node]) -> Vec<(&str, &str)> {
    let mut result = Vec::new();
    for node in nodes {
        match &node.value {
            Value::String(value) => result.push((node.key.as_str(), value.as_str())),
            Value::Block(children) => result.extend(strings(children)),
        }
    }
    result
}

pub(super) fn ensure_writable(nodes: &[Node]) -> Result<(), String> {
    for node in nodes {
        if node.key.contains(['"', '\0', '\n', '\r']) {
            return Err("Reflection material contains an unrepresentable key".into());
        }
        match &node.value {
            Value::String(value) if value.contains(['"', '\0', '\n', '\r']) => {
                return Err("Reflection material contains an unrepresentable value".into());
            }
            Value::Block(children) => ensure_writable(children)?,
            _ => {}
        }
    }
    Ok(())
}

fn rewrite_nodes(
    nodes: &mut [Node],
    texture: &str,
    dependencies: &BTreeMap<String, String>,
) -> Result<(), String> {
    for node in nodes {
        match &mut node.value {
            Value::String(value) => {
                if node.key.eq_ignore_ascii_case("$envmap")
                    && value.eq_ignore_ascii_case("env_cubemap")
                {
                    *value = texture.to_owned();
                } else if DEPENDENCIES
                    .iter()
                    .any(|key| node.key.eq_ignore_ascii_case(key))
                    && !value.trim().is_empty()
                    && let Some(replacement) = dependencies.get(&material_name(value)?)
                {
                    *value = replacement.clone();
                }
            }
            Value::Block(children) => rewrite_nodes(children, texture, dependencies)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
