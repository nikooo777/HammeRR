use super::{LightingChannel, MAX_EXPOSURE_EV, MAX_STYLE_GAIN, MIN_EXPOSURE_EV};

#[derive(
    Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct StyleGain {
    #[schemars(range(min = 0, max = 63))]
    pub style: u8,
    #[schemars(range(min = 0, max = MAX_STYLE_GAIN))]
    pub gain: f64,
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct LightingSettings {
    pub channel: LightingChannel,
    pub exposure_ev: f64,
    pub style_gains: Vec<StyleGain>,
}

impl LightingSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.exposure_ev.is_finite()
            || !(MIN_EXPOSURE_EV..=MAX_EXPOSURE_EV).contains(&self.exposure_ev)
        {
            return Err("Baked surface exposure must be finite and between -8 and 8 EV".into());
        }
        if self.style_gains.len() > 64 {
            return Err("Baked surfaces accept at most 64 distinct stored style gains".into());
        }
        let mut seen = [false; 64];
        for entry in &self.style_gains {
            if entry.style > 63
                || seen[usize::from(entry.style)]
                || !entry.gain.is_finite()
                || !(0.0..=MAX_STYLE_GAIN).contains(&entry.gain)
            {
                return Err(
                    "Baked surface gains require distinct styles 0–63 and finite gains 0–2".into(),
                );
            }
            seen[usize::from(entry.style)] = true;
        }
        Ok(())
    }

    pub fn gain(&self, style: u8) -> f64 {
        self.style_gains
            .iter()
            .find(|entry| entry.style == style)
            .map_or(if style == 0 { 1.0 } else { 0.0 }, |entry| entry.gain)
    }
}
