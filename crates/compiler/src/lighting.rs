use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::{MAX_LIGHTING_BYTES, RgbExp32, TexInfo, WorldLight, world_light_kind};

use crate::map::MapEntity;
mod bake;
mod bounce;
mod falloff;
mod normals;
mod probes;
mod radiance;
mod shadows;
mod sky;
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

impl PointLight {
    pub fn from_entity(entity: &MapEntity, cluster: i32) -> Result<Self, String> {
        let origin = entity.origin.ok_or("Point light needs an origin")?;
        if !origin.is_finite() || origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
            return Err(
                "Point light origin must be finite and inside Source compile bounds".into(),
            );
        }
        if number(entity, "style")? != 0.0
            || property(entity, "targetname").is_some_and(|value| !value.is_empty())
        {
            return Err("Direct LDR supports static style-0 lights; named or animated lights need light-style baking".into());
        }
        let values: Vec<f64> = property(entity, "_light")
            .ok_or("Point light needs a brightness value")?
            .split_whitespace()
            .take(9)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| "Invalid point-light color/brightness")?;
        let components = match values.len() {
            1 => [values[0]; 3],
            3 | 4 | 8 => [values[0], values[1], values[2]],
            _ => {
                return Err(
                    "Point light brightness needs one, three, four or eight numbers".into(),
                );
            }
        };
        let brightness = if values.len() >= 4 { values[3] } else { 255.0 };
        if components
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=255.0).contains(value))
            || !brightness.is_finite()
            || brightness < 0.0
        {
            return Err(
                "Point light colors must be 0–255 with finite nonnegative brightness".into(),
            );
        }
        let falloff = falloff::Falloff::from_entity(entity)?;
        let intensity = DVec3::from_array(
            components.map(|value| (value / 255.0).powf(2.2) * brightness * falloff.gain),
        );
        if !intensity.is_finite() || intensity.max_element() > f64::from(f32::MAX) {
            return Err("Point light intensity exceeds native world-light limits".into());
        }
        Ok(Self {
            origin,
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
            style: 0,
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
