pub use map_core::fog::Fog;

pub(crate) fn uniforms(fog: Fog) -> ([f32; 4], [f32; 4]) {
    let color = fog.color().map(|value| {
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
            if fog.enabled() {
                fog.max_density()
            } else {
                0.0
            },
        ],
        [fog.start(), 1.0 / (fog.end() - fog.start()), 0.0, 0.0],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_uniforms_encode_color_range_and_disabled_density() {
        let fog = Fog::new([0, 128, 255], 100.0, 300.0, 0.5, true).unwrap();
        let (color, range) = uniforms(fog);
        assert_eq!(color[0], 0.0);
        assert!((color[1] - 0.21586).abs() < 0.00001);
        assert_eq!(color[2..], [1.0, 0.5]);
        assert_eq!(range[..2], [100.0, 0.005]);
        assert_eq!(
            uniforms(Fog::new([255; 3], 0.0, 1.0, 1.0, false).unwrap()).0[3],
            0.0
        );
    }
}
