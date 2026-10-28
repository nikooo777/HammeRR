use source_formats::vtf::{ImageFormat, Vtf};

pub fn validate_hdr_texture(texture: &Vtf) -> Result<(), String> {
    if !matches!(
        texture.format,
        ImageFormat::Rgba16161616F | ImageFormat::Rgba16161616
    ) {
        return Err("HDR preview requires RGBA16F or RGBA16".into());
    }
    if texture.format == ImageFormat::Rgba16161616F {
        for mip in 0..texture.mip_count {
            for pixel in texture
                .image_data(mip, 0, 0)
                .map_err(|error| error.to_string())?
                .as_chunks::<8>()
                .0
            {
                for (channel, pair) in pixel.as_chunks::<2>().0.iter().enumerate() {
                    let value =
                        half::f16::from_bits(u16::from_le_bytes([pair[0], pair[1]])).to_f32();
                    if !value.is_finite() || channel < 3 && value < 0.0 {
                        return Err(format!(
                            "HDR mip {mip} contains nonfinite components or negative RGB radiance"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
pub fn hdr_mip_bytes(texture: &Vtf, mip: u8) -> Result<std::borrow::Cow<'_, [u8]>, String> {
    let bytes = texture
        .image_data(mip, 0, 0)
        .map_err(|error| error.to_string())?;
    match texture.format {
        ImageFormat::Rgba16161616F => Ok(std::borrow::Cow::Borrowed(bytes)),
        ImageFormat::Rgba16161616 => {
            let mut converted = Vec::with_capacity(bytes.len());
            for pixel in bytes.as_chunks::<8>().0 {
                for (channel, pair) in pixel.as_chunks::<2>().0.iter().enumerate() {
                    let value = f32::from(u16::from_le_bytes([pair[0], pair[1]])) / 65535.0;
                    let value = if channel < 3 { value * 16.0 } else { value };
                    converted
                        .extend_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
                }
            }
            Ok(std::borrow::Cow::Owned(converted))
        }
        _ => Err("HDR preview requires RGBA16F or RGBA16".into()),
    }
}
