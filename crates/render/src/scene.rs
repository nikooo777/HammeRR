//! Converts a VMF into GPU-ready triangle batches, one per material.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::{DVec3, Vec3};
use map_core::surface::FaceId;
use map_core::{BrushGeometry, DisplacementMesh, ObjectId, texel_uv};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Color, Entity, Solid, Vmf};

use crate::materials::{MaterialId, MaterialLibrary};

pub(crate) const MATERIAL_EMISSION: u32 = 4;
const MODEL_SURFACE: u32 = 8;
pub(crate) const MODEL_PHONG_FRAME: u32 = 16;
pub(crate) const DETAIL_LAYER: u32 = 32;
pub(crate) const ENVIRONMENT_LAYER: u32 = 64;
const WATER_CAPTURE_BRUSH: u32 = 512;

mod model_lighting;
mod model_normals;
pub(crate) mod projected_overlays;
mod rope_mesh;
pub(crate) mod water;

pub(crate) mod terrain;
pub use projected_overlays::{Draft as OverlayDraft, Preview as OverlayPreview};

const HELPER_SEGMENTS: usize = 16;
const MAX_PREVIEW_ISSUES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NormalReceiver {
    Flat,
    Terrain,
    Model,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SolidOwner {
    World,
    Detail,
    Entity,
}

/// Hammer's color for entities whose editor block has none.
const DEFAULT_ENTITY_COLOR: [u8; 3] = [220, 30, 220];
const INVALID_HELPER_COLOR: [u8; 3] = [255, 176, 48];
const SELECTED_TINT: [u8; 3] = [255, 130, 130];
const SELECTED_EDGE: [u8; 4] = [255, 64, 64, 255];
const SELECTED_MODEL_TINT: [u8; 3] = [255, 210, 150];
const SELECTED_MODEL_MIN: [u8; 3] = [96, 80, 64];
const WIRE_EDGE: [u8; 4] = [151, 169, 191, 255];

fn lightmap_vertex(side: &source_formats::vmf::Side, point: DVec3) -> [f32; 3] {
    map_core::surface::lightmap_uv(side, point)
        .map(|uv| [uv.x as f32, uv.y as f32, side.lightmap_scale as f32])
        .filter(|values| values.iter().all(|value| value.is_finite()))
        .unwrap_or([0.0, 0.0, -1.0])
}

fn displacement_lines(
    mesh: &DisplacementMesh,
    color: [u8; 4],
) -> impl Iterator<Item = LineVertex> + '_ {
    let mut edges = BTreeSet::new();
    for [a, b, c] in mesh.triangles() {
        for (a, b) in [(a, b), (b, c), (c, a)] {
            edges.insert((a.min(b), a.max(b)));
        }
    }
    edges.into_iter().flat_map(move |(a, b)| {
        [a, b].map(|index| LineVertex {
            position: mesh.positions[index].as_vec3().to_array(),
            color,
        })
    })
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// RGB tint; alpha blends brush layers.
    pub color: [u8; 4],
    pub lightmap: [f32; 3],
    pub smoothing: u32,
    /// Baked FACE receivers use this slot for the native terrain blend fraction.
    pub opacity: f32,
    pub texture_flags: u32,
    pub tangent: [f32; 4],
    pub lighting_receiver: u32,
    pub baked_irradiance: [f32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub material: MaterialId,
    pub first_index: u32,
    pub index_count: u32,
    pub rope: bool,
    pub model_translucent: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SceneStats {
    pub brushes: usize,
    pub invalid_brushes: usize,
    pub displacements: usize,
    pub point_entities: usize,
    pub model_entities: usize,
    pub fgd_model_helpers: usize,
    pub missing_materials: usize,
}

#[derive(Debug)]
pub struct SceneMesh {
    pub(crate) water: Option<water::Captures>,
    pub(crate) projected_overlay_batches: std::ops::Range<usize>,
    pub baked_faces: Option<std::sync::Arc<crate::compiled_faces::LightingData>>,
    pub baked_face_materials:
        Vec<Option<std::sync::Arc<crate::compiled_faces::materials::Material>>>,
    pub baked_face_batches: std::ops::Range<usize>,
    pub baked_face_visible_surfaces: usize,
    pub detail_billboards: Vec<crate::compiled_details::Billboard>,
    pub detail_parts: Vec<crate::compiled_details::Part>,
    pub compiled_detail_hdr: Option<std::sync::Arc<crate::compiled_details::HdrRadiance>>,
    pub compiled_detail_hdr_ordinals: Vec<u32>,
    pub compiled_detail_hdr_draw_issue: Option<crate::compiled_details::HdrRadianceTableIssue>,
    pub reference_lines: Vec<LineVertex>,
    pub reference_selection_ranges: Vec<std::ops::Range<u32>>,
    pub lighting: crate::lighting::Preview,
    pub normal_maps: crate::normal_maps::Report,
    pub issues: SceneIssues,
    pub skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
    pub sky3d: Option<std::sync::Arc<crate::sky3d::Scene>>,
    pub icons: Vec<crate::icons::Instance>,
    pub reference_images: Vec<crate::reference_images::Plane>,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// Ordered editor lines; selection ranges exclude diagnostic and tool guides.
    pub lines: Vec<LineVertex>,
    pub selection_outline_ranges: Vec<std::ops::Range<u32>>,
    pub leak_lines: Vec<LineVertex>,
    pub wire_lines: Vec<LineVertex>,
    pub tool_wire_lines: Vec<LineVertex>,
    pub rope_wire_lines: Vec<LineVertex>,
    pub rope_lines: Vec<LineVertex>,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub stats: SceneStats,
}

#[derive(Debug, Clone)]
pub struct TerrainIssue {
    pub face: FaceId,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum SceneIssue {
    Brush {
        solid: u32,
        message: String,
    },
    Surface {
        face: FaceId,
        message: String,
    },
    Model {
        entity: Option<u32>,
        message: String,
    },
    Terrain(TerrainIssue),
    Overlay {
        entity: Option<u32>,
        message: String,
    },
}

impl SceneIssue {
    pub fn solid(&self) -> Option<u32> {
        match self {
            Self::Brush { solid, .. } => Some(*solid),
            Self::Surface { face, .. } => Some(face.solid),
            Self::Terrain(issue) => Some(issue.face.solid),
            Self::Model { .. } | Self::Overlay { .. } => None,
        }
    }

    pub fn entity(&self) -> Option<u32> {
        match self {
            Self::Model { entity, .. } | Self::Overlay { entity, .. } => *entity,
            _ => None,
        }
    }

    pub fn face(&self) -> Option<FaceId> {
        match self {
            Self::Brush { .. } | Self::Model { .. } | Self::Overlay { .. } => None,
            Self::Surface { face, .. } => Some(*face),
            Self::Terrain(issue) => Some(issue.face),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Brush { message, .. } => message,
            Self::Surface { message, .. } => message,
            Self::Terrain(issue) => &issue.message,
            Self::Model { message, .. } => message,
            Self::Overlay { message, .. } => message,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Brush { solid, message } => format!("Brush {solid}: {message}"),
            Self::Surface { face, message } => {
                format!("Face {} on brush {}: {message}", face.side, face.solid)
            }
            Self::Terrain(issue) => format!(
                "Terrain face {} on brush {}: {}",
                issue.face.side, issue.face.solid, issue.message
            ),
            Self::Model { entity, message } => match entity {
                Some(id) => format!("Model {id} preview: {message}"),
                None => format!("Model preview: {message}"),
            },
            Self::Overlay { entity, message } => match entity {
                Some(id) => format!("Overlay {id} preview: {message}"),
                None => format!("Overlay draft preview: {message}"),
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SceneIssues {
    pub entries: Vec<SceneIssue>,
    pub omitted: usize,
}

impl SceneIssues {
    pub fn prepend(&mut self, issue: SceneIssue) {
        let retained = MAX_PREVIEW_ISSUES - 1;
        if self.entries.len() > retained {
            self.omitted += self.entries.len() - retained;
            self.entries.truncate(retained);
        }
        self.entries.insert(0, issue);
    }

    fn push(&mut self, issue: SceneIssue) {
        if self.entries.len() < MAX_PREVIEW_ISSUES {
            self.entries.push(issue);
        } else {
            self.omitted += 1;
        }
    }

    pub fn summary(&self) -> String {
        self.entries.first().map_or_else(String::new, |issue| {
            let mut summary = issue.description();
            let remaining = self.entries.len() - 1 + self.omitted;
            if remaining > 0 {
                summary.push_str(&format!(" ({remaining} more preview issues)"));
            }
            summary
        })
    }
}

pub fn terrain_issues(vmf: &Vmf) -> Vec<TerrainIssue> {
    terrain::prepare(vmf).issues
}

#[derive(Debug, Clone)]
pub struct FaceOverlay {
    pub source: FaceId,
    pub points: Vec<DVec3>,
    pub material: String,
    pub projection: Option<map_core::surface::Projection>,
}

pub struct TerrainNormalOverride {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[usize; 3]>,
    pub normals: Vec<[f64; 3]>,
}

struct Builder<'a> {
    authored_surfaces: bool,
    water: Option<water::Pool>,
    water_face: Option<FaceId>,
    entity_helpers: bool,
    issues: SceneIssues,
    tool_textures: bool,
    selection_tint: bool,
    lighting: crate::lighting::Preview,
    normal_maps: crate::normal_maps::Report,
    textured: bool,
    icons: &'a BTreeMap<u32, crate::icons::Instance>,
    displayed_icons: Vec<crate::icons::Instance>,
    helper_bounds: Vec<Vec3>,
    models: &'a [crate::models::Instance],
    helpers: &'a BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
    overlays: &'a [FaceOverlay],
    projections: projected_overlays::Projections,
    selected_faces: &'a HashSet<FaceId>,
    fs: &'a GameFs,
    library: &'a mut MaterialLibrary,
    vertices: Vec<Vertex>,
    /// Indices per material id, concatenated into batches at the end.
    indices: Vec<Vec<u32>>,
    model_batches: Vec<(MaterialId, Vec<u32>)>,
    lines: Vec<LineVertex>,
    selection_outline_ranges: Vec<std::ops::Range<u32>>,
    wire_lines: Vec<LineVertex>,
    tool_wire_lines: Vec<LineVertex>,
    stats: SceneStats,
    light_targets: map_core::light::Targets<'a>,
    displacements: HashMap<FaceId, Displacement>,
    terrain_geometry: HashMap<u32, Result<BrushGeometry, String>>,
    unshaded_terrain: HashMap<FaceId, DisplacementMesh>,
    terrain_issues: HashMap<u32, Vec<TerrainIssue>>,
}

struct Displacement {
    mesh: DisplacementMesh,
    normals: Vec<[f64; 3]>,
}

impl Builder<'_> {
    fn material(&mut self, name: &str) -> MaterialId {
        let id = self.library.load(self.fs, name);
        if self.indices.len() <= id {
            self.indices.resize_with(id + 1, Vec::new);
        }
        id
    }

    fn material_emission(&self, material: MaterialId, model: bool) -> Result<u32, String> {
        let info = self.library.get(material);
        if !self.textured || info.tool && !self.tool_textures {
            return Ok(0);
        }
        match info.emission_for(model) {
            Ok(true) if info.tool => Err(format!(
                "{}: Material emission requires an opaque non-tool material",
                info.name
            )),
            Ok(true) => Ok(MATERIAL_EMISSION),
            Ok(false) => Ok(0),
            Err(reason) => Err(format!("{}: {reason}", info.name)),
        }
    }

    fn base_transform(
        &self,
        material: MaterialId,
        model: bool,
        coordinates: impl Iterator<Item = [f32; 2]>,
    ) -> Result<u32, String> {
        let info = self.library.get(material);
        if !self.textured || info.tool && !self.tool_textures {
            return Ok(0);
        }
        let error = match &info.base_transform {
            Ok(None) => return Ok(0),
            Ok(Some(transform)) if transform.model == model => {
                if crate::materials::uv::fits_gpu(transform.rows, coordinates) {
                    return Ok(crate::materials::uv::FLAG);
                }
                "Base texture transform coordinates exceed finite GPU range"
            }
            Ok(Some(_)) => "Base texture transform requires its matching brush or model shader",
            Err(error) => error,
        };
        Err(format!("{}: {error}", info.name))
    }

    fn surface_animation(&mut self, material: MaterialId, face: FaceId, supported: bool) -> u32 {
        use crate::materials::animation::{FLAG, Layer};

        let info = self.library.get(material);
        if !self.textured
            || info.tool && !self.tool_textures
            || matches!(info.animation, Layer::Absent)
        {
            return 0;
        }
        if !supported {
            self.issues.push(SceneIssue::Surface {
                face,
                message: format!(
                    "{}: Terrain base frame preview requires world or func_detail terrain",
                    info.name
                ),
            });
            return 0;
        }
        self.library.prepare_animation(material);
        let info = self.library.get(material);
        match &info.animation {
            Layer::Ready(_) => FLAG,
            Layer::Unavailable(reason) => {
                let message = format!("{}: {reason}", info.name);
                self.issues.push(SceneIssue::Surface { face, message });
                0
            }
            _ => 0,
        }
    }

    fn surface_base_transform(
        &mut self,
        material: MaterialId,
        face: FaceId,
        coordinates: impl Iterator<Item = [f32; 2]>,
    ) -> u32 {
        match self.base_transform(material, false, coordinates) {
            Ok(flags) => flags,
            Err(message) => {
                self.issues.push(SceneIssue::Surface { face, message });
                0
            }
        }
    }

    fn surface_emission(&mut self, material: MaterialId, face: FaceId, supported: bool) -> u32 {
        let emission = self.material_emission(material, false).and_then(|flags| {
            if flags != 0 && !supported {
                Err("Terrain material emission requires world or func_detail terrain".into())
            } else {
                Ok(flags)
            }
        });
        match emission {
            Ok(flags) => flags,
            Err(message) => {
                self.issues.push(SceneIssue::Surface { face, message });
                0
            }
        }
    }

    fn surface_detail(
        &mut self,
        material: MaterialId,
        face: FaceId,
        supported: bool,
        coordinates: impl Iterator<Item = [f32; 2]>,
    ) -> u32 {
        use crate::materials::detail::Layer;

        let info = self.library.get(material);
        if !self.textured
            || info.tool && !self.tool_textures
            || matches!(info.detail, Layer::Absent)
        {
            return 0;
        }
        let message = if !supported {
            format!(
                "{}: Terrain detail layers require world or func_detail terrain",
                info.name
            )
        } else {
            self.library.load_detail(self.fs, material);
            let info = self.library.get(material);
            match &info.detail {
                Layer::Ready(asset) => {
                    let scale = asset.parameters.scale;
                    if crate::materials::uv::fits_gpu(
                        [[scale, 0.0, 0.0], [0.0, scale, 0.0]],
                        coordinates,
                    ) {
                        return DETAIL_LAYER;
                    }
                    format!(
                        "{}: Detail layer coordinates exceed finite GPU range",
                        info.name
                    )
                }
                Layer::Unavailable(reason) => format!("{}: {reason}", info.name),
                _ => return 0,
            }
        };
        self.issues.push(SceneIssue::Surface { face, message });
        0
    }

    fn surface_environment(&mut self, material: MaterialId, face: FaceId, flat: bool) -> u32 {
        use crate::materials::environment::Layer;

        let info = self.library.get(material);
        if !self.textured
            || info.tool && !self.tool_textures
            || matches!(info.environment, Layer::Absent)
        {
            return 0;
        }
        if flat {
            self.library.load_environment(self.fs, material);
        }
        let info = self.library.get(material);
        let reason = match &info.environment {
            Layer::Ready(_) if flat => return ENVIRONMENT_LAYER,
            Layer::Unavailable(reason) => reason,
            _ => "Environment preview requires a flat authored brush face",
        };
        self.issues.push(SceneIssue::Surface {
            face,
            message: format!("{}: {reason}", info.name),
        });
        0
    }

    fn push_vertex(&mut self, vertex: Vertex) -> u32 {
        self.vertices.push(vertex);
        (self.vertices.len() - 1) as u32
    }

    fn push_loop(&mut self, points: impl IntoIterator<Item = Vec3>) {
        let points: Vec<Vec3> = points.into_iter().collect();
        for (i, &a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            for p in [a, b] {
                self.lines.push(LineVertex {
                    position: p.to_array(),
                    color: SELECTED_EDGE,
                });
            }
        }
    }

    fn mark_selection_outline(&mut self, start: usize) {
        let end = self.lines.len() as u32;
        if end == start as u32 {
            return;
        }
        if let Some(previous) = self.selection_outline_ranges.last_mut()
            && previous.end == start as u32
        {
            previous.end = end;
        } else {
            self.selection_outline_ranges.push(start as u32..end);
        }
    }

    fn selection_loop(&mut self, points: impl IntoIterator<Item = Vec3>) {
        let start = self.lines.len();
        self.push_loop(points);
        self.mark_selection_outline(start);
    }

    fn selection_line(&mut self, from: Vec3, to: Vec3, color: [u8; 4]) {
        let start = self.lines.len();
        self.helper_line(from, to, color);
        self.mark_selection_outline(start);
    }

    fn add_solid(&mut self, solid: &Solid, selected: bool, owner: SolidOwner) {
        let terrain_owner_supported = matches!(owner, SolidOwner::World | SolidOwner::Detail);
        if let Some(issues) = self.terrain_issues.remove(&solid.id) {
            for issue in issues {
                self.issues.push(SceneIssue::Terrain(issue));
            }
        }
        self.stats.brushes += 1;
        let geometry = match self.terrain_geometry.remove(&solid.id) {
            Some(geometry) => geometry,
            None => BrushGeometry::from_solid(solid).map_err(|error| error.to_string()),
        };
        let brush = match geometry {
            Ok(brush) => brush,
            Err(message) => {
                if !solid.sides.iter().any(|side| side.displacement.is_some()) {
                    self.issues.push(SceneIssue::Brush {
                        solid: solid.id,
                        message,
                    });
                }
                self.stats.invalid_brushes += 1;
                for side in solid
                    .sides
                    .iter()
                    .filter(|side| side.displacement.is_some())
                {
                    self.exclude_terrain_geometry(
                        FaceId {
                            solid: solid.id,
                            side: side.id,
                        },
                        &side.material,
                        0,
                        "No prepared terrain geometry",
                    );
                }
                return;
            }
        };
        // Hammer draws only the displaced faces of a brush with displacements.
        let has_displacement = solid.sides.iter().any(|s| s.displacement.is_some());
        for face in &brush.faces {
            let side = &solid.sides[face.side];
            let id = FaceId {
                solid: solid.id,
                side: side.id,
            };
            let selected = selected || self.selected_faces.contains(&id);
            let material = self.material(&side.material);
            if self.textured
                && self.authored_surfaces
                && !matches!(
                    self.library.get(material).water,
                    crate::materials::water::Layer::Absent
                )
            {
                self.record_water(solid, &brush, face, material, owner);
            }
            if !self.authored_surfaces && !self.library.get(material).sky {
                if selected {
                    let id = FaceId {
                        solid: solid.id,
                        side: side.id,
                    };
                    if let Some(displacement) = self.displacements.get(&id) {
                        let outline = displacement
                            .mesh
                            .outline()
                            .into_iter()
                            .map(|index| displacement.mesh.positions[index].as_vec3())
                            .collect::<Vec<_>>();
                        self.selection_loop(outline);
                    } else {
                        self.selection_loop(face.vertices.iter().map(|point| point.as_vec3()));
                    }
                }
                continue;
            }
            let tint = if selected && self.selection_tint {
                SELECTED_TINT
            } else {
                [255; 3]
            };
            let info = self.library.get(material);
            let tool = info.tool;
            let size = DVec3::new(f64::from(info.width), f64::from(info.height), 1.0);
            let uv = |p: DVec3| (texel_uv(side, p) / size.truncate()).as_vec2().to_array();

            match (&side.displacement, has_displacement) {
                (Some(_), _) => {
                    if !self.admit_surface(id, side, face.plane.normal, &face.vertices) {
                        continue;
                    }
                    let displacement = self.displacements.remove(&id);
                    let Some(Displacement { mesh, normals }) = displacement else {
                        self.exclude_terrain_geometry(
                            id,
                            &side.material,
                            0,
                            "No prepared shaded terrain surface; ordinary outline retained (see terrain issues)",
                        );
                        self.surface_outline(id, &face.vertices);
                        continue;
                    };
                    self.stats.displacements += 1;
                    self.add_displacement(
                        material,
                        &mesh,
                        &normals,
                        uv,
                        tint,
                        (id, side, terrain_owner_supported),
                    );
                    if terrain_owner_supported {
                        self.projections.terrain(
                            side.id,
                            &mesh,
                            &normals,
                            face.plane.normal,
                            self.library.get(material),
                        );
                    }
                    self.push_displacement_wire(&mesh, tool);
                    if selected {
                        self.selection_loop(
                            mesh.outline()
                                .into_iter()
                                .map(|i| mesh.positions[i].as_vec3()),
                        );
                    }
                }
                (None, true) => {}
                (None, false) => {
                    let overlays = self
                        .overlays
                        .iter()
                        .filter(|overlay| {
                            overlay.source
                                == FaceId {
                                    solid: solid.id,
                                    side: side.id,
                                }
                        })
                        .collect::<Vec<_>>();
                    if overlays.is_empty() {
                        let drawn = self.add_polygon(
                            (id, side, terrain_owner_supported),
                            &face.vertices,
                            face.plane.normal,
                            &side.material,
                            tint,
                            selected,
                        );
                        if terrain_owner_supported && drawn {
                            self.projections.flat(
                                side.id,
                                &face.vertices,
                                face.plane.normal,
                                self.library.get(material),
                            );
                        }
                    } else {
                        for overlay in overlays {
                            let mut projected = side.clone();
                            if let Some(projection) = &overlay.projection {
                                projected.u_axis = projection.u;
                                projected.v_axis = projection.v;
                                projected.rotation = projection.rotation;
                            }
                            self.add_polygon(
                                (id, &projected, false),
                                &overlay.points,
                                face.plane.normal,
                                &overlay.material,
                                tint,
                                selected,
                            );
                        }
                    }
                }
            }
        }
    }

    fn record_water(
        &mut self,
        solid: &Solid,
        brush: &BrushGeometry,
        face: &map_core::brush::Face,
        material: MaterialId,
        owner: SolidOwner,
    ) {
        use crate::materials::water::Layer;

        let side = &solid.sides[face.side];
        let id = FaceId {
            solid: solid.id,
            side: side.id,
        };
        if self.water_face.replace(id).is_some() {
            let reason = "Water refraction supports one visible world-pool top";
            if let Some(pool) = self.water.take() {
                self.issues.push(pool.issue(reason));
            }
            self.issues.push(SceneIssue::Surface {
                face: id,
                message: reason.into(),
            });
            return;
        }
        let result = (|| {
            if owner != SolidOwner::World
                || self.overlays.iter().any(|overlay| overlay.source == id)
            {
                return Err(
                    "Water refraction needs an unmodified structural-world brush top".into(),
                );
            }
            self.library.load_water(self.fs, material);
            let info = self.library.get(material);
            let asset = match &info.water {
                Layer::Ready(asset) => asset,
                Layer::Unavailable(error) => return Err(error.clone()),
                _ => return Err("Water material was not prepared".into()),
            };
            if [info.width, info.height] != asset.texel_size {
                return Err("Water display and normal texture dimensions differ".into());
            }
            map_core::surface::validate_projection(side, face.plane.normal, &face.vertices)
                .and_then(|()| map_core::surface::validate_lightmap_scale(side.lightmap_scale))
                .map_err(|error| error.to_string())?;
            water::Pool::from_brush(solid, brush, face, material, asset.texel_size)
        })();
        match result {
            Ok(pool) => self.water = Some(pool),
            Err(message) => self.issues.push(SceneIssue::Surface {
                face: id,
                message: format!("{}: {message}", side.material),
            }),
        }
    }

    fn surface_outline(&mut self, face: FaceId, points: &[DVec3]) {
        let mesh = self
            .displacements
            .get(&face)
            .map(|displacement| &displacement.mesh)
            .or_else(|| self.unshaded_terrain.get(&face));
        if let Some(mesh) = mesh {
            self.lines.extend(displacement_lines(mesh, SELECTED_EDGE));
        } else {
            self.push_loop(points.iter().map(|point| point.as_vec3()));
        }
    }

    fn admit_surface(
        &mut self,
        face: FaceId,
        side: &source_formats::vmf::Side,
        normal: DVec3,
        points: &[DVec3],
    ) -> bool {
        let result = map_core::surface::validate_projection(side, normal, points)
            .and_then(|()| map_core::surface::validate_lightmap_scale(side.lightmap_scale));
        if let Err(error) = result {
            if side.displacement.is_some() {
                self.exclude_terrain_geometry(face, &side.material, 0, &error.to_string());
            }
            self.issues.push(SceneIssue::Surface {
                face,
                message: error.to_string(),
            });
            self.surface_outline(face, points);
            return false;
        }
        true
    }

    fn add_polygon(
        &mut self,
        (face, side, structural): (FaceId, &source_formats::vmf::Side, bool),
        points: &[DVec3],
        normal: DVec3,
        name: &str,
        tint: [u8; 3],
        selected: bool,
    ) -> bool {
        if !self.admit_surface(face, side, normal, points) {
            return false;
        }
        let material = self.material(name);
        let emission = self.surface_emission(material, face, true);
        let info = self.library.get(material);
        let size = glam::DVec2::new(f64::from(info.width), f64::from(info.height));
        let uv = |point| (texel_uv(side, point) / size).as_vec2().to_array();
        let base_transform =
            self.surface_base_transform(material, face, points.iter().copied().map(uv));
        let detail = self.surface_detail(material, face, true, points.iter().copied().map(uv));
        let environment = self.surface_environment(material, face, true);
        let animation = self.surface_animation(material, face, true);
        let info = self.library.get(material);
        let tool = info.tool;
        let dimensions = [info.width, info.height];
        let missing_base = info.base.is_none() || info.missing;
        let mut lighting_receiver = 0;
        if self.lighting.report.active {
            let reason = if tool {
                Some("Tool and sky surfaces are excluded from authored lighting")
            } else if missing_base {
                Some("Authored lighting needs an available base display texture")
            } else if points.iter().any(|point| {
                !point.is_finite()
                    || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
            }) {
                Some("Authored-light receivers must fit finite Source compile coordinates")
            } else {
                info.lighting_error.as_deref()
            };
            if let Some(reason) = reason {
                self.lighting.report.excluded_receivers += 1;
                self.lighting.report.diagnostic(None, Some(name), reason);
            } else {
                lighting_receiver = 1;
                self.lighting.report.admitted_receivers += 1;
            }
        }
        let mut tangent = self
            .water
            .filter(|pool| pool.face == face)
            .map_or([0.0; 4], |pool| pool.tangent);
        let water_capture = if structural { WATER_CAPTURE_BRUSH } else { 0 };
        if self.normal_maps.active && !tool {
            let result = self
                .normal_asset(material, NormalReceiver::Flat)
                .and_then(|asset| {
                    asset
                        .map(|asset| {
                            crate::normal_maps::tangent(
                                side,
                                normal,
                                points,
                                dimensions,
                                asset.report.transform,
                            )
                        })
                        .transpose()
                });
            match result {
                Ok(Some(value)) => {
                    tangent = value;
                    self.normal_maps.applied_faces += 1;
                }
                Ok(None) => {}
                Err(error) => {
                    self.normal_maps.rejected_faces += 1;
                    self.normal_maps
                        .error(format!("Face {} · {}: {error}", side.id, name));
                }
            }
        }
        self.push_wire_loop(points.iter().map(|point| point.as_vec3()), tool);
        let first = self.vertices.len() as u32;
        for &point in points {
            self.push_vertex(Vertex {
                position: point.as_vec3().to_array(),
                normal: normal.as_vec3().to_array(),
                uv: uv(point),
                color: [tint[0], tint[1], tint[2], 0],
                lightmap: lightmap_vertex(side, point),
                smoothing: side.smoothing_groups.unwrap_or(0),
                opacity: 1.0,
                texture_flags: emission
                    | base_transform
                    | detail
                    | environment
                    | animation
                    | water_capture,
                tangent,
                baked_irradiance: [0.0; 3],
                lighting_receiver,
            });
        }
        for index in 1..points.len() as u32 - 1 {
            self.indices[material].extend([first, first + index, first + index + 1]);
        }
        if selected {
            self.selection_loop(points.iter().map(|point| point.as_vec3()));
        }
        true
    }

    fn push_wire_loop(&mut self, points: impl IntoIterator<Item = Vec3>, tool: bool) {
        let points: Vec<Vec3> = points.into_iter().collect();
        let lines = if tool {
            &mut self.tool_wire_lines
        } else {
            &mut self.wire_lines
        };
        for (index, &a) in points.iter().enumerate() {
            let b = points[(index + 1) % points.len()];
            lines.extend([a, b].map(|point| LineVertex {
                position: point.to_array(),
                color: WIRE_EDGE,
            }));
        }
    }

    fn push_displacement_wire(&mut self, mesh: &DisplacementMesh, tool: bool) {
        let lines = if tool {
            &mut self.tool_wire_lines
        } else {
            &mut self.wire_lines
        };
        lines.extend(displacement_lines(mesh, WIRE_EDGE));
    }

    fn exclude_terrain_geometry(
        &mut self,
        face: FaceId,
        material: &str,
        triangles: usize,
        reason: &str,
    ) {
        self.exclude_terrain(face, material, triangles, reason);
        if self.normal_maps.active {
            self.normal_maps
                .reject_terrain(face, material, triangles, reason);
        }
    }

    fn exclude_terrain(&mut self, face: FaceId, material: &str, triangles: usize, reason: &str) {
        if !self.lighting.report.active {
            return;
        }
        self.lighting.report.terrain.excluded_surfaces += 1;
        self.lighting.report.terrain.excluded_triangles += triangles;
        self.lighting.report.diagnostic(
            None,
            Some(material),
            &format!(
                "Terrain face {} on brush {}: {reason}",
                face.side, face.solid
            ),
        );
    }

    fn normal_asset(
        &mut self,
        material: MaterialId,
        receiver: NormalReceiver,
    ) -> Result<Option<std::sync::Arc<crate::normal_maps::Asset>>, String> {
        let info = self.library.get(material);
        if receiver == NormalReceiver::Model && (!info.model_bumped || info.tool || info.sky) {
            return Err("Model normal preview requires opaque VertexLitGeneric".into());
        }
        if receiver != NormalReceiver::Terrain && info.terrain_blend_bumped {
            return Err(
                "WorldVertexTransition normal preview requires world or func_detail terrain".into(),
            );
        }
        if receiver != NormalReceiver::Model && info.model_bumped {
            return Err("VertexLitGeneric normal preview requires an authored MDL mesh".into());
        }
        let asset = self.library.normal_map(self.fs, material);
        self.normal_maps.material(&asset.report);
        if let Some(error) = &asset.report.error {
            return Err(error.clone());
        }
        if asset.image.is_none() {
            return Ok(None);
        }
        let info = self.library.get(material);
        if info.base.is_none() || info.missing {
            return Err("Normal preview needs an available base display texture to establish its UV dimensions".into());
        }
        Ok(Some(asset))
    }

    fn add_displacement(
        &mut self,
        material: MaterialId,
        mesh: &DisplacementMesh,
        normals: &[[f64; 3]],
        uv: impl Fn(DVec3) -> [f32; 2],
        tint: [u8; 3],
        source: (FaceId, &source_formats::vmf::Side, bool),
    ) {
        let (face, side, owner_supported) = source;
        let emission = self.surface_emission(material, face, owner_supported);
        let triangles: Vec<[usize; 3]> = mesh.triangles().collect();
        let coordinates = mesh
            .base_positions
            .iter()
            .copied()
            .map(uv)
            .collect::<Vec<_>>();
        let base_transform =
            self.surface_base_transform(material, face, coordinates.iter().copied());
        let detail =
            self.surface_detail(material, face, owner_supported, coordinates.iter().copied());
        self.surface_environment(material, face, false);
        let animation = self.surface_animation(material, face, owner_supported);
        let info = self.library.get(material);
        let tool = info.tool;
        let dimensions = [info.width, info.height];
        let drawn_triangles = if tool && !self.tool_textures {
            0
        } else {
            triangles.len()
        };
        let mut normal_frames = None;
        let mut normal_error = None;
        if self.normal_maps.active && !tool {
            let result = self
                .normal_asset(material, NormalReceiver::Terrain)
                .and_then(|asset| {
                    asset
                        .map(|asset| {
                            if !owner_supported {
                                return Err(
                                    "Terrain normal preview requires world or func_detail terrain"
                                        .into(),
                                );
                            }
                            let expanded = triangles.len() * 3;
                            if self.normal_maps.terrain_vertices + expanded
                                > crate::normal_maps::MAX_TERRAIN_VERTICES
                            {
                                return Err(
                                "Terrain normal preview exceeds its shared expanded-vertex budget"
                                    .into(),
                            );
                            }
                            crate::normal_maps::terrain_frames(
                                mesh,
                                normals,
                                &coordinates,
                                side,
                                dimensions,
                                asset.report.transform,
                            )
                        })
                        .transpose()
                });
            match result {
                Ok(Some(value)) => {
                    normal_frames = Some(value);
                    self.normal_maps.terrain_vertices += triangles.len() * 3;
                    self.normal_maps.terrain.admitted_surfaces += 1;
                    self.normal_maps.terrain.admitted_triangles += drawn_triangles;
                }
                Ok(None) => {}
                Err(error) => {
                    self.normal_maps
                        .reject_terrain(face, &side.material, drawn_triangles, &error);
                    normal_error = Some(error);
                }
            }
        }
        let mut lighting_receiver = 0;
        if self.lighting.report.active {
            let info = self.library.get(material);
            let drawn_triangles = if info.tool && !self.tool_textures {
                0
            } else {
                triangles.len()
            };
            let reason = if !owner_supported {
                Some("Terrain authored lighting requires world or func_detail terrain")
            } else if info.tool {
                Some("Tool and sky terrain are excluded from authored lighting")
            } else if info.base.is_none() || info.missing {
                Some("Terrain authored lighting needs an available base display texture")
            } else {
                info.terrain_lighting_error
                    .as_deref()
                    .or_else(|| {
                        (info.terrain_blend_bumped && normal_frames.is_none()).then(|| {
                            normal_error.as_deref().unwrap_or("Bumped WorldVertexTransition authored lighting requires Normal maps in Detail quality")
                        })
                    })
                    .or_else(|| terrain::receiver_error(mesh, normals))
            };
            if let Some(reason) = reason.map(str::to_owned) {
                self.exclude_terrain(face, &side.material, drawn_triangles, &reason);
            } else {
                lighting_receiver = 1;
                self.lighting.report.terrain.admitted_surfaces += 1;
                self.lighting.report.terrain.admitted_triangles += triangles.len();
            }
        }
        // Texture coordinates come from the undisplaced face, as in the engine,
        // so they're computed from the flat grid position.
        let vertex = |i: usize, tangent| Vertex {
            position: mesh.positions[i].as_vec3().to_array(),
            normal: normals[i].map(|value| value as f32),
            uv: coordinates[i],
            color: [
                tint[0],
                tint[1],
                tint[2],
                mesh.alphas[i].clamp(0.0, 255.0) as u8,
            ],
            lightmap: lightmap_vertex(side, mesh.base_positions[i]),
            smoothing: side.smoothing_groups.unwrap_or(0),
            opacity: 1.0,
            texture_flags: emission | base_transform | detail | animation,
            tangent,
            baked_irradiance: [0.0; 3],
            lighting_receiver,
        };
        if let Some(frames) = normal_frames {
            for (triangle, frame) in triangles.iter().zip(frames) {
                let first = self.vertices.len() as u32;
                for &index in triangle {
                    self.push_vertex(vertex(index, frame));
                }
                self.indices[material].extend(first..first + 3);
            }
        } else {
            let first = self.vertices.len() as u32;
            for index in 0..mesh.positions.len() {
                self.push_vertex(vertex(index, [0.0; 4]));
            }
            self.indices[material].extend(triangles.iter().flatten().map(|&i| first + i as u32));
        }
    }

    fn add_point_entity(&mut self, entity: &Entity, selected: bool, model_guide: bool) {
        let Some(origin) = entity.origin() else {
            return;
        };
        self.stats.point_entities += 1;
        if let Some(instance) = self
            .models
            .iter()
            .find(|instance| instance.id == ObjectId::Entity(entity.id) && instance.has_geometry())
        {
            self.add_model(instance, selected);
            return;
        }
        if self.entity_helpers && self.add_light(entity, selected) {
            return;
        }
        let helper = self.helpers.get(&ObjectId::Entity(entity.id));
        let diagnostic = helper.is_some_and(|helper| !helper.errors.is_empty())
            || self
                .icons
                .get(&entity.id)
                .is_some_and(|icon| icon.asset.material.is_none());
        let [r, g, b] = if diagnostic {
            INVALID_HELPER_COLOR
        } else if selected && self.selection_tint {
            SELECTED_EDGE[..3].try_into().expect("three channels")
        } else {
            helper
                .and_then(|helper| helper.color)
                .or_else(|| {
                    entity
                        .editor
                        .as_ref()
                        .and_then(|editor| editor.get("color"))
                        .and_then(Color::parse)
                        .map(|color| color.0)
                })
                .unwrap_or(DEFAULT_ENTITY_COLOR)
        };
        let material = self.material_editor();
        let (mins, maxs) = helper
            .and_then(|helper| helper.world_bounds(entity))
            .unwrap_or_else(|| map_core::query::point_entity_bounds(origin));
        let center = ((mins + maxs) * 0.5).as_vec3();
        let half = ((maxs - mins) * 0.5).as_vec3();
        let icon = self
            .icons
            .get(&entity.id)
            .filter(|icon| icon.asset.material.is_some())
            .cloned();
        if let Some(icon) = &icon {
            self.displayed_icons.push(icon.clone());
        }
        let resource_guide = icon.is_some() || model_guide || !self.entity_helpers && !diagnostic;
        if resource_guide {
            self.helper_bounds.extend([mins.as_vec3(), maxs.as_vec3()]);
        }
        for normal in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ] {
            // Two in-plane axes chosen so the quad winds clockwise from outside.
            let up = if normal.z.abs() > 0.5 {
                Vec3::X
            } else {
                Vec3::Z
            };
            let right = up.cross(normal);
            let corners = [up - right, up + right, -up + right, -up - right];
            if resource_guide {
                if selected {
                    self.selection_loop(corners.map(|c| center + (normal + c) * half * 1.05));
                }
                continue;
            }
            let first = self.vertices.len() as u32;
            for corner in corners {
                self.push_vertex(Vertex {
                    position: (center + (normal + corner) * half).to_array(),
                    normal: normal.to_array(),
                    uv: [0.0, 0.0],
                    color: [r, g, b, 0],
                    lightmap: [0.0; 3],
                    smoothing: 0,
                    opacity: 1.0,
                    texture_flags: 0,
                    tangent: [0.0; 4],
                    baked_irradiance: [0.0; 3],
                    lighting_receiver: 0,
                });
            }
            self.indices[material].extend([
                first,
                first + 1,
                first + 2,
                first,
                first + 2,
                first + 3,
            ]);
            if selected {
                self.selection_loop(corners.map(|c| center + (normal + c) * half * 1.05));
            }
        }
    }

    fn material_editor(&mut self) -> MaterialId {
        let id = MaterialLibrary::EDITOR;
        if self.indices.len() <= id {
            self.indices.resize_with(id + 1, Vec::new);
        }
        id
    }

    fn helper_triangle(&mut self, points: [Vec3; 3], color: [u8; 3]) {
        let normal = (points[0] - points[1])
            .cross(points[2] - points[1])
            .normalize_or_zero();
        if normal == Vec3::ZERO {
            return;
        }
        let material = self.material_editor();
        let first = self.vertices.len() as u32;
        for point in points {
            self.push_vertex(Vertex {
                position: point.to_array(),
                normal: normal.to_array(),
                uv: [0.0; 2],
                color: [color[0], color[1], color[2], 0],
                lightmap: [0.0; 3],
                smoothing: 0,
                opacity: 1.0,
                texture_flags: 0,
                tangent: [0.0; 4],
                baked_irradiance: [0.0; 3],
                lighting_receiver: 0,
            });
        }
        self.indices[material].extend([first, first + 1, first + 2]);
    }

    fn helper_line(&mut self, from: Vec3, to: Vec3, color: [u8; 4]) {
        self.lines.extend([from, to].map(|point| LineVertex {
            position: point.to_array(),
            color,
        }));
    }

    fn lathe(
        &mut self,
        center: Vec3,
        axis: Vec3,
        profile: &[(f32, f32)],
        color: [u8; 3],
        selected: bool,
    ) {
        let right = axis.any_orthonormal_vector();
        let up = axis.cross(right);
        let point = |radius: f32, height: f32, segment: usize| {
            let angle = segment as f32 * std::f32::consts::TAU / HELPER_SEGMENTS as f32;
            center + axis * height + (right * angle.cos() + up * angle.sin()) * radius
        };
        for pair in profile.windows(2) {
            let [(r0, h0), (r1, h1)] = [pair[0], pair[1]];
            for segment in 0..HELPER_SEGMENTS {
                let a = point(r0, h0, segment);
                let b = point(r0, h0, segment + 1);
                let c = point(r1, h1, segment + 1);
                let d = point(r1, h1, segment);
                self.helper_triangle([a, c, b], color);
                self.helper_triangle([a, d, c], color);
                if selected && segment % 4 == 0 {
                    self.selection_line(a, d, SELECTED_EDGE);
                }
            }
        }
        if selected {
            for &(radius, height) in profile.iter().filter(|(radius, _)| *radius > 0.0) {
                self.selection_loop(
                    (0..HELPER_SEGMENTS).map(|segment| point(radius, height, segment)),
                );
            }
        }
    }

    fn light_cone(&mut self, center: Vec3, direction: Vec3, angle: f32, color: [u8; 4]) {
        let length = 96.0;
        let radius = length * angle.clamp(0.0, 85.0).to_radians().tan();
        let right = direction.any_orthonormal_vector();
        let up = direction.cross(right);
        let point = |segment: usize| {
            let angle = segment as f32 * std::f32::consts::TAU / HELPER_SEGMENTS as f32;
            center + direction * length + (right * angle.cos() + up * angle.sin()) * radius
        };
        for segment in 0..HELPER_SEGMENTS {
            self.helper_line(point(segment), point(segment + 1), color);
            if segment % 4 == 0 {
                self.helper_line(center, point(segment), color);
            }
        }
    }

    fn add_model(&mut self, instance: &crate::models::Instance, selected: bool) {
        self.add_model_with_material(instance, selected, None);
    }

    fn add_model_with_material(
        &mut self,
        instance: &crate::models::Instance,
        selected: bool,
        material_override: Option<MaterialId>,
    ) {
        if !instance.draw_surface {
            let (mins, maxs) = instance.bounds();
            self.helper_bounds.extend([mins.as_vec3(), maxs.as_vec3()]);
            if selected {
                self.add_model_outline(instance);
            }
            self.stats.model_entities += 1;
            return;
        }
        let first = self.vertices.len() as u32;
        let model = &instance.asset.model;
        for vertex in &model.vertices {
            self.push_vertex(Vertex {
                position: instance.point(vertex.position).as_vec3().to_array(),
                normal: (instance.rotation * DVec3::from_array(vertex.normal.map(f64::from)))
                    .normalize_or_zero()
                    .as_vec3()
                    .to_array(),
                uv: vertex.uv,
                color: std::array::from_fn(|channel| {
                    if channel == 3 {
                        0
                    } else if selected && self.selection_tint {
                        ((u16::from(instance.tint[channel])
                            * u16::from(SELECTED_MODEL_TINT[channel])
                            / 255) as u8)
                            .max(SELECTED_MODEL_MIN[channel])
                    } else {
                        instance.tint[channel]
                    }
                }),
                lightmap: [0.0, 0.0, -1.0],
                smoothing: 0,
                opacity: instance.appearance.opacity(),
                texture_flags: u32::from(
                    instance.appearance.mode == source_formats::render_color::TEXTURE,
                ),
                tangent: [0.0; 4],
                baked_irradiance: [0.0; 3],
                lighting_receiver: 0,
            });
        }
        let mut lighting = (self.lighting.report.active && material_override.is_none())
            .then(|| model_lighting::Admission::new(instance));
        let mut normals = (self.normal_maps.active && material_override.is_none())
            .then(model_normals::Admission::default);
        let mut base_transforms_valid = true;
        let materials = instance.asset.materials();
        for part in model.parts_for_body(instance.body) {
            let material_index = model.skins[instance.skin][part.material];
            let material = if let Some(material) = material_override {
                if self.indices.len() <= material {
                    self.indices.resize_with(material + 1, Vec::new);
                }
                material
            } else {
                match &materials {
                    Ok(materials) => self.material(&materials[material_index]),
                    Err(_) => {
                        let id = self.library.unavailable_model();
                        if self.indices.len() <= id {
                            self.indices.resize_with(id + 1, Vec::new);
                        }
                        id
                    }
                }
            };
            if material_override.is_none() {
                match self.base_transform(
                    material,
                    true,
                    part.indices
                        .iter()
                        .map(|&index| model.vertices[index as usize].uv),
                ) {
                    Ok(0) => {}
                    Ok(flags) => {
                        for &index in &part.indices {
                            self.vertices[first as usize + index as usize].texture_flags |= flags;
                        }
                    }
                    Err(message) => {
                        base_transforms_valid = false;
                        let entity = match instance.id {
                            ObjectId::Entity(id) => Some(id),
                            _ => None,
                        };
                        self.issues.push(SceneIssue::Model {
                            entity,
                            message: format!(
                                "{message}. All base transforms on this model are unavailable."
                            ),
                        });
                    }
                }
                let info = self.library.get(material);
                if self.textured
                    && !matches!(info.animation, crate::materials::animation::Layer::Absent)
                {
                    let entity = match instance.id {
                        ObjectId::Entity(id) => Some(id),
                        _ => None,
                    };
                    self.issues.push(SceneIssue::Model {
                        entity,
                        message: format!(
                            "{}: Base frame preview requires a brush or terrain surface",
                            info.name
                        ),
                    });
                }
                if self.textured && !matches!(info.detail, crate::materials::detail::Layer::Absent)
                {
                    let entity = match instance.id {
                        ObjectId::Entity(id) => Some(id),
                        _ => None,
                    };
                    self.issues.push(SceneIssue::Model {
                        entity,
                        message: format!(
                            "{}: Detail layers require a brush or terrain surface",
                            info.name
                        ),
                    });
                }
                if self.textured
                    && !matches!(
                        info.environment,
                        crate::materials::environment::Layer::Absent
                    )
                    && (!info.tool || self.tool_textures)
                {
                    let entity = match instance.id {
                        ObjectId::Entity(id) => Some(id),
                        _ => None,
                    };
                    let reason = match &info.environment {
                        crate::materials::environment::Layer::Unavailable(reason) => reason,
                        _ => "Environment preview requires a flat authored brush face",
                    };
                    self.issues.push(SceneIssue::Model {
                        entity,
                        message: format!("{}: {reason}", info.name),
                    });
                }
                match self.material_emission(material, true) {
                    Ok(0) => {}
                    Ok(flags)
                        if instance.appearance.mode == source_formats::render_color::NORMAL =>
                    {
                        for &index in &part.indices {
                            self.vertices[first as usize + index as usize].texture_flags |=
                                flags | MODEL_SURFACE;
                        }
                    }
                    result => {
                        let message = result.err().unwrap_or_else(|| {
                            format!(
                                "{}: Material emission requires opaque Normal model render mode",
                                self.library.get(material).name
                            )
                        });
                        let entity = match instance.id {
                            ObjectId::Entity(id) => Some(id),
                            _ => None,
                        };
                        self.issues.push(SceneIssue::Model { entity, message });
                    }
                }
            }
            let normal_result = if let Some(normals) = &mut normals
                && normals.begin_part(part, self.library.get(material), self.tool_textures)
            {
                let result = self.prepare_model_normals(instance, part, material, first as usize);
                Some(normals.finish_part(&self.library.get(material).name, result))
            } else {
                None
            };
            if let Some(lighting) = &mut lighting {
                lighting.part(
                    part,
                    self.library.get(material),
                    &self.vertices[first as usize..],
                    self.tool_textures,
                    normal_result,
                );
            }
            let indices = part.indices.iter().map(|&index| first + index);
            if material_override.is_none()
                && instance.appearance.mode == source_formats::render_color::TEXTURE
            {
                self.model_batches.push((material, indices.collect()));
            } else {
                self.indices[material].extend(indices);
            }
            for triangle in part.indices.as_chunks::<3>().0 {
                for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                    self.wire_lines
                        .extend([triangle[a], triangle[b]].map(|index| {
                            LineVertex {
                                position: instance
                                    .point(model.vertices[index as usize].position)
                                    .as_vec3()
                                    .to_array(),
                                color: [90, 125, 150, 255],
                            }
                        }));
                }
            }
        }
        if !base_transforms_valid {
            for vertex in &mut self.vertices[first as usize..] {
                vertex.texture_flags &= !crate::materials::uv::FLAG;
            }
        }
        if selected {
            self.add_model_outline(instance);
        }
        if let Some(normals) = normals {
            normals.finish(
                instance,
                &mut self.vertices[first as usize..],
                &mut self.normal_maps,
            );
        }
        if let Some(lighting) = lighting {
            lighting.finish(
                instance,
                &mut self.vertices[first as usize..],
                &mut self.lighting.report,
            );
        }
        self.stats.model_entities += 1;
    }

    fn prepare_model_normals(
        &mut self,
        instance: &crate::models::Instance,
        part: &source_formats::model::Part,
        material: MaterialId,
        first: usize,
    ) -> Result<(), String> {
        let tangents = model_normals::source_tangents(instance)?;
        let asset = self
            .normal_asset(material, NormalReceiver::Model)?
            .ok_or("Model normal preview needs its declared $bumpmap")?;
        model_normals::prepare_part(
            instance,
            part,
            &mut self.vertices[first..],
            tangents,
            asset.report.transform,
            self.library.get(material).model_phong.is_some(),
        )
    }

    fn add_model_outline(&mut self, instance: &crate::models::Instance) {
        let (mins, maxs) = instance.bounds();
        for x in [mins.x, maxs.x] {
            for y in [mins.y, maxs.y] {
                for z in [mins.z, maxs.z] {
                    let corner = DVec3::new(x, y, z);
                    let sign = (mins + maxs) * 0.5 - corner;
                    for axis in DVec3::AXES {
                        self.selection_line(
                            corner.as_vec3(),
                            (corner + axis * sign.dot(axis).signum() * 4.0).as_vec3(),
                            SELECTED_EDGE,
                        );
                    }
                }
            }
        }
    }

    fn add_light(&mut self, entity: &Entity, selected: bool) -> bool {
        let class = entity.classname().unwrap_or("").to_ascii_lowercase();
        if !map_core::entity_helpers::specialized_light(entity) {
            return false;
        }
        let center = entity.origin().expect("point entity origin").as_vec3();
        let color = light_color(entity);
        let direction = light_direction(entity, &self.light_targets);
        if class == "light_spot" || class == "light_dynamic" {
            self.lathe(
                center,
                direction,
                &[
                    (0.0, -10.0),
                    (5.0, -10.0),
                    (5.0, -4.0),
                    (11.0, 4.0),
                    (11.0, 7.0),
                ],
                [110, 118, 132],
                selected,
            );
            self.lathe(
                center,
                direction,
                &[(11.0, 7.0), (0.0, 7.0)],
                color,
                selected,
            );
            if selected {
                self.light_cone(
                    center,
                    direction,
                    light_number(entity, "_cone", 45.0),
                    SELECTED_EDGE,
                );
                self.light_cone(
                    center,
                    direction,
                    light_number(entity, "_inner_cone", 30.0),
                    [color[0], color[1], color[2], 255],
                );
            }
        } else if class == "light_environment" {
            self.lathe(
                center,
                Vec3::Z,
                &[(0.0, -9.0), (6.0, -6.0), (9.0, 0.0), (6.0, 6.0), (0.0, 9.0)],
                color,
                selected,
            );
            for ray in [
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z,
            ] {
                self.helper_line(
                    center + ray * 12.0,
                    center + ray * 18.0,
                    [color[0], color[1], color[2], 255],
                );
            }
            if selected {
                let tip = center + direction * 96.0;
                self.helper_line(center, tip, SELECTED_EDGE);
                let right = direction.any_orthonormal_vector();
                for sign in [-1.0, 1.0] {
                    self.helper_line(
                        tip,
                        tip - direction * 12.0 + right * sign * 6.0,
                        SELECTED_EDGE,
                    );
                }
            }
        }
        true
    }
}

fn light_number(entity: &Entity, key: &str, default: f32) -> f32 {
    entity
        .properties
        .get(key)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn light_color(entity: &Entity) -> [u8; 3] {
    if entity.properties.get("_light").is_some_and(|value| {
        map_core::light::color_parameter(entity.classname().unwrap_or_default(), "_light", value)
            .is_some()
    }) {
        return [110, 118, 132];
    }
    let color = entity.properties.get("_light").and_then(|value| {
        source_formats::fgd::vector(value, 4).or_else(|| source_formats::fgd::vector(value, 3))
    });
    color
        .filter(|color| color[..3].iter().all(|value| (0.0..=255.0).contains(value)))
        .map_or([255, 240, 200], |values| {
            std::array::from_fn(|i| values[i].round() as u8)
        })
}

fn light_direction(entity: &Entity, targets: &map_core::light::Targets<'_>) -> Vec3 {
    map_core::light::target(entity.properties.get("target"), targets)
        .and_then(|target| {
            map_core::light::direction(
                entity.properties.get("angles"),
                entity.properties.get("angle"),
                entity.properties.get("pitch"),
                entity.origin().unwrap_or(DVec3::ZERO),
                target,
            )
        })
        .unwrap_or(DVec3::X)
        .as_vec3()
}

impl SceneMesh {
    pub fn append_reference_outlines(
        &mut self,
        mesh: &crate::references::Mesh,
        owners: &BTreeSet<u32>,
        selected: &HashSet<ObjectId>,
    ) -> Result<(), String> {
        let selected_brushes = mesh
            .brushes
            .iter()
            .filter(|brush| {
                owners.contains(&brush.owner) && selected.contains(&ObjectId::Entity(brush.owner))
            })
            .collect::<Vec<_>>();
        let mut count = 0usize;
        for brush in &selected_brushes {
            if brush.edges.start > brush.edges.end
                || brush.edges.end as usize > mesh.edges.len()
                || brush.edges.len() % 2 != 0
            {
                return Err("Reference selection edge range is invalid".into());
            }
            count = count
                .checked_add(brush.edges.len())
                .ok_or("Reference selection edge count overflow")?;
        }
        if !self.reference_lines.is_empty()
            || count > crate::references::MAX_LINES
            || self
                .reference_lines
                .len()
                .checked_add(count)
                .is_none_or(|count| count > u32::MAX as usize)
        {
            return Err(
                "Reference selection exceeds its line budget or was already appended".into(),
            );
        }
        self.reference_lines
            .try_reserve_exact(count)
            .map_err(|error| format!("Reference selection lines unavailable: {error}"))?;
        self.reference_selection_ranges
            .try_reserve_exact(selected_brushes.len())
            .map_err(|error| format!("Reference selection ranges unavailable: {error}"))?;
        for brush in selected_brushes {
            let first = self.reference_lines.len() as u32;
            self.reference_lines.extend(
                mesh.edges[brush.edges.start as usize..brush.edges.end as usize]
                    .iter()
                    .map(|line| LineVertex {
                        position: line.position,
                        color: [255, 167, 94, 255],
                    }),
            );
            let end = self.reference_lines.len() as u32;
            if end > first {
                self.reference_selection_ranges.push(first..end);
            }
        }
        Ok(())
    }

    /// Builds the visible geometry, loading new materials into `library`.
    /// Objects in `selected` are tinted and outlined.
    pub fn build(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selected: &HashSet<ObjectId>,
    ) -> Self {
        Self::build_with_faces(vmf, fs, library, selected, &HashSet::new())
    }

    pub fn build_with_faces(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selected: &HashSet<ObjectId>,
        selected_faces: &HashSet<FaceId>,
    ) -> Self {
        Self::build_with_face_overlays(vmf, fs, library, selected, selected_faces, &[])
    }

    pub fn build_with_face_overlays(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selected: &HashSet<ObjectId>,
        selected_faces: &HashSet<FaceId>,
        overlays: &[FaceOverlay],
    ) -> Self {
        Self::build_with_models(vmf, fs, library, selected, selected_faces, overlays, &[])
    }

    pub fn build_with_models(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selected: &HashSet<ObjectId>,
        selected_faces: &HashSet<FaceId>,
        overlays: &[FaceOverlay],
        models: &[crate::models::Instance],
    ) -> Self {
        Self::build_with_models_filtered(
            vmf,
            fs,
            library,
            (selected, selected_faces),
            overlays,
            models,
            |_| true,
        )
    }

    pub fn build_with_models_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        models: &[crate::models::Instance],
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_helpers_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            (models, &BTreeMap::new()),
            include,
        )
    }

    pub fn build_with_helpers_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_helpers_and_icons_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            &BTreeMap::new(),
            include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_helpers_and_icons_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        icons: &BTreeMap<u32, crate::icons::Instance>,
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_visual_helpers_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            (icons, &[]),
            include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_visual_helpers_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        let skybox = vmf
            .world
            .properties
            .get("skyname")
            .filter(|name| !name.trim().is_empty())
            .map(|name| library.skybox(fs, name));
        Self::build_with_visual_helpers_and_sky_filtered(
            vmf, fs, library, selection, overlays, entities, guides, skybox, include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_visual_helpers_and_sky_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_normal_maps_filtered(
            vmf, fs, library, selection, overlays, entities, guides, skybox, false, include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_normal_maps_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        normal_maps: bool,
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_previews_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            guides,
            skybox,
            true,
            normal_maps,
            false,
            include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_previews_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        tool_textures: bool,
        normal_maps: bool,
        lighting: bool,
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_reference_models_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            guides,
            skybox,
            tool_textures,
            true,
            normal_maps,
            lighting,
            &[],
            include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_reference_models_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        tool_textures: bool,
        selection_tint: bool,
        normal_maps: bool,
        lighting: bool,
        reference_models: &[crate::models::Instance],
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_reference_models_and_surfaces_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            guides,
            skybox,
            tool_textures,
            selection_tint,
            normal_maps,
            lighting,
            reference_models,
            true,
            true,
            include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_reference_models_and_surfaces_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        tool_textures: bool,
        selection_tint: bool,
        normal_maps: bool,
        lighting: bool,
        reference_models: &[crate::models::Instance],
        authored_surfaces: bool,
        entity_helpers: bool,
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        Self::build_with_terrain_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            guides,
            skybox,
            tool_textures,
            selection_tint,
            normal_maps,
            lighting,
            false,
            reference_models,
            authored_surfaces,
            entity_helpers,
            terrain::prepare(vmf),
            None,
            include,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_build_with_reference_terrain_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        tool_textures: bool,
        selection_tint: bool,
        normal_maps: bool,
        lighting: bool,
        textured: bool,
        reference_models: &[crate::models::Instance],
        authored_surfaces: bool,
        entity_helpers: bool,
        terrain_normals: &BTreeMap<FaceId, TerrainNormalOverride>,
        projected_overlays: Option<OverlayPreview>,
        include: impl Fn(ObjectId) -> bool,
    ) -> Result<Self, String> {
        if !authored_surfaces && !terrain_normals.is_empty() {
            return Err("Terrain normal overrides require authored root surfaces".into());
        }
        let mut terrain = terrain::prepare(vmf);
        terrain.apply_normal_overrides(vmf, terrain_normals)?;
        Ok(Self::build_with_terrain_filtered(
            vmf,
            fs,
            library,
            selection,
            overlays,
            entities,
            guides,
            skybox,
            tool_textures,
            selection_tint,
            normal_maps,
            lighting,
            textured,
            reference_models,
            authored_surfaces,
            entity_helpers,
            terrain,
            projected_overlays,
            include,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_with_terrain_filtered(
        vmf: &Vmf,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        selection: (&HashSet<ObjectId>, &HashSet<FaceId>),
        overlays: &[FaceOverlay],
        entities: (
            &[crate::models::Instance],
            &BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
        ),
        guides: (
            &BTreeMap<u32, crate::icons::Instance>,
            &[crate::models::Instance],
        ),
        skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
        tool_textures: bool,
        selection_tint: bool,
        normal_maps: bool,
        lighting: bool,
        textured: bool,
        reference_models: &[crate::models::Instance],
        authored_surfaces: bool,
        entity_helpers: bool,
        terrain: terrain::Prepared,
        projected_overlays: Option<OverlayPreview>,
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        let (icons, model_guides) = guides;
        let (models, helpers) = entities;
        let (selected, selected_faces) = selection;
        let terrain_issues = terrain.issues.into_iter().fold(
            HashMap::<u32, Vec<TerrainIssue>>::new(),
            |mut issues, issue| {
                issues.entry(issue.face.solid).or_default().push(issue);
                issues
            },
        );
        let mut issues = SceneIssues::default();
        let projections =
            projected_overlays.map_or_else(projected_overlays::Projections::default, |preview| {
                projected_overlays::Projections::prepare(vmf, preview.draft, &include, &mut issues)
            });
        let mut builder = Builder {
            authored_surfaces,
            water: None,
            water_face: None,
            entity_helpers,
            issues,
            terrain_issues,
            tool_textures,
            selection_tint,
            lighting: crate::lighting::Preview {
                report: crate::lighting::Report {
                    requested: lighting,
                    active: lighting,
                    ..Default::default()
                },
                ..Default::default()
            },
            normal_maps: crate::normal_maps::Report {
                requested: normal_maps,
                active: normal_maps,
                ..Default::default()
            },
            textured,
            icons,
            displayed_icons: Vec::new(),
            helper_bounds: Vec::new(),
            models,
            helpers,
            overlays,
            projections,
            selected_faces,
            fs,
            library,
            vertices: Vec::new(),
            indices: Vec::new(),
            model_batches: Vec::new(),
            lines: Vec::new(),
            selection_outline_ranges: Vec::new(),
            wire_lines: Vec::new(),
            tool_wire_lines: Vec::new(),
            stats: SceneStats::default(),
            displacements: terrain.meshes,
            terrain_geometry: terrain.geometries,
            unshaded_terrain: terrain.unshaded,
            light_targets: {
                let mut targets = map_core::light::Targets::new();
                for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
                    if let (Some(name), Some(origin)) = (
                        entity
                            .properties
                            .get("targetname")
                            .filter(|name| !name.is_empty()),
                        entity.origin(),
                    ) {
                        targets.entry(name).or_default().push(origin);
                    }
                }
                targets
            },
        };
        for solid in &vmf.world.solids {
            let id = ObjectId::Solid(solid.id);
            if !solid.hidden && include(id) {
                builder.add_solid(solid, selected.contains(&id), SolidOwner::World);
            } else {
                builder.projections.hide_sides(solid);
            }
        }
        for entity in &vmf.entities {
            if entity.hidden || !include(ObjectId::Entity(entity.id)) {
                for solid in &entity.solids {
                    builder.projections.hide_sides(solid);
                }
                continue;
            }
            let is_selected = selected.contains(&ObjectId::Entity(entity.id));
            if entity.solids.is_empty() {
                let model_guide = model_guides
                    .iter()
                    .any(|guide| guide.id == ObjectId::Entity(entity.id));
                builder.add_point_entity(entity, is_selected, model_guide);
            }
            for solid in &entity.solids {
                if solid.hidden {
                    builder.projections.hide_sides(solid);
                    continue;
                }
                builder.add_solid(
                    solid,
                    is_selected || selected.contains(&ObjectId::Solid(solid.id)),
                    if entity.classname() == Some("func_detail") {
                        SolidOwner::Detail
                    } else {
                        SolidOwner::Entity
                    },
                );
            }
        }

        let lighting_active = builder.lighting.report.active;
        builder.lighting.report.active = false;
        let normal_maps_active = builder.normal_maps.active;
        builder.normal_maps.active = false;
        builder.textured = false;
        for instance in reference_models {
            builder.add_model(instance, selected.contains(&instance.id));
        }
        builder.lighting.report.active = lighting_active;
        builder.normal_maps.active = normal_maps_active;
        let (mins, maxs) = builder
            .vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .chain(builder.helper_bounds.iter().copied())
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)));
        for instance in model_guides.iter().filter(|instance| include(instance.id)) {
            builder.add_model_with_material(instance, false, Some(MaterialLibrary::EDITOR));
            builder.stats.model_entities -= 1;
            builder.stats.fgd_model_helpers += 1;
        }
        let mut indices = Vec::with_capacity(builder.indices.iter().map(Vec::len).sum());
        let mut batches = Vec::new();
        let mut stats = builder.stats;
        let mut missing_materials = BTreeSet::new();
        for (material, list) in builder.indices.into_iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            if builder.library.get(material).missing {
                missing_materials.insert(material);
            }
            batches.push(Batch {
                material,
                first_index: indices.len() as u32,
                index_count: list.len() as u32,
                rope: false,
                model_translucent: false,
            });
            indices.extend(list);
        }
        for (material, list) in builder.model_batches {
            if list.is_empty() {
                continue;
            }
            if builder.library.get(material).missing {
                missing_materials.insert(material);
            }
            batches.push(Batch {
                material,
                first_index: indices.len() as u32,
                index_count: list.len() as u32,
                rope: false,
                model_translucent: true,
            });
            indices.extend(list);
        }
        stats.missing_materials = missing_materials.len();
        let first_overlay = batches.len();
        builder.projections.finish(
            builder.fs,
            builder.library,
            (&mut builder.vertices, &mut indices, &mut batches),
            &mut builder.issues,
        );
        let mut mesh = Self {
            water: None,
            projected_overlay_batches: first_overlay..batches.len(),
            baked_faces: None,
            baked_face_materials: Vec::new(),
            baked_face_batches: 0..0,
            baked_face_visible_surfaces: 0,
            detail_billboards: Vec::new(),
            detail_parts: Vec::new(),
            compiled_detail_hdr: None,
            compiled_detail_hdr_ordinals: Vec::new(),
            compiled_detail_hdr_draw_issue: None,
            reference_lines: Vec::new(),
            reference_selection_ranges: Vec::new(),
            lighting: builder.lighting,
            normal_maps: builder.normal_maps,
            icons: builder.displayed_icons,
            reference_images: Vec::new(),
            issues: builder.issues,
            skybox,
            sky3d: None,
            vertices: builder.vertices,
            indices,
            batches,
            lines: builder.lines,
            selection_outline_ranges: builder.selection_outline_ranges,
            leak_lines: Vec::new(),
            wire_lines: builder.wire_lines,
            tool_wire_lines: builder.tool_wire_lines,
            rope_wire_lines: Vec::new(),
            rope_lines: Vec::new(),
            mins,
            maxs,
            stats,
        };
        if let Some(pool) = builder.water {
            match pool.captures(&mesh, builder.fs, builder.library) {
                Ok(captures) => mesh.water = Some(captures),
                Err(error) => mesh.issues.push(pool.issue(error)),
            }
        }
        mesh
    }

    pub fn issue_summary(&self) -> String {
        self.issues.summary()
    }

    pub fn guide_line_ranges(&self) -> Vec<std::ops::Range<u32>> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for outline in &self.selection_outline_ranges {
            if start < outline.start {
                ranges.push(start..outline.start);
            }
            start = outline.end;
        }
        let end = self.lines.len() as u32;
        if start < end {
            ranges.push(start..end);
        }
        ranges
    }

    pub fn add_guide_line(&mut self, start: DVec3, end: DVec3, color: [u8; 4]) {
        self.lines.extend([start, end].map(|point| LineVertex {
            position: point.as_vec3().to_array(),
            color,
        }));
    }

    pub fn add_bounds_lines(&mut self, mins: DVec3, maxs: DVec3, color: [u8; 4]) {
        let corners: [DVec3; 8] = std::array::from_fn(|bits| {
            DVec3::new(
                if bits & 1 == 0 { mins.x } else { maxs.x },
                if bits & 2 == 0 { mins.y } else { maxs.y },
                if bits & 4 == 0 { mins.z } else { maxs.z },
            )
        });
        for index in 0..8 {
            for bit in [1, 2, 4] {
                let other = index ^ bit;
                if index < other {
                    self.add_guide_line(corners[index], corners[other], color);
                }
            }
        }
    }

    pub fn set_leak_path(&mut self, points: &[DVec3]) {
        self.leak_lines = points
            .windows(2)
            .flat_map(|pair| pair.iter())
            .map(|p| LineVertex {
                position: p.as_vec3().to_array(),
                color: [255, 32, 32, 255],
            })
            .collect();
    }
}
