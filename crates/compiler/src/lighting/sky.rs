use glam::DVec3;
use source_formats::bsp::{LightingKind, WorldLight, world_light_kind};

use super::{PointLight, checked_intensity, hdr_color, hdr_scale, number, property};
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

fn sun_source(
    entity: &MapEntity,
    kind: LightingKind,
) -> Result<(MapEntity, PointLight, f64), String> {
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
            return Err("Directional sky lights cannot use point-light distance falloff".into());
        }
    }
    let authored_origin = property(entity, "origin")
        .map(|raw| {
            source_formats::vmf::parse_vec3(raw)
                .filter(|origin| {
                    origin.is_finite()
                        && origin.abs().max_element() <= crate::map::MAX_COMPILE_COORD
                })
                .ok_or(
                    "Sun origin must contain three finite coordinates inside Source compile bounds",
                )
        })
        .transpose()?;
    let origin = entity.origin.or(authored_origin).unwrap_or(DVec3::ZERO);
    let mut source = entity.clone();
    source.origin = Some(origin);
    source.properties.retain(|(key, _)| {
        !["_constant_attn", "_linear_attn", "_quadratic_attn"]
            .into_iter()
            .any(|field| key.eq_ignore_ascii_case(field))
    });
    let point = PointLight::from_entity_kind(&source, -1, kind)?;
    Ok((source, point, spread))
}

fn sun_direction(
    entity: &MapEntity,
    targets: &map_core::light::Targets<'_>,
    origin: DVec3,
) -> Result<DVec3, String> {
    let target = map_core::light::target(property(entity, "target"), targets)?;
    map_core::light::direction(
        property(entity, "angles"),
        property(entity, "angle"),
        property(entity, "pitch"),
        origin,
        target,
    )
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SkyLight {
    pub direction: DVec3,
    pub(super) intensity: DVec3,
    pub(super) ambient: DVec3,
    origin: DVec3,
    spread: f64,
}

impl SkyLight {
    pub(super) fn from_entity(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        Self::from_entity_kind(entity, targets, LightingKind::Ldr)
    }

    pub(super) fn from_entity_kind(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
        kind: LightingKind,
    ) -> Result<Self, String> {
        let (source, point, spread) = sun_source(entity, kind)?;
        let origin = point.origin;
        let ambient = match kind {
            LightingKind::Ldr => {
                if let Some(ambient) =
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
                }
            }
            LightingKind::Hdr => {
                let color = match hdr_color(entity, "_ambientHDR", true)? {
                    Some(color) => Some(color),
                    None => hdr_color(entity, "_ambient", true)?,
                };
                let ambient = color.map_or(point.intensity * 0.5, |color| color.intensity(1.0));
                checked_intensity(ambient * hdr_scale(entity, "_AmbientScaleHDR")?)
                    .map_err(|error| format!("HDR sky ambient: {error}"))?
            }
        };
        let direction = sun_direction(entity, targets, origin)?;
        Ok(Self {
            direction,
            intensity: point.intensity,
            ambient,
            origin,
            spread: spread.to_radians().sin(),
        })
    }

    pub(super) fn direct_from_entity(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        let (_, point, spread) = sun_source(entity, LightingKind::Ldr)?;
        let origin = point.origin;
        Ok(Self {
            direction: sun_direction(entity, targets, origin)?,
            intensity: point.intensity,
            ambient: DVec3::ZERO,
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

    fn environment(properties: &[(&str, &str)]) -> MapEntity {
        MapEntity {
            id: 10,
            classname: "light_environment".into(),
            origin: None,
            brushes: Vec::new(),
            properties: properties
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn ldr_sky_keeps_legacy_sun_and_ambient_with_ignored_hdr_inputs() {
        let targets = map_core::light::Targets::new();
        let reference = SkyLight::from_entity(
            &environment(&[("_light", "255 0 0 200"), ("_ambient", "0 255 0 20")]),
            &targets,
        )
        .unwrap();
        let source = environment(&[
            ("_light", "255 0 0 200 0 0 255 510"),
            ("_ambient", "0 255 0 20 255 0 255 60"),
            ("_lightHDR", "invalid sun"),
            ("_ambientHDR", "invalid ambient"),
            ("_lightscaleHDR", "NaN"),
            ("_AmbientScaleHDR", "-1"),
        ]);
        for light in [
            SkyLight::from_entity(&source, &targets).unwrap(),
            SkyLight::from_entity_kind(&source, &targets, LightingKind::Ldr).unwrap(),
        ] {
            assert_eq!(light.world_light(), reference.world_light());
            assert_eq!(light.ambient_world_light(), reference.ambient_world_light());
            assert_eq!(light.intensity, DVec3::new(200.0, 0.0, 0.0));
            assert_eq!(light.ambient, DVec3::new(0.0, 20.0, 0.0));
        }
    }

    #[test]
    fn hdr_sun_and_ambient_select_independent_colors_and_scales() {
        let targets = map_core::light::Targets::new();
        for fields in [
            vec![
                ("_lightHDR", "0 255 0 1020"),
                ("_ambientHDR", "0 0 255 120"),
                ("_ambient", "invalid ignored fallback"),
            ],
            vec![
                ("_light", "255 0 0 200 0 255 0 1020"),
                ("_ambientHDR", "255 0 0 10 0 0 255 120"),
            ],
            vec![
                ("_light", "255 0 0 200 0 255 0 1020"),
                ("_ambientHDR", "-1 -1 -1 1"),
                ("_ambient", "255 0 0 10 0 0 255 120"),
            ],
        ] {
            let mut source = environment(&fields);
            source.properties.extend([
                ("_lightscaleHDR".into(), "2".into()),
                ("_AmbientScaleHDR".into(), "3".into()),
            ]);
            let light = SkyLight::from_entity_kind(&source, &targets, LightingKind::Hdr).unwrap();
            assert_eq!(light.intensity, DVec3::new(0.0, 2040.0, 0.0));
            assert_eq!(light.world_light().intensity, [0.0, 8.0, 0.0]);
            assert_eq!(light.ambient, DVec3::new(0.0, 0.0, 360.0));
            assert_eq!(
                light.ambient_radiance(),
                DVec3::new(0.0, 0.0, 360.0 / 255.0)
            );
            assert_eq!(
                light.ambient_world_light().unwrap().intensity,
                [0.0, 0.0, (360.0 / 255.0) as f32]
            );
        }
    }

    #[test]
    fn hdr_missing_or_sentinel_ambient_falls_back_after_sun_scale() {
        let targets = map_core::light::Targets::new();
        for fields in [
            vec![],
            vec![("_ambientHDR", ""), ("_ambient", "")],
            vec![("_ambientHDR", "-1 -1 -1 1")],
            vec![("_ambientHDR", "0 -1 255 1"), ("_ambient", "-1 -1 -1 1")],
        ] {
            let mut source = environment(&fields);
            source.properties.extend([
                ("_light".into(), "255 0 0 200".into()),
                ("_lightHDR".into(), "0 255 0 1020".into()),
                ("_lightscaleHDR".into(), "2".into()),
                ("_AmbientScaleHDR".into(), "3".into()),
            ]);
            let light = SkyLight::from_entity_kind(&source, &targets, LightingKind::Hdr).unwrap();
            assert_eq!(light.intensity, DVec3::new(0.0, 2040.0, 0.0));
            assert_eq!(light.ambient, DVec3::new(0.0, 3060.0, 0.0));
            assert_eq!(light.ambient_radiance(), DVec3::new(0.0, 12.0, 0.0));
        }
    }

    #[test]
    fn hdr_explicit_zero_ambient_overrides_the_sun_fallback() {
        let targets = map_core::light::Targets::new();
        for fields in [
            vec![("_ambientHDR", "0")],
            vec![("_ambientHDR", "255 255 255 0")],
            vec![
                ("_ambientHDR", "255 255 255 120"),
                ("_AmbientScaleHDR", "0"),
            ],
        ] {
            let mut source = environment(&fields);
            source.properties.extend([
                ("_lightHDR".into(), "255 0 0 1020".into()),
                ("_ambient".into(), "0 255 0 200".into()),
            ]);
            let light = SkyLight::from_entity_kind(&source, &targets, LightingKind::Hdr).unwrap();
            assert_eq!(light.intensity, DVec3::new(1020.0, 0.0, 0.0));
            assert_eq!(light.ambient, DVec3::ZERO);
            assert!(!light.has_ambient());
            assert!(light.ambient_world_light().is_none());
        }
    }

    #[test]
    fn malformed_selected_hdr_ambient_identifies_the_authored_field() {
        let targets = map_core::light::Targets::new();
        for (key, value) in [
            ("_ambientHDR", "not a color"),
            ("_ambientHDR", "1 2"),
            ("_ambientHDR", "NaN 0 0 100"),
            ("_ambientHDR", "0 0 0 inf"),
            ("_ambientHDR", "255 0 0 -1"),
            ("_ambientHDR", "255 0 0 100 0 NaN 0 100"),
            ("_ambient", "invalid fallback"),
            ("_AmbientScaleHDR", ""),
            ("_AmbientScaleHDR", "invalid"),
            ("_AmbientScaleHDR", "-1"),
            ("_AmbientScaleHDR", "NaN"),
            ("_AmbientScaleHDR", "inf"),
        ] {
            let source = environment(&[("_lightHDR", "0 255 0 1020"), (key, value)]);
            let error =
                SkyLight::from_entity_kind(&source, &targets, LightingKind::Hdr).unwrap_err();
            assert!(error.contains(key), "{key}={value}: {error}");
        }
    }

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
