use serde::Serialize;
use source_formats::fgd::{Class, Property, vector};
use source_formats::vmf::Entity;

pub(crate) const WATER_CURRENT_DEFAULT: &str = "0 0 0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Text,
    Number,
    Slider,
    Choices,
    Flags,
    Boolean,
    Vector,
    Color,
    Material,
    Model,
    Sound,
    Particle,
}

#[derive(Debug, Serialize)]
pub struct Inspector {
    pub helpers: Option<crate::session::fgd_helpers::Report>,
    pub title: String,
    pub classname: String,
    pub description: String,
    pub properties: Vec<Field>,
}

#[derive(Debug, Serialize)]
pub struct Field {
    pub key: String,
    pub value: String,
    pub label: String,
    pub description: String,
    pub kind: String,
    pub control: Control,
    pub advanced: bool,
    pub read_only: bool,
    pub choices: Vec<Choice>,
    pub flags: Vec<Flag>,
    pub components: Vec<String>,
    pub component_labels: Vec<String>,
    pub color: Option<[u8; 3]>,
    pub intensity: Option<f64>,
    pub range: Option<[f64; 2]>,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct Flag {
    pub label: String,
    pub value: String,
    pub checked: bool,
}

pub fn title(classname: &str) -> String {
    match classname.to_ascii_lowercase().as_str() {
        "light" => "Point light".into(),
        "light_spot" => "Spotlight".into(),
        "light_environment" => "Sunlight".into(),
        "light_dynamic" => "Dynamic light".into(),
        "ambient_generic" => "Ambient sound".into(),
        "info_particle_system" => "Particle system".into(),
        "info_player_terrorist" => "Terrorist spawn".into(),
        "info_player_counterterrorist" => "Counter-terrorist spawn".into(),
        "func_detail" => "Detail brushes".into(),
        other => words(other),
    }
}

fn words(value: &str) -> String {
    let mut text = value.trim_start_matches('_').replace('_', " ");
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    text
}

fn label(key: &str, property: Option<&Property>) -> String {
    match key.to_ascii_lowercase().as_str() {
        "classname" => "Entity class".into(),
        "targetname" => "Name".into(),
        "origin" => "Position".into(),
        "angles" => "Rotation".into(),
        "_light" => "Light color".into(),
        "_ambient" => "Ambient color".into(),
        "_cone" => "Outer cone angle".into(),
        "_inner_cone" => "Inner cone angle".into(),
        "_detail_lighting_tint" => "Baked lighting tint · linear RGB".into(),
        "_generated_detail_lighting_tint" => "Generated detail lighting tint · linear RGB".into(),
        "style" => "Light pattern".into(),
        "spawnflags" => "Options".into(),
        _ => property
            .filter(|property| !property.label.is_empty())
            .map_or_else(|| words(key), |property| property.label.clone()),
    }
}

fn dynamic_model(entity: &Entity, class: Option<&Class>) -> bool {
    entity.solids.is_empty()
        && entity
            .properties
            .get("model")
            .is_some_and(|path| render::models::model_path(path).is_ok())
        && !crate::sprite_resource::is_source(class)
        && !entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
}

fn field(entity: &Entity, class: Option<&Class>, key: String, stored_value: Option<&str>) -> Field {
    let material_emission = compiler::is_material_emission_key(&key)
        && entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"));
    let native_emission = compiler::is_material_emission_native_direct_key(&key)
        && entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"));
    let exact_emission = compiler::is_material_emission_exact_visibility_key(&key)
        && entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"));
    let emission_policy = native_emission || exact_emission;
    let property = if material_emission || emission_policy {
        None
    } else {
        class.and_then(|class| class.property(&key))
    };
    let lower = key.to_ascii_lowercase();
    let dynamic_model = dynamic_model(entity, class);
    let detail_tint = lower == "_detail_lighting_tint"
        && entity.classname().is_some_and(|class| {
            ["prop_detail", "detail_prop", "prop_detail_sprite"]
                .iter()
                .any(|name| class.eq_ignore_ascii_case(name))
        });
    let generated_detail_tint = lower == compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY
        && entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"));
    let water_current = lower == compiler::WATER_CURRENT_VELOCITY_KEY
        && entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"));
    let water_damping = lower == compiler::WATER_DAMPING_KEY
        && entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"));
    let kind = if emission_policy {
        "boolean"
    } else if water_current {
        "vector"
    } else if water_damping {
        "float"
    } else if detail_tint || generated_detail_tint {
        "string"
    } else {
        property.map_or_else(
            || match lower.as_str() {
                "origin" => "origin",
                "angles" => "vector",
                "_light" | "_ambient" => "color255",
                _ => "string",
            },
            |property| property.kind.as_str(),
        )
    };
    let stored = if material_emission || emission_policy {
        stored_value
    } else {
        entity.properties.get(&key)
    };
    let value = stored
        .map(str::to_owned)
        .or_else(|| emission_policy.then(|| "0".into()))
        .or_else(|| water_current.then(|| WATER_CURRENT_DEFAULT.into()))
        .or_else(|| water_damping.then(|| compiler::DEFAULT_WATER_DAMPING.to_string()))
        .or_else(|| property.and_then(Property::default_value))
        .unwrap_or_default();
    let mut result = Field {
        label: label(&key, property),
        description: property.map_or("", |property| &property.description).into(),
        kind: kind.into(),
        control: Control::Text,
        advanced: !matches!(
            lower.as_str(),
            "targetname"
                | "origin"
                | "angles"
                | "_light"
                | "_ambient"
                | "_cone"
                | "_inner_cone"
                | "pitch"
                | "style"
                | "spawnflags"
                | "material"
                | "model"
                | "skin"
        ),
        read_only: property.is_some_and(|property| property.read_only),
        choices: Vec::new(),
        flags: Vec::new(),
        components: Vec::new(),
        component_labels: Vec::new(),
        color: None,
        intensity: None,
        range: None,
        key,
        value,
    };
    if map_core::light::value_parameter(
        entity.classname().unwrap_or_default(),
        &result.key,
        &result.value,
    )
    .is_some()
    {
        if !result.description.is_empty() {
            result.description.push(' ');
        }
        result.description.push_str("Unresolved template light value. The immediately containing func_instance must bind this whole-value token through replace01–replace10. Standalone compilation and authored-lighting preview are unavailable until it is resolved.");
    }
    if detail_tint {
        result.description = "Three integer linear RGB channels from 0 to 255 attenuate compiled detail lighting. White keeps its original intensity; 128 approximately halves a linear channel. Compiler Fullbright starts from tinted white. Dynamic lights and the client's mat_fullbright override keep their engine behavior.".into();
    }
    if generated_detail_tint {
        result.description = "Three integer linear RGB channels from 0 to 255 attenuate baked lighting on all generated detail models, cards and shapes. White preserves their lighting; 128 approximately halves a linear channel. Explicit detail entities keep their own tint. Compiler Fullbright starts from tinted white; dynamic lights and the client's mat_fullbright override keep their engine behavior. This map-wide control does not edit dictionary entries.".into();
    }
    if material_emission {
        result.label = "Emission file".into();
        result.description = format!(
            "Mounted relative path, up to {} ASCII bytes. Blank turns emission off; disable Native direct emission and Exact emission visibility before clearing the file, or change them in one draft. Compile to bake uniform or texture-driven light from declared world/detail and eligible stationary brush materials. Fullbright does not bake emission.",
            compiler::MAX_MATERIAL_EMISSION_FILE_PATH_BYTES
        );
        result.advanced = false;
        if let Err(error) = compiler::validate_material_emission_file(&result.value) {
            result
                .description
                .push_str(&format!(" Stored emission file is invalid: {error}"));
        }
        if let Err(error) = compiler::material_emission_file_field(&entity.properties) {
            result.description.push_str(&format!(" {error}"));
        }
    }
    if native_emission {
        result.label = "Native direct emission".into();
        result.description = "Light runtime models from emitting surfaces. Uses native point approximations with limited light slots and ambient folding. Requires a nonblank Emission file. Fullbright keeps the setting without emitting lights.".into();
        result.advanced = false;
        if let Err(error) = compiler::validate_material_emission_native_direct(&result.value) {
            result.description.push_str(&format!(
                " Stored native emission policy is invalid: {error}"
            ));
        }
        if let Err(error) = compiler::material_emission_native_direct(&entity.properties) {
            result.description.push_str(&format!(" {error}"));
        }
    }
    if exact_emission {
        result.label = "Exact emission visibility".into();
        result.description = "Exact shadows from uniquely owned world/detail or stationary Shadows Yes func_brush emitting faces. Certified planar convex actual native faces may differ from their retained posed plane. Generated texture partitions at nonzero residual require exact actual-plane support and complete nonoverlapping coverage, including black groups, with no floating coverage allowance. Colour selection retains nearest mip0 RGB and binary64 UV semantics. Owning hulls retain real self-occlusion. Casters use certified closed convex opaque brushes and actual two-sided opaque terrain or admitted static-prop triangles. Nonplanar faces, uncertified nonzero-residual texture partitions, terrain and prop emitting sources, alpha and unqualified prop casters, unrepresentable on-plane fragments, other unsupported sources or casters, and numeric or resource limits stop the build. Requires Emission file. Native direct emission stays a point approximation. Fullbright checks the setting and declarations.".into();
        result.advanced = false;
        if let Err(error) = compiler::validate_material_emission_exact_visibility(&result.value) {
            result.description.push_str(&format!(
                " Stored exact emission visibility policy is invalid: {error}"
            ));
        }
        if let Err(error) = compiler::material_emission_exact_visibility(&entity.properties) {
            result.description.push_str(&format!(" {error}"));
        }
    }
    if water_current {
        result.label = "Liquid current · world units/s".into();
        result.description = format!(
            "Map-wide steady physical liquid velocity in world X/Y/Z units per second. Absent means 0 0 0 without a stored override. Exactly three finite components, at most {} input bytes and vector length at most {} units/s; no clamping or normalization. A stored value, including zero, requires CS:S engine physics and emitted structural-world WATER or opaque SLIME with visible horizontal tops; retained detail/model liquids reject. Liquid pressure damping is a separate World setting, defaulting to {}. This does not drive player/NPC movement or edit liquid materials.",
            compiler::MAX_WATER_CURRENT_VALUE_BYTES,
            compiler::MAX_WATER_CURRENT_SPEED,
            compiler::DEFAULT_WATER_DAMPING
        );
        result.advanced = false;
    }
    if water_damping {
        result.label = "Liquid pressure damping".into();
        result.description = format!(
            "Map-wide dimensionless coefficient for native liquid's projected-pressure term. Absent means {} without a stored override. Zero removes only the projected-pressure term; the native fixed pressure term remains. Enter one finite decimal from 0 to {}, at most {} input bytes; no clamping. A stored value, including zero or the default, requires CS:S engine physics and emitted structural-world WATER or opaque SLIME with visible horizontal tops; retained detail/model liquids reject. This does not drive player/NPC movement or edit liquid materials.",
            compiler::DEFAULT_WATER_DAMPING,
            compiler::MAX_WATER_DAMPING,
            compiler::MAX_WATER_DAMPING_VALUE_BYTES
        );
        result.advanced = false;
        result.range = Some([0.0, compiler::MAX_WATER_DAMPING]);
    }
    match kind {
        "choices" => {
            result.control = Control::Choices;
            result.choices = property
                .into_iter()
                .flat_map(|property| &property.choices)
                .map(|choice| Choice {
                    label: choice.label.clone(),
                    value: choice.value.clone(),
                })
                .collect();
            if !result
                .choices
                .iter()
                .any(|choice| choice.value == result.value)
            {
                result.choices.insert(
                    0,
                    Choice {
                        label: format!("{} (stored value)", result.value),
                        value: result.value.clone(),
                    },
                );
            }
        }
        "flags" => {
            result.control = Control::Flags;
            let mask = result.value.parse::<u64>().unwrap_or_default();
            result.flags = property
                .into_iter()
                .flat_map(|property| &property.choices)
                .map(|choice| {
                    let bit = choice.value.parse::<u64>().unwrap_or_default();
                    Flag {
                        label: choice.label.clone(),
                        value: choice.value.clone(),
                        checked: bit != 0 && mask & bit == bit,
                    }
                })
                .collect();
        }
        "boolean" | "bool" if matches!(result.value.as_str(), "0" | "1") => {
            result.control = Control::Boolean
        }
        "origin" | "vector" | "angle" if kind != "angle" || vector(&result.value, 3).is_some() => {
            if vector(&result.value, 3).is_some() {
                result.control = Control::Vector;
                result.components = result.value.split_whitespace().map(str::to_owned).collect();
                result.component_labels = if kind == "angle" || lower == "angles" {
                    ["Pitch", "Yaw", "Roll"]
                } else {
                    ["X", "Y", "Z"]
                }
                .map(str::to_owned)
                .into();
            }
        }
        "color255" | "color1" => {
            let values = vector(&result.value, 3).or_else(|| vector(&result.value, 4));
            if let Some(values) = values.filter(|values| {
                let max = if kind == "color1" { 1.0 } else { 255.0 };
                values[..3].iter().all(|value| (0.0..=max).contains(value))
                    && (kind != "color255" || values[..3].iter().all(|value| value.fract() == 0.0))
                    && (kind != "color1" || values.len() == 3)
                    && values.get(3).is_none_or(|value| *value >= 0.0)
            }) {
                let scale = if kind == "color1" { 255.0 } else { 1.0 };
                result.color = Some(std::array::from_fn(|i| {
                    (values[i] * scale).round().clamp(0.0, 255.0) as u8
                }));
                result.intensity = values.get(3).copied();
                result.control = Control::Color;
            }
        }
        "integer" | "float" | "angle" => {
            if !water_damping
                && let Ok(value) = result.value.parse::<f64>()
                && value.is_finite()
            {
                result.control = Control::Number;
                let range: Option<[f64; 2]> = match lower.as_str() {
                    "_cone" | "_inner_cone" => Some([0.0, 180.0]),
                    "pitch"
                        if entity
                            .classname()
                            .is_some_and(|name| name.starts_with("light")) =>
                    {
                        Some([-90.0, 90.0])
                    }
                    _ => None,
                };
                if let Some([min, max]) = range {
                    result.control = Control::Slider;
                    result.range = Some([min.min(value), max.max(value)]);
                }
            }
        }
        "material" | "decal" | "sprite" => result.control = Control::Material,
        "sound" => {
            result.control = Control::Sound;
            result.advanced = false;
        }
        _ if lower == "effect_name"
            && entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("info_particle_system")) =>
        {
            result.control = Control::Particle;
            result.advanced = false;
        }
        _ if lower == "model" => result.control = Control::Model,
        _ if lower == "material" => result.control = Control::Material,
        _ => {}
    }
    if lower == "rendercolor" && kind == "color255" {
        result.intensity = None;
        if dynamic_model {
            result.label = "Tint".into();
            result.description="Multiply model textures by this color. White keeps the original colors. Model opacity uses Render mode and Opacity; an authored fourth color component is preserved independently.".into();
            result.advanced = false;
        }
    }
    if dynamic_model && lower == "rendermode" {
        result.label = "Render mode".into();
        result.description = "Textured model preview supports Normal (0) and Texture (2). Normal ignores Opacity; Texture alpha-blends the model with depth testing and no depth writes. Other stored modes and render FX remain unsupported and retain a helper; no native game-parity claim.".into();
        result.kind = "choices".into();
        result.control = Control::Choices;
        result.advanced = false;
        result.choices = [("0", "Normal"), ("2", "Texture · opacity")]
            .map(|(value, label)| Choice {
                value: value.into(),
                label: label.into(),
            })
            .into();
        if !result
            .choices
            .iter()
            .any(|choice| choice.value == result.value)
        {
            result.choices.insert(
                0,
                Choice {
                    label: if result.value.parse::<u8>().is_ok_and(|mode| {
                        matches!(
                            mode,
                            source_formats::render_color::NORMAL
                                | source_formats::render_color::TEXTURE
                        )
                    }) {
                        format!("{} (stored mode)", result.value)
                    } else {
                        format!("{} (unsupported stored mode)", result.value)
                    },
                    value: result.value.clone(),
                },
            );
        }
    }
    if dynamic_model && lower == "renderamt" {
        result.label = "Opacity".into();
        result.description = "Texture render mode multiplies material alpha by this integer divided by 255: 0 is invisible, 255 keeps the material alpha. Normal render mode ignores this value. Bounds, picking and selection outlines remain available at every opacity.".into();
        result.advanced = false;
        if result.value.parse::<u8>().is_ok() {
            result.kind = "integer".into();
            result.control = Control::Slider;
            result.range = Some([0.0, 255.0]);
        }
    }
    if dynamic_model && render::models::scale_editable(entity) && lower == "modelscale" {
        result.label = "Model scale".into();
        result.description = "Positive uniform scale about the entity origin, at native float precision. The editor supports 1/64 through 64 with transformed mesh bounds within ±65536; these are editor numeric limits. Missing means 1. Mesh, picking, bounds and attachments share this scale. No animation, physics/collision or native game appearance parity is implied.".into();
        result.advanced = false;
        result.range = Some([
            f64::from(render::models::MIN_MODEL_SCALE),
            f64::from(render::models::MAX_MODEL_SCALE),
        ]);
        if entity.properties.get("modelscale").is_none() {
            result.value = "1".into();
        }
        if render::models::scale(entity).is_ok() {
            result.kind = "float".into();
            result.control = Control::Number;
        } else {
            result.control = Control::Text;
        }
    }
    if water_current
        && let Err(error) = compiler::validate_water_current_velocity(Some(&result.value))
    {
        result.control = Control::Text;
        result.components.clear();
        result.component_labels.clear();
        result
            .description
            .push_str(&format!(" Stored current is invalid: {error}"));
    }
    if water_damping {
        match compiler::validate_water_damping(Some(&result.value)) {
            Ok(()) => result.control = Control::Number,
            Err(error) => result
                .description
                .push_str(&format!(" Stored damping is invalid: {error}")),
        }
        if entity
            .properties
            .get_all(compiler::WATER_DAMPING_KEY)
            .count()
            > 1
        {
            result.control = Control::Text;
            result
                .description
                .push_str(" World has duplicate damping keys; resolve them before editing.");
        }
    }
    result
}

pub fn inspect(entity: &Entity, class: Option<&Class>) -> Inspector {
    let mut names: Vec<(String, Option<&str>)> = entity
        .properties
        .iter()
        .map(|(key, value)| (key.into(), Some(value)))
        .collect();
    if let Some(class) = class {
        for property in &class.properties {
            if entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"))
                && (compiler::is_material_emission_key(&property.name)
                    || compiler::is_material_emission_native_direct_key(&property.name)
                    || compiler::is_material_emission_exact_visibility_key(&property.name))
            {
                continue;
            }
            if !names
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(&property.name))
            {
                names.push((property.name.clone(), None));
            }
        }
    }
    if dynamic_model(entity, class)
        && render::models::scale_editable(entity)
        && !names
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("modelscale"))
    {
        names.push(("modelscale".into(), None));
    }
    if entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("worldspawn"))
    {
        if !names
            .iter()
            .any(|(name, _)| name == compiler::MATERIAL_EMISSION_FILE_KEY)
        {
            names.push((compiler::MATERIAL_EMISSION_FILE_KEY.into(), None));
        }
        if !names
            .iter()
            .any(|(name, _)| name == compiler::MATERIAL_EMISSION_NATIVE_DIRECT_KEY)
        {
            names.push((compiler::MATERIAL_EMISSION_NATIVE_DIRECT_KEY.into(), None));
        }
        if !names
            .iter()
            .any(|(name, _)| name == compiler::MATERIAL_EMISSION_EXACT_VISIBILITY_KEY)
        {
            names.push((
                compiler::MATERIAL_EMISSION_EXACT_VISIBILITY_KEY.into(),
                None,
            ));
        }
        for key in [
            compiler::WATER_CURRENT_VELOCITY_KEY,
            compiler::WATER_DAMPING_KEY,
        ] {
            if !names.iter().any(|(name, _)| name.eq_ignore_ascii_case(key)) {
                names.push((key.into(), None));
            }
        }
    }
    let mut properties: Vec<Field> = names
        .into_iter()
        .map(|(key, value)| field(entity, class, key, value))
        .collect();
    properties.sort_by_key(|field| {
        let priority = match field.key.to_ascii_lowercase().as_str() {
            "targetname" => 0,
            "_light" => 1,
            "_ambient" => 2,
            "origin" => 3,
            "angles" => 4,
            compiler::MATERIAL_EMISSION_FILE_KEY => 5,
            compiler::MATERIAL_EMISSION_NATIVE_DIRECT_KEY => 6,
            compiler::MATERIAL_EMISSION_EXACT_VISIBILITY_KEY => 7,
            _ => 8,
        };
        (field.advanced, priority)
    });
    let classname = entity.classname().unwrap_or("entity");
    Inspector {
        helpers: None,
        title: title(classname),
        classname: classname.into(),
        description: class.map_or("", |class| &class.description).into(),
        properties,
    }
}
