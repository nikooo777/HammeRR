use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    Rgb255,
    Rgb1,
    Hsv,
    Hsl,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Input {
    pub space: Space,
    pub channels: [f64; 3],
}

#[derive(Clone, Debug, Serialize)]
pub struct Converted {
    pub rgb1: [f64; 3],
    pub rgb255: [u8; 3],
    pub hsv: [f64; 3],
    pub hsl: [f64; 3],
}

impl Input {
    pub fn property(kind: &str, value: &str) -> Result<Self, String> {
        let space = match kind {
            "color1" => Space::Rgb1,
            "color255" => Space::Rgb255,
            _ => return Err("Property is not a color".into()),
        };
        let values = source_formats::fgd::vector(value, 3)
            .or_else(|| {
                if kind == "color255" {
                    source_formats::fgd::vector(value, 4)
                } else {
                    None
                }
            })
            .ok_or("Property requires a finite RGB color")?;
        if values.get(3).is_some_and(|value| *value < 0.0) {
            return Err("Brightness must be nonnegative".into());
        }
        let input = Self {
            space,
            channels: [values[0], values[1], values[2]],
        };
        convert(input)?;
        Ok(input)
    }
}

pub fn convert(input: Input) -> Result<Converted, String> {
    let channels = input.channels;
    if channels.iter().any(|value| !value.is_finite()) {
        return Err("Color channels must be finite numbers".into());
    }
    let rgb = match input.space {
        Space::Rgb255 => {
            if channels
                .iter()
                .any(|value| !(0.0..=255.0).contains(value) || value.fract() != 0.0)
            {
                return Err("RGB channels must be whole numbers from 0 to 255".into());
            }
            channels.map(|value| value / 255.0)
        }
        Space::Rgb1 => {
            if channels.iter().any(|value| !(0.0..=1.0).contains(value)) {
                return Err("Normalized RGB channels must be from 0 to 1".into());
            }
            channels
        }
        Space::Hsv | Space::Hsl => {
            let [hue, saturation, level] = channels;
            if !(0.0..=360.0).contains(&hue)
                || !(0.0..=100.0).contains(&saturation)
                || !(0.0..=100.0).contains(&level)
            {
                return Err("Hue must be from 0 to 360 degrees; saturation and value/lightness from 0 to 100 percent".into());
            }
            let saturation = saturation / 100.0;
            let level = level / 100.0;
            let chroma = match input.space {
                Space::Hsv => level * saturation,
                Space::Hsl => 2.0 * level.min(1.0 - level) * saturation,
                _ => unreachable!(),
            };
            let sector = (hue % 360.0) / 60.0;
            let secondary = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
            let base = match sector as u8 {
                0 => [chroma, secondary, 0.0],
                1 => [secondary, chroma, 0.0],
                2 => [0.0, chroma, secondary],
                3 => [0.0, secondary, chroma],
                4 => [secondary, 0.0, chroma],
                5 => [chroma, 0.0, secondary],
                _ => unreachable!(),
            };
            let offset = match input.space {
                Space::Hsv => level - chroma,
                Space::Hsl => level - chroma / 2.0,
                _ => unreachable!(),
            };
            base.map(|value| value + offset)
        }
    };
    if rgb
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err("Color conversion did not produce normalized RGB channels".into());
    }
    let maximum = rgb.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let minimum = rgb.into_iter().fold(f64::INFINITY, f64::min);
    let chroma = maximum - minimum;
    let hue = if chroma == 0.0 {
        0.0
    } else if maximum == rgb[0] {
        (60.0 * (rgb[1] - rgb[2]) / chroma).rem_euclid(360.0)
    } else if maximum == rgb[1] {
        60.0 * ((rgb[2] - rgb[0]) / chroma + 2.0)
    } else {
        60.0 * ((rgb[0] - rgb[1]) / chroma + 4.0)
    };
    let lightness = (maximum + minimum) / 2.0;
    let hsv_saturation = if maximum == 0.0 {
        0.0
    } else {
        chroma / maximum
    };
    let hsl_saturation = if chroma == 0.0 {
        0.0
    } else {
        let span = if lightness <= 0.5 {
            maximum + minimum
        } else {
            (1.0 - maximum) + (1.0 - minimum)
        };
        chroma / span
    };
    Ok(Converted {
        rgb1: rgb,
        rgb255: rgb.map(|value| (value * 255.0).round() as u8),
        hsv: [hue, hsv_saturation * 100.0, maximum * 100.0],
        hsl: [hue, hsl_saturation * 100.0, lightness * 100.0],
    })
}

#[cfg(test)]
mod tests;
