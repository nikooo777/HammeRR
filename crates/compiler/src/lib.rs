//! Compiles VMF brush maps into Source engine BSP files.

mod areas;
pub mod control;
mod emit;
pub mod faces;
mod hints;
pub mod lighting;
pub mod map;
pub mod materials;
mod overlays;
pub mod physics;
pub mod planes;
pub mod portals;
mod static_props;
pub mod tree;
mod visibility;
mod water;
pub mod winding;

use std::io::Write;
use std::time::{Duration, Instant};
use std::{io, path::Path};

use glam::DVec3;
use source_formats::bsp::{self, Bsp};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

pub use control::{Control, Stage};
pub use portals::Leak;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingMode {
    #[default]
    Fullbright,
    DirectLdr,
}

impl LightingMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fullbright => "fullbright",
            Self::DirectLdr => "direct_ldr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fullbright => "Fullbright",
            Self::DirectLdr => "Direct LDR",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Fullbright => "No lightmaps; useful for fast geometry checks.",
            Self::DirectLdr => {
                "Bakes static, switchable and animated point/spot lights, sun and sky on world brushes, brush entities and terrain, with directional normal-map lightmaps, shadows, scoped world/model brush smoothing and model ambient. Up to four style-preserving diffuse bounces reach world, terrain and brush entities. HDR and prop lightmaps remain unfinished."
            }
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "fullbright" => Ok(Self::Fullbright),
            "direct_ldr" => Ok(Self::DirectLdr),
            _ => Err("Lighting must be fullbright or direct_ldr".into()),
        }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub lighting: LightingMode,
    pub physics: PhysicsMode,
    pub sky_quality: SkyQuality,
    pub visibility: VisibilityMode,
    pub bounces: u8,
    pub smoothing_angle: u8,
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
    pub fn validate(self) -> Result<Self, String> {
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
                "Clips sight volumes through portal paths to remove hidden areas. Uses up to eight CPU workers on larger maps, with conservative fallbacks. Original hint priority, occluders and broader map coverage remain unfinished."
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
                "Adds world, clip, brush-model, water and terrain collision with world/terrain material surfaces. Static props use model assets. Model surfaces, density and compiler-derived masses remain unfinished."
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
pub struct Stats {
    pub lighting_mode: LightingMode,
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
    pub static_props: usize,
    pub static_prop_models: usize,
    pub static_prop_leaf_refs: usize,
    pub displacements: usize,
    pub displacement_vertices: usize,
    pub displacement_triangles: usize,
    pub displacement_neighbor_pairs: usize,
    pub displacement_corner_neighbor_pairs: usize,
    pub displacement_mixed_power_pairs: usize,
    pub displacement_half_edge_pairs: usize,
    pub displacement_lod_disabled_vertices: usize,
    pub displacement_lod_snapped_vertices: usize,
    pub clusters: usize,
    pub visibility_portals: usize,
    pub visibility_bytes: usize,
    pub visibility_work: usize,
    pub visibility_removed_pairs: usize,
    pub visibility_fallbacks: usize,
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
    #[serde(rename = "elapsed_ms", serialize_with = "serialize_elapsed")]
    pub elapsed: Duration,
}

fn serialize_elapsed<S: serde::Serializer>(
    elapsed: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

#[derive(Debug)]
pub struct Output {
    pub bsp: Option<Bsp>,
    pub report: Report,
    pub leak: Option<Leak>,
    pub stats: Stats,
}

impl Output {
    pub fn write(&self, path: &Path) -> io::Result<bool> {
        self.write_controlled(path, &Control::default())
    }

    pub fn write_controlled(&self, path: &Path, control: &Control) -> io::Result<bool> {
        control.checkpoint().map_err(io::Error::other)?;
        if self.bsp.is_none() && self.leak.is_none() {
            return Ok(false);
        }
        control.advance(Stage::Writing).map_err(io::Error::other)?;
        let (destination, bytes, success) = if let Some(bsp) = &self.bsp {
            (path.to_owned(), bsp.to_bytes(), true)
        } else if let Some(leak) = &self.leak {
            let text: String = leak
                .points
                .iter()
                .map(|&point| format!("{}\n", source_formats::vmf::format_vec3(point)))
                .collect();
            (path.with_extension("lin"), text.into_bytes(), false)
        } else {
            return Ok(false);
        };
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        for chunk in bytes.chunks(64 * 1024) {
            control.checkpoint().map_err(io::Error::other)?;
            temporary.write_all(chunk)?;
        }
        temporary.as_file().sync_all()?;
        control.publish(|| {
            temporary
                .persist(&destination)
                .map(|_| ())
                .map_err(|error| error.error)
        })?;
        Ok(success)
    }
}

fn build(
    vmf: &Vmf,
    fs: &GameFs,
    output: &mut Output,
    options: Options,
    control: &Control,
) -> Result<(), String> {
    options.validate()?;
    control.advance(Stage::Loading)?;
    let mut map = map::Map::load_controlled(vmf, fs, &mut output.report, control)?;
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
    for info in map.materials.iter().filter(|m| !m.found) {
        output.report.diagnostics.push(Diagnostic::warning(
            format!("material {} was not found", info.name),
            None,
            None,
        ));
    }
    if output.report.has_errors() {
        return Ok(());
    }
    if options.lighting == LightingMode::DirectLdr {
        output.stats.switchable_light_groups = lighting::styles::prepare(&mut map, control)?;
    }
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
    output.leak = portals::flood_controlled(
        &mut tree,
        &map.planes,
        &portals,
        &map.entities,
        &mut output.report,
        control,
    )?;
    if output.report.has_errors() {
        return Ok(());
    }
    control.advance(Stage::Hints)?;
    let hints = hints::apply(&map, &mut tree, &mut output.report, control, hint_priority)?;
    output.stats.hint_splits = hints.splits;
    output.stats.hint_priority_splits = hints.priority_splits;
    output.stats.hint_work = hints.work;
    if hints.splits > 0 {
        control.advance(Stage::Portals)?;
        portals = portals::build_controlled(&tree, &map.planes, control)?;
    }
    control.advance(Stage::Areas)?;
    let areas = areas::build(&mut map, &tree, &portals, control)?;
    output.stats.areas = areas.data.areas.len().saturating_sub(1);
    output.stats.areaportals = map.area_portals.len();
    output.stats.areaportal_windows = map
        .area_portals
        .iter()
        .filter(|portal| map.entities[portal.entity].classname == "func_areaportalwindow")
        .count();
    output.stats.areaportal_vertices = areas.data.clip_vertices.len();
    control.advance(Stage::Water)?;
    let water = water::build(&map, &tree, &portals, &mut output.report, control)?;
    output.stats.water_regions = water.records.len();
    output.stats.water_leaves = water.leaves.iter().filter(|&&region| region >= 0).count();
    if output.report.has_errors() {
        return Ok(());
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
    let mut visibility = visibility::build(&tree, &portals, control, options.visibility)?;
    output.stats.clusters = visibility.count;
    output.stats.visibility_portals = visibility.portals;
    output.stats.visibility_bytes = visibility.data.len();
    output.stats.visibility_mode = options.visibility;
    output.stats.visibility_work = visibility.work;
    output.stats.visibility_removed_pairs = visibility.removed_pairs;
    output.stats.visibility_fallbacks = visibility.fallbacks;
    output.stats.visibility_workers = visibility.workers;
    output.report.diagnostics.push(Diagnostic::info(
        match options.visibility {
            VisibilityMode::Fast => "Visibility uses conservative portal bounds; choose Detailed for portal separator clipping",
            VisibilityMode::Detailed => "Visibility uses bounded portal separator clipping and source-cluster tasks; hint priority and occluders remain unfinished",
        },
        None,
        None,
    ));
    control.advance(Stage::Faces)?;
    let mut retained_points = 0;
    let retention = if options.lighting != LightingMode::DirectLdr {
        faces::SourceRetention::None
    } else if options.smoothing_angle == 0 {
        faces::SourceRetention::Assigned
    } else {
        faces::SourceRetention::All
    };
    let world_retention = if options.lighting == LightingMode::Fullbright {
        faces::SourceRetention::Assigned
    } else {
        retention
    };
    let mut faces = faces::build_with_smoothing(
        &map,
        &tree,
        0,
        control,
        world_retention,
        &mut retained_points,
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
    )?;
    let mut bsp = emission.bsp;
    if let Some(data) = bsp.overlay_data().map_err(|error| error.to_string())? {
        output.stats.overlays = data.overlays.len();
        output.stats.overlay_face_refs = data
            .overlays
            .iter()
            .map(|overlay| overlay.face_count())
            .sum();
    }
    control.advance(Stage::StaticProps)?;
    let prop_stats = static_props::bake(vmf, fs, &mut bsp, &mut output.report, control)?;
    if output.report.has_errors() {
        return Ok(());
    }
    output.stats.static_props = prop_stats.props;
    output.stats.static_prop_models = prop_stats.models;
    output.stats.static_prop_leaf_refs = prop_stats.leaf_refs;
    if options.lighting == LightingMode::DirectLdr {
        control.advance(Stage::Lighting)?;
        let stats = lighting::bake(
            &map,
            &models[0].tree,
            &visibility,
            &mut bsp,
            &mut output.report,
            control,
            lighting::Settings {
                options,
                smoothing_sources: &emission.smoothing_sources,
                model_entities: &emission.model_entities,
            },
        )?;
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
            fs,
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
    output.stats.nodes = bsp.records::<bsp::Node>().map_err(|e| e.to_string())?.len();
    output.stats.leaves = bsp.records::<bsp::Leaf>().map_err(|e| e.to_string())?.len();
    output.stats.faces = bsp.records::<bsp::Face>().map_err(|e| e.to_string())?.len();
    output.stats.models = models.len();
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
    let start = Instant::now();
    let mut output = Output {
        bsp: None,
        report: Report::default(),
        leak: None,
        stats: Stats::default(),
    };
    output.stats.lighting_mode = options.lighting;
    output.stats.physics_mode = options.physics;
    output.stats.visibility_mode = options.visibility;
    if let Err(message) = build(vmf, fs, &mut output, options, control) {
        output
            .report
            .diagnostics
            .push(Diagnostic::error(message, None, None));
    }
    if control.cancelled() {
        output.bsp = None;
        output.leak = None;
    }
    output.stats.elapsed = start.elapsed();
    output
}
