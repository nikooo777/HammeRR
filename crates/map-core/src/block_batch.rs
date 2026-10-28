use glam::DVec3;

use crate::surface::{self, FaceId, SurfaceEdit};
use crate::{EditError, ObjectId, Transaction, edit, primitive, transform};

pub const MAX_BLOCKS: usize = 256;
pub const MAX_MATERIAL_BYTES: usize = 1024;

#[derive(Debug, Clone)]
pub struct Spec {
    pub mins: DVec3,
    pub maxs: DVec3,
    pub rotation: DVec3,
    pub material: String,
    pub texture_scale: Option<[f64; 2]>,
    pub lightmap_scale: Option<u32>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn create_one(tx: &mut Transaction, spec: &Spec) -> Result<ObjectId, EditError> {
    if !spec.mins.is_finite()
        || !spec.maxs.is_finite()
        || !(spec.maxs - spec.mins)
            .cmpge(DVec3::splat(primitive::MIN_SIZE))
            .all()
        || spec.mins.abs().max(spec.maxs.abs()).max_element() > primitive::MAX_COORDINATE
    {
        return Err(invalid(
            "Block bounds must be finite, within ±65536 units, and at least 0.125 units thick on every axis",
        ));
    }
    if spec.material.len() > MAX_MATERIAL_BYTES {
        return Err(invalid(format!(
            "Material paths must be at most {MAX_MATERIAL_BYTES} bytes"
        )));
    }
    if spec.material.chars().any(char::is_control) {
        return Err(invalid(
            "Material paths must not contain control characters",
        ));
    }
    let id = edit::create_block(tx, spec.mins, spec.maxs, &spec.material)?;
    let ObjectId::Solid(solid) = id else {
        unreachable!()
    };
    if spec.texture_scale.is_some() || spec.lightmap_scale.is_some() {
        let faces = tx
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .map(|side| FaceId {
                solid,
                side: side.id,
            })
            .collect::<Vec<_>>();
        surface::edit_faces(
            tx,
            &faces,
            &SurfaceEdit {
                scale_u: spec.texture_scale.map(|scale| scale[0]),
                scale_v: spec.texture_scale.map(|scale| scale[1]),
                lightmap_scale: spec.lightmap_scale,
                ..Default::default()
            },
        )?;
    }
    transform::transform(
        tx,
        &[id],
        transform::Transform {
            rotation: spec.rotation,
            pivot: spec.mins + (spec.maxs - spec.mins) * 0.5,
            ..Default::default()
        },
    )?;
    let geometry = transform::validated_geometry(tx.document().solid(solid).unwrap())?;
    if geometry.mins.abs().max(geometry.maxs.abs()).max_element() > primitive::MAX_COORDINATE {
        return Err(invalid(
            "Rotated block exceeds the editor's coordinate bounds",
        ));
    }
    Ok(id)
}

pub fn create(tx: &mut Transaction, specs: &[Spec]) -> Result<Vec<ObjectId>, EditError> {
    if !(1..=MAX_BLOCKS).contains(&specs.len()) {
        return Err(invalid(format!(
            "Create between 1 and {MAX_BLOCKS} blocks per batch"
        )));
    }
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            create_one(tx, spec).map_err(|error| invalid(format!("Block {}: {error}", index + 1)))
        })
        .collect()
}
