use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub translation: DVec3,
    pub rotation: DVec3,
    pub scale: DVec3,
    pub pivot: DVec3,
    pub basis: DMat3,
    pub texture_lock: bool,
}

impl Placement {
    pub fn transform(self) -> edit::Transform {
        edit::Transform {
            translation: self.translation,
            rotation: self.rotation,
            scale: self.scale,
            pivot: self.pivot,
            basis: self.basis,
            texture_lock: self.texture_lock,
            ..Default::default()
        }
    }

    pub(super) fn validate(self) -> std::result::Result<(), EditError> {
        if !self.translation.is_finite()
            || !self.rotation.is_finite()
            || !self.pivot.is_finite()
            || self.pivot.abs().max_element() > crate::primitive::MAX_COORDINATE
            || !self.scale.is_finite()
            || !self.scale.cmpgt(DVec3::ZERO).all()
        {
            return Err(invalid(
                "Placement transforms must be finite, have positive scale and a pivot within ±65536",
            ));
        }
        crate::transform::transform_point(DVec3::ZERO, self.transform())?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub placements: Vec<Placement>,
    pub policies: Policies,
}

impl Plan {
    pub fn array(options: Options, policies: Policies) -> std::result::Result<Self, EditError> {
        if options.count == 0 || options.count > MAX_COPIES {
            return Err(invalid("Use 1–64 additional copies"));
        }
        let limit = crate::primitive::MAX_COORDINATE;
        if [options.translation, options.rotation, options.pivot]
            .iter()
            .any(|value| !value.is_finite() || value.abs().max_element() > limit)
        {
            return Err(invalid(
                "Array steps and pivot must be finite and within ±65536",
            ));
        }
        if !options.scale.is_finite() || !options.scale.cmpgt(DVec3::ZERO).all() {
            return Err(invalid("Array scale factors must be finite and positive"));
        }
        if options.translation == DVec3::ZERO
            && options.rotation.map(|angle| angle.rem_euclid(360.0)) == DVec3::ZERO
            && options.scale == DVec3::ONE
        {
            return Err(invalid(
                "Choose a nonzero translation or rotation step, or a scale factor other than 1",
            ));
        }
        policies.validate(options.count)?;
        let placements = (1..=options.count)
            .map(|index| {
                let factor = f64::from(index);
                let placement = Placement {
                    translation: options.basis * (options.translation * factor),
                    rotation: options.rotation * factor,
                    scale: options.copy_scale(index),
                    pivot: options.pivot,
                    basis: options.basis,
                    texture_lock: options.texture_lock,
                };
                placement.validate()?;
                Ok(placement)
            })
            .collect::<std::result::Result<Vec<_>, EditError>>()?;
        Ok(Self {
            placements,
            policies,
        })
    }
}
