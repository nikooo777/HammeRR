use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::DisplacementMesh;
use crate::{EditError, brush::Face};

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn weights(size: usize, row: f64, col: f64) -> ([usize; 3], [f64; 3]) {
    let r = (row as usize).min(size - 2);
    let c = (col as usize).min(size - 2);
    let y = row - r as f64;
    let x = col - c as f64;
    let even = (r + c).is_multiple_of(2);
    let a = r * size + c;
    let (b, c, d) = (a + 1, a + size, a + size + 1);
    if even {
        if y >= x {
            ([a, c, d], [1.0 - y, y - x, x])
        } else {
            ([a, d, b], [1.0 - x, y, x - y])
        }
    } else if x + y <= 1.0 {
        ([a, c, b], [1.0 - x - y, y, x])
    } else {
        ([c, d, b], [1.0 - x, x + y - 1.0, 1.0 - y])
    }
}

pub fn change(face: &Face, info: &DispInfo, power: u32) -> Result<DispInfo, EditError> {
    if !(2..=4).contains(&power) {
        return Err(invalid("Terrain power must be 2, 3 or 4"));
    }
    if power == info.power {
        return Ok(info.clone());
    }
    if !info.extra.is_empty() {
        return Err(invalid(
            "Extra terrain blocks need topology remapping before changing resolution",
        ));
    }
    let old = DisplacementMesh::build(face, info).map_err(|error| invalid(&error.to_string()))?;
    if old
        .alphas
        .iter()
        .any(|alpha| !(0.0..=255.0).contains(alpha))
    {
        return Err(invalid("Terrain blend values must be 0–255"));
    }
    let tag = if let Some(rows) = &info.triangle_tags {
        let cells = old.size - 1;
        if rows.len() != cells || rows.iter().any(|row| row.len() != cells * 2) {
            return Err(invalid("Terrain triangle tags do not match the grid"));
        }
        let tag = rows[0][0];
        if rows.iter().flatten().any(|value| *value != tag) {
            return Err(invalid(
                "Mixed triangle tags need topology remapping before changing resolution",
            ));
        }
        Some(tag)
    } else {
        None
    };
    if info
        .allowed_verts
        .as_ref()
        .is_some_and(|mask| mask.len() != 10 || mask.iter().any(|value| *value != -1))
    {
        return Err(invalid(
            "Restricted vertex masks need topology remapping before changing resolution",
        ));
    }
    if old.offset_normals_need_remapping(info) {
        return Err(invalid(
            "Nonzero or malformed offset normals need topology remapping before changing resolution",
        ));
    }
    let mut result = info.clone();
    result.power = power;
    result.normals = None;
    result.distances = None;
    result.offsets = None;
    result.offset_normals = None;
    result.elevation = 0.0;
    result.alphas = None;
    let base =
        DisplacementMesh::build(face, &result).map_err(|error| invalid(&error.to_string()))?;
    let ratio = (old.size - 1) as f64 / (base.size - 1) as f64;
    let mut offsets = Vec::with_capacity(base.size);
    let mut alphas = Vec::with_capacity(base.size);
    for row in 0..base.size {
        let mut offset_row = Vec::with_capacity(base.size);
        let mut alpha_row = Vec::with_capacity(base.size);
        for col in 0..base.size {
            let (indices, weights) = weights(old.size, row as f64 * ratio, col as f64 * ratio);
            let mut point = DVec3::ZERO;
            let mut alpha = 0.0;
            for (index, weight) in indices.into_iter().zip(weights) {
                point += old.positions[index] * weight;
                alpha += old.alphas[index] * weight;
            }
            let offset = point - base.positions[row * base.size + col];
            if !offset.is_finite() || !alpha.is_finite() {
                return Err(invalid("Resampled terrain values must be finite"));
            }
            offset_row.push(offset);
            alpha_row.push(alpha);
        }
        offsets.push(offset_row);
        alphas.push(alpha_row);
    }
    result.triangle_tags = tag.map(|tag| vec![vec![tag; 2 * (base.size - 1)]; base.size - 1]);
    result.offset_normals = info
        .offset_normals
        .as_ref()
        .map(|_| vec![vec![DVec3::ZERO; base.size]; base.size]);
    result.offsets = Some(offsets);
    result.alphas = info.alphas.as_ref().map(|_| alphas);
    Ok(result)
}
