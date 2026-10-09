//! Converts a VMF into GPU-ready triangle batches, one per material.

use std::collections::HashMap;

use glam::{DVec3, Vec3};
use map_core::{BrushGeometry, DisplacementMesh, texel_uv};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Color, Entity, Solid, Vmf};

use crate::materials::MaterialInfo;

/// Half the edge length of the box drawn for point entities.
const POINT_ENTITY_HALF_SIZE: f32 = 8.0;
/// Hammer's color for entities whose editor block has none.
const DEFAULT_ENTITY_COLOR: [u8; 3] = [220, 30, 220];
const ENTITY_MATERIAL: &str = "__editor/entity";

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Tint in RGB; alpha is the blend factor between a material's two layers.
    pub color: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub material: usize,
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
    pub materials: Vec<MaterialInfo>,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub stats: SceneStats,
}

#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    /// Indices per material, concatenated into batches at the end.
    indices: Vec<Vec<u32>>,
    materials: Vec<MaterialInfo>,
    by_name: HashMap<String, usize>,
    stats: SceneStats,
}

impl Builder {
    fn material(&mut self, fs: &GameFs, name: &str) -> usize {
        let key = name.to_ascii_lowercase();
        if let Some(&index) = self.by_name.get(&key) {
            return index;
        }
        let info = if key == ENTITY_MATERIAL {
            MaterialInfo::untextured(ENTITY_MATERIAL)
        } else {
            MaterialInfo::load(fs, &key)
        };
        self.stats.missing_materials += usize::from(info.missing);
        self.materials.push(info);
        self.indices.push(Vec::new());
        self.by_name.insert(key, self.materials.len() - 1);
        self.materials.len() - 1
    }

    fn push_vertex(&mut self, vertex: Vertex) -> u32 {
        self.vertices.push(vertex);
        (self.vertices.len() - 1) as u32
    }

    fn add_solid(&mut self, fs: &GameFs, solid: &Solid) {
        self.stats.brushes += 1;
        let Ok(brush) = BrushGeometry::from_solid(solid) else {
            self.stats.invalid_brushes += 1;
            return;
        };
        // Hammer draws only the displaced faces of a brush with displacements.
        let has_displacement = solid.sides.iter().any(|s| s.displacement.is_some());
        for face in &brush.faces {
            let side = &solid.sides[face.side];
            let material = self.material(fs, &side.material);
            let info = &self.materials[material];
            let size = DVec3::new(f64::from(info.width), f64::from(info.height), 1.0);
            let uv = |p: DVec3| (texel_uv(side, p) / size.truncate()).as_vec2().to_array();

            match (&side.displacement, has_displacement) {
                (Some(disp), _) => {
                    let Ok(mesh) = DisplacementMesh::build(face, disp) else {
                        continue;
                    };
                    self.stats.displacements += 1;
                    self.add_displacement(material, &mesh, uv);
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
                            color: [255, 255, 255, 0],
                        });
                    }
                    let indices = &mut self.indices[material];
                    for i in 1..face.vertices.len() as u32 - 1 {
                        indices.extend([first, first + i, first + i + 1]);
                    }
                }
            }
        }
    }

    fn add_displacement(
        &mut self,
        material: usize,
        mesh: &DisplacementMesh,
        uv: impl Fn(DVec3) -> [f32; 2],
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
                color: [255, 255, 255, alpha],
            });
        }
        self.indices[material].extend(
            triangles
                .iter()
                .flatten()
                .map(|&i| first + i as u32),
        );
    }

    fn add_point_entity(&mut self, fs: &GameFs, entity: &Entity) {
        let Some(origin) = entity.origin() else {
            return;
        };
        self.stats.point_entities += 1;
        let [r, g, b] = entity
            .editor
            .as_ref()
            .and_then(|e| e.get("color"))
            .and_then(Color::parse)
            .map_or(DEFAULT_ENTITY_COLOR, |c| c.0);
        let material = self.material(fs, ENTITY_MATERIAL);
        let center = origin.as_vec3();
        let h = POINT_ENTITY_HALF_SIZE;
        for normal in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
            // Two in-plane axes chosen so the quad winds clockwise from outside.
            let up = if normal.z.abs() > 0.5 { Vec3::X } else { Vec3::Z };
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
            self.indices[material].extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }
}

impl SceneMesh {
    pub fn build(vmf: &Vmf, fs: &GameFs) -> Self {
        let mut builder = Builder::default();
        for solid in vmf.world.solids.iter().filter(|s| !s.hidden) {
            builder.add_solid(fs, solid);
        }
        for entity in vmf.entities.iter().filter(|e| !e.hidden) {
            if entity.solids.is_empty() {
                builder.add_point_entity(fs, entity);
            }
            for solid in entity.solids.iter().filter(|s| !s.hidden) {
                builder.add_solid(fs, solid);
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
        for (material, list) in builder.indices.into_iter().enumerate() {
            if list.is_empty() {
                continue;
            }
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
            materials: builder.materials,
            mins,
            maxs,
            stats: builder.stats,
        }
    }
}
