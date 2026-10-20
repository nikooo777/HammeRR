use glam::DVec3;
use source_formats::vhv::Bgra8;

pub(super) fn encode(color: DVec3) -> Result<Bgra8, String> {
    let packed = super::super::encode_color(color)?;
    let scale = (2.0_f64.powi(i32::from(packed.exponent)) / 255.0) as f32;
    let levels = packed
        .rgb
        .map(|channel| f32::from(channel) * scale * 1024.0);
    if levels
        .iter()
        .any(|value| !value.is_finite() || *value >= i32::MAX as f32)
    {
        return Err("Baked prop color exceeds the native vertex-table integer range".into());
    }
    let channels = levels.map(|level| {
        let index = level.round_ties_even().clamp(0.0, 4095.0) as u32;
        let table = (f64::from(index) / 1024.0).powf(1.0 / f64::from(2.2_f32)) as f32;
        (table * 0.5).min(1.0)
    });
    let [red, green, blue] = channels.map(|value| (value.max(0.0) * 255.0).round_ties_even() as u8);
    Ok(Bgra8 {
        blue,
        green,
        red,
        alpha: 255,
    })
}
