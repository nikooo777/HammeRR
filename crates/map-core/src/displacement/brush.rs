use glam::DVec3;
use source_formats::vmf::DispInfo;
use std::collections::BTreeSet;

use super::DisplacementMesh;
use crate::{EditError, brush::Face, primitive::MAX_COORDINATE};

#[derive(Debug, Clone, Copy)]
pub enum Axis {
    WorldZ,
    FaceNormal,
}

#[derive(Debug, Clone, Copy)]
pub struct Brush {
    pub center: DVec3,
    pub radius: f64,
    pub axis: Axis,
}

#[derive(Debug, Clone, Copy)]
pub enum Sculpt {
    Raise(f64),
    Flatten(f64),
    Smooth(f64),
    Noise { amplitude: f64, seed: u32 },
}

#[derive(Debug, Clone, Copy)]
pub enum Blend {
    Paint { value: f64, strength: f64 },
    Fill(f64),
    Invert,
}

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn mesh(face: &Face, info: &DispInfo) -> Result<DisplacementMesh, EditError> {
    if !info.extra.is_empty() {
        return Err(invalid("Opaque terrain blocks need repair before editing"));
    }
    let mesh = DisplacementMesh::build(face, info).map_err(|error| invalid(&error.to_string()))?;
    mesh.validate_edit_bounds()?;
    Ok(mesh)
}

fn axis(face: &Face, brush: Brush) -> Result<DVec3, EditError> {
    if !brush.center.is_finite()
        || brush.center.abs().max_element() > MAX_COORDINATE
        || !brush.radius.is_finite()
        || !(1.0..=8192.0).contains(&brush.radius)
    {
        return Err(invalid(
            "Brush center must fit ±65536 and radius must be 1–8192 world units",
        ));
    }
    Ok(match brush.axis {
        Axis::WorldZ => DVec3::Z,
        Axis::FaceNormal => face.plane.normal,
    })
}

fn weight(point: DVec3, brush: Brush, axis: DVec3) -> f64 {
    let delta = point - brush.center;
    let projected = delta - axis * delta.dot(axis);
    let value = (1.0 - projected.length() / brush.radius).clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn strength(value: f64) -> Result<(), EditError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid("Brush strength must be 0–1"));
    }
    Ok(())
}

fn amount(value: f64) -> Result<(), EditError> {
    if !value.is_finite() || value.abs() > 4096.0 {
        return Err(invalid("Height change must be within ±4096 world units"));
    }
    Ok(())
}

fn noise(point: DVec3, axis: DVec3, seed: u32) -> f64 {
    let projected = point - axis * point.dot(axis);
    let mut hash = u64::from(seed) ^ 0x9e3779b97f4a7c15;
    for value in projected.to_array() {
        hash ^= (value * 1024.0).round() as i64 as u64;
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d049bb133111eb);
        hash ^= hash >> 31;
    }
    (hash >> 11) as f64 / (1_u64 << 53) as f64 * 2.0 - 1.0
}

pub fn sculpt(
    face: &Face,
    info: &DispInfo,
    brush: Brush,
    operation: Sculpt,
) -> Result<DispInfo, EditError> {
    sculpt_masked(face, info, brush, operation, None)
}

fn validate_mask(mesh: &DisplacementMesh, mask: Option<&BTreeSet<usize>>) -> Result<(), EditError> {
    if mask.is_some_and(|mask| {
        mask.is_empty() || mask.iter().any(|&index| index >= mesh.positions.len())
    }) {
        return Err(invalid("Choose valid terrain vertices for the brush mask"));
    }
    Ok(())
}

fn sculpt_masked(
    face: &Face,
    info: &DispInfo,
    brush: Brush,
    operation: Sculpt,
    mask: Option<&BTreeSet<usize>>,
) -> Result<DispInfo, EditError> {
    let axis = axis(face, brush)?;
    match operation {
        Sculpt::Raise(value)
        | Sculpt::Noise {
            amplitude: value, ..
        } => amount(value)?,
        Sculpt::Flatten(value) | Sculpt::Smooth(value) => strength(value)?,
    }
    let mesh = mesh(face, info)?;
    validate_mask(&mesh, mask)?;
    if mesh.offset_normals_need_remapping(info) {
        return Err(invalid(
            "Nonzero or malformed offset normals need remapping before sculpting",
        ));
    }
    let mut result = info.clone();
    let mut offsets = info
        .offsets
        .clone()
        .unwrap_or_else(|| vec![vec![DVec3::ZERO; mesh.size]; mesh.size]);
    let mut changed = false;
    for (row, offset_row) in offsets.iter_mut().enumerate() {
        for (col, offset) in offset_row.iter_mut().enumerate() {
            let index = row * mesh.size + col;
            if mask.is_some_and(|mask| !mask.contains(&index)) {
                continue;
            }
            let point = mesh.positions[index];
            let falloff = weight(point, brush, axis);
            let delta = match operation {
                Sculpt::Raise(value) => value,
                Sculpt::Flatten(value) => (brush.center.dot(axis) - point.dot(axis)) * value,
                Sculpt::Noise { amplitude, seed } => amplitude * noise(point, axis, seed),
                Sculpt::Smooth(value) => {
                    let mut total = 0.0;
                    let mut count = 0;
                    for r in row.saturating_sub(1)..=(row + 1).min(mesh.size - 1) {
                        for c in col.saturating_sub(1)..=(col + 1).min(mesh.size - 1) {
                            if (r, c) != (row, col) {
                                total += mesh.positions[r * mesh.size + c].dot(axis);
                                count += 1;
                            }
                        }
                    }
                    (total / count as f64 - point.dot(axis)) * value
                }
            } * falloff;
            let change = axis * delta;
            let updated = point + change;
            if !updated.is_finite()
                || updated.abs().max_element() > MAX_COORDINATE
                || !(*offset + change).is_finite()
            {
                return Err(invalid(
                    "Sculpted terrain must fit finite coordinates within ±65536",
                ));
            }
            *offset += change;
            changed |= delta != 0.0;
        }
    }
    if changed {
        result.offsets = Some(offsets);
    }
    Ok(result)
}

pub fn blend(
    face: &Face,
    info: &DispInfo,
    brush: Brush,
    operation: Blend,
) -> Result<DispInfo, EditError> {
    blend_masked(face, info, brush, operation, None)
}

fn blend_masked(
    face: &Face,
    info: &DispInfo,
    brush: Brush,
    operation: Blend,
    mask: Option<&BTreeSet<usize>>,
) -> Result<DispInfo, EditError> {
    let axis = match operation {
        Blend::Paint { .. } => axis(face, brush)?,
        _ => DVec3::Z,
    };
    match operation {
        Blend::Paint {
            value,
            strength: factor,
        } => {
            strength(factor)?;
            if !value.is_finite() || !(0.0..=255.0).contains(&value) {
                return Err(invalid("Blend value must be 0–255"));
            }
        }
        Blend::Fill(value) => {
            if !value.is_finite() || !(0.0..=255.0).contains(&value) {
                return Err(invalid("Blend value must be 0–255"));
            }
        }
        Blend::Invert => {}
    }
    let mesh = mesh(face, info)?;
    validate_mask(&mesh, mask)?;
    let mut result = info.clone();
    let mut rows = Vec::with_capacity(mesh.size);
    let mut changed = false;
    for row in 0..mesh.size {
        let mut values = Vec::with_capacity(mesh.size);
        for col in 0..mesh.size {
            let index = row * mesh.size + col;
            let old = mesh.alphas[index];
            if mask.is_some_and(|mask| !mask.contains(&index)) {
                values.push(old);
                continue;
            }
            let value = match operation {
                Blend::Paint { value, strength } => {
                    old + (value - old) * strength * weight(mesh.positions[index], brush, axis)
                }
                Blend::Fill(value) => value,
                Blend::Invert => 255.0 - old,
            }
            .clamp(0.0, 255.0);
            changed |= value != old;
            values.push(value);
        }
        rows.push(values);
    }
    if changed {
        result.alphas = Some(rows);
    }
    Ok(result)
}

pub fn masked(
    face: &Face,
    info: &DispInfo,
    operation: &super::Operation,
    vertices: &[usize],
) -> Result<DispInfo, EditError> {
    let vertices = vertices.iter().copied().collect::<BTreeSet<_>>();
    match operation {
        super::Operation::Sculpt { brush, operation } => {
            sculpt_masked(face, info, *brush, *operation, Some(&vertices))
        }
        super::Operation::Blend { brush, operation } => {
            blend_masked(face, info, *brush, *operation, Some(&vertices))
        }
        super::Operation::Stroke { spec, points } => super::stroke::apply(
            face,
            info,
            *spec,
            points,
            Some(&vertices.iter().copied().collect::<Vec<_>>()),
        ),
        _ => Err(invalid(
            "Only sculpting and blending accept a terrain vertex mask",
        )),
    }
}

pub fn move_vertices(
    face: &Face,
    info: &DispInfo,
    vertices: &[usize],
    offset: DVec3,
) -> Result<DispInfo, EditError> {
    if !offset.is_finite() || offset.abs().max_element() > 4096.0 {
        return Err(invalid(
            "Terrain vertex offsets must be finite and within ±4096 per axis",
        ));
    }
    let mesh = mesh(face, info)?;
    let vertices = vertices.iter().copied().collect::<BTreeSet<_>>();
    validate_mask(&mesh, Some(&vertices))?;
    if mesh.offset_normals_need_remapping(info) {
        return Err(invalid(
            "Nonzero or malformed offset normals need remapping before moving vertices",
        ));
    }
    if offset == DVec3::ZERO {
        return Ok(info.clone());
    }
    let mut offsets = info
        .offsets
        .clone()
        .unwrap_or_else(|| vec![vec![DVec3::ZERO; mesh.size]; mesh.size]);
    for index in vertices {
        let point = mesh.positions[index] + offset;
        let value = offsets[index / mesh.size][index % mesh.size] + offset;
        if !point.is_finite() || point.abs().max_element() > MAX_COORDINATE || !value.is_finite() {
            return Err(invalid(
                "Moved terrain vertices must fit finite coordinates within ±65536",
            ));
        }
        offsets[index / mesh.size][index % mesh.size] = value;
    }
    let mut result = info.clone();
    result.offsets = Some(offsets);
    Ok(result)
}

pub fn set_vertices(
    face: &Face,
    info: &DispInfo,
    vertices: &[super::VertexSet],
) -> Result<DispInfo, EditError> {
    if vertices.is_empty() || vertices.len() > super::MAX_GRID_VERTICES {
        return Err(invalid("Set 1–289 native vertices per terrain face"));
    }
    let mesh = mesh(face, info)?;
    if mesh.offset_normals_need_remapping(info) {
        return Err(invalid(
            "Nonzero or malformed offset normals need remapping before setting vertices",
        ));
    }
    let mut selected = BTreeSet::new();
    let mut offsets = info
        .offsets
        .clone()
        .unwrap_or_else(|| vec![vec![DVec3::ZERO; mesh.size]; mesh.size]);
    let elevation = face.plane.normal * info.elevation;
    let mut expected = Vec::with_capacity(vertices.len());
    let mut changed = false;
    for target in vertices {
        let index = target.vertex;
        if index >= mesh.positions.len() || !selected.insert(index) {
            return Err(invalid("Use each valid native terrain vertex ID once"));
        }
        let old = mesh.positions[index];
        let point = match target.value {
            super::VertexValue::Position(point) => point,
            super::VertexValue::WorldZ(z) => DVec3::new(old.x, old.y, z),
        };
        if !point.is_finite() || point.abs().max_element() > MAX_COORDINATE {
            return Err(invalid(
                "Absolute terrain positions must be finite and within ±65536",
            ));
        }
        expected.push((index, point));
        if point == old {
            continue;
        }
        let row = index / mesh.size;
        let column = index % mesh.size;
        let mut base = mesh.base_positions[index];
        if let (Some(normals), Some(distances)) = (&info.normals, &info.distances) {
            base += normals[row][column] * distances[row][column];
        }
        let value = (point - elevation) - base;
        offsets[row][column] = match target.value {
            super::VertexValue::Position(_) => value,
            super::VertexValue::WorldZ(_) => {
                DVec3::new(offsets[row][column].x, offsets[row][column].y, value.z)
            }
        };
        if !offsets[row][column].is_finite() {
            return Err(invalid("Absolute terrain offsets must be finite"));
        }
        changed = true;
    }
    if !changed {
        return Ok(info.clone());
    }
    let mut result = info.clone();
    result.offsets = Some(offsets);
    let rebuilt =
        DisplacementMesh::build(face, &result).map_err(|error| invalid(&error.to_string()))?;
    if expected
        .iter()
        .any(|&(index, point)| rebuilt.positions[index] != point)
    {
        return Err(invalid(
            "Absolute positions cannot be reconstructed exactly from existing terrain deformation",
        ));
    }
    Ok(result)
}
