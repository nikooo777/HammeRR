use std::io;

pub const NORMAL: u8 = 0;
pub const TEXTURE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelAppearance {
    pub mode: u8,
    pub amount: u8,
}

impl ModelAppearance {
    pub fn opacity(self) -> f32 {
        if self.mode == NORMAL {
            1.0
        } else {
            f32::from(self.amount) / f32::from(u8::MAX)
        }
    }
}

pub fn model_appearance(
    mode: Option<&str>,
    amount: Option<&str>,
    effects: Option<&str>,
) -> Result<ModelAppearance, String> {
    let mode = mode.map_or(Ok(NORMAL), str::parse::<u8>).map_err(|_| {
        "Model render mode must be an integer; only Normal (0) and Texture (2) have editor previews"
            .to_owned()
    })?;
    if !matches!(mode, NORMAL | TEXTURE) {
        return Err(format!(
            "Model render mode {mode} has no editor preview; only Normal (0) and Texture (2) are supported"
        ));
    }
    let amount = amount
        .map_or(Ok(u8::MAX), str::parse::<u8>)
        .map_err(|_| "Model opacity renderamt must be an integer from 0 to 255".to_owned())?;
    let effects = effects
        .map_or(Ok(0), str::parse::<u8>)
        .map_err(|_| "Model render FX must be 0 for an editor preview".to_owned())?;
    if effects != 0 {
        return Err(format!(
            "Model render FX {effects} has no editor preview; use None (0) for static appearance"
        ));
    }
    Ok(ModelAppearance { mode, amount })
}

pub fn rgb(value: Option<&str>) -> io::Result<[u8; 3]> {
    let Some(value) = value else {
        return Ok([255; 3]);
    };
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Model tint needs three integer RGB channels from 0 to 255; an optional fourth component does not set model opacity",
        )
    };
    let mut components = value.split_whitespace();
    let mut color = [0; 3];
    for channel in &mut color {
        *channel = components
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
    }
    components.next();
    if components.next().is_some() {
        return Err(invalid());
    }
    Ok(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_opacity_uses_render_amount_only_in_texture_mode() {
        assert_eq!(model_appearance(None, None, None).unwrap().opacity(), 1.0);
        assert_eq!(
            model_appearance(Some("0"), Some("064"), Some("0"))
                .unwrap()
                .opacity(),
            1.0
        );
        for amount in [0, 64, 128, 255] {
            let appearance = model_appearance(Some("2"), Some(&amount.to_string()), None).unwrap();
            assert_eq!(appearance.mode, TEXTURE);
            assert_eq!(appearance.opacity(), amount as f32 / 255.0);
        }
        for mode in [
            "1", "3", "4", "5", "6", "7", "8", "9", "10", "-1", "256", "text",
        ] {
            assert!(model_appearance(Some(mode), None, None).is_err(), "{mode}");
        }
        for amount in ["-1", "256", "127.5", "NaN", ""] {
            assert!(
                model_appearance(Some("2"), Some(amount), None).is_err(),
                "{amount}"
            );
        }
        for effects in ["1", "2", "255", "-1", "bad"] {
            assert!(
                model_appearance(None, None, Some(effects)).is_err(),
                "{effects}"
            );
        }
    }

    #[test]
    fn render_colors_read_rgb_without_changing_an_optional_fourth_component() {
        assert_eq!(rgb(None).unwrap(), [255; 3]);
        assert_eq!(rgb(Some("12 34 56")).unwrap(), [12, 34, 56]);
        assert_eq!(rgb(Some("12 34 56 064")).unwrap(), [12, 34, 56]);
        for value in [
            "",
            "1 2",
            "1 2 3 4 5",
            "-1 0 0",
            "256 0 0",
            "0.5 0 0",
            "nan 0 0",
        ] {
            assert!(rgb(Some(value)).is_err(), "{value}");
        }
    }
}
