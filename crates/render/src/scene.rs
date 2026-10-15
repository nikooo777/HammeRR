//! Converts a VMF into GPU-ready triangle batches, one per material.

use std::collections::{BTreeSet, HashMap, HashSet};

use glam::{DVec3, Vec3};
use map_core::query::POINT_ENTITY_HALF_SIZE;
use map_core::surface::FaceId;
use map_core::{BrushGeometry, DisplacementMesh, ObjectId, texel_uv};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Color, Entity, Solid, Vmf};

use crate::helpers::bulb_model;
use crate::materials::{MaterialId, MaterialLibrary};

const HELPER_SEGMENTS: usize = 16;

/// Hammer's color for entities whose editor block has none.
const DEFAULT_ENTITY_COLOR: [u8; 3] = [220, 30, 220];
const SELECTED_TINT: [u8; 3] = [255, 130, 130];
const SELECTED_EDGE: [u8; 4] = [255, 64, 64, 255];
const WIRE_EDGE: [u8; 4] = [151, 169, 191, 255];

fn lightmap_vertex(side: &source_formats::vmf::Side, point: DVec3) -> [f32; 3] {
    map_core::surface::lightmap_uv(side, point)
        .map(|uv| [uv.x as f32, uv.y as f32, side.lightmap_scale as f32])
        .filter(|values| values.iter().all(|value| value.is_finite()))
        .unwrap_or([0.0, 0.0, -1.0])
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// RGB tint; alpha blends brush layers or marks an emissive helper surface.
    pub color: [u8; 4],
    pub lightmap: [f32; 3],
    pub smoothing: u32,
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
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SceneStats {
    pub brushes: usize,
    pub invalid_brushes: usize,
    pub displacements: usize,
    pub point_entities: usize,
    pub model_entities: usize,
    pub missing_materials: usize,
}

#[derive(Debug)]
pub struct SceneMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// Edges of selected objects, as a line list.
    pub lines: Vec<LineVertex>,
    pub leak_lines: Vec<LineVertex>,
    pub wire_lines: Vec<LineVertex>,
    pub tool_wire_lines: Vec<LineVertex>,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub stats: SceneStats,
}

#[derive(Debug, Clone)]
pub struct FaceOverlay {
    pub source: FaceId,
    pub points: Vec<DVec3>,
    pub material: String,
    pub projection: Option<map_core::surface::Projection>,
}

struct Builder<'a> {
    models: &'a [crate::models::Instance],
    overlays: &'a [FaceOverlay],
    selected_faces: &'a HashSet<FaceId>,
    fs: &'a GameFs,
    library: &'a mut MaterialLibrary,
    vertices: Vec<Vertex>,
    /// Indices per material id, concatenated into batches at the end.
    indices: Vec<Vec<u32>>,
    lines: Vec<LineVertex>,
    wire_lines: Vec<LineVertex>,
    tool_wire_lines: Vec<LineVertex>,
    stats: SceneStats,
    light_targets: map_core::light::Targets<'a>,
    displacements: HashMap<FaceId, Displacement>,
}

struct Displacement {
    mesh: DisplacementMesh,
    normals: Vec<[f64; 3]>,
}

fn local_normals(mesh: &DisplacementMesh) -> Vec<[f64; 3]> {
    source_formats::bsp::DisplacementInfo {
        power: (mesh.size - 1).ilog2() as i32,
        ..Default::default()
    }
    .vertex_normals(
        &mesh
            .positions
            .iter()
            .map(|point| point.to_array())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| vec![[0.0; 3]; mesh.positions.len()])
}

fn world_displacements(vmf: &Vmf) -> HashMap<FaceId, Displacement> {
    use map_core::displacement::neighbors::{self, SurfaceGrid};
    let mut meshes = Vec::new();
    for solid in vmf
        .world
        .solids
        .iter()
        .chain(
            vmf.entities
                .iter()
                .filter(|entity| !entity.hidden && entity.classname() == Some("func_detail"))
                .flat_map(|entity| &entity.solids),
        )
        .filter(|solid| !solid.hidden && solid.sides.iter().any(|side| side.displacement.is_some()))
    {
        let Ok(brush) = BrushGeometry::from_solid(solid) else {
            continue;
        };
        for face in &brush.faces {
            let side = &solid.sides[face.side];
            if let Some(disp) = &side.displacement
                && let Ok(mesh) = DisplacementMesh::build(face, disp)
            {
                if meshes.len() == source_formats::bsp::MAX_DISPLACEMENTS {
                    return HashMap::new();
                }
                meshes.push((
                    FaceId {
                        solid: solid.id,
                        side: side.id,
                    },
                    mesh,
                    map_core::displacement::smoothing::angle(disp).unwrap_or(None),
                ));
            }
        }
    }
    let grids = meshes
        .iter()
        .map(|(_, mesh, _)| SurfaceGrid {
            power: (mesh.size - 1).ilog2(),
            corners: [
                0,
                (mesh.size - 1) * mesh.size,
                mesh.size * mesh.size - 1,
                mesh.size - 1,
            ]
            .map(|index| mesh.base_positions[index].as_vec3().as_dvec3()),
            positions: &mesh.positions,
        })
        .collect::<Vec<_>>();
    let angles = meshes
        .iter()
        .map(|(_, _, angle)| *angle)
        .collect::<Vec<_>>();
    let normals = neighbors::find_spans(&grids, &mut |_| Ok(())).and_then(|links| {
        neighbors::vertex_normals_with_smoothing(&grids, &links.pairs, &angles, &mut |_| Ok(()))
    });
    let mut normals = normals.unwrap_or_default().into_iter();
    meshes
        .into_iter()
        .map(|(face, mesh, _)| {
            let normals = normals.next().unwrap_or_else(|| local_normals(&mesh));
            (face, Displacement { mesh, normals })
        })
        .collect()
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

    fn add_solid(&mut self, solid: &Solid, selected: bool) {
        self.stats.brushes += 1;
        let Ok(brush) = BrushGeometry::from_solid(solid) else {
            self.stats.invalid_brushes += 1;
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
                (Some(disp), _) => {
                    let displacement = self
                        .displacements
                        .remove(&FaceId {
                            solid: solid.id,
                            side: side.id,
                        })
                        .or_else(|| {
                            DisplacementMesh::build(face, disp).ok().map(|mesh| {
                                let normals = local_normals(&mesh);
                                Displacement { mesh, normals }
                            })
                        });
                    let Some(Displacement { mesh, normals }) = displacement else {
                        continue;
                    };
                    self.stats.displacements += 1;
                    self.add_displacement(material, &mesh, &normals, uv, tint, side);
                    self.push_displacement_wire(&mesh, tool);
                    if selected {
                        self.push_loop(
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
            });
        }
        for index in 1..points.len() as u32 - 1 {
            self.indices[material].extend([first, first + index, first + index + 1]);
        }
        if selected {
            self.push_loop(points.iter().map(|point| point.as_vec3()));
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
        let mut edges = BTreeSet::new();
        for [a, b, c] in mesh.triangles() {
            for (a, b) in [(a, b), (b, c), (c, a)] {
                edges.insert((a.min(b), a.max(b)));
            }
        }
        let lines = if tool {
            &mut self.tool_wire_lines
        } else {
            &mut self.wire_lines
        };
        for (a, b) in edges {
            lines.extend([a, b].map(|index| LineVertex {
                position: mesh.positions[index].as_vec3().to_array(),
                color: WIRE_EDGE,
            }));
        }
    }

    fn add_displacement(
        &mut self,
        material: MaterialId,
        mesh: &DisplacementMesh,
        normals: &[[f64; 3]],
        uv: impl Fn(DVec3) -> [f32; 2],
        tint: [u8; 3],
        side: &source_formats::vmf::Side,
    ) {
        let triangles: Vec<[usize; 3]> = mesh.triangles().collect();
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
            });
        }
        self.indices[material].extend(triangles.iter().flatten().map(|&i| first + i as u32));
    }

    fn add_point_entity(&mut self, entity: &Entity, selected: bool) {
        let Some(origin) = entity.origin() else {
            return;
        };
        self.stats.point_entities += 1;
        if let Some(instance) = self
            .models
            .iter()
            .find(|instance| instance.id == ObjectId::Entity(entity.id))
        {
            self.add_model(instance, selected);
            return;
        }
        if self.add_light(entity, selected) {
            return;
        }
        let [r, g, b] = if selected {
            SELECTED_EDGE[..3].try_into().expect("three channels")
        } else {
            entity
                .editor
                .as_ref()
                .and_then(|e| e.get("color"))
                .and_then(Color::parse)
                .map_or(DEFAULT_ENTITY_COLOR, |c| c.0)
        };
        let material = self.material_editor();
        let center = origin.as_vec3();
        let h = POINT_ENTITY_HALF_SIZE as f32;
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
            let first = self.vertices.len() as u32;
            for corner in corners {
                self.push_vertex(Vertex {
                    position: (center + (normal + corner) * h).to_array(),
                    normal: normal.to_array(),
                    uv: [0.0, 0.0],
                    color: [r, g, b, 0],
                    lightmap: [0.0; 3],
                    smoothing: 0,
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
                self.push_loop(corners.map(|c| center + (normal + c) * h * 1.05));
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
                    self.helper_line(a, d, SELECTED_EDGE);
                }
            }
        }
        if selected {
            for &(radius, height) in profile.iter().filter(|(radius, _)| *radius > 0.0) {
                self.push_loop((0..HELPER_SEGMENTS).map(|segment| point(radius, height, segment)));
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

    fn add_bulb(&mut self, center: Vec3, color: [u8; 3], selected: bool) {
        let model = bulb_model();
        let material = self.library.light_bulb();
        if self.indices.len() <= material {
            self.indices.resize_with(material + 1, Vec::new);
        }
        let first = self.vertices.len() as u32;
        for &template in &model.vertices {
            let tint = if template.color[3] == 255 {
                color
            } else {
                [255; 3]
            };
            self.push_vertex(Vertex {
                position: (center + Vec3::from_array(template.position)).to_array(),
                color: [tint[0], tint[1], tint[2], template.color[3]],
                ..template
            });
        }
        self.indices[material].extend(model.indices.iter().map(|&i| first + i));
        if selected {
            let lo = model.mins - Vec3::splat(1.0);
            let hi = model.maxs + Vec3::splat(1.0);
            for x in [lo.x, hi.x] {
                for y in [lo.y, hi.y] {
                    for z in [lo.z, hi.z] {
                        let corner = Vec3::new(x, y, z);
                        let sign = (lo + hi) * 0.5 - corner;
                        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                            self.helper_line(
                                center + corner,
                                center + corner + axis * sign.dot(axis).signum() * 4.0,
                                SELECTED_EDGE,
                            );
                        }
                    }
                }
            }
        }
    }

    fn add_model(&mut self, instance: &crate::models::Instance, selected: bool) {
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
                color: if selected {
                    [255, 210, 150, 0]
                } else {
                    [255, 255, 255, 0]
                },
                lightmap: [0.0, 0.0, -1.0],
                smoothing: 0,
            });
        }
        for part in &model.parts {
            let material_index = model.skins[instance.skin][part.material];
            let material = self.material(&instance.asset.materials[material_index]);
            self.indices[material].extend(part.indices.iter().map(|&index| first + index));
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
                            self.helper_line(
                                corner.as_vec3(),
                                (corner + axis * sign.dot(axis).signum() * 4.0).as_vec3(),
                                SELECTED_EDGE,
                            );
                        }
                    }
                }
            }
        }
        self.stats.model_entities += 1;
    }

    fn add_light(&mut self, entity: &Entity, selected: bool) -> bool {
        let class = entity.classname().unwrap_or("").to_ascii_lowercase();
        if !matches!(
            class.as_str(),
            "light" | "light_spot" | "light_dynamic" | "light_environment"
        ) {
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
        } else {
            self.add_bulb(center, color, selected);
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
        let mut builder = Builder {
            models,
            overlays,
            selected_faces,
            fs,
            library,
            vertices: Vec::new(),
            indices: Vec::new(),
            lines: Vec::new(),
            wire_lines: Vec::new(),
            tool_wire_lines: Vec::new(),
            stats: SceneStats::default(),
            displacements: world_displacements(vmf),
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
        for solid in vmf.world.solids.iter().filter(|s| !s.hidden) {
            builder.add_solid(solid, selected.contains(&ObjectId::Solid(solid.id)));
        }
        for entity in vmf.entities.iter().filter(|e| !e.hidden) {
            let is_selected = selected.contains(&ObjectId::Entity(entity.id));
            if entity.solids.is_empty() {
                builder.add_point_entity(entity, is_selected);
            }
            for solid in entity.solids.iter().filter(|s| !s.hidden) {
                builder.add_solid(solid, is_selected);
            }
        }

        let (mins, maxs) = builder
            .vertices
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| {
                let p = Vec3::from(v.position);
                (lo.min(p), hi.max(p))
            });
        let mut indices = Vec::with_capacity(builder.indices.iter().map(Vec::len).sum());
        let mut batches = Vec::new();
        let mut stats = builder.stats;
        for (material, list) in builder.indices.into_iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            stats.missing_materials += usize::from(builder.library.get(material).missing);
            batches.push(Batch {
                material,
                first_index: indices.len() as u32,
                index_count: list.len() as u32,
            });
            indices.extend(list);
        }
        Self {
            vertices: builder.vertices,
            indices,
            batches,
            lines: builder.lines,
            leak_lines: Vec::new(),
            wire_lines: builder.wire_lines,
            tool_wire_lines: builder.tool_wire_lines,
            mins,
            maxs,
            stats,
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
        let mut vmf = map_core::Document::empty().vmf().clone();
        let mut light = Entity::new(1, classname);
        light.properties.set("origin", "0 0 0");
        light.properties.set("_light", "255 210 150 450");
        light.properties.set("angles", "0 90 0");
        light.properties.set("pitch", "-90");
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
        for classname in ["light", "light_spot", "light_dynamic", "light_environment"] {
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
    fn bulb_selection_preserves_geometry_and_only_the_globe_is_tinted() {
        let plain = light_mesh("light", false);
        let selected = light_mesh("light", true);
        assert_eq!(plain.vertices, selected.vertices);
        assert_eq!(plain.indices, selected.indices);
        assert!(plain.lines.is_empty());
        assert!(!selected.lines.is_empty());
        for vertex in &plain.vertices {
            let expected = if vertex.color[3] == 255 {
                [255, 210, 150]
            } else {
                [255; 3]
            };
            assert_eq!(vertex.color[..3], expected);
        }
        assert_eq!(plain.stats.missing_materials, 0);
    }

    #[test]
    fn bulb_instances_share_one_material_across_scene_rebuilds() {
        let mut vmf = map_core::Document::empty().vmf().clone();
        for (id, origin, color) in [
            (1, "0 0 0", "255 210 150 450"),
            (2, "64 0 0", "140 190 255 450"),
        ] {
            let mut entity = Entity::new(id, "light");
            entity.properties.set("origin", origin);
            entity.properties.set("_light", color);
            vmf.entities.push(entity);
        }
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: vec![],
        };
        let mut library = MaterialLibrary::default();
        for _ in 0..2 {
            let mesh = SceneMesh::build(&vmf, &fs, &mut library, &HashSet::new());
            assert_eq!(library.len(), 2);
            assert_eq!(mesh.batches.len(), 1);
            assert_eq!(mesh.stats.point_entities, 2);
            assert_eq!(mesh.stats.brushes, 0);
            let count = bulb_model().vertices.len();
            for (a, b) in mesh.vertices[..count].iter().zip(&mesh.vertices[count..]) {
                assert!(
                    (Vec3::from_array(b.position) - Vec3::from_array(a.position) - Vec3::X * 64.0)
                        .length()
                        < 0.0001
                );
                if a.color[3] == 255 {
                    assert_eq!(b.color, [140, 190, 255, 255]);
                } else {
                    assert_eq!(a.color, b.color);
                }
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
    }
}
