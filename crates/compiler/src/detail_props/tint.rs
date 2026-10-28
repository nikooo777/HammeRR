use glam::DVec3;
use source_formats::bsp::RgbExp32;
use source_formats::vmf::{Entity, Vmf};

pub(super) const KEY: &str = "_detail_lighting_tint";
pub const GENERATED_DETAIL_LIGHTING_TINT_KEY: &str = "_generated_detail_lighting_tint";

pub fn validate_generated_detail_lighting_tint(raw: Option<&str>) -> Result<(), String> {
    Tint::parse_key(raw, GENERATED_DETAIL_LIGHTING_TINT_KEY).map(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tint {
    color: [u8; 3],
    entry: [u8; 3],
}

impl Default for Tint {
    fn default() -> Self {
        Self {
            color: [255; 3],
            entry: [255; 3],
        }
    }
}

impl Tint {
    pub(crate) fn from_world(vmf: &Vmf) -> Result<Self, String> {
        let mut values = vmf
            .world
            .properties
            .get_all(GENERATED_DETAIL_LIGHTING_TINT_KEY);
        let raw = values.next();
        if values.next().is_some() {
            return Err(format!(
                "World repeats {GENERATED_DETAIL_LIGHTING_TINT_KEY}"
            ));
        }
        Self::parse_key(raw, GENERATED_DETAIL_LIGHTING_TINT_KEY)
    }

    pub(crate) fn from_entity(entity: &Entity) -> Result<Self, String> {
        let mut values = entity.properties.get_all(KEY);
        let raw = values.next();
        if values.next().is_some() {
            return Err("Explicit detail entity repeats _detail_lighting_tint".into());
        }
        Self::parse(raw)
    }

    fn parse(raw: Option<&str>) -> Result<Self, String> {
        Self::parse_key(raw, KEY)
    }

    pub(super) fn with_entry(self, raw: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            entry: Self::parse(raw)?.color,
            ..self
        })
    }

    fn parse_key(raw: Option<&str>, key: &str) -> Result<Self, String> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        let invalid = || {
            format!("Detail {key} needs exactly three integer linear RGB channels from 0 to 255")
        };
        let mut values = raw.split_whitespace();
        let mut color = [0; 3];
        for channel in &mut color {
            *channel = values
                .next()
                .ok_or_else(invalid)?
                .parse::<u8>()
                .map_err(|_| invalid())?;
        }
        if values.next().is_some() {
            return Err(invalid());
        }
        Ok(Self {
            color,
            ..Self::default()
        })
    }

    fn single_color(self) -> Option<[u8; 3]> {
        if self.entry == [255; 3] {
            Some(self.color)
        } else if self.color == [255; 3] {
            Some(self.entry)
        } else {
            None
        }
    }

    fn product(self) -> [u32; 3] {
        std::array::from_fn(|index| u32::from(self.color[index]) * u32::from(self.entry[index]))
    }

    pub(crate) fn fullbright(self) -> Result<RgbExp32, String> {
        if let Some(rgb) = self.single_color() {
            return Ok(RgbExp32 { rgb, exponent: 0 });
        }
        crate::lighting::encode_color(DVec3::from_array(
            self.product().map(|channel| f64::from(channel) / 255.0),
        ))
    }

    pub(crate) fn apply(self, color: DVec3) -> DVec3 {
        let factors = if let Some(rgb) = self.single_color() {
            if rgb == [255; 3] {
                return color;
            }
            rgb.map(|channel| f64::from(channel) / 255.0)
        } else {
            self.product().map(|channel| f64::from(channel) / 65_025.0)
        };
        color * DVec3::from_array(factors)
    }
}
