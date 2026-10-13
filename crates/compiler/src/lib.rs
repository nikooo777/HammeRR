//! Compiles VMF brush maps into Source engine BSP files.

pub mod control;
mod emit;
pub mod faces;
pub mod lighting;
pub mod map;
pub mod materials;
pub mod planes;
pub mod portals;
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
                "Bakes unnamed static point lights with constant, linear or quadratic falloff and opaque brush shadows. Bounced light, sky/ambient, HDR, props and brush-model lighting are unfinished."
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
pub struct Options {
    pub lighting: LightingMode,
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

#[derive(Debug, Default)]
pub struct Stats {
    pub lighting_mode: LightingMode,
    pub brushes: usize,
    pub nodes: usize,
    pub leaves: usize,
    pub faces: usize,
    pub models: usize,
    pub cubemap_samples: usize,
    pub clusters: usize,
    pub visibility_portals: usize,
    pub visibility_bytes: usize,
    pub water_regions: usize,
    pub water_leaves: usize,
    pub sky_clusters: usize,
    pub world_lights: usize,
    pub lighting_faces: usize,
    pub lightmap_samples: usize,
    pub lighting_bytes: usize,
    pub lighting_work: usize,
    pub elapsed: Duration,
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
    control.advance(Stage::Loading)?;
    let mut map = map::Map::load_controlled(vmf, fs, &mut output.report, control)?;
    output.stats.brushes = map.brushes.len();
    output.stats.cubemap_samples = map.cubemaps.len();
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
    let structural: Vec<usize> = map.entities[0]
        .brushes
        .iter()
        .copied()
        .filter(|&i| map.brushes[i].structural())
        .collect();
    control.advance(Stage::WorldTree)?;
    let mut tree =
        tree::Tree::build_controlled(&map.brushes, &structural, &mut map.planes, control)?;
    control.advance(Stage::Portals)?;
    let portals = portals::build_controlled(&tree, &map.planes, control)?;
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
    let mut visibility = visibility::build(&tree, &portals, control)?;
    output.stats.clusters = visibility.count;
    output.stats.visibility_portals = visibility.portals;
    output.stats.visibility_bytes = visibility.data.len();
    output.report.diagnostics.push(Diagnostic::info(
        "Visibility uses conservative portal bounds; tighter portal-flow clipping is not implemented yet",
        None,
        None,
    ));
    control.advance(Stage::Faces)?;
    let faces = faces::build_controlled(&map, &tree, 0, control)?;
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
        let faces = faces::build_controlled(&map, &tree, entity, control)?;
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
    let mut bsp = emit::build(&map, &models, &visibility, &water, revision, control)?;
    if options.lighting == LightingMode::DirectLdr {
        control.advance(Stage::Lighting)?;
        let stats = lighting::bake(
            &map,
            &models[0].tree,
            &visibility,
            &mut bsp,
            &mut output.report,
            control,
        )?;
        if output.report.has_errors() {
            return Ok(());
        }
        output.stats.world_lights = stats.lights;
        output.stats.lighting_faces = stats.faces;
        output.stats.lightmap_samples = stats.samples;
        output.stats.lighting_bytes = stats.bytes;
        output.stats.lighting_work = stats.work;
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
