//! Compiles VMF brush maps into Source engine BSP files.

mod areas;
pub mod asset_sources;
mod brush_model_pose;
pub mod control;
mod cordons;
pub mod cubemap_assets;
mod detail_props;
mod emit;
pub mod face_origins;
pub mod faces;
mod hints;
pub mod instances;
pub mod lighting;
mod lighting_origins;
pub mod map;
pub mod materials;
mod overlays;
pub mod physics;
pub mod planes;
pub mod portals;
pub mod prop_lighting_assets;
mod publication;
pub mod sky_assets;
pub mod sky_region;
pub mod static_prop_assets;
mod static_props;
pub mod tree;
mod visclusters;
mod visibility;
mod water;
mod water_analog;
pub mod water_assets;
pub mod water_dependencies;
pub mod winding;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;

#[cfg(test)]
#[path = "../tests/common/static_prop_model.rs"]
mod test_static_prop_model;

#[cfg(all(test, target_os = "linux"))]
mod write_reservation_tests;

use std::io::Write;
use std::time::{Duration, Instant};
use std::{io, path::Path};

use glam::DVec3;
use source_formats::bsp::{self, Bsp};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

pub use control::{Control, Stage};
pub use cordons::RegionSummary;
pub use cubemap_assets::{CompileContext, CubemapAssetManifest, CubemapAssetPhase};
pub use cubemap_assets::{
    finalize_reflections, finalize_reflections_with_capture, inspect_cubemap_bindings,
    read_generated_cubemaps, rebase_generated_cubemaps, verify_generated_cubemaps,
};
pub use detail_props::{
    GENERATED_DETAIL_LIGHTING_TINT_KEY, validate_generated_detail_lighting_tint,
};
pub use physics::current::{
    MAX_WATER_CURRENT_SPEED, MAX_WATER_CURRENT_VALUE_BYTES, WATER_CURRENT_COMPONENTS,
    WATER_CURRENT_VELOCITY_KEY, validate_water_current_velocity,
};
pub use physics::damping::{
    DEFAULT_WATER_DAMPING, MAX_WATER_DAMPING, MAX_WATER_DAMPING_VALUE_BYTES, WATER_DAMPING_KEY,
    validate_water_damping,
};
pub use portals::Leak;
pub use publication::{FileIdentity, FileStamp, OutputLease};
pub use sky_assets::{SkyAssetManifest, SkyAssetPlan, read_packed_sky, verify_packed_sky};
pub use water_analog::{WATER_ANALOG_CLASSNAME, WATER_ANALOG_DEFAULTS};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingMode {
    #[default]
    Fullbright,
    DirectLdr,
    DirectLdrHdr,
}

impl LightingMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fullbright => "fullbright",
            Self::DirectLdr => "direct_ldr",
            Self::DirectLdrHdr => "direct_ldr_hdr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fullbright => "Fullbright",
            Self::DirectLdr => "Direct LDR",
            Self::DirectLdrHdr => "Direct LDR + HDR",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Fullbright => "No lightmaps; useful for fast geometry checks.",
            Self::DirectLdr => {
                "Bakes static, switchable and animated point/spot lights, sun and sky on world brushes, brush entities and terrain, with directional normal-map lightmaps, shadows, scoped world/model brush smoothing and model ambient. Up to four style-preserving diffuse bounces reach world, terrain and brush entities. Prop lightmaps remain unfinished."
            }
            Self::DirectLdrHdr => {
                "Bakes separate LDR and HDR lightmaps, world lights and model ambient, selecting each channel's authored light inputs. Sampling, diffuse bounces and brush smoothing apply to both channels within one shared work limit. Empty HDR lighting, world-light or ambient data cannot activate native HDR. Runtime HDR depends on the game and hardware; Generate reflections captures both LDR and HDR cubemaps when enabled. HDR viewport, exposure authoring and prop lightmaps remain unfinished."
            }
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "fullbright" => Ok(Self::Fullbright),
            "direct_ldr" => Ok(Self::DirectLdr),
            "direct_ldr_hdr" => Ok(Self::DirectLdrHdr),
            _ => Err("Lighting must be fullbright, direct_ldr or direct_ldr_hdr".into()),
        }
    }

    pub const fn performs_lighting(self) -> bool {
        matches!(self, Self::DirectLdr | Self::DirectLdrHdr)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SkyQuality {
    #[default]
    Fast,
    Balanced,
    Detailed,
}

impl SkyQuality {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Detailed => "detailed",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast (32 directions)",
            Self::Balanced => "Balanced (128 directions)",
            Self::Detailed => "Detailed (512 directions)",
        }
    }
    pub fn directions(self) -> usize {
        match self {
            Self::Fast => 32,
            Self::Balanced => 128,
            Self::Detailed => 512,
        }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "balanced" => Ok(Self::Balanced),
            "detailed" => Ok(Self::Detailed),
            _ => Err("Sampling quality must be fast, balanced or detailed".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub lighting: LightingMode,
    pub physics: PhysicsMode,
    pub sky_quality: SkyQuality,
    pub lighting_work_limit: u64,
    pub visibility: VisibilityMode,
    pub bounces: u8,
    pub smoothing_angle: u8,
    pub generate_reflections: bool,
    pub pack_sky_assets: bool,
    pub pack_water_assets: bool,
    pub pack_static_prop_assets: bool,
    pub bake_static_prop_vertices: bool,
}

pub const MIN_LIGHTING_WORK_LIMIT: u64 = 1;
pub const DEFAULT_LIGHTING_WORK_LIMIT: u64 = 1_000_000_000;
pub const MAX_LIGHTING_WORK_LIMIT: u64 = 10_000_000_000;

pub fn validate_lighting_work_limit(value: u64) -> Result<u64, String> {
    if !(MIN_LIGHTING_WORK_LIMIT..=MAX_LIGHTING_WORK_LIMIT).contains(&value) {
        return Err(format!(
            "Lighting work limit must be between {MIN_LIGHTING_WORK_LIMIT} and {MAX_LIGHTING_WORK_LIMIT} work units"
        ));
    }
    Ok(value)
}

impl Default for Options {
    fn default() -> Self {
        Self {
            lighting: LightingMode::default(),
            physics: PhysicsMode::default(),
            sky_quality: SkyQuality::default(),
            lighting_work_limit: DEFAULT_LIGHTING_WORK_LIMIT,
            visibility: VisibilityMode::default(),
            bounces: 0,
            smoothing_angle: 0,
            generate_reflections: false,
            pack_sky_assets: false,
            pack_water_assets: false,
            pack_static_prop_assets: false,
            bake_static_prop_vertices: false,
        }
    }
}

pub const MAX_LIGHT_BOUNCES: u8 = 4;
pub const BOUNCE_LABELS: [&str; MAX_LIGHT_BOUNCES as usize + 1] = [
    "Direct only",
    "One diffuse bounce",
    "Two diffuse bounces",
    "Three diffuse bounces",
    "Four diffuse bounces",
];

pub fn bounce_label(count: u8) -> &'static str {
    BOUNCE_LABELS
        .get(usize::from(count))
        .copied()
        .unwrap_or("Unsupported bounce count")
}

impl Options {
    pub fn requires_guarded_publication_for(self, vmf: &Vmf) -> bool {
        self.requires_guarded_publication()
            || instances::requires_source(vmf)
            || vmf.entities.iter().any(|entity| {
                !entity.hidden
                    && entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("prop_static"))
            })
    }

    pub const fn requires_guarded_publication(self) -> bool {
        self.generate_reflections
            || self.pack_sky_assets
            || self.pack_water_assets
            || self.pack_static_prop_assets
            || self.bake_static_prop_vertices
    }

    pub fn validate(self) -> Result<Self, String> {
        validate_lighting_work_limit(self.lighting_work_limit)?;
        if self.bake_static_prop_vertices && !self.lighting.performs_lighting() {
            return Err(
                "Bake static-prop vertex lighting requires Direct LDR or Direct LDR + HDR lighting"
                    .into(),
            );
        }
        if self.generate_reflections && !self.lighting.performs_lighting() {
            return Err(
                "Generate reflections requires Direct LDR or Direct LDR + HDR lighting".into(),
            );
        }
        if self.bounces > MAX_LIGHT_BOUNCES {
            return Err(format!(
                "Light bounces must be between 0 and {MAX_LIGHT_BOUNCES}"
            ));
        }
        if self.smoothing_angle > 90 {
            return Err("Brush smoothing angle must be a whole number from 0 to 90 degrees".into());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityMode {
    #[default]
    Fast,
    Detailed,
}

impl VisibilityMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Detailed => "detailed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast (conservative)",
            Self::Detailed => "Detailed (portal clipping)",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Fast => {
                "Quickly builds conservative visible and audible sets. Some hidden areas remain visible."
            }
            Self::Detailed => {
                "Clips sight volumes through portal paths to remove hidden areas. Uses up to eight CPU workers on larger maps. Invalid apertures and exceeded work/allocation budgets stop the build. Broader map coverage and overlapping occluder geometry remain under development."
            }
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "detailed" => Ok(Self::Detailed),
            _ => Err("Visibility must be fast or detailed".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicsMode {
    #[default]
    None,
    Engine,
}

impl PhysicsMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Engine => "engine",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "No physics bodies",
            Self::Engine => "CS:S engine physics",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::None => {
                "Keeps BSP brush hulls and static-prop asset collisions; omits compiled brush/fluid bodies."
            }
            Self::Engine => {
                "Adds world, clip, brush-model, water and terrain collision with world, terrain and brush-model material surfaces. Static props use model assets. Blended surfaces, density and compiler-derived masses remain unfinished."
            }
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "engine" => Ok(Self::Engine),
            _ => Err("Physics must be none or engine".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// A compile message tied to a map object (`solid:<id>` / `entity:<id>`)
/// and position where possible, so the editor and agents can act on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
    pub object: Option<String>,
    pub position: Option<DVec3>,
}

impl Diagnostic {
    pub fn info(
        message: impl Into<String>,
        object: Option<String>,
        position: Option<DVec3>,
    ) -> Self {
        Self {
            level: Level::Info,
            message: message.into(),
            object,
            position,
        }
    }

    pub fn warning(
        message: impl Into<String>,
        object: Option<String>,
        position: Option<DVec3>,
    ) -> Self {
        Self {
            level: Level::Warning,
            ..Self::info(message, object, position)
        }
    }

    pub fn error(
        message: impl Into<String>,
        object: Option<String>,
        position: Option<DVec3>,
    ) -> Self {
        Self {
            level: Level::Error,
            ..Self::info(message, object, position)
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.level == Level::Error)
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct HdrLightingStats {
    pub world_lights: usize,
    pub light_styles: usize,
    pub styled_faces: usize,
    pub light_style_maps: usize,
    pub lighting_faces: usize,
    pub lit_brush_models: usize,
    pub model_lighting_faces: usize,
    pub lightmap_samples: usize,
    pub lighting_bytes: usize,
    pub lighting_work: usize,
    pub ambient_samples: usize,
    pub ambient_bytes: usize,
    pub bounces: u8,
    pub bounce_rays: usize,
    pub bounce_samples: usize,
    pub bounce_work: usize,
    pub model_bounce_faces: usize,
    pub model_bounce_samples: usize,
    pub bump_faces: usize,
    pub bump_lightmaps: usize,
    pub map_data_present: bool,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Stats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instances: Option<instances::InstanceStats>,
    pub compile_region: Option<RegionSummary>,
    pub lighting_mode: LightingMode,
    pub hdr: Option<HdrLightingStats>,
    pub lighting_total_work: usize,
    pub lighting_progress: Option<lighting::LightingProgress>,
    pub bounces: u8,
    pub bounce_rays: usize,
    pub bounce_samples: usize,
    pub bounce_work: usize,
    pub model_bounce_faces: usize,
    pub model_bounce_samples: usize,
    pub bump_faces: usize,
    pub bump_lightmaps: usize,
    pub smoothed_faces: usize,
    pub smoothed_corners: usize,
    pub normal_work: usize,
    pub smoothing_angle: u8,
    pub physics_mode: PhysicsMode,
    pub visibility_mode: VisibilityMode,
    pub brushes: usize,
    pub nodes: usize,
    pub leaves: usize,
    pub faces: usize,
    pub models: usize,
    pub cubemap_samples: usize,
    pub occluders: usize,
    pub occluder_polygons: usize,
    pub occluder_vertex_refs: usize,
    pub static_props: usize,
    pub static_prop_models: usize,
    pub static_prop_leaf_refs: usize,
    pub detail_props: usize,
    pub detail_prop_models: usize,
    pub detail_prop_sprites: usize,
    pub prop_shadow_props: usize,
    pub prop_shadow_models: usize,
    pub prop_shadow_triangles: usize,
    pub prop_shadow_disabled: usize,
    pub prop_shadow_work: usize,
    pub prop_shadow_alpha_triangles: usize,
    pub prop_shadow_alpha_materials: usize,
    pub prop_shadow_alpha_textures: usize,
    pub prop_shadow_alpha_bytes: usize,
    pub displacements: usize,
    pub displacement_vertices: usize,
    pub displacement_triangles: usize,
    pub displacement_neighbor_pairs: usize,
    pub displacement_corner_neighbor_pairs: usize,
    pub displacement_mixed_power_pairs: usize,
    pub displacement_half_edge_pairs: usize,
    pub displacement_smoothing_overrides: usize,
    pub displacement_lod_disabled_vertices: usize,
    pub displacement_lod_snapped_vertices: usize,
    pub clusters: usize,
    pub visibility_solved_clusters: usize,
    pub viscluster_helpers: usize,
    pub viscluster_groups: usize,
    pub viscluster_grouped_leaves: usize,
    pub visibility_portals: usize,
    pub visibility_bytes: usize,
    pub visibility_work: usize,
    pub visibility_removed_pairs: usize,
    pub visibility_workers: usize,
    pub hint_surfaces: usize,
    pub hint_splits: usize,
    pub hint_priority_splits: usize,
    pub hint_work: usize,
    pub areas: usize,
    pub areaportals: usize,
    pub areaportal_windows: usize,
    pub areaportal_vertices: usize,
    pub overlays: usize,
    pub overlay_face_refs: usize,
    pub water_regions: usize,
    pub water_leaves: usize,
    pub sky_clusters: usize,
    pub world_lights: usize,
    pub switchable_light_groups: usize,
    pub light_styles: usize,
    pub styled_faces: usize,
    pub light_style_maps: usize,
    pub lighting_faces: usize,
    pub lit_brush_models: usize,
    pub model_lighting_faces: usize,
    pub lightmap_samples: usize,
    pub lighting_bytes: usize,
    pub lighting_work: usize,
    pub light_evaluations: usize,
    pub light_pvs_skipped: usize,
    pub light_range_skipped: usize,
    pub light_culling_work: usize,
    pub ambient_samples: usize,
    pub ambient_bytes: usize,
    pub physics_models: usize,
    pub physics_solids: usize,
    pub physics_convexes: usize,
    pub physics_bytes: usize,
    pub physics_fluids: usize,
    pub physics_mesh_triangles: usize,
    pub physics_surface_properties: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reflections: Option<ReflectionStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sky_assets: Option<PackedSkyStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub water_assets: Option<PackedWaterStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_prop_assets: Option<PackedStaticPropStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prop_vertex_lighting: Option<PropVertexLightingStats>,
    #[serde(rename = "elapsed_ms", serialize_with = "serialize_elapsed")]
    pub elapsed: Duration,
}

#[derive(Debug, serde::Serialize)]
pub struct ReflectionStats {
    pub channel: &'static str,
    pub verified_samples: usize,
    pub material_aliases: usize,
    pub packed_bytes: usize,
    pub capture_ms: u64,
    pub artifacts: std::path::PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capture_artifacts: Vec<std::path::PathBuf>,
}

#[derive(Debug, serde::Serialize)]
pub struct PackedSkyStats {
    pub family: String,
    pub channels: &'static str,
    pub files: usize,
    pub asset_bytes: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct PackedWaterStats {
    pub roots: usize,
    pub channels: &'static str,
    pub files: usize,
    pub asset_bytes: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct PackedStaticPropStats {
    pub models: usize,
    pub files: usize,
    pub asset_bytes: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PropVertexLightingStats {
    pub props: usize,
    pub models: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub ldr_bytes: usize,
    pub hdr_bytes: usize,
    pub work: usize,
    pub asset_bytes: usize,
}

fn serialize_elapsed<S: serde::Serializer>(
    elapsed: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

#[derive(Debug)]
pub struct Output {
    pub face_origins: Result<std::sync::Arc<face_origins::FaceOrigins>, String>,
    pub instance_dependencies: Option<instances::DependencyGuard>,
    pub bsp: Option<Bsp>,
    pub report: Report,
    pub leak: Option<Leak>,
    pub stats: Stats,
    pub cubemap_assets: Option<CubemapAssetManifest>,
    pub sky_assets: Option<SkyAssetPlan>,
    pub water_assets: Option<water_assets::WaterAssetPlan>,
    pub static_prop_assets: Option<static_prop_assets::StaticPropAssetPlan>,
    pub material_sources: Option<asset_sources::AssetSourceGuard>,
    pub static_prop_sources: Option<asset_sources::AssetSourceGuard>,
    pub prop_lighting_assets: Option<prop_lighting_assets::PropLightingPlan>,
}

#[derive(Debug)]
pub struct PreparedWrite {
    staged: source_formats::file_publication::StagedFile,
    destination: std::path::PathBuf,
    success: bool,
    instance_dependencies: Option<instances::DependencyGuard>,
    instance_output_stamp: Option<FileStamp>,
    prop_lighting_sources: Option<asset_sources::AssetSourceGuard>,
    material_sources: Option<asset_sources::AssetSourceGuard>,
    static_prop_sources: Option<asset_sources::AssetSourceGuard>,
}

#[derive(Debug)]
pub struct WriteReservation {
    staged: source_formats::file_publication::StagedFile,
    destination: source_formats::file_publication::BoundDestination,
    pointfile: source_formats::file_publication::BoundDestination,
    requested: std::path::PathBuf,
}

#[derive(Debug)]
pub struct PublicationReceipt {
    pub success: bool,
    pub publication: source_formats::file_publication::CommittedReceipt,
}

impl WriteReservation {
    pub fn new(path: &Path, control: &Control) -> io::Result<Self> {
        use source_formats::file_publication::{BoundDestination, Policy};
        control.checkpoint().map_err(io::Error::other)?;
        Self::for_destinations(
            BoundDestination::capture(path, Policy::ReplaceEntry)?,
            BoundDestination::capture(&path.with_extension("lin"), Policy::ReplaceEntry)?,
            control,
        )
    }

    pub fn for_destinations(
        destination: source_formats::file_publication::BoundDestination,
        pointfile: source_formats::file_publication::BoundDestination,
        control: &Control,
    ) -> io::Result<Self> {
        control.checkpoint().map_err(io::Error::other)?;
        destination.verify()?;
        pointfile.verify()?;
        if pointfile.authored_path() != destination.authored_path().with_extension("lin")
            || pointfile.physical_path() != destination.physical_path().with_extension("lin")
        {
            return Err(io::Error::other(
                "Pointfile destination must be derived from the admitted BSP destination",
            ));
        }
        let requested = destination.authored_path().to_owned();
        let staged = destination.stage()?;
        Ok(Self {
            staged,
            destination,
            pointfile,
            requested,
        })
    }
}

impl PreparedWrite {
    pub fn commit(self, control: &Control) -> io::Result<bool> {
        self.commit_receipt(control).map(|receipt| receipt.success)
    }

    pub fn commit_receipt(self, control: &Control) -> io::Result<PublicationReceipt> {
        if self.instance_dependencies.is_some() {
            let lease = OutputLease::acquire(&self.destination, control)?;
            return self.commit_checked_with_receipt(&lease, control, || Ok(()));
        }
        control.publish(|| {
            if let Some(sources) = &self.prop_lighting_sources {
                sources.verify_metadata()?;
            }
            if let Some(sources) = &self.material_sources {
                sources.verify_metadata()?;
            }
            if let Some(sources) = &self.static_prop_sources {
                sources.verify_metadata()?;
            }
            self.staged.commit().map(|publication| PublicationReceipt {
                success: self.success,
                publication,
            })
        })
    }

    pub fn commit_checked(self, lease: &OutputLease, control: &Control) -> io::Result<bool> {
        self.commit_checked_with(lease, control, || Ok(()))
    }

    pub fn commit_checked_with(
        self,
        lease: &OutputLease,
        control: &Control,
        validate: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<bool> {
        self.commit_checked_with_receipt(lease, control, validate)
            .map(|receipt| receipt.success)
    }

    pub fn commit_checked_with_receipt(
        self,
        lease: &OutputLease,
        control: &Control,
        validate: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<PublicationReceipt> {
        if let Some(dependencies) = &self.instance_dependencies {
            dependencies.verify(control)?;
        }
        control.publish(|| {
            if let Some(dependencies) = &self.instance_dependencies {
                dependencies.verify_metadata()?;
            }
            if let Some(stamp) = &self.instance_output_stamp {
                stamp.verify_lease(lease)?;
            }
            validate()?;
            if let Some(sources) = &self.prop_lighting_sources {
                sources.verify_metadata()?;
            }
            if let Some(sources) = &self.material_sources {
                sources.verify_metadata()?;
            }
            if let Some(sources) = &self.static_prop_sources {
                sources.verify_metadata()?;
            }
            lease.verify_commit(&self.destination)?;
            self.staged.commit().map(|publication| PublicationReceipt {
                success: self.success,
                publication,
            })
        })
    }
}

impl Output {
    pub(crate) fn static_prop_source_guard(
        &self,
    ) -> io::Result<Option<&asset_sources::AssetSourceGuard>> {
        if self.stats.static_props > 0 && self.static_prop_sources.is_none() {
            return Err(io::Error::other(
                "Static-prop output is missing its original model/physics source guard",
            ));
        }
        Ok(self.static_prop_sources.as_ref())
    }

    pub fn write(&self, path: &Path) -> io::Result<bool> {
        self.write_controlled(path, &Control::default())
    }

    pub fn write_controlled(&self, path: &Path, control: &Control) -> io::Result<bool> {
        match self.prepare_write_controlled(path, control)? {
            Some(prepared) => prepared.commit(control),
            None => Ok(false),
        }
    }

    pub fn prepare_write_controlled(
        &self,
        path: &Path,
        control: &Control,
    ) -> io::Result<Option<PreparedWrite>> {
        control.checkpoint().map_err(io::Error::other)?;
        if self.bsp.is_none()
            && self.leak.is_none()
            && self.cubemap_assets.is_none()
            && self.sky_assets.is_none()
            && self.water_assets.is_none()
            && self.static_prop_assets.is_none()
            && self.prop_lighting_assets.is_none()
        {
            return Ok(None);
        }
        self.prepare_write_reserved(WriteReservation::new(path, control)?, control)
    }

    pub fn prepare_write_reserved(
        &self,
        reservation: WriteReservation,
        control: &Control,
    ) -> io::Result<Option<PreparedWrite>> {
        control.checkpoint().map_err(io::Error::other)?;
        let path = reservation.destination.physical_path();
        let instance_output_stamp = self
            .instance_dependencies
            .as_ref()
            .map(|_| {
                let (requested, destination) = if self.bsp.is_none() && self.leak.is_some() {
                    (
                        reservation.requested.with_extension("lin"),
                        path.with_extension("lin"),
                    )
                } else {
                    (reservation.requested.clone(), path.to_owned())
                };
                let stamp = FileStamp::capture(&requested)?;
                stamp.verify_path(&destination)?;
                Ok::<_, io::Error>(stamp)
            })
            .transpose()?;
        if let Some(dependencies) = &self.instance_dependencies {
            dependencies.verify(control)?;
        }
        if self
            .cubemap_assets
            .as_ref()
            .is_some_and(|assets| assets.phase != CubemapAssetPhase::Verified)
        {
            return Err(io::Error::other(
                "Reflection assets are pending native capture; no BSP can be published",
            ));
        }
        if let Some(assets) = &self.cubemap_assets {
            let bsp = self
                .bsp
                .as_ref()
                .ok_or_else(|| io::Error::other("Verified reflection output has no BSP"))?;
            if self.report.has_errors() {
                return Err(io::Error::other(
                    "Reflection build diagnostics contain errors",
                ));
            }
            cubemap_assets::verify_generated_cubemaps(bsp, assets, control)?;
            let destination = CompileContext::from_output(path).map_err(io::Error::other)?;
            if destination.namespace() != assets.namespace {
                return Err(io::Error::other(
                    "Reflection output basename differs from its verified asset namespace",
                ));
            }
        }
        if let Some(plan) = &self.sky_assets {
            let bsp = self
                .bsp
                .as_ref()
                .ok_or_else(|| io::Error::other("Packed sky output has no BSP"))?;
            if self.report.has_errors() {
                return Err(io::Error::other(
                    "Packed sky build diagnostics contain errors",
                ));
            }
            verify_packed_sky(bsp, &plan.manifest(), control)?;
        }
        if let Some(plan) = &self.water_assets {
            if self.material_sources.is_none() {
                return Err(io::Error::other(
                    "Packed Water output is missing its original build material guard",
                ));
            }
            let bsp = self
                .bsp
                .as_ref()
                .ok_or_else(|| io::Error::other("Packed Water output has no BSP"))?;
            if self.report.has_errors() {
                return Err(io::Error::other(
                    "Packed Water build diagnostics contain errors",
                ));
            }
            plan.verify_source_metadata()?;
            water_assets::verify_packed_water(bsp, &plan.manifest(), control)?;
        }
        if let Some(plan) = &self.static_prop_assets {
            if self.material_sources.is_none() {
                return Err(io::Error::other(
                    "Packed static-prop output is missing its original build asset guard",
                ));
            }
            let bsp = self
                .bsp
                .as_ref()
                .ok_or_else(|| io::Error::other("Packed static-prop output has no BSP"))?;
            if self.report.has_errors() {
                return Err(io::Error::other(
                    "Packed static-prop build diagnostics contain errors",
                ));
            }
            plan.verify_source_metadata()?;
            static_prop_assets::verify_packed_static_props(bsp, &plan.manifest(), control)?;
        }
        if let Some(sources) = &self.material_sources {
            sources.verify_metadata()?;
        }
        if let Some(sources) = self.static_prop_source_guard()? {
            sources.verify_metadata()?;
        }
        let prop_lighting_sources = if let Some(plan) = &self.prop_lighting_assets {
            let bsp = self
                .bsp
                .as_ref()
                .ok_or_else(|| io::Error::other("Static-prop vertex lighting output has no BSP"))?;
            if self.report.has_errors() {
                return Err(io::Error::other(
                    "Static-prop vertex lighting diagnostics contain errors",
                ));
            }
            plan.verify_source_metadata()?;
            prop_lighting_assets::verify_generated_prop_lighting(bsp, plan.manifest(), control)?;
            Some(plan.source_guard())
        } else {
            None
        };
        if self.bsp.is_none() && self.leak.is_none() {
            return Ok(None);
        }
        control.advance(Stage::Writing).map_err(io::Error::other)?;
        let (destination, bytes, success) = if let Some(bsp) = &self.bsp {
            (&reservation.destination, bsp.to_bytes(), true)
        } else if let Some(leak) = &self.leak {
            let text: String = leak
                .points
                .iter()
                .map(|&point| format!("{}\n", source_formats::vmf::format_vec3(point)))
                .collect();
            (&reservation.pointfile, text.into_bytes(), false)
        } else {
            return Ok(None);
        };
        let destination_path = destination.physical_path().to_owned();
        let mut staged = reservation.staged;
        staged.retarget_output(destination)?;
        for chunk in bytes.chunks(64 * 1024) {
            control.checkpoint().map_err(io::Error::other)?;
            staged.file_mut().write_all(chunk)?;
        }
        staged.sync_all()?;
        Ok(Some(PreparedWrite {
            staged,
            destination: destination_path,
            success,
            instance_dependencies: self.instance_dependencies.clone(),
            instance_output_stamp,
            prop_lighting_sources,
            material_sources: self.material_sources.clone(),
            static_prop_sources: self.static_prop_sources.clone(),
        }))
    }
}

fn build(
    prepared_instances: &instances::Prepared<'_>,
    fs: &GameFs,
    output: &mut Output,
    options: Options,
    context: Option<&CompileContext>,
    control: &Control,
    mut face_origins: Result<face_origins::FaceOrigins, String>,
) -> Result<(), String> {
    let vmf = prepared_instances.vmf.as_ref();
    options.validate()?;
    let water_current = physics::current::from_world(vmf, options.physics)?;
    let water_damping = physics::damping::from_world(vmf, options.physics)?;
    control.advance(Stage::Loading)?;
    for entity in &vmf.entities {
        control.checkpoint()?;
        if let Some(diagnostic) = map::unresolved_instance_diagnostic(entity) {
            output.report.diagnostics.push(diagnostic);
        }
    }
    if output.report.has_errors() {
        return Ok(());
    }
    if options.generate_reflections && context.is_none() {
        return Err("Generate reflections requires an explicit output CompileContext".into());
    }
    if options.generate_reflections {
        cubemap_assets::binding::capture_preflight(vmf)?;
    }
    let material_sources = (options.pack_water_assets || options.pack_static_prop_assets)
        .then(asset_sources::BuildMaterialSources::default);
    let prepared = cordons::prepare_with_material_sources(
        vmf,
        fs,
        &mut output.report,
        control,
        material_sources.as_ref(),
    )?;
    face_origins = face_origins.and_then(|mut origins| {
        origins.add_region(prepared.vmf.as_ref(), prepared.region.as_ref())?;
        Ok(origins)
    });
    output.stats.compile_region = prepared.region;
    let vmf = prepared.vmf.as_ref();
    let prop_sources = options
        .bake_static_prop_vertices
        .then_some(if options.bounces == 0 {
            materials::PropSourceMode::Direct
        } else {
            materials::PropSourceMode::WithReflectivity
        });
    let mut map = if let Some(sources) = &material_sources {
        map::Map::load_with_material_sources_controlled(
            vmf,
            fs,
            &mut output.report,
            control,
            prop_sources,
            sources,
        )?
    } else if let Some(mode) = prop_sources {
        map::Map::load_with_prop_sources_controlled(vmf, fs, mode, &mut output.report, control)?
    } else {
        map::Map::load_controlled(vmf, fs, &mut output.report, control)?
    };
    map.water_current = water_current;
    map.water_damping = water_damping;
    water_analog::admit_mode(&map, options.physics, &mut output.report);
    if output.report.has_errors() {
        return Ok(());
    }
    let water_plan = if options.pack_water_assets
        && let Some(sources) = &material_sources
        && !output.report.has_errors()
    {
        let roots = map
            .materials
            .iter()
            .filter(|info| info.water)
            .map(|info| {
                water_dependencies::logical_material_path(&info.name.to_ascii_lowercase())
                    .map_err(|error| format!("Pack water assets: material {}: {error}", info.name))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Some(
            water_assets::collect_water_asset_plan(
                roots.iter().map(String::as_str),
                fs,
                sources,
                &bsp::PakArchive::default(),
                bsp::PakLimits::default(),
                control,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let reflection_plan = if options.generate_reflections && !output.report.has_errors() {
        control.advance(Stage::PrepareReflections)?;
        output.report.diagnostics.retain(|item| {
            !item.message.starts_with("Cubemap sample records compiled;")
                && !item
                    .message
                    .starts_with("Cubemap face bindings require Generate reflections;")
        });
        Some(cubemap_assets::prepare(
            vmf,
            fs,
            &mut map,
            context.unwrap(),
            true,
            options.lighting == LightingMode::DirectLdrHdr,
            control,
        )?)
    } else {
        None
    };
    output.stats.brushes = map.brushes.len();
    output.stats.cubemap_samples = map.cubemaps.len();
    output.stats.displacements = map.terrains.len();
    output.stats.hint_surfaces = map.hints.len();
    output.stats.displacement_vertices = map
        .terrains
        .iter()
        .map(|terrain| terrain.vertices.len())
        .sum();
    output.stats.displacement_triangles = map
        .terrains
        .iter()
        .map(|terrain| terrain.triangles.len())
        .sum();
    output.stats.displacement_smoothing_overrides = map
        .terrains
        .iter()
        .filter(|terrain| terrain.smoothing_angle.is_some())
        .count();
    let mut no_preview_materials = 0;
    for info in map.materials.iter() {
        if matches!(
            &info.metrics,
            materials::MetricState::Resolved(materials::TextureMetrics {
                dimension_source: materials::DimensionSource::NativeNoPreview,
                ..
            })
        ) {
            no_preview_materials += 1;
        }
        if !info.found {
            output.report.diagnostics.push(Diagnostic::warning(
                format!("material {} was not found", info.name),
                None,
                None,
            ));
        }
        output.report.diagnostics.extend(
            info.warnings
                .iter()
                .map(|message| Diagnostic::warning(message, None, None)),
        );
    }
    if no_preview_materials != 0 {
        output.report.diagnostics.push(Diagnostic::info(
            format!(
                "SDK no-preview dimensions (128x128) selected for {no_preview_materials} materials by authored convention; diffuse reflectivity resolves independently"
            ),
            None,
            None,
        ));
    }
    if output.report.has_errors() {
        return Ok(());
    }
    if options.lighting.performs_lighting() {
        output.stats.switchable_light_groups = lighting::styles::prepare(&mut map, control)?;
    }
    physics::current::admit_map(&map, control)?;
    physics::damping::admit_map(&map, control)?;
    let neighbors = map.connect_terrain(&mut output.report, control)?;
    output.stats.displacement_neighbor_pairs = neighbors.pairs;
    output.stats.displacement_corner_neighbor_pairs = neighbors.corner_pairs;
    output.stats.displacement_mixed_power_pairs = neighbors.mixed_pairs;
    output.stats.displacement_half_edge_pairs = neighbors.half_pairs;
    output.stats.displacement_lod_disabled_vertices = neighbors.disabled_vertices;
    output.stats.displacement_lod_snapped_vertices = neighbors.snapped_vertices;
    let structural: Vec<usize> = map.entities[0]
        .brushes
        .iter()
        .copied()
        .filter(|&i| map.brushes[i].structural())
        .collect();
    control.advance(Stage::WorldTree)?;
    let (mut tree, hint_priority) = tree::Tree::build_with_hints(
        &map.brushes,
        &structural,
        &mut map.planes,
        &map.hints,
        control,
    )?;
    control.advance(Stage::Portals)?;
    let mut portals = portals::build_controlled(&tree, &map.planes, control)?;
    control.advance(Stage::Flood)?;
    let region_build = output.stats.compile_region.is_some();
    output.leak = portals::flood_controlled(
        &mut tree,
        &map.planes,
        &portals,
        map.entities
            .iter()
            .filter(|entity| !region_build || !cordons::is_global(&entity.classname)),
        &mut output.report,
        control,
    )?;
    if output.report.has_errors() {
        if let Some(sources) = &material_sources {
            output.material_sources = Some(
                sources
                    .source_guard(fs, control)
                    .map_err(|error| error.to_string())?,
            );
        }
        return Ok(());
    }
    control.advance(Stage::Hints)?;
    water_analog::admit_world(&map, &tree, &mut output.report, control)?;
    if output.report.has_errors() {
        return Ok(());
    }
    let hints = hints::apply(&map, &mut tree, &mut output.report, control, hint_priority)?;
    output.stats.hint_splits = hints.splits;
    output.stats.hint_priority_splits = hints.priority_splits;
    output.stats.hint_work = hints.work;
    if hints.splits > 0 || !map.occluders.is_empty() {
        control.advance(Stage::Portals)?;
        portals = portals::build_controlled(&tree, &map.planes, control)?;
    }
    control.advance(Stage::Areas)?;
    let areas = areas::build(&mut map, &tree, &portals, control)?;
    map::occluders::assign_areas(
        &mut map,
        &tree,
        &portals,
        &areas,
        &mut output.report,
        control,
    )?;
    if output.report.has_errors() {
        return Ok(());
    }
    output.stats.areas = areas.data.areas.len().saturating_sub(1);
    output.stats.areaportals = map.area_portals.len();
    output.stats.areaportal_windows = map
        .area_portals
        .iter()
        .filter(|portal| map.entities[portal.entity].classname == "func_areaportalwindow")
        .count();
    output.stats.areaportal_vertices = areas.data.clip_vertices.len();
    control.advance(Stage::Water)?;
    let mut water = water::build(&map, &tree, &portals, &mut output.report, control)?;
    output.stats.water_regions = water.records.len();
    output.stats.water_leaves = water.leaves.iter().filter(|&&region| region >= 0).count();
    if output.report.has_errors() {
        return Ok(());
    }
    if options.pack_water_assets {
        for record in &water.records {
            let texinfo = &map.texinfos.infos[record.surface_texinfo as usize];
            let material = map.materials.get(map.texdata[texinfo.texdata as usize]);
            if !material.water {
                return Err(format!(
                    "Pack water assets requires supported Water-shader liquid surfaces; material {} is outside that subset",
                    material.name
                ));
            }
        }
    } else if !water.records.is_empty() {
        output.report.diagnostics.push(Diagnostic::warning("Water material assets must remain mounted; enable Pack water assets to embed supported static Water files", None, None));
    }
    if !water.records.is_empty()
        && !map
            .entities
            .iter()
            .any(|entity| entity.classname == "water_lod_control")
    {
        map.entities.push(map::MapEntity {
            id: 0,
            classname: "water_lod_control".into(),
            properties: vec![
                ("classname".into(), "water_lod_control".into()),
                ("cheapwaterstartdistance".into(), "1000".into()),
                ("cheapwaterenddistance".into(), "2000".into()),
            ],
            brushes: Vec::new(),
            origin: None,
        });
    }
    control.advance(Stage::Visibility)?;
    let (mut visibility, visclusters) = visibility::build_authored(
        visclusters::Context {
            map: &map,
            tree: &tree,
            areas: &areas,
            water: &water,
            control,
        },
        &portals,
        &mut output.report,
        options.visibility,
    )?;
    if !water.records.is_empty() {
        water.fog_tests = visibility.water_fog_tests(&tree, &water.leaves, control)?;
    }
    output.stats.visibility_solved_clusters = visclusters.solved_clusters;
    output.stats.viscluster_helpers = visclusters.helpers;
    output.stats.viscluster_groups = visclusters.groups;
    output.stats.viscluster_grouped_leaves = visclusters.grouped_leaves;
    output.stats.clusters = visibility.count;
    output.stats.visibility_portals = visibility.portals;
    output.stats.visibility_bytes = visibility.data.len();
    output.stats.visibility_mode = options.visibility;
    output.stats.visibility_work = visibility.work;
    output.stats.visibility_removed_pairs = visibility.removed_pairs;
    output.stats.visibility_workers = visibility.workers;
    output.report.diagnostics.push(Diagnostic::info(
        match options.visibility {
            VisibilityMode::Fast => "Visibility uses conservative portal bounds; choose Detailed for portal separator clipping",
            VisibilityMode::Detailed => "Visibility uses bounded portal separator clipping and source-cluster tasks",
        },
        None,
        None,
    ));
    control.advance(Stage::Faces)?;
    let mut retained_points = 0;
    let retention = if !options.lighting.performs_lighting() {
        faces::SourceRetention::None
    } else if options.smoothing_angle == 0 {
        faces::SourceRetention::Assigned
    } else {
        faces::SourceRetention::All
    };
    let world_retention = if !options.lighting.performs_lighting() {
        faces::SourceRetention::Assigned
    } else {
        retention
    };
    let mut faces = faces::build_with_water_regions(
        &map,
        &tree,
        0,
        control,
        world_retention,
        &mut retained_points,
        &water.leaves,
    )?;
    faces.extend(map.terrain_faces(&tree, &mut output.report, control)?);
    if output.report.has_errors() {
        return Ok(());
    }
    let mut models = vec![emit::Model {
        entity: 0,
        tree,
        faces,
    }];
    control.advance(Stage::Models)?;
    for entity in 1..map.entities.len() {
        control.checkpoint()?;
        let indices = &map.entities[entity].brushes;
        if indices.is_empty() {
            continue;
        }
        let tree = tree::Tree::build_controlled(&map.brushes, indices, &mut map.planes, control)?;
        let faces = faces::build_with_smoothing(
            &map,
            &tree,
            entity,
            control,
            retention,
            &mut retained_points,
        )?;
        models.push(emit::Model {
            entity,
            tree,
            faces,
        });
    }
    water_analog::admit_models(&map, &models, &mut output.report);
    if output.report.has_errors() {
        return Ok(());
    }
    control.advance(Stage::SkyVisibility)?;
    visibility.mark_sky(&map, &models[0].faces, control)?;
    output.stats.sky_clusters = visibility.sky.iter().filter(|&&flags| flags != 0).count();
    let revision = vmf
        .version_info
        .as_ref()
        .and_then(|p| p.get("mapversion"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    control.advance(Stage::Emitting)?;
    let emission = emit::build(
        &map,
        &models,
        &visibility,
        &water,
        &areas,
        revision,
        control,
        face_origins,
    )?;
    let mut bsp = emission.bsp;
    if let Some(data) = bsp.occlusion().map_err(|error| error.to_string())? {
        output.stats.occluders = data.occluders.len();
        output.stats.occluder_polygons = data.polygons.len();
        output.stats.occluder_vertex_refs = data.vertex_indices.len();
    }
    if let Some(data) = bsp.overlay_data().map_err(|error| error.to_string())? {
        output.stats.overlays = data.overlays.len();
        output.stats.overlay_face_refs = data
            .overlays
            .iter()
            .map(|overlay| overlay.face_count())
            .sum();
    }
    if let Some(data) = bsp
        .water_overlay_data()
        .map_err(|error| error.to_string())?
    {
        output.stats.overlays += data.overlays.len();
        output.stats.overlay_face_refs += data
            .overlays
            .iter()
            .map(|overlay| overlay.face_count())
            .sum::<usize>();
    }
    control.advance(Stage::StaticProps)?;
    if options.pack_static_prop_assets
        && let Some(reads) = map.materials.prop_lighting_sources_mut()
        && let Some(sources) = &material_sources
    {
        reads.share_sources(sources.clone());
    }
    let prop_stats = static_props::bake(
        vmf,
        fs,
        &mut bsp,
        &mut output.report,
        control,
        map.materials.prop_lighting_sources_mut(),
        &prepared_instances.static_prop_metadata(),
        material_sources
            .as_ref()
            .filter(|_| options.pack_static_prop_assets),
    )?;
    output.static_prop_sources = prop_stats.source_guard;
    if output.report.has_errors() {
        return Ok(());
    }
    output.stats.static_props = prop_stats.props;
    output.stats.static_prop_models = prop_stats.models;
    output.stats.static_prop_leaf_refs = prop_stats.leaf_refs;
    let static_prop_plan = if options.pack_static_prop_assets {
        let sources = material_sources
            .as_ref()
            .ok_or("Static-prop packing source owner is missing")?;
        let archive = bsp
            .pak_data_controlled(bsp::PakLimits::default(), &mut || {
                control.checkpoint().map_err(io::Error::other)
            })
            .map_err(|error| error.to_string())?;
        Some(
            static_prop_assets::collect_static_prop_asset_plan(
                &bsp,
                fs,
                sources,
                &archive,
                bsp::PakLimits::default(),
                control,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let detail_stats = detail_props::bake(
        vmf,
        fs,
        &map,
        &models,
        &mut bsp,
        &mut output.report,
        control,
    )?;
    if output.report.has_errors() {
        return Ok(());
    }
    output.stats.detail_props = detail_stats.props;
    output.stats.detail_prop_models = detail_stats.models;
    output.stats.detail_prop_sprites = detail_stats.sprites;
    if options.lighting.performs_lighting() {
        control.advance(Stage::Lighting)?;
        let prop_material_sources = map.materials.take_prop_lighting_sources();
        let result = lighting::bake(
            &map,
            &models[0].tree,
            &visibility,
            &mut bsp,
            &mut output.report,
            lighting::Settings {
                control,
                fs,
                vmf,
                options,
                smoothing_sources: &emission.smoothing_sources,
                face_sources: &emission.face_sources,
                model_entities: &emission.model_entities,
                world_leaf_nodes: &emission.world_leaf_nodes,
                detail_model_centers: &detail_stats.model_centers,
                detail_lighting_origins: &detail_stats.lighting_origins,
                detail_lighting_tints: &detail_stats.lighting_tints,
                prop_sources: &prop_stats.sources,
                prop_material_sources,
            },
            &mut output.stats.lighting_progress,
        );
        if let Some(progress) = &output.stats.lighting_progress {
            output.stats.lighting_work = (progress.preparation_work + progress.ldr_work) as usize;
            output.stats.lighting_total_work = progress.work as usize;
            output.stats.lightmap_samples = progress.ldr_lightmap_samples_completed as usize;
            if options.lighting == LightingMode::DirectLdrHdr {
                output.stats.hdr = Some(HdrLightingStats {
                    lighting_work: progress.hdr_work as usize,
                    lightmap_samples: progress.hdr_lightmap_samples_completed as usize,
                    ..Default::default()
                });
            }
        }
        let stats = result?;
        if output.report.has_errors() {
            return Ok(());
        }
        output.stats.world_lights = stats.lights;
        output.stats.light_styles = stats.light_styles;
        output.stats.styled_faces = stats.styled_faces;
        output.stats.light_style_maps = stats.light_style_maps;
        output.stats.lighting_faces = stats.faces;
        output.stats.lit_brush_models = stats.models;
        output.stats.model_lighting_faces = stats.model_faces;
        output.stats.lightmap_samples = stats.samples;
        output.stats.lighting_bytes = stats.bytes;
        output.stats.lighting_work = stats.work;
        output.stats.lighting_total_work = stats.total_work;
        output.prop_lighting_assets = stats.prop_lighting_assets;
        output.stats.prop_vertex_lighting = stats.prop_vertex_lighting;
        output.stats.hdr = stats.hdr;
        if let Some(hdr) = output
            .stats
            .hdr
            .as_ref()
            .filter(|hdr| !hdr.map_data_present)
        {
            let mut missing = Vec::new();
            if hdr.lighting_bytes == 0 {
                missing.push("HDR light samples (lump 53)");
            }
            if hdr.world_lights == 0 {
                missing.push("HDR world lights (lump 54)");
            }
            if hdr.ambient_samples == 0 {
                missing.push("HDR model ambient samples (lump 55)");
            }
            output.report.diagnostics.push(Diagnostic::warning(
                format!(
                    "HDR unavailable: incomplete map data; missing {}. These arrays must all be nonempty to activate native HDR",
                    missing.join(", ")
                ),
                None,
                None,
            ));
        }
        output.stats.prop_shadow_props = stats.prop_shadows.props;
        output.stats.prop_shadow_models = stats.prop_shadows.models;
        output.stats.prop_shadow_triangles = stats.prop_shadows.triangles;
        output.stats.prop_shadow_disabled = stats.prop_shadows.disabled;
        output.stats.prop_shadow_work = stats.prop_shadows.work;
        output.stats.prop_shadow_alpha_triangles = stats.prop_shadows.alpha_triangles;
        output.stats.prop_shadow_alpha_materials = stats.prop_shadows.alpha_materials;
        output.stats.prop_shadow_alpha_textures = stats.prop_shadows.alpha_textures;
        output.stats.prop_shadow_alpha_bytes = stats.prop_shadows.alpha_bytes;
        output.stats.light_evaluations = stats.culling.evaluated;
        output.stats.light_pvs_skipped = stats.culling.pvs_skipped;
        output.stats.light_range_skipped = stats.culling.range_skipped;
        output.stats.light_culling_work = stats.culling.work;
        output.stats.ambient_samples = stats.ambient_samples;
        output.stats.ambient_bytes = stats.ambient_bytes;
        output.stats.bounces = stats.bounces;
        output.stats.bounce_rays = stats.bounce_rays;
        output.stats.bounce_samples = stats.bounce_samples;
        output.stats.bounce_work = stats.bounce_work;
        output.stats.model_bounce_faces = stats.model_bounce_faces;
        output.stats.model_bounce_samples = stats.model_bounce_samples;
        output.stats.bump_faces = stats.bump_faces;
        output.stats.bump_lightmaps = stats.bump_lightmaps;
        output.stats.smoothed_faces = stats.smoothed_faces;
        output.stats.smoothed_corners = stats.smoothed_corners;
        output.stats.normal_work = stats.normal_work;
        output.stats.smoothing_angle = options.smoothing_angle;
    }
    if options.physics == PhysicsMode::Engine {
        control.advance(Stage::Physics)?;
        let stats = physics::bake(
            &map,
            &models,
            &water,
            physics::Settings { vmf, fs },
            &mut bsp,
            &mut output.report,
            control,
        )?;
        output.stats.physics_models = stats.models;
        output.stats.physics_solids = stats.solids;
        output.stats.physics_convexes = stats.convexes;
        output.stats.physics_bytes = stats.bytes;
        output.stats.physics_fluids = stats.fluids;
        output.stats.physics_mesh_triangles = stats.mesh_triangles;
        output.stats.physics_surface_properties = stats.surface_properties;
    }
    control.checkpoint()?;
    if let Some(mut plan) = reflection_plan {
        cubemap_assets::pack(&mut bsp, &mut plan.manifest, plan.entries, control)
            .map_err(|error| error.to_string())?;
        output.cubemap_assets = Some(plan.manifest);
    }
    if options.pack_sky_assets {
        let archive = bsp
            .pak_data_controlled(bsp::PakLimits::default(), &mut || {
                control.checkpoint().map_err(io::Error::other)
            })
            .map_err(|error| error.to_string())?;
        let plan = sky_assets::collect_sky_asset_plan(
            vmf,
            fs,
            &archive,
            bsp::PakLimits::default(),
            control,
        )
        .map_err(|error| error.to_string())?;
        plan.attach(&mut bsp, control)
            .map_err(|error| error.to_string())?;
        output.stats.sky_assets = Some(PackedSkyStats {
            family: plan.family().into(),
            channels: if plan.has_hdr() { "ldr_hdr" } else { "ldr" },
            files: plan.entries().len(),
            asset_bytes: plan.asset_bytes(),
        });
        output.sky_assets = Some(plan);
    }
    if let Some(plan) = water_plan {
        plan.verify_sources(fs, control)
            .map_err(|error| error.to_string())?;
        plan.attach(&mut bsp, control)
            .map_err(|error| error.to_string())?;
        output.stats.water_assets = Some(PackedWaterStats {
            roots: plan.roots().len(),
            channels: "ldr_hdr",
            files: plan.entries().len(),
            asset_bytes: plan.asset_bytes(),
        });
        output.water_assets = Some(plan);
    }
    if let Some(plan) = static_prop_plan {
        plan.verify_sources(fs, control)
            .map_err(|error| error.to_string())?;
        plan.attach(&mut bsp, control)
            .map_err(|error| error.to_string())?;
        output.stats.static_prop_assets = Some(PackedStaticPropStats {
            models: plan.models().len(),
            files: plan.entries().len(),
            asset_bytes: plan.asset_bytes(),
        });
        output.static_prop_assets = Some(plan);
    }
    if let Some(sources) = &material_sources {
        output.material_sources = Some(
            sources
                .source_guard(fs, control)
                .map_err(|error| error.to_string())?,
        );
    }
    if options.pack_static_prop_assets && output.stats.static_props > 0 {
        output.static_prop_sources = output.material_sources.clone();
    }
    output.stats.nodes = bsp.records::<bsp::Node>().map_err(|e| e.to_string())?.len();
    output.stats.leaves = bsp.records::<bsp::Leaf>().map_err(|e| e.to_string())?.len();
    output.stats.faces = bsp.records::<bsp::Face>().map_err(|e| e.to_string())?.len();
    output.stats.models = models.len();
    output.face_origins = emission
        .face_origins
        .and_then(|origins| origins.finish(&bsp))
        .map(std::sync::Arc::new);
    output.bsp = Some(bsp);
    Ok(())
}

pub fn compile(vmf: &Vmf, fs: &GameFs) -> Output {
    compile_controlled(vmf, fs, &Control::default())
}

pub fn compile_controlled(vmf: &Vmf, fs: &GameFs, control: &Control) -> Output {
    compile_with_options_controlled(vmf, fs, Options::default(), control)
}

pub fn compile_with_options(vmf: &Vmf, fs: &GameFs, options: Options) -> Output {
    compile_with_options_controlled(vmf, fs, options, &Control::default())
}

pub fn compile_with_options_controlled(
    vmf: &Vmf,
    fs: &GameFs,
    options: Options,
    control: &Control,
) -> Output {
    compile_with_context_controlled(vmf, fs, options, None, control)
}

pub fn compile_with_context_controlled(
    vmf: &Vmf,
    fs: &GameFs,
    options: Options,
    context: Option<&CompileContext>,
    control: &Control,
) -> Output {
    compile_with_source_controlled(vmf, fs, options, context, None, control)
}

pub fn compile_with_source_controlled(
    vmf: &Vmf,
    fs: &GameFs,
    options: Options,
    context: Option<&CompileContext>,
    source: Option<&instances::SourceContext>,
    control: &Control,
) -> Output {
    let start = Instant::now();
    let mut output = Output {
        face_origins: Err("No successful face emission mapping was retained".into()),
        instance_dependencies: None,
        bsp: None,
        report: Report::default(),
        leak: None,
        stats: Stats::default(),
        cubemap_assets: None,
        sky_assets: None,
        water_assets: None,
        static_prop_assets: None,
        material_sources: None,
        static_prop_sources: None,
        prop_lighting_assets: None,
    };
    output.stats.lighting_mode = options.lighting;
    output.stats.physics_mode = options.physics;
    output.stats.visibility_mode = options.visibility;
    let prepared = if let Err(message) = options
        .validate()
        .and_then(|_| control.advance(Stage::Loading))
    {
        Err(Diagnostic::error(message, None, None))
    } else {
        let bindings = if instances::requires_source(vmf) && options.generate_reflections {
            cubemap_assets::binding::validate_instance_root_bindings(vmf, control)
        } else {
            Ok(())
        };
        bindings
            .map_err(|message| Diagnostic::error(message, None, None))
            .and_then(|()| instances::prepare(vmf, source, control, &mut output.report))
    };
    match prepared {
        Err(diagnostic) => output.report.diagnostics.push(diagnostic),
        Ok(prepared) => {
            output.instance_dependencies = prepared.dependencies.clone();
            output.stats.instances = prepared.stats.clone();
            let diagnostics_before_build = output.report.diagnostics.len();
            if let Err(message) = build(
                &prepared,
                fs,
                &mut output,
                options,
                context,
                control,
                face_origins::FaceOrigins::prepare(prepared.vmf.as_ref(), &prepared),
            ) && !output.report.diagnostics[diagnostics_before_build..]
                .iter()
                .any(|diagnostic| diagnostic.level == Level::Error && diagnostic.message == message)
            {
                output
                    .report
                    .diagnostics
                    .push(Diagnostic::error(message, None, None));
            }
            if let Some(progress) = &mut output.stats.lighting_progress {
                for contributor in &mut progress.contributors {
                    if let Some((owner, file, original_entity_id)) =
                        prepared.entity_origin(contributor.compiled_entity_id)
                    {
                        contributor.entity_id = owner;
                        contributor.instance = Some(lighting::LightingInstanceSource {
                            file: map_core::entity_helpers::bounded_preview(
                                file,
                                instances::MAX_FIELD_BYTES,
                            ),
                            original_entity_id,
                            file_truncated: file.len() > instances::MAX_FIELD_BYTES,
                        });
                    }
                }
            }
            prepared.annotate(&mut output.report);
            if let Some(leak) = &mut output.leak
                && let Some(owner) = prepared.root_entity(leak.entity)
            {
                leak.entity = owner;
            }
        }
    }
    if control.cancelled() {
        output.bsp = None;
        output.leak = None;
    }
    output.stats.elapsed = start.elapsed();
    output
}
