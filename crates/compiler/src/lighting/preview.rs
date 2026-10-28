use super::sky::SkyLight;
use super::{MIN_LIGHT_DISTANCE, PointLight};
use source_formats::bsp::LightingKind;

#[derive(Clone, Copy, Debug)]
pub struct Sun {
    light: SkyLight,
}

#[derive(Clone, Copy, Debug)]
pub struct SunParameters {
    pub direction: [f32; 3],
    pub intensity: [f32; 3],
    pub ambient: [f32; 3],
    pub contribution_bound: f64,
}

impl Sun {
    pub fn from_entity(
        entity: &crate::map::MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        Self::from_entity_kind(entity, targets, LightingKind::Ldr, false)
    }

    pub fn from_entity_kind(
        entity: &crate::map::MapEntity,
        targets: &map_core::light::Targets<'_>,
        kind: LightingKind,
        sky_ambient: bool,
    ) -> Result<Self, String> {
        let light = if sky_ambient {
            let channel = match kind {
                LightingKind::Ldr => "LDR",
                LightingKind::Hdr => "HDR",
            };
            SkyLight::from_entity_kind(entity, targets, kind)
                .map_err(|error| format!("Sun with {channel} sky ambient: {error}"))?
        } else {
            SkyLight::direct_from_entity(entity, targets, kind)?
        };
        Ok(Self { light })
    }

    pub fn contribution(self, normal: glam::DVec3) -> Result<glam::DVec3, String> {
        Ok(self.light.contribution(normal)? + self.light.ambient)
    }

    pub fn from_entity_with_sky_ambient(
        entity: &crate::map::MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        Self::from_entity_kind(entity, targets, LightingKind::Ldr, true)
    }

    pub fn preview_parameters(self) -> Result<SunParameters, String> {
        let intensity = vector(self.light.intensity.to_array())?;
        let ambient = vector(self.light.ambient.to_array())?;
        let contribution_bound = intensity
            .into_iter()
            .zip(ambient)
            .map(|(direct, ambient)| f64::from(direct + ambient))
            .fold(0.0, f64::max);
        if contribution_bound > MAX_CONTRIBUTION {
            return Err("Sun exceeds the editor preview's linear-radiance headroom".into());
        }
        Ok(SunParameters {
            direction: vector((-self.light.direction).to_array())?,
            intensity,
            ambient,
            contribution_bound,
        })
    }
}

pub const MAX_CONTRIBUTION: f64 = 1.0e12;

#[derive(Clone, Copy, Debug)]
pub struct Cone {
    pub direction: [f32; 3],
    pub inner: f32,
    pub outer: f32,
    pub exponent: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Parameters {
    pub origin: [f32; 3],
    pub intensity: [f32; 3],
    pub coefficients: [f32; 3],
    pub radius: f32,
    pub cap: Option<f32>,
    pub fade: Option<[f32; 2]>,
    pub cone: Option<Cone>,
    pub contribution_bound: f64,
}

fn scalar(value: f64) -> Result<f32, String> {
    let packed = value as f32;
    if !packed.is_finite() || (value != 0.0 && packed.abs() < f32::MIN_POSITIVE) {
        return Err("Light cannot be represented safely by the editor's float preview".into());
    }
    Ok(packed)
}

fn vector(values: [f64; 3]) -> Result<[f32; 3], String> {
    Ok([scalar(values[0])?, scalar(values[1])?, scalar(values[2])?])
}

impl PointLight {
    pub fn preview_parameters(&self) -> Result<Parameters, String> {
        if !self.origin.is_finite()
            || self.origin.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
        {
            return Err("Light origin is outside the finite editor preview bounds".into());
        }
        if self.style != 0 {
            return Err("Unshadowed preview supports constant style-0 lights only".into());
        }
        let scale = self
            .falloff
            .coefficients
            .into_iter()
            .map(f64::abs)
            .fold(0.0, f64::max);
        let coefficients = vector(self.falloff.coefficients.map(|value| value / scale))?;
        let intensity = vector((self.intensity / scale).to_array())?;
        let radius = scalar(self.falloff.radius)?;
        let cap = self.falloff.cap.map(scalar).transpose()?;
        let fade = self
            .falloff
            .fade
            .map(|[start, end]| Ok::<_, String>([scalar(start)?, scalar(end)?]))
            .transpose()?;
        if fade.is_some_and(|[start, end]| end <= start) {
            return Err("Light fade interval collapses at editor preview precision".into());
        }
        let maximum_distance = 2.0 * map_core::checks::SOURCE_COORDINATE_LIMIT * 3.0_f64.sqrt();
        let end = self
            .falloff
            .fade
            .map_or(maximum_distance, |[_, end]| end.min(maximum_distance))
            .min(if radius > 0.0 {
                f64::from(radius)
            } else {
                maximum_distance
            });
        let low = cap.map_or(MIN_LIGHT_DISTANCE, |cap| {
            MIN_LIGHT_DISTANCE.min(f64::from(cap))
        });
        let high = cap.map_or(end.max(MIN_LIGHT_DISTANCE), |cap| {
            end.max(MIN_LIGHT_DISTANCE).min(f64::from(cap))
        });
        let [c, l, q] = coefficients.map(f64::from);
        let mut probes = vec![low, high];
        if q > 0.0 {
            probes.push((-l / (2.0 * q)).clamp(low, high));
        }
        let mut minimum = f64::INFINITY;
        for distance in probes {
            let terms = [c, distance * l, distance * distance * q];
            let divisor = terms.into_iter().sum::<f64>();
            let uncertainty =
                terms.into_iter().map(f64::abs).sum::<f64>() * 64.0 * f64::from(f32::EPSILON);
            if divisor <= uncertainty || !divisor.is_finite() {
                return Err(
                    "Light falloff loses positive-divisor precision in the editor preview".into(),
                );
            }
            minimum = minimum.min(divisor - uncertainty);
        }
        let contribution_bound = f64::from(intensity.into_iter().fold(0.0, f32::max)) / minimum;
        if !contribution_bound.is_finite() || contribution_bound > MAX_CONTRIBUTION {
            return Err("Light exceeds the editor preview's linear-radiance headroom".into());
        }
        let cone = self
            .cone
            .map(|cone| {
                let inner = scalar(cone.inner)?;
                let outer = scalar(cone.outer)?;
                if cone.inner > cone.outer && inner <= outer {
                    return Err("Spot cone interval collapses at editor preview precision".into());
                }
                Ok::<_, String>(Cone {
                    direction: vector(cone.direction.to_array())?,
                    inner,
                    outer,
                    exponent: scalar(cone.exponent)?,
                })
            })
            .transpose()?;
        Ok(Parameters {
            origin: vector(self.origin.to_array())?,
            intensity,
            coefficients,
            radius,
            cap,
            fade,
            cone,
            contribution_bound,
        })
    }
}
