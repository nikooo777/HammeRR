//! Compiles VMF brush maps into fullbright Source engine BSP files.

mod emit;
pub mod faces;
pub mod map;
pub mod materials;
pub mod planes;
pub mod portals;
pub mod tree;
pub mod winding;

use std::io::Write;
use std::time::{Duration, Instant};
use std::{io, path::Path};

use glam::DVec3;
use source_formats::bsp::{self, Bsp};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

pub use portals::Leak;

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
    pub brushes: usize,
    pub nodes: usize,
    pub leaves: usize,
    pub faces: usize,
    pub models: usize,
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
        if let Some(leak) = &self.leak {
            let text: String = leak
                .points
                .iter()
                .map(|&p| format!("{}\n", source_formats::vmf::format_vec3(p)))
                .collect();
            std::fs::write(path.with_extension("lin"), text)?;
        }
        let Some(bsp) = self.bsp.as_ref() else {
            return Ok(false);
        };
        let temporary = path.with_extension(format!("bsp.{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = file
            .write_all(&bsp.to_bytes())
            .and_then(|()| std::fs::rename(&temporary, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        Ok(true)
    }
}

fn build(vmf: &Vmf, fs: &GameFs, output: &mut Output) -> Result<(), String> {
    let mut map = map::Map::load(vmf, fs, &mut output.report);
    output.stats.brushes = map.brushes.len();
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
    let mut tree = tree::Tree::build(&map.brushes, &structural, &mut map.planes)?;
    let portals = portals::build(&tree, &map.planes);
    output.leak = portals::flood(
        &mut tree,
        &map.planes,
        &portals,
        &map.entities,
        &mut output.report,
    );
    if output.report.has_errors() {
        return Ok(());
    }
    let faces = faces::build(&map, &tree, 0);
    let mut models = vec![emit::Model {
        entity: 0,
        tree,
        faces,
    }];
    for entity in 1..map.entities.len() {
        let indices = &map.entities[entity].brushes;
        if indices.is_empty() {
            continue;
        }
        let tree = tree::Tree::build(&map.brushes, indices, &mut map.planes)?;
        let faces = faces::build(&map, &tree, entity);
        models.push(emit::Model {
            entity,
            tree,
            faces,
        });
    }
    let revision = vmf
        .version_info
        .as_ref()
        .and_then(|p| p.get("mapversion"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let bsp = emit::build(&map, &models, revision)?;
    output.stats.nodes = bsp.records::<bsp::Node>().map_err(|e| e.to_string())?.len();
    output.stats.leaves = bsp.records::<bsp::Leaf>().map_err(|e| e.to_string())?.len();
    output.stats.faces = bsp.records::<bsp::Face>().map_err(|e| e.to_string())?.len();
    output.stats.models = models.len();
    output.bsp = Some(bsp);
    Ok(())
}

pub fn compile(vmf: &Vmf, fs: &GameFs) -> Output {
    let start = Instant::now();
    let mut output = Output {
        bsp: None,
        report: Report::default(),
        leak: None,
        stats: Stats::default(),
    };
    if let Err(message) = build(vmf, fs, &mut output) {
        output
            .report
            .diagnostics
            .push(Diagnostic::error(message, None, None));
    }
    output.stats.elapsed = start.elapsed();
    output
}
