use super::{Batch, SELECTED_EDGE, SELECTED_TINT, SceneMesh, Vertex, WIRE_EDGE};

use std::collections::HashSet;

use map_core::ObjectId;

use crate::ropes::Preview;
use crate::ropes::materials::Prepared;
use crate::scene::LineVertex;

impl SceneMesh {
    pub fn append_rope_preview(
        &mut self,
        preview: &Preview,
        appearance: &Prepared,
        selected: &HashSet<ObjectId>,
        selection_tint: bool,
    ) -> Result<(), String> {
        self.append_rope(preview, appearance, selected, selection_tint, false)
    }

    pub fn append_pending_rope_preview(
        &mut self,
        preview: &Preview,
        appearance: &Prepared,
        selected: &HashSet<ObjectId>,
        selection_tint: bool,
    ) -> Result<(), String> {
        self.append_rope(preview, appearance, selected, selection_tint, true)
    }

    fn append_rope(
        &mut self,
        preview: &Preview,
        appearance: &Prepared,
        selected: &HashSet<ObjectId>,
        selection_tint: bool,
        pending: bool,
    ) -> Result<(), String> {
        if preview.links.len() > crate::ropes::MAX_LINKS || !appearance.issues.is_empty() {
            return Err(
                "Rope appearance snapshot exceeds its limits or has unavailable materials".into(),
            );
        }
        let mut vertex_count = 0usize;
        let mut line_count = 0usize;
        for link in &preview.links {
            if link.points.len() < 2 || link.points.len() > crate::ropes::MAX_CURVE_SEGMENTS + 1 {
                return Err("Rope mesh point count is outside the admitted span limits".into());
            }
            let material = appearance.by_source.get(&link.source).ok_or_else(|| {
                format!("Rope material for entity:{} is unavailable", link.source)
            })?;
            let height = f64::from(material.report.dimensions[1]);
            if height <= 0.0 {
                return Err("Rope material has no texture mapping height".into());
            }
            for triangle in link.mesh_triangles() {
                vertex_count = vertex_count
                    .checked_add(triangle.len())
                    .ok_or("Rope mesh vertex count overflow")?;
                for vertex in triangle {
                    let uv = [
                        vertex.uv[0] as f32,
                        (vertex.uv[1] / height + link.scroll) as f32,
                    ];
                    if !vertex.position.as_vec3().is_finite()
                        || !vertex.normal.as_vec3().is_finite()
                        || !uv.into_iter().all(f32::is_finite)
                    {
                        return Err(
                            "Rope material mesh contains non-finite positions, normals or UVs"
                                .into(),
                        );
                    }
                }
            }
            if selected.contains(&ObjectId::Entity(link.source)) {
                line_count = line_count
                    .checked_add(link.points.len().saturating_sub(1) * 2)
                    .ok_or("Rope selection line count overflow")?;
            }
        }
        if self
            .vertices
            .len()
            .checked_add(vertex_count)
            .is_none_or(|count| count > u32::MAX as usize)
            || self
                .indices
                .len()
                .checked_add(vertex_count)
                .is_none_or(|count| count > u32::MAX as usize)
        {
            return Err("Rope mesh exceeds the scene's index capacity".into());
        }
        let wire_count = vertex_count
            .checked_mul(2)
            .ok_or("Rope wireframe line count overflow")?;
        let (wire_lines, selection_lines) = if pending {
            (&mut self.wire_lines, &mut self.lines)
        } else {
            (&mut self.rope_wire_lines, &mut self.rope_lines)
        };
        if wire_lines
            .len()
            .checked_add(wire_count)
            .is_none_or(|count| count > u32::MAX as usize)
            || selection_lines
                .len()
                .checked_add(line_count)
                .is_none_or(|count| count > u32::MAX as usize)
        {
            return Err("Rope lines exceed the scene's line capacity".into());
        }
        self.vertices
            .try_reserve(vertex_count)
            .map_err(|_| "Rope vertex allocation failed")?;
        self.indices
            .try_reserve(vertex_count)
            .map_err(|_| "Rope index allocation failed")?;
        self.batches
            .try_reserve(preview.links.len())
            .map_err(|_| "Rope batch allocation failed")?;
        selection_lines
            .try_reserve(line_count)
            .map_err(|_| "Rope selection allocation failed")?;
        wire_lines
            .try_reserve(wire_count)
            .map_err(|_| "Rope wireframe allocation failed")?;
        for link in &preview.links {
            let material = &appearance.by_source[&link.source];
            let height = f64::from(material.report.dimensions[1]);
            let is_selected = selected.contains(&ObjectId::Entity(link.source));
            let color = if is_selected && selection_tint {
                SELECTED_TINT
            } else {
                [255; 3]
            };
            let first_index = self.indices.len() as u32;
            for triangle in link.mesh_triangles() {
                for [a, b] in [[0, 1], [1, 2], [2, 0]] {
                    wire_lines.extend([a, b].map(|index| LineVertex {
                        position: triangle[index].position.as_vec3().to_array(),
                        color: WIRE_EDGE,
                    }));
                }
                for vertex in triangle {
                    let position = vertex.position.as_vec3();
                    self.indices.push(self.vertices.len() as u32);
                    self.vertices.push(Vertex {
                        position: position.to_array(),
                        normal: vertex.normal.as_vec3().to_array(),
                        uv: [
                            vertex.uv[0] as f32,
                            (vertex.uv[1] / height + link.scroll) as f32,
                        ],
                        color: [color[0], color[1], color[2], 0],
                        lightmap: [0.0; 3],
                        smoothing: 0,
                        opacity: material.report.opacity,
                        texture_alpha: u32::from(material.report.translucent),
                        tangent: [0.0; 4],
                        lighting_receiver: 0,
                        baked_irradiance: [0.0; 3],
                    });
                    self.mins = self.mins.min(position);
                    self.maxs = self.maxs.max(position);
                }
            }
            let index_count = self.indices.len() as u32 - first_index;
            if index_count > 0 {
                self.batches.push(Batch {
                    material: material.material,
                    first_index,
                    index_count,
                    rope: !pending,
                    model_translucent: material.report.translucent || material.report.opacity < 1.0,
                });
            }
            if is_selected {
                selection_lines.extend(link.points.windows(2).flat_map(|pair| {
                    pair.iter().map(|point| LineVertex {
                        position: point.as_vec3().to_array(),
                        color: SELECTED_EDGE,
                    })
                }));
            }
        }
        Ok(())
    }
}
