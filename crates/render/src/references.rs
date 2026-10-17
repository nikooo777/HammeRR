use std::ops::Range;

use crate::scene::{LineVertex, Vertex};

pub const MAX_VERTICES: usize = 262_144;
pub const MAX_INDICES: usize = 786_432;
pub const MAX_LINES: usize = 524_288;
pub const MAX_TRIANGLES: usize = 262_144;
pub const MAX_CPU_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_GPU_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub edges: Vec<LineVertex>,
    pub brushes: Vec<Brush>,
}

#[derive(Debug)]
pub struct Brush {
    pub owner: u32,
    pub vertices: Range<u32>,
    pub indices: Range<u32>,
    pub edges: Range<u32>,
}

const COLOR: [u8; 3] = [94, 177, 209];
const SELECTED: [u8; 3] = [255, 167, 94];

impl crate::scene::SceneMesh {
    pub fn append_references(
        &mut self,
        mesh: &Mesh,
        library: &mut crate::MaterialLibrary,
        owners: &std::collections::BTreeSet<u32>,
        selected: &std::collections::HashSet<map_core::ObjectId>,
    ) -> Result<(), String> {
        let mut counts = [0usize; 3];
        for brush in mesh
            .brushes
            .iter()
            .filter(|brush| owners.contains(&brush.owner))
        {
            for (range, maximum) in [
                (&brush.vertices, mesh.vertices.len()),
                (&brush.indices, mesh.indices.len()),
                (&brush.edges, mesh.edges.len()),
            ] {
                if range.start > range.end || range.end as usize > maximum {
                    return Err("Invalid reference mesh range; ordinary scene retained".into());
                }
            }
            if brush.indices.len() % 3 != 0
                || brush.edges.len() % 2 != 0
                || mesh.indices[brush.indices.start as usize..brush.indices.end as usize]
                    .iter()
                    .any(|index| !brush.vertices.contains(index))
            {
                return Err("Invalid reference mesh indices; ordinary scene retained".into());
            }
            for (count, range) in
                counts
                    .iter_mut()
                    .zip([&brush.vertices, &brush.indices, &brush.edges])
            {
                *count = count
                    .checked_add(range.len())
                    .ok_or("Reference mesh count overflow")?;
            }
        }
        if counts[0] > MAX_VERTICES
            || counts[1] > MAX_INDICES
            || counts[2] > MAX_LINES
            || counts[1] / 3 > MAX_TRIANGLES
        {
            return Err(
                "Instance contents exceed the reference geometry limits; ordinary scene retained"
                    .into(),
            );
        }
        let bytes = counts
            .into_iter()
            .zip([
                size_of::<Vertex>(),
                size_of::<u32>(),
                size_of::<LineVertex>(),
            ])
            .try_fold(0usize, |total, (count, size)| {
                count
                    .checked_mul(size)
                    .and_then(|bytes| total.checked_add(bytes))
            })
            .ok_or("Reference GPU payload overflow")?;
        if bytes > MAX_GPU_BYTES {
            return Err("Instance contents exceed the 32 MiB reference GPU payload; ordinary scene retained".into());
        }
        for (existing, extra) in [
            (self.vertices.len(), counts[0]),
            (self.indices.len(), counts[1]),
        ] {
            if existing
                .checked_add(extra)
                .is_none_or(|count| count > u32::MAX as usize)
            {
                return Err(
                    "Combined root/reference indices overflow; ordinary scene retained".into(),
                );
            }
        }
        if !self.reference_lines.is_empty() {
            return Err("Reference geometry was already appended; ordinary scene retained".into());
        }
        if counts[0] == 0 {
            return Ok(());
        }
        self.vertices
            .try_reserve_exact(counts[0])
            .map_err(|error| format!("Reference vertices unavailable: {error}"))?;
        self.indices
            .try_reserve_exact(counts[1])
            .map_err(|error| format!("Reference indices unavailable: {error}"))?;
        self.reference_lines
            .try_reserve_exact(counts[2])
            .map_err(|error| format!("Reference edges unavailable: {error}"))?;
        self.reference_selection_ranges
            .try_reserve_exact(mesh.brushes.len())
            .map_err(|error| format!("Reference outlines unavailable: {error}"))?;
        self.batches
            .try_reserve_exact(1)
            .map_err(|error| format!("Reference batch unavailable: {error}"))?;
        let first_index = self.indices.len() as u32;
        for brush in mesh
            .brushes
            .iter()
            .filter(|brush| owners.contains(&brush.owner))
        {
            let selected = selected.contains(&map_core::ObjectId::Entity(brush.owner));
            let color = if selected { SELECTED } else { COLOR };
            let first_vertex = self.vertices.len() as u32;
            self.vertices.extend(
                mesh.vertices[brush.vertices.start as usize..brush.vertices.end as usize]
                    .iter()
                    .map(|vertex| Vertex {
                        color: [color[0], color[1], color[2], 255],
                        lightmap: [0.0; 3],
                        smoothing: 0,
                        opacity: 1.0,
                        texture_alpha: 0,
                        tangent: [0.0; 4],
                        lighting_receiver: 0,
                        ..*vertex
                    }),
            );
            self.indices.extend(
                mesh.indices[brush.indices.start as usize..brush.indices.end as usize]
                    .iter()
                    .map(|index| first_vertex + (index - brush.vertices.start)),
            );
            let first_line = self.reference_lines.len() as u32;
            self.reference_lines.extend(
                mesh.edges[brush.edges.start as usize..brush.edges.end as usize]
                    .iter()
                    .map(|line| LineVertex {
                        color: [color[0], color[1], color[2], 255],
                        ..*line
                    }),
            );
            if selected && self.reference_lines.len() as u32 > first_line {
                self.reference_selection_ranges
                    .push(first_line..self.reference_lines.len() as u32);
            }
        }
        self.batches.push(crate::scene::Batch {
            material: library.reference_geometry(),
            first_index,
            index_count: self.indices.len() as u32 - first_index,
            rope: false,
            model_translucent: false,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
