use glam::DVec3;
use source_formats::bsp::RgbExp32;
use source_formats::vmf::Entity;

const KEY: &str = "_detail_lighting_tint";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tint([u8; 3]);

impl Default for Tint {
    fn default() -> Self {
        Self([255; 3])
    }
}

impl Tint {
    pub(crate) fn from_entity(entity: &Entity) -> Result<Self, String> {
        let mut values = entity.properties.get_all(KEY);
        let raw = values.next();
        if values.next().is_some() {
            return Err("Explicit detail entity repeats _detail_lighting_tint".into());
        }
        Self::parse(raw)
    }

    fn parse(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        let invalid = || {
            "Detail _detail_lighting_tint needs exactly three integer linear RGB channels from 0 to 255".to_owned()
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
        Ok(Self(color))
    }

    pub(crate) fn fullbright(self) -> RgbExp32 {
        RgbExp32 {
            rgb: self.0,
            exponent: 0,
        }
    }

    pub(crate) fn apply(self, color: DVec3) -> DVec3 {
        if self == Self::default() {
            return color;
        }
        color * DVec3::from_array(self.0.map(|channel| f64::from(channel) / 255.0))
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
            assert_eq!(tint.fullbright().rgb, [255; 3]);
            assert_eq!(tint.apply(color), color);
        }
        let tint = Tint::parse(Some("128 64 0")).unwrap();
        assert_eq!(tint.fullbright().rgb, [128, 64, 0]);
        assert_eq!(tint.fullbright().exponent, 0);
        assert_eq!(tint.apply(DVec3::splat(255.0)).z, 0.0);
        assert!((tint.apply(DVec3::splat(255.0)) - DVec3::new(128.0, 64.0, 0.0)).length() < 1e-12);
        for invalid in [
            "", "255 255", "1 2 3 4", "-1 0 0", "256 0 0", "0.5 0 0", "NaN 0 0",
        ] {
            assert!(Tint::parse(Some(invalid)).is_err(), "{invalid}");
        }
    }
}
