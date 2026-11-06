use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::bsp;

use super::field::ScalarField;
use super::{Exact, FIELD_WORK, Work, invalid, labels};
use crate::clip::{TerrainBlendError, TerrainBlendMode};
use crate::{EditError, displacement::DisplacementMesh};

const PACK_WORK: usize = 8;
const ALPHA_WORK: usize = 8;
const DIFFERENCE_WORK: usize = 8;

fn scalar_error(field: &ScalarField, base: DVec3, actual: f64) -> Result<f64, EditError> {
    Ok(Exact::float(field.value(base)?)?
        .sub(Exact::float(actual)?)?
        .value()?
        .abs())
}

fn alpha(value: f64) -> Result<f32, EditError> {
    Ok(TerrainBlendMode::Exact.sample_alpha(value)? as f32)
}

pub(super) fn sample_alpha(value: f64, mode: TerrainBlendMode) -> Result<f64, EditError> {
    mode.sample_alpha(value)
}

fn packed_alpha(value: f32, rounding: bsp::NativeDisplacementRounding) -> Result<f64, EditError> {
    let profile = bsp::NativeDisplacementProfile::CssLinux64Build25338318 { rounding };
    Ok(f64::from(
        bsp::native_alpha_byte(value, profile).map_err(|error| invalid(error.to_string()))?,
    ))
}

pub(super) struct PaintField {
    raw: ScalarField,
    packed: [ScalarField; 4],
    source_base: [DVec3; 4],
    source_alpha: [f64; 4],
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
            let mut values = [0.0; 4];
            for (output, value) in values.iter_mut().zip(encoded) {
                *output = packed_alpha(value, rounding)?;
            }
            *field = ScalarField::new(base, values).map_err(|error| {
                invalid(format!(
                    "Flat terrain native packed alpha must form one exact affine field under {rounding:?}: {error}"
                ))
            })?;
        }
        Ok(Self {
            raw,
            packed,
            source_base: base,
            source_alpha: values,
        })
    }

    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        work.consume(5 * FIELD_WORK).map_err(invalid)?;
        for (first, second) in std::iter::once(&self.raw)
            .chain(&self.packed)
            .zip(std::iter::once(&other.raw).chain(&other.packed))
        {
            if !first.matches(second)? {
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

    pub(super) fn sample_alpha(
        &self,
        base: DVec3,
        mode: TerrainBlendMode,
    ) -> Result<f64, EditError> {
        for (source_base, source_alpha) in self.source_base.iter().zip(self.source_alpha) {
            if *source_base == base {
                return Ok(source_alpha);
            }
        }
        if mode == TerrainBlendMode::Exact {
            self.alpha(base)
        } else {
            sample_alpha(self.raw.value(base)?, mode)
        }
    }

    pub(super) fn rounding_error(
        &self,
        base: [DVec3; 3],
        values: [f64; 3],
        mode: TerrainBlendMode,
        work: &mut Work,
    ) -> Result<TerrainBlendError, EditError> {
        work.consume(3 * (5 * (FIELD_WORK + DIFFERENCE_WORK) + 4 * PACK_WORK + ALPHA_WORK))
            .map_err(invalid)?;
        let mut error = TerrainBlendError::default();
        for (base, value) in base.into_iter().zip(values) {
            let encoded = alpha(value)?;
            error.raw = error.raw.max(scalar_error(&self.raw, base, value)?);
            for ((field, bound), rounding) in self
                .packed
                .iter()
                .zip(&mut error.packed)
                .zip(bsp::NativeDisplacementRounding::ALL)
            {
                *bound = (*bound).max(scalar_error(field, base, packed_alpha(encoded, rounding)?)?);
            }
        }
        error.validate(mode)?;
        Ok(error)
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
                if field.value(base)? != packed_alpha(vertex.alpha, rounding)? {
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

pub(super) fn certify_lod(
    mesh: &DisplacementMesh,
    fine: &BTreeSet<[usize; 3]>,
    maximal: &BTreeSet<[usize; 3]>,
    work: &mut Work,
) -> Result<(), EditError> {
    if fine == maximal {
        return Ok(());
    }
    let removed_count = fine.difference(maximal).count();
    let added_count = maximal.difference(fine).count();
    if removed_count == 0 || added_count == 0 {
        return Err(invalid(
            "Terrain LOD paint requires both original fine and maximal changed triangles",
        ));
    }
    work.consume(
        FIELD_WORK * (removed_count + added_count + 1)
            + labels::AABB_WORK * removed_count * added_count
            + removed_count,
    )
    .map_err(invalid)?;
    let (_, axes) = labels::projection(mesh, work)?;
    let mut removed = Vec::with_capacity(removed_count);
    for &triangle in fine.difference(maximal) {
        let points = triangle.map(|index| mesh.base_positions[index]);
        removed.push((
            labels::Projected::new(points, axes)?,
            PaintField::triangle(points, triangle.map(|index| mesh.alphas[index]), work)?,
            false,
        ));
    }
    for &triangle in maximal.difference(fine) {
        let points = triangle.map(|index| mesh.base_positions[index]);
        let projected = labels::Projected::new(points, axes)?;
        let paint = PaintField::triangle(points, triangle.map(|index| mesh.alphas[index]), work)?;
        let mut matched = false;
        for (source, source_paint, covered) in &mut removed {
            if !projected.overlaps(source, axes, work)? {
                continue;
            }
            if !paint.matches(source_paint, work)? {
                return Err(invalid(
                    "Native LOD triangle changed an overlapping original raw or native packed paint field",
                ));
            }
            *covered = true;
            matched = true;
        }
        if !matched {
            return Err(invalid(
                "Native LOD paint triangle has no positive-area original fine coverage",
            ));
        }
    }
    if removed.iter().any(|(_, _, covered)| !*covered) {
        return Err(invalid(
            "Original fine paint triangle has no positive-area native LOD coverage",
        ));
    }
    Ok(())
}
