use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::bsp::PakLimits;
use source_formats::gamefs::asset_relative;
use source_formats::keyvalues::{self, Node};
use source_formats::vmt::{self, Material};
use source_formats::vtf::Vtf;

use super::collection::invalid;
use super::{
    MAX_ASSETS, MAX_FAMILY_BYTES, MAX_MATERIAL_BYTES, MAX_TOTAL_BYTES, SUFFIXES,
    supported_parameter, texture_issues,
};
use crate::Control;

const MAX_RAW_DEPTH: usize = 64;
const MAX_RAW_NODES: usize = 4096;

type RawEdits = Vec<(bool, Vec<(String, String)>)>;

#[derive(Clone)]
struct RawMaterial {
    include: Option<String>,
    edits: RawEdits,
    textures: Vec<(String, String)>,
}

pub(super) struct ClosureSummary {
    pub hdr: bool,
    pub paths: BTreeSet<String>,
}

type ReadAsset<'a> = dyn FnMut(&str, bool) -> io::Result<Vec<u8>> + 'a;

struct Closure<'a, 'b> {
    read: &'a mut ReadAsset<'b>,
    control: &'a Control,
    limits: PakLimits,
    raw: BTreeMap<String, RawMaterial>,
    paths: BTreeSet<String>,
    bytes: usize,
}

impl Closure<'_, '_> {
    fn read(&mut self, path: &str, material: bool) -> io::Result<Vec<u8>> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let path = pak_path(path, if material { "vmt" } else { "vtf" }, self.limits)?;
        if !self.paths.contains(&path) && self.paths.len() >= MAX_ASSETS {
            return Err(invalid("Sky closure exceeds 128 source lookups"));
        }
        let bytes = (self.read)(&path, material)?;
        let maximum = if material {
            MAX_MATERIAL_BYTES
        } else {
            MAX_TOTAL_BYTES
        };
        if bytes.len() > maximum.min(self.limits.max_entry_bytes) {
            return Err(invalid(format!(
                "{path}: sky source exceeds its byte limit"
            )));
        }
        if self.paths.insert(path.clone()) {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .ok_or_else(|| invalid("Sky closure byte size overflow"))?;
            if self.bytes > MAX_TOTAL_BYTES.min(self.limits.max_total_bytes) {
                return Err(invalid("Sky closure exceeds its total byte limit"));
            }
        }
        if material && !self.raw.contains_key(&path) {
            let mut raw = raw_material(&path, &bytes)?;
            if let Some(include) = &raw.include {
                raw.include = Some(pak_path(include, "vmt", self.limits)?);
            }
            self.raw.insert(path, raw);
        }
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

pub(super) fn verify_closure(
    family: &str,
    limits: PakLimits,
    control: &Control,
    read: &mut ReadAsset<'_>,
) -> io::Result<ClosureSummary> {
    validate_family(family)?;
    let mut closure = Closure {
        read,
        control,
        limits,
        raw: BTreeMap::new(),
        paths: BTreeSet::new(),
        bytes: 0,
    };
    let mut materials = Vec::new();
    for suffix in SUFFIXES {
        let path = vmt::material_path(&format!("skybox/{family}{suffix}"));
        materials.push(closure.resolve(&path)?);
        closure.validate_edits(&path, &mut BTreeSet::new())?;
    }
    let declarations = closure
        .raw
        .values()
        .flat_map(|raw| raw.textures.clone())
        .collect::<Vec<_>>();
    let mut validated_textures = BTreeSet::new();
    for (parameter, reference) in declarations {
        control.checkpoint().map_err(io::Error::other)?;
        if reference.len() > limits.max_name_bytes
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
        let path = pak_path(&vmt::texture_path(&reference), "vtf", limits)?;
        if !validated_textures.insert((parameter.clone(), path.clone())) {
            continue;
        }
        let bytes = closure.read(&path, false)?;
        let texture = Vtf::parse(bytes).map_err(|error| invalid(format!("{path}: {error}")))?;
        let issues = texture_issues(&texture, &parameter, parameter == "$hdrbasetexture");
        if !issues.is_empty() {
            return Err(invalid(format!("{path}: {}", issues.join("; "))));
        }
    }
    let hdr = materials
        .iter()
        .any(|material| material.get("$hdrbasetexture").is_some());
    for (suffix, material) in SUFFIXES.into_iter().zip(&materials) {
        control.checkpoint().map_err(io::Error::other)?;
        if material.get("$basetexture").is_none()
            || hdr && (material.shader != "sky" || material.get("$hdrbasetexture").is_none())
            || !hdr && material.shader != "unlitgeneric"
        {
            return Err(invalid(format!(
                "skybox/{family}{suffix}: packing requires six UnlitGeneric LDR faces or six Sky faces with complete LDR and supported HDR bases"
            )));
        }
    }
    Ok(ClosureSummary {
        hdr,
        paths: closure.paths,
    })
}

pub(super) fn validate_family(family: &str) -> io::Result<()> {
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
    Ok(())
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
            "{path}: bracket-wrapped scalar values are outside the supported sky packing subset"
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

pub(super) fn pak_path(path: &str, extension: &str, limits: PakLimits) -> io::Result<String> {
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
