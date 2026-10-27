//! Valve Texture Format (VTF) files, versions 7.0 to 7.5.

const SIGNATURE: &[u8; 4] = b"VTF\0";
const FLAG_ENVMAP: u32 = 0x4000;
pub const FLAG_NOMIP: u32 = 0x100;
const RESOURCE_HIGH_RES: [u8; 3] = [0x30, 0, 0];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("not a VTF file")]
    NotVtf,
    #[error("unsupported VTF version {0}.{1}")]
    UnsupportedVersion(u32, u32),
    #[error("unsupported image format {0}")]
    UnsupportedFormat(i32),
    #[error("VTF data is truncated")]
    Truncated,
    #[error("invalid VTF header: {0}")]
    InvalidHeader(&'static str),
    #[error(
        "texture stores {stored} mip levels but native sampling requires {required}; provide a complete chain or set NOMIP"
    )]
    IncompleteMipChain { stored: u8, required: u8 },
    #[error("mip {mip}, frame {frame}, face {face} doesn't exist")]
    NoSuchImage { mip: u8, frame: u16, face: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Rgba8888,
    Abgr8888,
    Rgb888,
    Bgr888,
    Rgb565,
    I8,
    Ia88,
    A8,
    Rgb888Bluescreen,
    Bgr888Bluescreen,
    Argb8888,
    Bgra8888,
    Dxt1,
    Dxt3,
    Dxt5,
    Bgrx8888,
    Bgr565,
    Bgrx5551,
    Bgra4444,
    Dxt1OneBitAlpha,
    Bgra5551,
    Uv88,
    Uvwq8888,
    Rgba16161616F,
    Rgba16161616,
    Uvlx8888,
}

impl ImageFormat {
    fn from_id(id: i32) -> Result<Self, Error> {
        use ImageFormat::*;
        Ok(match id {
            0 => Rgba8888,
            1 => Abgr8888,
            2 => Rgb888,
            3 => Bgr888,
            4 => Rgb565,
            5 => I8,
            6 => Ia88,
            8 => A8,
            9 => Rgb888Bluescreen,
            10 => Bgr888Bluescreen,
            11 => Argb8888,
            12 => Bgra8888,
            13 => Dxt1,
            14 => Dxt3,
            15 => Dxt5,
            16 => Bgrx8888,
            17 => Bgr565,
            18 => Bgrx5551,
            19 => Bgra4444,
            20 => Dxt1OneBitAlpha,
            21 => Bgra5551,
            22 => Uv88,
            23 => Uvwq8888,
            24 => Rgba16161616F,
            25 => Rgba16161616,
            26 => Uvlx8888,
            other => return Err(Error::UnsupportedFormat(other)),
        })
    }

    /// Bytes per 4x4 block for block-compressed formats.
    fn block_bytes(self) -> Option<usize> {
        match self {
            Self::Dxt1 | Self::Dxt1OneBitAlpha => Some(8),
            Self::Dxt3 | Self::Dxt5 => Some(16),
            _ => None,
        }
    }

    fn pixel_bytes(self) -> usize {
        use ImageFormat::*;
        match self {
            I8 | A8 => 1,
            Ia88 | Rgb565 | Bgr565 | Bgrx5551 | Bgra4444 | Bgra5551 | Uv88 => 2,
            Rgb888 | Bgr888 | Rgb888Bluescreen | Bgr888Bluescreen => 3,
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 => 4,
            Rgba16161616F | Rgba16161616 => 8,
            Dxt1 | Dxt1OneBitAlpha | Dxt3 | Dxt5 => 0,
        }
    }

    pub fn image_size(self, width: u32, height: u32) -> usize {
        let (w, h) = (width.max(1) as usize, height.max(1) as usize);
        match self.block_bytes() {
            Some(block) => w.div_ceil(4) * h.div_ceil(4) * block,
            None => w * h * self.pixel_bytes(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Vtf {
    pub version: (u32, u32),
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub flags: u32,
    pub frames: u16,
    pub authored_frames: u16,
    pub authored_depth: Option<u16>,
    pub first_frame: u16,
    pub faces: u8,
    pub mip_count: u8,
    pub format: ImageFormat,
    pub reflectivity: [f32; 3],
    data: Vec<u8>,
    high_res_offset: usize,
}

/// A decoded image, 8-bit RGBA, row-major from the top-left corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        d.get(at..at + 2)
            .ok_or(Error::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        d.get(at..at + 4)
            .ok_or(Error::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn f32_at(d: &[u8], at: usize) -> Result<f32, Error> {
    u32_at(d, at).map(f32::from_bits)
}

fn full_mip_count(width: u32, height: u32, depth: u32) -> u8 {
    (u32::BITS - width.max(height).max(depth).leading_zeros()) as u8
}

pub fn validate_native_mips(dimensions: [u32; 3], flags: u32, stored: u8) -> Result<(), Error> {
    let required = full_mip_count(dimensions[0], dimensions[1], dimensions[2]);
    if flags & FLAG_NOMIP == 0 && stored < required {
        return Err(Error::IncompleteMipChain { stored, required });
    }
    Ok(())
}

impl Vtf {
    pub fn validate_native_mips(&self) -> Result<(), Error> {
        validate_native_mips(
            [self.width, self.height, self.depth],
            self.flags,
            self.mip_count,
        )
    }

    pub fn parse(data: Vec<u8>) -> Result<Self, Error> {
        if data.get(..4) != Some(SIGNATURE) {
            return Err(Error::NotVtf);
        }
        let version = (u32_at(&data, 4)?, u32_at(&data, 8)?);
        if version.0 != 7 || version.1 > 5 {
            return Err(Error::UnsupportedVersion(version.0, version.1));
        }
        let header_size = u32_at(&data, 12)? as usize;
        let width = u32::from(u16_at(&data, 16)?);
        let height = u32::from(u16_at(&data, 18)?);
        let flags = u32_at(&data, 20)?;
        let authored_frames = u16_at(&data, 24)?;
        let frames = authored_frames.max(1);
        let first_frame = u16_at(&data, 26)?;
        let reflectivity = [f32_at(&data, 32)?, f32_at(&data, 36)?, f32_at(&data, 40)?];
        let format = ImageFormat::from_id(u32_at(&data, 52)? as i32)?;
        let mip_count = *data.get(56).ok_or(Error::Truncated)?;
        let low_res_format = u32_at(&data, 57)? as i32;
        let low_res_width = u32::from(*data.get(61).ok_or(Error::Truncated)?);
        let low_res_height = u32::from(*data.get(62).ok_or(Error::Truncated)?);
        let authored_depth = if version.1 >= 2 {
            Some(u16_at(&data, 63)?)
        } else {
            None
        };
        let depth = u32::from(authored_depth.unwrap_or(1).max(1));

        let minimum_header = if version.1 >= 3 {
            80
        } else if version.1 >= 2 {
            65
        } else {
            63
        };
        if header_size < minimum_header || header_size > data.len() {
            return Err(Error::InvalidHeader("header size lies outside the file"));
        }
        let max_mips = full_mip_count(width, height, depth);
        if width == 0 || height == 0 || mip_count == 0 || mip_count > max_mips {
            return Err(Error::InvalidHeader("invalid dimensions or mip count"));
        }

        // Before 7.5, environment maps carried a seventh (sphere map) face
        // unless the first-frame field was -1.
        let faces = if flags & FLAG_ENVMAP == 0 {
            1
        } else if version.1 < 5 && first_frame != 0xffff {
            7
        } else {
            6
        };

        let high_res_offset = if version.1 >= 3 {
            let count = u32_at(&data, 68)? as usize;
            if count > (header_size - 80) / 8 {
                return Err(Error::InvalidHeader("resource count exceeds the header"));
            }
            (0..count)
                .find_map(|i| {
                    let at = 80 + i * 8;
                    let tag = data.get(at..at + 3)?;
                    (tag == RESOURCE_HIGH_RES).then(|| u32_at(&data, at + 4).ok())?
                })
                .ok_or(Error::Truncated)? as usize
        } else {
            let low_res_size = if low_res_format < 0 || low_res_width == 0 {
                0
            } else {
                ImageFormat::from_id(low_res_format)?.image_size(low_res_width, low_res_height)
            };
            header_size + low_res_size
        };

        let vtf = Self {
            version,
            width,
            height,
            depth,
            flags,
            frames,
            authored_frames,
            authored_depth,
            first_frame,
            faces,
            mip_count,
            format,
            reflectivity,
            data,
            high_res_offset,
        };
        let mut size = 0usize;
        for mip in 0..vtf.mip_count {
            let bytes = vtf
                .image_size(mip)
                .checked_mul(vtf.slices(mip))
                .and_then(|n| n.checked_mul(usize::from(vtf.frames)))
                .and_then(|n| n.checked_mul(usize::from(vtf.faces)))
                .ok_or(Error::InvalidHeader("image size overflow"))?;
            size = size
                .checked_add(bytes)
                .ok_or(Error::InvalidHeader("mip size overflow"))?;
        }
        if vtf.high_res_offset < header_size
            || vtf
                .high_res_offset
                .checked_add(size)
                .is_none_or(|end| end > vtf.data.len())
        {
            return Err(Error::Truncated);
        }
        Ok(vtf)
    }

    pub fn mip_size(&self, mip: u8) -> (u32, u32) {
        (
            (self.width.checked_shr(u32::from(mip)).unwrap_or(0)).max(1),
            (self.height.checked_shr(u32::from(mip)).unwrap_or(0)).max(1),
        )
    }

    fn slices(&self, mip: u8) -> usize {
        (self.depth >> mip).max(1) as usize
    }

    fn image_size(&self, mip: u8) -> usize {
        let (w, h) = self.mip_size(mip);
        self.format.image_size(w, h)
    }

    /// Bytes taken by one mip level across all frames, faces and slices.
    fn mip_bytes(&self, mip: u8) -> usize {
        usize::from(self.frames) * usize::from(self.faces) * self.slices(mip) * self.image_size(mip)
    }

    /// Offset into the high-res data of an image. Mips are stored from the
    /// smallest to the largest; each mip holds every frame, face and slice.
    fn offset_of(&self, mip: u8, frame: u16, face: u8) -> usize {
        let smaller: usize = (mip + 1..self.mip_count).map(|m| self.mip_bytes(m)).sum();
        let index = usize::from(frame) * usize::from(self.faces) + usize::from(face);
        smaller + index * self.slices(mip) * self.image_size(mip)
    }

    /// Raw bytes of one image (first depth slice), in `self.format`.
    pub fn image_data(&self, mip: u8, frame: u16, face: u8) -> Result<&[u8], Error> {
        if mip >= self.mip_count || frame >= self.frames || face >= self.faces {
            return Err(Error::NoSuchImage { mip, frame, face });
        }
        let start = self.high_res_offset + self.offset_of(mip, frame, face);
        let len = self.image_size(mip);
        self.data.get(start..start + len).ok_or(Error::Truncated)
    }

    /// The largest mip no bigger than `max_size` on either side.
    pub fn mip_for_size(&self, max_size: u32) -> u8 {
        (0..self.mip_count)
            .find(|&m| {
                let (w, h) = self.mip_size(m);
                w <= max_size && h <= max_size
            })
            .unwrap_or(self.mip_count - 1)
    }

    pub fn decode_rgba8(&self, mip: u8, frame: u16, face: u8) -> Result<RgbaImage, Error> {
        let data = self.image_data(mip, frame, face)?;
        let (width, height) = self.mip_size(mip);
        let pixels = match self.format {
            ImageFormat::Dxt1 | ImageFormat::Dxt1OneBitAlpha => {
                decode_blocks(data, width, height, 8, decode_bc1)
            }
            ImageFormat::Dxt3 => decode_blocks(data, width, height, 16, decode_bc2),
            ImageFormat::Dxt5 => decode_blocks(data, width, height, 16, decode_bc3),
            format => decode_pixels(format, data),
        };
        Ok(RgbaImage {
            width,
            height,
            pixels,
        })
    }
}

fn expand5(v: u16) -> u8 {
    ((v << 3) | (v >> 2)) as u8
}

fn expand6(v: u16) -> u8 {
    ((v << 2) | (v >> 4)) as u8
}

fn expand4(v: u16) -> u8 {
    (v * 17) as u8
}

fn rgb565(v: u16) -> [u8; 3] {
    [
        expand5(v >> 11),
        expand6((v >> 5) & 0x3f),
        expand5(v & 0x1f),
    ]
}

fn half_to_unorm8(bits: u16) -> u8 {
    let sign = bits >> 15;
    let exp = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    let value = match exp {
        0 => mantissa / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exp - 15),
    };
    if sign == 1 {
        0
    } else {
        (value.min(1.0) * 255.0).round() as u8
    }
}

fn decode_pixels(format: ImageFormat, data: &[u8]) -> Vec<u8> {
    use ImageFormat::*;
    let size = format.pixel_bytes();
    let mut out = Vec::with_capacity(data.len() / size * 4);
    for p in data.chunks_exact(size) {
        let le16 = || u16::from_le_bytes([p[0], p[1]]);
        let rgba: [u8; 4] = match format {
            Rgba8888 | Uvwq8888 | Uvlx8888 => [p[0], p[1], p[2], p[3]],
            Abgr8888 => [p[3], p[2], p[1], p[0]],
            Argb8888 => [p[1], p[2], p[3], p[0]],
            Bgra8888 => [p[2], p[1], p[0], p[3]],
            Bgrx8888 => [p[2], p[1], p[0], 255],
            Rgb888 => [p[0], p[1], p[2], 255],
            Bgr888 => [p[2], p[1], p[0], 255],
            Rgb888Bluescreen | Bgr888Bluescreen => {
                let [r, g, b] = if format == Rgb888Bluescreen {
                    [p[0], p[1], p[2]]
                } else {
                    [p[2], p[1], p[0]]
                };
                let alpha = if [r, g, b] == [0, 0, 255] { 0 } else { 255 };
                [r, g, b, alpha]
            }
            I8 => [p[0], p[0], p[0], 255],
            Ia88 => [p[0], p[0], p[0], p[1]],
            A8 => [255, 255, 255, p[0]],
            Uv88 => [p[0], p[1], 0, 255],
            Rgb565 => {
                let [b, g, r] = rgb565(le16());
                [r, g, b, 255]
            }
            Bgr565 => {
                let [r, g, b] = rgb565(le16());
                [r, g, b, 255]
            }
            Bgra4444 => {
                let v = le16();
                [
                    expand4((v >> 8) & 0xf),
                    expand4((v >> 4) & 0xf),
                    expand4(v & 0xf),
                    expand4(v >> 12),
                ]
            }
            Bgrx5551 | Bgra5551 => {
                let v = le16();
                let alpha = if format == Bgrx5551 || v >> 15 == 1 {
                    255
                } else {
                    0
                };
                [
                    expand5((v >> 10) & 0x1f),
                    expand5((v >> 5) & 0x1f),
                    expand5(v & 0x1f),
                    alpha,
                ]
            }
            Rgba16161616 => [p[1], p[3], p[5], p[7]],
            Rgba16161616F => {
                let half = |i: usize| half_to_unorm8(u16::from_le_bytes([p[i], p[i + 1]]));
                [half(0), half(2), half(4), half(6)]
            }
            Dxt1 | Dxt1OneBitAlpha | Dxt3 | Dxt5 => unreachable!("block formats decode separately"),
        };
        out.extend_from_slice(&rgba);
    }
    out
}

fn decode_blocks(
    data: &[u8],
    width: u32,
    height: u32,
    block_bytes: usize,
    decode: fn(&[u8], &mut [[u8; 4]; 16]),
) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let blocks_x = w.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];
    let mut texels = [[0u8; 4]; 16];
    for (i, block) in data.chunks_exact(block_bytes).enumerate() {
        let (bx, by) = (i % blocks_x * 4, i / blocks_x * 4);
        decode(block, &mut texels);
        for (t, texel) in texels.iter().enumerate() {
            let (x, y) = (bx + t % 4, by + t / 4);
            if x < w && y < h {
                let at = (y * w + x) * 4;
                out[at..at + 4].copy_from_slice(texel);
            }
        }
    }
    out
}

/// Decodes the color half of a BC1/2/3 block. In BC1 a block whose first
/// endpoint is not greater than the second uses 3 colors plus transparent.
fn decode_color_block(block: &[u8], allow_transparent: bool, out: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u16, wb: u16, div: u16| -> [u8; 4] {
        let ch = |i: usize| ((u16::from(a[i]) * wa + u16::from(b[i]) * wb) / div) as u8;
        [ch(0), ch(1), ch(2), 255]
    };
    let palette = if c0 > c1 || !allow_transparent {
        [mix(1, 0, 1), mix(0, 1, 1), mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [mix(1, 0, 1), mix(0, 1, 1), mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, texel) in out.iter_mut().enumerate() {
        *texel = palette[(indices >> (i * 2) & 3) as usize];
    }
}

fn decode_bc1(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_color_block(block, true, out);
}

fn decode_bc2(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_color_block(&block[8..], false, out);
    let alpha = u64::from_le_bytes(block[..8].try_into().unwrap());
    for (i, texel) in out.iter_mut().enumerate() {
        texel[3] = expand4((alpha >> (i * 4) & 0xf) as u16);
    }
}

fn decode_bc3(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_color_block(&block[8..], false, out);
    let (a0, a1) = (u16::from(block[0]), u16::from(block[1]));
    let palette: [u8; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            _ => (((8 - i as u16) * a0 + (i as u16 - 1) * a1) / 7) as u8,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            6 => 0,
            7 => 255,
            _ => (((6 - i as u16) * a0 + (i as u16 - 1) * a1) / 5) as u8,
        })
    };
    let mut bits = [0u8; 8];
    bits[..6].copy_from_slice(&block[2..8]);
    let indices = u64::from_le_bytes(bits);
    for (i, texel) in out.iter_mut().enumerate() {
        texel[3] = palette[(indices >> (i * 3) & 7) as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 7.2 header for a single-mip, single-frame texture with no thumbnail.
    fn header(width: u16, height: u16, format: i32) -> Vec<u8> {
        let mut h = vec![0u8; 80];
        h[..4].copy_from_slice(SIGNATURE);
        h[4..8].copy_from_slice(&7u32.to_le_bytes());
        h[8..12].copy_from_slice(&2u32.to_le_bytes());
        h[12..16].copy_from_slice(&80u32.to_le_bytes());
        h[16..18].copy_from_slice(&width.to_le_bytes());
        h[18..20].copy_from_slice(&height.to_le_bytes());
        h[24..26].copy_from_slice(&1u16.to_le_bytes());
        h[52..56].copy_from_slice(&format.to_le_bytes());
        h[56] = 1;
        h[57..61].copy_from_slice(&(-1i32).to_le_bytes());
        h[63..65].copy_from_slice(&1u16.to_le_bytes());
        h
    }

    #[test]
    fn decodes_bgr888() {
        let mut data = header(2, 1, 3);
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let image = Vtf::parse(data).unwrap().decode_rgba8(0, 0, 0).unwrap();
        assert_eq!(image.pixels, [3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn decodes_dxt1_with_transparency() {
        let mut data = header(4, 4, 13);
        // c0 = black <= c1 = white: 3-color mode. Indices: 0,1,2,3 repeated.
        data.extend_from_slice(&[0, 0, 0xff, 0xff, 0xe4, 0xe4, 0xe4, 0xe4]);
        let image = Vtf::parse(data).unwrap().decode_rgba8(0, 0, 0).unwrap();
        let first_row: Vec<_> = image.pixels.chunks(4).take(4).collect();
        assert_eq!(first_row[0], [0, 0, 0, 255]);
        assert_eq!(first_row[1], [255, 255, 255, 255]);
        assert_eq!(first_row[2], [127, 127, 127, 255]);
        assert_eq!(first_row[3], [0, 0, 0, 0]);
    }

    #[test]
    fn rejects_truncated_data() {
        let mut data = header(4, 4, 13);
        data.extend_from_slice(&[0; 4]);
        assert_eq!(Vtf::parse(data).unwrap_err(), Error::Truncated);
    }

    #[test]
    fn native_sampling_requires_stored_mips_unless_disabled() {
        for (width, height, depth, mips, required) in [
            (1, 1, 1_u16, 1, 1),
            (2, 1, 1, 1, 2),
            (64, 32, 1, 1, 7),
            (64, 32, 1, 6, 7),
            (64, 32, 1, 7, 7),
            (1, 1, 8, 1, 4),
        ] {
            let mut data = header(width, height, 0);
            data[56] = mips;
            data[63..65].copy_from_slice(&depth.to_le_bytes());
            for mip in (0..mips).rev() {
                let pixels = usize::from((width >> mip).max(1))
                    * usize::from((height >> mip).max(1))
                    * usize::from((depth >> mip).max(1));
                data.extend(std::iter::repeat_n(127, pixels * 4));
            }
            let texture = Vtf::parse(data.clone()).unwrap();
            let expected = if mips == required {
                Ok(())
            } else {
                Err(Error::IncompleteMipChain {
                    stored: mips,
                    required,
                })
            };
            assert_eq!(texture.validate_native_mips(), expected);
            data[20..24].copy_from_slice(&FLAG_NOMIP.to_le_bytes());
            let unfiltered = Vtf::parse(data).unwrap();
            assert_eq!(unfiltered.validate_native_mips(), Ok(()));
            assert_eq!(texture.image_data(0, 0, 0), unfiltered.image_data(0, 0, 0));
        }
    }
}
