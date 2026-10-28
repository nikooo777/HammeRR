use glam::DVec3;
use source_formats::bsp::{LightingKind, WorldLight, world_light_kind};

use super::{
    PointLight, ReceiverAngularMode, checked_intensity, hdr_color, hdr_scale, number, property,
    unresolved_value,
};
use crate::map::MapEntity;

pub(super) const TRACE_LENGTH: f64 = crate::map::MAX_COMPILE_COORD * 4.0;
const GOLDEN_ANGLE: f64 = 2.399963229728653;
const MAX_SUN_SPREAD_ANGLE: f64 = 90.0;

fn checked_visibility(value: f64) -> Result<f64, String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err("Light visibility must be finite and between zero and one".into());
    }
    Ok(value)
}

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
    defer_parameters: bool,
) -> Result<(MapEntity, Option<PointLight>, Option<f64>), String> {
    if number(entity, "style")? != 0.0
        || property(entity, "targetname").is_some_and(|name| !name.is_empty())
        || property(entity, "pattern").is_some_and(|pattern| !pattern.is_empty())
    {
        return Err("Sun and sky lighting require unnamed static Appearance Normal".into());
    }
    let spread = if unresolved_value(entity, "SunSpreadAngle", defer_parameters) {
        None
    } else {
        Some(number(entity, "SunSpreadAngle")?)
    };
    if spread.is_some_and(|spread| spread > MAX_SUN_SPREAD_ANGLE) {
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
    let point = PointLight::parse(&source, -1, kind, defer_parameters)?;
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
    pub(super) source_id: u32,
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
        Self::parse(entity, targets, kind, false)?
            .ok_or_else(|| "Sun and sky light has an unresolved light-value parameter".into())
    }

    pub(super) fn parse(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
        kind: LightingKind,
        defer_parameters: bool,
    ) -> Result<Option<Self>, String> {
        let (source, point, spread) = sun_source(entity, kind, defer_parameters)?;
        let origin = source.origin.ok_or("Sun needs an origin")?;
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
                    PointLight::parse(&source, -1, kind, defer_parameters)?
                        .map(|point| point.intensity)
                } else {
                    point.as_ref().map(|point| point.intensity * 0.5)
                }
            }
            LightingKind::Hdr => {
                let scale = hdr_scale(entity, "_AmbientScaleHDR", defer_parameters)?;
                let ambient = if unresolved_value(entity, "_ambientHDR", defer_parameters) {
                    None
                } else {
                    let color = hdr_color(entity, "_ambientHDR", true)?;
                    if color.is_none() && unresolved_value(entity, "_ambient", defer_parameters) {
                        None
                    } else {
                        let color = match color {
                            Some(color) => Some(color),
                            None => hdr_color(entity, "_ambient", true)?,
                        };
                        color
                            .map(|color| color.intensity(1.0))
                            .or_else(|| point.as_ref().map(|point| point.intensity * 0.5))
                    }
                };
                ambient
                    .zip(scale)
                    .map(|(ambient, scale)| checked_intensity(ambient * scale))
                    .transpose()
                    .map_err(|error| format!("HDR sky ambient: {error}"))?
            }
        };
        let direction = sun_direction(entity, targets, origin)?;
        let (Some(point), Some(ambient), Some(spread)) = (point, ambient, spread) else {
            return Ok(None);
        };
        Ok(Some(Self {
            source_id: entity.id,
            direction,
            intensity: point.intensity,
            ambient,
            origin,
            spread: spread.to_radians().sin(),
        }))
    }

    pub(super) fn direct_from_entity(
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
        kind: LightingKind,
    ) -> Result<Self, String> {
        let (_, point, spread) = sun_source(entity, kind, false)?;
        let point = point.ok_or("Sun has an unresolved light-value parameter")?;
        let spread = spread.ok_or("Sun has an unresolved light-value parameter")?;
        let origin = point.origin;
        Ok(Self {
            source_id: entity.id,
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

    pub(super) fn sun_contribution_with_visibility(
        self,
        normal: DVec3,
        directions: &[DVec3],
        visible: &mut impl FnMut(DVec3) -> Result<f64, String>,
    ) -> Result<DVec3, String> {
        self.sun_contribution_with_mode_visibility(
            normal,
            directions,
            ReceiverAngularMode::Normal,
            visible,
        )
    }

    pub(super) fn sun_contribution_with_mode_visibility(
        self,
        normal: DVec3,
        directions: &[DVec3],
        mode: ReceiverAngularMode,
        visible: &mut impl FnMut(DVec3) -> Result<f64, String>,
    ) -> Result<DVec3, String> {
        let color = self.contribution_with_mode(normal, mode)?;
        if color == DVec3::ZERO {
            return Ok(color);
        }
        if directions.is_empty() {
            return Err("Sun visibility needs at least one direction".into());
        }
        let mut received = 0.0;
        for &direction in directions {
            received += checked_visibility(visible(direction)?)?;
        }
        Ok(color * (received / directions.len() as f64))
    }

    pub(super) fn contribution(self, normal: DVec3) -> Result<DVec3, String> {
        self.contribution_with_mode(normal, ReceiverAngularMode::Normal)
    }

    fn contribution_with_mode(
        self,
        normal: DVec3,
        mode: ReceiverAngularMode,
    ) -> Result<DVec3, String> {
        let normal = normal
            .try_normalize()
            .ok_or("Sky sample normal must be finite and nonzero")?;
        let color = self.intensity * mode.direct_factor(normal, -self.direction);
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

    pub(super) fn ambient_contribution_with_visibility(
        self,
        normal: DVec3,
        directions: &[DVec3],
        visible: &mut impl FnMut(DVec3) -> Result<f64, String>,
    ) -> Result<DVec3, String> {
        self.ambient_contribution_with_mode_visibility(
            normal,
            directions,
            ReceiverAngularMode::Normal,
            visible,
        )
    }

    pub(super) fn ambient_contribution_with_mode_visibility(
        self,
        normal: DVec3,
        directions: &[DVec3],
        mode: ReceiverAngularMode,
        visible: &mut impl FnMut(DVec3) -> Result<f64, String>,
    ) -> Result<DVec3, String> {
        let normal = normal
            .try_normalize()
            .ok_or("Sky ambient needs a finite nonzero normal")?;
        let mut total = 0.0;
        let mut received = 0.0;
        for &direction in directions {
            let weight = mode.ambient_weight(normal, direction);
            if weight == 0.0 {
                continue;
            }
            total += weight;
            received += weight * checked_visibility(visible(direction)?)?;
        }
        if total <= 0.0 {
            return Err(match mode {
                ReceiverAngularMode::Normal => "Sky ambient hemisphere has no valid directions",
                ReceiverAngularMode::IgnoreNormals => "Sky ambient sphere has no valid directions",
            }
            .into());
        }
        Ok(self.ambient * (received / total))
    }
}
