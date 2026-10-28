use std::ffi::CStr;
use std::io;

use super::{Bsp, GameLump, Leaf, invalid};

pub const MAX_STATIC_PROP_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_STATIC_PROPS: usize = 16_384;
pub const MAX_STATIC_PROP_MODELS: usize = 1024;
pub const MAX_STATIC_PROP_LEAVES: usize = 65_535;
pub const STATIC_PROP_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct StaticProp {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub model: u16,
    pub first_leaf: u16,
    pub leaf_count: u16,
    pub solid: u8,
    pub flags: u32,
    pub skin: i32,
    pub fade_min: f32,
    pub fade_max: f32,
    pub lighting_origin: [f32; 3],
    pub fade_scale: f32,
    pub dx_levels: [u16; 2],
    pub lightmap_size: [u16; 2],
    pub padding: u8,
}

impl Default for StaticProp {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            angles: [0.0; 3],
            model: 0,
            first_leaf: 0,
            leaf_count: 0,
            solid: 0,
            flags: 0,
            skin: 0,
            fade_min: -1.0,
            fade_max: 0.0,
            lighting_origin: [0.0; 3],
            fade_scale: 1.0,
            dx_levels: [0; 2],
            lightmap_size: [0; 2],
            padding: 0,
        }
    }
}

impl StaticProp {
    pub fn transform(&self) -> io::Result<glam::DAffine3> {
        if self
            .origin
            .into_iter()
            .chain(self.angles)
            .any(|value| !value.is_finite())
        {
            return Err(invalid("static-prop placement must be finite"));
        }
        let angles = self.angles.map(|value| f64::from(value).to_radians());
        let rotation =
            glam::DQuat::from_euler(glam::EulerRot::ZYX, angles[1], angles[0], angles[2]);
        let origin = glam::DVec3::from_array(self.origin.map(f64::from));
        let transform = glam::DAffine3::from_rotation_translation(rotation, origin);
        if !transform.is_finite() {
            return Err(invalid("static-prop transform must be finite"));
        }
        Ok(transform)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticProps {
    pub version: u16,
    pub models: Vec<[u8; STATIC_PROP_NAME_BYTES]>,
    pub leaves: Vec<u16>,
    pub props: Vec<StaticProp>,
}

fn stride(version: u16) -> io::Result<usize> {
    match version {
        4 => Ok(56),
        5 => Ok(60),
        6 => Ok(64),
        10 => Ok(72),
        _ => Err(invalid("unsupported static-prop version")),
    }
}

fn take<'a>(data: &mut &'a [u8], count: usize) -> io::Result<&'a [u8]> {
    let (head, tail) = data
        .split_at_checked(count)
        .ok_or_else(|| invalid("static-prop payload is truncated"))?;
    *data = tail;
    Ok(head)
}

fn count(data: &mut &[u8], maximum: usize) -> io::Result<usize> {
    let value = i32::from_le_bytes(take(data, 4)?.try_into().unwrap());
    if value < 0 || value as usize > maximum {
        return Err(invalid("static-prop count exceeds supported bounds"));
    }
    Ok(value as usize)
}

fn float(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn short(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn vector(data: &[u8], offset: usize) -> [f32; 3] {
    std::array::from_fn(|i| float(data, offset + i * 4))
}

impl StaticProps {
    pub fn model_name(&self, index: usize) -> io::Result<&str> {
        let raw = self
            .models
            .get(index)
            .ok_or_else(|| invalid("static-prop model index is invalid"))?;
        let name = CStr::from_bytes_until_nul(raw)
            .map_err(|_| invalid("static-prop model name is not terminated"))?
            .to_str()
            .map_err(|_| invalid("static-prop model name is not UTF-8"))?;
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(invalid(
                "static-prop model name is empty or contains control characters",
            ));
        }
        Ok(name)
    }

    pub fn model_record(name: &str) -> io::Result<[u8; STATIC_PROP_NAME_BYTES]> {
        if name.is_empty()
            || name.len() >= STATIC_PROP_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(invalid("static-prop model name exceeds supported bounds"));
        }
        let mut raw = [0; STATIC_PROP_NAME_BYTES];
        raw[..name.len()].copy_from_slice(name.as_bytes());
        Ok(raw)
    }

    pub fn parse(version: u16, mut data: &[u8], leaf_count: usize) -> io::Result<Self> {
        let size = stride(version)?;
        if data.len() > MAX_STATIC_PROP_BYTES {
            return Err(invalid("static-prop payload exceeds supported bounds"));
        }
        let model_count = count(&mut data, MAX_STATIC_PROP_MODELS)?;
        let models = take(&mut data, model_count * STATIC_PROP_NAME_BYTES)?
            .as_chunks::<STATIC_PROP_NAME_BYTES>()
            .0
            .to_vec();
        let leaves_count = count(&mut data, MAX_STATIC_PROP_LEAVES)?;
        let leaves = take(&mut data, leaves_count * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|raw| u16::from_le_bytes(*raw))
            .collect();
        let prop_count = count(&mut data, MAX_STATIC_PROPS)?;
        let raw_props = take(&mut data, prop_count * size)?;
        if !data.is_empty() {
            return Err(invalid("static-prop payload has trailing bytes"));
        }
        let props = raw_props
            .chunks_exact(size)
            .map(|raw| StaticProp {
                origin: vector(raw, 0),
                angles: vector(raw, 12),
                model: short(raw, 24),
                first_leaf: short(raw, 26),
                leaf_count: short(raw, 28),
                solid: raw[30],
                flags: if version == 10 {
                    u32::from_le_bytes(raw[64..68].try_into().unwrap())
                } else {
                    u32::from(raw[31])
                },
                skin: i32::from_le_bytes(raw[32..36].try_into().unwrap()),
                fade_min: float(raw, 36),
                fade_max: float(raw, 40),
                lighting_origin: vector(raw, 44),
                fade_scale: if version >= 5 { float(raw, 56) } else { 1.0 },
                dx_levels: if version >= 6 {
                    [short(raw, 60), short(raw, 62)]
                } else {
                    [0; 2]
                },
                lightmap_size: if version == 10 {
                    [short(raw, 68), short(raw, 70)]
                } else {
                    [0; 2]
                },
                padding: if version == 10 { raw[31] } else { 0 },
            })
            .collect();
        let result = Self {
            version,
            models,
            leaves,
            props,
        };
        result.validate(leaf_count)?;
        Ok(result)
    }

    pub fn validate(&self, leaf_count: usize) -> io::Result<()> {
        stride(self.version)?;
        if self.models.len() > MAX_STATIC_PROP_MODELS
            || self.leaves.len() > MAX_STATIC_PROP_LEAVES
            || self.props.len() > MAX_STATIC_PROPS
        {
            return Err(invalid("static-prop count exceeds supported bounds"));
        }
        for index in 0..self.models.len() {
            self.model_name(index)?;
        }
        if self
            .leaves
            .iter()
            .any(|&leaf| usize::from(leaf) >= leaf_count)
        {
            return Err(invalid("static-prop leaf index is invalid"));
        }
        for prop in &self.props {
            if usize::from(prop.model) >= self.models.len()
                || usize::from(prop.first_leaf) + usize::from(prop.leaf_count) > self.leaves.len()
            {
                return Err(invalid("static-prop model or leaf range is invalid"));
            }
            if prop
                .origin
                .into_iter()
                .chain(prop.angles)
                .chain([prop.fade_min, prop.fade_max, prop.fade_scale])
                .any(|v| !v.is_finite())
                || prop.flags & 2 != 0 && prop.lighting_origin.iter().any(|v| !v.is_finite())
            {
                return Err(invalid("static-prop numeric fields must be finite"));
            }
            if self.version < 10
                && (prop.flags > u8::MAX.into()
                    || prop.lightmap_size != [0; 2]
                    || prop.padding != 0)
                || self.version < 6 && prop.dx_levels != [0; 2]
                || self.version < 5 && prop.fade_scale != 1.0
            {
                return Err(invalid(
                    "static-prop fields do not fit the selected version",
                ));
            }
        }
        Ok(())
    }

    pub fn to_bytes(&self, leaf_count: usize) -> io::Result<Vec<u8>> {
        self.validate(leaf_count)?;
        let size = 12
            + self.models.len() * STATIC_PROP_NAME_BYTES
            + self.leaves.len() * 2
            + self.props.len() * stride(self.version)?;
        if size > MAX_STATIC_PROP_BYTES {
            return Err(invalid("static-prop payload exceeds supported bounds"));
        }
        let mut data = Vec::with_capacity(size);
        data.extend_from_slice(&(self.models.len() as i32).to_le_bytes());
        for model in &self.models {
            data.extend_from_slice(model);
        }
        data.extend_from_slice(&(self.leaves.len() as i32).to_le_bytes());
        for leaf in &self.leaves {
            data.extend_from_slice(&leaf.to_le_bytes());
        }
        data.extend_from_slice(&(self.props.len() as i32).to_le_bytes());
        for prop in &self.props {
            for value in prop.origin.into_iter().chain(prop.angles) {
                data.extend_from_slice(&value.to_le_bytes());
            }
            for value in [prop.model, prop.first_leaf, prop.leaf_count] {
                data.extend_from_slice(&value.to_le_bytes());
            }
            data.push(prop.solid);
            data.push(if self.version == 10 {
                prop.padding
            } else {
                prop.flags as u8
            });
            data.extend_from_slice(&prop.skin.to_le_bytes());
            for value in [prop.fade_min, prop.fade_max]
                .into_iter()
                .chain(prop.lighting_origin)
            {
                data.extend_from_slice(&value.to_le_bytes());
            }
            if self.version >= 5 {
                data.extend_from_slice(&prop.fade_scale.to_le_bytes());
            }
            if self.version >= 6 {
                for value in prop.dx_levels {
                    data.extend_from_slice(&value.to_le_bytes());
                }
            }
            if self.version == 10 {
                data.extend_from_slice(&prop.flags.to_le_bytes());
                for value in prop.lightmap_size {
                    data.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        Ok(data)
    }
}

impl Bsp {
    pub fn static_props(&self) -> io::Result<Option<StaticProps>> {
        let mut lumps = self.game_lumps.iter().filter(|lump| lump.id == *b"sprp");
        let Some(lump) = lumps.next() else {
            return Ok(None);
        };
        if lumps.next().is_some() || lump.flags != 0 {
            return Err(invalid("static-prop game lump is duplicated or compressed"));
        }
        StaticProps::parse(lump.version, &lump.data, self.records::<Leaf>()?.len()).map(Some)
    }

    pub fn set_static_props(&mut self, props: &StaticProps) -> io::Result<()> {
        let data = props.to_bytes(self.records::<Leaf>()?.len())?;
        self.game_lumps.retain(|lump| lump.id != *b"sprp");
        self.game_lumps.push(GameLump {
            id: *b"sprp",
            flags: 0,
            version: props.version,
            data,
        });
        Ok(())
    }
}
