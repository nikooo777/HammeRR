//! Editor state shared by the UI and the MCP server. Every user-visible
//! operation goes through here, so both front ends behave identically.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use glam::{DVec2, DVec3};
use map_core::query::{self, Hit, Ray};
use map_core::surface::{self, FaceId, SurfaceEdit};
use map_core::{Document, EditError, ObjectId, Transaction};
use render::{Camera, MaterialLibrary, Projection, SceneMesh, ViewOptions};
use source_formats::fgd::{self, Catalog, ClassKind};
use source_formats::gamefs::GameFs;
use source_formats::keyvalues::WriteOptions;
use source_formats::vmf::Vmf;

use crate::game;
use crate::inspector;
use crate::spatial;

pub mod carving;
pub mod checks;
pub mod clip_drag;
pub mod clip_points;
pub mod clipboard;
mod clipping;
pub mod compilation;
pub mod compile_region;
pub mod component_drag;
pub mod component_picking;
pub mod components;
pub mod context;
mod favorites;
pub mod groups;
pub mod hollowing;
pub mod layers;
mod management;
pub mod material_assets;
pub mod material_search;
pub mod merging;
mod model_attachments;
pub mod model_browser;
pub mod models;
pub mod object_picking;
pub mod overlay;
pub mod paint;
pub mod placement;
pub mod pointfiles;
pub mod prefabs;
pub mod primitives;
mod recent;
mod recovery;
pub mod repeat;
pub mod saved_views;
pub mod sounds;
pub mod statistics;
mod surfaces;
pub mod terrain_handles;
pub mod terrain_strokes;
pub mod terrain_triangles;
pub mod terrain_vertices;
pub mod volume;
pub use clipping::ClipPreview;
#[cfg(test)]
use compilation::CompileResult;
pub use management::{ObjectFilter, PropertyChange, PropertyDraft, PropertyTarget};
pub use recovery::RecoveryRequest;
pub use surfaces::GroupProjection;
mod connections;
pub use connections::{ConnectionDraft, ConnectionFields, ConnectionRecord};
mod references;
pub use references::RenameDraft;
mod manipulation;
#[cfg(test)]
mod preview_tests;
mod previews;
mod rename_view;
pub use manipulation::{
    AreaPreview, GridSnap, Pointer, TransformDrag, TransformPreview, TransformTool,
};

const LOG_LENGTH: usize = 50;

struct FacePreview {
    token: u64,
    document: Document,
    edits: Vec<(FaceId, SurfaceEdit)>,
    terrain_mask_token: Option<u64>,
}

#[derive(Debug)]
pub struct MaterialPreview {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub missing: bool,
    pub tool: bool,
    pub image: Option<source_formats::vtf::RgbaImage>,
}

#[derive(Clone)]
struct Snapshot {
    generation: u64,
    revision: u64,
}

struct CompiledMap {
    path: PathBuf,
    snapshot: Snapshot,
    options: compiler::Options,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    User,
    Agent,
    System,
}

impl Actor {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "you",
            Self::Agent => "agent",
            Self::System => "system",
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
    compile_region: compile_region::State,
    overlay: overlay::State,
    geometry: previews::State,
    groups: groups::State,
    layers: layers::State,
    prefabs: prefabs::State,
    model_browser: model_browser::State,
    sounds: sounds::State,
    placement: placement::State,
    model_attachments: model_attachments::State,
    models: std::cell::RefCell<render::models::Library>,
    game: Arc<GameFs>,
    game_error: Option<String>,
    document: Document,
    path: Option<PathBuf>,
    selection: BTreeSet<ObjectId>,
    face_selection: BTreeSet<FaceId>,
    face_mode: bool,
    face_preview_serial: u64,
    terrain_triangle_highlight: Option<terrain_triangles::Highlight>,
    terrain_triangle_serial: u64,
    terrain_strokes: terrain_strokes::State,
    terrain_handles: terrain_handles::State,
    surface_sample: Option<surface::SurfaceSample>,
    recovery: recovery::State,
    recent_maps: recent::State,
    checks: checks::State,
    material_search: material_search::State,
    context: context::State,
    clip_tool: bool,
    clip_epoch: u64,
    clip_points: Option<clip_points::Draft>,
    clip_handles: clip_drag::Options,
    clip_drag: Option<clip_drag::Drag>,
    transform_tool: TransformTool,
    transform_drag: Option<TransformDrag>,
    area_preview: Option<AreaPreview>,
    pointfile: pointfiles::State,
    volume: volume::State,
    connection_draft: Option<ConnectionDraft>,
    connection_serial: u64,
    rename_draft: Option<RenameDraft>,
    rename_serial: u64,
    property_draft: Option<PropertyDraft>,
    management_serial: u64,
    visibility_restore: Option<management::VisibilityRestore>,
    area_rule: map_core::region::Match,
    area_mode: SelectMode,
    interaction_epoch: u64,
    material_index: Option<Vec<String>>,
    material_reload: Option<material_assets::Report>,
    material_favorites: BTreeSet<String>,
    favorite_preferences: favorites::State,
    material_recent: VecDeque<String>,
    camera: Camera,
    view: ViewOptions,
    spatial: spatial::Settings,
    saved_cameras: [Option<Camera>; 4],
    block_preview: Option<(DVec3, DVec3)>,
    brush_epoch: u64,
    view_serial: u64,
    materials: MaterialLibrary,
    log: VecDeque<LogEntry>,
    /// Bumped whenever what the viewport shows changes, except the camera.
    scene_serial: u64,
    /// Bumped when the camera is set from outside the viewport.
    camera_serial: u64,
    log_serial: u64,
    document_generation: u64,
    compiling: bool,
    compilation: compilation::State,
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

pub struct LaunchRequest {
    install: game::Install,
    bsp: PathBuf,
}

impl LaunchRequest {
    pub fn run(self) -> Result<game::Run, String> {
        game::Run::launch(&self.install, &self.bsp)
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
            geometry: previews::State::default(),
            compile_region: compile_region::State::default(),
            overlay: overlay::State::default(),
            groups: groups::State::default(),
            layers: layers::State::default(),
            prefabs: prefabs::State::default(),
            game: Arc::new(game),
            game_error,
            document: Document::empty(),
            path: None,
            selection: BTreeSet::new(),
            face_selection: BTreeSet::new(),
            face_mode: false,
            face_preview_serial: 0,
            terrain_triangle_highlight: None,
            terrain_triangle_serial: 0,
            terrain_strokes: terrain_strokes::State::default(),
            models: std::cell::RefCell::default(),
            model_browser: model_browser::State::default(),
            sounds: sounds::State::default(),
            placement: placement::State::default(),
            model_attachments: model_attachments::State::default(),
            terrain_handles: terrain_handles::State::default(),
            surface_sample: None,
            recovery: recovery::State::default(),
            recent_maps: recent::State::default(),
            checks: checks::State::default(),
            material_search: material_search::State::default(),
            context: context::State::default(),
            clip_tool: false,
            clip_epoch: 0,
            clip_points: None,
            clip_handles: clip_drag::Options::default(),
            clip_drag: None,
            transform_tool: TransformTool::default(),
            transform_drag: None,
            area_preview: None,
            pointfile: pointfiles::State::default(),
            volume: volume::State::default(),
            connection_draft: None,
            connection_serial: 0,
            rename_draft: None,
            rename_serial: 0,
            property_draft: None,
            management_serial: 0,
            visibility_restore: None,
            area_rule: map_core::region::Match::Intersecting,
            area_mode: SelectMode::Replace,
            interaction_epoch: 0,
            material_index: None,
            material_reload: None,
            material_favorites: BTreeSet::new(),
            favorite_preferences: favorites::State::default(),
            material_recent: VecDeque::new(),
            camera: Camera::looking_at(
                DVec3::new(-512.0, -512.0, 384.0).as_vec3(),
                glam::Vec3::ZERO,
            ),
            view: ViewOptions::default(),
            spatial: spatial::Settings::default(),
            saved_cameras: [None; 4],
            block_preview: None,
            brush_epoch: 0,
            view_serial: 1,
            materials: MaterialLibrary::default(),
            log: VecDeque::new(),
            scene_serial: 1,
            camera_serial: 1,
            log_serial: 0,
            document_generation: 0,
            compiling: false,
            compilation: compilation::State::default(),
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

    pub fn document_generation(&self) -> u64 {
        self.document_generation
    }

    pub fn entity_inspector(&self, id: u32) -> Result<inspector::Inspector, String> {
        let entity = self
            .document
            .entity(id)
            .ok_or_else(|| format!("no entity:{id}"))?;
        let class = entity
            .classname()
            .and_then(|name| self.definitions.class(name));
        Ok(inspector::inspect(entity, class))
    }

    pub fn set_entity_color(
        &mut self,
        actor: Actor,
        id: u32,
        key: &str,
        color: [u8; 3],
        intensity: Option<f64>,
    ) -> Result<(), String> {
        let inspector = self.entity_inspector(id)?;
        let field = inspector
            .properties
            .iter()
            .find(|field| field.key.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("unknown color property {key}"))?;
        if !matches!(field.kind.as_str(), "color255" | "color1") {
            return Err(format!("{key} is not a color property"));
        }
        if intensity.is_some_and(|value| !value.is_finite() || value < 0.0) {
            return Err("brightness must be a finite, nonnegative number".into());
        }
        if field.kind == "color1" && intensity.is_some() {
            return Err("normalized colors have no brightness component".into());
        }
        let render_color = key.eq_ignore_ascii_case("rendercolor") && field.kind == "color255";
        if render_color && intensity.is_some() {
            return Err("Model tint has no light brightness component".into());
        }
        let scale = if field.kind == "color1" { 255.0 } else { 1.0 };
        let mut values: Vec<String> = color
            .iter()
            .map(|value| (f64::from(*value) / scale).to_string())
            .collect();
        if render_color {
            let components = field.value.split_whitespace().collect::<Vec<_>>();
            if components.len() == 4 {
                values.push(components[3].to_owned());
            }
        } else if let Some(intensity) = intensity.or(field.intensity) {
            values.push(intensity.to_string());
        }
        self.set_entity_properties(actor, id, &[(key.into(), Some(values.join(" ")))])
    }

    pub fn apply_material(
        &mut self,
        actor: Actor,
        targets: &[map_core::edit::FaceTarget],
        material: &str,
    ) -> Result<usize, String> {
        let changed = self.edit(actor, &format!("apply {material}"), |tx| {
            map_core::edit::set_material(tx, targets, material)
        })?;
        if changed > 0 {
            self.remember_material(material);
        }
        Ok(changed)
    }

    fn remember_material(&mut self, material: &str) {
        self.view_serial += 1;
        let material = crate::material_favorites::canonical_name(material)
            .unwrap_or_else(|_| material.trim().to_ascii_lowercase());
        self.material_recent.retain(|name| *name != material);
        self.material_recent.push_front(material);
        self.material_recent.truncate(32);
    }

    pub fn material_catalog(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::materials::Catalog, String> {
        if self.material_index.is_none() {
            self.material_index = Some(self.search_materials("", usize::MAX)?);
        }
        let used = self
            .document
            .vmf()
            .solids()
            .flat_map(|solid| &solid.sides)
            .map(|side| {
                crate::material_favorites::canonical_name(&side.material)
                    .unwrap_or_else(|_| side.material.to_ascii_lowercase())
            })
            .collect();
        let mut catalog = crate::materials::browse(
            self.material_index.as_ref().unwrap(),
            &used,
            &self.material_favorites,
            &self.material_recent,
            query,
            group,
            (offset, limit),
        )?;
        catalog.favorites = self.material_favorite_status();
        Ok(catalog)
    }

    pub fn face_mode(&self) -> bool {
        self.face_mode
    }

    pub fn set_face_mode(&mut self, enabled: bool) {
        self.clear_component_edit();
        if self.face_mode != enabled {
            self.face_mode = enabled;
            if enabled && self.clip_tool {
                self.clip_tool = false;
                self.view_serial += 1;
            }
            if enabled && self.spatial.draw_block {
                self.spatial.draw_block = false;
                self.view_serial += 1;
            }
            self.selection.clear();
            self.face_selection.clear();
            self.changed();
        }
    }

    pub fn selected_faces(&self) -> Vec<FaceId> {
        self.face_selection.iter().copied().collect()
    }

    pub fn select_faces(&mut self, faces: &[FaceId], mode: SelectMode) -> Result<(), String> {
        for face in faces {
            if surface::side(&self.document, *face).is_none() {
                return Err(format!("solid {} has no side {}", face.solid, face.side));
            }
        }
        let mut next = self.face_selection.clone();
        match mode {
            SelectMode::Replace => next = faces.iter().copied().collect(),
            SelectMode::Add => next.extend(faces),
            SelectMode::Remove => faces.iter().for_each(|face| {
                next.remove(face);
            }),
            SelectMode::Toggle => faces.iter().for_each(|face| {
                if !next.remove(face) {
                    next.insert(*face);
                }
            }),
        }
        if next != self.face_selection || !self.face_mode || !self.selection.is_empty() {
            self.face_mode = true;
            if self.clip_tool {
                self.clip_tool = false;
                self.view_serial += 1;
            }
            if self.spatial.draw_block {
                self.spatial.draw_block = false;
                self.view_serial += 1;
            }
            self.selection.clear();
            self.face_selection = next;
            self.changed();
        }
        Ok(())
    }

    pub fn material_targets(&self) -> Vec<map_core::edit::FaceTarget> {
        if self.face_mode {
            self.face_selection
                .iter()
                .map(|face| map_core::edit::FaceTarget::Side {
                    solid: face.solid,
                    side: face.side,
                })
                .collect()
        } else {
            self.selected_ids()
                .into_iter()
                .map(map_core::edit::FaceTarget::Object)
                .collect()
        }
    }

    pub fn has_face_preview(&self) -> bool {
        self.geometry.face_preview.is_some()
    }

    pub fn face_preview_token(&self) -> Option<u64> {
        self.geometry
            .face_preview
            .as_ref()
            .map(|preview| preview.token)
    }

    pub fn check_face_preview_token(&self, token: Option<u64>) -> Result<(), String> {
        if token.is_some() && self.face_preview_token() != token {
            return Err("Face preview was closed or replaced".into());
        }
        Ok(())
    }

    pub fn preview_side(&self, face: FaceId) -> Option<&source_formats::vmf::Side> {
        surface::side(&self.geometry.face_preview.as_ref()?.document, face)
    }

    pub fn preview_faces(&self) -> Vec<FaceId> {
        self.geometry
            .face_preview
            .as_ref()
            .map(|preview| preview.edits.iter().map(|(face, _)| *face).collect())
            .unwrap_or_default()
    }

    pub fn face_preview_document(&self) -> Option<&Document> {
        self.geometry
            .face_preview
            .as_ref()
            .map(|preview| &preview.document)
    }

    pub fn cancel_face_preview(&mut self) {
        if self.geometry.face_preview.take().is_some() {
            self.scene_serial += 1;
        }
    }

    pub fn edit_faces(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        patch: SurfaceEdit,
        fit: bool,
        preview: bool,
    ) -> Result<usize, String> {
        if faces.is_empty() {
            return Err("select at least one face".into());
        }
        let mut edits = Vec::new();
        for face in faces.iter().copied().collect::<BTreeSet<_>>() {
            let side = surface::side(&self.document, face)
                .ok_or_else(|| format!("solid {} has no side {}", face.solid, face.side))?;
            let name = patch
                .material
                .as_deref()
                .unwrap_or(&side.material)
                .to_owned();
            let mut patch = patch.clone();
            if fit || patch.justify_u.is_some() || patch.justify_v.is_some() {
                let material = self.material_preview(&name)?;
                if material.missing {
                    return Err(format!(
                        "cannot place {name}: texture dimensions unavailable"
                    ));
                }
                if fit {
                    patch.fit = Some([material.width, material.height]);
                }
                if let Some((placement, _)) = patch.justify_u {
                    patch.justify_u = Some((placement, material.width));
                }
                if let Some((placement, _)) = patch.justify_v {
                    patch.justify_v = Some((placement, material.height));
                }
            }
            edits.push((face, patch));
        }
        self.preview_or_apply_faces(actor, edits, preview)
    }

    fn preview_or_apply_faces(
        &mut self,
        actor: Actor,
        edits: Vec<(FaceId, SurfaceEdit)>,
        preview: bool,
    ) -> Result<usize, String> {
        if preview {
            let mut document = self.document.preview_snapshot();
            let changed = document
                .edit("preview face settings", |tx| {
                    edits.iter().try_fold(0, |count, (face, patch)| {
                        surface::edit_faces(tx, &[*face], patch).map(|changed| count + changed)
                    })
                })
                .map_err(|error| error.to_string())?;
            self.clear_geometry_previews_except(&[previews::Kind::Face, previews::Kind::Paint]);

            self.clear_volume_selection();
            self.clip_tool = false;

            self.cancel_area_selection();
            self.view_serial += 1;
            self.face_preview_serial += 1;
            self.geometry.face_preview = Some(FacePreview {
                token: self.face_preview_serial,
                document,
                edits,
                terrain_mask_token: None,
            });
            self.scene_serial += 1;
            Ok(changed)
        } else {
            self.apply_face_edits(actor, edits)
        }
    }

    fn apply_face_edits(
        &mut self,
        actor: Actor,
        edits: Vec<(FaceId, SurfaceEdit)>,
    ) -> Result<usize, String> {
        let label = edits
            .iter()
            .find_map(|(_, patch)| patch.terrain.as_ref())
            .map(|operation| match operation {
                map_core::displacement::Operation::Resolution(_) => "change terrain resolution",
                map_core::displacement::Operation::Sewn(_) => "sew terrain edges",
                map_core::displacement::Operation::Subdivided(_) => "subdivide terrain base cage",
                map_core::displacement::Operation::Collision(_) => "edit terrain collision",
                map_core::displacement::Operation::Smoothing(_) => "edit terrain border smoothing",
                map_core::displacement::Operation::TriangleTags { .. } => {
                    "edit terrain triangle tags"
                }
                map_core::displacement::Operation::Sculpt { .. } => "sculpt terrain",
                map_core::displacement::Operation::Masked { operation, .. } => {
                    match operation.as_ref() {
                        map_core::displacement::Operation::Sculpt { .. } => {
                            "sculpt selected terrain vertices"
                        }
                        map_core::displacement::Operation::Stroke { .. } => {
                            "edit selected terrain stroke"
                        }
                        _ => "blend selected terrain vertices",
                    }
                }
                map_core::displacement::Operation::Blend { .. } => "edit terrain blend",
                map_core::displacement::Operation::Stroke { .. } => "edit terrain brush stroke",
                map_core::displacement::Operation::VertexMove { .. } => {
                    "move selected terrain vertices"
                }
            })
            .unwrap_or("edit face settings");
        let changed = self.edit(actor, label, |tx| {
            edits.iter().try_fold(0, |count, (face, patch)| {
                surface::edit_faces(tx, &[*face], patch).map(|changed| count + changed)
            })
        })?;
        self.cancel_face_preview();
        if changed > 0 {
            for (_, patch) in edits {
                if let Some(material) = patch.material {
                    self.remember_material(&material);
                }
            }
        }
        Ok(changed)
    }

    pub fn commit_face_preview(&mut self, actor: Actor) -> Result<usize, String> {
        let preview = self
            .geometry
            .face_preview
            .take()
            .ok_or("no face preview to apply")?;
        self.scene_serial += 1;
        let changed = self.apply_face_edits(actor, preview.edits)?;
        if let Some(token) = preview.terrain_mask_token {
            self.refresh_terrain_mask_after_commit(token);
        }
        Ok(changed)
    }

    pub fn set_entity_component(
        &mut self,
        actor: Actor,
        id: u32,
        key: &str,
        index: usize,
        value: &str,
    ) -> Result<(), String> {
        let inspector = self.entity_inspector(id)?;
        let field = inspector
            .properties
            .iter()
            .find(|field| field.key.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("unknown property {key}"))?;
        if !matches!(
            field.control,
            inspector::Control::Vector | inspector::Control::Color
        ) {
            return Err(format!("{key} has no editable components"));
        }
        let mut components: Vec<String> =
            field.value.split_whitespace().map(str::to_owned).collect();
        let component = components
            .get_mut(index)
            .ok_or_else(|| format!("invalid component {index} for {key}"))?;
        let number = value.parse::<f64>().map_err(|error| error.to_string())?;
        if !number.is_finite() {
            return Err("component must be finite".into());
        }
        if field.control == inspector::Control::Color {
            let max = if field.kind == "color1" { 1.0 } else { 255.0 };
            if number < 0.0
                || index < 3 && (number > max || field.kind == "color255" && number.fract() != 0.0)
            {
                return Err("color component is out of range".into());
            }
        }
        *component = value.into();
        self.set_entity_properties(actor, id, &[(key.into(), Some(components.join(" ")))])
    }

    pub fn material_preview(&self, name: &str) -> Result<MaterialPreview, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("material name must not be empty".into());
        }
        let loaded;
        let material = if let Some(material) = self.materials.find(name) {
            material
        } else {
            loaded = render::materials::MaterialInfo::load(&self.game, name);
            &loaded
        };
        if let Some(error) = &material.error {
            return Err(error.clone());
        }
        let image = material
            .base
            .as_ref()
            .map(|texture| texture.decode_rgba8(texture.mip_for_size(256), 0, 0))
            .transpose()
            .map_err(|error| error.to_string())?;
        Ok(MaterialPreview {
            name: material.name.clone(),
            width: material.width,
            height: material.height,
            missing: material.missing,
            tool: material.tool,
            image,
        })
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
        let position = self
            .camera
            .plane_point((0.0, 0.0), 1.0, self.spatial.elevation as f32)
            .unwrap_or(self.camera.position + self.camera.forward() * 128.0)
            .as_dvec3();
        self.spatial.snap_point(position)
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
                    crate::identity::id_string(*id)
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
        self.overlay.context_changed();
        self.refresh_scene();
    }

    fn refresh_scene(&mut self) {
        self.clear_geometry_previews_except(&[previews::Kind::Paint]);

        self.cancel_block_preview();

        self.clear_volume_selection();

        self.cancel_area_selection();
        self.view_serial += 1;
        if self.clip_tool && !self.can_clip_selection() {
            self.clip_tool = false;
            self.view_serial += 1;
        }
        self.scene_serial += 1;
    }

    fn ensure_discardable(&self, discard_changes: bool) -> Result<(), String> {
        if !discard_changes {
            self.ensure_partitions_resolved()?;
        }
        if self.document.is_dirty() && !discard_changes {
            return Err(format!(
                "{} has unsaved changes; save first or pass discard_changes",
                self.title()
            ));
        }
        Ok(())
    }

    fn replace_document(&mut self, document: Document, path: Option<PathBuf>) {
        self.groups.draft = None;
        self.layers.clear_document_draft();
        self.surface_sample = None;
        self.material_search.report = None;
        self.context.draft = None;
        self.context.focused = None;
        self.context.serial += 1;
        self.property_draft = None;
        self.management_serial += 1;
        self.visibility_restore = None;
        self.cancel_connection_edit();
        self.cancel_entity_rename();
        self.camera = Camera::from_vmf(document.vmf()).unwrap_or_else(|| {
            let bounds = self.object_bounds_in(&document, &document.objects().collect::<Vec<_>>());
            bounds.map_or(self.camera, |(lo, hi)| {
                Camera::overview(lo.as_vec3(), hi.as_vec3())
            })
        });
        self.document = document;
        self.saved_cameras = [None; 4];
        self.document_generation += 1;
        self.compiled = None;
        self.leak = None;
        self.reset_pointfile();
        self.path = path;
        self.reset_recovery_document();
        self.selection.clear();
        self.face_selection.clear();
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
        let document =
            Document::try_new(vmf).map_err(|error| format!("{}: {error}", path.display()))?;
        document
            .context()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let message = format!(
            "opened {} ({} brushes, {} entities) in {:.0?}",
            path.display(),
            document.vmf().world.solids.len(),
            document.vmf().entities.len(),
            start.elapsed()
        );
        self.replace_document(document, Some(path.to_owned()));
        self.restore_recent_map_context(path);
        self.remember_recent_map(path);
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
        self.ensure_partitions_resolved()?;
        let path = path
            .map(Path::to_owned)
            .or_else(|| self.path.clone())
            .ok_or("the map has no file name yet; give a path")?;
        let text = self
            .document
            .snapshot_for_save()
            .write(&WriteOptions::default());
        let backup = self.backup_before_save(&path);
        if let Err(error) = crate::recovery::atomic_write(&path, text.as_bytes()) {
            let _ = self.finish_save_backup(backup.as_ref().ok().and_then(Option::as_ref), false);
            return Err(error);
        }
        let retention_error = self
            .finish_save_backup(backup.as_ref().ok().and_then(Option::as_ref), true)
            .err();
        let backup_error = backup.err().or(retention_error);
        self.document.bump_map_version();
        self.document.mark_saved();
        self.path = Some(path.clone());
        self.update_recovery_path();
        self.remember_saved_map(&path);
        self.changed();
        self.log(
            actor,
            match backup_error {
                Some(error) => format!(
                    "saved {}, but the previous-file backup failed: {error}",
                    path.display()
                ),
                None => format!("saved {}", path.display()),
            },
        );
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
        self.face_selection
            .retain(|face| surface::side(doc, *face).is_some());
    }

    pub fn selection(&self) -> &BTreeSet<ObjectId> {
        &self.selection
    }

    pub fn selected_ids(&self) -> Vec<ObjectId> {
        self.selection.iter().copied().collect()
    }

    pub fn select(&mut self, ids: &[ObjectId], mode: SelectMode) -> Result<(), String> {
        let units = ids.iter().map(|&id| vec![id]).collect::<Vec<_>>();
        self.select_object_units(&units, mode)
    }

    pub fn select_grouped(&mut self, ids: &[ObjectId], mode: SelectMode) -> Result<(), String> {
        if !self.view.tool_textures {
            self.hidden_tool_brushes();
        }
        let units = self.group_selection_units(ids);
        self.select_object_units(&units, mode)
    }

    fn select_exact(&mut self, ids: &[ObjectId], mode: SelectMode) -> Result<(), String> {
        self.select_object_units(&[ids.to_vec()], mode)
    }

    fn select_object_units(
        &mut self,
        units: &[Vec<ObjectId>],
        mode: SelectMode,
    ) -> Result<(), String> {
        let ids: BTreeSet<_> = units.iter().flatten().copied().collect();
        if let Some(missing) = ids.iter().find(|&&id| !self.document.contains(id)) {
            return Err(format!("{missing} does not exist"));
        }
        self.clear_component_edit();
        let before = self.selection.clone();
        let was_face_mode = self.face_mode;
        self.face_mode = false;
        self.face_selection.clear();
        match mode {
            SelectMode::Replace => self.selection = ids.clone(),
            SelectMode::Add => self.selection.extend(&ids),
            SelectMode::Remove => ids.iter().for_each(|id| {
                self.selection.remove(id);
            }),
            SelectMode::Toggle => units.iter().for_each(|unit| {
                if unit.iter().all(|id| self.selection.contains(id)) {
                    for id in unit {
                        self.selection.remove(id);
                    }
                } else {
                    self.selection.extend(unit);
                }
            }),
        }
        if self.selection != before || was_face_mode {
            self.changed();
        }
        Ok(())
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// Moves the viewport camera from outside the viewport.
    pub fn set_camera(&mut self, camera: Camera) {
        self.saved_cameras[self.camera.projection.index()] = Some(self.camera);
        self.camera = camera;
        self.cancel_block_preview();
        self.clip_epoch += 1;
        self.interaction_camera_changed();
        self.camera_serial += 1;
    }

    /// Records the viewport's own camera movement.
    pub fn publish_camera(&mut self, camera: Camera) {
        if self.camera != camera {
            self.interaction_camera_changed();
        }
        self.camera = camera;
    }

    pub fn skybox_preview(&mut self) -> Option<Arc<render::sky::Skybox>> {
        let name = self.document.vmf().world.properties.get("skyname")?.trim();
        if name.is_empty() {
            return None;
        }
        Some(self.materials.skybox(&self.game, name))
    }

    pub fn view(&self) -> ViewOptions {
        ViewOptions {
            grid_size: self.spatial.grid_size as f32,
            grid_visible: self.spatial.grid_visible,
            grid_elevation: self.spatial.elevation as f32,
            block_preview: self
                .block_preview
                .map(|(mins, maxs)| [mins.as_vec3(), maxs.as_vec3()]),
            clip_preview: self
                .geometry
                .clip_preview
                .as_ref()
                .map(|preview| render::ClipGuide {
                    normal: preview.plane.normal.as_vec3(),
                    distance: preview.plane.dist as f32,
                    bounds: preview.bounds.map(|point| point.as_vec3()),
                }),
            transform_handles: self
                .terrain_move_guide()
                .or_else(|| self.clip_guide())
                .or_else(|| self.component_guide())
                .or_else(|| self.transform_guide()),
            area_preview: self.area_preview.as_ref().map(|preview| render::AreaGuide {
                corners: [
                    preview.start.ndc().as_vec2(),
                    Pointer {
                        position: preview.end,
                        ..preview.start
                    }
                    .ndc()
                    .as_vec2(),
                ],
                contained: preview.rule == map_core::region::Match::Contained,
                camera: preview.camera,
                aspect: preview.start.size.0 as f32 / preview.start.size.1 as f32,
            }),
            paint_preview: self.paint_stroke().map(|stroke| render::PaintGuide {
                center: stroke.cursor.as_vec2(),
                radius: stroke.spec.radius as f32,
                size: stroke.spec.pointer.size,
                camera: stroke.spec.camera,
            }),
            ..self.view
        }
    }

    pub fn set_view(&mut self, view: ViewOptions) {
        self.restore_clip_drag();
        self.clear_component_drag();
        if self.view.tool_textures != view.tool_textures {
            if self.component_picking().is_some() || !self.component_selection().is_empty() {
                self.scene_serial += 1;
            }
            self.cancel_area_selection();
            self.clear_volume_selection();
        }
        self.view = view;
        self.refresh_component_origin();
        self.view_serial += 1;
    }

    pub fn view_serial(&self) -> u64 {
        self.view_serial
    }

    pub fn camera_for_projection(&self, projection: Projection) -> Camera {
        if projection == self.camera.projection {
            return self.camera;
        }
        if let Some(camera) = self.saved_cameras[projection.index()] {
            return camera;
        }
        let bounds = self.selection_bounds().or_else(|| {
            self.object_bounds_in(&self.document, &self.document.objects().collect::<Vec<_>>())
        });
        let (mins, maxs) = bounds.unwrap_or((DVec3::splat(-384.0), DVec3::splat(384.0)));
        if projection == Projection::Perspective {
            Camera::overview(mins.as_vec3(), maxs.as_vec3())
        } else {
            Camera::planar(
                projection,
                ((mins + maxs) * 0.5).as_vec3(),
                ((maxs - mins).max_element() as f32 * 1.2).max(512.0),
            )
        }
    }

    pub fn set_projection(&mut self, actor: Actor, projection: Projection) {
        self.set_camera(self.camera_for_projection(projection));
        self.log(actor, format!("{} view", projection.name()));
    }

    pub fn frame_selection(&mut self, actor: Actor, aspect: f32) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let bounds = if self.selection.is_empty() && self.face_selection.is_empty() {
            let document = self.render_document();
            self.object_bounds_in(document, &document.objects().collect::<Vec<_>>())
        } else {
            self.selection_bounds_in(self.render_document())
        };
        let (mins, maxs) = bounds.ok_or("No objects to frame")?;
        self.set_camera(
            self.camera
                .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect.max(0.01)),
        );
        self.log(actor, "framed objects");
        Ok(())
    }

    pub fn selection_bounds(&self) -> Option<(DVec3, DVec3)> {
        self.selection_bounds_in(&self.document)
    }

    fn selection_bounds_in(&self, document: &Document) -> Option<(DVec3, DVec3)> {
        if !self.face_mode {
            return self.object_bounds_in(document, &self.selected_ids());
        }
        query::union(self.face_selection.iter().filter_map(|selected| {
            let solid = surface::solid(document, selected.solid)?;
            query::face_bounds(solid, selected.side)
        }))
    }

    pub fn spatial(&self) -> spatial::Settings {
        self.spatial
    }

    pub fn set_spatial(&mut self, actor: Actor, update: spatial::Update) -> Result<(), String> {
        let settings = self.spatial.updated(update)?;
        if settings == self.spatial {
            return Ok(());
        }
        self.spatial = settings;
        if settings.draw_block {
            self.geometry.paint.enabled = false;
            self.clip_tool = false;
            self.set_face_mode(false);
        }
        if settings.draw_block && self.camera.projection == Projection::Perspective {
            self.set_projection(actor, Projection::Top);
        }
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[previews::Kind::Face, previews::Kind::Paint]);

        self.clear_volume_selection();

        self.cancel_area_selection();
        self.view_serial += 1;
        self.log(
            actor,
            format!(
                "grid {} · snapping {} · brush depth {}",
                settings.grid_size,
                if settings.snap { "on" } else { "off" },
                settings.depth
            ),
        );
        Ok(())
    }

    pub fn block_preview(&self) -> Option<(DVec3, DVec3)> {
        self.block_preview
    }

    pub fn preview_block(&mut self, mins: DVec3, maxs: DVec3) -> Result<(), String> {
        spatial::validate_bounds(mins, maxs)?;
        self.clear_geometry_previews_except(&[
            previews::Kind::Clip,
            previews::Kind::Transform,
            previews::Kind::Face,
            previews::Kind::Paint,
        ]);

        self.clear_volume_selection();
        if self.block_preview != Some((mins, maxs)) {
            self.block_preview = Some((mins, maxs));
            self.view_serial += 1;
        }
        Ok(())
    }

    pub fn cancel_block_preview(&mut self) {
        self.brush_epoch += 1;
        self.clear_block_preview();
    }

    pub fn clear_block_preview(&mut self) {
        if self.block_preview.take().is_some() {
            self.view_serial += 1;
        }
    }

    pub fn brush_epoch(&self) -> u64 {
        self.brush_epoch
    }

    pub fn drawn_block_bounds(
        &self,
        projection: Projection,
        start: DVec2,
        end: DVec2,
        elevation: Option<f64>,
        depth: Option<f64>,
    ) -> Result<(DVec3, DVec3), String> {
        let settings = self.spatial.updated(spatial::Update {
            elevation,
            depth,
            ..spatial::Update::default()
        })?;
        settings.block_bounds(projection, start, end)
    }

    pub fn create_block(
        &mut self,
        actor: Actor,
        mins: DVec3,
        maxs: DVec3,
        material: &str,
        snap: bool,
    ) -> Result<ObjectId, String> {
        let (mins, maxs) = if snap {
            (self.spatial.snap_point(mins), self.spatial.snap_point(maxs))
        } else {
            (mins, maxs)
        };
        spatial::validate_bounds(mins, maxs)?;
        let id = self.edit(actor, "create block", |tx| {
            map_core::edit::create_block(tx, mins, maxs, material)
        })?;
        self.select(&[id], SelectMode::Replace)?;
        Ok(id)
    }

    pub fn selection_pivot(&self, ids: &[ObjectId]) -> Result<DVec3, String> {
        if ids.is_empty() {
            return Err("Select objects to transform".into());
        }
        for &id in ids {
            if !self.document.contains(id) {
                return Err(format!("{id} does not exist"));
            }
        }
        let (mins, maxs) = self
            .object_bounds_in(&self.document, ids)
            .ok_or("Selected objects have no valid bounds")?;
        Ok((mins + maxs) * 0.5)
    }

    pub fn transform_objects(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        mut transform: map_core::edit::Transform,
        pivot: Option<DVec3>,
    ) -> Result<DVec3, String> {
        let center = self.selection_pivot(ids)?;
        transform.pivot = pivot.unwrap_or(center);
        let ids: Vec<_> = ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let operation = if transform.rotation == DVec3::ZERO && transform.scale == DVec3::ONE {
            "move"
        } else if transform.translation == DVec3::ZERO && transform.scale == DVec3::ONE {
            "rotate"
        } else if transform.translation == DVec3::ZERO && transform.rotation == DVec3::ZERO {
            "scale"
        } else {
            "transform"
        };
        self.edit(actor, &format!("{operation} {} objects", ids.len()), |tx| {
            map_core::edit::transform(tx, &ids, transform)
        })?;
        Ok(transform.pivot)
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

    pub fn launch_readiness(&self) -> Result<&Path, String> {
        self.ensure_partitions_resolved()?;
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
        if compiled.options != self.compile_options() {
            return Err("Build settings changed since compilation; compile again or restore the previous settings before launching".into());
        }
        if !compiled.path.is_file() {
            return Err("the compiled BSP is missing; compile the map again".into());
        }
        game::Install::from_game(&self.game)?;
        Ok(&compiled.path)
    }

    pub fn begin_launch(&mut self, actor: Actor) -> Result<LaunchRequest, String> {
        let bsp = self.launch_readiness()?.to_owned();
        let request = LaunchRequest {
            install: game::Install::from_game(&self.game)?,
            bsp,
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
        self.shutdown_sound_preview();
        self.cancel_compile_on_shutdown();
        self.game_run = None;
    }

    /// The nearest visible object along a ray. Brushes made only of tool
    /// materials can't be picked while tool textures are hidden.
    pub fn pick(&mut self, ray: &Ray) -> Option<Hit> {
        let hidden = self.hidden_tool_brushes();
        let document = self.render_document();
        if self.face_mode {
            query::pick_face(document, ray, |s| !hidden.contains(&s.id))
        } else {
            let models = self.model_instances(document);
            let ids = models
                .instances
                .iter()
                .filter(|instance| instance.has_geometry())
                .map(|instance| instance.id)
                .collect::<HashSet<_>>();
            let ordinary = query::pick_with_entity_filter(
                document,
                ray,
                |solid| !hidden.contains(&solid.id),
                |entity| !ids.contains(&ObjectId::Entity(entity.id)),
            );
            ordinary
                .into_iter()
                .chain(
                    models
                        .instances
                        .iter()
                        .filter_map(|instance| instance.pick(ray)),
                )
                .min_by(|a, b| a.distance.total_cmp(&b.distance))
        }
    }

    fn hidden_tool_brushes(&mut self) -> HashSet<u32> {
        let document = self.geometry.document(&self.document);
        let mut hidden = HashSet::new();
        if !self.view.tool_textures {
            let (materials, game) = (&mut self.materials, &self.game);
            for solid in document.vmf().solids() {
                let all_tools = solid.sides.iter().all(|side| {
                    let id = materials.load(game, &side.material);
                    materials.get(id).tool
                });
                if all_tools {
                    hidden.insert(solid.id);
                }
            }
        }
        hidden
    }

    pub fn render_document(&self) -> &Document {
        self.geometry.document(&self.document)
    }

    pub fn build_scene(&mut self) -> SceneMesh {
        let selected: HashSet<ObjectId> = self.geometry.clip_preview.as_ref().map_or_else(
            || self.selection.iter().copied().collect(),
            |preview| preview.result.objects.iter().copied().collect(),
        );
        let selected = self
            .geometry
            .primitives
            .preview
            .as_ref()
            .map_or(selected, |preview| preview.ids.iter().copied().collect());
        let selected = self
            .geometry
            .repeat
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects().into_iter().collect()
            });
        let selected = self
            .geometry
            .clipboard
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects.iter().copied().collect()
            });
        let selected = self
            .area_preview
            .as_ref()
            .map_or(selected.clone(), |preview| {
                self.grouped_preview_selection(&selected, &preview.matches.objects, preview.mode)
            });
        let selected_faces: HashSet<FaceId> = self.face_selection.iter().copied().collect();
        let selected_faces = self.area_preview.as_ref().map_or_else(
            || selected_faces.clone(),
            |preview| {
                manipulation::combined_selection(
                    &selected_faces,
                    &preview.matches.faces,
                    preview.mode,
                )
            },
        );
        let (selected, selected_faces) = if let Some(preview) = self.volume_preview() {
            if preview.spec.faces {
                (
                    HashSet::new(),
                    manipulation::combined_selection(
                        &selected_faces,
                        &preview.matches.faces,
                        preview.spec.mode,
                    ),
                )
            } else {
                (
                    self.grouped_preview_selection(
                        &selected,
                        &preview.matches.objects,
                        preview.spec.mode,
                    ),
                    HashSet::new(),
                )
            }
        } else {
            (selected, selected_faces)
        };
        let (selected, selected_faces) = if let Some(stroke) = self.paint_stroke() {
            if stroke.spec.faces {
                (
                    HashSet::new(),
                    manipulation::combined_selection(
                        &selected_faces,
                        &stroke.faces.iter().copied().collect::<Vec<_>>(),
                        stroke.spec.mode,
                    ),
                )
            } else {
                (
                    self.grouped_preview_selection(
                        &selected,
                        &stroke.objects.iter().copied().collect::<Vec<_>>(),
                        stroke.spec.mode,
                    ),
                    HashSet::new(),
                )
            }
        } else {
            (selected, selected_faces)
        };
        let selected = self
            .geometry
            .merging
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                [preview.result.object].into_iter().collect()
            });
        let selected = self
            .geometry
            .carving
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects.iter().copied().collect()
            });
        let selected = self
            .geometry
            .hollowing
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects.iter().copied().collect()
            });
        let selected_faces = if self.geometry.primitives.preview.is_some() {
            HashSet::new()
        } else {
            selected_faces
        };
        let overlays = self.partition_face_overlays();
        let document = self.geometry.document(&self.document);
        let selected = if self.geometry.components.selection.is_empty()
            && self.component_picking().is_none()
        {
            selected
        } else {
            HashSet::new()
        };
        let models = self.model_instances(document);
        let mut mesh = SceneMesh::build_with_models(
            document.vmf(),
            &self.game,
            &mut self.materials,
            &selected,
            &selected_faces,
            &overlays,
            &models.instances,
        );
        if let Some(points) = self.pointfile_points() {
            mesh.set_leak_path(points);
        }
        self.append_context_guides(&mut mesh);
        self.append_model_attachment_guide(&mut mesh);
        self.append_surface_placement_guide(&mut mesh);
        self.append_volume_guide(&mut mesh);
        self.append_compile_region_guide(&mut mesh);
        self.append_overlay_guide(&mut mesh);
        self.append_component_guides(&mut mesh);
        self.append_terrain_triangle_guides(&mut mesh);
        self.append_terrain_stroke_guides(&mut mesh);
        self.append_clip_points(&mut mesh);
        mesh
    }

    /// Material names (as used in maps) containing `query`, case-insensitive.
    pub fn search_materials(&self, query: &str, limit: usize) -> Result<Vec<String>, String> {
        let query = query.to_ascii_lowercase();
        Ok(self
            .game
            .list("materials", ".vmt")
            .map_err(|error| format!("Material catalog: {error}"))?
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
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_assets::room;
    #[test]
    fn sky_preview_tracks_world_settings_and_undo_without_view_history() {
        let mut session = Session::new(Err("no mounted assets".into()));
        let before = session.document.vmf().clone();
        let original = session.skybox_preview().unwrap();
        assert!(!original.ready());
        let serial = session.view_serial();
        session.set_view(ViewOptions {
            sky_preview: true,
            ..session.view()
        });
        assert!(session.view().sky_preview);
        assert!(session.view_serial() > serial);
        assert_eq!(&before, session.document.vmf());
        assert_eq!(session.document.undo_labels().count(), 0);
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .update_property_edit(
                token,
                vec![PropertyChange::Set {
                    key: "skyname".into(),
                    value: "other_sky".into(),
                }],
            )
            .unwrap();
        session.commit_property_edit(Actor::Agent, token).unwrap();
        assert_eq!(session.skybox_preview().unwrap().name, "other_sky");
        session.undo(Actor::Agent).unwrap();
        assert!(Arc::ptr_eq(&original, &session.skybox_preview().unwrap()));
        assert!(session.view().sky_preview);
    }
    #[test]
    fn face_selection_previews_cancellation_and_undo_are_shared_and_atomic() {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let face = FaceId {
            solid: id,
            side: session.document().solid(id).unwrap().sides[0].id,
        };
        let before = session.document().vmf().clone();
        let revision = session.document().revision();
        session.select_faces(&[face], SelectMode::Replace).unwrap();
        assert!(session.face_mode());
        assert!(session.selection().is_empty());
        session.select_faces(&[face], SelectMode::Add).unwrap();
        assert_eq!(session.selected_faces(), [face]);
        assert!(
            session
                .select_faces(
                    &[
                        face,
                        FaceId {
                            solid: id,
                            side: u32::MAX
                        }
                    ],
                    SelectMode::Replace
                )
                .is_err()
        );
        assert_eq!(session.selected_faces(), [face]);
        let patch = SurfaceEdit {
            material: Some("wood/test".into()),
            shift_u: Some(13.5),
            ..Default::default()
        };
        session
            .edit_faces(Actor::User, &[face], patch.clone(), false, true)
            .unwrap();
        assert!(session.has_face_preview());
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().revision(), revision);
        let mesh = session.build_scene();
        assert_eq!(mesh.lines.len(), 8);
        assert!(
            mesh.batches
                .iter()
                .any(|batch| session.materials().get(batch.material).name == "wood/test")
        );
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().revision(), revision);
        session
            .edit_faces(Actor::Agent, &[face], patch.clone(), false, true)
            .unwrap();
        session.commit_face_preview(Actor::User).unwrap();
        assert_eq!(session.document().revision(), revision + 1);
        assert_eq!(
            surface::side(session.document(), face)
                .unwrap()
                .u_axis
                .offset,
            13.5
        );
        assert!(!session.has_face_preview());
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.redo(Actor::User).unwrap();
        let after = session.document().vmf().clone();
        session
            .edit_faces(
                Actor::Agent,
                &[face],
                SurfaceEdit {
                    shift_u: Some(-25.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        session.select_faces(&[], SelectMode::Replace).unwrap();
        assert!(!session.has_face_preview());
        assert!(session.commit_face_preview(Actor::Agent).is_err());
        assert_eq!(session.document().vmf(), &after);
        session
            .edit_faces(Actor::Agent, &[face], patch, false, true)
            .unwrap();
        session.new_map(Actor::Agent, true).unwrap();
        assert!(session.selected_faces().is_empty());
        assert!(!session.has_face_preview());
    }

    #[test]
    fn selection_transforms_share_geometry_pivots_and_single_step_undo() {
        let mut results = Vec::new();
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let id = session
                .create_block(
                    actor,
                    DVec3::ZERO,
                    DVec3::new(64.0, 32.0, 16.0),
                    "brick/wall",
                    false,
                )
                .unwrap();
            let before = session.document().vmf().clone();
            let undo_count = session.document().undo_labels().count();
            session
                .preview_block(DVec3::ZERO, DVec3::splat(16.0))
                .unwrap();
            let pivot = session
                .transform_objects(
                    actor,
                    &[id, id],
                    edit::Transform {
                        translation: DVec3::new(8.0, -8.0, 4.0),
                        rotation: DVec3::new(0.0, 0.0, 90.0),
                        scale: DVec3::new(2.0, 1.0, 0.5),
                        ..edit::Transform::default()
                    },
                    None,
                )
                .unwrap();
            assert_eq!(pivot, DVec3::new(32.0, 16.0, 8.0));
            let (mins, maxs) = query::bounds(session.document(), id).unwrap();
            assert!(mins.abs_diff_eq(DVec3::new(24.0, -56.0, 8.0), 1e-8));
            assert!(maxs.abs_diff_eq(DVec3::new(56.0, 72.0, 16.0), 1e-8));
            assert_eq!(session.document().undo_labels().count(), undo_count + 1);
            assert!(session.block_preview().is_none());
            assert_eq!(session.selected_ids(), [id]);
            results.push(session.document().vmf().clone());
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document().vmf(), results.last().unwrap());
            session.undo(actor).unwrap();
            let pivot = session
                .transform_objects(
                    actor,
                    &[id],
                    edit::Transform {
                        scale: DVec3::splat(2.0),
                        ..edit::Transform::default()
                    },
                    Some(DVec3::ZERO),
                )
                .unwrap();
            assert_eq!(pivot, DVec3::ZERO);
            assert_eq!(
                query::bounds(session.document(), id),
                Some((DVec3::ZERO, DVec3::new(128.0, 64.0, 32.0)))
            );
        }
        assert_eq!(
            results[0].world.solids[0].sides,
            results[1].world.solids[0].sides
        );
    }

    #[test]
    fn rejected_transforms_preserve_document_history_and_preview() {
        let mut session = Session::new(Err("no assets needed".into()));
        let id = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(32.0),
                "brick/wall",
                false,
            )
            .unwrap();
        session
            .preview_block(DVec3::ZERO, DVec3::splat(16.0))
            .unwrap();
        let before = session.document().vmf().clone();
        let serial = session.scene_serial();
        let count = session.document().undo_labels().count();
        for (ids, transform, pivot) in [
            (
                vec![id],
                edit::Transform {
                    scale: DVec3::new(1.0, 0.0, 1.0),
                    ..edit::Transform::default()
                },
                None,
            ),
            (
                vec![id],
                edit::Transform::default(),
                Some(DVec3::splat(f64::NAN)),
            ),
            (
                vec![id, ObjectId::Solid(u32::MAX)],
                edit::Transform {
                    translation: DVec3::X,
                    ..edit::Transform::default()
                },
                None,
            ),
            (vec![], edit::Transform::default(), None),
        ] {
            assert!(
                session
                    .transform_objects(Actor::Agent, &ids, transform, pivot)
                    .is_err()
            );
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.scene_serial(), serial);
            assert_eq!(session.document().undo_labels().count(), count);
            assert!(session.block_preview().is_some());
        }
    }

    #[test]
    fn spatial_edits_match_for_humans_and_agents_and_previews_do_not_modify_maps() {
        let mut brush_faces = Vec::new();
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            session
                .set_spatial(
                    actor,
                    spatial::Update {
                        grid_size: Some(0.25),
                        elevation: Some(-8.0),
                        depth: Some(2.0),
                        draw_block: Some(true),
                        ..spatial::Update::default()
                    },
                )
                .unwrap();
            assert_eq!(session.camera().projection, Projection::Top);
            let revision = session.document().revision();
            let serial = session.scene_serial();
            let (mins, maxs) = session
                .drawn_block_bounds(
                    Projection::Front,
                    DVec2::new(9.12, 4.13),
                    DVec2::new(-1.13, -3.12),
                    None,
                    None,
                )
                .unwrap();
            session.preview_block(mins, maxs).unwrap();
            assert_eq!(session.document().revision(), revision);
            assert_eq!(session.scene_serial(), serial);
            assert_eq!(session.document().undo_labels().count(), 0);
            assert!(session.view().block_preview.is_some());
            let id = session
                .create_block(actor, mins, maxs, "tools/toolsnodraw", false)
                .unwrap();
            assert_eq!(query::bounds(session.document(), id), Some((mins, maxs)));
            assert!(session.block_preview().is_none());
            assert_eq!(session.document().undo_labels().count(), 1);
            brush_faces.push(session.document().vmf().world.solids[0].sides.clone());
            session.undo(actor).unwrap();
            assert!(session.document().vmf().world.solids.is_empty());
            session.redo(actor).unwrap();
            assert_eq!(query::bounds(session.document(), id), Some((mins, maxs)));
        }
        assert_eq!(brush_faces[0], brush_faces[1]);
    }

    #[test]
    fn settings_validation_is_atomic_and_projection_switches_restore_camera_state() {
        let mut session = Session::new(Err("no assets needed".into()));
        let settings = session.spatial();
        let revision = session.document().revision();
        assert!(
            session
                .set_spatial(
                    Actor::Agent,
                    spatial::Update {
                        snap: Some(false),
                        grid_size: Some(0.0),
                        ..spatial::Update::default()
                    }
                )
                .is_err()
        );
        assert_eq!(settings, session.spatial());
        assert_eq!(revision, session.document().revision());
        let perspective = session.camera();
        session.set_projection(Actor::User, Projection::Top);
        let top = Camera::planar(Projection::Top, glam::Vec3::new(10.0, -20.0, 0.0), 256.0);
        session.publish_camera(top);
        session.set_projection(Actor::Agent, Projection::Front);
        session.set_projection(Actor::User, Projection::Top);
        assert_eq!(session.camera(), top);
        session.set_projection(Actor::User, Projection::Perspective);
        assert_eq!(session.camera(), perspective);
        let serial = session.scene_serial();
        let view = session.view();
        session.set_view(ViewOptions {
            tool_textures: false,
            ..view
        });
        assert_eq!(session.scene_serial(), serial);
        assert!(!session.view().tool_textures);
    }

    #[test]
    fn replacing_or_editing_the_map_invalidates_brush_previews() {
        let mut session = Session::new(Err("no assets needed".into()));
        session
            .preview_block(DVec3::ZERO, DVec3::splat(32.0))
            .unwrap();
        session.new_map(Actor::Agent, true).unwrap();
        assert!(session.block_preview().is_none());
        session
            .preview_block(DVec3::ZERO, DVec3::splat(32.0))
            .unwrap();
        assert!(
            session
                .create_block(
                    Actor::User,
                    DVec3::ZERO,
                    DVec3::ZERO,
                    "tools/toolsnodraw",
                    false
                )
                .is_err()
        );
        assert_eq!(session.document().undo_labels().count(), 0);
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(32.0),
                "tools/toolsnodraw",
                false,
            )
            .unwrap();
        assert!(session.block_preview().is_none());
    }
    use map_core::edit;

    const ENTITY_FGD: &str = r#"
        @BaseClass = LightValues [
            _light(color255) : "Brightness" : "255 240 220 250"
            angles(angle) : "Orientation" : "0 0 0"
            style(choices) : "Style" : 0 = [ 0 : "Steady" 1 : "Pulse" ]
            spawnflags(flags) = [ 1 : "Enabled" : 1 ]
            editor_id(integer) readonly : "Editor ID"
        ]
        @PointClass base(LightValues) = light []
        @SolidClass = func_detail []
    "#;

    #[test]
    fn color_and_component_controls_share_validation_and_undo() {
        let mut results = Vec::new();
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no game required".into()));
            session.set_definitions(actor, Catalog::parse(ENTITY_FGD).unwrap());
            let ObjectId::Entity(id) = session
                .create_entity(
                    actor,
                    "light",
                    Some(DVec3::new(1.0, 2.0, 3.0)),
                    BTreeMap::new(),
                )
                .unwrap()
            else {
                unreachable!()
            };
            session
                .set_entity_color(actor, id, "_light", [140, 190, 255], None)
                .unwrap();
            assert_eq!(
                session
                    .document()
                    .entity(id)
                    .unwrap()
                    .properties
                    .get("_light"),
                Some("140 190 255 250")
            );
            session
                .set_entity_component(actor, id, "_light", 3, "450.5")
                .unwrap();
            session
                .set_entity_component(actor, id, "origin", 1, "-22.125")
                .unwrap();
            assert_eq!(
                session
                    .document()
                    .entity(id)
                    .unwrap()
                    .properties
                    .get("origin"),
                Some("1 -22.125 3")
            );
            let before = session.document().entity(id).unwrap().clone();
            assert!(
                session
                    .set_entity_color(actor, id, "_light", [255; 3], Some(-1.0))
                    .is_err()
            );
            assert!(
                session
                    .set_entity_color(actor, id, "_light", [255; 3], Some(f64::NAN))
                    .is_err()
            );
            assert!(
                session
                    .set_entity_component(actor, id, "origin", 3, "9")
                    .is_err()
            );
            assert!(
                session
                    .set_entity_component(actor, id, "origin", 0, "NaN")
                    .is_err()
            );
            assert!(
                session
                    .set_entity_component(actor, id, "_light", 0, "999")
                    .is_err()
            );
            assert!(
                session
                    .set_entity_component(actor, id, "_light", 3, "-1")
                    .is_err()
            );
            assert_eq!(session.document().entity(id).unwrap(), &before);
            session.undo(actor);
            assert_eq!(
                session
                    .document()
                    .entity(id)
                    .unwrap()
                    .properties
                    .get("origin"),
                Some("1 2 3")
            );
            session.redo(actor);
            results.push(session.document().entity(id).unwrap().clone());
        }
        assert_eq!(results[0], results[1]);
    }

    #[test]
    fn normalized_colors_and_missing_previews_are_safe() {
        let mut session = Session::new(Err("no game required".into()));
        session.set_definitions(Actor::User, Catalog::parse(r#"@PointClass = info_target [ tint(color1) : "Tint" : "1 1 1" locked(color255) readonly : "Locked" : "255 255 255" ]"#).unwrap());
        let ObjectId::Entity(id) = session
            .create_entity(Actor::User, "info_target", None, BTreeMap::new())
            .unwrap()
        else {
            unreachable!()
        };
        session
            .set_entity_color(Actor::User, id, "tint", [0, 255, 0], None)
            .unwrap();
        assert_eq!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("tint"),
            Some("0 1 0")
        );
        assert!(
            session
                .set_entity_color(Actor::User, id, "tint", [255; 3], Some(1.0))
                .is_err()
        );
        assert!(
            session
                .set_entity_color(Actor::User, id, "locked", [0; 3], None)
                .is_err()
        );
        let revision = session.document().revision();
        let preview = session.material_preview("missing/wall").unwrap();
        assert!(preview.missing);
        assert!(preview.image.is_none());
        assert_eq!(session.document().revision(), revision);
    }

    #[test]
    fn malformed_materials_report_the_same_errors_from_cached_and_uncached_previews() {
        let (directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let folder = directory.path().join("materials/test");
        std::fs::write(folder.join("broken.vtf"), b"broken VTF").unwrap();
        for text in [
            "LightmappedGeneric { $basetexture }",
            "Patch { include materials/test/absent.vmt }",
            "LightmappedGeneric { %tooltexture test/broken $basetexture test/checker }",
            "WorldVertexTransition { $basetexture test/checker $basetexture2 test/absent }",
        ] {
            std::fs::write(folder.join("broken.vmt"), text).unwrap();
            session.materials = MaterialLibrary::default();
            let error = session.material_preview("test/broken").unwrap_err();
            let id = session.materials.load(&session.game, "test/broken");
            assert_eq!(
                session.materials.get(id).error.as_deref(),
                Some(error.as_str())
            );
            assert_eq!(session.material_preview("test/broken").unwrap_err(), error);
            let brush = session
                .create_block(
                    Actor::Agent,
                    DVec3::ZERO,
                    DVec3::splat(32.0),
                    "test/broken",
                    false,
                )
                .unwrap();
            let before = session.document.vmf().clone();
            session.run_map_checks(Actor::User);
            assert!(
                session
                    .map_check_report()
                    .unwrap()
                    .result
                    .issues
                    .iter()
                    .any(|issue| issue.code == "invalid_material"
                        && issue.location.object == Some(brush))
            );
            assert_eq!(session.document.vmf(), &before);
            session.undo(Actor::Agent).unwrap();
        }
        std::fs::write(
            folder.join("broken.vmt"),
            "LightmappedGeneric { $basetexture test/checker }",
        )
        .unwrap();
        session.materials = MaterialLibrary::default();
        assert!(!session.material_preview("test/broken").unwrap().missing);
    }

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
                    BTreeMap::from([
                        ("targetname".into(), "test_light".into()),
                        ("angles".into(), "0 90 0".into()),
                    ]),
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
            ("angles", "90"),
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
                    ("angles".into(), Some("0 180 0".into())),
                    ("_light".into(), Some("128 96 64 500".into())),
                    ("style".into(), Some("1".into())),
                    ("spawnflags".into(), Some("1025".into())),
                ],
            )
            .unwrap();
        let entity = session.document().entity(id).unwrap();
        assert_eq!(entity.classname(), Some("light"));
        assert_eq!(entity.properties.get("angles"), Some("0 180 0"));
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

    fn compile(session: &mut Session) -> CompileResult {
        let result = session.begin_compile(Actor::Agent, None).unwrap().run();
        session.finish_compile(Actor::Agent, &result);
        result.unwrap()
    }

    #[test]
    fn changed_document_cannot_launch_an_earlier_compile() {
        let (directory, game, _) = crate::test_assets::game_process(true);
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
        let (directory, game, _) = crate::test_assets::game_process(true);
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
        let (directory, game, _) = crate::test_assets::game_process(true);
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
        let (directory, game, _) = crate::test_assets::game_process(true);
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
        let (directory, game, _) = crate::test_assets::game_process(true);
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
        let (directory, game, _) = crate::test_assets::game_process(true);
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
    #[test]
    fn compile_jobs_cancel_guard_tokens_and_preserve_previous_output() {
        for actor in [Actor::User, Actor::Agent] {
            let (directory, game, _) = crate::test_assets::game_process(true);
            let mut session = Session::new(Ok(game));
            session.replace_document(
                Document::new(room()),
                Some(directory.path().join("room.vmf")),
            );
            let path = directory.path().join("room.bsp");
            std::fs::write(&path, b"previous BSP").unwrap();
            std::fs::write(path.with_extension("lin"), b"previous leak").unwrap();
            let request = session.begin_compile(actor, None).unwrap();
            let token = session.compile_token().unwrap();
            assert_eq!(session.compile_status()["job"]["stage"], "queued");
            assert!(session.compile_cancellable());
            assert!(session.cancel_compile(actor, token + 1).is_err());
            session.cancel_compile(actor, token).unwrap();
            assert_eq!(session.compile_status()["job"]["status"], "cancelling");
            assert!(session.begin_compile(actor, None).is_err());
            let result = request.run();
            assert!(result.is_err());
            session.finish_compile(actor, &result);
            assert_eq!(session.compile_status()["job"]["status"], "cancelled");
            assert!(!session.compiling());
            assert!(!session.compile_cancellable());
            assert!(session.begin_launch(actor).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
            assert_eq!(
                std::fs::read(path.with_extension("lin")).unwrap(),
                b"previous leak"
            );
            let request = session.begin_compile(actor, None).unwrap();
            assert_ne!(session.compile_token(), Some(token));
            assert!(session.cancel_compile(actor, token).is_err());
            let result = request.run();
            assert!(
                session
                    .cancel_compile(actor, session.compile_token().unwrap())
                    .is_err()
            );
            session.finish_compile(actor, &result);
            let status = session.compile_status();
            assert_eq!(status["job"]["status"], "completed");
            assert_eq!(status["job"]["result"]["success"], true);
            assert_eq!(status["job"]["stale"], false);
            assert!(std::fs::read(&path).unwrap().starts_with(b"VBSP"));
            session.new_map(actor, true).unwrap();
            assert_eq!(session.compile_status()["job"]["stale"], true);
            assert!(session.compile_label().contains("earlier map snapshot"));
        }
    }

    #[test]
    fn compile_jobs_report_failures_and_shutdown_cancels_work() {
        let (directory, game, _) = crate::test_assets::game_process(true);
        let mut session = Session::new(Ok(game));
        let path = directory.path().join("empty.bsp");
        let request = session.begin_compile(Actor::Agent, Some(&path)).unwrap();
        let result = request.run();
        session.finish_compile(Actor::Agent, &result);
        assert_eq!(session.compile_status()["job"]["status"], "failed");
        assert_eq!(session.compile_status()["job"]["result"]["success"], false);
        assert!(!path.exists());
        assert!(!session.compile_cancellable());
        session.replace_document(Document::new(room()), None);
        let missing = directory.path().join("missing/room.bsp");
        let request = session.begin_compile(Actor::Agent, Some(&missing)).unwrap();
        let result = request.run();
        assert!(result.is_err());
        session.finish_compile(Actor::Agent, &result);
        assert_eq!(session.compile_status()["job"]["status"], "failed");
        assert!(session.compile_status()["job"]["result"]["error"].is_string());
        let request = session.begin_compile(Actor::Agent, Some(&path)).unwrap();
        session.shutdown();
        let result = request.run();
        assert!(result.is_err());
        session.finish_compile(Actor::Agent, &result);
        assert_eq!(session.compile_status()["job"]["status"], "cancelled");
        assert!(!path.exists());
    }
}
