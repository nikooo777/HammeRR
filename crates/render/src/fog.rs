#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    color: [u8; 3],
    start: f32,
    end: f32,
    max_density: f32,
    enabled: bool,
}

impl Fog {
    pub fn new(
        color: [u8; 3],
        start: f32,
        end: f32,
        max_density: f32,
        enabled: bool,
    ) -> Result<Self, String> {
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end <= start
            || !(1.0 / (end - start)).is_finite()
        {
            return Err("fogstart and fogend need finite values, 0 ≤ start < end and a finite inverse range".into());
        }
        if !max_density.is_finite() || !(0.0..=1.0).contains(&max_density) {
            return Err("fogmaxdensity must be finite and between 0 and 1".into());
        }
        Ok(Self {
            color,
            start,
            end,
            max_density,
            enabled,
        })
    }

    pub fn color(self) -> [u8; 3] {
        self.color
    }
    pub fn start(self) -> f32 {
        self.start
    }
    pub fn end(self) -> f32 {
        self.end
    }
    pub fn max_density(self) -> f32 {
        self.max_density
    }
    pub fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn uniforms(self) -> ([f32; 4], [f32; 4]) {
        let color = self.color.map(|value| {
            let value = f32::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        (
            [
                color[0],
                color[1],
                color[2],
                if self.enabled { self.max_density } else { 0.0 },
            ],
            [self.start, 1.0 / (self.end - self.start), 0.0, 0.0],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_guards_nonfinite_ranges_density_and_encodes_color() {
        for (start, end, density) in [
            (f32::NAN, 100.0, 1.0),
            (0.0, f32::INFINITY, 1.0),
            (0.0, f32::from_bits(1), 1.0),
            (-1.0, 100.0, 1.0),
            (100.0, 100.0, 1.0),
            (0.0, 100.0, f32::NAN),
            (0.0, 100.0, -0.1),
            (0.0, 100.0, 1.1),
        ] {
            assert!(Fog::new([0; 3], start, end, density, true).is_err());
        }
        let fog = Fog::new([0, 128, 255], 100.0, 300.0, 0.5, true).unwrap();
        let (color, range) = fog.uniforms();
        assert_eq!(color[0], 0.0);
        assert!((color[1] - 0.21586).abs() < 0.00001);
        assert_eq!(color[2..], [1.0, 0.5]);
        assert_eq!(range[..2], [100.0, 0.005]);
        assert_eq!(
            Fog::new([255; 3], 0.0, 1.0, 1.0, false)
                .unwrap()
                .uniforms()
                .0[3],
            0.0
        );
    }
}
