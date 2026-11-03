use crate::EditError;
use crate::surface::FaceId;

use super::invalid;

pub const REFINEMENT_LEVELS: usize = 4;
pub const MAX_FACES: usize = 16;
pub const MAX_FINAL_QUADS: usize = 65_536;
const CELLS: usize = 1 << REFINEMENT_LEVELS;
const QUADS_PER_SEED: usize = CELLS * CELLS;
pub const MAX_SAMPLE_SLOTS: usize = MAX_FINAL_QUADS + 2 * (MAX_FINAL_QUADS / CELLS) + MAX_FACES;
pub const MAX_VERTICES: usize = MAX_SAMPLE_SLOTS;
pub(super) const FIXED_TOPOLOGY_WORK: usize = 32;
const FAN_TOPOLOGY_WORK: usize = 32;
pub const MAX_TOPOLOGY_WORK: usize = MAX_FINAL_QUADS
    * (FAN_TOPOLOGY_WORK * (4 * MAX_FACES) + FIXED_TOPOLOGY_WORK)
    + REFINEMENT_LEVELS * MAX_VERTICES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWork {
    pub face: FaceId,
    pub cage_cells: usize,
    pub sample_size: usize,
    pub sample_slots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageWork {
    pub level: usize,
    pub quads: usize,
    pub vertex_bound: usize,
    pub topology_work_bound: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Work {
    pub sources: Vec<SourceWork>,
    pub stages: Vec<StageWork>,
    pub seed_quads: usize,
    pub final_quads: usize,
    pub sample_slots: usize,
    pub peak_vertices_bound: usize,
    pub topology_work_bound: usize,
}

fn overflow() -> EditError {
    invalid("Terrain subdivision work counts overflow")
}

fn grid_size(cells: usize, level: usize) -> Result<usize, EditError> {
    let shift = u32::try_from(level).map_err(|_| overflow())?;
    let factor = 1usize.checked_shl(shift).ok_or_else(overflow)?;
    cells
        .checked_mul(factor)
        .and_then(|cells| cells.checked_add(1))
        .ok_or_else(overflow)
}

pub(super) fn plan(faces: &[FaceId], cells: &[usize]) -> Result<Work, EditError> {
    if faces.is_empty() || faces.len() > MAX_FACES || faces.len() != cells.len() {
        return Err(invalid("Subdivision needs 1–16 source cage identities"));
    }
    let mut seed_quads = 0usize;
    for &cells in cells {
        if !cells.is_power_of_two() {
            return Err(invalid(
                "Subdivision cage density must be a positive power of two",
            ));
        }
        let quads = cells.checked_mul(cells).ok_or_else(overflow)?;
        seed_quads = seed_quads.checked_add(quads).ok_or_else(overflow)?;
    }
    let final_quads = seed_quads
        .checked_mul(QUADS_PER_SEED)
        .ok_or_else(overflow)?;
    if final_quads > MAX_FINAL_QUADS {
        return Err(invalid(&format!(
            "Subdivision needs {final_quads} refined quads; the work limit is {MAX_FINAL_QUADS}"
        )));
    }
    let mut sources = Vec::with_capacity(faces.len());
    for (&face, &cage_cells) in faces.iter().zip(cells) {
        let sample_size = grid_size(cage_cells, REFINEMENT_LEVELS)?;
        let sample_slots = sample_size.checked_mul(sample_size).ok_or_else(overflow)?;
        sources.push(SourceWork {
            face,
            cage_cells,
            sample_size,
            sample_slots,
        });
    }
    let fan_bound = faces.len().checked_mul(4).ok_or_else(overflow)?;
    let quad_work = fan_bound
        .checked_mul(FAN_TOPOLOGY_WORK)
        .and_then(|work| work.checked_add(FIXED_TOPOLOGY_WORK))
        .ok_or_else(overflow)?;
    let mut quads = seed_quads;
    let mut topology_work_bound = 0usize;
    let mut stages = Vec::with_capacity(REFINEMENT_LEVELS + 1);
    for level in 0..=REFINEMENT_LEVELS {
        let mut vertex_bound = 0usize;
        for &cells in cells {
            let size = grid_size(cells, level)?;
            let vertices = size.checked_mul(size).ok_or_else(overflow)?;
            vertex_bound = vertex_bound.checked_add(vertices).ok_or_else(overflow)?;
        }
        if vertex_bound > MAX_VERTICES {
            return Err(invalid(&format!(
                "Subdivision needs up to {vertex_bound} vertices; the work limit is {MAX_VERTICES}"
            )));
        }
        let stage_work = if level == REFINEMENT_LEVELS {
            0
        } else {
            quads
                .checked_mul(quad_work)
                .and_then(|work| work.checked_add(vertex_bound))
                .ok_or_else(overflow)?
        };
        topology_work_bound = topology_work_bound
            .checked_add(stage_work)
            .ok_or_else(overflow)?;
        if topology_work_bound > MAX_TOPOLOGY_WORK {
            return Err(invalid("Subdivision exceeds its topology work limit"));
        }
        stages.push(StageWork {
            level,
            quads,
            vertex_bound,
            topology_work_bound: stage_work,
        });
        if level != REFINEMENT_LEVELS {
            quads = quads.checked_mul(4).ok_or_else(overflow)?;
        }
    }
    let sample_slots = sources.iter().try_fold(0usize, |total, source| {
        total.checked_add(source.sample_slots).ok_or_else(overflow)
    })?;
    if sample_slots > MAX_SAMPLE_SLOTS {
        return Err(invalid(
            "Subdivision exceeds its intermediate sample-slot limit",
        ));
    }
    Ok(Work {
        sources,
        stages,
        seed_quads,
        final_quads,
        sample_slots,
        peak_vertices_bound: sample_slots,
        topology_work_bound,
    })
}

pub(super) fn check_mesh(
    quads: usize,
    vertices: usize,
    stage: &StageWork,
) -> Result<(), EditError> {
    if quads != stage.quads || vertices > stage.vertex_bound {
        return Err(invalid(
            "Subdivision geometry exceeds its admitted stage counts",
        ));
    }
    Ok(())
}

pub(super) fn refinement_capacity(
    vertices: usize,
    edges: usize,
    quads: usize,
    next: &StageWork,
) -> Result<(usize, usize), EditError> {
    let points = vertices
        .checked_add(edges)
        .and_then(|count| count.checked_add(quads))
        .ok_or_else(overflow)?;
    let faces = quads.checked_mul(4).ok_or_else(overflow)?;
    check_mesh(faces, points, next)?;
    Ok((points, faces))
}
