use glam::DVec3;
use source_formats::bsp;

use super::{FIELD_WORK, PlanarField, Work, invalid};
use crate::{EditError, displacement::DisplacementMesh};

const PACK_WORK: usize = 8;
const ALPHA_WORK: usize = 8;
const MATERIAL_ALPHA_THRESHOLD: f32 = 382.5;

#[derive(Clone, Copy)]
struct ScalarField(PlanarField);

impl ScalarField {
    fn new(base: [DVec3; 4], values: [f64; 4]) -> Result<Self, EditError> {
        let axis = (0..3)
            .find(|&axis| base.iter().all(|point| point[axis] == base[0][axis]))
            .ok_or_else(|| invalid("Terrain paint requires a world-axis base plane"))?;
        let mut world = base;
        for (point, value) in world.iter_mut().zip(values) {
            point[axis] = value;
        }
        Ok(Self(PlanarField::new(base, world)?))
    }

    fn value(&self, base: DVec3) -> Result<f64, EditError> {
        Ok(self.0.position(base)?[self.0.axis()])
    }
}

fn alpha(value: f64) -> Result<f32, EditError> {
    let encoded = value as f32;
    if !(0.0..=255.0).contains(&value) || value.fract() != 0.0 || f64::from(encoded) != value {
        return Err(invalid(format!(
            "Terrain paint sample {value} must be an exact integral native alpha from 0 to 255"
        )));
    }
    Ok(encoded)
}

pub(super) struct PaintField {
    raw: ScalarField,
    packed: [ScalarField; 4],
}

impl PaintField {
    pub(super) fn triangle(
        base: [DVec3; 3],
        alphas: [f64; 3],
        work: &mut Work,
    ) -> Result<Self, EditError> {
        Self::new(
            [base[0], base[1], base[2], base[0]],
            [alphas[0], alphas[1], alphas[2], alphas[0]],
            work,
        )
    }

    pub(super) fn new(
        base: [DVec3; 4],
        values: [f64; 4],
        work: &mut Work,
    ) -> Result<Self, EditError> {
        work.consume(5 * 8 * FIELD_WORK + 4 * 4 * PACK_WORK)
            .map_err(invalid)?;
        let mut encoded = [0.0; 4];
        for (output, value) in encoded.iter_mut().zip(values) {
            *output = alpha(value)?;
        }
        let raw = ScalarField::new(base, values).map_err(|error| {
            invalid(format!(
                "Flat terrain alpha must form one exact affine field: {error}"
            ))
        })?;
        let mut packed = [raw; 4];
        for (field, rounding) in packed.iter_mut().zip(bsp::NativeDisplacementRounding::ALL) {
            let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
            let mut values = [0.0; 4];
            for (output, value) in values.iter_mut().zip(encoded) {
                *output = f64::from(
                    bsp::native_alpha_byte(value, profile)
                        .map_err(|error| invalid(error.to_string()))?,
                );
            }
            *field = ScalarField::new(base, values).map_err(|error| {
                invalid(format!(
                    "Flat terrain native packed alpha must form one exact affine field under {rounding:?}: {error}"
                ))
            })?;
        }
        Ok(Self { raw, packed })
    }

    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        work.consume(5 * FIELD_WORK).map_err(invalid)?;
        for (first, second) in std::iter::once(&self.raw)
            .chain(&self.packed)
            .zip(std::iter::once(&other.raw).chain(&other.packed))
        {
            if !first.0.matches(&second.0)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn alpha(&self, base: DVec3) -> Result<f64, EditError> {
        let value = self.raw.value(base)?;
        alpha(value)?;
        Ok(value)
    }

    pub(super) fn certify(
        &self,
        mesh: &DisplacementMesh,
        data: &bsp::Displacements,
        work: &mut Work,
    ) -> Result<(), EditError> {
        validate_data(mesh, data, work)?;
        work.consume((5 * FIELD_WORK + 4 * PACK_WORK) * mesh.alphas.len())
            .map_err(invalid)?;
        for ((&base, &value), vertex) in mesh
            .base_positions
            .iter()
            .zip(&mesh.alphas)
            .zip(&data.vertices)
        {
            if self.raw.value(base)? != value {
                return Err(invalid(
                    "Flat terrain authored and native alpha must share one exact affine field",
                ));
            }
            for (field, rounding) in self.packed.iter().zip(bsp::NativeDisplacementRounding::ALL) {
                let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
                let packed = bsp::native_alpha_byte(vertex.alpha, profile)
                    .map_err(|error| invalid(error.to_string()))?;
                if field.value(base)? != f64::from(packed) {
                    return Err(invalid(
                        "Flat terrain native packed alpha must remain affine under every rounding profile",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn validate_data(
    mesh: &DisplacementMesh,
    data: &bsp::Displacements,
    work: &mut Work,
) -> Result<(), EditError> {
    if !mesh.size.checked_mul(mesh.size).is_some_and(|count| {
        mesh.positions.len() == count
            && mesh.base_positions.len() == count
            && mesh.alphas.len() == count
            && data.vertices.len() == count
    }) {
        return Err(invalid(
            "Terrain paint samples must match their native grid",
        ));
    }
    work.consume(ALPHA_WORK * mesh.alphas.len())
        .map_err(invalid)?;
    for (&value, vertex) in mesh.alphas.iter().zip(&data.vertices) {
        if vertex.alpha.to_bits() != alpha(value)?.to_bits() {
            return Err(invalid(
                "Terrain authored and native alpha encodings must match exactly",
            ));
        }
    }
    Ok(())
}

pub(super) fn triangle_class(values: [f32; 3]) -> Result<bool, EditError> {
    let mut sum = 0.0;
    for value in values {
        alpha(f64::from(value))?;
        sum += value;
    }
    Ok(sum > MATERIAL_ALPHA_THRESHOLD)
}
