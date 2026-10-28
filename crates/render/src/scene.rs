//! Converts a VMF into GPU-ready triangle batches, one per material.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::{DVec3, Vec3};
use map_core::surface::FaceId;
use map_core::{BrushGeometry, DisplacementMesh, ObjectId, texel_uv};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Color, Entity, Solid, Vmf};

use crate::materials::{MaterialId, MaterialLibrary};

mod model_lighting;

mod terrain;

const HELPER_SEGMENTS: usize = 16;
const MAX_PREVIEW_ISSUES: usize = 64;

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
    pub texture_alpha: u32,
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
    pub baked_faces: Option<std::sync::Arc<crate::compiled_faces::LightingData>>,
    pub baked_face_materials:
        Vec<Option<std::sync::Arc<crate::compiled_faces::materials::Material>>>,
    pub baked_face_batches: std::ops::Range<usize>,
    pub baked_face_visible_surfaces: usize,
    pub detail_billboards: Vec<crate::compiled_details::Billboard>,
    pub detail_parts: Vec<crate::compiled_details::Part>,
    pub reference_lines: Vec<LineVertex>,
    pub reference_selection_ranges: Vec<std::ops::Range<u32>>,
    pub lighting: crate::lighting::Preview,
    pub normal_maps: crate::normal_maps::Report,
    pub issues: SceneIssues,
    pub skybox: Option<std::sync::Arc<crate::sky::Skybox>>,
    pub sky3d: Option<std::sync::Arc<crate::sky3d::Scene>>,
    pub icons: Vec<crate::icons::Instance>,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// Ordered editor lines; selection ranges exclude diagnostic and tool guides.
    pub lines: Vec<LineVertex>,
    pub selection_outline_ranges: Vec<std::ops::Range<u32>>,
    pub leak_lines: Vec<LineVertex>,
    pub wire_lines: Vec<LineVertex>,
    pub tool_wire_lines: Vec<LineVertex>,
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
    Brush { solid: u32, message: String },
    Terrain(TerrainIssue),
}

impl SceneIssue {
    pub fn solid(&self) -> u32 {
        match self {
            Self::Brush { solid, .. } => *solid,
            Self::Terrain(issue) => issue.face.solid,
        }
    }

    pub fn face(&self) -> Option<FaceId> {
        match self {
            Self::Brush { .. } => None,
            Self::Terrain(issue) => Some(issue.face),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Brush { message, .. } => message,
            Self::Terrain(issue) => &issue.message,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Brush { solid, message } => format!("Brush {solid}: {message}"),
            Self::Terrain(issue) => format!(
                "Terrain face {} on brush {}: {}",
                issue.face.side, issue.face.solid, issue.message
            ),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SceneIssues {
    pub entries: Vec<SceneIssue>,
    pub omitted: usize,
}

impl SceneIssues {
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

struct Builder<'a> {
    authored_surfaces: bool,
    issues: SceneIssues,
    tool_textures: bool,
    selection_tint: bool,
    lighting: crate::lighting::Preview,
    normal_maps: crate::normal_maps::Report,
    icons: &'a BTreeMap<u32, crate::icons::Instance>,
    displayed_icons: Vec<crate::icons::Instance>,
    helper_bounds: Vec<Vec3>,
    models: &'a [crate::models::Instance],
    helpers: &'a BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
    overlays: &'a [FaceOverlay],
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

    fn add_solid(&mut self, solid: &Solid, selected: bool, terrain_owner_supported: bool) {
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
                    self.exclude_terrain(
                        FaceId {
                            solid: solid.id,
                            side: side.id,
                        },
                        &side.material,
                        0,
                        "No prepared terrain geometry; authored lighting excluded",
                    );
                }
                return;
            }
        };
        // Hammer draws only the displaced faces of a brush with displacements.
        let has_displacement = solid.sides.iter().any(|s| s.displacement.is_some());
        for face in &brush.faces {
            let side = &solid.sides[face.side];
            let selected = selected
                || self.selected_faces.contains(&FaceId {
                    solid: solid.id,
                    side: side.id,
                });
            let material = self.material(&side.material);
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
                    let id = FaceId {
                        solid: solid.id,
                        side: side.id,
                    };
                    let displacement = self.displacements.remove(&id);
                    let Some(Displacement { mesh, normals }) = displacement else {
                        self.exclude_terrain(
                            id,
                            &side.material,
                            0,
                            "No prepared shaded terrain surface; ordinary outline retained (see terrain issues)",
                        );
                        if let Some(mesh) = self.unshaded_terrain.remove(&id) {
                            self.lines.extend(displacement_lines(&mesh, SELECTED_EDGE));
                        } else {
                            self.push_loop(face.vertices.iter().map(|point| point.as_vec3()));
                        }
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
                        self.add_polygon(
                            side,
                            &face.vertices,
                            face.plane.normal,
                            &side.material,
                            tint,
                            selected,
                        );
                    } else {
                        for overlay in overlays {
                            let mut projected = side.clone();
                            if let Some(projection) = &overlay.projection {
                                projected.u_axis = projection.u;
                                projected.v_axis = projection.v;
                                projected.rotation = projection.rotation;
                            }
                            self.add_polygon(
                                &projected,
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

    fn add_polygon(
        &mut self,
        side: &source_formats::vmf::Side,
        points: &[DVec3],
        normal: DVec3,
        name: &str,
        tint: [u8; 3],
        selected: bool,
    ) {
        let material = self.material(name);
        let info = self.library.get(material);
        let tool = info.tool;
        let size = glam::DVec2::new(f64::from(info.width), f64::from(info.height));
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
        let mut tangent = [0.0; 4];
        if self.normal_maps.active && !tool {
            let asset = self.library.normal_map(self.fs, material);
            if !self
                .normal_maps
                .materials
                .iter()
                .any(|report| report.material == asset.report.material)
            {
                if self.normal_maps.materials.len() < crate::normal_maps::MAX_ASSETS {
                    self.normal_maps.materials.push(asset.report.clone());
                } else {
                    self.normal_maps.materials_truncated = true;
                }
            }
            let result = if asset.image.is_some() && missing_base {
                Err("Normal preview needs an available base display texture to establish its UV dimensions".into())
            } else if asset.image.is_some() {
                crate::normal_maps::tangent(
                    side,
                    normal,
                    points,
                    dimensions,
                    asset.report.transform,
                )
                .map(Some)
            } else if let Some(error) = &asset.report.error {
                Err(error.clone())
            } else {
                Ok(None)
            };
            match result {
                Ok(Some(value)) => {
                    tangent = value;
                    self.normal_maps.applied_faces += 1;
                }
                Ok(None) => {}
                Err(error) => {
                    self.normal_maps.rejected_faces += 1;
                    if self.normal_maps.errors.len() < crate::normal_maps::MAX_ASSETS {
                        self.normal_maps.errors.push(format!(
                            "Face {} · {}: {error}",
                            side.id, asset.report.material
                        ));
                    } else {
                        self.normal_maps.errors_truncated = true;
                    }
                }
            }
        }
        self.push_wire_loop(points.iter().map(|point| point.as_vec3()), tool);
        let first = self.vertices.len() as u32;
        for &point in points {
            self.push_vertex(Vertex {
                position: point.as_vec3().to_array(),
                normal: normal.as_vec3().to_array(),
                uv: (texel_uv(side, point) / size).as_vec2().to_array(),
                color: [tint[0], tint[1], tint[2], 0],
                lightmap: lightmap_vertex(side, point),
                smoothing: side.smoothing_groups.unwrap_or(0),
                opacity: 1.0,
                texture_alpha: 0,
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
        let triangles: Vec<[usize; 3]> = mesh.triangles().collect();
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
                    .or_else(|| terrain::lighting_error(mesh, normals))
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
        let first = self.vertices.len() as u32;
        for (i, &p) in mesh.positions.iter().enumerate() {
            let alpha = mesh.alphas[i].clamp(0.0, 255.0) as u8;
            self.push_vertex(Vertex {
                position: p.as_vec3().to_array(),
                normal: normals[i].map(|value| value as f32),
                uv: uv(mesh.base_positions[i]),
                color: [tint[0], tint[1], tint[2], alpha],
                lightmap: lightmap_vertex(side, mesh.base_positions[i]),
                smoothing: side.smoothing_groups.unwrap_or(0),
                opacity: 1.0,
                texture_alpha: 0,
                tangent: [0.0; 4],
                baked_irradiance: [0.0; 3],
                lighting_receiver,
            });
        }
        self.indices[material].extend(triangles.iter().flatten().map(|&i| first + i as u32));
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
        if self.add_light(entity, selected) {
            return;
        }
        let helper = self.helpers.get(&ObjectId::Entity(entity.id));
        let [r, g, b] = if selected && self.selection_tint {
            SELECTED_EDGE[..3].try_into().expect("three channels")
        } else if helper.is_some_and(|helper| !helper.errors.is_empty())
            || self
                .icons
                .get(&entity.id)
                .is_some_and(|icon| icon.asset.material.is_none())
        {
            INVALID_HELPER_COLOR
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
        let resource_guide = icon.is_some() || model_guide;
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
                    texture_alpha: 0,
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
                texture_alpha: 0,
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
                texture_alpha: u32::from(
                    instance.appearance.mode == source_formats::render_color::TEXTURE,
                ),
                tangent: [0.0; 4],
                baked_irradiance: [0.0; 3],
                lighting_receiver: 0,
            });
        }
        let mut lighting = (self.lighting.report.active && material_override.is_none())
            .then(|| model_lighting::Admission::new(instance));
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
            if let Some(lighting) = &mut lighting {
                lighting.part(
                    part,
                    self.library.get(material),
                    &self.vertices[first as usize..],
                    self.tool_textures,
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
        if selected {
            self.add_model_outline(instance);
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
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        let (icons, model_guides) = guides;
        let (models, helpers) = entities;
        let (selected, selected_faces) = selection;
        let terrain = terrain::prepare(vmf);
        let mut issues = SceneIssues::default();
        for issue in terrain.issues {
            issues.push(SceneIssue::Terrain(issue));
        }
        let mut builder = Builder {
            authored_surfaces,
            issues,
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
            icons,
            displayed_icons: Vec::new(),
            helper_bounds: Vec::new(),
            models,
            helpers,
            overlays,
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
        for solid in vmf
            .world
            .solids
            .iter()
            .filter(|s| !s.hidden && include(ObjectId::Solid(s.id)))
        {
            builder.add_solid(solid, selected.contains(&ObjectId::Solid(solid.id)), true);
        }
        for entity in vmf
            .entities
            .iter()
            .filter(|e| !e.hidden && include(ObjectId::Entity(e.id)))
        {
            let is_selected = selected.contains(&ObjectId::Entity(entity.id));
            if entity.solids.is_empty() {
                let model_guide = model_guides
                    .iter()
                    .any(|guide| guide.id == ObjectId::Entity(entity.id));
                builder.add_point_entity(entity, is_selected, model_guide);
            }
            for solid in entity.solids.iter().filter(|s| !s.hidden) {
                builder.add_solid(
                    solid,
                    is_selected,
                    entity.classname() == Some("func_detail"),
                );
            }
        }

        let lighting_active = builder.lighting.report.active;
        builder.lighting.report.active = false;
        for instance in reference_models {
            builder.add_model(instance, selected.contains(&instance.id));
        }
        builder.lighting.report.active = lighting_active;
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
        let mut mesh = Self {
            baked_faces: None,
            baked_face_materials: Vec::new(),
            baked_face_batches: 0..0,
            baked_face_visible_surfaces: 0,
            detail_billboards: Vec::new(),
            detail_parts: Vec::new(),
            reference_lines: Vec::new(),
            reference_selection_ranges: Vec::new(),
            lighting: builder.lighting,
            normal_maps: builder.normal_maps,
            icons: builder.displayed_icons,
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
            rope_lines: Vec::new(),
            mins,
            maxs,
            stats,
        };
        let ropes = crate::ropes::prepare(vmf, include);
        mesh.add_rope_preview(&ropes, selected, selection_tint);
        mesh
    }

    fn add_rope_preview(
        &mut self,
        preview: &crate::ropes::Preview,
        selected: &HashSet<ObjectId>,
        selection_tint: bool,
    ) {
        let first_index = self.indices.len() as u32;
        for link in &preview.links {
            let selected = selected.contains(&ObjectId::Entity(link.source));
            let color = if selected && selection_tint {
                SELECTED_TINT
            } else {
                [170, 156, 125]
            };
            for points in link.triangles() {
                let normal = (points[0] - points[1])
                    .cross(points[2] - points[1])
                    .normalize_or_zero();
                for point in points {
                    self.indices.push(self.vertices.len() as u32);
                    self.vertices.push(Vertex {
                        position: point.as_vec3().to_array(),
                        normal: normal.as_vec3().to_array(),
                        uv: [0.0; 2],
                        color: [color[0], color[1], color[2], 0],
                        lightmap: [0.0; 3],
                        smoothing: 0,
                        opacity: 1.0,
                        texture_alpha: 0,
                        tangent: [0.0; 4],
                        baked_irradiance: [0.0; 3],
                        lighting_receiver: 0,
                    });
                    self.mins = self.mins.min(point.as_vec3());
                    self.maxs = self.maxs.max(point.as_vec3());
                }
            }
            if selected {
                self.rope_lines
                    .extend(link.points.windows(2).flat_map(|pair| {
                        pair.iter().map(|point| LineVertex {
                            position: point.as_vec3().to_array(),
                            color: SELECTED_EDGE,
                        })
                    }));
            }
        }
        let index_count = self.indices.len() as u32 - first_index;
        if index_count > 0 {
            self.batches.push(Batch {
                material: MaterialLibrary::EDITOR,
                first_index,
                index_count,
                rope: true,
                model_translucent: false,
            });
        }
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
                    for point in [corners[index], corners[other]] {
                        self.lines.push(LineVertex {
                            position: point.as_vec3().to_array(),
                            color,
                        });
                    }
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
