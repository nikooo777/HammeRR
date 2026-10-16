use serde::Serialize;
use source_formats::fgd::{Class, Property, vector};
use source_formats::vmf::Entity;

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
        "style" => "Light pattern".into(),
        "spawnflags" => "Options".into(),
        _ => property
            .filter(|property| !property.label.is_empty())
            .map_or_else(|| words(key), |property| property.label.clone()),
    }
}

fn field(entity: &Entity, class: Option<&Class>, key: String) -> Field {
    let property = class.and_then(|class| class.property(&key));
    let lower = key.to_ascii_lowercase();
    let kind = property.map_or_else(
        || match lower.as_str() {
            "origin" => "origin",
            "angles" => "vector",
            "_light" | "_ambient" => "color255",
            _ => "string",
        },
        |property| property.kind.as_str(),
    );
    let value = entity
        .properties
        .get(&key)
        .map(str::to_owned)
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
            if let Ok(value) = result.value.parse::<f64>()
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
        "material" | "decal" => result.control = Control::Material,
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
        if entity.solids.is_empty()
            && entity.properties.get("model").is_some()
            && !entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
        {
            result.label = "Tint".into();
            result.description="Multiply model textures by this color. White keeps the original colors. Opacity preview is not supported.".into();
            result.advanced = false;
        }
    }
    result
}

pub fn inspect(entity: &Entity, class: Option<&Class>) -> Inspector {
    let mut names: Vec<String> = entity
        .properties
        .iter()
        .map(|(key, _)| key.into())
        .collect();
    if let Some(class) = class {
        for property in &class.properties {
            if !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&property.name))
            {
                names.push(property.name.clone());
            }
        }
    }
    let mut properties: Vec<Field> = names
        .into_iter()
        .map(|key| field(entity, class, key))
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
        }
    }
}
