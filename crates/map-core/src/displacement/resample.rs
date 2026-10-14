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
    if !info.start_position.is_finite() {
        return Err(invalid("Terrain start position must be finite"));
    }
    if info.subdivided || !info.extra.is_empty() {
        return Err(invalid(
            "Subdivision and extra terrain blocks need topology remapping before changing resolution",
        ));
    }
    if info.normals.is_some() != info.distances.is_some() {
        return Err(invalid(
            "Terrain normals and distances must be provided together",
        ));
    }
    let old = DisplacementMesh::build(face, info).map_err(|error| invalid(&error.to_string()))?;
    if old.positions.iter().any(|p| !p.is_finite())
        || old
            .alphas
            .iter()
            .any(|a| !a.is_finite() || !(0.0..=255.0).contains(a))
    {
        return Err(invalid(
            "Terrain positions must be finite and blend values must be 0–255",
        ));
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
    if info.offset_normals.as_ref().is_some_and(|rows| {
        rows.len() != old.size
            || rows
                .iter()
                .any(|row| row.len() != old.size || row.iter().any(|value| *value != DVec3::ZERO))
    }) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::FaceId;
    use crate::{BrushGeometry, Document, ObjectId, edit};

    fn fixture(corner: u32) -> (Face, DispInfo) {
        let mut doc = Document::empty();
        let ObjectId::Solid(id) = doc
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let solid = doc.solid(id).unwrap();
        let shape = BrushGeometry::from_solid(solid).unwrap();
        let face = shape
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap()
            .clone();
        let target = FaceId {
            solid: id,
            side: solid.sides[face.side].id,
        };
        doc.edit("terrain", |tx| {
            super::super::edit::create(tx, &[target], 2, corner)
        })
        .unwrap();
        let mut info = doc.solid(id).unwrap().sides[face.side]
            .displacement
            .clone()
            .unwrap();
        info.extra_properties.set("custom_note", "retain");
        for row in 0..5 {
            for col in 0..5 {
                info.distances.as_mut().unwrap()[row][col] = (row * row * 7 + col * 3) as f64;
                info.alphas.as_mut().unwrap()[row][col] = (row * 29 + col * 7) as f64;
            }
        }
        (face, info)
    }

    #[test]
    fn refinement_and_coarsening_keep_shared_positions_alpha_orientation_and_properties() {
        for corner in 0..4 {
            let (face, info) = fixture(corner);
            let old = DisplacementMesh::build(&face, &info).unwrap();
            for power in [3, 4] {
                let result = change(&face, &info, power).unwrap();
                let new = DisplacementMesh::build(&face, &result).unwrap();
                let stride = (new.size - 1) / (old.size - 1);
                for row in 0..old.size {
                    for col in 0..old.size {
                        let a = row * old.size + col;
                        let b = row * stride * new.size + col * stride;
                        assert!(old.positions[a].distance(new.positions[b]) < 1e-9);
                        assert_eq!(old.alphas[a], new.alphas[b]);
                    }
                }
                assert_eq!(result.start_position, info.start_position);
                assert_eq!(result.flags, info.flags);
                assert_eq!(result.extra_properties, info.extra_properties);
                let reduced = change(&face, &result, 2).unwrap();
                let reduced = DisplacementMesh::build(&face, &reduced).unwrap();
                assert_eq!(reduced.positions, old.positions);
                assert_eq!(reduced.alphas, old.alphas);
            }
        }
    }

    #[test]
    fn refinement_follows_alternating_triangle_diagonals_instead_of_bilinear_heights() {
        for col in [0, 1] {
            let (face, mut info) = fixture(0);
            let rows = info.distances.as_mut().unwrap();
            for row in rows.iter_mut() {
                row.fill(0.0);
            }
            rows[0][col] = 50.0;
            rows[1][col + 1] = 50.0;
            let result = change(&face, &info, 3).unwrap();
            let new = DisplacementMesh::build(&face, &result).unwrap();
            assert_eq!(
                new.positions[new.size + col * 2 + 1].z,
                64.0 + if col == 0 { 50.0 } else { 0.0 }
            );
        }
    }

    #[test]
    fn unknown_topology_data_and_malformed_rows_reject_but_same_power_is_exact() {
        let (face, info) = fixture(0);
        let mut explicit = info.clone();
        explicit.triangle_tags = Some(vec![vec![7; 8]; 4]);
        explicit.triangle_tags.as_mut().unwrap()[0][0] = 1;
        assert!(change(&face, &explicit, 3).is_err());
        assert_eq!(change(&face, &explicit, 2).unwrap(), explicit);
        explicit = info.clone();
        explicit.allowed_verts = Some(vec![0; 10]);
        assert!(change(&face, &explicit, 3).is_err());
        explicit = info.clone();
        explicit.distances.as_mut().unwrap()[0][0] = f64::NAN;
        assert!(change(&face, &explicit, 3).is_err());
        explicit = info.clone();
        explicit.alphas.as_mut().unwrap()[0][0] = 256.0;
        assert!(change(&face, &explicit, 3).is_err());
        explicit = info.clone();
        explicit.start_position.x = f64::NAN;
        assert!(change(&face, &explicit, 3).is_err());
        assert!(change(&face, &info, 5).is_err());
    }
    #[test]
    fn uniform_tags_and_unrestricted_masks_resize_without_changing_their_meaning() {
        let (face, mut info) = fixture(0);
        info.triangle_tags = Some(vec![vec![7; 8]; 4]);
        info.allowed_verts = Some(vec![-1; 10]);
        info.offset_normals = Some(vec![vec![DVec3::ZERO; 5]; 5]);
        let result = change(&face, &info, 3).unwrap();
        assert_eq!(result.triangle_tags, Some(vec![vec![7; 16]; 8]));
        assert_eq!(result.allowed_verts, info.allowed_verts);
        assert_eq!(result.offset_normals, Some(vec![vec![DVec3::ZERO; 9]; 9]));
    }
}
