//! Converts a VMF into GPU-ready triangle batches, one per material.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::{DVec3, Vec3};
use map_core::surface::FaceId;
use map_core::{BrushGeometry, DisplacementMesh, ObjectId, texel_uv};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Color, Entity, Solid, Vmf};

use crate::materials::{MaterialId, MaterialLibrary};

mod model_lighting;
#[cfg(test)]
mod selection_outline_tests;
mod terrain;

const HELPER_SEGMENTS: usize = 16;

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
    pub opacity: f32,
    pub texture_alpha: u32,
    pub tangent: [f32; 4],
    pub lighting_receiver: u32,
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
    pub reference_lines: Vec<LineVertex>,
    pub reference_selection_ranges: Vec<std::ops::Range<u32>>,
    pub lighting: crate::lighting::Preview,
    pub normal_maps: crate::normal_maps::Report,
    pub issues: Vec<SceneIssue>,
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
pub struct SceneIssue {
    pub face: FaceId,
    pub message: String,
}

impl SceneIssue {
    pub fn description(&self) -> String {
        format!(
            "Terrain face {} on brush {}: {}",
            self.face.side, self.face.solid, self.message
        )
    }
}

pub fn terrain_issues(vmf: &Vmf) -> Vec<SceneIssue> {
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
    tool_textures: bool,
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
        let Ok(brush) = geometry else {
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
            let tint = if selected { SELECTED_TINT } else { [255; 3] };
            let material = self.material(&side.material);
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
        let [r, g, b] = if selected {
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
                    } else if selected {
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
        if let Some(lighting) = lighting {
            lighting.finish(
                instance,
                &mut self.vertices[first as usize..],
                &mut self.lighting.report,
            );
        }
        self.stats.model_entities += 1;
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
        normal_maps: bool,
        lighting: bool,
        reference_models: &[crate::models::Instance],
        include: impl Fn(ObjectId) -> bool,
    ) -> Self {
        let (icons, model_guides) = guides;
        let (models, helpers) = entities;
        let (selected, selected_faces) = selection;
        let terrain = terrain::prepare(vmf);
        let mut builder = Builder {
            tool_textures,
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
            reference_lines: Vec::new(),
            reference_selection_ranges: Vec::new(),
            lighting: builder.lighting,
            normal_maps: builder.normal_maps,
            icons: builder.displayed_icons,
            issues: terrain.issues,
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
        mesh.add_rope_preview(&ropes, selected);
        mesh
    }

    fn add_rope_preview(&mut self, preview: &crate::ropes::Preview, selected: &HashSet<ObjectId>) {
        let first_index = self.indices.len() as u32;
        for link in &preview.links {
            let selected = selected.contains(&ObjectId::Entity(link.source));
            let color = if selected {
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
        self.issues.first().map_or_else(String::new, |issue| {
            let mut summary = issue.description();
            if self.issues.len() > 1 {
                summary.push_str(&format!(" ({} more preview issues)", self.issues.len() - 1));
            }
            summary
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_point_emission_preserves_external_light_targets_and_duplicate_resolution() {
        let mut vmf = map_core::Document::empty().vmf().clone();
        let mut light = Entity::new(10, "light_spot");
        light.properties.set("origin", "0 0 0");
        light.properties.set("target", "outside_target");
        let mut target = Entity::new(11, "info_target");
        target.properties.set("origin", "100 200 300");
        target.properties.set("targetname", "outside_target");
        vmf.entities.extend([light, target]);
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        for duplicate in [false, true] {
            if duplicate {
                let mut other = vmf.entities[1].clone();
                other.id = 12;
                other.properties.set("origin", "-200 100 50");
                vmf.entities.push(other);
            }
            let selected = HashSet::from([ObjectId::Entity(10)]);
            let mut materials = MaterialLibrary::default();
            let complete = SceneMesh::build(&vmf, &fs, &mut materials, &selected);
            let filtered = SceneMesh::build_with_models_filtered(
                &vmf,
                &fs,
                &mut materials,
                (&selected, &HashSet::new()),
                &[],
                &[],
                |id| id == ObjectId::Entity(10),
            );
            assert_eq!(filtered.stats.point_entities, 1);
            assert_eq!(filtered.lines, complete.lines);
            assert_eq!(
                filtered.vertices,
                &complete.vertices[..filtered.vertices.len()]
            );
            let empty = SceneMesh::build_with_models_filtered(
                &vmf,
                &fs,
                &mut materials,
                (&selected, &HashSet::new()),
                &[],
                &[],
                |_| false,
            );
            assert!(empty.vertices.is_empty());
            assert!(empty.lines.is_empty());
            assert!(empty.wire_lines.is_empty());
        }
    }

    #[test]
    fn curved_world_neighbors_share_normals_and_hidden_patches_do_not_contribute() {
        let mut document = map_core::Document::empty();
        for x in [0.0, 128.0] {
            document
                .edit("terrain", |tx| {
                    map_core::edit::create_block(
                        tx,
                        DVec3::new(x, 0.0, 0.0),
                        DVec3::new(x + 128.0, 128.0, 16.0),
                        "brick/test",
                    )
                })
                .unwrap();
        }
        let mut vmf = document.vmf().clone();
        for (rotation, solid) in vmf.world.solids.iter_mut().enumerate() {
            let brush = BrushGeometry::from_solid(solid).unwrap();
            let face = brush
                .faces
                .iter()
                .find(|face| face.plane.normal == DVec3::Z)
                .unwrap();
            let side = &mut solid.sides[face.side];
            let mut disp = source_formats::vmf::DispInfo {
                power: 2,
                start_position: face.vertices[rotation * 2],
                flags: None,
                elevation: 0.0,
                subdivided: false,
                normals: None,
                distances: None,
                offsets: None,
                offset_normals: None,
                alphas: None,
                triangle_tags: None,
                allowed_verts: None,
                extra_properties: Default::default(),
                extra: Vec::new(),
            };
            let mesh = DisplacementMesh::build(face, &disp).unwrap();
            disp.normals = Some(vec![vec![DVec3::Z; mesh.size]; mesh.size]);
            disp.distances = Some(
                mesh.positions
                    .chunks(mesh.size)
                    .map(|row| {
                        row.iter()
                            .map(|point| (point.x - 128.0).powi(2) / 512.0)
                            .collect()
                    })
                    .collect(),
            );
            side.displacement = Some(disp);
        }
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        let mut materials = MaterialLibrary::default();
        let mesh = SceneMesh::build(&vmf, &fs, &mut materials, &HashSet::new());
        let seam = mesh
            .vertices
            .iter()
            .filter(|vertex| vertex.position == [128.0, 64.0, 16.0])
            .collect::<Vec<_>>();
        assert_eq!(seam.len(), 2);
        assert_eq!(seam[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(seam[0].normal, seam[1].normal);
        let retained = SceneMesh::build_with_models_filtered(
            &vmf,
            &fs,
            &mut materials,
            (&HashSet::new(), &HashSet::new()),
            &[],
            &[],
            |id| id == ObjectId::Solid(vmf.world.solids[0].id),
        );
        assert_eq!(retained.stats.displacements, 1);
        assert_eq!(
            retained
                .vertices
                .iter()
                .find(|vertex| vertex.position == [128.0, 64.0, 16.0])
                .unwrap()
                .normal,
            seam[0].normal
        );
        vmf.world.solids[1].hidden = true;
        let mesh = SceneMesh::build(&vmf, &fs, &mut materials, &HashSet::new());
        let seam = mesh
            .vertices
            .iter()
            .find(|vertex| vertex.position == [128.0, 64.0, 16.0])
            .unwrap();
        assert!(seam.normal[0].abs() > 0.01);
    }

    #[test]
    fn displacement_wireframe_includes_raised_internal_edges_once() {
        let mut document = map_core::Document::empty();
        document
            .edit("terrain", |tx| {
                map_core::edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap();
        let mut vmf = document.vmf().clone();
        let side = vmf.world.solids[0]
            .sides
            .iter_mut()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap();
        let mut distances = vec![vec![0.0; 5]; 5];
        distances[2][2] = 32.0;
        side.displacement = Some(source_formats::vmf::DispInfo {
            power: 2,
            start_position: DVec3::new(0.0, 0.0, 64.0),
            normals: Some(vec![vec![DVec3::Z; 5]; 5]),
            distances: Some(distances),
            flags: None,
            elevation: 0.0,
            subdivided: false,
            offsets: None,
            offset_normals: None,
            alphas: None,
            triangle_tags: None,
            allowed_verts: None,
            extra_properties: Default::default(),
            extra: Vec::new(),
        });
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        let mut materials = MaterialLibrary::default();
        let mesh = SceneMesh::build(&vmf, &fs, &mut materials, &HashSet::new());
        assert_eq!(mesh.stats.displacements, 1);
        assert_eq!(mesh.wire_lines.len(), 112);
        assert!(mesh.wire_lines.iter().any(|line| line.position[2] == 96.0));
        assert!(mesh.tool_wire_lines.is_empty());
        let peak = mesh
            .vertices
            .iter()
            .find(|vertex| vertex.position == [32.0, 32.0, 96.0])
            .unwrap();
        assert_eq!(peak.normal, [0.0, 0.0, 1.0]);
        for side in &mut vmf.world.solids[0].sides {
            side.material = "tools/toolsnodraw".into();
        }
        let mesh = SceneMesh::build(&vmf, &fs, &mut materials, &HashSet::new());
        assert_eq!(mesh.wire_lines.len() + mesh.tool_wire_lines.len(), 112);
    }

    #[test]
    fn surface_diagnostic_vertices_keep_exact_masks_and_authored_coordinates() {
        let mut document = map_core::Document::empty();
        document
            .edit("brush", |tx| {
                map_core::edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap();
        let mut vmf = document.vmf().clone();
        for side in &mut vmf.world.solids[0].sides {
            side.lightmap_scale = 8;
            side.smoothing_groups = Some(0x8000_0001);
            side.u_axis.offset = 12.0;
        }
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        let mut materials = MaterialLibrary::default();
        let mesh = SceneMesh::build(&vmf, &fs, &mut materials, &HashSet::new());
        assert!(!mesh.vertices.is_empty());
        for vertex in &mesh.vertices {
            assert_eq!(vertex.smoothing, 0x8000_0001);
            assert_eq!(vertex.lightmap[2], 8.0);
            let normal = Vec3::from_array(vertex.normal).as_dvec3();
            let side = vmf.world.solids[0]
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == normal)
                .unwrap();
            let expected =
                map_core::surface::lightmap_uv(side, Vec3::from_array(vertex.position).as_dvec3())
                    .unwrap()
                    .as_vec2();
            assert_eq!(&vertex.lightmap[..2], expected.as_ref());
        }
        for side in &mut vmf.world.solids[0].sides {
            side.lightmap_scale = 0;
        }
        let mesh = SceneMesh::build(&vmf, &fs, &mut materials, &HashSet::new());
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.lightmap[2] == -1.0)
        );
        assert!(
            light_mesh("light", false)
                .vertices
                .iter()
                .all(|vertex| vertex.lightmap == [0.0; 3] && vertex.smoothing == 0)
        );
    }

    fn light_mesh(classname: &str, selected: bool) -> SceneMesh {
        light_mesh_with_pitch(classname, selected, "-90")
    }

    fn light_mesh_with_pitch(classname: &str, selected: bool, pitch: &str) -> SceneMesh {
        let mut vmf = map_core::Document::empty().vmf().clone();
        let mut light = Entity::new(1, classname);
        light.properties.set("origin", "0 0 0");
        light.properties.set("_light", "255 210 150 450");
        light.properties.set("angles", "0 90 0");
        light.properties.set("pitch", pitch);
        vmf.entities.push(light);
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: vec![],
        };
        let selected = if selected {
            HashSet::from([ObjectId::Entity(1)])
        } else {
            HashSet::new()
        };
        SceneMesh::build(&vmf, &fs, &mut MaterialLibrary::default(), &selected)
    }

    #[test]
    fn light_shapes_keep_their_color_when_selected() {
        for classname in ["light_spot", "light_dynamic", "light_environment"] {
            let mesh = light_mesh(classname, true);
            assert_eq!(mesh.stats.point_entities, 1);
            assert_eq!(mesh.stats.brushes, 0);
            assert!(mesh.vertices.len() > 24);
            assert!(
                mesh.vertices
                    .iter()
                    .any(|vertex| vertex.color[..3] == [255, 210, 150])
            );
            assert!(mesh.lines.iter().any(|line| line.color == SELECTED_EDGE));
            assert!(mesh.vertices.iter().all(|vertex| {
                vertex
                    .position
                    .iter()
                    .chain(&vertex.normal)
                    .all(|value| value.is_finite())
            }));
            assert!(mesh.mins.cmpge(Vec3::splat(-18.0)).all());
            assert!(mesh.maxs.cmple(Vec3::splat(18.0)).all());
        }
    }

    #[test]
    fn point_lights_use_standard_entity_markers() {
        for selected in [false, true] {
            for light in [
                light_mesh("light", selected),
                light_mesh_with_pitch("light_spot", selected, "$Tilt"),
            ] {
                let generic = light_mesh("info_target", selected);
                assert_eq!(light.vertices.len(), 24);
                assert_eq!(light.indices.len(), 36);
                assert_eq!(light.vertices, generic.vertices);
                assert_eq!(light.indices, generic.indices);
                assert!(
                    light
                        .lines
                        .iter()
                        .map(|line| (line.position, line.color))
                        .eq(generic.lines.iter().map(|line| (line.position, line.color)))
                );
                assert_eq!(light.batches, generic.batches);
                assert_eq!(light.stats.missing_materials, 0);
            }
        }
    }

    #[test]
    fn spotlight_cones_follow_source_pitch_and_yaw() {
        let mesh = light_mesh("light_spot", true);
        assert!(mesh.lines.iter().any(|line| line.position[2] < -90.0));
        let unselected = light_mesh("light_spot", false);
        assert!(unselected.lines.is_empty());
        let mut entity = Entity::new(1, "light_spot");
        entity.properties.set("angles", "0 90 0");
        let direction = light_direction(&entity, &Default::default());
        assert!((direction - Vec3::Y).length() < 0.001);
        entity.properties.set("pitch", "-90");
        assert!((light_direction(&entity, &Default::default()) - Vec3::NEG_Z).length() < 0.001);
        entity.properties.set("target", "aim");
        let targets = [("aim", vec![DVec3::Y])].into();
        assert!((light_direction(&entity, &targets) - Vec3::Y).length() < 0.001);
        entity.properties.set("_light", "garbage");
        assert_eq!(light_color(&entity), [255, 240, 200]);
        entity.properties.set("_light", "$Lamp");
        assert_eq!(light_color(&entity), [110, 118, 132]);
        entity.properties.set("_light", "10 20 30 400");
        entity.properties.set("_lightHDR", "$HDR");
        assert_eq!(light_color(&entity), [10, 20, 30]);
    }
}
