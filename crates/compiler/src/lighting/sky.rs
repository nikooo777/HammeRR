use glam::DVec3;
use source_formats::bsp::{WorldLight, world_light_kind};

use super::{PointLight, number, property};
use crate::map::MapEntity;

pub(super) const TRACE_LENGTH: f64 = crate::map::MAX_COMPILE_COORD * 4.0;
pub(super) const SKY_HIT_BIAS: f64 = 0.0001;
const GOLDEN_ANGLE: f64 = 2.399963229728653;
const MAX_SUN_SPREAD_ANGLE: f64 = 90.0;

fn sphere_direction(index: usize, count: usize) -> DVec3 {
    let z = 1.0 - 2.0 * (index as f64 + 0.5) / count as f64;
    let radius = (1.0 - z * z).sqrt();
    let angle = index as f64 * GOLDEN_ANGLE;
    DVec3::new(radius * angle.cos(), radius * angle.sin(), z)
}

pub(super) fn ambient_directions(quality: crate::SkyQuality) -> Vec<DVec3> {
    let count = quality.directions();
    (0..count)
        .map(|index| sphere_direction(index, count))
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SkyLight {
    pub direction: DVec3,
    intensity: DVec3,
    ambient: DVec3,
    origin: DVec3,
    spread: f64,
}

impl SkyLight {
    pub(super) fn from_entity(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        if number(entity, "style")? != 0.0
            || property(entity, "targetname").is_some_and(|name| !name.is_empty())
            || property(entity, "pattern").is_some_and(|pattern| !pattern.is_empty())
        {
            return Err("Sun and sky lighting require unnamed static Appearance Normal".into());
        }
        let spread = number(entity, "SunSpreadAngle")?;
        if spread > MAX_SUN_SPREAD_ANGLE {
            return Err("SunSpreadAngle must be between 0 and 90 degrees".into());
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
        let ambient = if let Some(ambient) =
            property(entity, "_ambient").filter(|value| !value.trim().is_empty())
        {
            let mut source = source.clone();
            source
                .properties
                .retain(|(key, _)| !key.eq_ignore_ascii_case("_light"));
            source.properties.push(("_light".into(), ambient.into()));
            PointLight::from_entity(&source, -1)?.intensity
        } else {
            point.intensity * 0.5
        };
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
            ambient,
            origin,
            spread: spread.to_radians().sin(),
        })
    }

    pub(super) fn sun_directions(self, quality: crate::SkyQuality) -> Vec<DVec3> {
        let center = -self.direction;
        if self.spread == 0.0 {
            return vec![center];
        }
        let axis = if center.z.abs() < 0.9 {
            DVec3::Z
        } else {
            DVec3::X
        };
        let right = axis.cross(center).normalize();
        let up = center.cross(right);
        let count = quality.directions();
        std::iter::once(center)
            .chain((0..count - 1).map(|index| {
                let offset = sphere_direction(index, count - 1);
                let offset = right * offset.x + up * offset.y + center * offset.z;
                (center + offset * self.spread).normalize()
            }))
            .collect()
    }

    pub(super) fn sun_contribution(
        self,
        normal: DVec3,
        directions: &[DVec3],
        visible: &mut impl FnMut(DVec3) -> Result<bool, String>,
    ) -> Result<DVec3, String> {
        let color = self.contribution(normal)?;
        if color == DVec3::ZERO {
            return Ok(color);
        }
        if directions.is_empty() {
            return Err("Sun visibility needs at least one direction".into());
        }
        let mut received = 0;
        for &direction in directions {
            received += usize::from(visible(direction)?);
        }
        Ok(color * (received as f64 / directions.len() as f64))
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
    pub(super) fn ambient_world_light(self) -> Option<WorldLight> {
        (self.ambient != DVec3::ZERO).then(|| WorldLight {
            normal: [0.0; 3],
            intensity: (self.ambient / 255.0).as_vec3().to_array(),
            kind: world_light_kind::SKY_AMBIENT,
            ..self.world_light()
        })
    }

    pub(super) fn has_ambient(self) -> bool {
        self.ambient != DVec3::ZERO
    }

    pub(super) fn ambient_radiance(self) -> DVec3 {
        self.ambient / 255.0
    }

    pub(super) fn ambient_contribution(
        self,
        normal: DVec3,
        directions: &[DVec3],
        visible: &mut impl FnMut(DVec3) -> Result<bool, String>,
    ) -> Result<DVec3, String> {
        let normal = normal
            .try_normalize()
            .ok_or("Sky ambient needs a finite nonzero normal")?;
        let mut total = 0.0;
        let mut received = 0.0;
        for &direction in directions {
            let weight = normal.dot(direction).max(0.0);
            if weight == 0.0 {
                continue;
            }
            total += weight;
            if visible(direction)? {
                received += weight;
            }
        }
        if total <= 0.0 {
            return Err("Sky ambient hemisphere has no valid directions".into());
        }
        Ok(self.ambient * (received / total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SkyQuality;

    #[test]
    fn sun_sampling_stays_in_its_angular_bound_and_preserves_central_brightness() {
        for direction in [
            DVec3::NEG_Z,
            DVec3::X,
            DVec3::new(1.0, 2.0, -3.0).normalize(),
        ] {
            for angle in [0.0_f64, 0.001, 5.0, 45.0, 90.0] {
                let light = SkyLight {
                    direction,
                    intensity: DVec3::new(200.0, 100.0, 50.0),
                    ambient: DVec3::ZERO,
                    origin: DVec3::ZERO,
                    spread: angle.to_radians().sin(),
                };
                for quality in [SkyQuality::Fast, SkyQuality::Balanced, SkyQuality::Detailed] {
                    let directions = light.sun_directions(quality);
                    assert_eq!(directions[0], -direction);
                    assert_eq!(
                        directions.len(),
                        if angle == 0.0 {
                            1
                        } else {
                            quality.directions()
                        }
                    );
                    assert_eq!(directions, light.sun_directions(quality));
                    for ray in &directions {
                        assert!(ray.is_finite());
                        assert!((ray.length() - 1.0).abs() < 1e-12);
                        assert!(ray.dot(-direction) + 1e-12 >= angle.to_radians().cos());
                    }
                    let color = light
                        .sun_contribution(-direction, &directions, &mut |_| Ok(true))
                        .unwrap();
                    assert_eq!(color, light.contribution(-direction).unwrap());
                    assert_eq!(
                        light
                            .sun_contribution(-direction, &directions, &mut |_| Ok(false))
                            .unwrap(),
                        DVec3::ZERO
                    );
                    assert!(
                        light
                            .sun_contribution(-direction, &directions, &mut |_| Err(
                                "cancelled".into()
                            ))
                            .is_err()
                    );
                }
            }
        }
    }
}
