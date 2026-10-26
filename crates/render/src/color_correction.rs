use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub const EDGE: u32 = 32;
pub const RAW_BYTES: usize = 98_304;
pub const RGBA_BYTES: usize = 131_072;
pub const UPLOAD_BYTES: usize = 262_144;
pub const MAX_TARGET_DIMENSION: u32 = 4096;
pub const MAX_TARGET_BYTES: u64 = 64 * 1024 * 1024;
pub const POLICY: &str = "One explicit Source .raw LUT: exactly 98304 packed RGB8 bytes, 32 cubed entries, red index fastest, then green, then blue. Trilinear lookup and weight 0..1 apply to the complete editor viewport's sRGB-encoded display bytes, including background, helpers and selection. No entity/volume evaluation, multiple-LUT composition, native HDR/tone-map parity or map/compiler mutation. One additional RGBA8 target, at most 4096 pixels per axis and 64 MiB; one 128 KiB GPU LUT and a conservative 256 KiB row-padded transient LUT upload, excluding driver overhead. Source failure invalidates the LUT until an explicit successful load. GPU or target failure is explicit; the desktop shows an ungraded frame and render_view fails.";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct Lut {
    pub(crate) id: u64,
    rgb: Vec<u8>,
}

impl Lut {
    pub fn from_raw(rgb: Vec<u8>) -> Result<Self, String> {
        if rgb.len() != RAW_BYTES {
            return Err(format!(
                "Source .raw LUT requires exactly {RAW_BYTES} RGB8 bytes; got {}",
                rgb.len()
            ));
        }
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            rgb,
        })
    }

    pub fn raw(&self) -> &[u8] {
        &self.rgb
    }

    pub fn identity(&self) -> u64 {
        self.id
    }

    pub(crate) fn rgba(&self) -> Result<Vec<u8>, String> {
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(RGBA_BYTES)
            .map_err(|error| format!("Could not allocate color-correction LUT upload: {error}"))?;
        for rgb in self.rgb.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], u8::MAX]);
        }
        Ok(rgba)
    }
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub lut: Arc<Lut>,
    pub weight: f32,
}

impl Preview {
    pub fn validate(&self) -> Result<(), String> {
        validate_weight(self.weight)
    }
}

pub fn validate_weight(weight: f32) -> Result<(), String> {
    if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
        return Err("Color-correction weight must be finite and between 0 and 1".into());
    }
    Ok(())
}

pub fn target_bytes(size: (u32, u32)) -> Result<u64, String> {
    if size.0 == 0 || size.1 == 0 || size.0 > MAX_TARGET_DIMENSION || size.1 > MAX_TARGET_DIMENSION
    {
        return Err(format!(
            "Color-correction target requires 1..{MAX_TARGET_DIMENSION} pixels per axis and at most {MAX_TARGET_BYTES} bytes"
        ));
    }
    Ok(u64::from(size.0) * u64::from(size.1) * 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_raw_entries_keep_red_fastest_rgb_and_distinct_cache_identity() {
        let mut raw = vec![0; RAW_BYTES];
        raw[3..6].copy_from_slice(&[17, 29, 43]);
        raw[96..99].copy_from_slice(&[61, 73, 89]);
        raw[3072..3075].copy_from_slice(&[101, 127, 149]);
        let lut = Lut::from_raw(raw.clone()).unwrap();
        let rgba = lut.rgba().unwrap();
        assert_eq!(&rgba[4..8], &[17, 29, 43, 255]);
        assert_eq!(&rgba[128..132], &[61, 73, 89, 255]);
        assert_eq!(&rgba[4096..4100], &[101, 127, 149, 255]);
        assert_ne!(lut.identity(), Lut::from_raw(raw).unwrap().identity());
        assert!(Lut::from_raw(vec![0; RAW_BYTES - 1]).is_err());
        assert!(Lut::from_raw(vec![0; RAW_BYTES + 1]).is_err());
        assert!(validate_weight(f32::NAN).is_err());
        assert!(validate_weight(-0.01).is_err());
        assert!(validate_weight(1.01).is_err());
        assert_eq!(target_bytes((4096, 4096)).unwrap(), 67_108_864);
        assert!(target_bytes((4097, 1)).is_err());
        assert!(target_bytes((0, 1)).is_err());
        assert!(target_bytes((u32::MAX, u32::MAX)).is_err());
    }
}
