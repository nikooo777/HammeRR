use map_core::displacement::neighbors::{self, SurfaceGrid};
use source_formats::bsp::DisplacementNeighbor;

use super::Terrain;
use crate::emit::stored_vertex;
use crate::{Control, Diagnostic, Report};

const MAX_WORK: usize = 2_000_000;

fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
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
) -> Result<usize, String> {
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
    let neighbors = neighbors::find(&grids, &mut |amount| consume(&mut work, amount, control))?;
    let mut pending = vec![[[DisplacementNeighbor::default(); 2]; 4]; terrains.len()];
    for pair in &neighbors.pairs {
        for (index, edge, other, other_edge) in [
            (pair.first, pair.first_edge, pair.second, pair.second_edge),
            (pair.second, pair.second_edge, pair.first, pair.first_edge),
        ] {
            pending[index][edge][0] = DisplacementNeighbor {
                index: other as u16,
                orientation: ((2 + other_edge + 4 - edge) % 4) as u8,
                ..Default::default()
            };
        }
    }
    control.checkpoint()?;
    for (terrain, edges) in terrains.iter_mut().zip(pending) {
        terrain.info.edges = edges;
    }
    if neighbors.ambiguous + neighbors.unequal_power + neighbors.mismatched != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("Terrain shared edges left unlinked: {} ambiguous, {} unequal-power, {} mismatched surfaces. Neighbor metadata currently supports exact native base edges with matching equal-power grids", neighbors.ambiguous, neighbors.unequal_power, neighbors.mismatched), None, None));
    }
    Ok(neighbors.pairs.len())
}
