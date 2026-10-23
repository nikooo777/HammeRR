use glam::DVec3;
use source_formats::bsp::{LightingKind, WorldLight, world_light_kind};

use super::{
    PointLight, ReceiverAngularMode, checked_intensity, hdr_color, hdr_scale, number, property,
    unresolved_value,
};
use crate::map::MapEntity;

pub(super) const TRACE_LENGTH: f64 = crate::map::MAX_COMPILE_COORD * 4.0;
pub(super) const SKY_HIT_BIAS: f64 = 0.0001;
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
) -> Result<(MapEntity, Option<PointLight>, f64), String> {
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
        let (Some(point), Some(ambient)) = (point, ambient) else {
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
    ) -> Result<Self, String> {
        let (_, point, spread) = sun_source(entity, LightingKind::Ldr, false)?;
        let point = point.ok_or("Sun has an unresolved light-value parameter")?;
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

    #[cfg(test)]
    pub(super) fn sun_contribution(
        self,
        normal: DVec3,
        directions: &[DVec3],
        visible: &mut impl FnMut(DVec3) -> Result<bool, String>,
    ) -> Result<DVec3, String> {
        self.sun_contribution_with_mode(normal, directions, ReceiverAngularMode::Normal, visible)
    }

    #[cfg(test)]
    pub(super) fn sun_contribution_with_mode(
        self,
        normal: DVec3,
        directions: &[DVec3],
        mode: ReceiverAngularMode,
        visible: &mut impl FnMut(DVec3) -> Result<bool, String>,
    ) -> Result<DVec3, String> {
        self.sun_contribution_with_mode_visibility(normal, directions, mode, &mut |direction| {
            visible(direction).map(|visible| if visible { 1.0 } else { 0.0 })
        })
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

    #[cfg(test)]
    pub(super) fn ambient_contribution(
        self,
        normal: DVec3,
        directions: &[DVec3],
        visible: &mut impl FnMut(DVec3) -> Result<bool, String>,
    ) -> Result<DVec3, String> {
        self.ambient_contribution_with_mode(
            normal,
            directions,
            ReceiverAngularMode::Normal,
            visible,
        )
    }

    #[cfg(test)]
    pub(super) fn ambient_contribution_with_mode(
        self,
        normal: DVec3,
        directions: &[DVec3],
        mode: ReceiverAngularMode,
        visible: &mut impl FnMut(DVec3) -> Result<bool, String>,
    ) -> Result<DVec3, String> {
        self.ambient_contribution_with_mode_visibility(normal, directions, mode, &mut |direction| {
            visible(direction).map(|visible| if visible { 1.0 } else { 0.0 })
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SkyQuality;

    #[test]
    fn fractional_visibility_preserves_sun_ambient_weights_and_rejects_invalid_scalars() {
        let light = SkyLight {
            source_id: 0,
            direction: -DVec3::Z,
            intensity: DVec3::splat(100.0),
            ambient: DVec3::splat(80.0),
            origin: DVec3::ZERO,
            spread: 0.0,
        };
        let directions = [DVec3::Z, DVec3::X, DVec3::Y, -DVec3::X];
        for (mode, normal, expected) in [
            (ReceiverAngularMode::Normal, DVec3::Z, 49.901_960_784_313_72),
            (
                ReceiverAngularMode::IgnoreNormals,
                -DVec3::Z,
                17.465686274509803,
            ),
        ] {
            let mut values = [1.0, 191.0 / 255.0, 63.0 / 255.0, 0.0].into_iter();
            let color = light
                .sun_contribution_with_mode_visibility(normal, &directions, mode, &mut |_| {
                    Ok(values.next().unwrap())
                })
                .unwrap();
            assert!(color.abs_diff_eq(DVec3::splat(expected), 1e-12));
        }
        let directions = [DVec3::new((8.0_f64 / 9.0).sqrt(), 0.0, 1.0 / 3.0), DVec3::Z];
        let mut values = [191.0 / 255.0, 63.0 / 255.0].into_iter();
        let ambient = light
            .ambient_contribution_with_visibility(DVec3::Z, &directions, &mut |_| {
                Ok(values.next().unwrap())
            })
            .unwrap();
        assert!(ambient.abs_diff_eq(DVec3::splat(29.80392156862745), 1e-12));
        let mut values = [1.0, 191.0 / 255.0, 63.0 / 255.0, 0.0].into_iter();
        let ambient = light
            .ambient_contribution_with_mode_visibility(
                -DVec3::Z,
                &[DVec3::Z; 4],
                ReceiverAngularMode::IgnoreNormals,
                &mut |_| Ok(values.next().unwrap()),
            )
            .unwrap();
        assert!(ambient.abs_diff_eq(DVec3::splat(39.92156862745098), 1e-12));
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.0001] {
            assert!(
                light
                    .sun_contribution_with_visibility(DVec3::Z, &[DVec3::Z], &mut |_| Ok(value))
                    .is_err()
            );
            assert!(
                light
                    .ambient_contribution_with_visibility(DVec3::Z, &[DVec3::Z], &mut |_| Ok(value))
                    .is_err()
            );
        }
    }

    #[test]
    fn ignore_normals_sun_uses_constant_reception_and_all_visibility_rays() {
        let light = SkyLight {
            source_id: 0,
            direction: -DVec3::Z,
            intensity: DVec3::splat(100.0),
            ambient: DVec3::splat(40.0),
            origin: DVec3::ZERO,
            spread: 0.0,
        };
        let directions = [DVec3::Z, DVec3::X, DVec3::Y, -DVec3::X];
        for normal in [
            DVec3::X,
            -DVec3::X,
            DVec3::Y,
            -DVec3::Y,
            DVec3::Z,
            -DVec3::Z,
        ] {
            let mut calls = 0;
            let received = light
                .sun_contribution_with_mode(
                    normal,
                    &directions,
                    ReceiverAngularMode::IgnoreNormals,
                    &mut |direction| {
                        calls += 1;
                        Ok(direction != -DVec3::X)
                    },
                )
                .unwrap();
            assert_eq!(received, DVec3::splat(26.25));
            assert_eq!(calls, 4);
        }
        assert_eq!(
            light
                .sun_contribution(-DVec3::Z, &directions, &mut |_| Ok(true))
                .unwrap(),
            DVec3::ZERO
        );
        assert!(
            light
                .sun_contribution_with_mode(
                    DVec3::Z,
                    &directions,
                    ReceiverAngularMode::IgnoreNormals,
                    &mut |_| Err("canceled visibility".into())
                )
                .is_err()
        );
    }

    #[test]
    fn ignore_normals_ambient_is_a_full_sphere_mean_without_direct_scaling() {
        let light = SkyLight {
            source_id: 0,
            direction: -DVec3::Z,
            intensity: DVec3::splat(100.0),
            ambient: DVec3::splat(40.0),
            origin: DVec3::ZERO,
            spread: 0.0,
        };
        for quality in [SkyQuality::Fast, SkyQuality::Balanced, SkyQuality::Detailed] {
            let directions = ambient_directions(quality);
            for normal in [DVec3::Z, -DVec3::Z, DVec3::new(1.0, 2.0, -3.0).normalize()] {
                for (visibility, expected) in [(0, 0.0), (1, 20.0), (2, 40.0)] {
                    let mut calls = 0;
                    let received = light
                        .ambient_contribution_with_mode(
                            normal,
                            &directions,
                            ReceiverAngularMode::IgnoreNormals,
                            &mut |direction| {
                                calls += 1;
                                Ok(visibility == 2 || (visibility == 1 && direction.z > 0.0))
                            },
                        )
                        .unwrap();
                    assert_eq!(received, DVec3::splat(expected));
                    assert_eq!(calls, directions.len());
                }
            }
        }
        for normal in [DVec3::ZERO, DVec3::splat(f64::NAN)] {
            assert!(
                light
                    .ambient_contribution_with_mode(
                        normal,
                        &[DVec3::Z],
                        ReceiverAngularMode::IgnoreNormals,
                        &mut |_| Ok(true)
                    )
                    .is_err()
            );
            assert!(
                light
                    .sun_contribution_with_mode(
                        normal,
                        &[DVec3::Z],
                        ReceiverAngularMode::IgnoreNormals,
                        &mut |_| Ok(true)
                    )
                    .is_err()
            );
        }
    }

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
                    source_id: 0,
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
