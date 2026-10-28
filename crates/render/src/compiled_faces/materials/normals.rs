use glam::{DVec2, DVec3};

use super::{Material, MaterialLighting, NormalEncoding, NormalTexture};

const MAX_PROOF_STEPS: usize = 1_048_576;
const MAX_POLYGON_VERTICES: usize = 64;
const PROOF_MARGIN: f64 = 1e-6;
const MAX_SUBDIVISIONS: u32 = 16;
type Cell = [DVec3; 4];

fn basis() -> [DVec3; 3] {
    [
        DVec3::new((2.0_f64 / 3.0).sqrt(), 0.0, (1.0_f64 / 3.0).sqrt()),
        DVec3::new(
            -(1.0_f64 / 6.0).sqrt(),
            (1.0_f64 / 2.0).sqrt(),
            (1.0_f64 / 3.0).sqrt(),
        ),
        DVec3::new(
            -(1.0_f64 / 6.0).sqrt(),
            -(1.0_f64 / 2.0).sqrt(),
            (1.0_f64 / 3.0).sqrt(),
        ),
    ]
}

fn vector(pixel: &[f32; 4]) -> DVec3 {
    DVec3::new(
        f64::from(pixel[0]),
        f64::from(pixel[1]),
        f64::from(pixel[2]),
    )
}

pub fn validate_directional_inputs(material: &Material) -> Result<(), String> {
    validate_directional_inputs_controlled(material, &mut || Ok(()))
}

pub fn validate_directional_inputs_controlled(
    material: &Material,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    check()?;
    let Some(primary) = &material.normals[0] else {
        if material.lighting == (MaterialLighting::Lightmap { directional: true })
            || material.normals[1].is_some()
        {
            return Err("Directional material requires its actual primary normal input".into());
        }
        return Ok(());
    };
    if material.lighting != (MaterialLighting::Lightmap { directional: true }) {
        return Err("Normal reconstruction requires a lit directional material".into());
    }
    for normal in material.normals.iter().flatten() {
        if normal.encoding != primary.encoding {
            return Err("Primary and secondary normals require the same encoding".into());
        }
        let texture = &normal.texture;
        if texture.width == 0
            || texture.height == 0
            || (texture.width as usize).checked_mul(texture.height as usize)
                != Some(texture.pixels.len())
        {
            return Err("Normal texture dimensions disagree with its pixels".into());
        }
        for (index, pixel) in texture.pixels.iter().enumerate() {
            if index % 1024 == 0 {
                check()?;
            }
            if pixel.iter().any(|value| !value.is_finite()) {
                return Err("Normal texture components must be finite".into());
            }
            if primary.encoding == NormalEncoding::SsBump {
                if pixel[..3].iter().any(|value| *value < 0.0) {
                    return Err("SSBump coefficients must be nonnegative".into());
                }
            } else if !defined(vector(pixel)) {
                return Err("Normal texel has an undefined directional response".into());
            }
        }
    }
    if primary.encoding == NormalEncoding::SsBump {
        return Ok(());
    }
    let pixels = material
        .normals
        .iter()
        .flatten()
        .flat_map(|normal| normal.texture.pixels.iter());
    if pixels
        .clone()
        .all(|pixel| vector(pixel).normalize().z > PROOF_MARGIN * 3.0_f64.sqrt())
        || certificate(pixels.map(vector), check)?
    {
        return Ok(());
    }
    let mut remaining = MAX_PROOF_STEPS;
    for y in 0..primary.texture.height {
        for x in 0..primary.texture.width {
            let first = cell(primary, x, y);
            if let Some(secondary) = &material.normals[1] {
                for second_y in 0..secondary.texture.height {
                    for second_x in 0..secondary.texture.width {
                        certify(
                            first,
                            Some(cell(secondary, second_x, second_y)),
                            0,
                            &mut remaining,
                            check,
                        )?;
                    }
                }
            } else {
                certify(first, None, 0, &mut remaining, check)?;
            }
        }
    }
    Ok(())
}

fn defined(normal: DVec3) -> bool {
    let Some(normal) = normal.try_normalize() else {
        return false;
    };
    basis().iter().any(|basis| normal.dot(*basis) > 0.0)
}

fn certificate(
    normals: impl IntoIterator<Item = DVec3>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<bool, String> {
    // A positive nonnegative basis combination certifies every convex mixture.
    let mut polygon = vec![DVec2::X, DVec2::Y, DVec2::ZERO];
    let basis = basis();
    for (index, normal) in normals.into_iter().enumerate() {
        if index % 1024 == 0 {
            check()?;
        }
        let Some(normal) = normal.try_normalize() else {
            return Ok(false);
        };
        let projected = basis.map(|basis| normal.dot(basis));
        let a = projected[0] - projected[2];
        let b = projected[1] - projected[2];
        let c = projected[2] - PROOF_MARGIN;
        let mut clipped = Vec::with_capacity(polygon.len() + 1);
        let mut previous = *polygon.last().unwrap();
        let mut previous_value = a * previous.x + b * previous.y + c;
        for point in &polygon {
            let value = a * point.x + b * point.y + c;
            if (value >= 0.0) != (previous_value >= 0.0) {
                let fraction = previous_value / (previous_value - value);
                clipped.push(previous + (*point - previous) * fraction);
            }
            if value >= 0.0 {
                clipped.push(*point);
            }
            previous = *point;
            previous_value = value;
        }
        if clipped.is_empty() || clipped.len() > MAX_POLYGON_VERTICES {
            return Ok(false);
        }
        polygon = clipped;
    }
    Ok(true)
}

fn cell(normal: &NormalTexture, x: u32, y: u32) -> Cell {
    let texture = &normal.texture;
    let next_x = (x + 1) % texture.width;
    let next_y = (y + 1) % texture.height;
    [(x, y), (next_x, y), (x, next_y), (next_x, next_y)]
        .map(|(x, y)| vector(&texture.pixels[(y * texture.width + x) as usize]))
}

fn midpoint(cell: Cell) -> DVec3 {
    cell.into_iter().sum::<DVec3>() / 4.0
}

fn variation(cell: Cell) -> f64 {
    cell.into_iter()
        .map(|corner| corner.distance_squared(midpoint(cell)))
        .fold(0.0, f64::max)
}

fn split(cell: Cell) -> [Cell; 4] {
    let top = (cell[0] + cell[1]) / 2.0;
    let bottom = (cell[2] + cell[3]) / 2.0;
    let left = (cell[0] + cell[2]) / 2.0;
    let right = (cell[1] + cell[3]) / 2.0;
    let middle = midpoint(cell);
    [
        [cell[0], top, left, middle],
        [top, cell[1], middle, right],
        [left, middle, cell[2], bottom],
        [middle, right, bottom, cell[3]],
    ]
}

fn certify(
    first: Cell,
    second: Option<Cell>,
    depth: u32,
    remaining: &mut usize,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if *remaining == 0 || depth > MAX_SUBDIVISIONS {
        return Err("Filtered directional normal response could not be certified within the material preparation working budget".into());
    }
    *remaining -= 1;
    if (*remaining).is_multiple_of(1024) {
        check()?;
    }
    if certificate(first.into_iter().chain(second.into_iter().flatten()), check)? {
        return Ok(());
    }
    let middle = second.map_or_else(
        || midpoint(first),
        |second| (midpoint(first) + midpoint(second)) / 2.0,
    );
    if !defined(middle) {
        return Err(
            "Filtered normal or vertex-alpha mixture has an undefined directional response".into(),
        );
    }
    if second.is_none_or(|second| variation(first) >= variation(second)) {
        for part in split(first) {
            certify(part, second, depth + 1, remaining, check)?;
        }
    } else if let Some(second) = second {
        for part in split(second) {
            certify(first, Some(part), depth + 1, remaining, check)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiled_faces::materials::{Coverage, Kind, Texture};
    use std::sync::Arc;

    fn material(first: Vec<[f32; 4]>, second: Option<Vec<[f32; 4]>>) -> Material {
        let texture = |pixels: Vec<[f32; 4]>| NormalTexture {
            texture: Arc::new(Texture {
                width: pixels.len() as u32,
                height: 1,
                pixels,
            }),
            encoding: NormalEncoding::Tangent,
        };
        Material {
            name: "filter-domain".into(),
            kind: Kind::VertexTransition,
            diffuse: [None, None],
            normals: [Some(texture(first)), second.map(texture)],
            diffuse_transform: [[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 2],
            normal_transform: [[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 2],
            color: [1.0; 3],
            detail_alpha_mask: false,
            lighting: MaterialLighting::Lightmap { directional: true },
            coverage: Coverage::Opaque,
            nocull: false,
        }
    }

    #[test]
    fn lower_normals_with_defined_filtered_response_are_admitted() {
        let material = material(
            vec![[1.0, 0.0, -0.2, 1.0]],
            Some(vec![[1.0, 0.4, -0.1, 1.0]]),
        );
        validate_directional_inputs(&material).unwrap();
    }

    #[test]
    fn defined_texels_do_not_hide_undefined_repeat_filter_or_vertex_blend() {
        let first = [1.0, 0.0, -0.2, 1.0];
        let second = [-1.0, 0.0, -0.2, 1.0];
        assert!(defined(vector(&first)) && defined(vector(&second)));
        let filtered = material(vec![first, second], None);
        assert!(
            validate_directional_inputs(&filtered)
                .unwrap_err()
                .contains("Filtered normal")
        );
        let blended = material(vec![first], Some(vec![second]));
        assert!(
            validate_directional_inputs(&blended)
                .unwrap_err()
                .contains("vertex-alpha mixture")
        );
    }
}
