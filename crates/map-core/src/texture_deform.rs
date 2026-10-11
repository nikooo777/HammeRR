use glam::{DMat3, DVec3};
use source_formats::vmf::{Side, UvAxis};

use crate::surface::Projection;
use crate::{EditError, Plane, texel_uv};

const TEXEL_EPSILON: f64 = 0.0001;

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn axis(source: UvAxis, inverse_transpose: DMat3, offset: DVec3) -> Result<UvAxis, EditError> {
    let transformed = inverse_transpose * source.axis;
    let length = transformed.length();
    let scale = if source.scale == 0.0 {
        1.0
    } else {
        source.scale
    };
    if !length.is_finite() || length <= 0.0 || !scale.is_finite() || !source.offset.is_finite() {
        return Err(invalid("Cannot attach an invalid texture projection"));
    }
    let mut result = UvAxis {
        axis: transformed / length,
        scale: scale / length,
        offset: source.offset,
    };
    result.offset -= offset.dot(result.axis) / result.scale;
    if !result.axis.is_finite()
        || !result.scale.is_finite()
        || result.scale == 0.0
        || !result.offset.is_finite()
    {
        return Err(invalid(
            "Attached texture projection exceeds numeric limits",
        ));
    }
    Ok(result)
}

pub fn attach(side: &Side, before: &[DVec3], after: &[DVec3]) -> Result<Projection, EditError> {
    if before.len() != after.len()
        || before.len() < 3
        || before.iter().chain(after).any(|point| !point.is_finite())
    {
        return Err(invalid(
            "Texture attachment needs matching finite face corners",
        ));
    }
    let index = (1..after.len() - 1)
        .max_by(|a, b| {
            let area = |index: usize| {
                (after[index] - after[0])
                    .cross(after[index + 1] - after[0])
                    .length_squared()
            };
            area(*a).total_cmp(&area(*b))
        })
        .unwrap();
    let old_plane = Plane::from_points([before[0], before[index], before[index + 1]])
        .ok_or_else(|| invalid("Cannot attach textures to a degenerate source face"))?;
    let new_plane = Plane::from_points([after[0], after[index], after[index + 1]])
        .ok_or_else(|| invalid("Cannot attach textures to a collapsed face"))?;
    if !side.u_axis.axis.is_finite()
        || !side.v_axis.axis.is_finite()
        || side
            .u_axis
            .axis
            .cross(side.v_axis.axis)
            .dot(old_plane.normal)
            .abs()
            < 1e-10
    {
        return Err(invalid(
            "Texture attachment requires independent U and V directions on the source face",
        ));
    }
    let old_basis = DMat3::from_cols(
        before[index] - before[0],
        before[index + 1] - before[0],
        old_plane.normal,
    );
    let new_basis = DMat3::from_cols(
        after[index] - after[0],
        after[index + 1] - after[0],
        new_plane.normal,
    );
    let linear = new_basis * old_basis.inverse();
    let offset = after[0] - linear * before[0];
    let inverse_transpose = linear.inverse().transpose();
    let projection = Projection {
        u: axis(side.u_axis, inverse_transpose, offset)?,
        v: axis(side.v_axis, inverse_transpose, offset)?,
        rotation: side.rotation,
    };
    let mut mapped = side.clone();
    mapped.u_axis = projection.u;
    mapped.v_axis = projection.v;
    for (before, after) in before.iter().zip(after) {
        let expected = texel_uv(side, *before);
        let actual = texel_uv(&mapped, *after);
        if !expected.is_finite()
            || !actual.is_finite()
            || (expected - actual).abs().max_element() > TEXEL_EPSILON
        {
            return Err(invalid(
                "This face cannot keep every corner's texture coordinates with a single Source projection; disable texture attachment or move the whole face",
            ));
        }
    }
    Ok(projection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, ObjectId, edit};

    fn side() -> Side {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let mut side = document
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .clone();
        side.u_axis.offset = 13.5;
        side.v_axis.offset = -17.25;
        side.v_axis.scale = -0.5;
        side
    }

    fn square() -> Vec<DVec3> {
        vec![
            DVec3::new(0.0, 0.0, 64.0),
            DVec3::new(0.0, 64.0, 64.0),
            DVec3::new(64.0, 64.0, 64.0),
            DVec3::new(64.0, 0.0, 64.0),
        ]
    }

    #[test]
    fn affine_stretch_shear_rotation_and_translation_preserve_corner_texels() {
        let source = side();
        let before = square();
        for linear in [
            DMat3::IDENTITY,
            DMat3::from_diagonal(DVec3::new(1.5, 0.75, 1.0)),
            DMat3::from_cols(DVec3::X, DVec3::new(0.5, 1.0, 0.0), DVec3::Z),
            DMat3::from_rotation_x(0.7),
        ] {
            let after = before
                .iter()
                .map(|point| linear * point + DVec3::new(123.0, -43.0, 17.0))
                .collect::<Vec<_>>();
            let result = attach(&source, &before, &after).unwrap();
            let mut changed = source.clone();
            changed.u_axis = result.u;
            changed.v_axis = result.v;
            for (before, after) in before.iter().zip(after) {
                assert!(texel_uv(&source, *before).abs_diff_eq(texel_uv(&changed, after), 1e-8));
            }
        }
    }

    #[test]
    fn triangles_attach_but_non_affine_quadrilaterals_reject() {
        let source = side();
        let before = square();
        let mut after = before.clone();
        after[2].x += 16.0;
        assert!(attach(&source, &before, &after).is_err());
        assert!(attach(&source, &before[..3], &after[..3]).is_ok());
    }

    #[test]
    fn tiny_faces_at_large_coordinates_and_invalid_projections() {
        let mut source = side();
        let before = square()
            .iter()
            .map(|point| point / 512.0 + DVec3::splat(32_000.0))
            .collect::<Vec<_>>();
        let after = before
            .iter()
            .map(|point| (*point - DVec3::splat(32_000.0)) * 2.0 + DVec3::splat(32_000.0))
            .collect::<Vec<_>>();
        assert!(attach(&source, &before, &after).is_ok());
        source.u_axis.scale = f64::NAN;
        assert!(attach(&source, &before, &after).is_err());
        source.u_axis.scale = 0.25;
        source.v_axis.axis = source.u_axis.axis;
        assert!(attach(&source, &before, &after).is_err());
    }
}
