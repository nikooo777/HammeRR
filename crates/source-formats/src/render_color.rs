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
