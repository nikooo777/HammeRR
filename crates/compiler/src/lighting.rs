use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::{
    LightingKind, MAX_LIGHTING_BYTES, RgbExp32, TexInfo, WorldLight, world_light_kind,
};

use crate::map::MapEntity;
mod bake;
mod bounce;
mod bump;
mod culling;
mod falloff;
mod models;
mod normals;
mod probes;
mod props;
mod radiance;
mod shadows;
mod sky;
pub(crate) mod styles;
mod terrain;
pub(crate) use bake::{Settings, bake};

const PROJECTION_EPSILON: f64 = 1e-10;
const MIN_LIGHT_DISTANCE: f64 = 1.0;
const REFERENCE_DISTANCE: f64 = 100.0;

#[derive(Debug, Clone, Copy)]
pub struct LuxelFrame {
    origin: DVec3,
    axes: [DVec3; 2],
    dimensions: [usize; 2],
}

impl LuxelFrame {
    pub fn new(
        plane: Plane,
        info: &TexInfo,
        mins: [i32; 2],
        size: [i32; 2],
    ) -> Result<Self, String> {
        let vectors = info
            .lightmap_vecs
            .map(|row| DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])));
        let normal = plane.normal;
        let determinant = vectors[0].dot(vectors[1].cross(normal));
        let scale = vectors[0].length() * vectors[1].length() * normal.length();
        if !plane.dist.is_finite()
            || !scale.is_finite()
            || scale == 0.0
            || !determinant.is_finite()
            || determinant.abs() <= PROJECTION_EPSILON * scale
        {
            return Err("Lightmap projection is singular or nonfinite".into());
        }
        let dual = [
            vectors[1].cross(normal) / determinant,
            normal.cross(vectors[0]) / determinant,
        ];
        let origin = dual[0] * (f64::from(mins[0]) - f64::from(info.lightmap_vecs[0][3]))
            + dual[1] * (f64::from(mins[1]) - f64::from(info.lightmap_vecs[1][3]))
            + vectors[0].cross(vectors[1]) * (plane.dist / determinant);
        let mut dimensions = [0; 2];
        for (dimension, extent) in dimensions.iter_mut().zip(size) {
            *dimension = usize::try_from(extent)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or("Lightmap extent is invalid")?;
        }
        let count = dimensions[0]
            .checked_mul(dimensions[1])
            .ok_or("Lightmap sample count overflow")?;
        if count > MAX_LIGHTING_BYTES / size_of::<RgbExp32>()
            || !origin.is_finite()
            || dual.iter().any(|axis| !axis.is_finite())
        {
            return Err("Lightmap samples exceed finite Source limits".into());
        }
        Ok(Self {
            origin,
            axes: dual,
            dimensions,
        })
    }

    pub fn dimensions(&self) -> [usize; 2] {
        self.dimensions
    }

    pub fn sample_count(&self) -> usize {
        self.dimensions[0] * self.dimensions[1]
    }

    pub fn position(&self, s: usize, t: usize) -> Result<DVec3, String> {
        if s >= self.dimensions[0] || t >= self.dimensions[1] {
            return Err("Luxel coordinates are outside the face".into());
        }
        let point = self.origin + self.axes[0] * s as f64 + self.axes[1] * t as f64;
        if !point.is_finite() {
            return Err("Luxel world position is nonfinite".into());
        }
        Ok(point)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    pub origin: DVec3,
    style: u8,
    intensity: DVec3,
    falloff: falloff::Falloff,
    cluster: i32,
    cone: Option<SpotCone>,
}

#[derive(Debug, Clone, Copy)]
struct SpotCone {
    direction: DVec3,
    inner: f64,
    outer: f64,
    exponent: f64,
}

fn property<'a>(entity: &'a MapEntity, name: &str) -> Option<&'a str> {
    entity
        .properties
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn number(entity: &MapEntity, name: &str) -> Result<f64, String> {
    let value = property(entity, name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("0")
        .parse::<f64>()
        .map_err(|_| format!("Invalid light {name}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("Light {name} must be finite and nonnegative"));
    }
    Ok(value)
}

#[derive(Clone, Copy)]
struct LightColor {
    components: [f64; 3],
    brightness: f64,
}

impl LightColor {
    fn parse(value: &str, kind: LightingKind) -> Result<Self, String> {
        let values: Vec<f64> = value
            .split_whitespace()
            .take(9)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| "Invalid point-light color/brightness")?;
        let offset = usize::from(kind == LightingKind::Hdr && values.len() == 8) * 4;
        let components = match values.len() {
            1 => [values[0]; 3],
            3 | 4 | 8 => [values[offset], values[offset + 1], values[offset + 2]],
            _ => {
                return Err(
                    "Point light brightness needs one, three, four or eight numbers".into(),
                );
            }
        };
        let brightness = if values.len() >= 4 {
            values[offset + 3]
        } else {
            255.0
        };
        Ok(Self {
            components,
            brightness,
        })
    }

    fn intensity(self, gain: f64) -> DVec3 {
        DVec3::from_array(
            self.components
                .map(|value| (value / 255.0).powf(2.2) * self.brightness * gain),
        )
    }
}

fn hdr_color(
    entity: &MapEntity,
    name: &str,
    allow_sentinel: bool,
) -> Result<Option<LightColor>, String> {
    let Some(value) = property(entity, name).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let color = LightColor::parse(value, LightingKind::Hdr)
        .map_err(|error| format!("HDR light {name}: {error}"))?;
    if color.components.iter().any(|value| !value.is_finite()) || !color.brightness.is_finite() {
        return Err(format!(
            "HDR light {name} color and brightness must be finite"
        ));
    }
    if color.components.iter().any(|&value| value < 0.0) {
        if allow_sentinel {
            return Ok(None);
        }
        return Err(format!("HDR light {name} colors must be nonnegative"));
    }
    if color.brightness < 0.0 {
        return Err(format!("HDR light {name} brightness must be nonnegative"));
    }
    Ok(Some(color))
}

fn hdr_scale(entity: &MapEntity, name: &str) -> Result<f64, String> {
    let Some(value) = property(entity, name) else {
        return Ok(1.0);
    };
    let value = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Invalid HDR light {name}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("HDR light {name} must be finite and nonnegative"));
    }
    Ok(value)
}

fn checked_intensity(intensity: DVec3) -> Result<DVec3, String> {
    if !intensity.is_finite() || intensity.max_element() > f64::from(f32::MAX) {
        return Err("Point light intensity exceeds native world-light limits".into());
    }
    Ok(intensity)
}

impl PointLight {
    pub fn from_entity(entity: &MapEntity, cluster: i32) -> Result<Self, String> {
        Self::from_entity_kind(entity, cluster, LightingKind::Ldr)
    }

    pub fn from_entity_kind(
        entity: &MapEntity,
        cluster: i32,
        kind: LightingKind,
    ) -> Result<Self, String> {
        let origin = entity.origin.ok_or("Point light needs an origin")?;
        if !origin.is_finite() || origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
            return Err(
                "Point light origin must be finite and inside Source compile bounds".into(),
            );
        }
        let style = number(entity, "style")?;
        let named = property(entity, "targetname").is_some_and(|value| !value.is_empty());
        if style.fract() != 0.0 || style > 63.0 || (style >= 32.0) != named {
            return Err("Point/spot light styles require an authored Appearance from 0 to 31 or a compiler-assigned named style from 32 to 63".into());
        }
        let color = match kind {
            LightingKind::Ldr => {
                let color = LightColor::parse(
                    property(entity, "_light").ok_or("Point light needs a brightness value")?,
                    kind,
                )?;
                if color
                    .components
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=255.0).contains(value))
                    || !color.brightness.is_finite()
                    || color.brightness < 0.0
                {
                    return Err(
                        "Point light colors must be 0–255 with finite nonnegative brightness"
                            .into(),
                    );
                }
                color
            }
            LightingKind::Hdr => match hdr_color(entity, "_lightHDR", true)? {
                Some(color) => color,
                None => hdr_color(entity, "_light", false)?
                    .ok_or("HDR point light needs _lightHDR or _light brightness")?,
            },
        };
        let falloff = falloff::Falloff::from_entity(entity)?;
        let mut intensity = color.intensity(falloff.gain);
        if kind == LightingKind::Hdr {
            intensity *= hdr_scale(entity, "_lightscaleHDR")?;
        }
        let intensity = checked_intensity(intensity)?;
        Ok(Self {
            origin,
            style: style as u8,
            intensity,
            falloff,
            cluster,
            cone: None,
        })
    }

    pub fn with_spotlight(
        mut self,
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        let inner = if property(entity, "_inner_cone").is_none() {
            10.0
        } else {
            number(entity, "_inner_cone")?
        };
        let inner = if inner == 0.0 { 10.0 } else { inner };
        let outer = number(entity, "_cone")?;
        let outer = if outer == 0.0 { inner } else { outer };
        if inner == 180.0 && outer == 180.0 {
            return Ok(self);
        }
        if inner > 90.0 || outer > 90.0 || outer < inner {
            return Err(
                "Spotlight cones need inner ≤ outer ≤ 90 degrees, or both 180 for a point light"
                    .into(),
            );
        }
        let exponent = number(entity, "_exponent")?;
        if exponent > f64::from(f32::MAX) {
            return Err("Spotlight focus exceeds native float limits".into());
        }
        let target = map_core::light::target(property(entity, "target"), targets)?;
        let direction = map_core::light::direction(
            property(entity, "angles"),
            property(entity, "angle"),
            property(entity, "pitch"),
            self.origin,
            target,
        )?;
        self.cone = Some(SpotCone {
            direction,
            inner: inner.to_radians().cos(),
            outer: outer.to_radians().cos(),
            exponent,
        });
        Ok(self)
    }

    pub fn contribution(&self, point: DVec3, normal: DVec3) -> Result<DVec3, String> {
        if !point.is_finite() || !normal.is_finite() {
            return Err("Light sample and normal must be finite".into());
        }
        let offset = self.origin - point;
        let distance = offset.length();
        if !distance.is_finite() {
            return Err("Point-light distance is nonfinite".into());
        }
        let normal = normal
            .try_normalize()
            .ok_or("Light sample needs a nonzero normal")?;
        if distance == 0.0 {
            return Ok(DVec3::ZERO);
        }
        let dot = normal.dot(offset / distance).clamp(0.0, 1.0);
        let falloff = self.falloff.factor(distance)?;
        let cone = if let Some(cone) = self.cone {
            let angle = cone.direction.dot(-offset / offset.length());
            if angle <= cone.outer {
                0.0
            } else if angle >= cone.inner {
                angle
            } else {
                let fringe = ((angle - cone.outer) / (cone.inner - cone.outer)).clamp(0.0, 1.0);
                angle
                    * if cone.exponent == 0.0 || cone.exponent == 1.0 {
                        fringe
                    } else {
                        fringe.powf(cone.exponent)
                    }
            }
        } else {
            1.0
        };
        let result = self.intensity * (dot * cone * falloff);
        if !result.is_finite() {
            return Err("Point-light contribution is nonfinite".into());
        }
        Ok(result)
    }

    pub fn world_light(&self) -> WorldLight {
        let mut light = WorldLight {
            origin: self.origin.as_vec3().to_array(),
            intensity: (self.intensity / 255.0).as_vec3().to_array(),
            cluster: self.cluster,
            kind: world_light_kind::POINT,
            style: i32::from(self.style),
            radius: self.falloff.radius as f32,
            constant_attenuation: self.falloff.coefficients[0] as f32,
            linear_attenuation: self.falloff.coefficients[1] as f32,
            quadratic_attenuation: self.falloff.coefficients[2] as f32,
            texinfo: -1,
            owner: -1,
            ..WorldLight::default()
        };
        if let Some(cone) = self.cone {
            light.kind = world_light_kind::SPOT;
            light.normal = cone.direction.as_vec3().to_array();
            light.stop_dot = cone.inner as f32;
            light.stop_dot2 = cone.outer as f32;
            light.exponent = cone.exponent as f32;
        }
        light
    }
}

pub fn encode_color(color: DVec3) -> Result<RgbExp32, String> {
    if !color.is_finite() || color.min_element() < 0.0 {
        return Err("Light color must be finite and nonnegative".into());
    }
    let maximum = color.max_element();
    if maximum == 0.0 {
        return Ok(RgbExp32::default());
    }
    let exponent = (maximum.log2().floor() as i32 - 7).max(i32::from(i8::MIN));
    if exponent > i32::from(i8::MAX) {
        return Err("Light color exceeds RGBExp32 limits".into());
    }
    let scaled = color / 2.0_f64.powi(exponent);
    Ok(RgbExp32 {
        rgb: scaled.to_array().map(|value| value.min(255.0) as u8),
        exponent: exponent as i8,
    })
}

#[derive(Debug, Clone)]
pub struct Occluder {
    planes: Vec<Plane>,
    sky: Vec<bool>,
    mins: DVec3,
    maxs: DVec3,
}

impl Occluder {
    pub fn new(planes: Vec<Plane>, mins: DVec3, maxs: DVec3) -> Result<Self, String> {
        if planes.len() < 4
            || planes.iter().any(|plane| {
                !plane.normal.is_finite()
                    || !plane.normal.length_squared().is_finite()
                    || plane.normal.length_squared() == 0.0
                    || !plane.dist.is_finite()
            })
            || !mins.is_finite()
            || !maxs.is_finite()
            || !mins.cmplt(maxs).all()
        {
            return Err("Shadow hull needs finite convex planes and positive bounds".into());
        }
        Ok(Self {
            sky: vec![false; planes.len()],
            planes,
            mins,
            maxs,
        })
    }

    pub fn plane_count(&self) -> usize {
        self.planes.len()
    }

    pub fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        Ok(self.entry(start, end)?.is_some())
    }

    pub(super) fn with_sky(mut self, flags: Vec<bool>) -> Result<Self, String> {
        if flags.len() != self.planes.len() {
            return Err("Sky flags must match shadow hull planes".into());
        }
        self.sky = flags;
        Ok(self)
    }

    pub(super) fn entry(&self, start: DVec3, end: DVec3) -> Result<Option<(f64, bool)>, String> {
        if !start.is_finite() || !end.is_finite() {
            return Err("Shadow ray must be finite".into());
        }
        if start.min(end).cmpgt(self.maxs).any() || start.max(end).cmplt(self.mins).any() {
            return Ok(None);
        }
        let mut entering: f64 = 0.0;
        let mut leaving: f64 = 1.0;
        let mut sky = false;
        for (index, plane) in self.planes.iter().enumerate() {
            let is_sky = self.sky.get(index).copied().unwrap_or(false);
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            if !a.is_finite() || !b.is_finite() {
                return Err("Shadow-plane distance is nonfinite".into());
            }
            if a > 0.0 && b > 0.0 {
                return Ok(None);
            }
            if a <= 0.0 && b <= 0.0 {
                continue;
            }
            let fraction = a / (a - b);
            if a > 0.0 {
                if fraction > entering {
                    sky = is_sky;
                } else if fraction == entering {
                    sky &= is_sky;
                }
                entering = entering.max(fraction);
            } else {
                leaving = leaving.min(fraction);
            }
            if entering > leaving {
                return Ok(None);
            }
        }
        Ok((entering < leaving && leaving > 0.0 && entering < 1.0).then_some((entering, sky)))
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    fn entity(properties: &[(&str, &str)]) -> MapEntity {
        MapEntity {
            id: 10,
            classname: "light".into(),
            origin: Some(DVec3::new(0.0, 0.0, 100.0)),
            brushes: Vec::new(),
            properties: properties
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn ldr_keeps_first_tuple_and_ignores_hdr_fields() {
        let reference = PointLight::from_entity(&entity(&[("_light", "255 0 0 120")]), 7).unwrap();
        for fields in [
            vec![
                ("_light", "255 0 0 120 0 255 0 1020"),
                ("_lightHDR", "not a color"),
                ("_lightscaleHDR", "NaN"),
            ],
            vec![
                ("_light", "255 0 0 120 NaN -1 inf -1"),
                ("_lightHDR", "0 0 0 0"),
                ("_lightscaleHDR", "-1"),
            ],
        ] {
            let source = entity(&fields);
            let legacy = PointLight::from_entity(&source, 7).unwrap();
            let explicit = PointLight::from_entity_kind(&source, 7, LightingKind::Ldr).unwrap();
            assert_eq!(legacy.world_light(), reference.world_light());
            assert_eq!(explicit.world_light(), reference.world_light());
            assert_eq!(legacy.intensity, DVec3::new(120.0, 0.0, 0.0));
            assert_eq!(
                encode_color(legacy.contribution(DVec3::ZERO, DVec3::Z).unwrap()).unwrap(),
                RgbExp32 {
                    rgb: [240, 0, 0],
                    exponent: -1,
                }
            );
        }
    }

    #[test]
    fn hdr_selects_its_tuple_and_override_before_scaling_linear_radiance() {
        for fields in [
            vec![
                ("_light", "255 0 0 120 0 255 0 1020"),
                ("_lightscaleHDR", "3"),
            ],
            vec![
                ("_light", "invalid ignored fallback"),
                ("_lightHDR", "255 0 0 120 0 255 0 1020"),
                ("_lightscaleHDR", "3"),
            ],
            vec![
                ("_lightHDR", "NaN -1 inf -1 0 255 0 1020"),
                ("_lightscaleHDR", "3"),
            ],
        ] {
            let light =
                PointLight::from_entity_kind(&entity(&fields), 7, LightingKind::Hdr).unwrap();
            assert_eq!(light.intensity, DVec3::new(0.0, 3060.0, 0.0));
            assert_eq!(light.world_light().intensity, [0.0, 12.0, 0.0]);
            assert_eq!(
                encode_color(light.contribution(DVec3::ZERO, DVec3::Z).unwrap()).unwrap(),
                RgbExp32 {
                    rgb: [0, 191, 0],
                    exponent: 4,
                }
            );
        }
        let light = PointLight::from_entity_kind(
            &entity(&[("_lightHDR", "128 255 0 1020"), ("_lightscaleHDR", "2")]),
            0,
            LightingKind::Hdr,
        )
        .unwrap();
        assert!((light.intensity.x - 447.8202248727305).abs() < 1e-10);
        assert_eq!(light.intensity.y, 2040.0);
        assert_eq!(light.intensity.z, 0.0);
        let light = PointLight::from_entity_kind(
            &entity(&[("_lightHDR", "510 0 0 255")]),
            0,
            LightingKind::Hdr,
        )
        .unwrap();
        assert!((light.intensity.x - 1171.672322096976).abs() < 1e-9);
        for (value, expected) in [
            ("255", DVec3::splat(255.0)),
            ("255 0 255", DVec3::new(255.0, 0.0, 255.0)),
        ] {
            let light = PointLight::from_entity_kind(
                &entity(&[("_lightHDR", value)]),
                0,
                LightingKind::Hdr,
            )
            .unwrap();
            assert_eq!(light.intensity, expected);
        }
    }

    #[test]
    fn hdr_sentinels_fall_back_and_explicit_zero_stays_dark() {
        for value in ["", "-1 -1 -1 1", "0 -1 255 1", "255 0 0 20 -1 -1 -1 1"] {
            let source = entity(&[("_light", "255 0 0 120 0 0 255 510"), ("_lightHDR", value)]);
            let light = PointLight::from_entity_kind(&source, 0, LightingKind::Hdr).unwrap();
            assert_eq!(light.intensity, DVec3::new(0.0, 0.0, 510.0));
        }
        for fields in [
            vec![("_lightHDR", "0 0 0 1020")],
            vec![("_lightHDR", "255 255 255 0")],
            vec![("_lightHDR", "255 255 255 1020"), ("_lightscaleHDR", "0")],
        ] {
            let mut source = entity(&fields);
            source.properties.extend([
                ("_light".into(), "255 255 255 200".into()),
                ("style".into(), "32".into()),
                ("targetname".into(), "switchable".into()),
            ]);
            let light = PointLight::from_entity_kind(&source, 7, LightingKind::Hdr).unwrap();
            assert_eq!(light.intensity, DVec3::ZERO);
            assert_eq!(light.world_light().intensity, [0.0; 3]);
            assert_eq!(light.world_light().style, 32);
            assert_eq!(light.world_light().cluster, 7);
        }
    }

    #[test]
    fn malformed_selected_hdr_values_identify_the_authored_field() {
        for (key, value) in [
            ("_lightHDR", "not a color"),
            ("_lightHDR", "1 2"),
            ("_lightHDR", "1 2 3 4 5"),
            ("_lightHDR", "1 2 3 4 5 6 7 8 9"),
            ("_lightHDR", "NaN 0 0 100"),
            ("_lightHDR", "0 0 0 inf"),
            ("_lightHDR", "255 0 0 -1"),
            ("_lightHDR", "255 0 0 100 0 NaN 0 100"),
            ("_lightscaleHDR", ""),
            ("_lightscaleHDR", "invalid"),
            ("_lightscaleHDR", "-1"),
            ("_lightscaleHDR", "NaN"),
            ("_lightscaleHDR", "inf"),
            ("_light", "255 0 0 100 0 0 0 -1"),
        ] {
            let source = if key == "_light" {
                entity(&[(key, value)])
            } else {
                entity(&[("_light", "255 255 255 200"), (key, value)])
            };
            let error = PointLight::from_entity_kind(&source, 0, LightingKind::Hdr).unwrap_err();
            assert!(error.contains(key), "{key}={value}: {error}");
        }
        let source = entity(&[("_lightHDR", "255 0 0 1e40")]);
        let error = PointLight::from_entity_kind(&source, 0, LightingKind::Hdr).unwrap_err();
        assert!(error.contains("intensity"), "{error}");
    }
}
