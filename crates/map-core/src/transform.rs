use std::collections::BTreeSet;

use glam::{DMat3, DQuat, DVec3, EulerRot};
use source_formats::vmf::{
    DispInfo, Entity, Solid, UvAxis, format_number, format_vec3, parse_vec3,
};

use crate::{BrushGeometry, EditError, ObjectId, Plane, Transaction};

pub const MAX_SHEAR_FACTOR: f64 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shear {
    pub axis: usize,
    pub reference: usize,
    pub factor: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: DVec3,
    pub rotation: DVec3,
    pub scale: DVec3,
    pub reflection: [bool; 3],
    pub shear: Option<Shear>,
    pub pivot: DVec3,
    pub basis: DMat3,
    pub texture_lock: bool,
    pub texture_scale_lock: Option<bool>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: DVec3::ZERO,
            rotation: DVec3::ZERO,
            scale: DVec3::ONE,
            reflection: [false; 3],
            shear: None,
            pivot: DVec3::ZERO,
            basis: DMat3::IDENTITY,
            texture_lock: true,
            texture_scale_lock: None,
        }
    }
}

struct Mapping {
    linear: DMat3,
    inverse_transpose: DMat3,
    rotation: DQuat,
    offset: DVec3,
    texture_lock: bool,
    translation_texture_lock: bool,
    texture_inverse_transpose: DMat3,
    texture_offset: DVec3,
    translation_only: bool,
    brush_only: bool,
    reverses_winding: bool,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

pub(crate) fn validated_geometry(solid: &Solid) -> Result<BrushGeometry, EditError> {
    let geometry = BrushGeometry::from_solid(solid)
        .map_err(|error| invalid(format!("Brush {}: {error}", solid.id)))?;
    if !geometry.mins.is_finite()
        || !geometry.maxs.is_finite()
        || !geometry.mins.cmplt(geometry.maxs).all()
    {
        return Err(invalid(format!(
            "Brush {} has invalid bounds after transform",
            solid.id
        )));
    }
    Ok(geometry)
}

impl Mapping {
    fn new(transform: Transform) -> Result<Self, EditError> {
        if !transform.translation.is_finite()
            || !transform.rotation.is_finite()
            || !transform.scale.is_finite()
            || !transform.pivot.is_finite()
            || !transform.basis.is_finite()
            || !transform.scale.cmpgt(DVec3::ZERO).all()
        {
            return Err(invalid(
                "Transform values must be finite and scale factors must be positive",
            ));
        }
        if !(transform.basis.transpose() * transform.basis).abs_diff_eq(DMat3::IDENTITY, 1e-8)
            || (transform.basis.determinant() - 1.0).abs() > 1e-8
        {
            return Err(invalid(
                "Transform basis must be a right-handed orthonormal frame",
            ));
        }
        let reflected = transform.reflection.into_iter().any(|axis| axis);
        let mut shear_matrix = DMat3::IDENTITY;
        if let Some(shear) = transform.shear {
            if shear.axis >= 3
                || shear.reference >= 3
                || shear.axis == shear.reference
                || !shear.factor.is_finite()
                || shear.factor.abs() > MAX_SHEAR_FACTOR
            {
                return Err(invalid(
                    "Shear needs distinct X/Y/Z axes and a finite factor between -64 and 64",
                ));
            }
            let mut columns = [DVec3::X, DVec3::Y, DVec3::Z];
            columns[shear.reference][shear.axis] = shear.factor;
            shear_matrix = DMat3::from_cols(columns[0], columns[1], columns[2]);
        }
        let brush_only = reflected || shear_matrix != DMat3::IDENTITY;
        if brush_only && transform.pivot.abs().max_element() > crate::primitive::MAX_COORDINATE {
            return Err(invalid(
                "Brush deformation pivot exceeds the editor's coordinate bounds",
            ));
        }
        let signed_scale = transform.scale
            * DVec3::from_array(
                transform
                    .reflection
                    .map(|axis| if axis { -1.0 } else { 1.0 }),
            );
        let deformation = shear_matrix * DMat3::from_diagonal(signed_scale);
        let angles = transform
            .rotation
            .map(|angle| angle.rem_euclid(360.0).to_radians());
        let local_rotation = DMat3::from_quat(DQuat::from_euler(
            EulerRot::ZYX,
            angles.z,
            angles.y,
            angles.x,
        ));
        let rotation_matrix = if angles == DVec3::ZERO {
            DMat3::IDENTITY
        } else {
            transform.basis * local_rotation * transform.basis.transpose()
        };
        let rotation = DQuat::from_mat3(&rotation_matrix);
        let linear = if deformation == DMat3::IDENTITY {
            rotation_matrix
        } else {
            rotation_matrix * transform.basis * deformation * transform.basis.transpose()
        };
        let inverse_transpose = linear.inverse().transpose();
        let offset = transform.pivot - linear * transform.pivot + transform.translation;
        if !inverse_transpose.is_finite() || !offset.is_finite() {
            return Err(invalid("Transform is too large or too small"));
        }
        let scale_lock = transform
            .texture_scale_lock
            .unwrap_or(transform.texture_lock);
        let texture_linear = match (transform.texture_lock, scale_lock) {
            (true, true) => linear,
            (true, false) => rotation_matrix,
            (false, true) if deformation != DMat3::IDENTITY => {
                transform.basis * deformation * transform.basis.transpose()
            }
            (false, _) => DMat3::IDENTITY,
        };
        let texture_inverse_transpose = texture_linear.inverse().transpose();
        let texture_offset = transform.pivot - texture_linear * transform.pivot
            + if transform.texture_lock {
                transform.translation
            } else {
                DVec3::ZERO
            };
        if !texture_inverse_transpose.is_finite() || !texture_offset.is_finite() {
            return Err(invalid("Transform produces an invalid texture mapping"));
        }
        Ok(Self {
            linear,
            inverse_transpose,
            rotation,
            offset,
            texture_lock: transform.texture_lock || scale_lock,
            translation_texture_lock: transform.texture_lock,
            texture_inverse_transpose,
            texture_offset,
            translation_only: angles == DVec3::ZERO && deformation == DMat3::IDENTITY,
            brush_only,
            reverses_winding: transform
                .reflection
                .into_iter()
                .filter(|axis| *axis)
                .count()
                % 2
                == 1,
        })
    }

    fn point(&self, point: DVec3) -> Result<DVec3, EditError> {
        let result = self.linear * point + self.offset;
        if !result.is_finite() {
            return Err(invalid("Transform produces a nonfinite coordinate"));
        }
        Ok(result)
    }

    fn axis(&self, axis: &mut UvAxis) -> Result<(), EditError> {
        if self.texture_inverse_transpose == DMat3::IDENTITY && self.texture_offset == DVec3::ZERO {
            return Ok(());
        }
        let transformed = self.texture_inverse_transpose * axis.axis;
        let length = transformed.length();
        let scale = if axis.scale == 0.0 { 1.0 } else { axis.scale };
        if !length.is_finite() || length <= 0.0 || !scale.is_finite() || !axis.offset.is_finite() {
            return Err(invalid("Cannot lock an invalid texture projection"));
        }
        axis.axis = transformed / length;
        axis.scale = scale / length;
        axis.offset -= self.texture_offset.dot(axis.axis) / axis.scale;
        if !axis.scale.is_finite() || axis.scale == 0.0 || !axis.offset.is_finite() {
            return Err(invalid("Transform produces an invalid texture projection"));
        }
        Ok(())
    }

    fn displacement(&self, disp: &mut DispInfo, normal: DVec3) -> Result<(), EditError> {
        if !(2..=4).contains(&disp.power) {
            return Err(invalid(
                "Displacement transforms support powers 2 through 4",
            ));
        }
        let size = (1 << disp.power) + 1;
        disp.start_position = self.point(disp.start_position)?;
        let elevation = self.linear * normal * disp.elevation;
        if !elevation.is_finite() {
            return Err(invalid("Displacement elevation must be finite"));
        }
        if let (Some(normals), Some(distances)) = (&mut disp.normals, &mut disp.distances) {
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
                let vector = self.linear * *normal;
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
        if elevation != DVec3::ZERO && disp.offsets.is_none() {
            disp.offsets = Some(vec![vec![DVec3::ZERO; size]; size]);
        }
        if let Some(offsets) = &mut disp.offsets {
            if offsets.len() != size || offsets.iter().any(|row| row.len() != size) {
                return Err(invalid("Displacement offset rows have invalid dimensions"));
            }
            for offset in offsets.iter_mut().flatten() {
                *offset = self.linear * *offset + elevation;
                if !offset.is_finite() {
                    return Err(invalid("Displacement offset overflow"));
                }
            }
        }
        disp.elevation = 0.0;
        if let Some(normals) = &mut disp.offset_normals {
            for normal in normals.iter_mut().flatten() {
                let transformed = self.inverse_transpose * *normal;
                if !transformed.is_finite() {
                    return Err(invalid("Displacement normal overflow"));
                }
                *normal = transformed.normalize_or_zero();
            }
        }
        Ok(())
    }

    fn solid(&self, solid: &mut Solid) -> Result<(), EditError> {
        if self.brush_only && solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(
                "Brush deformation does not yet support displacement brushes",
            ));
        }
        if self.translation_only {
            return crate::edit::translate_solid(solid, self.offset, self.translation_texture_lock);
        }
        for side in &mut solid.sides {
            if side.plane.0.iter().any(|point| !point.is_finite())
                || side
                    .displacement
                    .as_ref()
                    .is_some_and(|disp| !disp.start_position.is_finite())
            {
                return Err(invalid("Transform produces a nonfinite brush coordinate"));
            }
            let normal = Plane::from_points(side.plane.0)
                .ok_or_else(|| invalid(format!("Brush {} has a degenerate face", solid.id)))?
                .normal;
            for point in &mut side.plane.0 {
                *point = self.point(*point)?;
            }
            if self.reverses_winding {
                side.plane.0.swap(0, 2);
            }
            if self.texture_lock {
                self.axis(&mut side.u_axis)?;
                self.axis(&mut side.v_axis)?;
            }
            if let Some(disp) = &mut side.displacement {
                self.displacement(disp, normal)?;
            }
        }
        let geometry = validated_geometry(solid)?;
        if self.brush_only
            && geometry.mins.abs().max(geometry.maxs.abs()).max_element()
                > crate::primitive::MAX_COORDINATE
        {
            return Err(invalid(format!(
                "Deformed brush {} exceeds the editor's coordinate bounds",
                solid.id
            )));
        }
        Ok(())
    }

    fn entity(&self, entity: &mut Entity, rotate: bool) -> Result<(), EditError> {
        if self.brush_only && entity.solids.is_empty() {
            return Err(invalid(
                "Brush deformation currently supports brushes and brush entities only",
            ));
        }
        if let Some(origin) = crate::edit::checked_entity_origin(entity)? {
            entity
                .properties
                .set("origin", format_vec3(self.point(origin)?));
        } else if entity.solids.is_empty() {
            return Err(invalid(format!(
                "Entity {} has no origin to transform",
                entity.id
            )));
        }
        if rotate && entity.solids.is_empty() {
            self.orientation(entity)?;
        }
        for solid in &mut entity.solids {
            self.solid(solid)?;
        }
        Ok(())
    }

    fn orientation(&self, entity: &mut Entity) -> Result<(), EditError> {
        let mut angles = if let Some(value) = entity.properties.get("angles") {
            parse_vec3(value)
                .filter(|point| point.is_finite())
                .ok_or_else(|| invalid(format!("Entity {} has invalid angles", entity.id)))?
        } else if let Some(value) = entity.properties.get("angle") {
            let angle = value
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|angle| angle.is_finite())
                .ok_or_else(|| invalid(format!("Entity {} has an invalid angle", entity.id)))?;
            match angle {
                -1.0 => DVec3::new(-90.0, 0.0, 0.0),
                -2.0 => DVec3::new(90.0, 0.0, 0.0),
                _ => DVec3::new(0.0, angle, 0.0),
            }
        } else {
            DVec3::ZERO
        };
        let pitch = matches!(entity.classname(), Some("light_spot" | "light_environment"))
            && entity.properties.get("pitch").is_some();
        if pitch {
            angles.x = -entity
                .properties
                .get("pitch")
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite())
                .ok_or_else(|| invalid("Light pitch must be finite"))?;
        }
        let angles = angles.map(|angle| angle.rem_euclid(360.0).to_radians());
        let orientation =
            self.rotation * DQuat::from_euler(EulerRot::ZYX, angles.y, angles.x, angles.z);
        let (yaw, pitch_angle, roll) = orientation.to_euler(EulerRot::ZYX);
        let angles = DVec3::new(pitch_angle, yaw, roll).map(|value| {
            let degrees = value.to_degrees();
            if degrees.abs() < 1e-9 { 0.0 } else { degrees }
        });
        entity.properties.set("angles", format_vec3(angles));
        entity.properties.remove("angle");
        if pitch {
            entity.properties.set("pitch", format_number(-angles.x));
        }
        Ok(())
    }
}

pub fn transform_point(point: DVec3, transform: Transform) -> Result<DVec3, EditError> {
    Mapping::new(transform)?.point(point)
}

pub fn transform(
    tx: &mut Transaction,
    ids: &[ObjectId],
    transform: Transform,
) -> Result<(), EditError> {
    let mapping = Mapping::new(transform)?;
    let rotate = transform.rotation.map(|value| value.rem_euclid(360.0)) != DVec3::ZERO;
    if ids.is_empty() {
        return Err(invalid("Select objects to transform"));
    }
    let mut visited = BTreeSet::new();
    for &id in ids {
        if !visited.insert(id) {
            continue;
        }
        if !rotate
            && !mapping.brush_only
            && transform.scale == DVec3::ONE
            && transform.translation == DVec3::ZERO
        {
            match id {
                ObjectId::Solid(id) => {
                    tx.solid_mut(id)?;
                }
                ObjectId::Entity(id) => {
                    tx.entity_mut(id)?;
                }
            }
            continue;
        }
        match id {
            ObjectId::Solid(id) => mapping.solid(tx.solid_mut(id)?)?,
            ObjectId::Entity(id) => mapping.entity(tx.entity_mut(id)?, rotate)?,
        }
    }
    Ok(())
}

pub fn transform_each(
    tx: &mut Transaction,
    transforms: &[(ObjectId, Transform)],
) -> Result<(), EditError> {
    if transforms.is_empty() {
        return Err(invalid("Select objects to transform"));
    }
    let mut visited = BTreeSet::new();
    for &(id, transform_spec) in transforms {
        if !visited.insert(id) {
            return Err(invalid("Each object must have exactly one transform"));
        }
        transform(tx, &[id], transform_spec)?;
    }
    Ok(())
}
