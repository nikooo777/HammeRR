use map_core::displacement::neighbors::{self, Span, SurfaceGrid};

use super::{NeighborStats, Terrain};
use crate::emit::stored_vertex;
use crate::{Control, Diagnostic, Report};

const MAX_WORK: usize = 2_000_000;

pub(super) fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .ok_or("Terrain neighbor work count overflow")?;
    if *work > MAX_WORK {
        return Err("Terrain neighbors exceeded their work budget".into());
    }
    Ok(())
}

pub(super) fn connect(
    terrains: &mut [Terrain],
    report: &mut Report,
    control: &Control,
) -> Result<NeighborStats, String> {
    let mut work = 0;
    let mut positions = Vec::with_capacity(terrains.len());
    for terrain in terrains.iter() {
        consume(&mut work, terrain.triangles.len(), control)?;
        positions.push(
            terrain
                .native_positions()?
                .into_iter()
                .map(|point| glam::DVec3::from_array(point.map(f64::from)))
                .collect::<Vec<_>>(),
        );
    }
    let grids = terrains
        .iter()
        .zip(&positions)
        .map(|(terrain, positions)| {
            let size = terrain.mesh.size;
            SurfaceGrid {
                power: terrain.info.power as u32,
                corners: [0, (size - 1) * size, size * size - 1, size - 1]
                    .map(|index| stored_vertex(terrain.mesh.base_positions[index])),
                positions,
            }
        })
        .collect::<Vec<_>>();
    let neighbors =
        neighbors::find_spans(&grids, &mut |amount| consume(&mut work, amount, control))?;
    let connections = neighbors::native_connections(&grids, &neighbors.pairs, &mut |amount| {
        consume(&mut work, amount, control)
    })?;
    let mixed_pairs = neighbors
        .pairs
        .iter()
        .filter(|pair| grids[pair.first].power != grids[pair.second].power)
        .count();
    let half_pairs = neighbors
        .pairs
        .iter()
        .filter(|pair| pair.first_span != Span::Full || pair.second_span != Span::Full)
        .count();
    let (lod, stats) = super::lod::prepare(terrains, &grids, &neighbors.pairs, &mut work, control)?;
    control.checkpoint()?;
    for (((terrain, edges), corners), lod) in terrains
        .iter_mut()
        .zip(connections.edges)
        .zip(connections.corners)
        .zip(lod)
    {
        terrain.info.edges = edges;
        terrain.info.corners = corners;
        if let Some(lod) = lod {
            terrain.info.allowed_vertices = lod.mask;
            terrain.mesh.positions = lod.positions;
            terrain.vertices = lod.vertices;
            terrain.indices = lod.indices;
        }
    }
    if neighbors.ambiguous + neighbors.mismatched != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("Terrain shared edges left unlinked: {} ambiguous, {} mismatched surfaces. Native full/half edges require matching common samples; other partial spans are not connected", neighbors.ambiguous, neighbors.mismatched), None, None));
    }
    if stats.snapped != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("Terrain LOD snapped {} inactive vertices onto the compatible surface; editable terrain remains unchanged", stats.snapped), None, None));
    }
    Ok(NeighborStats {
        pairs: neighbors.pairs.len(),
        corner_pairs: connections.corner_pairs,
        mixed_pairs,
        half_pairs,
        disabled_vertices: stats.disabled,
        snapped_vertices: stats.snapped,
    })
}
