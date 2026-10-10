//! Editor state shared by the UI and the MCP server. Every user-visible
//! operation goes through here, so both front ends behave identically.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use glam::DVec3;
use map_core::query::{self, Hit, Ray};
use map_core::{Document, EditError, ObjectId, Transaction};
use render::{Camera, MaterialLibrary, SceneMesh, ViewOptions};
use source_formats::fgd::{self, Catalog, ClassKind};
use source_formats::gamefs::GameFs;
use source_formats::keyvalues::WriteOptions;
use source_formats::vmf::Vmf;

use crate::game;

const LOG_LENGTH: usize = 50;

#[derive(Clone)]
struct Snapshot {
    generation: u64,
    revision: u64,
}

struct CompiledMap {
    path: PathBuf,
    snapshot: Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    User,
    Agent,
}

impl Actor {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "you",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub actor: Actor,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    Replace,
    Add,
    Remove,
    Toggle,
}

pub struct Session {
    game: Arc<GameFs>,
    game_error: Option<String>,
    document: Document,
    path: Option<PathBuf>,
    selection: BTreeSet<ObjectId>,
    camera: Camera,
    view: ViewOptions,
    materials: MaterialLibrary,
    log: VecDeque<LogEntry>,
    /// Bumped whenever what the viewport shows changes, except the camera.
    scene_serial: u64,
    /// Bumped when the camera is set from outside the viewport.
    camera_serial: u64,
    log_serial: u64,
    document_generation: u64,
    compiling: bool,
    launching: bool,
    compiled: Option<CompiledMap>,
    leak: Option<(Snapshot, compiler::Leak)>,
    game_run: Option<game::Run>,
    game_exit_reported: bool,
    closing: bool,
    definitions: Catalog,
    definitions_error: Option<String>,
    definitions_serial: u64,
}

pub struct CompileRequest {
    vmf: Vmf,
    game: Arc<GameFs>,
    path: PathBuf,
    generation: u64,
    revision: u64,
}

pub struct CompileResult {
    pub output: compiler::Output,
    pub path: PathBuf,
    generation: u64,
    revision: u64,
}

pub struct LaunchRequest {
    install: game::Install,
    bsp: PathBuf,
}

impl LaunchRequest {
    pub fn run(self) -> Result<game::Run, String> {
        game::Run::launch(&self.install, &self.bsp)
    }
}

impl CompileRequest {
    pub fn run(self) -> Result<CompileResult, String> {
        let output = compiler::compile(&self.vmf, &self.game);
        output
            .write(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        Ok(CompileResult {
            output,
            path: self.path,
            generation: self.generation,
            revision: self.revision,
        })
    }
}

impl Session {
    pub fn new(game: Result<GameFs, String>) -> Self {
        let (game, game_error) = match game {
            Ok(game) => (game, None),
            Err(error) => (
                GameFs {
                    game_dir: PathBuf::new(),
                    base_dir: PathBuf::new(),
                    mounts: Vec::new(),
                },
                Some(error),
            ),
        };
        Self {
            game: Arc::new(game),
            game_error,
            document: Document::empty(),
            path: None,
            selection: BTreeSet::new(),
            camera: Camera::looking_at(
                DVec3::new(-512.0, -512.0, 384.0).as_vec3(),
                glam::Vec3::ZERO,
            ),
            view: ViewOptions::default(),
            materials: MaterialLibrary::default(),
            log: VecDeque::new(),
            scene_serial: 1,
            camera_serial: 1,
            log_serial: 0,
            document_generation: 0,
            compiling: false,
            launching: false,
            compiled: None,
            leak: None,
            game_run: None,
            game_exit_reported: false,
            closing: false,
            definitions: Catalog::default(),
            definitions_error: None,
            definitions_serial: 0,
        }
    }

    pub fn game(&self) -> &GameFs {
        &self.game
    }

    pub fn game_error(&self) -> Option<&str> {
        self.game_error.as_deref()
    }

    pub fn definitions(&self) -> &Catalog {
        &self.definitions
    }

    pub fn search_entity_classes(
        &self,
        query: &str,
        kind: Option<ClassKind>,
        limit: usize,
    ) -> Vec<&fgd::Class> {
        let query = query.to_ascii_lowercase();
        self.definitions
            .classes()
            .filter(|class| {
                kind.is_none_or(|kind| class.kind == kind)
                    && (class.name.to_ascii_lowercase().contains(&query)
                        || class.description.to_ascii_lowercase().contains(&query))
            })
            .take(limit.min(1000))
            .collect()
    }

    pub fn definitions_error(&self) -> Option<&str> {
        self.definitions_error.as_deref()
    }

    pub fn definitions_serial(&self) -> u64 {
        self.definitions_serial
    }

    pub fn set_definitions(&mut self, actor: Actor, catalog: Catalog) {
        let count = catalog.classes().count();
        self.definitions = catalog;
        self.definitions_error = None;
        self.definitions_serial += 1;
        self.changed();
        self.log(actor, format!("loaded {count} entity classes"));
    }

    pub fn report_definitions_error(&mut self, error: String) {
        self.log(Actor::User, format!("entity definitions: {error}"));
        self.definitions_error = Some(error);
    }

    pub fn placement_origin(&self) -> DVec3 {
        let position = (self.camera.position + self.camera.forward() * 128.0).as_dvec3();
        (position / 16.0).round() * 16.0
    }

    fn entity_properties(
        &self,
        classname: &str,
        kind: ClassKind,
        properties: BTreeMap<String, String>,
    ) -> Result<(String, Vec<(String, String)>), String> {
        let class = self.definitions.class(classname);
        if let Some(class) = class
            && class.kind != kind
        {
            return Err(format!(
                "{} is a {} entity class",
                class.name,
                class.kind.label()
            ));
        }
        let classname = class.map_or(classname, |class| class.name.as_str());
        if classname.trim().is_empty() {
            return Err("classname must not be empty".into());
        }
        let mut values = class.map_or_else(BTreeMap::new, |class| class.defaults());
        for (key, value) in properties {
            if key.eq_ignore_ascii_case("classname") {
                if !value.eq_ignore_ascii_case(classname) {
                    return Err("classname property conflicts with the chosen class".into());
                }
                continue;
            }
            self.validate_property(classname, &key, Some(&value))?;
            let name = class
                .and_then(|class| class.property(&key))
                .map_or(key, |property| property.name.clone());
            values.insert(name, value);
        }
        Ok((classname.into(), values.into_iter().collect()))
    }

    fn validate_property(
        &self,
        classname: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        if key.trim().is_empty() {
            return Err("property name must not be empty".into());
        }
        if let Some(property) = self
            .definitions
            .class(classname)
            .and_then(|class| class.property(key))
        {
            if property.read_only {
                return Err(format!("{key} is read-only"));
            }
            if let Some(value) = value {
                property.validate(value)?;
            }
        }
        if let Some(value) = value
            && (key.eq_ignore_ascii_case("origin") || key.eq_ignore_ascii_case("angles"))
            && fgd::vector(value, 3).is_none()
        {
            return Err(format!("{key} expects three finite coordinates"));
        }
        Ok(())
    }

    pub fn create_entity(
        &mut self,
        actor: Actor,
        classname: &str,
        origin: Option<DVec3>,
        properties: BTreeMap<String, String>,
    ) -> Result<ObjectId, String> {
        let (classname, properties) =
            self.entity_properties(classname, ClassKind::Point, properties)?;
        let origin = origin
            .or_else(|| {
                properties
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("origin"))
                    .and_then(|(_, value)| fgd::vector(value, 3))
                    .map(|value| DVec3::new(value[0], value[1], value[2]))
            })
            .unwrap_or_else(|| self.placement_origin());
        if !origin.is_finite() {
            return Err("origin expects three finite coordinates".into());
        }
        let id = self.edit(actor, &format!("create {classname}"), |tx| {
            map_core::edit::create_entity(tx, &classname, Some(origin), &properties, Vec::new())
        })?;
        self.select(&[id], SelectMode::Replace)?;
        Ok(id)
    }

    pub fn set_entity_properties(
        &mut self,
        actor: Actor,
        entity: u32,
        changes: &[(String, Option<String>)],
    ) -> Result<(), String> {
        let current = self
            .document
            .entity(entity)
            .ok_or_else(|| format!("entity:{entity} does not exist"))?;
        let classname = changes
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case("classname"))
            .and_then(|(_, value)| value.as_deref())
            .or_else(|| current.classname())
            .unwrap_or("");
        if classname.trim().is_empty() {
            return Err("classname must not be empty".into());
        }
        if let Some(class) = self.definitions.class(classname) {
            let kind = if current.solids.is_empty() {
                ClassKind::Point
            } else {
                ClassKind::Solid
            };
            if class.kind != kind {
                return Err(format!(
                    "{} is a {} entity class",
                    class.name,
                    class.kind.label()
                ));
            }
        }
        for (key, value) in changes {
            self.validate_property(classname, key, value.as_deref())?;
        }
        let canonical = self
            .definitions
            .class(classname)
            .map(|class| class.name.clone());
        let mut changes = changes.to_vec();
        if let Some(canonical) = canonical {
            for (key, value) in &mut changes {
                if key.eq_ignore_ascii_case("classname") && value.is_some() {
                    *value = Some(canonical.clone());
                }
            }
        }
        self.edit(actor, "edit entity properties", |tx| {
            map_core::edit::set_properties(tx, entity, &changes)
        })
    }

    pub fn tie_to_entity(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        classname: &str,
        properties: BTreeMap<String, String>,
    ) -> Result<ObjectId, String> {
        let solids = ids
            .iter()
            .map(|id| match id {
                ObjectId::Solid(id) => Ok(*id),
                ObjectId::Entity(_) => Err(format!(
                    "{} is not a world brush",
                    crate::mcp::id_string(*id)
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (classname, properties) =
            self.entity_properties(classname, ClassKind::Solid, properties)?;
        let id = self.edit(actor, &format!("tie to {classname}"), |tx| {
            map_core::edit::tie_to_entity(tx, &solids, &classname, &properties)
        })?;
        self.select(&[id], SelectMode::Replace)?;
        Ok(id)
    }

    pub fn move_to_world(&mut self, actor: Actor, entity: u32) -> Result<Vec<ObjectId>, String> {
        let solids = self.edit(actor, "move to world", |tx| {
            map_core::edit::move_to_world(tx, entity)
        })?;
        self.select(&solids, SelectMode::Replace)?;
        Ok(solids)
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map_or_else(
            || "untitled.vmf".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        if self.document.is_dirty() {
            format!("{name} *")
        } else {
            name
        }
    }

    pub fn scene_serial(&self) -> u64 {
        self.scene_serial
    }

    pub fn camera_serial(&self) -> u64 {
        self.camera_serial
    }

    pub fn log_serial(&self) -> u64 {
        self.log_serial
    }

    pub fn log(&mut self, actor: Actor, text: impl Into<String>) {
        if self.log.len() == LOG_LENGTH {
            self.log.pop_front();
        }
        self.log.push_back(LogEntry {
            actor,
            text: text.into(),
        });
        self.log_serial += 1;
    }

    /// Most recent first.
    pub fn recent_log(&self) -> impl Iterator<Item = &LogEntry> {
        self.log.iter().rev()
    }

    fn changed(&mut self) {
        self.scene_serial += 1;
    }

    fn ensure_discardable(&self, discard_changes: bool) -> Result<(), String> {
        if self.document.is_dirty() && !discard_changes {
            return Err(format!(
                "{} has unsaved changes; save first or pass discard_changes",
                self.title()
            ));
        }
        Ok(())
    }

    fn replace_document(&mut self, document: Document, path: Option<PathBuf>) {
        self.camera = Camera::from_vmf(document.vmf()).unwrap_or_else(|| {
            let bounds = query::union(
                document
                    .objects()
                    .filter_map(|id| query::bounds(&document, id)),
            );
            bounds.map_or(self.camera, |(lo, hi)| {
                Camera::overview(lo.as_vec3(), hi.as_vec3())
            })
        });
        self.document = document;
        self.document_generation += 1;
        self.compiled = None;
        self.leak = None;
        self.path = path;
        self.selection.clear();
        self.camera_serial += 1;
        self.changed();
    }

    pub fn open(
        &mut self,
        actor: Actor,
        path: &Path,
        discard_changes: bool,
    ) -> Result<String, String> {
        self.ensure_discardable(discard_changes)?;
        let start = Instant::now();
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let vmf = Vmf::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let document = Document::new(vmf);
        let message = format!(
            "opened {} ({} brushes, {} entities) in {:.0?}",
            path.display(),
            document.vmf().world.solids.len(),
            document.vmf().entities.len(),
            start.elapsed()
        );
        self.replace_document(document, Some(path.to_owned()));
        self.log(actor, &message);
        Ok(message)
    }

    pub fn new_map(&mut self, actor: Actor, discard_changes: bool) -> Result<(), String> {
        self.ensure_discardable(discard_changes)?;
        self.replace_document(Document::empty(), None);
        self.camera = Camera::looking_at(glam::Vec3::new(-512.0, -512.0, 384.0), glam::Vec3::ZERO);
        self.log(actor, "new map");
        Ok(())
    }

    /// Saves to `path`, or to the current file. Writes a temporary file
    /// first so a failed write never truncates the map.
    pub fn save(&mut self, actor: Actor, path: Option<&Path>) -> Result<PathBuf, String> {
        let path = path
            .map(Path::to_owned)
            .or_else(|| self.path.clone())
            .ok_or("the map has no file name yet; give a path")?;
        self.document.bump_map_version();
        let text = self.document.vmf().write(&WriteOptions::default());
        let temp = path.with_extension("vmf.tmp");
        std::fs::write(&temp, text)
            .and_then(|()| std::fs::rename(&temp, &path))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.document.mark_saved();
        self.path = Some(path.clone());
        self.changed();
        self.log(actor, format!("saved {}", path.display()));
        Ok(path)
    }

    /// Runs one undoable edit and logs it under `label`.
    pub fn edit<R>(
        &mut self,
        actor: Actor,
        label: &str,
        f: impl FnOnce(&mut Transaction) -> Result<R, EditError>,
    ) -> Result<R, String> {
        let before = self.document.revision();
        let result = self.document.edit(label, f).map_err(|e| e.to_string())?;
        if self.document.revision() != before {
            self.prune_selection();
            self.changed();
            self.log(actor, label);
        }
        Ok(result)
    }

    pub fn undo(&mut self, actor: Actor) -> Option<String> {
        let label = self.document.undo()?.label.clone();
        self.prune_selection();
        self.changed();
        self.log(actor, format!("undo {label}"));
        Some(label)
    }

    pub fn redo(&mut self, actor: Actor) -> Option<String> {
        let label = self.document.redo()?.label.clone();
        self.prune_selection();
        self.changed();
        self.log(actor, format!("redo {label}"));
        Some(label)
    }

    fn prune_selection(&mut self) {
        let doc = &self.document;
        self.selection.retain(|&id| doc.contains(id));
    }

    pub fn selection(&self) -> &BTreeSet<ObjectId> {
        &self.selection
    }

    pub fn selected_ids(&self) -> Vec<ObjectId> {
        self.selection.iter().copied().collect()
    }

    pub fn select(&mut self, ids: &[ObjectId], mode: SelectMode) -> Result<(), String> {
        if let Some(missing) = ids.iter().find(|&&id| !self.document.contains(id)) {
            return Err(format!("{missing} does not exist"));
        }
        let before = self.selection.clone();
        match mode {
            SelectMode::Replace => self.selection = ids.iter().copied().collect(),
            SelectMode::Add => self.selection.extend(ids),
            SelectMode::Remove => ids.iter().for_each(|id| {
                self.selection.remove(id);
            }),
            SelectMode::Toggle => ids.iter().for_each(|&id| {
                if !self.selection.remove(&id) {
                    self.selection.insert(id);
                }
            }),
        }
        if self.selection != before {
            self.changed();
        }
        Ok(())
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// Moves the viewport camera from outside the viewport.
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
        self.camera_serial += 1;
    }

    /// Records the viewport's own camera movement.
    pub fn publish_camera(&mut self, camera: Camera) {
        self.camera = camera;
    }

    pub fn view(&self) -> ViewOptions {
        self.view
    }

    pub fn set_view(&mut self, view: ViewOptions) {
        self.view = view;
        self.changed();
    }

    pub fn materials(&self) -> &MaterialLibrary {
        &self.materials
    }

    pub fn compiling(&self) -> bool {
        self.compiling
    }

    pub fn launching(&self) -> bool {
        self.launching
    }

    fn current_snapshot(&self, snapshot: &Snapshot) -> bool {
        snapshot.generation == self.document_generation
            && snapshot.revision == self.document.revision()
    }

    pub fn leak(&self) -> Option<&compiler::Leak> {
        self.leak
            .as_ref()
            .filter(|(snapshot, _)| self.current_snapshot(snapshot))
            .map(|(_, leak)| leak)
    }

    pub fn game_running(&self) -> bool {
        self.game_run.as_ref().is_some_and(game::Run::running)
    }

    pub fn begin_launch(&mut self, actor: Actor) -> Result<LaunchRequest, String> {
        if self.closing {
            return Err("the editor is closing".into());
        }
        if self.launching || self.game_running() {
            return Err(
                "CS:S is already launching or running; close it before running another map".into(),
            );
        }
        if self.compiling {
            return Err("wait for map compilation to finish".into());
        }
        let compiled = self
            .compiled
            .as_ref()
            .ok_or("compile the map before launching CS:S")?;
        if !self.current_snapshot(&compiled.snapshot) {
            return Err(
                "the map changed since compilation; compile it again before launching".into(),
            );
        }
        let request = LaunchRequest {
            install: game::Install::from_game(&self.game)?,
            bsp: compiled.path.clone(),
        };
        self.launching = true;
        self.log(actor, "launching CS:S");
        Ok(request)
    }

    pub fn finish_launch(
        &mut self,
        actor: Actor,
        result: Result<game::Run, String>,
    ) -> Result<game::Log, String> {
        self.launching = false;
        if self.closing {
            drop(result);
            return Err("the editor is closing".into());
        }
        match result {
            Ok(run) => {
                self.game_run = Some(run);
                self.game_exit_reported = false;
                self.log(actor, "launched CS:S; open Game Log for engine output");
                self.game_log(game::DEFAULT_LOG_LINES)
            }
            Err(error) => {
                self.log(actor, format!("game launch failed: {error}"));
                Err(error)
            }
        }
    }

    pub fn poll_game(&mut self) {
        if !self.game_exit_reported
            && let Some(run) = &self.game_run
            && !run.running()
        {
            let status = run.status();
            self.game_exit_reported = true;
            self.log(Actor::User, status.label());
        }
    }

    pub fn game_log(&mut self, lines: usize) -> Result<game::Log, String> {
        self.poll_game();
        self.game_run
            .as_ref()
            .ok_or("no game has been launched by HammeRR")?
            .log(lines)
    }

    pub fn stop_game(&mut self, actor: Actor) -> Result<(), String> {
        let run = self
            .game_run
            .as_ref()
            .ok_or("no game has been launched by HammeRR")?;
        run.stop()?;
        self.game_exit_reported = true;
        self.log(actor, "stopped CS:S");
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.closing = true;
        self.game_run = None;
    }

    pub fn begin_compile(
        &mut self,
        actor: Actor,
        path: Option<&Path>,
    ) -> Result<CompileRequest, String> {
        if self.closing {
            return Err("the editor is closing".into());
        }
        if self.compiling {
            return Err("a map compilation is already running".into());
        }
        if self.launching {
            return Err("wait for CS:S to finish launching".into());
        }
        let path = path
            .map(Path::to_owned)
            .or_else(|| self.path.as_ref().map(|p| p.with_extension("bsp")))
            .ok_or("save the map first or give a .bsp output path")?;
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("bsp"))
        {
            return Err("compile output must be a .bsp file".into());
        }
        if self.path.as_deref() == Some(path.as_path()) {
            return Err("compile output must be distinct from the source map".into());
        }
        if let Some(error) = &self.game_error {
            return Err(error.clone());
        }
        let request = CompileRequest {
            vmf: self.document.vmf().clone(),
            game: self.game.clone(),
            path,
            generation: self.document_generation,
            revision: self.document.revision(),
        };
        self.compiling = true;
        self.log(actor, "compiling map");
        Ok(request)
    }

    pub fn finish_compile(&mut self, actor: Actor, result: &Result<CompileResult, String>) {
        self.compiling = false;
        if let Ok(result) = result {
            let snapshot = Snapshot {
                generation: result.generation,
                revision: result.revision,
            };
            if self.current_snapshot(&snapshot) {
                self.leak = result
                    .output
                    .leak
                    .clone()
                    .map(|leak| (snapshot.clone(), leak));
                self.changed();
            }
            if result.output.bsp.is_some() {
                self.compiled = Some(CompiledMap {
                    path: result.path.clone(),
                    snapshot,
                });
            }
        }
        let message = match result {
            Err(message) => format!("compile failed: {message}"),
            Ok(result) if result.output.bsp.is_none() => {
                let message = result
                    .output
                    .report
                    .diagnostics
                    .iter()
                    .find(|d| d.level == compiler::Level::Error)
                    .map(|d| d.message.as_str())
                    .unwrap_or("no BSP produced");
                let pointfile = if result.output.leak.is_some() {
                    format!(
                        "; pointfile {}",
                        result.path.with_extension("lin").display()
                    )
                } else {
                    String::new()
                };
                format!("compile failed: {message}{pointfile}")
            }
            Ok(result) => {
                let stale = if result.generation != self.document_generation
                    || result.revision != self.document.revision()
                {
                    " (an earlier snapshot)"
                } else {
                    ""
                };
                let stats = &result.output.stats;
                format!(
                    "compiled {}: {} brushes, {} faces in {:.1?}{stale}",
                    result.path.display(),
                    stats.brushes,
                    stats.faces,
                    stats.elapsed
                )
            }
        };
        self.log(actor, message);
    }

    /// The nearest visible object along a ray. Brushes made only of tool
    /// materials can't be picked while tool textures are hidden.
    pub fn pick(&mut self, ray: &Ray) -> Option<Hit> {
        let mut hidden = HashSet::new();
        if !self.view.tool_textures {
            let (materials, game) = (&mut self.materials, &self.game);
            for solid in self.document.vmf().solids() {
                let all_tools = solid.sides.iter().all(|side| {
                    let id = materials.load(game, &side.material);
                    materials.get(id).tool
                });
                if all_tools {
                    hidden.insert(solid.id);
                }
            }
        }
        query::pick(&self.document, ray, |s| !hidden.contains(&s.id))
    }

    pub fn build_scene(&mut self) -> SceneMesh {
        let selected: HashSet<ObjectId> = self.selection.iter().copied().collect();
        let mut mesh = SceneMesh::build(
            self.document.vmf(),
            &self.game,
            &mut self.materials,
            &selected,
        );
        if let Some(leak) = self.leak() {
            mesh.set_leak_path(&leak.points);
        }
        mesh
    }

    /// Material names (as used in maps) containing `query`, case-insensitive.
    pub fn search_materials(&self, query: &str, limit: usize) -> Vec<String> {
        let query = query.to_ascii_lowercase();
        self.game
            .list("materials", ".vmt")
            .into_iter()
            .filter_map(|p| {
                Some(
                    p.strip_prefix("materials/")?
                        .strip_suffix(".vmt")?
                        .to_owned(),
                )
            })
            .filter(|name| name.contains(&query))
            .take(limit)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::edit;

    const ENTITY_FGD: &str = r#"
        @BaseClass = LightValues [
            _light(color255) : "Brightness" : "255 240 220 250"
            style(choices) : "Style" : 0 = [ 0 : "Steady" 1 : "Pulse" ]
            spawnflags(flags) = [ 1 : "Enabled" : 1 ]
            editor_id(integer) readonly : "Editor ID"
        ]
        @PointClass base(LightValues) = light []
        @SolidClass = func_detail []
    "#;

    #[test]
    fn humans_and_agents_create_the_same_lights_with_defaults() {
        let mut entities = Vec::new();
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no game required".into()));
            session.set_definitions(actor, Catalog::parse(ENTITY_FGD).unwrap());
            let id = session
                .create_entity(
                    actor,
                    "LIGHT",
                    Some(DVec3::new(16., 32., 96.)),
                    BTreeMap::from([("targetname".into(), "test_light".into())]),
                )
                .unwrap();
            let ObjectId::Entity(id) = id else { panic!() };
            let entity = session.document().entity(id).unwrap();
            assert_eq!(entity.classname(), Some("light"));
            assert_eq!(entity.properties.get("_light"), Some("255 240 220 250"));
            assert_eq!(entity.properties.get("spawnflags"), Some("1"));
            assert_eq!(entity.origin(), Some(DVec3::new(16., 32., 96.)));
            let saved = session.document().vmf().write(&WriteOptions::default());
            let parsed = Vmf::parse(&saved).unwrap();
            assert_eq!(
                parsed.entities[0].properties.get("_light"),
                entity.properties.get("_light")
            );
            entities.push(entity.clone());
            session.undo(actor).unwrap();
            assert!(session.document().entity(id).is_none());
            session.redo(actor).unwrap();
            assert!(session.document().entity(id).is_some());
        }
        assert_eq!(entities[0], entities[1]);
    }

    #[test]
    fn invalid_entity_edits_are_atomic_and_custom_data_survives() {
        let mut session = Session::new(Err("no game required".into()));
        session.set_definitions(Actor::Agent, Catalog::parse(ENTITY_FGD).unwrap());
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::Agent,
                "light",
                Some(DVec3::ZERO),
                BTreeMap::from([("custom_key".into(), "keep me".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        let revision = session.document().revision();
        for (key, value) in [
            ("_light", "300 0 0 250"),
            ("style", "99"),
            ("origin", "0 NaN 0"),
            ("editor_id", "2"),
            ("classname", "func_detail"),
        ] {
            assert!(
                session
                    .set_entity_properties(
                        Actor::User,
                        id,
                        &[
                            ("targetname".into(), Some("must not apply".into())),
                            (key.into(), Some(value.into()))
                        ]
                    )
                    .is_err()
            );
            assert_eq!(session.document().revision(), revision);
        }
        session
            .set_entity_properties(
                Actor::User,
                id,
                &[
                    ("classname".into(), Some("LIGHT".into())),
                    ("_light".into(), Some("128 96 64 500".into())),
                    ("style".into(), Some("1".into())),
                    ("spawnflags".into(), Some("1025".into())),
                ],
            )
            .unwrap();
        let entity = session.document().entity(id).unwrap();
        assert_eq!(entity.classname(), Some("light"));
        assert_eq!(entity.properties.get("_light"), Some("128 96 64 500"));
        assert_eq!(entity.properties.get("custom_key"), Some("keep me"));
        assert_eq!(entity.properties.get("spawnflags"), Some("1025"));
        assert!(entity.properties.get("targetname").is_none());
        assert!(
            session
                .create_entity(Actor::Agent, "func_detail", None, BTreeMap::new())
                .is_err()
        );
        assert!(
            session
                .create_entity(Actor::User, "custom_light", None, BTreeMap::new())
                .is_ok()
        );
    }

    #[test]
    fn brush_entity_conversion_uses_shared_validation_and_undo() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no game required".into()));
            session.set_definitions(actor, Catalog::parse(ENTITY_FGD).unwrap());
            let brush = session
                .edit(actor, "block", |tx| {
                    edit::create_block(tx, DVec3::ZERO, DVec3::splat(32.), "tools/toolsnodraw")
                })
                .unwrap();
            assert!(
                session
                    .tie_to_entity(actor, &[brush], "light", BTreeMap::new())
                    .is_err()
            );
            let ObjectId::Entity(id) = session
                .tie_to_entity(actor, &[brush], "func_detail", BTreeMap::new())
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(session.document().entity(id).unwrap().solids.len(), 1);
            assert_eq!(session.move_to_world(actor, id).unwrap(), [brush]);
            session.undo(actor).unwrap();
            assert!(session.document().entity(id).is_some());
            session.undo(actor).unwrap();
            assert!(session.document().entity(id).is_none());
            assert!(
                session
                    .document()
                    .solid(match brush {
                        ObjectId::Solid(id) => id,
                        _ => panic!(),
                    })
                    .is_some()
            );
        }
    }

    fn room() -> Vmf {
        let mut document = Document::empty();
        document
            .edit("room", |tx| {
                for (mins, maxs) in [
                    ([-272., -272., -16.], [272., 272., 0.]),
                    ([-272., -272., 256.], [272., 272., 272.]),
                    ([-272., -272., 0.], [-256., 272., 256.]),
                    ([256., -272., 0.], [272., 272., 256.]),
                    ([-256., -272., 0.], [256., -256., 256.]),
                    ([-256., 256., 0.], [256., 272., 256.]),
                ] {
                    edit::create_block(
                        tx,
                        DVec3::from_array(mins),
                        DVec3::from_array(maxs),
                        "dev/dev_measuregeneric01",
                    )?;
                }
                edit::create_entity(
                    tx,
                    "info_player_terrorist",
                    Some(DVec3::new(0., 0., 8.)),
                    &[],
                    Vec::new(),
                )?;
                Ok(())
            })
            .unwrap();
        document.vmf().clone()
    }

    fn compile(session: &mut Session) -> CompileResult {
        let result = session.begin_compile(Actor::Agent, None).unwrap().run();
        session.finish_compile(Actor::Agent, &result);
        result.unwrap()
    }

    #[test]
    fn changed_document_cannot_launch_an_earlier_compile() {
        let (directory, game, _) = game::tests::fixture(true);
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(room()),
            Some(directory.path().join("room.vmf")),
        );
        let request = session.begin_compile(Actor::Agent, None).unwrap();
        assert!(session.begin_compile(Actor::User, None).is_err());
        session
            .edit(Actor::User, "add target", |tx| {
                edit::create_entity(tx, "info_target", None, &[], Vec::new())
            })
            .unwrap();
        let result = request.run();
        session.finish_compile(Actor::Agent, &result);
        assert!(result.unwrap().output.bsp.is_some());
        assert!(
            session
                .begin_launch(Actor::Agent)
                .err()
                .unwrap()
                .contains("map changed")
        );
        assert!(!session.compiling() && !session.launching());
    }

    #[test]
    fn replacing_document_rejects_a_late_compile_result() {
        let (directory, game, _) = game::tests::fixture(true);
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(room()),
            Some(directory.path().join("room.vmf")),
        );
        let request = session.begin_compile(Actor::Agent, None).unwrap();
        session.new_map(Actor::User, true).unwrap();
        let result = request.run();
        session.finish_compile(Actor::Agent, &result);
        assert!(session.begin_launch(Actor::Agent).is_err());
        assert!(session.leak().is_none());
        assert!(
            session
                .recent_log()
                .next()
                .unwrap()
                .text
                .contains("earlier snapshot")
        );
    }

    #[test]
    fn leaks_are_drawn_until_the_document_changes() {
        let (directory, game, _) = game::tests::fixture(true);
        let mut vmf = room();
        vmf.world.solids.pop();
        let mut session = Session::new(Ok(game));
        session.replace_document(Document::new(vmf), Some(directory.path().join("room.vmf")));
        let result = compile(&mut session);
        assert!(result.output.leak.is_some());
        let lines = session.build_scene().leak_lines;
        assert_eq!(lines.len(), (session.leak().unwrap().points.len() - 1) * 2);
        assert!(lines.iter().all(|line| line.color == [255, 32, 32, 255]));
        let solid = session.document().vmf().world.solids[0].id;
        session
            .edit(Actor::User, "move room", |tx| {
                edit::translate(tx, &[ObjectId::Solid(solid)], DVec3::X, true)
            })
            .unwrap();
        assert!(session.leak().is_none());
        assert!(session.build_scene().leak_lines.is_empty());
    }

    #[test]
    fn duplicate_launches_are_rejected_and_stop_preserves_logs() {
        let (directory, game, _) = game::tests::fixture(true);
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(room()),
            Some(directory.path().join("room.vmf")),
        );
        assert!(session.begin_launch(Actor::User).is_err());
        assert!(compile(&mut session).output.bsp.is_some());
        let request = session.begin_launch(Actor::Agent).unwrap();
        assert!(session.begin_launch(Actor::User).is_err());
        assert!(session.begin_compile(Actor::User, None).is_err());
        session.finish_launch(Actor::Agent, request.run()).unwrap();
        assert!(session.game_running());
        assert!(session.begin_launch(Actor::User).is_err());
        session.stop_game(Actor::Agent).unwrap();
        assert!(!session.game_running());
        assert!(matches!(
            session.game_log(1).unwrap().status,
            game::Status::Exited { .. }
        ));
        assert!(session.begin_launch(Actor::User).is_ok());
    }

    #[test]
    fn shutdown_stops_an_active_game_with_other_session_owners() {
        let (directory, game, _) = game::tests::fixture(true);
        let game_dir = game.game_dir.clone();
        let shared = Arc::new(std::sync::Mutex::new(Session::new(Ok(game))));
        let other_owner = shared.clone();
        let mut session = shared.lock().unwrap();
        session.replace_document(
            Document::new(room()),
            Some(directory.path().join("room.vmf")),
        );
        compile(&mut session);
        let request = session.begin_launch(Actor::Agent).unwrap();
        let log = session.finish_launch(Actor::Agent, request.run()).unwrap();
        session.shutdown();
        assert!(!Path::new(&format!("/proc/{}", log.pid)).exists());
        assert!(
            !game_dir
                .join("maps")
                .join(format!("{}.bsp", log.map))
                .exists()
        );
        assert!(session.game_log(1).is_err());
        assert!(session.begin_launch(Actor::Agent).is_err());
        assert!(session.begin_compile(Actor::Agent, None).is_err());
        drop(session);
        assert!(!other_owner.lock().unwrap().game_running());
    }

    #[test]
    fn shutdown_stops_a_launch_that_finishes_late() {
        let (directory, game, _) = game::tests::fixture(true);
        let game_dir = game.game_dir.clone();
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(room()),
            Some(directory.path().join("room.vmf")),
        );
        compile(&mut session);
        let request = session.begin_launch(Actor::Agent).unwrap();
        session.shutdown();
        assert!(session.launching());
        let run = request.run().unwrap();
        let log = run.log(1).unwrap();
        assert!(session.finish_launch(Actor::Agent, Ok(run)).is_err());
        assert!(!session.launching() && !session.game_running());
        assert!(!Path::new(&format!("/proc/{}", log.pid)).exists());
        assert!(
            !game_dir
                .join("maps")
                .join(format!("{}.bsp", log.map))
                .exists()
        );
    }
}
