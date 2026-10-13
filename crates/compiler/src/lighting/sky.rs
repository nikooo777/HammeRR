use glam::DVec3;
use source_formats::bsp::{WorldLight, world_light_kind};

use super::{PointLight, number, property};
use crate::map::MapEntity;

pub(super) const TRACE_LENGTH: f64 = crate::map::MAX_COMPILE_COORD * 4.0;
pub(super) const SKY_HIT_BIAS: f64 = 0.0001;

#[derive(Debug, Clone, Copy)]
pub(super) struct SkyLight {
    pub direction: DVec3,
    intensity: DVec3,
    origin: DVec3,
}

impl SkyLight {
    pub(super) fn from_entity(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        if number(entity, "SunSpreadAngle")? != 0.0 {
            return Err(
                "Direct LDR soft sun spread is unfinished; set SunSpreadAngle to zero".into(),
            );
        }
        for key in [
            "_fifty_percent_distance",
            "_zero_percent_distance",
            "_hardfalloff",
            "_distance",
        ] {
            if number(entity, key)? != 0.0 {
                return Err(
                    "Directional sky lights cannot use point-light distance falloff".into(),
                );
            }
        }
        let origin = entity.origin.unwrap_or(DVec3::ZERO);
        let mut source = entity.clone();
        source.origin = Some(origin);
        source.properties.retain(|(key, _)| {
            !["_constant_attn", "_linear_attn", "_quadratic_attn"]
                .into_iter()
                .any(|field| key.eq_ignore_ascii_case(field))
        });
        let point = PointLight::from_entity(&source, -1)?;
        if let Some(ambient) = property(entity, "_ambient").filter(|value| !value.trim().is_empty())
        {
            let mut source = source.clone();
            source
                .properties
                .retain(|(key, _)| !key.eq_ignore_ascii_case("_light"));
            source.properties.push(("_light".into(), ambient.into()));
            if PointLight::from_entity(&source, -1)?.intensity != DVec3::ZERO {
                return Err("Direct LDR sky ambient is unfinished; set _ambient to zero for direct sun lighting".into());
            }
        }
        let target = map_core::light::target(property(entity, "target"), targets)?;
        let direction = map_core::light::direction(
            property(entity, "angles"),
            property(entity, "angle"),
            property(entity, "pitch"),
            origin,
            target,
        )?;
        Ok(Self {
            direction,
            intensity: point.intensity,
            origin,
        })
    }

    pub(super) fn contribution(self, normal: DVec3) -> Result<DVec3, String> {
        let normal = normal
            .try_normalize()
            .ok_or("Sky sample normal must be finite and nonzero")?;
        let color = self.intensity * normal.dot(-self.direction).clamp(0.0, 1.0);
        if !color.is_finite() {
            return Err("Sky light contribution is nonfinite".into());
        }
        Ok(color)
    }

    pub(super) fn world_light(self) -> WorldLight {
        WorldLight {
            origin: self.origin.as_vec3().to_array(),
            normal: self.direction.as_vec3().to_array(),
            intensity: (self.intensity / 255.0).as_vec3().to_array(),
            kind: world_light_kind::SKY,
            cluster: -1,
            texinfo: -1,
            owner: -1,
            ..Default::default()
        }
    }
}
