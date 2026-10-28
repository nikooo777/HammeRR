mod closure;
mod raw;
mod report;

pub(crate) use closure::{
    GeneratedLiquidAssets, canonical_material_path, collect_water_closure, logical_material_path,
};
pub use report::*;

use crate::Control;
use crate::asset_sources::{
    CapturedMaterial, MaterialInspectionCapture, MaterialInspectionIssueKind, hash,
};
use raw::Parameter;
use source_formats::gamefs::{AssetOrigin, asset_relative};
use source_formats::{vmt, vtf};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
struct Failure {
    state: WaterDependencyState,
    code: &'static str,
    message: String,
    key: Option<String>,
}

impl Failure {
    fn new(state: WaterDependencyState, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            state,
            code,
            message: message.into(),
            key: None,
        }
    }

    fn key(mut self, key: &str) -> Self {
        self.key = Some(key.into());
        self
    }
}

#[derive(Clone)]
struct Resolved {
    shader: String,
    parameters: BTreeMap<String, Parameter>,
    patched: bool,
}

struct Collector<'a, 'b> {
    sources: &'a mut MaterialInspectionCapture<'b>,
    control: &'a Control,
    report: WaterDependencyReport,
    materials: BTreeMap<String, CapturedMaterial>,
    assets: BTreeMap<String, usize>,
    resolved: BTreeMap<String, Resolved>,
    include_active: BTreeSet<String>,
    material_active: BTreeSet<String>,
    completed: BTreeSet<String>,
    stopped: bool,
}

fn limit(code: &'static str, message: &str) -> Failure {
    Failure::new(WaterDependencyState::Limited, code, message)
}

fn checkpoint(control: &Control) -> Result<(), Failure> {
    control
        .checkpoint()
        .map_err(|error| Failure::new(WaterDependencyState::UnavailableSource, "cancelled", error))
}

fn rank(state: WaterDependencyState) -> u8 {
    match state {
        WaterDependencyState::NotApplicable | WaterDependencyState::SupportedStaticInventory => 0,
        WaterDependencyState::Unsupported => 1,
        WaterDependencyState::UnavailableSource => 2,
        WaterDependencyState::Limited => 3,
    }
}

fn bounded(text: &str, maximum: usize) -> (String, bool) {
    (
        text.chars().take(maximum).collect(),
        text.chars().nth(maximum).is_some(),
    )
}

fn origin(source: &AssetOrigin) -> DependencyOrigin {
    let (kind, mount, file, files) = match source {
        AssetOrigin::Directory { mount, file } => ("directory", mount, Some(file), &[][..]),
        AssetOrigin::Vpk { index, files } => ("vpk", index, None, files.as_slice()),
    };
    let (mount, mut truncated) = bounded(&mount.to_string_lossy(), MAX_ORIGIN_CHARS);
    let file = file.map(|file| {
        let (value, cut) = bounded(&file.to_string_lossy(), MAX_ORIGIN_CHARS);
        truncated |= cut;
        value
    });
    truncated |= files.len() > MAX_MATERIAL_READS;
    let storage_files = files
        .iter()
        .take(MAX_MATERIAL_READS)
        .map(|path| {
            let (value, cut) = bounded(&path.to_string_lossy(), MAX_ORIGIN_CHARS);
            truncated |= cut;
            value
        })
        .collect();
    DependencyOrigin {
        kind,
        mount,
        file,
        storage_files,
        paths_truncated: truncated,
    }
}

fn validate_reference(reference: &str) -> Result<(), Failure> {
    if reference.is_empty()
        || reference.trim() != reference
        || reference.len() > MAX_PATH_BYTES
        || reference.chars().any(char::is_control)
        || reference.starts_with('[')
        || reference.ends_with(']')
    {
        return Err(Failure::new(
            WaterDependencyState::UnavailableSource,
            "unsafe_reference",
            "Asset reference must be a bounded nonblank path without surrounding whitespace or control/bracket syntax",
        ));
    }
    Ok(())
}

fn reference_path(reference: &str, material: bool, include: bool) -> Result<String, Failure> {
    validate_reference(reference)?;
    let safe = asset_relative(reference).map_err(|error| {
        Failure::new(
            WaterDependencyState::UnavailableSource,
            "unsafe_reference",
            error.to_string(),
        )
    })?;
    let path = if include {
        safe
    } else if material {
        vmt::material_path(&safe)
    } else {
        vmt::texture_path(&safe)
    };
    let extension = if material { ".vmt" } else { ".vtf" };
    if path.len() > MAX_PATH_BYTES || !path.starts_with("materials/") || !path.ends_with(extension)
    {
        return Err(Failure::new(
            WaterDependencyState::UnavailableSource,
            "unsafe_reference",
            "Dependency must resolve to a bounded materials VMT/VTF path",
        ));
    }
    Ok(path)
}

impl<'a, 'b> Collector<'a, 'b> {
    fn issue(&mut self, owner: &str, path: Option<&str>, failure: Failure) {
        if rank(failure.state) > rank(self.report.state) {
            self.report.state = failure.state;
        }
        let issue = DependencyIssue {
            state: failure.state,
            code: failure.code,
            owner: bounded(owner, MAX_PATH_BYTES).0,
            key: failure.key.map(|key| bounded(&key, MAX_KEY_BYTES).0),
            path: path.map(|path| bounded(path, MAX_PATH_BYTES).0),
            message: bounded(&failure.message, MAX_MESSAGE_CHARS).0,
        };
        if self.report.issues.len() == MAX_ISSUES {
            self.report.state = WaterDependencyState::Limited;
            self.report.issues[MAX_ISSUES - 1] = DependencyIssue {
                state: WaterDependencyState::Limited,
                code: "issue_limit",
                owner: self.report.root_material.clone(),
                key: None,
                path: None,
                message: "Water dependency issue limit exceeded; inventory is partial".into(),
            };
            self.stopped = true;
        } else {
            self.report.issues.push(issue);
        }
    }

    fn edge(&mut self, edge: DependencyEdge) -> Result<usize, Failure> {
        checkpoint(self.control)?;
        if self.report.edges.len() == MAX_EDGES {
            self.stopped = true;
            return Err(limit("edge_limit", "Water dependency edge limit exceeded"));
        }
        let index = self.report.edges.len();
        self.report.edges.push(edge);
        self.report.counters.edges += 1;
        Ok(index)
    }

    fn reserve_asset(&mut self, path: &str, kind: AssetKind) -> Result<usize, Failure> {
        checkpoint(self.control)?;
        if let Some(&index) = self.assets.get(path) {
            return Ok(index);
        }
        if self.report.assets.len() == MAX_ASSETS {
            return Err(limit(
                "asset_limit",
                "Water dependency asset lookup limit exceeded",
            ));
        }
        if kind == AssetKind::Material && self.report.counters.material_reads == MAX_MATERIAL_READS
        {
            return Err(limit(
                "material_read_limit",
                "Water dependency material read limit exceeded",
            ));
        }
        let index = self.report.assets.len();
        self.assets.insert(path.into(), index);
        self.report.assets.push(DependencyAsset {
            path: path.into(),
            kind,
            state: AssetState::Unavailable,
            bytes: 0,
            sha256: None,
            origin: None,
            shader: None,
            texture: None,
        });
        self.report.counters.asset_lookups += 1;
        if kind == AssetKind::Material {
            self.report.counters.material_reads += 1;
        } else {
            self.report.counters.texture_reads += 1;
        }
        Ok(index)
    }

    fn found(&mut self, index: usize, bytes: &[u8], source: &AssetOrigin) -> Result<(), Failure> {
        let provenance = origin(source);
        let row = &mut self.report.assets[index];
        row.state = AssetState::Found;
        row.bytes = bytes.len();
        row.sha256 = Some(hash(bytes));
        let truncated = provenance.paths_truncated;
        row.origin = Some(provenance);
        self.report.counters.source_bytes += bytes.len();
        if row.kind == AssetKind::Material {
            self.report.counters.material_bytes += bytes.len();
        } else {
            self.report.counters.texture_bytes += bytes.len();
        }
        if truncated {
            return Err(limit(
                "origin_display_limit",
                "Winning source provenance exceeds bounded display",
            ));
        }
        Ok(())
    }

    fn material(&mut self, path: &str, original: bool) -> Result<CapturedMaterial, Failure> {
        if let Some(material) = self.materials.get(path) {
            return Ok(material.clone());
        }
        if original {
            return Err(Failure::new(
                WaterDependencyState::UnavailableSource,
                "uncaptured_original",
                "Original root/Patch include was not captured by the ordinary Source read",
            ));
        }
        let index = self.reserve_asset(path, AssetKind::Material)?;
        let maximum = MAX_MATERIAL_BYTES
            .min(MAX_MATERIAL_TOTAL_BYTES.saturating_sub(self.report.counters.material_bytes))
            .min(MAX_SOURCE_BYTES.saturating_sub(self.report.counters.source_bytes));
        if maximum == 0 {
            return Err(limit(
                "material_byte_limit",
                "Water dependency material/source byte limit exceeded",
            ));
        }
        let read = self.sources.read_material(path, maximum).map_err(|error| {
            let state = if error.to_string().contains("byte read limit")
                || error.to_string().contains("beyond its byte read limit")
            {
                WaterDependencyState::Limited
            } else {
                WaterDependencyState::UnavailableSource
            };
            Failure::new(state, "material_read", error.to_string())
        })?;
        let Some(material) = read else {
            self.report.assets[index].state = AssetState::Missing;
            return Err(Failure::new(
                WaterDependencyState::UnavailableSource,
                "missing_material",
                "Material dependency was not found",
            ));
        };
        self.found(index, material.bytes(), material.origin())?;
        self.materials.insert(path.into(), material.clone());
        Ok(material)
    }

    fn resolve(&mut self, path: &str, depth: usize, original: bool) -> Result<Resolved, Failure> {
        checkpoint(self.control)?;
        if depth > MAX_DEPTH {
            return Err(limit(
                "material_depth_limit",
                "Water material/Patch depth exceeds eight",
            ));
        }
        if self.include_active.contains(path) {
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "include_cycle",
                "Cyclic Patch material includes",
            ));
        }
        if let Some(resolved) = self.resolved.get(path) {
            return Ok(resolved.clone());
        }
        self.include_active.insert(path.into());
        let result = self.resolve_inner(path, depth, original);
        self.include_active.remove(path);
        if let Ok(resolved) = &result {
            self.resolved.insert(path.into(), resolved.clone());
        }
        result
    }

    fn resolve_inner(
        &mut self,
        path: &str,
        depth: usize,
        original: bool,
    ) -> Result<Resolved, Failure> {
        let source = self.material(path, original)?;
        let raw = raw::parse(source.bytes())?;
        if let Some(&index) = self.assets.get(path) {
            self.report.assets[index].shader = Some(raw.shader.clone());
        }
        let Some(include) = raw.include else {
            return Ok(Resolved {
                shader: raw.shader,
                parameters: raw.parameters,
                patched: false,
            });
        };
        let target = reference_path(&include.value, true, true)?;
        let edge = self.edge(DependencyEdge {
            owner: path.into(),
            key: include.key,
            reference: include.value,
            target: Some(target.clone()),
            role: DependencyRole::PatchInclude,
            origin: DeclarationOrigin::Authored,
            state: EdgeState::Mounted,
            evidence_basis: "sdk",
        })?;
        let result = self.resolve(&target, depth + 1, original);
        let mut resolved = match result {
            Ok(value) => value,
            Err(error) => {
                self.report.edges[edge].state = if self
                    .assets
                    .get(&target)
                    .is_some_and(|&row| self.report.assets[row].state == AssetState::Missing)
                {
                    EdgeState::Missing
                } else {
                    edge_failure(error.state)
                };
                return Err(error);
            }
        };
        for (replace, edits) in raw.edits {
            for (key, value) in edits {
                let existing = resolved.parameters.get(&key);
                if replace && existing.is_none()
                    || !replace && existing.is_some_and(|old| old.value != value.value)
                {
                    return Err(Failure::new(WaterDependencyState::Unsupported, "patch_conflict", "Patch replace requires an existing key; insert cannot alter an existing value").key(&value.key));
                }
                resolved.parameters.insert(key, value);
            }
        }
        resolved.patched = true;
        if let Some(&index) = self.assets.get(path) {
            self.report.assets[index].shader = Some(resolved.shader.clone());
        }
        Ok(resolved)
    }

    fn walk(&mut self, path: &str, depth: usize, original: bool) -> Result<(), Failure> {
        checkpoint(self.control)?;
        if depth > MAX_DEPTH {
            return Err(limit(
                "material_depth_limit",
                "Water material edge depth exceeds eight",
            ));
        }
        if self.material_active.contains(path) {
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "material_cycle",
                "Cyclic Water material dependency",
            ));
        }
        if self.completed.contains(path) {
            return Ok(());
        }
        let resolved = self.resolve(path, depth, original)?;
        if !is_water_shader(&resolved.shader) {
            if path == self.report.root_material {
                self.report.state = WaterDependencyState::NotApplicable;
                return Ok(());
            }
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "unsupported_shader",
                format!(
                    "Child shader {} is outside the Water-family inventory",
                    resolved.shader
                ),
            ));
        }
        self.material_active.insert(path.into());
        for (key, role, default) in [
            (
                "$normalmap",
                DependencyRole::NormalTexture,
                Some("dev/water_normal"),
            ),
            ("$basetexture", DependencyRole::ColorTexture, None),
            (
                "$reflecttexture",
                DependencyRole::ColorTexture,
                Some("_rt_WaterReflection"),
            ),
            (
                "$refracttexture",
                DependencyRole::ColorTexture,
                Some("_rt_WaterRefraction"),
            ),
            (
                "$envmap",
                DependencyRole::CubemapTexture,
                Some("env_cubemap"),
            ),
            ("%tooltexture", DependencyRole::EditorPreviewTexture, None),
            ("$bottommaterial", DependencyRole::CompilerMaterial, None),
            ("$underwateroverlay", DependencyRole::RuntimeMaterial, None),
        ] {
            if self.stopped {
                break;
            }
            let parameter = resolved.parameters.get(key);
            let Some(reference) = parameter.map(|value| value.value.as_str()).or(default) else {
                continue;
            };
            let raw_key = parameter.map_or(key, |value| value.key.as_str());
            let declaration = if parameter.is_none() {
                DeclarationOrigin::SdkDefault
            } else if resolved.patched {
                DeclarationOrigin::PatchEffective
            } else {
                DeclarationOrigin::Authored
            };
            if let Err(error) = self.dependency(path, raw_key, reference, role, declaration, depth)
            {
                self.issue(path, Some(reference), error.key(raw_key));
            }
        }
        self.material_active.remove(path);
        self.completed.insert(path.into());
        Ok(())
    }

    fn dependency(
        &mut self,
        owner: &str,
        key: &str,
        reference: &str,
        role: DependencyRole,
        declaration: DeclarationOrigin,
        depth: usize,
    ) -> Result<(), Failure> {
        let index = self.edge(DependencyEdge {
            owner: owner.into(),
            key: key.into(),
            reference: reference.into(),
            target: None,
            role,
            origin: declaration,
            state: EdgeState::Mounted,
            evidence_basis: if role == DependencyRole::RuntimeMaterial {
                "installed_client_and_sdk"
            } else {
                "sdk"
            },
        })?;
        let result = self.dependency_inner(index, key, reference, role, depth);
        if let Err(error) = &result
            && self.report.edges[index].state != EdgeState::Missing
        {
            self.report.edges[index].state = edge_failure(error.state);
        }
        result
    }

    fn dependency_inner(
        &mut self,
        index: usize,
        key: &str,
        reference: &str,
        role: DependencyRole,
        depth: usize,
    ) -> Result<(), Failure> {
        validate_reference(reference)?;
        let canonical = match key.to_ascii_lowercase().as_str() {
            "$reflecttexture" => Some("_rt_WaterReflection"),
            "$refracttexture" => Some("_rt_WaterRefraction"),
            "$envmap" => Some("env_cubemap"),
            _ => None,
        };
        if canonical.is_some_and(|value| reference.eq_ignore_ascii_case(value)) {
            self.report.edges[index].state = EdgeState::RuntimeResource;
            return Ok(());
        }
        if role == DependencyRole::RuntimeMaterial && reference == "0" {
            self.report.edges[index].state = EdgeState::Disabled;
            return Ok(());
        }
        if reference.starts_with('_')
            || reference.eq_ignore_ascii_case("env_cubemap")
            || role == DependencyRole::RuntimeMaterial && reference.starts_with('0')
        {
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "unsupported_resource",
                "Unrecognized or misplaced runtime resource/disable spelling",
            ));
        }
        let material = matches!(
            role,
            DependencyRole::CompilerMaterial | DependencyRole::RuntimeMaterial
        );
        let path = reference_path(reference, material, false)?;
        self.report.edges[index].target = Some(path.clone());
        let result = if material {
            self.walk(&path, depth + 1, false)
        } else {
            self.texture(&path, role)
        };
        if result.is_err()
            && self
                .assets
                .get(&path)
                .is_some_and(|&row| self.report.assets[row].state == AssetState::Missing)
        {
            self.report.edges[index].state = EdgeState::Missing;
        }
        result
    }

    fn texture(&mut self, path: &str, role: DependencyRole) -> Result<(), Failure> {
        let index = if let Some(&index) = self.assets.get(path) {
            index
        } else {
            let index = self.reserve_asset(path, AssetKind::Texture)?;
            let maximum = MAX_TEXTURE_BYTES
                .min(MAX_SOURCE_BYTES.saturating_sub(self.report.counters.source_bytes));
            if maximum == 0 {
                return Err(limit(
                    "source_byte_limit",
                    "Water source payload limit exceeded",
                ));
            }
            let asset = self.sources.read_texture(path, maximum).map_err(|error| {
                let state = if error.to_string().contains("byte read limit")
                    || error.to_string().contains("beyond its byte read limit")
                {
                    WaterDependencyState::Limited
                } else {
                    WaterDependencyState::UnavailableSource
                };
                Failure::new(state, "texture_read", error.to_string())
            })?;
            let Some(asset) = asset else {
                self.report.assets[index].state = AssetState::Missing;
                return Err(Failure::new(
                    WaterDependencyState::UnavailableSource,
                    "missing_texture",
                    "Texture dependency was not found",
                ));
            };
            self.found(index, &asset.data, &asset.origin)?;
            let texture = match vtf::Vtf::parse(asset.data) {
                Ok(texture) => texture,
                Err(error) => {
                    let state = if matches!(
                        error,
                        vtf::Error::UnsupportedVersion(_, _) | vtf::Error::UnsupportedFormat(_)
                    ) {
                        WaterDependencyState::Unsupported
                    } else {
                        WaterDependencyState::UnavailableSource
                    };
                    self.report.assets[index].state = if state == WaterDependencyState::Unsupported
                    {
                        AssetState::UnsupportedTexture
                    } else {
                        AssetState::Unavailable
                    };
                    return Err(Failure::new(state, "texture_layout", error.to_string()));
                }
            };
            self.report.assets[index].texture = Some(TextureObservation {
                layout_scope: "existing_vtf_parser",
                version: [texture.version.0, texture.version.1],
                width: texture.width,
                height: texture.height,
                depth: texture.depth,
                authored_depth: texture.authored_depth,
                authored_frames: texture.authored_frames,
                first_frame: texture.first_frame,
                faces: texture.faces,
                mip_count: texture.mip_count,
                flags: texture.flags,
                format: format!("{:?}", texture.format),
            });
            if texture.width > MAX_TEXTURE_AXIS
                || texture.height > MAX_TEXTURE_AXIS
                || texture.depth != 1
                || texture.authored_depth.is_some_and(|depth| depth != 1)
                || texture.authored_frames != 1
            {
                self.report.assets[index].state = AssetState::UnsupportedTexture;
                return Err(Failure::new(
                    WaterDependencyState::Unsupported,
                    "static_texture_subset",
                    "Texture is outside the bounded one-frame, depth-one static subset",
                ));
            }
            index
        };
        let row = &self.report.assets[index];
        if row.state == AssetState::UnsupportedTexture {
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "static_texture_subset",
                "Texture is outside the static subset",
            ));
        }
        let Some(texture) = &row.texture else {
            return Err(Failure::new(
                WaterDependencyState::UnavailableSource,
                "texture_unavailable",
                "A prior texture observation was unavailable",
            ));
        };
        if role != DependencyRole::EditorPreviewTexture
            && let Err(error) = vtf::validate_native_mips(
                [texture.width, texture.height, texture.depth],
                texture.flags,
                texture.mip_count,
            )
        {
            self.report.assets[index].state = AssetState::UnsupportedTexture;
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "incomplete_mip_chain",
                error.to_string(),
            ));
        }
        let cube = texture.flags & 0x4000 != 0;
        let valid = if role == DependencyRole::CubemapTexture {
            cube && texture.width == texture.height && matches!(texture.faces, 6 | 7)
        } else {
            !cube && texture.faces == 1
        };
        if !valid {
            return Err(Failure::new(
                WaterDependencyState::Unsupported,
                "texture_role",
                "Observed VTF shape does not match this 2D/cubemap dependency role",
            ));
        }
        Ok(())
    }
}

fn edge_failure(state: WaterDependencyState) -> EdgeState {
    if state == WaterDependencyState::Unsupported {
        EdgeState::Unsupported
    } else {
        EdgeState::Unavailable
    }
}

pub fn is_water_shader(shader: &str) -> bool {
    ["water", "water_dx90", "water_dx9_hdr"]
        .iter()
        .any(|known| shader.eq_ignore_ascii_case(known))
}

pub(crate) fn native_self_bottom(name: &str, key: &str, value: &str) -> bool {
    key.eq_ignore_ascii_case("$bottommaterial") && value.eq_ignore_ascii_case(name)
}

pub fn inspect_water_dependencies(
    root_vmt: &str,
    sources: &mut MaterialInspectionCapture<'_>,
    control: &Control,
) -> WaterDependencyReport {
    let root = match asset_relative(root_vmt) {
        Ok(root) if root.len() <= MAX_PATH_BYTES => root,
        _ => {
            return WaterDependencyReport::unavailable_source(
                root_vmt,
                "Root VMT path is unsafe or exceeds 512 bytes",
            );
        }
    };
    let captured: Vec<_> = sources.materials().cloned().collect();
    let original: BTreeSet<_> = captured
        .iter()
        .map(|value| value.path().to_owned())
        .collect();
    if !original.contains(&root) {
        return WaterDependencyReport::unavailable_source(
            &root,
            "Root must be captured by the original Source reader before dependency inspection",
        );
    }
    let mut collector = Collector {
        sources,
        control,
        report: WaterDependencyReport::new(&root),
        materials: BTreeMap::new(),
        assets: BTreeMap::new(),
        resolved: BTreeMap::new(),
        include_active: BTreeSet::new(),
        material_active: BTreeSet::new(),
        completed: BTreeSet::new(),
        stopped: false,
    };
    for material in captured {
        let path = material.path().to_owned();
        let result = collector
            .reserve_asset(&path, AssetKind::Material)
            .and_then(|index| {
                if material.bytes().len() > MAX_MATERIAL_BYTES
                    || collector
                        .report
                        .counters
                        .material_bytes
                        .saturating_add(material.bytes().len())
                        > MAX_MATERIAL_TOTAL_BYTES
                {
                    return Err(limit(
                        "material_byte_limit",
                        "Original captured material payload exceeds inventory limits",
                    ));
                }
                collector.found(index, material.bytes(), material.origin())
            });
        if let Err(error) = result {
            collector.issue(&root, Some(&path), error);
            collector.stopped = true;
            break;
        }
        collector.materials.insert(path, material);
    }
    if collector.sources.issue().is_some() {
        collector.stopped = true;
    }
    if !collector.stopped
        && let Err(error) = collector.walk(&root, 0, true)
    {
        collector.issue(&root, Some(&root), error);
    }
    if let Err(error) = collector.sources.verify_originals() {
        let (state, code, path, message) = match collector.sources.issue() {
            Some(issue) => (
                if issue.kind == MaterialInspectionIssueKind::Limited {
                    WaterDependencyState::Limited
                } else {
                    WaterDependencyState::UnavailableSource
                },
                if issue.kind == MaterialInspectionIssueKind::Limited {
                    "source_limit"
                } else {
                    "source_changed"
                },
                if issue.path.is_empty() {
                    None
                } else {
                    Some(issue.path.clone())
                },
                issue.message.clone(),
            ),
            None => (
                WaterDependencyState::UnavailableSource,
                "source_changed",
                None,
                error.to_string(),
            ),
        };
        collector.issue(&root, path.as_deref(), Failure::new(state, code, message));
    }
    collector.report
}
