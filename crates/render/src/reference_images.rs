use std::io::Cursor;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use source_formats::vtf::RgbaImage;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const DECODE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct Asset {
    pub sha256: String,
    pub image: RgbaImage,
}

#[derive(Clone, Debug)]
pub struct Plane {
    pub corners: [[f32; 3]; 4],
    pub center: [f32; 3],
    pub opacity: f32,
    pub flip: [bool; 2],
    pub views: [bool; 4],
    pub asset: Arc<Asset>,
}

pub fn decode_png(bytes: &[u8]) -> Result<Asset, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("Reference PNG exceeds 16 MiB".into());
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: DECODE_BYTES,
    });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("Could not decode reference PNG: {error}"))?;
    map_core::reference_images::validate_dimensions([reader.info().width, reader.info().height])?;
    if reader.info().animation_control.is_some() {
        return Err("Animated PNG references are unsupported".into());
    }
    let length = reader
        .output_buffer_size()
        .filter(|length| *length <= DECODE_BYTES)
        .ok_or("Reference PNG output exceeds the decode budget")?;
    let mut decoded = vec![0; length];
    let frame = reader
        .next_frame(&mut decoded)
        .map_err(|error| format!("Could not decode reference PNG pixels: {error}"))?;
    reader
        .finish()
        .map_err(|error| format!("Incomplete reference PNG: {error}"))?;
    let decoded = &decoded[..frame.buffer_size()];
    let mut pixels = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
    match frame.color_type {
        png::ColorType::Rgba => pixels.extend_from_slice(decoded),
        png::ColorType::Rgb => {
            for pixel in decoded.as_chunks::<3>().0 {
                pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], u8::MAX]);
            }
        }
        png::ColorType::Grayscale => {
            for gray in decoded {
                pixels.extend_from_slice(&[*gray, *gray, *gray, u8::MAX]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in decoded.as_chunks::<2>().0 {
                pixels.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => return Err("Reference PNG palette expansion failed".into()),
    }
    Ok(Asset {
        sha256: format!("{:x}", Sha256::digest(bytes)),
        image: RgbaImage {
            width: frame.width,
            height: frame.height,
            pixels,
        },
    })
}
