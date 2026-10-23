use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};

use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId};
use source_formats::vmf::{Entity, Solid, Vmf, parse_vec3};

use crate::{Control, Diagnostic, FileIdentity, FileStamp, Report};

mod bindings;
pub(crate) mod brush_models;
mod cubemaps;
mod entities;
mod expand;
mod names;
mod occluders;
mod overlays;
mod props;
mod spawns;
mod terrain;
mod visclusters;

pub const MAX_INSTANCES: usize = 256;
pub const MAX_EXPANSION_DEPTH: usize = 4;
pub const MAX_FILES: usize = 32;
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_OBJECTS: usize = 16_384;
pub const MAX_SIDES: usize = 65_536;
pub const MAX_BRUSH_SIDES: usize = 128;
pub const MAX_FILE_NODES: usize = 131_072;
pub const MAX_TOTAL_NODES: usize = 1_048_576;
pub const MAX_DEPTH: usize = 32;
pub const MAX_FIELD_BYTES: usize = 512;
pub const MAX_GEOMETRY_WORK: usize = 64_000_000;
pub const MAX_PARENT_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_TERRAIN_SOURCE_VERTICES: usize = 65_536;
pub const MAX_TERRAIN_EXPANDED_VERTICES: usize = 262_144;
pub const MAX_TERRAIN_GRID_WORK: usize = 2_097_152;
pub const MAX_LIGHTING_ORIGIN_NAME_BYTES: usize = 127;

#[derive(Clone, Debug)]
pub struct SourceLocation {
    requested: PathBuf,
    alias: FileStamp,
    directory_alias: Option<FileStamp>,
    directory_identity: Option<FileStamp>,
    root: FileStamp,
    directory: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ReferencePath {
    source: SourceLocation,
    relative: String,
    path: PathBuf,
    bytes: u64,
    identity: FileStamp,
}

#[derive(Clone, Debug)]
pub struct NewReferencePath {
    source: SourceLocation,
    relative: String,
    path: PathBuf,
    directories: Vec<FileStamp>,
}

impl NewReferencePath {
    pub fn relative(&self) -> &str {
        &self.relative
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify_metadata(&self) -> io::Result<()> {
        self.source.verify_metadata()?;
        for directory in &self.directories {
            directory.verify_container_identity()?;
        }
        if walk_reference_path(&self.source.directory, &self.relative, true)?.0 != self.path {
            return Err(io::Error::other(
                "New instance reference resolution changed",
            ));
        }
        self.source.verify_metadata()
    }
}

impl ReferencePath {
    pub fn relative(&self) -> &str {
        &self.relative
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn verify_metadata(&self) -> io::Result<()> {
        self.source.verify_metadata()?;
        if resolve_path_in(&self.source.directory, &self.relative)? != self.path {
            return Err(io::Error::other("Instance reference resolution changed"));
        }
        self.identity.verify_metadata()?;
        self.source.verify_metadata()
    }
}

impl SourceLocation {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let requested = std::path::absolute(path)?;
        let alias = FileStamp::capture_container(&requested)?;
        let parent = requested
            .parent()
            .ok_or_else(|| io::Error::other("Instance source has no parent directory"))?;
        let directory_alias = parent
            .file_name()
            .map(|_| FileStamp::capture_container(parent))
            .transpose()?;
        let directory = parent.canonicalize()?;
        let directory_identity = directory
            .file_name()
            .map(|_| FileStamp::capture_container(&directory))
            .transpose()?;
        let physical = requested.canonicalize()?;
        if std::fs::metadata(&physical)?.len() > MAX_PARENT_BYTES {
            return Err(io::Error::other("Saved instance parent exceeds 128 MiB"));
        }
        let location = Self {
            requested,
            alias,
            directory_alias,
            directory_identity,
            root: FileStamp::capture(&physical)?,
            directory,
        };
        location.verify_metadata()?;
        Ok(location)
    }

    pub fn reference(&self, value: &str) -> io::Result<ReferencePath> {
        self.verify_metadata()?;
        let relative = filename(value).map_err(io::Error::other)?;
        let path = resolve_path_in(&self.directory, &relative)?;
        let identity = FileStamp::capture(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(io::Error::other(
                "Instance reference must be a regular file of at most 4 MiB",
            ));
        }
        if identity.same_file(&self.root) {
            return Err(io::Error::other(
                "Instance reference forms a self-reference cycle",
            ));
        }
        let reference = ReferencePath {
            source: self.clone(),
            relative,
            path,
            bytes: metadata.len(),
            identity,
        };
        reference.verify_metadata()?;
        Ok(reference)
    }

    pub fn new_reference(&self, value: &str) -> io::Result<NewReferencePath> {
        self.verify_metadata()?;
        let relative = filename(value).map_err(io::Error::other)?;
        let (path, directories) = walk_reference_path(&self.directory, &relative, true)?;
        let reference = NewReferencePath {
            source: self.clone(),
            relative,
            path,
            directories,
        };
        reference.verify_metadata()?;
        Ok(reference)
    }

    pub fn reference_from_path(&self, selected: &Path) -> io::Result<ReferencePath> {
        if !selected.is_absolute() {
            return Err(io::Error::other("Choose an absolute instance filename"));
        }
        self.verify_metadata()?;
        let requested_parent = self
            .requested
            .parent()
            .ok_or_else(|| io::Error::other("Instance source has no parent directory"))?;
        let relative = selected
            .strip_prefix(requested_parent)
            .or_else(|_| selected.strip_prefix(&self.directory))
            .map_err(|_| {
                io::Error::other("Choose an instance beneath the referring map directory")
            })?;
        let value = relative
            .to_str()
            .ok_or_else(|| io::Error::other("Instance filename must be valid UTF-8"))?;
        let normalized = filename(value).map_err(io::Error::other)?;
        if Path::new(&normalized) != relative {
            return Err(io::Error::other("Selected instance filename is ambiguous"));
        }
        self.reference(&normalized)
    }

    pub fn verify_metadata(&self) -> io::Result<()> {
        self.alias.verify_metadata()?;
        if let Some(alias) = &self.directory_alias {
            alias.verify_container_identity()?;
        }
        if let Some(directory) = &self.directory_identity {
            directory.verify_container_identity()?;
        }
        if self.requested.canonicalize()? != self.root.path()
            || self
                .requested
                .parent()
                .map(Path::canonicalize)
                .transpose()?
                .as_deref()
                != Some(self.directory.as_path())
        {
            return Err(io::Error::other(
                "Instance referring filename or directory changed",
            ));
        }
        self.root.verify_metadata()
    }

    pub fn hydrate(self, control: &Control) -> io::Result<SourceContext> {
        self.verify_metadata()?;
        let root = FileIdentity::capture_bounded(self.root.path(), MAX_PARENT_BYTES, control)?;
        self.root.verify_identity(&root)?;
        self.verify_metadata()?;
        Ok(SourceContext {
            requested: self.requested,
            alias: self.alias,
            directory_alias: self.directory_alias,
            directory_identity: self.directory_identity,
            root,
            directory: self.directory,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SourceContext {
    requested: PathBuf,
    alias: FileStamp,
    directory_alias: Option<FileStamp>,
    directory_identity: Option<FileStamp>,
    root: FileIdentity,
    directory: PathBuf,
}

impl SourceContext {
    pub fn capture(path: &Path, control: &Control) -> io::Result<Self> {
        SourceLocation::capture(path)?.hydrate(control)
    }

    pub fn path(&self) -> &Path {
        &self.requested
    }
    pub fn root_identity(&self) -> &FileIdentity {
        &self.root
    }
    pub fn verify_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        self.root.verify_bytes(bytes)?;
        self.verify_metadata()
    }
    pub fn verify(&self, control: &Control) -> io::Result<()> {
        self.verify_metadata()?;
        self.root.verify(control)?;
        self.verify_metadata()
    }
    pub fn verify_metadata(&self) -> io::Result<()> {
        self.alias.verify_metadata()?;
        if let Some(alias) = &self.directory_alias {
            alias.verify_container_identity()?;
        }
        if let Some(directory) = &self.directory_identity {
            directory.verify_container_identity()?;
        }
        if self.requested.canonicalize()? != self.root.path()
            || self
                .requested
                .parent()
                .ok_or_else(|| io::Error::other("Instance source has no parent directory"))?
                .canonicalize()?
                != self.directory
        {
            return Err(io::Error::other(
                "Instance referring filename or directory changed",
            ));
        }
        self.root.verify_metadata()
    }
}

#[derive(Clone, Debug)]
pub struct DependencyGuard {
    source: SourceContext,
    files: Vec<(String, FileIdentity)>,
}

impl DependencyGuard {
    pub fn verify_metadata(&self) -> io::Result<()> {
        self.source.verify_metadata()?;
        for (relative, identity) in &self.files {
            if resolve_path(&self.source, relative)? != identity.path() {
                return Err(io::Error::other("Instance dependency resolution changed"));
            }
            identity.verify_metadata()?;
        }
        Ok(())
    }

    pub fn verify(&self, control: &Control) -> io::Result<()> {
        self.verify_metadata()?;
        self.source.verify(control)?;
        for (_, identity) in &self.files {
            identity.verify(control)?;
        }
        self.verify_metadata()
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct DependencyInfo {
    pub path: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct InstanceStats {
    pub instances: usize,
    pub max_expansion_depth: usize,
    pub files: usize,
    pub world_brushes: usize,
    pub detail_brushes: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub brush_models: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub consumed_origin_brushes: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub terrain_patches: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub terrain_vertices: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub point_lights: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub spot_lights: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub environment_lights: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub static_props: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub lighting_origins: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub player_spawns: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub visclusters: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub occluders: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub overlays: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub cubemaps: usize,
    pub dependencies: Vec<DependencyInfo>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

pub fn requires_source(vmf: &Vmf) -> bool {
    vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    })
}

fn field(value: &str) -> Result<(), String> {
    if value.len() > MAX_FIELD_BYTES || value.contains('\0') {
        return Err(format!(
            "Instance field exceeds {MAX_FIELD_BYTES} bytes or contains NUL"
        ));
    }
    Ok(())
}

fn vector(entity: &Entity, key: &str) -> Result<DVec3, String> {
    let Some(value) = entity.properties.get(key) else {
        return Ok(DVec3::ZERO);
    };
    field(value)?;
    parse_vec3(value)
        .filter(|point| point.is_finite() && point.as_vec3().is_finite())
        .ok_or_else(|| format!("Instance {key} requires three finite coordinates"))
}

pub fn filename(value: &str) -> Result<String, String> {
    field(value)?;
    let relative = value.replace('\\', "/");
    if relative.is_empty()
        || relative.contains(':')
        || relative.bytes().any(|byte| byte.is_ascii_control())
        || !Path::new(&relative)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        || !Path::new(&relative)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("vmf"))
    {
        return Err("Instance file must be a relative .vmf path without traversal, absolute roots or drive prefixes".into());
    }
    Ok(Path::new(&relative)
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned())
}

fn resolve_path(source: &SourceContext, relative: &str) -> io::Result<PathBuf> {
    resolve_path_in(&source.directory, relative)
}

fn resolve_path_in(directory: &Path, relative: &str) -> io::Result<PathBuf> {
    walk_reference_path(directory, relative, false).map(|(path, _)| path)
}

fn walk_reference_path(
    directory: &Path,
    relative: &str,
    absent_leaf: bool,
) -> io::Result<(PathBuf, Vec<FileStamp>)> {
    let relative = filename(relative).map_err(io::Error::other)?;
    let mut path = directory.to_owned();
    let mut directories = Vec::new();
    let mut components = Path::new(&relative).components().peekable();
    while let Some(part) = components.next() {
        path.push(part);
        if absent_leaf && components.peek().is_none() {
            return match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((path, directories)),
                Err(error) => Err(error),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "New instance reference destination already exists",
                )),
            };
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "Instance dependency paths cannot contain symlinks",
            ));
        }
        if absent_leaf {
            if !metadata.is_dir() {
                return Err(io::Error::other(
                    "New instance reference parents must be existing directories",
                ));
            }
            directories.push(FileStamp::capture_container(&path)?);
        }
    }
    Ok((path, directories))
}

pub fn helper_admission(entity: &Entity) -> Result<(), String> {
    if !entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    {
        return Err("Only func_instance supports brush-only expansion; parameter helpers remain unsupported".into());
    }
    if !entity.solids.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
        || !entity.extra.is_empty()
    {
        return Err("Brush instances cannot have brushes, connections or opaque blocks".into());
    }
    let mut keys = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(format!("Instance repeats property {key}"));
        }
        if lower.starts_with("replace") {
            bindings::replacement(key, value)
                .map_err(|error| format!("Instance {key}: {error}"))?;
        } else if !matches!(
            lower.as_str(),
            "classname" | "origin" | "angles" | "file" | "fixup_style" | "targetname" | "name"
        ) {
            return Err(format!(
                "Instance property {key} is unsupported by brush-only expansion"
            ));
        }
    }
    bindings::Bindings::new(entity)?;
    filename(
        entity
            .properties
            .get("file")
            .ok_or("Instance file is missing")?,
    )?;
    let origin = vector(entity, "origin")?;
    if origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Instance origin exceeds Source compile bounds".into());
    }
    vector(entity, "angles")?;
    if let Some(style) = entity.properties.get("fixup_style")
        && !matches!(style.trim(), "0" | "1" | "2")
    {
        return Err("Instance fixup_style must be 0, 1 or 2".into());
    }
    Ok(())
}

#[cfg(test)]
fn checked_solid(solid: &Solid) -> Result<(), String> {
    checked_solid_controlled(solid, &Control::default())
}

fn checked_solid_controlled(solid: &Solid, control: &Control) -> Result<(), String> {
    checked_solid_geometry_controlled(solid, control).map(|_| ())
}

pub(crate) fn checked_solid_geometry_controlled(
    solid: &Solid,
    control: &Control,
) -> Result<BrushGeometry, String> {
    if solid.sides.len() > MAX_BRUSH_SIDES
        || !solid.extra.is_empty()
        || !solid.extra_properties.0.is_empty()
    {
        return Err(format!(
            "Child brush {} exceeds {MAX_BRUSH_SIDES} sides or has opaque metadata",
            solid.id
        ));
    }
    for side in &solid.sides {
        field(&side.material)?;
        if !side.rotation.is_finite() || side.lightmap_scale == 0 {
            return Err(format!(
                "Child side {} has invalid rotation or lightmap scale",
                side.id
            ));
        }
        if !side.extra.is_empty() || !side.extra_properties.0.is_empty() {
            return Err(format!("Child side {} has opaque metadata", side.id));
        }
        for axis in [&side.u_axis, &side.v_axis] {
            if !axis.axis.is_finite()
                || axis.axis.length_squared() <= 0.0
                || !axis.scale.is_finite()
                || axis.scale == 0.0
                || !axis.offset.is_finite()
                || !(axis.axis / axis.scale).as_vec3().is_finite()
                || !(axis.offset as f32).is_finite()
            {
                return Err(format!(
                    "Child side {} has invalid texture coordinates",
                    side.id
                ));
            }
        }
    }
    let geometry = BrushGeometry::from_solid(solid)
        .map_err(|error| format!("Child brush {}: {error}", solid.id))?;
    if !geometry.mins.is_finite()
        || !geometry.maxs.is_finite()
        || !geometry.mins.cmplt(geometry.maxs).all()
        || geometry.mins.abs().max(geometry.maxs.abs()).max_element()
            > crate::map::MAX_COMPILE_COORD
    {
        return Err(format!(
            "Child brush {} exceeds finite Source compile bounds",
            solid.id
        ));
    }
    terrain::check_solid(solid, &geometry, control)?;
    Ok(geometry)
}

fn child_admission(child: &Vmf, control: &Control) -> Result<(), String> {
    child.validate_ids().map_err(|error| error.to_string())?;
    if child
        .extra
        .iter()
        .any(|node| !node.key.eq_ignore_ascii_case("cordons"))
        || !child.world.extra.is_empty()
        || !child.visgroups_extra.is_empty()
    {
        return Err("Instance child has unknown or compiler-owned map metadata".into());
    }
    if map_core::compile_region::active(child)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("Active child compile regions are unsupported".into());
    }
    for entity in child.entities.iter().filter(|entity| !entity.hidden) {
        control.checkpoint()?;
        if entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        {
            helper_admission(entity)
                .map_err(|error| format!("Child instance {}: {error}", entity.id))?;
            continue;
        }
        if let Some(kind) = entities::kind(entity) {
            entities::admit_template(entity, kind)
                .map_err(|error| format!("Child {} {}: {error}", kind.label(), entity.id))?;
            continue;
        }
        if brush_models::is_model(entity) {
            brush_models::admit(entity)
                .map_err(|error| format!("Child brush model {}: {error}", entity.id))?;
            continue;
        }
        if visclusters::is_helper(entity) {
            visclusters::admit(entity)
                .map_err(|error| format!("Child viscluster {}: {error}", entity.id))?;
            continue;
        }
        if occluders::is_helper(entity) {
            occluders::admit(entity)
                .map_err(|error| format!("Child occluder {}: {error}", entity.id))?;
            continue;
        }
        if overlays::is_helper(entity) {
            overlays::admit(entity)
                .map_err(|error| format!("Child overlay {}: {error}", entity.id))?;
            continue;
        }
        if cubemaps::is_helper(entity) {
            cubemaps::admit(entity)
                .map_err(|error| format!("Child cubemap {}: {error}", entity.id))?;
            continue;
        }
        for (key, value) in entity.properties.iter() {
            field(key)?;
            field(value)?;
        }
        if !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_detail"))
        {
            return Err(format!(
                "Child entity {} ({}) is unsupported; only world/func_detail brushes, bounded unnamed ordinary func_brush models, unnamed static point/spot/environment lights, bounded static props, scoped info_lighting helpers, neutral player spawns, consumed func_viscluster volumes, unnamed stationary func_occluder helpers, bounded unnamed info_overlay projections, bounded env_cubemap samples and bounded func_instance helpers are supported",
                entity.id,
                entity.classname().unwrap_or("missing class")
            ));
        }
        if !entity.extra.is_empty()
            || entity
                .connections
                .as_ref()
                .is_some_and(|pairs| !pairs.is_empty())
        {
            return Err("Child detail entity has opaque blocks or connections".into());
        }
        let mut seen = BTreeSet::new();
        for (key, value) in entity.properties.iter() {
            if !seen.insert(key.to_ascii_lowercase()) {
                return Err(format!("Child detail repeats {key}"));
            }
            match key.to_ascii_lowercase().as_str() {
                "classname" => {}
                "origin" | "angles" if vector(entity, key)? == DVec3::ZERO => {}
                "mindxlevel" | "maxdxlevel" if value.trim().parse::<u32>().ok() == Some(0) => {}
                _ => return Err(format!("Child detail property {key} is unsupported")),
            }
        }
    }
    Ok(())
}

type Provenance = BTreeMap<String, (u32, String, u32, Option<u32>)>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct StaticPropPose {
    pub origin: DVec3,
    pub angles: DVec3,
}

pub(crate) type StaticPropPoses = BTreeMap<u32, Vec<StaticPropPose>>;

pub(crate) type StaticPropBindings = BTreeMap<u32, crate::lighting_origins::Binding>;

#[derive(Default)]
pub(crate) struct StaticPropMetadata<'a> {
    pub poses: Option<&'a StaticPropPoses>,
    pub lighting_origins: Option<&'a StaticPropBindings>,
}

pub const BRUSH_MODEL_PREPARATION_DEFERRED: &str = "Child brush-model effective pivots, initial poses and local bounds require material-aware Compile; reference preparation is deferred";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushModelPreparation {
    Absent,
    Deferred,
    Resolved,
}

#[derive(Clone, Copy)]
enum PreparationPurpose<'a> {
    Reference,
    Compile(&'a source_formats::gamefs::GameFs),
}

pub struct PreparedInstances<'a> {
    pub(crate) vmf: Cow<'a, Vmf>,
    pub(crate) dependencies: Option<DependencyGuard>,
    pub(crate) stats: Option<InstanceStats>,
    pub(crate) static_prop_poses: StaticPropPoses,
    static_prop_bindings: StaticPropBindings,
    provenance: Provenance,
    pub(crate) material_sources: Option<crate::asset_sources::BuildMaterialSources>,
}

pub(crate) type Prepared<'a> = PreparedInstances<'a>;

pub struct InstanceBrush<'a> {
    pub owner: u32,
    pub file: &'a str,
    pub original: u32,
    pub detail: bool,
    pub solid: &'a Solid,
}

impl PreparedInstances<'_> {
    pub(crate) fn static_prop_metadata(&self) -> StaticPropMetadata<'_> {
        StaticPropMetadata {
            poses: Some(&self.static_prop_poses),
            lighting_origins: Some(&self.static_prop_bindings),
        }
    }
    /// Reference geometry and provenance; deferred model pivots are not compile-ready.
    /// Use `compilation_vmf` when resolved model geometry is required.
    pub fn vmf(&self) -> &Vmf {
        self.vmf.as_ref()
    }

    pub fn brush_model_preparation(&self) -> BrushModelPreparation {
        if !self.has_brush_models() {
            BrushModelPreparation::Absent
        } else if self.material_sources.is_some() {
            BrushModelPreparation::Resolved
        } else {
            BrushModelPreparation::Deferred
        }
    }

    pub fn compilation_vmf(&self) -> Result<&Vmf, &'static str> {
        if self.brush_model_preparation() == BrushModelPreparation::Deferred {
            return Err(BRUSH_MODEL_PREPARATION_DEFERRED);
        }
        Ok(self.vmf.as_ref())
    }

    pub fn dependencies(&self) -> Option<&DependencyGuard> {
        self.dependencies.as_ref()
    }

    pub fn stats(&self) -> Option<&InstanceStats> {
        self.stats.as_ref()
    }

    pub fn root_entity(&self, generated: u32) -> Option<u32> {
        self.provenance
            .get(&format!("entity:{generated}"))
            .map(|(owner, _, _, _)| *owner)
    }

    pub(crate) fn entity_origin(&self, generated: u32) -> Option<(u32, &str, u32)> {
        let (owner, file, original, _) = self.provenance.get(&format!("entity:{generated}"))?;
        Some((*owner, file, *original))
    }

    pub(crate) fn has_brush_models(&self) -> bool {
        self.stats
            .as_ref()
            .is_some_and(|stats| stats.brush_models > 0)
    }

    pub(crate) fn validate_brush_model_materials(
        &self,
        fs: &source_formats::gamefs::GameFs,
        sources: &crate::asset_sources::BuildMaterialSources,
        control: &Control,
    ) -> Result<(), Diagnostic> {
        let entities = self.vmf.entities.iter().filter(|entity| {
            brush_models::is_model(entity) && self.root_entity(entity.id).is_some()
        });
        brush_models::validate_materials(entities, fs, sources, control)
    }

    pub fn generated_brushes(&self) -> impl Iterator<Item = InstanceBrush<'_>> {
        self.vmf
            .world
            .solids
            .iter()
            .map(|solid| (solid, false))
            .chain(
                self.vmf
                    .entities
                    .iter()
                    .filter(|entity| {
                        !visclusters::is_helper(entity)
                            && !occluders::is_helper(entity)
                            && !brush_models::is_model(entity)
                    })
                    .flat_map(|entity| entity.solids.iter().map(|solid| (solid, true))),
            )
            .filter_map(move |(solid, detail)| {
                let (owner, file, original, _) =
                    self.provenance.get(&format!("solid:{}", solid.id))?;
                Some(InstanceBrush {
                    owner: *owner,
                    file,
                    original: *original,
                    detail,
                    solid,
                })
            })
    }

    pub fn face_origin(&self, side: u32) -> Option<(u32, &str, u32, u32)> {
        let (owner, file, original, solid) = self.provenance.get(&format!("side:{side}"))?;
        Some((*owner, file, (*solid)?, *original))
    }

    pub fn annotate(&self, report: &mut Report) {
        for diagnostic in &mut report.diagnostics {
            if let Some((owner, path, original, _)) = diagnostic
                .object
                .as_ref()
                .and_then(|object| self.provenance.get(object))
            {
                diagnostic.message = format!(
                    "{} (instance {path}, child object {original})",
                    diagnostic.message
                );
                diagnostic.object = Some(format!("entity:{owner}"));
            }
        }
    }
}

/// Resolve material-free reference data. Brush-model pivot/local checks remain deferred.
pub fn resolve<'a>(
    vmf: &'a Vmf,
    source: Option<&SourceContext>,
    control: &Control,
    report: &mut Report,
) -> Result<PreparedInstances<'a>, Diagnostic> {
    resolve_purpose(vmf, source, control, report, PreparationPurpose::Reference)
}

/// Resolve material-aware expanded geometry for inspection.
/// Compile the original VMF with `compile_with_source_controlled` to retain publication custody.
pub fn resolve_for_compile<'a>(
    vmf: &'a Vmf,
    source: Option<&SourceContext>,
    fs: &source_formats::gamefs::GameFs,
    control: &Control,
    report: &mut Report,
) -> Result<PreparedInstances<'a>, Diagnostic> {
    resolve_purpose(
        vmf,
        source,
        control,
        report,
        PreparationPurpose::Compile(fs),
    )
}

fn resolve_purpose<'a>(
    vmf: &'a Vmf,
    source: Option<&SourceContext>,
    control: &Control,
    report: &mut Report,
    purpose: PreparationPurpose<'_>,
) -> Result<PreparedInstances<'a>, Diagnostic> {
    if let Some(diagnostic) = vmf
        .entities
        .iter()
        .filter(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance_parms"))
        })
        .find_map(crate::map::unresolved_instance_diagnostic)
    {
        return Err(diagnostic);
    }
    if requires_source(vmf) && source.is_none() {
        return Err(Diagnostic::error(
            "Save the parent VMF before resolving instances",
            None,
            None,
        ));
    }
    prepare_purpose(vmf, source, control, report, purpose)
}

#[cfg(test)]
pub(crate) fn prepare<'a>(
    vmf: &'a Vmf,
    source: Option<&SourceContext>,
    control: &Control,
    report: &mut Report,
) -> Result<Prepared<'a>, Diagnostic> {
    prepare_purpose(vmf, source, control, report, PreparationPurpose::Reference)
}

pub(crate) fn prepare_for_compile<'a>(
    vmf: &'a Vmf,
    source: Option<&SourceContext>,
    fs: &source_formats::gamefs::GameFs,
    control: &Control,
    report: &mut Report,
) -> Result<Prepared<'a>, Diagnostic> {
    prepare_purpose(
        vmf,
        source,
        control,
        report,
        PreparationPurpose::Compile(fs),
    )
}

fn prepare_purpose<'a>(
    vmf: &'a Vmf,
    source: Option<&SourceContext>,
    control: &Control,
    report: &mut Report,
    purpose: PreparationPurpose<'_>,
) -> Result<Prepared<'a>, Diagnostic> {
    let mut prepared = Prepared {
        vmf: Cow::Borrowed(vmf),
        dependencies: None,
        stats: None,
        static_prop_poses: BTreeMap::new(),
        static_prop_bindings: BTreeMap::new(),
        provenance: BTreeMap::new(),
        material_sources: None,
    };
    if !requires_source(vmf) {
        return Ok(prepared);
    }
    let Some(source) = source else {
        return Ok(prepared);
    };
    let helpers = vmf
        .entities
        .iter()
        .filter(|entity| {
            !entity.hidden
                && entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        })
        .take(MAX_INSTANCES + 1)
        .collect::<Vec<_>>();
    let failure = |entity: &Entity, message: String| {
        Diagnostic::error(
            map_core::entity_helpers::bounded_preview(&message, MAX_FIELD_BYTES),
            Some(format!("entity:{}", entity.id)),
            entity.origin().filter(|point| point.is_finite()),
        )
    };
    let first = helpers[0];
    if let Some(diagnostic) = vmf
        .entities
        .iter()
        .filter(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance_parms"))
        })
        .find_map(crate::map::unresolved_instance_diagnostic)
    {
        return Err(diagnostic);
    }
    let check = |result: Result<(), String>| result.map_err(|error| failure(first, error));
    check(vmf.validate_ids().map_err(|error| error.to_string()))?;
    if helpers.len() > MAX_INSTANCES
        || vmf.object_ids().take(MAX_OBJECTS + 1).count() > MAX_OBJECTS
        || vmf.side_ids().take(MAX_SIDES + 1).count() > MAX_SIDES
    {
        return Err(failure(
            first,
            "Instance expansion exceeds object, side or occurrence limits".into(),
        ));
    }
    source
        .verify(control)
        .map_err(|error| failure(first, error.to_string()))?;
    for helper in &helpers {
        helper_admission(helper).map_err(|error| failure(helper, error))?;
    }
    let mut document =
        Document::try_new(vmf.clone()).map_err(|error| failure(first, error.to_string()))?;
    let mut expansion = expand::Expansion::new(vmf, source, control, report, purpose);
    for helper in &helpers {
        document
            .edit("expand brush instance", |tx| {
                expansion.helper(
                    tx,
                    helper,
                    helper,
                    &mut Vec::with_capacity(MAX_EXPANSION_DEPTH),
                    &mut vec![source.root.clone()],
                )
            })
            .map_err(|error| failure(helper, error.to_string()))?;
    }
    document
        .edit("consume instance helpers", |tx| {
            for helper in &helpers {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .map_err(|error| failure(first, error.to_string()))?;
    let expanded = document.into_vmf();
    check(expanded.validate_ids().map_err(|error| error.to_string()))?;
    let expanded_sources = expansion.finish();
    let stats = expanded_sources.stats;
    prepared.provenance = expanded_sources.provenance;
    prepared.static_prop_poses = expanded_sources.static_prop_poses;
    prepared.static_prop_bindings = expanded_sources.static_prop_bindings;
    prepared.material_sources = expanded_sources.material_sources;
    report.diagnostics.push(Diagnostic::info(
        format!(
            "Expanded {} brush instances from {} files: {} world brushes, {} detail brushes",
            stats.instances, stats.files, stats.world_brushes, stats.detail_brushes
        ),
        None,
        None,
    ));
    prepared.dependencies = Some(DependencyGuard {
        source: source.clone(),
        files: expanded_sources.files,
    });
    prepared.stats = Some(stats);
    prepared.vmf = Cow::Owned(expanded);
    if prepared.brush_model_preparation() == BrushModelPreparation::Deferred {
        report.diagnostics.push(Diagnostic::info(
            BRUSH_MODEL_PREPARATION_DEFERRED,
            None,
            None,
        ));
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests;
