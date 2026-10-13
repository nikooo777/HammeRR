use std::io;

use bytemuck::{Pod, Zeroable};

use super::{Bsp, Face, Lump, Record, TexInfo, invalid, lump, surf};

pub const MAX_LIGHTING_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_WORLD_LIGHTS: usize = 8192;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct RgbExp32 {
    pub rgb: [u8; 3],
    pub exponent: i8,
}

impl RgbExp32 {
    /// Linear RGB on Source's unnormalized 0–255 scale.
    pub fn linear(self) -> [f64; 3] {
        let scale = 2.0_f64.powi(i32::from(self.exponent));
        self.rgb.map(|channel| f64::from(channel) * scale)
    }
}

impl Record for RgbExp32 {
    const LUMP: usize = lump::LIGHTING;
    const VERSION: i32 = 1;
}

pub mod world_light_kind {
    pub const SURFACE: i32 = 0;
    pub const POINT: i32 = 1;
    pub const SPOT: i32 = 2;
    pub const SKY: i32 = 3;
    pub const QUAKE: i32 = 4;
    pub const SKY_AMBIENT: i32 = 5;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct WorldLight {
    pub origin: [f32; 3],
    pub intensity: [f32; 3],
    pub normal: [f32; 3],
    pub cluster: i32,
    pub kind: i32,
    pub style: i32,
    pub stop_dot: f32,
    pub stop_dot2: f32,
    pub exponent: f32,
    pub radius: f32,
    pub constant_attenuation: f32,
    pub linear_attenuation: f32,
    pub quadratic_attenuation: f32,
    pub flags: i32,
    pub texinfo: i32,
    pub owner: i32,
}

impl Record for WorldLight {
    const LUMP: usize = lump::WORLDLIGHTS;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingKind {
    Ldr,
    Hdr,
}

impl LightingKind {
    fn samples(self) -> usize {
        match self {
            Self::Ldr => lump::LIGHTING,
            Self::Hdr => lump::LIGHTING_HDR,
        }
    }

    fn lights(self) -> usize {
        match self {
            Self::Ldr => lump::WORLDLIGHTS,
            Self::Hdr => lump::WORLDLIGHTS_HDR,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Lighting<'a> {
    samples: &'a [RgbExp32],
}

#[derive(Debug, Clone, Copy)]
pub struct FaceLighting<'a> {
    styles: [u8; 4],
    style_count: usize,
    dimensions: [usize; 2],
    maps: usize,
    averages: &'a [RgbExp32],
    samples: &'a [RgbExp32],
}

impl Bsp {
    pub fn lighting(&self, kind: LightingKind) -> io::Result<Option<Lighting<'_>>> {
        let index = kind.samples();
        let record = &self.lumps[index];
        if record.data.is_empty() {
            return Ok(None);
        }
        if record.version != 1 {
            return Err(invalid("unsupported lighting lump version"));
        }
        Lighting::parse(self.checked(index, size_of::<RgbExp32>())?).map(Some)
    }

    pub fn world_lights(&self, kind: LightingKind) -> io::Result<Vec<WorldLight>> {
        let index = kind.lights();
        let record = &self.lumps[index];
        if !record.data.is_empty() && record.version != 0 {
            return Err(invalid("unsupported world-light lump version"));
        }
        let bytes = self.checked(index, size_of::<WorldLight>())?;
        if bytes.len() / size_of::<WorldLight>() > MAX_WORLD_LIGHTS {
            return Err(invalid("world-light count exceeds Source limits"));
        }
        Ok(bytemuck::pod_collect_to_vec(bytes))
    }

    pub fn set_lighting(&mut self, kind: LightingKind, samples: &[RgbExp32]) -> io::Result<()> {
        if samples.len() > MAX_LIGHTING_BYTES / size_of::<RgbExp32>() {
            return Err(invalid("lighting data exceeds Source limits"));
        }
        self.lumps[kind.samples()] = Lump {
            version: 1,
            fourcc: [0; 4],
            data: bytemuck::cast_slice(samples).to_vec(),
        };
        Ok(())
    }

    pub fn set_world_lights(
        &mut self,
        kind: LightingKind,
        lights: &[WorldLight],
    ) -> io::Result<()> {
        if lights.len() > MAX_WORLD_LIGHTS {
            return Err(invalid("world-light count exceeds Source limits"));
        }
        self.lumps[kind.lights()] = Lump {
            version: 0,
            fourcc: [0; 4],
            data: bytemuck::cast_slice(lights).to_vec(),
        };
        Ok(())
    }
}

impl<'a> Lighting<'a> {
    pub fn parse(bytes: &'a [u8]) -> io::Result<Self> {
        if bytes.len() > MAX_LIGHTING_BYTES || !bytes.len().is_multiple_of(size_of::<RgbExp32>()) {
            return Err(invalid("invalid lighting data length"));
        }
        Ok(Self {
            samples: bytemuck::cast_slice(bytes),
        })
    }

    pub fn samples(&self) -> &'a [RgbExp32] {
        self.samples
    }

    pub fn face(&self, face: &Face, info: &TexInfo) -> io::Result<Option<FaceLighting<'a>>> {
        if face.light_offset == -1 {
            return Ok(None);
        }
        let offset = usize::try_from(face.light_offset)
            .map_err(|_| invalid("negative face lighting offset"))?;
        if !offset.is_multiple_of(size_of::<RgbExp32>()) {
            return Err(invalid("unaligned face lighting offset"));
        }
        let style_count = face
            .styles
            .iter()
            .position(|&style| style == 255)
            .unwrap_or(4);
        if style_count == 0 || face.styles[style_count..].iter().any(|&style| style != 255) {
            return Err(invalid("invalid face lighting styles"));
        }
        let mut dimensions = [0; 2];
        for (dimension, extent) in dimensions.iter_mut().zip(face.lightmap_size) {
            *dimension = usize::try_from(extent)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| invalid("invalid lightmap extent"))?;
        }
        let maps = if info.flags & surf::BUMPLIGHT == 0 {
            1
        } else {
            4
        };
        let count = dimensions[0]
            .checked_mul(dimensions[1])
            .and_then(|value| value.checked_mul(maps))
            .and_then(|value| value.checked_mul(style_count))
            .ok_or_else(|| invalid("face lighting sample count overflow"))?;
        if count > MAX_LIGHTING_BYTES / size_of::<RgbExp32>() {
            return Err(invalid("face lighting exceeds Source limits"));
        }
        let offset = offset / size_of::<RgbExp32>();
        let average_start = offset
            .checked_sub(style_count)
            .ok_or_else(|| invalid("face average colors precede the lighting data"))?;
        let end = offset
            .checked_add(count)
            .ok_or_else(|| invalid("face lighting range overflow"))?;
        let averages = self
            .samples
            .get(average_start..offset)
            .ok_or_else(|| invalid("face average colors are outside the lighting data"))?;
        let samples = self
            .samples
            .get(offset..end)
            .ok_or_else(|| invalid("face samples are outside the lighting data"))?;
        Ok(Some(FaceLighting {
            styles: face.styles,
            style_count,
            dimensions,
            maps,
            averages,
            samples,
        }))
    }
}

impl<'a> FaceLighting<'a> {
    pub fn styles(&self) -> &[u8] {
        &self.styles[..self.style_count]
    }

    pub fn dimensions(&self) -> [usize; 2] {
        self.dimensions
    }

    pub fn maps(&self) -> usize {
        self.maps
    }

    pub fn average(&self, style_index: usize) -> Option<&'a RgbExp32> {
        let index = self.style_count.checked_sub(style_index.checked_add(1)?)?;
        self.averages.get(index)
    }

    pub fn samples(&self, style_index: usize, map_index: usize) -> Option<&'a [RgbExp32]> {
        if style_index >= self.style_count || map_index >= self.maps {
            return None;
        }
        let count = self.dimensions[0] * self.dimensions[1];
        let start = (style_index * self.maps + map_index) * count;
        self.samples.get(start..start + count)
    }
}
