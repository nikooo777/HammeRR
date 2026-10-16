use map_core::displacement::{
    lod,
    neighbors::{NeighborPair, SurfaceGrid},
};
use source_formats::bsp::DisplacementVertex;

use super::{Terrain, neighbors::consume};
use crate::Control;

pub(super) struct Patch {
    pub mask: [u32; 10],
    pub positions: Vec<glam::DVec3>,
    pub vertices: Vec<DisplacementVertex>,
    pub indices: Vec<[usize; 3]>,
}

pub(super) fn prepare(
    terrains: &[Terrain],
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    work: &mut usize,
    control: &Control,
) -> Result<(Vec<Option<Patch>>, lod::Stats), String> {
    let (patches, stats) =
        lod::prepare(grids, pairs, &mut |amount| consume(work, amount, control))?;
    let patches = terrains
        .iter()
        .zip(patches)
        .enumerate()
        .map(|(index, (terrain, patch))| {
            patch.map(|patch| {
                let mut vertices = terrain.vertices.clone();
                for (vertex, &active) in patch.active.iter().enumerate() {
                    if active {
                        continue;
                    }
                    let delta =
                        (patch.positions[vertex] - grids[index].positions[vertex]).as_vec3();
                    let old = vertices[vertex];
                    vertices[vertex].direction =
                        (glam::Vec3::from_array(old.direction) * old.distance + delta).to_array();
                    vertices[vertex].distance = 1.0;
                }
                Patch {
                    mask: patch.mask,
                    positions: patch.positions,
                    vertices,
                    indices: patch.indices,
                }
            })
        })
        .collect();
    Ok((patches, stats))
}
