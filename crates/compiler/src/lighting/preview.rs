use super::sky::SkyLight;
use super::{MIN_LIGHT_DISTANCE, PointLight};

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
        Ok(Self {
            light: SkyLight::direct_from_entity(entity, targets)?,
        })
    }

    pub fn contribution(self, normal: glam::DVec3) -> Result<glam::DVec3, String> {
        Ok(self.light.contribution(normal)? + self.light.ambient)
    }

    pub fn from_entity_with_sky_ambient(
        entity: &crate::map::MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        Ok(Self {
            light: SkyLight::from_entity(entity, targets)
                .map_err(|error| format!("Sun with LDR sky ambient: {error}"))?,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use source_formats::bsp::{Bsp, LightingKind};

    #[test]
    fn optional_ldr_sky_ambient_reuses_literals_defaults_and_per_channel_headroom() {
        let mut entity = crate::map::MapEntity {
            id: 10,
            classname: "light_environment".into(),
            origin: None,
            brushes: Vec::new(),
            properties: vec![
                ("_light".into(), "255 0 0 200".into()),
                ("pitch".into(), "-90".into()),
                ("_ambient".into(), "0 0 255 80".into()),
                ("_ambientHDR".into(), "ignored malformed HDR".into()),
            ],
        };
        let targets = Default::default();
        let light = Sun::from_entity_with_sky_ambient(&entity, &targets).unwrap();
        let parameters = light.preview_parameters().unwrap();
        assert_eq!(parameters.ambient, [0.0, 0.0, 80.0]);
        assert_eq!(parameters.contribution_bound, 200.0);
        for (normal, expected) in [
            (DVec3::Z, DVec3::new(200.0, 0.0, 80.0)),
            (DVec3::NEG_Z, DVec3::new(0.0, 0.0, 80.0)),
            (DVec3::Y, DVec3::new(0.0, 0.0, 80.0)),
        ] {
            assert_eq!(light.contribution(normal).unwrap(), expected);
        }
        let native = SkyLight::from_entity(&entity, &targets).unwrap();
        let directions = super::super::sky::ambient_directions(crate::SkyQuality::Fast);
        for normal in [DVec3::Z, DVec3::NEG_Z, DVec3::Y] {
            let ambient = native
                .ambient_contribution(normal, &directions, &mut |_| Ok(true))
                .unwrap();
            assert_eq!(ambient, DVec3::new(0.0, 0.0, 80.0));
        }
        entity.properties.retain(|(key, _)| key != "_ambient");
        let defaults = Sun::from_entity_with_sky_ambient(&entity, &targets)
            .unwrap()
            .preview_parameters()
            .unwrap();
        assert_eq!(defaults.ambient, [100.0, 0.0, 0.0]);
        assert_eq!(defaults.contribution_bound, 300.0);
        entity
            .properties
            .push(("_ambient".into(), "malformed".into()));
        assert!(Sun::from_entity(&entity, &targets).is_ok());
        assert!(Sun::from_entity_with_sky_ambient(&entity, &targets).is_err());
        entity.properties.pop();
        entity.properties[0].1 = "255 0 0 800000000000".into();
        assert!(
            Sun::from_entity(&entity, &targets)
                .unwrap()
                .preview_parameters()
                .is_ok()
        );
        assert!(
            Sun::from_entity_with_sky_ambient(&entity, &targets)
                .unwrap()
                .preview_parameters()
                .unwrap_err()
                .contains("headroom")
        );
    }

    #[test]
    fn sun_preview_reuses_compiler_direction_radiance_without_ambient_or_hdr() {
        let mut entity = crate::map::MapEntity {
            id: 10,
            classname: "light_environment".into(),
            origin: None,
            brushes: Vec::new(),
            properties: vec![
                ("_light".into(), "255 128 32 200".into()),
                ("pitch".into(), "-90".into()),
                ("_ambient".into(), "0 0 255 99".into()),
                ("_lightHDR".into(), "0 255 0 300".into()),
                ("SunSpreadAngle".into(), "45".into()),
            ],
        };
        let targets = Default::default();
        let native = SkyLight::from_entity(&entity, &targets).unwrap();
        assert_eq!(native.world_light().origin, [0.0; 3]);
        let hdr = SkyLight::from_entity_kind(&entity, &targets, LightingKind::Hdr).unwrap();
        assert_eq!(hdr.world_light().intensity, [0.0, 300.0 / 255.0, 0.0]);
        for light in [native, hdr] {
            assert_eq!(
                light.ambient_world_light().unwrap().intensity,
                [0.0, 0.0, 99.0 / 255.0]
            );
        }
        let sun = Sun::from_entity(&entity, &targets).unwrap();
        let parameters = sun.preview_parameters().unwrap();
        assert!(glam::Vec3::from_array(parameters.direction).distance(glam::Vec3::Z) < 1e-6);
        assert_eq!(parameters.contribution_bound, 200.0);
        let expected = DVec3::new(
            200.0,
            200.0 * (128.0_f64 / 255.0).powf(2.2),
            200.0 * (32.0_f64 / 255.0).powf(2.2),
        );
        assert!(sun.contribution(DVec3::Z).unwrap().distance(expected) < 1e-10);
        for normal in [DVec3::Z, DVec3::NEG_Z, DVec3::X, DVec3::new(1.0, 2.0, 3.0)] {
            assert_eq!(
                sun.contribution(normal).unwrap(),
                native.contribution(normal).unwrap()
            );
        }
        assert_eq!(sun.contribution(DVec3::NEG_Z).unwrap(), DVec3::ZERO);
        entity.origin = Some(DVec3::new(100.0, -200.0, 300.0));
        let translated = Sun::from_entity(&entity, &targets)
            .unwrap()
            .preview_parameters()
            .unwrap();
        assert_eq!(translated.direction, parameters.direction);
        assert_eq!(translated.intensity, parameters.intensity);
        let mut resolved = entity.clone();
        resolved.properties.push(("origin".into(), "1 2 3".into()));
        assert_eq!(
            SkyLight::from_entity(&resolved, &targets)
                .unwrap()
                .world_light()
                .origin,
            [100.0, -200.0, 300.0]
        );
        resolved
            .properties
            .push(("target".into(), "sun_origin_target".into()));
        let resolved_targets = map_core::light::Targets::from([(
            "sun_origin_target",
            vec![DVec3::new(101.0, -200.0, 300.0)],
        )]);
        assert_eq!(
            Sun::from_entity(&resolved, &resolved_targets)
                .unwrap()
                .preview_parameters()
                .unwrap()
                .direction,
            [-1.0, 0.0, 0.0]
        );
        for raw in ["", "0 0", "invalid 0 0", "NaN 0 0", "inf 0 0", "20000 0 0"] {
            for origin in [None, entity.origin] {
                let mut invalid = entity.clone();
                invalid.origin = origin;
                invalid.properties.push(("origin".into(), raw.into()));
                assert!(
                    Sun::from_entity(&invalid, &targets)
                        .unwrap_err()
                        .contains("Sun origin"),
                    "{raw:?} with resolved {origin:?}"
                );
                for kind in [LightingKind::Ldr, LightingKind::Hdr] {
                    assert!(
                        SkyLight::from_entity_kind(&invalid, &targets, kind)
                            .unwrap_err()
                            .contains("Sun origin")
                    );
                }
            }
        }
        entity.properties.push((
            "_ambientHDR".into(),
            "malformed ignored preview ambient".into(),
        ));
        assert!(
            Sun::from_entity(&entity, &targets)
                .unwrap()
                .preview_parameters()
                .is_ok()
        );
        assert!(SkyLight::from_entity_kind(&entity, &targets, LightingKind::Hdr).is_err());
        entity.properties[0].1 = "255 255 255 2000000000000".into();
        assert!(
            Sun::from_entity(&entity, &targets)
                .unwrap()
                .preview_parameters()
                .unwrap_err()
                .contains("headroom")
        );
        entity.properties.push(("_distance".into(), "10".into()));
        assert!(Sun::from_entity(&entity, &targets).is_err());
    }

    fn emitter(fields: &[(&str, &str)]) -> PointLight {
        let entity = crate::map::MapEntity {
            id: 10,
            classname: "light".into(),
            origin: Some(DVec3::new(0.0, 0.0, 100.0)),
            brushes: Vec::new(),
            properties: std::iter::once(("_light".to_string(), "255 128 32 120".to_string()))
                .chain(
                    fields
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string())),
                )
                .collect(),
        };
        PointLight::from_entity(&entity, 7).unwrap()
    }

    #[test]
    fn preview_export_preserves_native_bytes_and_cpu_contributions_and_retains_soft_fade() {
        for fields in [
            vec![],
            vec![
                ("_fifty_percent_distance", "1"),
                ("_zero_percent_distance", "1000"),
            ],
            vec![
                ("_fifty_percent_distance", "40"),
                ("_zero_percent_distance", "80"),
                ("_hardfalloff", "1"),
            ],
            vec![
                ("_constant_attn", "2"),
                ("_linear_attn", "3"),
                ("_quadratic_attn", "4"),
                ("_distance", "200"),
            ],
        ] {
            let light = emitter(&fields);
            let mut before = Bsp::new();
            before
                .set_world_lights(LightingKind::Ldr, &[light.world_light()])
                .unwrap();
            let reference = [
                DVec3::ZERO,
                DVec3::new(0.0, 0.0, 99.5),
                DVec3::new(0.0, 0.0, -1500.0),
            ]
            .map(|point| light.contribution(point, DVec3::Z).unwrap());
            let parameters = light.preview_parameters().unwrap();
            let mut after = Bsp::new();
            after
                .set_world_lights(LightingKind::Ldr, &[light.world_light()])
                .unwrap();
            assert_eq!(before.to_bytes(), after.to_bytes());
            assert_eq!(
                reference,
                [
                    DVec3::ZERO,
                    DVec3::new(0.0, 0.0, 99.5),
                    DVec3::new(0.0, 0.0, -1500.0)
                ]
                .map(|point| light.contribution(point, DVec3::Z).unwrap())
            );
            assert!(parameters.contribution_bound.is_finite());
            if fields.first() == Some(&("_fifty_percent_distance", "1")) {
                assert!(parameters.cap.is_some());
                assert!(parameters.fade.is_some());
                assert_eq!(light.world_light().radius, 0.0);
            }
        }
    }

    #[test]
    fn preview_export_rejects_unsafe_headroom_styles_and_precision_without_mutation() {
        let mut light = emitter(&[]);
        let native = light.world_light();
        light.intensity = DVec3::splat(MAX_CONTRIBUTION * 2.0);
        assert!(light.preview_parameters().unwrap_err().contains("headroom"));
        light.intensity = DVec3::ZERO;
        light.style = 1;
        assert!(light.preview_parameters().unwrap_err().contains("style-0"));
        assert_eq!(native.origin, light.world_light().origin);
        light.style = 0;
        light.origin = DVec3::splat(map_core::checks::SOURCE_COORDINATE_LIMIT + 1.0);
        assert!(light.preview_parameters().unwrap_err().contains("origin"));
        assert!(scalar(f64::MIN_POSITIVE).is_err());
        assert!(scalar(f64::INFINITY).is_err());
        let light = emitter(&[("_constant_attn", "1e-40"), ("_quadratic_attn", "1e30")]);
        assert!(light.preview_parameters().is_err());
    }
}
