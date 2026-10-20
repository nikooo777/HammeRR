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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_uses_exact_linear_bytes_and_preserves_identity_arithmetic() {
        let color = DVec3::new(0.123456789, 128.0, 9e20);
        for tint in [
            Tint::parse(None).unwrap(),
            Tint::parse(Some("255 255 255")).unwrap(),
        ] {
            assert_eq!(tint.fullbright().unwrap().rgb, [255; 3]);
            assert_eq!(tint.apply(color), color);
        }
        let tint = Tint::parse(Some("128 64 0")).unwrap();
        assert_eq!(tint.fullbright().unwrap().rgb, [128, 64, 0]);
        assert_eq!(tint.fullbright().unwrap().exponent, 0);
        assert_eq!(tint.apply(DVec3::splat(255.0)).z, 0.0);
        assert!((tint.apply(DVec3::splat(255.0)) - DVec3::new(128.0, 64.0, 0.0)).length() < 1e-12);
        for invalid in [
            "", "255 255", "1 2 3 4", "-1 0 0", "256 0 0", "0.5 0 0", "NaN 0 0",
        ] {
            assert!(Tint::parse(Some(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn dictionary_tint_composes_exact_products_without_intermediate_byte_rounding() {
        let world = Tint::parse(Some("128 64 7")).unwrap();
        for raw in [None, Some("255 255 255")] {
            let identity = world.with_entry(raw).unwrap();
            let color = DVec3::new(0.123456789, 128.0, 9e20);
            assert_eq!(identity.apply(color), world.apply(color));
            assert_eq!(identity.fullbright().unwrap(), world.fullbright().unwrap());
        }
        let entry = Tint::default().with_entry(Some("128 64 7")).unwrap();
        assert_eq!(
            entry.apply(DVec3::splat(255.0)),
            world.apply(DVec3::splat(255.0))
        );
        assert_eq!(entry.fullbright().unwrap(), world.fullbright().unwrap());

        let combined = world.with_entry(Some("129 127 64")).unwrap();
        let expected = DVec3::new(16_512.0, 8_128.0, 448.0);
        assert!(
            (combined.apply(DVec3::splat(65_025.0)) - expected)
                .abs()
                .max_element()
                < 1e-10
        );
        assert_eq!(
            combined.fullbright().unwrap(),
            RgbExp32 {
                rgb: [129, 63, 3],
                exponent: -1,
            }
        );
        for invalid in ["", "1 2", "1 2 3 4", "256 0 0", "1.0 0 0", "NaN 0 0"] {
            assert!(world.with_entry(Some(invalid)).is_err(), "{invalid}");
        }
    }
}
