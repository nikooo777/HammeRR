use map_core::entity_helpers::{HelperValue, MAX_HELPER_EXTENT};
use render::icons::Instance;
use serde::Serialize;
use source_formats::{fgd::Class, vmf::Entity};
use std::sync::Arc;

use crate::sprite_resource::{self, MAX_REFERENCE_BYTES, preview};

use super::{super::Session, classification::Facts};

pub(super) const LIMITS: &str = "Sprite resource guide: one zero-argument sprite() on a generic point class with model(sprite). Authored model/scale precede individually labeled FGD defaults. Explicit .vmt or sprites/<basename>.spr only; no dotted filename stems. Full virtual Browse choices retain nested materials/ directories. Direct centered vp_parallel Sprite material and static single-frame LDR texture use the shared strict Entity images cache: 64 success/failure entries, 32 MiB encoded bytes, 64 KiB VMT, 4 MiB VTF, 512 pixels per axis, 256 visible image instances in entity-ID order. Positive finite explicit/FGD scale multiplies texture pixel dimensions; blank/absent/nonpositive/nonfinite scale is unavailable. Native scale, dimensions and vertices must stay finite, with relative corner extents within ±65536. Raw independent live-camera unlit RGBA alpha and no fog remain default. Optional Additive Sprite guides evaluate only explicitly authored Render Mode 5 for an LDR editor guide, constrained by unique authored or labeled FGD-default FX Color 255 255 255, FX Amount 255 and Render FX 0; other requested appearances retain labeled raw RGBA with diagnostics. Additive RGB uses SRC_ALPHA/ONE and preserves destination alpha as an editor policy, with existing scene-depth testing, no depth writes and stable center-depth order across mixed guides. The sRGB editor pipeline, guide depth ordering and no-fog behavior do not establish native Sprite pixel parity. Runtime color/amount/FX, HDRColorScale, glow visibility, animation, enable state and I/O remain unevaluated; HDRColorScale imposes no LDR constraint. Oriented sprites, nonzero angles/legacy angle, parents, loaded models, studio/model helpers, specialized lights, ropes and brushes are excluded. Ordinary FGD bounds, selection outlines, picks, placement, framing and working regions remain unchanged. Entity images disables admission and drawing; material reload clears successes and failures.";

#[derive(Debug, Clone, Serialize)]
pub struct Input {
    pub property: &'static str,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BlendPreview {
    pub requested: bool,
    pub appearance_supported: bool,
    pub applied: bool,
    pub effective_policy: &'static str,
    pub rendermode: Input,
    pub rendercolor: Input,
    pub renderamt: Input,
    pub renderfx: Input,
    pub evaluated_guide_mode: Option<u8>,
    pub error: Option<String>,
    pub alpha_policy: &'static str,
    pub color_policy: &'static str,
    pub depth_policy: &'static str,
}

fn byte(input: &Input) -> Result<u8, String> {
    if let Some(error) = &input.error {
        return Err(error.clone());
    }
    if input
        .property_type
        .as_deref()
        .is_some_and(|kind| !["integer", "choices", "string"].contains(&kind))
    {
        return Err(format!(
            "{} requires an integer, choices or string property",
            input.property
        ));
    }
    input
        .raw
        .as_deref()
        .and_then(|raw| raw.trim().parse::<u8>().ok())
        .ok_or_else(|| {
            format!(
                "{} requires one explicit or FGD-default integer byte",
                input.property
            )
        })
}

impl BlendPreview {
    fn resolve(class: &Class, entity: &Entity, requested: bool) -> Self {
        let mut report = Self {
            requested,
            appearance_supported: false,
            applied: false,
            effective_policy: "raw_rgba",
            rendermode: Input::resolve(class, entity, "rendermode"),
            rendercolor: Input::resolve(class, entity, "rendercolor"),
            renderamt: Input::resolve(class, entity, "renderamt"),
            renderfx: Input::resolve(class, entity, "renderfx"),
            evaluated_guide_mode: None,
            error: None,
            alpha_policy: "preserve_destination_alpha_for_additive",
            color_policy: "ldr_srgb_editor_image_no_fog_hdrcolorscale_unevaluated",
            depth_policy: "tested_against_existing_scene_depth_no_depth_writes_sorted_guide_centers",
        };
        if !requested {
            return report;
        }
        let result = report.admit();
        report.appearance_supported = result.is_ok();
        report.error = result.err();
        report
    }

    fn admit(&mut self) -> Result<(), String> {
        let mode = byte(&self.rendermode)?;
        if self.rendermode.value_source != "authored" {
            return Err("Additive guide requires explicitly authored Render Mode 5; an FGD default does not select it".into());
        }
        self.evaluated_guide_mode = Some(mode);
        if mode != 5 {
            return Err(format!(
                "Render Mode {mode} is outside the authored Additive guide; raw RGBA retained"
            ));
        }
        if let Some(error) = &self.rendercolor.error {
            return Err(error.clone());
        }
        if self
            .rendercolor
            .property_type
            .as_deref()
            .is_some_and(|kind| !["color255", "string"].contains(&kind))
        {
            return Err("rendercolor requires a color255 or string property".into());
        }
        let color = self.rendercolor.raw.as_deref().and_then(|raw| {
            raw.split_whitespace()
                .map(str::parse::<u8>)
                .collect::<Result<Vec<_>, _>>()
                .ok()
        });
        if color.as_deref() != Some(&[255, 255, 255]) {
            return Err("Additive guide requires FX Color 255 255 255 from authored values or a labeled FGD default; raw RGBA retained".into());
        }
        if byte(&self.renderamt)? != 255 {
            return Err("Additive guide requires FX Amount 255; raw RGBA retained".into());
        }
        if byte(&self.renderfx)? != 0 {
            return Err("Additive guide requires Render FX 0; raw RGBA retained".into());
        }
        Ok(())
    }
}

impl Input {
    fn resolve(class: &Class, entity: &Entity, key: &'static str) -> Self {
        let value = HelperValue::resolve(class, entity, key, true);
        let property = value.property;
        let raw = value.raw;
        let error = if value.duplicate {
            Some(format!("Duplicate {key} values are ambiguous"))
        } else if raw.is_some_and(|raw| raw.len() > MAX_REFERENCE_BYTES) {
            Some(format!("{key} exceeds 256 bytes"))
        } else {
            None
        };
        Self {
            property: key,
            property_type: property.map(|property| preview(&property.kind, MAX_REFERENCE_BYTES)),
            raw: raw.map(|raw| preview(raw, MAX_REFERENCE_BYTES)),
            raw_truncated: raw.is_some_and(|raw| raw.len() > MAX_REFERENCE_BYTES),
            value_source: value.source,
            error,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub source_kind: &'static str,
    pub declaration_supported: bool,
    pub model: Input,
    pub scale: Input,
    pub resolved_vmt: Option<String>,
    pub scale_value: Option<f32>,
    pub world_size: Option<[f32; 2]>,
    pub image: Option<render::icons::Report>,
    pub blend_preview: BlendPreview,
    pub state: &'static str,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
    pub error: Option<String>,
    pub orientation_policy: &'static str,
    pub rendering_policy: &'static str,
    pub size_policy: &'static str,
    pub runtime_fields_evaluated: bool,
    pub unevaluated_runtime_fields: Vec<&'static str>,
}

fn orientation(class: &Class, entity: &Entity) -> Result<(), String> {
    for (key, count) in [("angles", 3), ("angle", 1)] {
        let input = Input::resolve(class, entity, key);
        if let Some(error) = input.error {
            return Err(error);
        }
        if let Some(raw) = input.raw {
            let mut values = raw.split_whitespace();
            for _ in 0..count {
                let value = values.next().and_then(|value| value.parse::<f64>().ok());
                if value != Some(0.0) {
                    return Err(format!(
                        "Sprite resource guide requires zero finite {key}; orientation variants are unsupported"
                    ));
                }
            }
            if values.next().is_some() {
                return Err(format!(
                    "Sprite resource guide requires {count} zero {key} components"
                ));
            }
        }
    }
    for key in ["parentname", "parent"] {
        let input = Input::resolve(class, entity, key);
        if let Some(error) = input.error {
            return Err(error);
        }
        if input.raw.is_some_and(|raw| !raw.trim().is_empty()) {
            return Err("Parented Sprite resource guides are unsupported".into());
        }
    }
    Ok(())
}

fn scale(input: &Input) -> Result<f32, String> {
    if let Some(error) = &input.error {
        return Err(error.clone());
    }
    if input
        .property_type
        .as_deref()
        .is_some_and(|kind| !["string", "float", "integer"].contains(&kind))
    {
        return Err("Sprite Scale requires a scalar string, float or integer property".into());
    }
    let value = input
        .raw
        .as_deref()
        .and_then(|raw| raw.trim().parse::<f64>().ok());
    let value = value.filter(|value| value.is_finite() && *value > 0.0)
        .ok_or("Sprite resource sizing unavailable: enter a positive finite Scale; missing, blank and nonpositive values have no inferred default")?;
    let native = value as f32;
    if !native.is_finite() || native <= 0.0 {
        return Err("Sprite Scale must remain positive and finite in native precision".into());
    }
    Ok(native)
}

pub(super) fn row(
    session: &Session,
    class: &Class,
    entity: &Entity,
    facts: &Facts,
    view: (bool, bool),
    displayed: usize,
) -> (Report, Option<Instance>) {
    let (report, mut instance) = resolve(session, class, entity, facts, view, displayed);
    if instance.is_none()
        && view.0
        && facts.point
        && facts.valid_origin
        && !facts.hidden
        && !facts.outside
        && report.suppression_reason.is_none()
        && let Some(error) = &report.error
    {
        let image = report.image.clone().unwrap_or_else(|| {
            render::icons::Report::failure(
                report.resolved_vmt.as_deref().unwrap_or_default(),
                "unavailable",
                error,
            )
        });
        instance = Some(Instance {
            center: entity.origin().unwrap().as_vec3().to_array(),
            world_size: [0.0; 2],
            asset: Arc::new(render::icons::Asset {
                id: 0,
                report: image,
                material: None,
            }),
            blend: render::icons::Blend::Alpha,
        });
    }
    (report, instance)
}

fn resolve(
    session: &Session,
    class: &Class,
    entity: &Entity,
    facts: &Facts,
    view: (bool, bool),
    displayed: usize,
) -> (Report, Option<Instance>) {
    let (enabled, additive) = view;
    let declarations = class
        .helpers
        .iter()
        .filter(|helper| helper.name == "sprite")
        .take(2)
        .collect::<Vec<_>>();
    let supported = declarations.len() == 1
        && declarations[0].arguments.is_empty()
        && sprite_resource::is_source(Some(class));
    let mut report = Report {
        source_kind: "sprite_resource",
        declaration_supported: supported,
        model: Input::resolve(class, entity, "model"),
        scale: Input::resolve(class, entity, "scale"),
        resolved_vmt: None,
        scale_value: None,
        world_size: None,
        image: None,
        blend_preview: BlendPreview::resolve(class, entity, additive),
        state: "unsupported",
        displayed: false,
        suppression_reason: None,
        error: None,
        orientation_policy: "live_camera_right_up",
        rendering_policy: "unlit_rgba_editor_image_no_fog",
        size_policy: "texture_pixels_times_positive_scale",
        runtime_fields_evaluated: false,
        unevaluated_runtime_fields: vec![
            "rendermode",
            "rendercolor",
            "renderamt",
            "renderfx",
            "HDRColorScale",
            "glow_visibility",
            "animation",
            "enable_state",
            "io",
        ],
    };
    if report.blend_preview.evaluated_guide_mode.is_some() {
        report
            .unevaluated_runtime_fields
            .retain(|key| *key != "rendermode");
    }
    if !supported {
        report.error = Some("Exactly one zero-argument sprite() declaration and model(sprite) property are required".into());
        return (report, None);
    }
    let path = report.model.error.clone().map_or_else(
        || {
            report
                .model
                .raw
                .as_deref()
                .ok_or_else(|| "Sprite Name is missing".to_owned())
                .and_then(sprite_resource::material_path)
        },
        Err,
    );
    match path {
        Ok(path) => report.resolved_vmt = Some(path),
        Err(error) => {
            report.state = "unavailable";
            report.error = Some(preview(&error, 512));
            return (report, None);
        }
    }
    let scale = scale(&report.scale).and_then(|scale| orientation(class, entity).map(|()| scale));
    let scale = match scale {
        Ok(scale) => {
            report.scale_value = Some(scale);
            scale
        }
        Err(error) => {
            report.state = "unavailable";
            report.error = Some(preview(&error, 512));
            return (report, None);
        }
    };
    report.suppression_reason = if !facts.point {
        Some("brush_or_nonpoint")
    } else if entity
        .classname()
        .is_some_and(|name| name.eq_ignore_ascii_case("env_sprite_oriented"))
    {
        Some("oriented_sprite")
    } else if facts.light {
        Some("specialized_light")
    } else if facts.rope {
        Some("specialized_rope")
    } else if facts.loaded {
        Some("loaded_model")
    } else if class.helpers.iter().any(|helper| {
        ["studio", "studioprop", "model", "decal", "overlay"].contains(&helper.name.as_str())
    }) {
        Some("specialized_helper")
    } else if facts.invalid_size {
        Some("invalid_fgd_size")
    } else if facts.hidden {
        Some("authored_hidden")
    } else if facts.editor_hidden {
        Some("editor_hidden")
    } else if facts.outside {
        Some("outside_working_region")
    } else if !facts.valid_origin {
        Some("invalid_origin")
    } else if !enabled {
        Some("view_disabled")
    } else if displayed >= render::icons::MAX_INSTANCES {
        Some("scene_image_limit")
    } else {
        None
    };
    if report.suppression_reason.is_some() {
        report.state = "suppressed";
        return (report, None);
    }
    let asset = session
        .materials
        .icons
        .load_material_path(&session.game, report.resolved_vmt.as_deref().unwrap());
    let mut image = asset.report.clone();
    image.size_policy = report.size_policy;
    report.state = image.status;
    report.error.clone_from(&image.error);
    if asset.material.is_none() {
        report.image = Some(image);
        return (report, None);
    }
    let size = [image.width as f32 * scale, image.height as f32 * scale];
    let half_diagonal = size[0].hypot(size[1]) * 0.5;
    let center = entity.origin().unwrap().as_vec3();
    if size
        .iter()
        .any(|size| !size.is_finite() || *size <= 0.0 || *size * 0.5 <= 0.0)
        || !half_diagonal.is_finite()
        || f64::from(half_diagonal) > MAX_HELPER_EXTENT
        || !(center + glam::Vec3::splat(half_diagonal)).is_finite()
        || !(center - glam::Vec3::splat(half_diagonal)).is_finite()
    {
        report.state = "unavailable";
        report.error = Some("Sprite resource sizing must remain finite and positive in native precision with all relative corner extents within ±65536".into());
        report.image = Some(image);
        return (report, None);
    }
    report.world_size = Some(size);
    report.displayed = true;
    let blend = if report.blend_preview.appearance_supported {
        report.blend_preview.applied = true;
        report.blend_preview.effective_policy = "authored_additive";
        report.rendering_policy = "authored_additive_rgb_src_alpha_one_editor_image_no_fog";
        image.rendering_policy = report.rendering_policy;
        render::icons::Blend::Additive
    } else {
        render::icons::Blend::Alpha
    };
    image.world_size = size;
    image.displayed = true;
    report.image = Some(image);
    (
        report,
        Some(Instance {
            center: center.to_array(),
            world_size: size,
            asset,
            blend,
        }),
    )
}
