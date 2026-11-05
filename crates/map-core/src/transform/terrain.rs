use glam::DVec3;
use source_formats::vmf::{DispInfo, Solid, format_vec3_bracketed, parse_vec3};

use super::{Mapping, invalid};
use crate::displacement::{ALLOWED_VERTEX_MASK_WORDS, DisplacementMesh, triangles};
use crate::{BrushGeometry, EditError};

const SAMPLE_EPSILON: f64 = 1e-5;
const MASK_BITS: usize = i32::BITS as usize;

pub(super) struct Surface {
    side: usize,
    positions: Vec<DVec3>,
    base_positions: Vec<DVec3>,
}

fn rows<T>(rows: &Option<Vec<Vec<T>>>, size: usize, field: &str) -> Result<(), EditError> {
    if rows
        .as_ref()
        .is_some_and(|rows| rows.len() != size || rows.iter().any(|row| row.len() != size))
    {
        return Err(invalid(format!(
            "Displacement {field} rows have invalid dimensions"
        )));
    }
    Ok(())
}

fn validate(info: &DispInfo, size: usize) -> Result<(), EditError> {
    if info.normals.is_some() != info.distances.is_some() {
        return Err(invalid(
            "Displacement normals and distances must be provided together",
        ));
    }
    rows(&info.normals, size, "normal")?;
    rows(&info.distances, size, "distance")?;
    rows(&info.offsets, size, "offset")?;
    rows(&info.offset_normals, size, "offset-normal")?;
    rows(&info.alphas, size, "alpha")?;
    let cells = size - 1;
    if info
        .triangle_tags
        .as_ref()
        .is_some_and(|rows| rows.len() != cells || rows.iter().any(|row| row.len() != cells * 2))
    {
        return Err(invalid(
            "Displacement triangle tag rows have invalid dimensions",
        ));
    }
    if info
        .allowed_verts
        .as_ref()
        .is_some_and(|mask| mask.len() != ALLOWED_VERTEX_MASK_WORDS)
    {
        return Err(invalid(
            "Displacement affine transforms require a ten-word allowed-vertex mask",
        ));
    }
    Ok(())
}

fn transpose<T>(rows: &mut Option<Vec<Vec<T>>>) {
    if let Some(rows) = rows {
        for row in 0..rows.len() {
            for col in 0..row {
                let (before, after) = rows.split_at_mut(row);
                std::mem::swap(&mut after[0][col], &mut before[col][row]);
            }
        }
    }
}

fn vertex(index: usize, size: usize) -> usize {
    index % size * size + index / size
}

fn tags(info: &mut DispInfo, size: usize) -> Result<(), EditError> {
    let Some(rows) = &mut info.triangle_tags else {
        return Ok(());
    };
    let cells = size - 1;
    if rows.len() != cells || rows.iter().any(|row| row.len() != cells * 2) {
        return Err(invalid(
            "Displacement triangle tag rows have invalid dimensions",
        ));
    }
    let indices = triangles::source_indices(info.power)?;
    let transformed = indices
        .iter()
        .map(|triangle| triangle.map(|index| vertex(index, size)))
        .collect::<Vec<_>>();
    let mapping = triangles::correspondence(&transformed, &indices)?;
    let mut remapped = vec![0; indices.len()];
    for (index, &target) in mapping.iter().enumerate() {
        remapped[target] = rows[index / (cells * 2)][index % (cells * 2)];
    }
    *rows = remapped.chunks(cells * 2).map(<[i32]>::to_vec).collect();
    Ok(())
}

fn allowed(info: &mut DispInfo, size: usize) -> Result<(), EditError> {
    let Some(mask) = &mut info.allowed_verts else {
        return Ok(());
    };
    if mask.len() != ALLOWED_VERTEX_MASK_WORDS {
        return Err(invalid(
            "Displacement reflection requires a ten-word allowed-vertex mask",
        ));
    }
    let mut remapped = mask.clone();
    for index in 0..size * size {
        let target = vertex(index, size);
        let bit = 1_i32 << (target % MASK_BITS);
        if mask[index / MASK_BITS] & (1_i32 << (index % MASK_BITS)) == 0 {
            remapped[target / MASK_BITS] &= !bit;
        } else {
            remapped[target / MASK_BITS] |= bit;
        }
    }
    *mask = remapped;
    Ok(())
}

fn reindex(info: &mut DispInfo, size: usize) -> Result<(), EditError> {
    if !info.extra.is_empty()
        || info.extra_properties.iter().any(|(key, _)| {
            !["smooth", "mintess", "uaxis", "vaxis"]
                .iter()
                .any(|known| key.eq_ignore_ascii_case(known))
        })
    {
        return Err(invalid(
            "Displacement reflection cannot reindex opaque terrain payloads",
        ));
    }
    transpose(&mut info.normals);
    transpose(&mut info.distances);
    transpose(&mut info.offsets);
    transpose(&mut info.offset_normals);
    transpose(&mut info.alphas);
    tags(info, size)?;
    allowed(info, size)
}

fn mapping_axes(mapping: &Mapping, info: &mut DispInfo) -> Result<(), EditError> {
    let mut seen = [false; 2];
    for (key, value) in &mut info.extra_properties.0 {
        let axis = if key.eq_ignore_ascii_case("uaxis") {
            0
        } else if key.eq_ignore_ascii_case("vaxis") {
            1
        } else {
            continue;
        };
        if seen[axis] {
            return Err(invalid("Displacement has duplicate legacy mapping axes"));
        }
        seen[axis] = true;
        let vector = parse_vec3(value)
            .filter(|vector| vector.is_finite())
            .ok_or_else(|| invalid("Displacement legacy mapping axes must be finite vectors"))?;
        let vector = mapping.inverse_transpose * vector;
        if !vector.is_finite() {
            return Err(invalid("Displacement legacy mapping axis overflow"));
        }
        *value = format_vec3_bracketed(vector);
    }
    Ok(())
}

pub(super) fn capture(mapping: &Mapping, solid: &Solid) -> Result<Vec<Surface>, EditError> {
    if !solid.sides.iter().any(|side| side.displacement.is_some()) {
        return Ok(Vec::new());
    }
    let geometry = BrushGeometry::from_solid(solid)
        .map_err(|error| invalid(format!("Brush {}: {error}", solid.id)))?;
    let mut surfaces = Vec::new();
    for (side, source) in solid.sides.iter().enumerate() {
        let Some(info) = &source.displacement else {
            continue;
        };
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == side)
            .ok_or_else(|| invalid("Displacement has no effective brush face"))?;
        let mesh =
            DisplacementMesh::build(face, info).map_err(|error| invalid(error.to_string()))?;
        let positions = mesh
            .positions
            .into_iter()
            .map(|point| mapping.point(point))
            .collect::<Result<Vec<_>, _>>()?;
        let base_positions = mesh
            .base_positions
            .into_iter()
            .map(|point| mapping.point(point))
            .collect::<Result<Vec<_>, _>>()?;
        surfaces.push(Surface {
            side,
            positions,
            base_positions,
        });
    }
    Ok(surfaces)
}

pub(super) fn verify(
    mapping: &Mapping,
    solid: &Solid,
    geometry: &BrushGeometry,
    surfaces: &[Surface],
) -> Result<(), EditError> {
    for surface in surfaces {
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == surface.side)
            .ok_or_else(|| invalid("Transformed displacement has no effective brush face"))?;
        let info = solid.sides[surface.side]
            .displacement
            .as_ref()
            .ok_or_else(|| invalid("Transformed displacement is missing"))?;
        let mesh =
            DisplacementMesh::build(face, info).map_err(|error| invalid(error.to_string()))?;
        for index in 0..mesh.positions.len() {
            let source = if mapping.reverses_winding {
                vertex(index, mesh.size)
            } else {
                index
            };
            if mesh.positions[index].distance(surface.positions[source]) > SAMPLE_EPSILON
                || mesh.base_positions[index].distance(surface.base_positions[source])
                    > SAMPLE_EPSILON
            {
                return Err(invalid(
                    "Displacement transform cannot preserve its source grid samples",
                ));
            }
            if mesh.positions[index].abs().max_element() > crate::primitive::MAX_COORDINATE {
                return Err(invalid(
                    "Deformed terrain exceeds the editor's coordinate bounds",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn transform(
    mapping: &Mapping,
    info: &mut DispInfo,
    normal: DVec3,
) -> Result<(), EditError> {
    if !(2..=4).contains(&info.power) {
        return Err(invalid(
            "Displacement transforms support powers 2 through 4",
        ));
    }
    let size = (1 << info.power) + 1;
    if mapping.deforms {
        validate(info, size)?;
        mapping_axes(mapping, info)?;
    }
    info.start_position = mapping.point(info.start_position)?;
    let elevation = mapping.linear * normal * info.elevation;
    if !elevation.is_finite() {
        return Err(invalid("Displacement elevation must be finite"));
    }
    if let (Some(normals), Some(distances)) = (&mut info.normals, &mut info.distances) {
        if normals.len() != size
            || distances.len() != size
            || normals.iter().any(|row| row.len() != size)
            || distances.iter().any(|row| row.len() != size)
        {
            return Err(invalid(
                "Displacement normal/distance rows have invalid dimensions",
            ));
        }
        for (normal, distance) in normals
            .iter_mut()
            .flatten()
            .zip(distances.iter_mut().flatten())
        {
            let vector = mapping.linear * *normal;
            let length = vector.length();
            if !length.is_finite() || !distance.is_finite() {
                return Err(invalid("Displacement vectors must be finite"));
            }
            *normal = vector.normalize_or_zero();
            *distance *= length;
            if !distance.is_finite() {
                return Err(invalid("Displacement distance overflow"));
            }
        }
    }
    if elevation != DVec3::ZERO && info.offsets.is_none() {
        info.offsets = Some(vec![vec![DVec3::ZERO; size]; size]);
    }
    if let Some(offsets) = &mut info.offsets {
        if offsets.len() != size || offsets.iter().any(|row| row.len() != size) {
            return Err(invalid("Displacement offset rows have invalid dimensions"));
        }
        for offset in offsets.iter_mut().flatten() {
            *offset = mapping.linear * *offset + elevation;
            if !offset.is_finite() {
                return Err(invalid("Displacement offset overflow"));
            }
        }
    }
    info.elevation = 0.0;
    if let Some(normals) = &mut info.offset_normals {
        for normal in normals.iter_mut().flatten() {
            let transformed = mapping.inverse_transpose * *normal;
            if !transformed.is_finite() {
                return Err(invalid("Displacement normal overflow"));
            }
            *normal = transformed.normalize_or_zero();
        }
    }
    if mapping.reverses_winding {
        reindex(info, size)?;
    }
    Ok(())
}
