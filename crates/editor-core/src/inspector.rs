use serde::Serialize;
use source_formats::fgd::{Class, Property, vector};
use source_formats::vmf::Entity;

#[cfg(test)]
mod scale_tests;

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
    let property = if material_emission {
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
    let kind = if water_current {
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
    let stored = if material_emission {
        stored_value
    } else {
        entity.properties.get(&key)
    };
    let value = stored
        .map(str::to_owned)
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
            "Mounted relative path, up to {} ASCII bytes. Blank turns emission off. Compile to bake light from the declared world/detail materials. Fullbright does not bake emission.",
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
                && compiler::is_material_emission_key(&property.name)
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
            _ => 5,
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

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::fgd::Catalog;

    #[test]
    fn generated_detail_tint_inspector_keeps_linear_world_values_as_text() {
        let key = compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY;
        let catalog = Catalog::parse(&format!(
            "@SolidClass = worldspawn [ {key}(color255) : \"Tint\" ]"
        ))
        .unwrap();
        for class in [None, catalog.class("worldspawn")] {
            for value in ["128 64 0", "malformed"] {
                let mut world = Entity::new(1, "worldspawn");
                world.properties.set(key.to_ascii_uppercase(), value);
                let before = world.clone();
                let inspected = inspect(&world, class);
                let field = inspected
                    .properties
                    .iter()
                    .find(|field| field.key.eq_ignore_ascii_case(key))
                    .unwrap();
                assert_eq!(field.label, "Generated detail lighting tint · linear RGB");
                assert_eq!(field.control, Control::Text);
                assert_eq!(field.kind, "string");
                assert_eq!(field.value, value);
                assert!(field.color.is_none() && field.intensity.is_none());
                assert!(field.description.contains("map-wide"));
                assert_eq!(world, before);
            }
        }
    }

    #[test]
    fn common_controls_and_advanced_data_preserve_stored_values() {
        let catalog = Catalog::parse(
            r#"@PointClass = light_spot [
            _light(color255) : "Brightness" : "255 220 180 400"
            _lightHDR(color255) : "HDR brightness" : "-1 -1 -1 1"
            angles(angle) : "Orientation" : "0 0 0"
            movedir(angle) : "Move direction" : "0 90 0"
            _cone(integer) : "Outer angle" : 45
            enabled(boolean) : "Enabled" : 1
            material(material) : "Material" : "brick/wall"
            style(choices) : "Style" : 0 = [ 0 : "Steady" 1 : "Pulse" ]
            spawnflags(flags) = [ 1 : "Enabled" : 1 2 : "Special" : 0 ]
        ]"#,
        )
        .unwrap();
        let mut entity = Entity::new(1, "light_spot");
        entity.properties.set("origin", "1.25 2.500 -30");
        entity.properties.set("style", "99");
        entity.properties.set("spawnflags", "9");
        entity.properties.set("custom_key", "keep me");
        let inspector = inspect(&entity, catalog.class("light_spot"));
        let field = |key| {
            inspector
                .properties
                .iter()
                .find(|field| field.key == key)
                .unwrap()
        };
        assert_eq!(inspector.title, "Spotlight");
        assert_eq!(field("_light").label, "Light color");
        assert_eq!(field("_light").control, Control::Color);
        assert_eq!(field("_light").intensity, Some(400.0));
        assert_eq!(field("_light").color, Some([255, 220, 180]));
        assert_eq!(field("origin").components, ["1.25", "2.500", "-30"]);
        assert_eq!(field("angles").component_labels, ["Pitch", "Yaw", "Roll"]);
        assert_eq!(field("movedir").control, Control::Vector);
        assert_eq!(field("movedir").component_labels, ["Pitch", "Yaw", "Roll"]);
        assert_eq!(field("movedir").components, ["0", "90", "0"]);
        assert_eq!(field("_cone").control, Control::Slider);
        assert_eq!(field("enabled").control, Control::Boolean);
        assert_eq!(field("material").control, Control::Material);
        assert_eq!(field("_lightHDR").control, Control::Text);
        assert!(field("_lightHDR").advanced);
        assert_eq!(field("_lightHDR").value, "-1 -1 -1 1");
        assert_eq!(field("style").choices[0].value, "99");
        assert!(field("spawnflags").flags[0].checked);
        assert!(!field("spawnflags").flags[1].checked);
        assert!(field("custom_key").advanced);
        assert_eq!(field("custom_key").value, "keep me");
        assert!(entity.properties.get("_light").is_none());
    }

    #[test]
    fn invalid_imported_colors_keep_the_raw_editor() {
        let catalog =
            Catalog::parse(r#"@PointClass = light [ _light(color255) : "Brightness" ]"#).unwrap();
        for value in [
            "300 255 255 400",
            "255 255 255 -3",
            "2.5 255 255 400",
            "broken",
            "$Lamp",
        ] {
            let mut entity = Entity::new(1, "light");
            entity.properties.set("_light", value);
            let inspector = inspect(&entity, catalog.class("light"));
            let field = inspector
                .properties
                .iter()
                .find(|field| field.key == "_light")
                .unwrap();
            assert_eq!(field.value, value);
            assert_eq!(field.control, Control::Text);
            assert!(field.color.is_none() && field.intensity.is_none());
            if value == "$Lamp" {
                assert!(
                    field
                        .description
                        .contains("Unresolved template light value")
                );
            }
        }
    }

    #[test]
    fn detail_lighting_tint_keeps_its_linear_label_and_advanced_text_control() {
        for classname in ["prop_detail", "detail_prop", "prop_detail_sprite"] {
            let catalog = Catalog::parse(&format!(
                "@PointClass = {classname} [ _detail_lighting_tint(color255) : \"Tint\" ]"
            ))
            .unwrap();
            for value in ["128 64 0", "invalid imported tint"] {
                let mut entity = Entity::new(1, classname);
                entity.properties.set("_DETAIL_LIGHTING_TINT", value);
                let before = entity.clone();
                for class in [None, catalog.class(classname)] {
                    let inspector = inspect(&entity, class);
                    let field = inspector
                        .properties
                        .iter()
                        .find(|field| field.key.eq_ignore_ascii_case("_detail_lighting_tint"))
                        .unwrap();
                    assert_eq!(field.label, "Baked lighting tint · linear RGB");
                    assert_eq!(field.value, value);
                    assert_eq!(field.control, Control::Text);
                    assert_eq!(field.kind, "string");
                    assert!(field.advanced);
                    assert!(field.color.is_none() && field.intensity.is_none());
                    assert!(field.description.contains("linear RGB channels"));
                    assert!(field.description.contains("Dynamic lights"));
                }
                assert_eq!(entity, before);
            }
        }
    }
}
