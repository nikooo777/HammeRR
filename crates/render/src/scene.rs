//! Converts a VMF into GPU-ready triangle batches, one per material.

use std::collections::HashSet;

use glam::{DVec3, Vec3};
use map_core::query::POINT_ENTITY_HALF_SIZE;
use map_core::{BrushGeometry, DisplacementMesh, ObjectId, texel_uv};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Color, Entity, Solid, Vmf};

use crate::materials::{MaterialId, MaterialLibrary};

/// Hammer's color for entities whose editor block has none.
const DEFAULT_ENTITY_COLOR: [u8; 3] = [220, 30, 220];
const SELECTED_TINT: [u8; 3] = [255, 130, 130];
const SELECTED_EDGE: [u8; 4] = [255, 64, 64, 255];

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Tint in RGB; alpha is the blend factor between a material's two layers.
    pub color: [u8; 4],
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
    pub missing_materials: usize,
}

#[derive(Debug)]
pub struct SceneMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// Edges of selected objects, as a line list.
    pub lines: Vec<LineVertex>,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub stats: SceneStats,
}

struct Builder<'a> {
    fs: &'a GameFs,
    library: &'a mut MaterialLibrary,
    vertices: Vec<Vertex>,
    /// Indices per material id, concatenated into batches at the end.
    indices: Vec<Vec<u32>>,
    lines: Vec<LineVertex>,
    stats: SceneStats,
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
        let tint = if selected { SELECTED_TINT } else { [255; 3] };
        // Hammer draws only the displaced faces of a brush with displacements.
        let has_displacement = solid.sides.iter().any(|s| s.displacement.is_some());
        for face in &brush.faces {
            let side = &solid.sides[face.side];
            let material = self.material(&side.material);
            let info = self.library.get(material);
            let size = DVec3::new(f64::from(info.width), f64::from(info.height), 1.0);
            let uv = |p: DVec3| (texel_uv(side, p) / size.truncate()).as_vec2().to_array();

            match (&side.displacement, has_displacement) {
                (Some(disp), _) => {
                    let Ok(mesh) = DisplacementMesh::build(face, disp) else {
                        continue;
                    };
                    self.stats.displacements += 1;
                    self.add_displacement(material, &mesh, uv, tint);
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
                    let normal = face.plane.normal.as_vec3().to_array();
                    let first = self.vertices.len() as u32;
                    for &p in &face.vertices {
                        self.push_vertex(Vertex {
                            position: p.as_vec3().to_array(),
                            normal,
                            uv: uv(p),
                            color: [tint[0], tint[1], tint[2], 0],
                        });
                    }
                    let indices = &mut self.indices[material];
                    for i in 1..face.vertices.len() as u32 - 1 {
                        indices.extend([first, first + i, first + i + 1]);
                    }
                    if selected {
                        self.push_loop(face.vertices.iter().map(|p| p.as_vec3()));
                    }
                }
            }
        }
    }

    fn add_displacement(
        &mut self,
        material: MaterialId,
        mesh: &DisplacementMesh,
        uv: impl Fn(DVec3) -> [f32; 2],
        tint: [u8; 3],
    ) {
        let triangles: Vec<[usize; 3]> = mesh.triangles().collect();
        let mut normals = vec![DVec3::ZERO; mesh.positions.len()];
        for &[a, b, c] in &triangles {
            let (pa, pb, pc) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
            let n = (pa - pb).cross(pc - pb);
            for i in [a, b, c] {
                normals[i] += n;
            }
        }
        // Texture coordinates come from the undisplaced face, as in the engine,
        // so they're computed from the flat grid position.
        let first = self.vertices.len() as u32;
        for (i, &p) in mesh.positions.iter().enumerate() {
            let alpha = mesh.alphas[i].clamp(0.0, 255.0) as u8;
            self.push_vertex(Vertex {
                position: p.as_vec3().to_array(),
                normal: normals[i].normalize_or_zero().as_vec3().to_array(),
                uv: uv(mesh.base_positions[i]),
                color: [tint[0], tint[1], tint[2], alpha],
            });
        }
        self.indices[material].extend(triangles.iter().flatten().map(|&i| first + i as u32));
    }

    fn add_point_entity(&mut self, entity: &Entity, selected: bool) {
        let Some(origin) = entity.origin() else {
            return;
        };
        self.stats.point_entities += 1;
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
        let mut builder = Builder {
            fs,
            library,
            vertices: Vec::new(),
            indices: Vec::new(),
            lines: Vec::new(),
            stats: SceneStats::default(),
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
            mins,
            maxs,
            stats,
        }
    }
}
