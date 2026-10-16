use source_formats::bsp::DisplacementVertex;

use super::{MAX_MESH_TRIANGLES, surface_index};
use crate::Control;

const ALPHA_PROPERTY_THRESHOLD: f32 = 382.5;

pub(super) fn materials(
    vertices: &[DisplacementVertex],
    triangles: &[[usize; 3]],
    primary: u8,
    secondary: Option<&str>,
    names: &mut Vec<String>,
    control: &Control,
) -> Result<Vec<u8>, String> {
    control.checkpoint()?;
    if triangles.len() > MAX_MESH_TRIANGLES {
        return Err("Terrain physics exceeds its 65,536-triangle budget".into());
    }
    let Some(secondary) = secondary else {
        return Ok(vec![primary; triangles.len()]);
    };
    let mut material = None;
    let mut result = Vec::with_capacity(triangles.len());
    for triangle in triangles {
        control.checkpoint()?;
        let mut alpha = 0.0_f32;
        for &index in triangle {
            let value = vertices
                .get(index)
                .ok_or("Terrain physics surface selection requires valid vertex indices")?
                .alpha;
            if !value.is_finite() {
                return Err(
                    "Terrain physics surface selection requires finite native alpha".into(),
                );
            }
            alpha += value;
        }
        result.push(if alpha > ALPHA_PROPERTY_THRESHOLD {
            match material {
                Some(index) => index,
                None => {
                    let index = surface_index(secondary, names)?;
                    material = Some(index);
                    index
                }
            }
        } else {
            primary
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
