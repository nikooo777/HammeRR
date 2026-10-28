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
