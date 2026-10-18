use super::{Actor, Session};
use crate::color::{Converted, Input, Space};

impl Session {
    pub fn convert_color(input: Input) -> Result<Converted, String> {
        crate::color::convert(input)
    }

    pub fn set_entity_color(
        &mut self,
        actor: Actor,
        id: u32,
        key: &str,
        color: [u8; 3],
        intensity: Option<f64>,
    ) -> Result<(), String> {
        self.set_entity_color_space(
            actor,
            id,
            key,
            Input {
                space: Space::Rgb255,
                channels: color.map(f64::from),
            },
            intensity,
        )
    }

    pub fn set_entity_color_space(
        &mut self,
        actor: Actor,
        id: u32,
        key: &str,
        input: Input,
        intensity: Option<f64>,
    ) -> Result<(), String> {
        let color = Self::convert_color(input)?;
        let inspector = self.entity_inspector(id)?;
        let field = inspector
            .properties
            .iter()
            .find(|field| field.key.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("unknown color property {key}"))?;
        if !matches!(field.kind.as_str(), "color255" | "color1") {
            return Err(format!("{key} is not a color property"));
        }
        if intensity.is_some_and(|value| !value.is_finite() || value < 0.0) {
            return Err("brightness must be a finite, nonnegative number".into());
        }
        if field.kind == "color1" && intensity.is_some() {
            return Err("normalized colors have no brightness component".into());
        }
        let render_color = key.eq_ignore_ascii_case("rendercolor") && field.kind == "color255";
        if render_color && intensity.is_some() {
            return Err("Model tint has no light brightness component".into());
        }
        let channels = if field.kind == "color1" {
            color.rgb1
        } else {
            color.rgb255.map(f64::from)
        };
        let mut values: Vec<String> = channels.iter().map(ToString::to_string).collect();
        if render_color {
            let components = field.value.split_whitespace().collect::<Vec<_>>();
            if components.len() == 4 {
                values.push(components[3].to_owned());
            }
        } else if let Some(intensity) = intensity.or(field.intensity) {
            values.push(intensity.to_string());
        }
        self.set_entity_properties(actor, id, &[(key.into(), Some(values.join(" ")))])
    }
}

#[cfg(test)]
mod tests;
