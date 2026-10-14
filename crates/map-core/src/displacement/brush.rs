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
    if info.subdivided
        || !info.extra.is_empty()
        || info.normals.is_some() != info.distances.is_some()
        || !info.start_position.is_finite()
    {
        return Err(invalid(
            "Terrain subdivision, opaque blocks or malformed geometry need repair before editing",
        ));
    }
    let mesh = DisplacementMesh::build(face, info).map_err(|error| invalid(&error.to_string()))?;
    if mesh
        .positions
        .iter()
        .any(|p| !p.is_finite() || p.abs().max_element() > MAX_COORDINATE)
        || mesh
            .alphas
            .iter()
            .any(|a| !a.is_finite() || !(0.0..=255.0).contains(a))
    {
        return Err(invalid(
            "Terrain positions must fit ±65536 and blend values must be 0–255",
        ));
    }
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
    if info.offset_normals.as_ref().is_some_and(|rows| {
        rows.len() != mesh.size
            || rows
                .iter()
                .any(|row| row.len() != mesh.size || row.iter().any(|v| *v != DVec3::ZERO))
    }) {
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
    if info.offset_normals.as_ref().is_some_and(|rows| {
        rows.len() != mesh.size
            || rows
                .iter()
                .any(|row| row.len() != mesh.size || row.iter().any(|v| *v != DVec3::ZERO))
    }) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushGeometry, Document, ObjectId, edit, surface::FaceId};

    fn fixture() -> (Face, DispInfo, Brush) {
        let mut doc = Document::empty();
        let ObjectId::Solid(id) = doc
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::splat(-64.0), DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let solid = doc.solid(id).unwrap();
        let face = BrushGeometry::from_solid(solid)
            .unwrap()
            .faces
            .into_iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap();
        let target = FaceId {
            solid: id,
            side: solid.sides[face.side].id,
        };
        doc.edit("terrain", |tx| {
            super::super::edit::create(tx, &[target], 2, 0)
        })
        .unwrap();
        let mut info = doc.solid(id).unwrap().sides[face.side]
            .displacement
            .clone()
            .unwrap();
        info.extra_properties.set("note", "retain");
        (
            face,
            info,
            Brush {
                center: DVec3::new(0.0, 0.0, 64.0),
                radius: 32.0,
                axis: Axis::WorldZ,
            },
        )
    }

    #[test]
    fn radial_raise_flatten_and_smooth_keep_scope_and_source_metadata() {
        let (face, info, brush) = fixture();
        let old = DisplacementMesh::build(&face, &info).unwrap();
        let raised = sculpt(&face, &info, brush, Sculpt::Raise(16.0)).unwrap();
        let mesh = DisplacementMesh::build(&face, &raised).unwrap();
        for (index, point) in mesh.positions.iter().enumerate() {
            assert_eq!(
                *point,
                old.positions[index] + DVec3::Z * if index == 12 { 16.0 } else { 0.0 }
            );
        }
        assert_eq!(raised.alphas, info.alphas);
        assert_eq!(raised.normals, info.normals);
        assert_eq!(raised.distances, info.distances);
        assert_eq!(raised.extra_properties, info.extra_properties);
        assert_eq!(raised.start_position, info.start_position);
        let flat = sculpt(
            &face,
            &raised,
            Brush {
                center: DVec3::new(0.0, 0.0, 72.0),
                ..brush
            },
            Sculpt::Flatten(1.0),
        )
        .unwrap();
        assert_eq!(
            DisplacementMesh::build(&face, &flat).unwrap().positions[12].z,
            72.0
        );
        let smooth = sculpt(&face, &raised, brush, Sculpt::Smooth(1.0)).unwrap();
        assert_eq!(
            DisplacementMesh::build(&face, &smooth).unwrap().positions,
            old.positions
        );
        assert_eq!(
            sculpt(&face, &info, brush, Sculpt::Raise(0.0)).unwrap(),
            info
        );
    }

    #[test]
    fn noise_is_repeatable_seeded_bounded_and_changes_only_the_chosen_direction() {
        let (face, info, brush) = fixture();
        let brush = Brush {
            radius: 96.0,
            ..brush
        };
        let op = Sculpt::Noise {
            amplitude: 32.0,
            seed: 7,
        };
        let first = sculpt(&face, &info, brush, op).unwrap();
        assert_eq!(first, sculpt(&face, &info, brush, op).unwrap());
        assert_ne!(
            first,
            sculpt(
                &face,
                &info,
                brush,
                Sculpt::Noise {
                    amplitude: 32.0,
                    seed: 8
                }
            )
            .unwrap()
        );
        let old = DisplacementMesh::build(&face, &info).unwrap();
        let new = DisplacementMesh::build(&face, &first).unwrap();
        for (a, b) in old.positions.iter().zip(new.positions) {
            assert_eq!(a.x, b.x);
            assert_eq!(a.y, b.y);
            assert!((a.z - b.z).abs() <= 32.0);
        }
    }

    #[test]
    fn tilted_faces_distinguish_world_z_from_surface_normal_brushes() {
        let (mut face, mut info, _) = fixture();
        let rotation = glam::DQuat::from_rotation_y(0.4);
        for point in &mut face.vertices {
            *point = rotation * *point;
        }
        face.plane.normal = rotation * face.plane.normal;
        info.start_position = face.vertices[0];
        let old = DisplacementMesh::build(&face, &info).unwrap();
        for axis in [Axis::WorldZ, Axis::FaceNormal] {
            let brush = Brush {
                center: old.positions[12],
                radius: 32.0,
                axis,
            };
            let result = sculpt(&face, &info, brush, Sculpt::Raise(16.0)).unwrap();
            let point = DisplacementMesh::build(&face, &result).unwrap().positions[12];
            let direction = match axis {
                Axis::WorldZ => DVec3::Z,
                Axis::FaceNormal => face.plane.normal,
            };
            assert!(point.distance(old.positions[12] + direction * 16.0) < 1e-9);
        }
    }

    #[test]
    fn blend_paint_fill_and_invert_keep_geometry_and_fractional_endpoints() {
        let (face, info, brush) = fixture();
        let old = DisplacementMesh::build(&face, &info).unwrap();
        let painted = blend(
            &face,
            &info,
            brush,
            Blend::Paint {
                value: 255.0,
                strength: 1.0,
            },
        )
        .unwrap();
        let mesh = DisplacementMesh::build(&face, &painted).unwrap();
        assert_eq!(mesh.positions, old.positions);
        for (index, alpha) in mesh.alphas.iter().enumerate() {
            assert_eq!(*alpha, if index == 12 { 255.0 } else { 0.0 });
        }
        let filled = blend(&face, &info, brush, Blend::Fill(22.5)).unwrap();
        assert!(
            DisplacementMesh::build(&face, &filled)
                .unwrap()
                .alphas
                .iter()
                .all(|a| *a == 22.5)
        );
        let inverted = blend(&face, &filled, brush, Blend::Invert).unwrap();
        let inverted = DisplacementMesh::build(&face, &inverted).unwrap();
        assert_eq!(inverted.positions, old.positions);
        assert!(inverted.alphas.iter().all(|a| *a == 232.5));
        assert_eq!(blend(&face, &info, brush, Blend::Fill(0.0)).unwrap(), info);
    }

    #[test]
    fn malformed_brush_values_and_out_of_bounds_geometry_reject() {
        let (face, mut info, brush) = fixture();
        for bad in [
            Brush {
                radius: 0.0,
                ..brush
            },
            Brush {
                radius: 8193.0,
                ..brush
            },
            Brush {
                center: DVec3::NAN,
                ..brush
            },
        ] {
            assert!(sculpt(&face, &info, bad, Sculpt::Raise(16.0)).is_err());
        }
        assert!(sculpt(&face, &info, brush, Sculpt::Raise(f64::NAN)).is_err());
        assert!(sculpt(&face, &info, brush, Sculpt::Smooth(1.1)).is_err());
        assert!(blend(&face, &info, brush, Blend::Fill(256.0)).is_err());
        info.elevation = 65470.0;
        assert!(sculpt(&face, &info, brush, Sculpt::Raise(16.0)).is_err());
    }
    #[test]
    fn masked_brushes_preserve_unselected_positions_alpha_and_metadata() {
        let (face, info, mut brush) = fixture();
        brush.radius = 8192.0;
        let before = DisplacementMesh::build(&face, &info).unwrap();
        let selected = [0, 5, 6];
        let raised = masked(
            &face,
            &info,
            &super::super::Operation::Sculpt {
                brush,
                operation: Sculpt::Raise(16.0),
            },
            &selected,
        )
        .unwrap();
        let after = DisplacementMesh::build(&face, &raised).unwrap();
        for (index, (a, b)) in before.positions.iter().zip(&after.positions).enumerate() {
            assert_eq!(a != b, selected.contains(&index));
        }
        let mut preserved = raised.clone();
        preserved.offsets = info.offsets.clone();
        assert_eq!(preserved, info);
        for operation in [Blend::Fill(255.0), Blend::Invert] {
            let changed = masked(
                &face,
                &info,
                &super::super::Operation::Blend { brush, operation },
                &selected,
            )
            .unwrap();
            let values = DisplacementMesh::build(&face, &changed).unwrap().alphas;
            for (index, (&old, &new)) in before.alphas.iter().zip(&values).enumerate() {
                assert_eq!(old != new, selected.contains(&index));
            }
            let mut preserved = changed.clone();
            preserved.alphas = info.alphas.clone();
            assert_eq!(preserved, info);
        }
        assert!(
            masked(
                &face,
                &info,
                &super::super::Operation::Sculpt {
                    brush,
                    operation: Sculpt::Raise(16.0)
                },
                &[25]
            )
            .is_err()
        );
        assert!(
            masked(
                &face,
                &info,
                &super::super::Operation::Resolution(3),
                &selected
            )
            .is_err()
        );
    }

    #[test]
    fn masked_smoothing_reads_immutable_unselected_neighbors() {
        let (face, mut info, mut brush) = fixture();
        info.distances.as_mut().unwrap()[2][2] = 32.0;
        let before = DisplacementMesh::build(&face, &info).unwrap();
        brush.center = before.positions[12];
        let changed = masked(
            &face,
            &info,
            &super::super::Operation::Sculpt {
                brush,
                operation: Sculpt::Smooth(1.0),
            },
            &[12],
        )
        .unwrap();
        let after = DisplacementMesh::build(&face, &changed).unwrap();
        for (index, (&old, &new)) in before.positions.iter().zip(&after.positions).enumerate() {
            if index == 12 {
                assert!((new.z - (old.z - 32.0)).abs() < 1e-9);
            } else {
                assert_eq!(old, new);
            }
        }
    }

    #[test]
    fn exact_vertex_moves_keep_other_samples_metadata_and_absent_zero_rows() {
        let (face, mut info, _) = fixture();
        info.flags = Some(14);
        info.triangle_tags = Some(vec![vec![9; 8]; 4]);
        let selected = [7, 12, 12];
        assert_eq!(
            move_vertices(&face, &info, &selected, DVec3::ZERO).unwrap(),
            info
        );
        let offset = DVec3::new(3.5, -2.0, 12.0);
        let moved = move_vertices(&face, &info, &selected, offset).unwrap();
        let before = DisplacementMesh::build(&face, &info).unwrap();
        let after = DisplacementMesh::build(&face, &moved).unwrap();
        for (id, (a, b)) in before.positions.iter().zip(&after.positions).enumerate() {
            assert_eq!(
                *b,
                *a + if selected.contains(&id) {
                    offset
                } else {
                    DVec3::ZERO
                }
            );
        }
        let mut metadata = moved.clone();
        metadata.offsets = info.offsets.clone();
        assert_eq!(metadata, info);
        for invalid in [DVec3::splat(f64::NAN), DVec3::X * 4097.0] {
            assert!(move_vertices(&face, &info, &selected, invalid).is_err());
        }
        assert!(move_vertices(&face, &info, &[25], offset).is_err());
        let restored = move_vertices(&face, &moved, &selected, -offset).unwrap();
        assert_eq!(DisplacementMesh::build(&face, &restored).unwrap(), before);
    }
}
