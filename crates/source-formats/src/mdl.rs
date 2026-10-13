use std::io;

pub const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;
pub const STATIC_PROP_FLAG: u32 = 0x10;
const HEADER_BYTES: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelHeader {
    pub checksum: i32,
    pub flags: u32,
    pub hull_mins: [f32; 3],
    pub hull_maxs: [f32; 3],
    pub render_mins: [f32; 3],
    pub render_maxs: [f32; 3],
    pub illumination: [f32; 3],
    pub skins: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn integer(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn vector(data: &[u8], offset: usize) -> [f32; 3] {
    std::array::from_fn(|i| {
        f32::from_le_bytes(data[offset + i * 4..offset + i * 4 + 4].try_into().unwrap())
    })
}

impl ModelHeader {
    pub fn parse(data: &[u8]) -> io::Result<Self> {
        if data.len() < HEADER_BYTES || data.len() > MAX_MODEL_BYTES || &data[..4] != b"IDST" {
            return Err(invalid("model header size or IDST magic is invalid"));
        }
        if ![44, 48].contains(&integer(data, 4)) {
            return Err(invalid("only Source MDL versions 44 and 48 are supported"));
        }
        if integer(data, 76) as usize != data.len() {
            return Err(invalid("model declared length does not match its file"));
        }
        let skins = integer(data, 224);
        if !(1..=65_535).contains(&skins) {
            return Err(invalid("model skin-family count exceeds supported bounds"));
        }
        let result = Self {
            checksum: integer(data, 8),
            flags: integer(data, 152) as u32,
            hull_mins: vector(data, 104),
            hull_maxs: vector(data, 116),
            render_mins: vector(data, 128),
            render_maxs: vector(data, 140),
            illumination: vector(data, 92),
            skins: skins as usize,
        };
        if result.illumination.iter().any(|v| !v.is_finite()) {
            return Err(invalid("model illumination position must be finite"));
        }
        for (mins, maxs) in [
            (result.hull_mins, result.hull_maxs),
            (result.render_mins, result.render_maxs),
        ] {
            if (0..3).any(|i| !mins[i].is_finite() || !maxs[i].is_finite() || mins[i] > maxs[i]) {
                return Err(invalid("model bounds must be finite and ordered"));
            }
        }
        Ok(result)
    }

    pub fn is_static_prop(self) -> bool {
        self.flags & STATIC_PROP_FLAG != 0
    }
}

pub fn validate_physics(data: &[u8], checksum: i32) -> io::Result<usize> {
    if data.len() < 16 || data.len() > crate::bsp::MAX_PHYSICS_BYTES {
        return Err(invalid("model physics file size is invalid"));
    }
    let count = integer(data, 8);
    if integer(data, 0) != 16
        || integer(data, 4) != 0
        || integer(data, 12) != checksum
        || !(1..=1024).contains(&count)
    {
        return Err(invalid("model physics header/count/checksum is invalid"));
    }
    let mut payload = &data[16..];
    for _ in 0..count {
        let (size, tail) = payload
            .split_at_checked(4)
            .ok_or_else(|| invalid("model physics solid length is truncated"))?;
        let size = i32::from_le_bytes(size.try_into().unwrap());
        if size < 0 || size as usize > crate::bsp::MAX_COLLISION_BYTES {
            return Err(invalid("model physics solid size is invalid"));
        }
        let (solid, tail) = tail
            .split_at_checked(size as usize)
            .ok_or_else(|| invalid("model physics solid is truncated"))?;
        if !matches!(
            crate::bsp::PhysicsSolid::parse(solid)?.header(),
            crate::bsp::CollisionHeader::Convex { .. }
        ) {
            return Err(invalid("model physics needs a supported convex solid"));
        }
        payload = tail;
    }
    if payload.is_empty()
        || payload.len() > crate::bsp::MAX_PHYSICS_KEY_BYTES
        || payload.last() != Some(&0)
        || payload[..payload.len() - 1].contains(&0)
    {
        return Err(invalid(
            "model physics keys need bounded, singly terminated text",
        ));
    }
    Ok(count as usize)
}
