//! Editor state shared by the UI and the MCP server. Every user-visible
//! operation goes through here, so both front ends behave identically.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::DVec3;
use map_core::query::{self, Hit, Ray};
use map_core::{Document, EditError, ObjectId, Transaction};
use render::{Camera, MaterialLibrary, SceneMesh, ViewOptions};
use source_formats::gamefs::GameFs;
use source_formats::keyvalues::WriteOptions;
use source_formats::vmf::Vmf;

const LOG_LENGTH: usize = 50;

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
    game: GameFs,
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
            game,
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
        }
    }

    pub fn game(&self) -> &GameFs {
        &self.game
    }

    pub fn game_error(&self) -> Option<&str> {
        self.game_error.as_deref()
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
        SceneMesh::build(
            self.document.vmf(),
            &self.game,
            &mut self.materials,
            &selected,
        )
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
