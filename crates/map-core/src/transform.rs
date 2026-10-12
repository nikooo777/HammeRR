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
            crate::edit::translate_solid(solid, self.offset, self.translation_texture_lock);
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
            if self.translation_only {
                if self.translation_texture_lock
                    && [&side.u_axis, &side.v_axis]
                        .iter()
                        .any(|axis| !axis.offset.is_finite())
                {
                    return Err(invalid("Translation produces a nonfinite texture offset"));
                }
                continue;
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
        if let Some(value) = entity.properties.get("origin") {
            let origin = parse_vec3(value)
                .filter(|point| point.is_finite())
                .ok_or_else(|| invalid(format!("Entity {} has an invalid origin", entity.id)))?;
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
                .and_then(|value| value.parse::<f64>().ok())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisplacementMesh, Document, edit, texel_uv};
    use source_formats::{
        keyvalues::{Node, WriteOptions},
        vmf::{Properties, Vmf},
    };

    fn block(document: &mut Document) -> ObjectId {
        document
            .edit("setup", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::new(64.0, 32.0, 16.0), "brick/wall")
            })
            .unwrap()
    }

    fn solid(document: &Document, id: ObjectId) -> &Solid {
        let ObjectId::Solid(id) = id else { panic!() };
        document.solid(id).unwrap()
    }

    fn orientation(entity: &Entity) -> DQuat {
        let angles = parse_vec3(entity.properties.get("angles").unwrap()).unwrap();
        DQuat::from_euler(
            EulerRot::ZYX,
            angles.y.to_radians(),
            angles.x.to_radians(),
            angles.z.to_radians(),
        )
    }

    #[test]
    fn every_shear_pair_preserves_geometry_uvs_metadata_and_saved_history() {
        for axis in 0..3 {
            for reference in 0..3 {
                if axis == reference {
                    continue;
                }
                for factor in [-1.25, 0.75] {
                    for basis in [DMat3::IDENTITY, DMat3::from_rotation_z(0.37)] {
                        for lock in [false, true] {
                            let mut document = Document::empty();
                            let id = block(&mut document);
                            document
                                .edit("oblique source", |tx| {
                                    transform(
                                        tx,
                                        &[id],
                                        Transform {
                                            rotation: DVec3::new(17.0, -23.0, 31.0),
                                            ..Default::default()
                                        },
                                    )
                                })
                                .unwrap();
                            let before = document.vmf().clone();
                            let old = before.world.solids[0].clone();
                            let spec = Transform {
                                shear: Some(Shear {
                                    axis,
                                    reference,
                                    factor,
                                }),
                                basis,
                                pivot: DVec3::new(12.125, -7.25, 3.5),
                                texture_scale_lock: Some(lock),
                                ..Default::default()
                            };
                            document
                                .edit("shear", |tx| transform(tx, &[id], spec))
                                .unwrap();
                            let after = document.vmf().clone();
                            let new = solid(&document, id);
                            assert_eq!(crate::subobject::topology(new).unwrap().vertices.len(), 8);
                            for (old, new) in old.sides.iter().zip(&new.sides) {
                                let mut metadata = old.clone();
                                metadata.plane = new.plane;
                                metadata.u_axis = new.u_axis;
                                metadata.v_axis = new.v_axis;
                                assert_eq!(&metadata, new);
                                for (point, actual) in old.plane.0.into_iter().zip(new.plane.0) {
                                    let mut delta = basis.transpose() * (point - spec.pivot);
                                    delta[axis] += factor * delta[reference];
                                    let moved = basis * delta + spec.pivot;
                                    assert!(actual.abs_diff_eq(moved, 1e-8));
                                    if lock {
                                        assert!(
                                            (texel_uv(old, point) - texel_uv(new, moved)).length()
                                                < 1e-8
                                        );
                                    } else {
                                        assert_eq!(
                                            (old.u_axis, old.v_axis),
                                            (new.u_axis, new.v_axis)
                                        );
                                    }
                                }
                            }
                            let reopened = Vmf::parse(&after.write(&Default::default())).unwrap();
                            reopened.validate_ids().unwrap();
                            assert_eq!(
                                crate::subobject::topology(&reopened.world.solids[0])
                                    .unwrap()
                                    .vertices
                                    .len(),
                                8
                            );
                            for (old, new) in new.sides.iter().zip(&reopened.world.solids[0].sides)
                            {
                                assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                            }
                            document.undo().unwrap();
                            assert_eq!(document.vmf(), &before);
                            document.redo().unwrap();
                            assert_eq!(document.vmf(), &after);
                            document
                                .edit("inverse shear", |tx| {
                                    transform(
                                        tx,
                                        &[id],
                                        Transform {
                                            shear: Some(Shear {
                                                axis,
                                                reference,
                                                factor: -factor,
                                            }),
                                            ..spec
                                        },
                                    )
                                })
                                .unwrap();
                            for (old, new) in old.sides.iter().zip(&solid(&document, id).sides) {
                                for (a, b) in old.plane.0.into_iter().zip(new.plane.0) {
                                    assert!(a.abs_diff_eq(b, 1e-8));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shear_guards_are_atomic_and_zero_factor_is_an_identity() {
        let mut document = Document::empty();
        let brush = block(&mut document);
        let point = document
            .edit("point", |tx| {
                edit::create_entity(tx, "info_target", Some(DVec3::ONE), &[], vec![])
            })
            .unwrap();
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        for shear in [
            Shear {
                axis: 0,
                reference: 0,
                factor: 0.5,
            },
            Shear {
                axis: 3,
                reference: 0,
                factor: 0.5,
            },
            Shear {
                axis: 0,
                reference: 1,
                factor: f64::NAN,
            },
            Shear {
                axis: 0,
                reference: 1,
                factor: 65.0,
            },
            Shear {
                axis: 0,
                reference: 1,
                factor: -65.0,
            },
            Shear {
                axis: 0,
                reference: 1,
                factor: 0.5,
            },
        ] {
            assert!(
                document
                    .edit("invalid shear", |tx| transform(
                        tx,
                        &[brush, point],
                        Transform {
                            shear: Some(shear),
                            ..Default::default()
                        }
                    ))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
            assert_eq!(document.undo_labels().count(), history);
        }
        document
            .edit("zero shear", |tx| {
                transform(
                    tx,
                    &[brush, point],
                    Transform {
                        shear: Some(Shear {
                            axis: 0,
                            reference: 1,
                            factor: 0.0,
                        }),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
        assert!(
            document
                .edit("out of bounds", |tx| transform(
                    tx,
                    &[brush],
                    Transform {
                        shear: Some(Shear {
                            axis: 0,
                            reference: 1,
                            factor: 1.0
                        }),
                        translation: DVec3::splat(100_000.0),
                        ..Default::default()
                    }
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn reflection_preserves_closed_geometry_winding_uvs_metadata_and_history() {
        for mask in 1..8 {
            for basis in [DMat3::IDENTITY, DMat3::from_rotation_z(0.37)] {
                for lock in [false, true] {
                    let mut document = Document::empty();
                    let id = block(&mut document);
                    let ObjectId::Solid(sid) = id else {
                        unreachable!()
                    };
                    document
                        .edit("authored mirror metadata", |tx| {
                            for side in &mut tx.solid_mut(sid)?.sides {
                                side.u_axis.axis *= 2.75;
                                side.u_axis.offset = 17.123456789;
                                side.v_axis.scale = -0.523456789;
                                side.extra_properties.set("vendor", "preserve");
                            }
                            Ok(())
                        })
                        .unwrap();
                    document
                        .edit("oblique source", |tx| {
                            transform(
                                tx,
                                &[id],
                                Transform {
                                    rotation: DVec3::new(17.0, -23.0, 31.0),
                                    ..Default::default()
                                },
                            )
                        })
                        .unwrap();
                    let before = document.vmf().clone();
                    let source = before.world.solids[0].clone();
                    let source_geometry = crate::subobject::topology(&source).unwrap();
                    let spec = Transform {
                        reflection: [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0],
                        pivot: DVec3::new(12.125, -7.25, 3.5),
                        basis,
                        texture_scale_lock: Some(lock),
                        ..Default::default()
                    };
                    let signs = DVec3::from_array(
                        spec.reflection.map(|axis| if axis { -1.0 } else { 1.0 }),
                    );
                    let linear = basis * DMat3::from_diagonal(signs) * basis.transpose();
                    document
                        .edit("mirror", |tx| transform(tx, &[id], spec))
                        .unwrap();
                    let after = document.vmf().clone();
                    let changed = solid(&document, id);
                    let geometry = crate::subobject::topology(changed).unwrap();
                    for point in &source_geometry.vertices {
                        let reflected = linear * (point.position - spec.pivot) + spec.pivot;
                        assert!(
                            geometry
                                .vertices
                                .iter()
                                .any(|point| point.position.abs_diff_eq(reflected, 1e-8))
                        );
                    }
                    for (old, new) in source.sides.iter().zip(&changed.sides) {
                        let mut metadata = old.clone();
                        metadata.plane = new.plane;
                        metadata.u_axis = new.u_axis;
                        metadata.v_axis = new.v_axis;
                        assert_eq!(&metadata, new);
                        let plane = Plane::from_points(new.plane.0).unwrap();
                        assert!(
                            geometry
                                .vertices
                                .iter()
                                .all(|point| plane.distance_to(point.position) < 1e-7)
                        );
                        for point in old.plane.0 {
                            let moved = linear * (point - spec.pivot) + spec.pivot;
                            assert!(
                                new.plane
                                    .0
                                    .iter()
                                    .any(|point| point.abs_diff_eq(moved, 1e-8))
                            );
                            if lock {
                                assert!(
                                    (texel_uv(old, point) - texel_uv(new, moved)).length() < 1e-8
                                );
                            } else {
                                assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                            }
                        }
                    }
                    let reopened = Vmf::parse(&after.write(&Default::default())).unwrap();
                    reopened.validate_ids().unwrap();
                    let saved = crate::subobject::topology(&reopened.world.solids[0]).unwrap();
                    assert_eq!(saved.vertices.len(), geometry.vertices.len());
                    for point in geometry.vertices {
                        assert!(
                            saved
                                .vertices
                                .iter()
                                .any(|saved| saved.position.abs_diff_eq(point.position, 1e-8))
                        );
                    }
                    document.undo().unwrap();
                    assert_eq!(document.vmf(), &before);
                    document.redo().unwrap();
                    assert_eq!(document.vmf(), &after);
                    document
                        .edit("mirror twice", |tx| transform(tx, &[id], spec))
                        .unwrap();
                    let restored = crate::subobject::topology(solid(&document, id)).unwrap();
                    for point in source_geometry.vertices {
                        assert!(
                            restored
                                .vertices
                                .iter()
                                .any(|saved| saved.position.abs_diff_eq(point.position, 1e-8))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn reflection_rejects_point_entities_and_bounds_atomically() {
        let mut document = Document::empty();
        let brush = block(&mut document);
        let entity = document
            .edit("point", |tx| {
                edit::create_entity(tx, "info_target", Some(DVec3::ONE), &[], vec![])
            })
            .unwrap();
        for spec in [
            Transform {
                reflection: [true, false, false],
                ..Default::default()
            },
            Transform {
                reflection: [true, true, false],
                ..Default::default()
            },
            Transform {
                reflection: [true, false, false],
                pivot: DVec3::X * 40_000.0,
                ..Default::default()
            },
            Transform {
                reflection: [true, false, false],
                pivot: DVec3::X * 70_000.0,
                ..Default::default()
            },
        ] {
            let before = document.vmf().clone();
            let history = document.undo_labels().count();
            let ids = if spec.pivot == DVec3::ZERO {
                vec![brush, entity]
            } else {
                vec![brush]
            };
            assert!(
                document
                    .edit("invalid reflection", |tx| transform(tx, &ids, spec))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
            assert_eq!(document.undo_labels().count(), history);
        }
    }

    #[test]
    fn reflection_preserves_brush_entity_ownership_and_mirrors_its_origin() {
        let mut document = Document::empty();
        let ObjectId::Solid(brush) = block(&mut document) else {
            unreachable!()
        };
        let entity = document
            .edit("tie", |tx| {
                edit::tie_to_entity(
                    tx,
                    &[brush],
                    "func_detail",
                    &[
                        ("origin".into(), "8 16 4".into()),
                        ("angles".into(), "0 15 0".into()),
                    ],
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        document
            .edit("mirror", |tx| {
                transform(
                    tx,
                    &[entity],
                    Transform {
                        reflection: [true, false, false],
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let ObjectId::Entity(eid) = entity else {
            unreachable!()
        };
        let changed = document.entity(eid).unwrap();
        assert_eq!(changed.solids[0].id, brush);
        assert_eq!(changed.origin(), Some(DVec3::new(-8.0, 16.0, 4.0)));
        assert_eq!(changed.properties.get("angles"), Some("0 15 0"));
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn reflection_rejects_displacements_without_modifying_earlier_brushes() {
        let mut document = Document::empty();
        let first = block(&mut document);
        let terrain = block(&mut document);
        let ObjectId::Solid(id) = terrain else {
            unreachable!()
        };
        document
            .edit("terrain", |tx| {
                tx.solid_mut(id)?.sides[0].displacement = Some(DispInfo {
                    power: 2,
                    start_position: DVec3::ZERO,
                    flags: None,
                    elevation: 0.0,
                    subdivided: false,
                    normals: None,
                    distances: None,
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Properties::default(),
                    extra: Vec::new(),
                });
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        let error = document
            .edit("mirror terrain", |tx| {
                transform(
                    tx,
                    &[first, terrain],
                    Transform {
                        reflection: [true, false, false],
                        ..Default::default()
                    },
                )
            })
            .unwrap_err();
        assert!(error.to_string().contains("displacement"));
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }

    #[test]
    fn compound_reflection_uses_independent_texture_policies() {
        for movement in [false, true] {
            for stretch in [false, true] {
                let mut document = Document::empty();
                let id = block(&mut document);
                let old = solid(&document, id).clone();
                let basis = DMat3::from_rotation_z(0.37);
                let spec = Transform {
                    reflection: [true, false, true],
                    shear: None,
                    translation: DVec3::new(12.125, -24.25, 8.5),
                    rotation: DVec3::new(17.0, -23.0, 31.0),
                    scale: DVec3::new(2.0, 0.5, 1.25),
                    pivot: DVec3::new(-19.125, 42.25, 5.5),
                    basis,
                    texture_lock: movement,
                    texture_scale_lock: Some(stretch),
                };
                let rotation = basis
                    * (DMat3::from_rotation_z(spec.rotation.z.to_radians())
                        * DMat3::from_rotation_y(spec.rotation.y.to_radians())
                        * DMat3::from_rotation_x(spec.rotation.x.to_radians()))
                    * basis.transpose();
                let scaling = basis
                    * DMat3::from_diagonal(spec.scale * DVec3::new(-1.0, 1.0, -1.0))
                    * basis.transpose();
                let projection = if movement { rotation } else { DMat3::IDENTITY }
                    * if stretch { scaling } else { DMat3::IDENTITY };
                document
                    .edit("compound mirror", |tx| transform(tx, &[id], spec))
                    .unwrap();
                for (old, new) in old.sides.iter().zip(&solid(&document, id).sides) {
                    for point in old.plane.0 {
                        let moved = rotation * scaling * (point - spec.pivot)
                            + spec.pivot
                            + spec.translation;
                        assert!(
                            new.plane
                                .0
                                .iter()
                                .any(|point| point.abs_diff_eq(moved, 1e-8))
                        );
                        let sample = projection.inverse()
                            * (moved
                                - spec.pivot
                                - if movement {
                                    spec.translation
                                } else {
                                    DVec3::ZERO
                                })
                            + spec.pivot;
                        assert!((texel_uv(new, moved) - texel_uv(old, sample)).length() < 1e-8);
                    }
                }
            }
        }
    }

    #[test]
    fn brush_rotation_scale_and_texture_lock_preserve_corresponding_uvs() {
        for lock in [false, true] {
            let mut document = Document::empty();
            let id = block(&mut document);
            let before = solid(&document, id).clone();
            let spec = Transform {
                translation: DVec3::new(12.0, -24.0, 8.0),
                rotation: DVec3::new(30.0, -20.0, 65.0),
                scale: DVec3::new(2.0, 0.5, 1.25),
                pivot: DVec3::new(-19.0, 42.0, 5.0),
                basis: DMat3::IDENTITY,
                texture_lock: lock,
                texture_scale_lock: None,
                reflection: [false; 3],
                shear: None,
            };
            document
                .edit("transform", |tx| transform(tx, &[id], spec))
                .unwrap();
            let after = solid(&document, id);
            let rotation = DQuat::from_rotation_z(spec.rotation.z.to_radians())
                * DQuat::from_rotation_y(spec.rotation.y.to_radians())
                * DQuat::from_rotation_x(spec.rotation.x.to_radians());
            for (old, new) in before.sides.iter().zip(&after.sides) {
                for point in old.plane.0 {
                    let moved = rotation * ((point - spec.pivot) * spec.scale)
                        + spec.pivot
                        + spec.translation;
                    if lock {
                        assert!((texel_uv(old, point) - texel_uv(new, moved)).length() < 1e-8);
                    } else {
                        assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                    }
                }
            }
            let brush = BrushGeometry::from_solid(after).unwrap();
            assert_eq!(brush.faces.len(), 6);
            assert!(
                brush
                    .faces
                    .iter()
                    .all(|face| face.plane.distance_to(brush.center()) < 0.0)
            );
        }
    }

    #[test]
    fn independent_texture_modes_preserve_density_pivots_metadata_and_round_trip() {
        for basis in [DMat3::IDENTITY, DMat3::from_rotation_z(0.37)] {
            for movement in [false, true] {
                for stretch in [false, true] {
                    let mut document = Document::empty();
                    let id = block(&mut document);
                    let ObjectId::Solid(sid) = id else {
                        unreachable!()
                    };
                    document
                        .edit("authored projections", |tx| {
                            for side in &mut tx.solid_mut(sid)?.sides {
                                side.u_axis.axis *= 2.75;
                                side.u_axis.offset = 17.123456789;
                                side.v_axis.scale = -0.523456789;
                                side.extra_properties.set("vendor", "preserve");
                            }
                            Ok(())
                        })
                        .unwrap();
                    let before = document.vmf().clone();
                    let spec = Transform {
                        translation: DVec3::new(12.125, -24.25, 8.5),
                        rotation: DVec3::new(30.0, -20.0, 65.0),
                        scale: DVec3::new(2.0, 0.5, 1.25),
                        pivot: DVec3::new(-19.125, 42.25, 5.5),
                        basis,
                        texture_lock: movement,
                        texture_scale_lock: Some(stretch),
                        reflection: [false; 3],
                        shear: None,
                    };
                    let local_rotation = DMat3::from_rotation_z(spec.rotation.z.to_radians())
                        * DMat3::from_rotation_y(spec.rotation.y.to_radians())
                        * DMat3::from_rotation_x(spec.rotation.x.to_radians());
                    let rotation = basis * local_rotation * basis.transpose();
                    let scaling = basis * DMat3::from_diagonal(spec.scale) * basis.transpose();
                    let projection = if movement { rotation } else { DMat3::IDENTITY }
                        * if stretch { scaling } else { DMat3::IDENTITY };
                    let projection_translation = if movement {
                        spec.translation
                    } else {
                        DVec3::ZERO
                    };
                    document
                        .edit("independent modes", |tx| transform(tx, &[id], spec))
                        .unwrap();
                    let after = document.vmf().clone();
                    for (old, new) in before.world.solids[0]
                        .sides
                        .iter()
                        .zip(&after.world.solids[0].sides)
                    {
                        assert_eq!(old.extra_properties, new.extra_properties);
                        assert_eq!(old.lightmap_scale, new.lightmap_scale);
                        assert_eq!(old.id, new.id);
                        for point in old.plane.0 {
                            let moved = rotation * scaling * (point - spec.pivot)
                                + spec.pivot
                                + spec.translation;
                            let sampled = projection.inverse()
                                * (moved - spec.pivot - projection_translation)
                                + spec.pivot;
                            assert!(
                                (texel_uv(new, moved) - texel_uv(old, sampled)).length() < 1e-8
                            );
                            if movement && stretch {
                                assert!(
                                    (texel_uv(new, moved) - texel_uv(old, point)).length() < 1e-8
                                );
                            }
                        }
                        if !movement && !stretch {
                            assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                        }
                    }
                    let reopened = Vmf::parse(&after.write(&WriteOptions::default())).unwrap();
                    for (expected, saved) in after.world.solids[0]
                        .sides
                        .iter()
                        .zip(&reopened.world.solids[0].sides)
                    {
                        assert_eq!(
                            (expected.u_axis, expected.v_axis),
                            (saved.u_axis, saved.v_axis)
                        );
                    }
                    document.undo().unwrap();
                    assert_eq!(document.vmf(), &before);
                    document.redo().unwrap();
                    assert_eq!(document.vmf(), &after);
                }
            }
        }
    }

    #[test]
    fn scale_lock_is_independent_of_translation_and_omitted_mode_keeps_legacy_behavior() {
        let mut baseline = Document::empty();
        let id = block(&mut baseline);
        for movement in [false, true] {
            for stretch in [false, true] {
                let mut document = baseline.clone();
                let translation = DVec3::new(12.125, 24.25, 36.5);
                let spec = Transform {
                    translation,
                    texture_lock: movement,
                    texture_scale_lock: Some(stretch),
                    ..Default::default()
                };
                document
                    .edit("translate", |tx| transform(tx, &[id], spec))
                    .unwrap();
                for (old, new) in solid(&baseline, id)
                    .sides
                    .iter()
                    .zip(&solid(&document, id).sides)
                {
                    if movement {
                        assert!(
                            (texel_uv(old, old.plane.0[0]) - texel_uv(new, new.plane.0[0]))
                                .length()
                                < 1e-10
                        );
                    } else {
                        assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                    }
                }
            }
            let spec = Transform {
                translation: DVec3::new(12.125, 24.25, 36.5),
                rotation: DVec3::new(13.0, 27.0, 39.0),
                scale: DVec3::new(2.0, 0.5, 1.25),
                texture_lock: movement,
                ..Default::default()
            };
            let mut legacy = baseline.clone();
            legacy
                .edit("legacy", |tx| transform(tx, &[id], spec))
                .unwrap();
            let mut explicit = baseline.clone();
            explicit
                .edit("explicit", |tx| {
                    transform(
                        tx,
                        &[id],
                        Transform {
                            texture_scale_lock: Some(movement),
                            ..spec
                        },
                    )
                })
                .unwrap();
            assert_eq!(legacy.vmf(), explicit.vmf());
        }
    }

    #[test]
    fn quarter_turn_has_exact_bounds_and_repeated_ids_apply_once() {
        let mut document = Document::empty();
        let id = block(&mut document);
        let before = document.vmf().clone();
        let spec = Transform {
            pivot: DVec3::new(32.0, 16.0, 8.0),
            rotation: DVec3::Z * 90.0,
            scale: DVec3::new(2.0, 1.0, 0.5),
            translation: DVec3::new(8.0, -8.0, 4.0),
            ..Transform::default()
        };
        document
            .edit("transform", |tx| transform(tx, &[id, id], spec))
            .unwrap();
        let brush = BrushGeometry::from_solid(solid(&document, id)).unwrap();
        assert!(brush.mins.distance(DVec3::new(24.0, -56.0, 8.0)) < 1e-8);
        assert!(brush.maxs.distance(DVec3::new(56.0, 72.0, 16.0)) < 1e-8);
        let after = document.vmf().clone();
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &after);
        let reopened = Vmf::parse(&after.write(&WriteOptions::default())).unwrap();
        let geometry = BrushGeometry::from_solid(&reopened.world.solids[0]).unwrap();
        assert!(geometry.mins.distance(brush.mins) < 1e-8);
        assert!(geometry.maxs.distance(brush.maxs) < 1e-8);
    }

    #[test]
    fn invalid_values_or_a_later_bad_object_roll_back_every_change() {
        let mut document = Document::empty();
        let id = block(&mut document);
        let bad = document
            .edit("setup", |tx| {
                edit::create_entity(tx, "info_target", None, &[], vec![])
            })
            .unwrap();
        let before = document.vmf().clone();
        let revision = document.revision();
        let history = document.undo_labels().count();
        for spec in [
            Transform {
                scale: DVec3::new(0.0, 1.0, 1.0),
                ..Transform::default()
            },
            Transform {
                scale: DVec3::splat(-1.0),
                ..Transform::default()
            },
            Transform {
                translation: DVec3::splat(f64::NAN),
                ..Transform::default()
            },
            Transform {
                pivot: DVec3::splat(f64::INFINITY),
                ..Transform::default()
            },
            Transform {
                scale: DVec3::splat(1e300),
                ..Transform::default()
            },
            Transform {
                rotation: DVec3::Z * 30.0,
                ..Transform::default()
            },
        ] {
            assert!(
                document
                    .edit("invalid", |tx| transform(tx, &[id, bad], spec))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
            assert_eq!(document.revision(), revision);
            assert_eq!(document.undo_labels().count(), history);
        }
    }

    #[test]
    fn point_orientation_and_light_pitch_follow_world_rotation() {
        let mut document = Document::empty();
        let point = document
            .edit("setup", |tx| {
                edit::create_entity(
                    tx,
                    "light_spot",
                    Some(DVec3::X * 64.0),
                    &[
                        ("angles".into(), "10 20 30".into()),
                        ("pitch".into(), "-45".into()),
                        ("_light".into(), "255 180 90 400".into()),
                    ],
                    vec![],
                )
            })
            .unwrap();
        let ObjectId::Entity(id) = point else {
            panic!()
        };
        let old = DQuat::from_euler(
            EulerRot::ZYX,
            20f64.to_radians(),
            45f64.to_radians(),
            30f64.to_radians(),
        );
        document
            .edit("rotate", |tx| {
                transform(
                    tx,
                    &[point],
                    Transform {
                        rotation: DVec3::Z * 90.0,
                        ..Transform::default()
                    },
                )
            })
            .unwrap();
        let entity = document.entity(id).unwrap();
        assert!(entity.origin().unwrap().distance(DVec3::Y * 64.0) < 1e-8);
        assert!(
            orientation(entity)
                .dot(DQuat::from_rotation_z(90f64.to_radians()) * old)
                .abs()
                > 1.0 - 1e-10
        );
        let angles = parse_vec3(entity.properties.get("angles").unwrap()).unwrap();
        assert!(
            (entity
                .properties
                .get("pitch")
                .unwrap()
                .parse::<f64>()
                .unwrap()
                + angles.x)
                .abs()
                < 1e-10
        );
        assert_eq!(entity.properties.get("_light"), Some("255 180 90 400"));
    }

    #[test]
    fn brush_entities_transform_origins_and_geometry_without_double_rotation() {
        let mut document = Document::empty();
        let brush = block(&mut document);
        let ObjectId::Solid(brush_id) = brush else {
            panic!()
        };
        let entity = document
            .edit("tie", |tx| {
                edit::tie_to_entity(
                    tx,
                    &[brush_id],
                    "func_detail",
                    &[
                        ("angles".into(), "0 15 0".into()),
                        ("origin".into(), "64 0 0".into()),
                    ],
                )
            })
            .unwrap();
        let ObjectId::Entity(id) = entity else {
            panic!()
        };
        document
            .edit("rotate", |tx| {
                transform(
                    tx,
                    &[entity],
                    Transform {
                        rotation: DVec3::Z * 90.0,
                        ..Transform::default()
                    },
                )
            })
            .unwrap();
        let entity = document.entity(id).unwrap();
        assert!(entity.origin().unwrap().distance(DVec3::Y * 64.0) < 1e-8);
        assert_eq!(entity.properties.get("angles"), Some("0 15 0"));
        let bounds = BrushGeometry::from_solid(&entity.solids[0]).unwrap();
        assert!(bounds.mins.distance(DVec3::new(-32.0, 0.0, 0.0)) < 1e-8);
        assert!(bounds.maxs.distance(DVec3::new(0.0, 64.0, 16.0)) < 1e-8);
    }

    #[test]
    fn displacement_positions_and_uvs_survive_nonuniform_scale_and_rotation() {
        let mut document = Document::empty();
        let id = block(&mut document);
        let ObjectId::Solid(sid) = id else { panic!() };
        document
            .edit("terrain", |tx| {
                let solid = tx.solid_mut(sid)?;
                solid.sides[0].displacement = Some(DispInfo {
                    power: 2,
                    start_position: DVec3::new(0.0, 0.0, 16.0),
                    flags: Some(0),
                    elevation: 3.0,
                    subdivided: false,
                    normals: Some(vec![vec![DVec3::new(1.0, 2.0, 3.0).normalize(); 5]; 5]),
                    distances: Some(
                        (0..5)
                            .map(|row| (0..5).map(|col| f64::from(row + col)).collect())
                            .collect(),
                    ),
                    offsets: Some(vec![vec![DVec3::new(0.25, 0.5, 1.0); 5]; 5]),
                    offset_normals: None,
                    alphas: Some(vec![vec![127.0; 5]; 5]),
                    triangle_tags: Some(vec![vec![9; 8]; 4]),
                    allowed_verts: Some(vec![-1; 10]),
                    extra_properties: Properties::default(),
                    extra: vec![Node::string("custom", "kept")],
                });
                Ok(())
            })
            .unwrap();
        let before = solid(&document, id).clone();
        let old_brush = BrushGeometry::from_solid(&before).unwrap();
        let old_face = old_brush.faces.iter().find(|face| face.side == 0).unwrap();
        let old_disp = before.sides[0].displacement.as_ref().unwrap();
        let old_mesh = DisplacementMesh::build(old_face, old_disp).unwrap();
        let spec = Transform {
            translation: DVec3::new(32.0, -16.0, 8.0),
            rotation: DVec3::new(30.0, -20.0, 65.0),
            scale: DVec3::new(2.0, 0.5, 1.25),
            pivot: DVec3::new(5.0, 10.0, 20.0),
            ..Transform::default()
        };
        document
            .edit("transform", |tx| transform(tx, &[id], spec))
            .unwrap();
        let after = solid(&document, id);
        let new_brush = BrushGeometry::from_solid(after).unwrap();
        let new_face = new_brush.faces.iter().find(|face| face.side == 0).unwrap();
        let new_disp = after.sides[0].displacement.as_ref().unwrap();
        let new_mesh = DisplacementMesh::build(new_face, new_disp).unwrap();
        let mapping = Mapping::new(spec).unwrap();
        for (old, new) in old_mesh.positions.iter().zip(&new_mesh.positions) {
            assert!(mapping.point(*old).unwrap().distance(*new) < 1e-7);
        }
        for (old, new) in old_mesh.base_positions.iter().zip(&new_mesh.base_positions) {
            assert!(
                (texel_uv(&before.sides[0], *old) - texel_uv(&after.sides[0], *new)).length()
                    < 1e-7
            );
        }
        assert_eq!(new_disp.alphas, old_disp.alphas);
        assert_eq!(new_disp.triangle_tags, old_disp.triangle_tags);
        assert_eq!(new_disp.allowed_verts, old_disp.allowed_verts);
        assert_eq!(new_disp.extra, old_disp.extra);
        let source = {
            let mut vmf = document.vmf().clone();
            vmf.world.solids[0] = before.clone();
            vmf
        };
        for movement in [false, true] {
            for stretch in [false, true] {
                let mut candidate = Document::try_new(source.clone()).unwrap();
                candidate
                    .edit("terrain texture policy", |tx| {
                        transform(
                            tx,
                            &[id],
                            Transform {
                                texture_lock: movement,
                                texture_scale_lock: Some(stretch),
                                ..spec
                            },
                        )
                    })
                    .unwrap();
                let changed = solid(&candidate, id);
                assert_eq!(changed.sides[0].displacement, after.sides[0].displacement);
                assert_eq!(
                    changed
                        .sides
                        .iter()
                        .map(|side| side.plane)
                        .collect::<Vec<_>>(),
                    after
                        .sides
                        .iter()
                        .map(|side| side.plane)
                        .collect::<Vec<_>>()
                );
                let vmf = Vmf::parse(&candidate.vmf().write(&WriteOptions::default())).unwrap();
                let saved = vmf.world.solids[0].sides[0].displacement.as_ref().unwrap();
                assert_eq!(saved.extra_properties.get("custom"), Some("kept"));
                assert_eq!(saved.allowed_verts, new_disp.allowed_verts);
                let geometry = BrushGeometry::from_solid(&vmf.world.solids[0]).unwrap();
                let face = geometry.faces.iter().find(|face| face.side == 0).unwrap();
                let mesh = DisplacementMesh::build(face, saved).unwrap();
                assert!(
                    mesh.positions
                        .iter()
                        .zip(&new_mesh.positions)
                        .all(|(saved, expected)| saved.abs_diff_eq(*expected, 1e-8))
                );
            }
        }
    }

    #[test]
    fn local_frame_scales_and_rotates_geometry_with_locked_uvs() {
        let mut document = Document::empty();
        let id = block(&mut document);
        document
            .edit("orient", |tx| {
                transform(
                    tx,
                    &[id],
                    Transform {
                        rotation: DVec3::new(20.0, 35.0, 40.0),
                        ..Transform::default()
                    },
                )
            })
            .unwrap();
        let before = solid(&document, id).clone();
        let basis = crate::query::local_basis(&document, id).unwrap();
        let pivot = DVec3::new(5.0, -8.0, 2.0);
        let spec = Transform {
            basis,
            pivot,
            scale: DVec3::new(2.0, 0.5, 1.25),
            rotation: DVec3::new(15.0, -30.0, 10.0),
            translation: basis.x_axis * 3.0,
            ..Transform::default()
        };
        let mapping = Mapping::new(spec).unwrap();
        document
            .edit("local", |tx| transform(tx, &[id], spec))
            .unwrap();
        let after = solid(&document, id);
        for (old, new) in before.sides.iter().zip(&after.sides) {
            for point in old.plane.0 {
                let changed = mapping.point(point).unwrap();
                assert!(
                    crate::texel_uv(old, point).abs_diff_eq(crate::texel_uv(new, changed), 1e-8)
                );
            }
        }
        let vmf = document.vmf().clone();
        assert_eq!(
            source_formats::vmf::Vmf::parse(&vmf.write(&Default::default())).unwrap(),
            vmf
        );
        document.undo();
        assert_eq!(solid(&document, id), &before);
        assert!(
            Mapping::new(Transform {
                basis: DMat3::from_diagonal(DVec3::splat(2.0)),
                ..spec
            })
            .is_err()
        );
    }

    #[test]
    fn identity_does_not_reformat_entity_properties_or_record_undo() {
        let mut document = Document::empty();
        let point = document
            .edit("setup", |tx| {
                edit::create_entity(
                    tx,
                    "info_target",
                    None,
                    &[("origin".into(), "0.00 2.00 0.00".into())],
                    vec![],
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        let revision = document.revision();
        document
            .edit("identity", |tx| {
                transform(tx, &[point], Transform::default())
            })
            .unwrap();
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
    }
}
