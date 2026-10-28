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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, Plane, texel_uv};

    fn spec() -> Spec {
        Spec {
            mins: DVec3::new(8.0, 16.0, 24.0),
            maxs: DVec3::new(72.0, 48.0, 40.0),
            rotation: DVec3::ZERO,
            material: "brick/test".into(),
            texture_scale: None,
            lightmap_scale: None,
        }
    }

    #[test]
    fn late_invalid_surface_restores_ids_history_and_map() {
        let mut document = Document::empty();
        let source = document.vmf().clone();
        let mut invalid = spec();
        invalid.lightmap_scale = Some(0);
        assert!(
            document
                .edit("batch", |tx| create(tx, &[spec(), invalid]))
                .is_err()
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.undo_steps().len(), 0);
        let ids = document.edit("batch", |tx| create(tx, &[spec()])).unwrap();
        let mut control = Document::new(source);
        let expected = control.edit("batch", |tx| create(tx, &[spec()])).unwrap();
        assert_eq!(ids, expected);
        let ObjectId::Solid(id) = ids[0] else {
            panic!()
        };
        let sides = |document: &Document| {
            document
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .map(|side| side.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(sides(&document), sides(&control));
    }

    #[test]
    fn heterogeneous_rotation_keeps_exact_texture_projection_and_one_step() {
        let mut document = Document::empty();
        let mut rotated = spec();
        rotated.rotation = DVec3::new(0.0, 0.0, 30.0);
        rotated.material = "metal/test".into();
        rotated.texture_scale = Some([0.5, -2.0]);
        rotated.lightmap_scale = Some(32);
        let ids = document
            .edit("batch", |tx| create(tx, &[spec(), rotated.clone()]))
            .unwrap();
        assert_eq!(document.undo_steps().len(), 1);
        let ObjectId::Solid(id) = ids[1] else {
            panic!()
        };
        let solid = document.solid(id).unwrap();
        assert!(
            solid
                .sides
                .iter()
                .all(|side| side.material == "METAL/TEST" && side.lightmap_scale == 32)
        );
        let mut baseline = Document::empty();
        let baseline_id = baseline
            .edit("base", |tx| {
                let mut unrotated = rotated.clone();
                unrotated.rotation = DVec3::ZERO;
                create(tx, &[unrotated])
            })
            .unwrap()[0];
        let ObjectId::Solid(baseline_id) = baseline_id else {
            panic!()
        };
        let pivot = rotated.mins + (rotated.maxs - rotated.mins) * 0.5;
        for (before, after) in baseline
            .solid(baseline_id)
            .unwrap()
            .sides
            .iter()
            .zip(&solid.sides)
        {
            for point in before.plane.0 {
                let relative = point - pivot;
                let cosine = 3.0_f64.sqrt() * 0.5;
                let mapped = pivot
                    + DVec3::new(
                        cosine * relative.x - relative.y * 0.5,
                        relative.x * 0.5 + cosine * relative.y,
                        relative.z,
                    );
                assert!(
                    after
                        .plane
                        .0
                        .iter()
                        .any(|point| point.abs_diff_eq(mapped, 1e-9))
                );
                assert!(texel_uv(before, point).abs_diff_eq(texel_uv(after, mapped), 1e-8));
            }
            assert!(Plane::from_points(after.plane.0).is_some());
        }
        document.undo().unwrap();
        assert!(document.vmf().world.solids.is_empty());
        document.redo().unwrap();
        assert_eq!(document.vmf().world.solids.len(), 2);
    }
}
