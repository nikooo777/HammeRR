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
            patch
                .map(|patch| {
                    let vertices =
                        lod::encoded_vertices(&terrain.vertices, grids[index].positions, &patch)?;
                    Ok(Patch {
                        mask: patch.mask,
                        positions: patch.positions,
                        vertices,
                        indices: patch.indices,
                    })
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((patches, stats))
}
