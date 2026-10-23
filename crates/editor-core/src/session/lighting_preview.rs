use std::collections::HashSet;

use compiler::lighting::PointLight;
use compiler::lighting::preview::{MAX_CONTRIBUTION, Parameters, Sun, SunParameters};
use map_core::ObjectId;
use render::lighting::{Light, MAX_LIGHTS, MAX_SUNS, Preview, Report};
use render::{SurfaceView, ViewOptions};
use source_formats::vmf::{Entity, Vmf};

use super::Session;

fn constant(entity: &Entity) -> Result<(), String> {
    for key in ["targetname", "pattern", "parentname"] {
        if entity
            .properties
            .get(key)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(format!(
                "Unshadowed preview excludes light {key}; only unnamed constant lights are supported"
            ));
        }
    }
    for key in ["style", "defaultstyle"] {
        let raw = entity
            .properties
            .get(key)
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .unwrap_or("0");
        if raw.parse::<u8>() != Ok(0) {
            return Err(format!(
                "Unshadowed preview requires {key} 0; animation and switchable lights are excluded"
            ));
        }
    }
    let flags = entity
        .properties
        .get("spawnflags")
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .unwrap_or("0")
        .parse::<u32>()
        .map_err(|_| "Light spawnflags must be a nonnegative integer")?;
    if flags & 1 != 0 {
        return Err("Unshadowed preview excludes Initially dark lights".into());
    }
    if !entity.solids.is_empty() {
        return Err(
            "Unshadowed preview requires point light entities without brush geometry".into(),
        );
    }
    Ok(())
}

fn pack(parameters: Parameters) -> Light {
    let cone = parameters.cone;
    let fade = parameters.fade.unwrap_or([0.0; 2]);
    Light {
        origin_radius: [
            parameters.origin[0],
            parameters.origin[1],
            parameters.origin[2],
            parameters.radius,
        ],
        intensity_cap: [
            parameters.intensity[0],
            parameters.intensity[1],
            parameters.intensity[2],
            parameters.cap.unwrap_or(0.0),
        ],
        coefficients_spot: [
            parameters.coefficients[0],
            parameters.coefficients[1],
            parameters.coefficients[2],
            if cone.is_some() {
                render::lighting::SPOT_KIND
            } else {
                render::lighting::POINT_KIND
            },
        ],
        direction_inner: cone.map_or([0.0; 4], |cone| {
            [
                cone.direction[0],
                cone.direction[1],
                cone.direction[2],
                cone.inner,
            ]
        }),
        outer_exponent_fade: [
            cone.map_or(0.0, |cone| cone.outer),
            cone.map_or(0.0, |cone| cone.exponent),
            fade[0],
            fade[1],
        ],
    }
}

fn pack_sun(parameters: SunParameters) -> Light {
    Light {
        origin_radius: [
            parameters.ambient[0],
            parameters.ambient[1],
            parameters.ambient[2],
            0.0,
        ],
        intensity_cap: [
            parameters.intensity[0],
            parameters.intensity[1],
            parameters.intensity[2],
            0.0,
        ],
        coefficients_spot: [0.0, 0.0, 0.0, render::lighting::SUN_KIND],
        direction_inner: [
            parameters.direction[0],
            parameters.direction[1],
            parameters.direction[2],
            0.0,
        ],
        ..Default::default()
    }
}

pub(super) fn prepare(vmf: &Vmf, visible: &HashSet<ObjectId>, view: ViewOptions) -> Preview {
    let mut preview = Preview {
        report: Report {
            requested: view.lighting_preview,
            active: view.lighting_preview && view.surface == SurfaceView::Textured,
            sky_ambient: view.lighting_sky_ambient,
            ..Default::default()
        },
        ..Default::default()
    };
    if !preview.report.active {
        return preview;
    }
    if !view.lighting_exposure_ev.is_finite() || !(-8.0..=8.0).contains(&view.lighting_exposure_ev)
    {
        preview.report.active = false;
        preview.report.unavailable =
            Some("Lighting exposure must be finite and between -8 and 8 EV".into());
        return preview;
    }
    if view.normal_maps && view.lighting_quality == render::lighting::Quality::Fast {
        preview.report.active = false;
        preview.report.unavailable = Some("Fast authored lighting does not support normal maps; choose Detail or disable Normal map preview".into());
        return preview;
    }
    let entities = vmf
        .entities
        .iter()
        .filter(|entity| !entity.hidden && visible.contains(&ObjectId::Entity(entity.id)));
    let suns = entities
        .clone()
        .filter(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("light_environment"))
        })
        .count();
    let mut targets = map_core::light::Targets::new();
    for entity in entities.clone() {
        if let (Some(name), Some(origin)) = (
            entity
                .properties
                .get("targetname")
                .filter(|name| !name.is_empty()),
            entity.origin(),
        ) {
            targets.entry(name).or_default().push(origin);
        }
    }
    let mut valid = 0;
    let mut headroom = 0.0;
    for entity in entities {
        let class = entity.classname().unwrap_or_default().to_ascii_lowercase();
        if class != "light" && !class.starts_with("light_") {
            continue;
        }
        let is_sun = class == "light_environment";
        let result = if !["light", "light_spot", "light_environment"].contains(&class.as_str()) {
            Err(format!(
                "Unshadowed preview does not support {class}; ambient and dynamic lights are excluded"
            ))
        } else if let Some((key, value)) = entity.properties.iter().find(|(key, value)| {
            map_core::light::color_parameter(entity.classname().unwrap_or_default(), key, value)
                .is_some()
        }) {
            Err(format!(
                "Unresolved template light value {key}={value}; bind it through replace01–replace10 on the immediately containing func_instance. Authored-lighting preview cannot resolve instance bindings"
            ))
        } else if is_sun && suns > MAX_SUNS {
            Err("Conflicting visible light_environment sources; every sun is excluded until only one remains visible".into())
        } else {
            constant(entity).and_then(|()| {
                let source = compiler::map::MapEntity {
                    id: entity.id,
                    classname: class.clone(),
                    properties: entity.properties.0.clone(),
                    brushes: Vec::new(),
                    origin: entity.origin(),
                };
                if is_sun {
                    let light = if view.lighting_sky_ambient {
                        Sun::from_entity_with_sky_ambient(&source, &targets)
                    } else {
                        Sun::from_entity(&source, &targets)
                    };
                    light
                        .and_then(Sun::preview_parameters)
                        .map(|parameters| (pack_sun(parameters), parameters.contribution_bound))
                } else {
                    PointLight::from_entity(&source, -1)
                        .and_then(|light| {
                            if class == "light_spot" {
                                light.with_spotlight(&source, &targets)
                            } else {
                                Ok(light)
                            }
                        })
                        .and_then(|light| light.preview_parameters())
                        .map(|parameters| (pack(parameters), parameters.contribution_bound))
                }
            })
        };
        match result {
            Ok((light, contribution_bound)) => {
                valid += 1;
                preview.report.admitted_suns += usize::from(is_sun);
                headroom += contribution_bound;
                if preview.lights.len() < MAX_LIGHTS {
                    preview.lights.push(light);
                    preview.report.admitted_entities.push(entity.id);
                }
            }
            Err(error) => {
                preview.report.excluded_lights += 1;
                preview.report.excluded_suns += usize::from(is_sun);
                preview.report.diagnostic(
                    Some(entity.id),
                    None,
                    &format!("Entity {}: {error}", entity.id),
                );
            }
        }
    }
    if valid > MAX_LIGHTS || headroom > MAX_CONTRIBUTION {
        preview.report.active = false;
        preview.report.unavailable = Some(if valid > MAX_LIGHTS {
            format!(
                "{valid} supported lights exceed the {MAX_LIGHTS}-light editor budget; no lights were selected"
            )
        } else {
            "Summed light radiance exceeds editor preview headroom; no lights were selected".into()
        });
        preview.lights.clear();
        preview.report.admitted_entities.clear();
        preview.report.excluded_lights += valid;
        preview.report.excluded_suns += preview.report.admitted_suns;
        preview.report.admitted_suns = 0;
    } else {
        preview.report.admitted_lights = valid;
    }
    preview
}

impl Session {
    pub fn lighting_preview_state(&mut self) -> serde_json::Value {
        let view = self.view();
        let report = self.build_scene_for_view(view).lighting.report;
        let source = report.source.clone();
        serde_json::json!({
            "enabled":view.lighting_preview, "sky_ambient":view.lighting_sky_ambient, "quality":view.lighting_quality.name(), "exposure_ev":view.lighting_exposure_ev,
            "summary":report.summary(), "report":report,
            "source":source,"source_scope":"current render document and visible working region; includes pending geometry previews",
            "limits":{"lights":MAX_LIGHTS,"suns":MAX_SUNS,"diagnostics":render::lighting::MAX_DIAGNOSTICS,"summed_linear_headroom":MAX_CONTRIBUTION,"exposure_ev":[-8,8],"receiver_counts":render::lighting::RECEIVER_COUNTS,"policy":render::lighting::POLICY}
        })
    }
}

#[cfg(test)]
mod tests;
