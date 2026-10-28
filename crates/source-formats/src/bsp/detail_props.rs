use std::ffi::CStr;
use std::io;

use bytemuck::{Pod, Zeroable};

use super::{Bsp, GameLump, Leaf, RgbExp32, invalid};

pub const DETAIL_PROP_NAME_BYTES: usize = 128;
pub const MAX_DETAIL_PROP_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DETAIL_PROP_MODELS: usize = 1024;
pub const MAX_DETAIL_PROPS: usize = 16_384;
const MAX_SPRITES: usize = 4096;
pub const MAX_DETAIL_PROP_LIGHTING: usize = 65_536;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct DetailSprite {
    pub upper_left: [f32; 2],
    pub lower_right: [f32; 2],
    pub texture_upper_left: [f32; 2],
    pub texture_lower_right: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct DetailProp {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub model: u16,
    pub leaf: u16,
    pub lighting: RgbExp32,
    pub light_styles: u32,
    pub light_style_count: u8,
    pub sway: u8,
    pub shape_angle: u8,
    pub shape_size: u8,
    pub orientation: u8,
    pub padding: [u8; 3],
    pub kind: u8,
    pub padding2: [u8; 3],
    pub scale: f32,
}

impl Default for DetailProp {
    fn default() -> Self {
        Self {
            lighting: RgbExp32 {
                rgb: [255; 3],
                exponent: 0,
            },
            scale: 1.0,
            ..Self::zeroed()
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct DetailPropLighting {
    pub lighting: RgbExp32,
    pub style: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailProps {
    pub models: Vec<[u8; DETAIL_PROP_NAME_BYTES]>,
    pub sprites: Vec<DetailSprite>,
    pub props: Vec<DetailProp>,
}

fn records<T: Pod>(data: &mut &[u8], maximum: usize) -> io::Result<Vec<T>> {
    let (raw, remaining) = data
        .split_at_checked(4)
        .ok_or_else(|| invalid("truncated detail-prop count"))?;
    let count = i32::from_le_bytes(raw.try_into().unwrap());
    if count < 0 || count as usize > maximum {
        return Err(invalid("detail-prop count exceeds supported bounds"));
    }
    let bytes = count as usize * size_of::<T>();
    let (raw, remaining) = remaining
        .split_at_checked(bytes)
        .ok_or_else(|| invalid("truncated detail-prop records"))?;
    *data = remaining;
    Ok(bytemuck::pod_collect_to_vec(raw))
}

fn append<T: Pod>(data: &mut Vec<u8>, records: &[T]) {
    data.extend_from_slice(&(records.len() as i32).to_le_bytes());
    data.extend_from_slice(bytemuck::cast_slice(records));
}

impl DetailProps {
    pub fn sprite_material_name(name: Option<&str>) -> &str {
        name.filter(|name| !name.is_empty())
            .unwrap_or("detail/detailsprites")
    }

    pub fn model_record(name: &str) -> io::Result<[u8; DETAIL_PROP_NAME_BYTES]> {
        super::StaticProps::model_record(name)
    }

    pub fn model_name(&self, index: usize) -> io::Result<&str> {
        let raw = self
            .models
            .get(index)
            .ok_or_else(|| invalid("detail-prop model index is invalid"))?;
        let name = CStr::from_bytes_until_nul(raw)
            .map_err(|_| invalid("detail-prop model name is not terminated"))?
            .to_str()
            .map_err(|_| invalid("detail-prop model name is not UTF-8"))?;
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(invalid(
                "detail-prop model name is empty or contains controls",
            ));
        }
        Ok(name)
    }

    pub fn validate(&self, leaf_count: usize) -> io::Result<()> {
        if self.models.len() > MAX_DETAIL_PROP_MODELS
            || self.sprites.len() > MAX_SPRITES
            || self.props.len() > MAX_DETAIL_PROPS
        {
            return Err(invalid("detail-prop count exceeds supported bounds"));
        }
        for index in 0..self.models.len() {
            self.model_name(index)?;
        }
        if self
            .sprites
            .iter()
            .flat_map(|sprite| {
                bytemuck::cast_slice::<DetailSprite, f32>(std::slice::from_ref(sprite))
            })
            .any(|value| !value.is_finite())
        {
            return Err(invalid("detail-prop sprite coordinates must be finite"));
        }
        for prop in &self.props {
            let models = if prop.kind == 0 {
                self.models.len()
            } else {
                self.sprites.len()
            };
            if prop.kind > 3
                || usize::from(prop.model) >= models
                || usize::from(prop.leaf) >= leaf_count
                || prop.orientation > 2
            {
                return Err(invalid(
                    "detail-prop type, dictionary, leaf or orientation is invalid",
                ));
            }
            if prop
                .origin
                .into_iter()
                .chain(prop.angles)
                .any(|value| !value.is_finite())
                || prop.kind != 0 && !prop.scale.is_finite()
            {
                return Err(invalid("detail-prop numeric fields must be finite"));
            }
            if prop.light_style_count != 0
                && u64::from(prop.light_styles) + u64::from(prop.light_style_count)
                    > MAX_DETAIL_PROP_LIGHTING as u64
            {
                return Err(invalid(
                    "detail-prop light-style range exceeds supported bounds",
                ));
            }
        }
        if self
            .props
            .windows(2)
            .any(|pair| pair[0].leaf > pair[1].leaf)
        {
            return Err(invalid("detail props must be ordered by leaf"));
        }
        Ok(())
    }

    pub fn parse(version: u16, mut data: &[u8], leaf_count: usize) -> io::Result<Self> {
        if version != 4 || data.len() > MAX_DETAIL_PROP_BYTES {
            return Err(invalid("unsupported detail-prop version or byte count"));
        }
        let result = Self {
            models: records(&mut data, MAX_DETAIL_PROP_MODELS)?,
            sprites: records(&mut data, MAX_SPRITES)?,
            props: records(&mut data, MAX_DETAIL_PROPS)?,
        };
        if !data.is_empty() {
            return Err(invalid("detail-prop payload has trailing bytes"));
        }
        result.validate(leaf_count)?;
        Ok(result)
    }

    pub fn to_bytes(&self, leaf_count: usize) -> io::Result<Vec<u8>> {
        self.validate(leaf_count)?;
        let mut data = Vec::new();
        append(&mut data, &self.models);
        append(&mut data, &self.sprites);
        append(&mut data, &self.props);
        if data.len() > MAX_DETAIL_PROP_BYTES {
            return Err(invalid("detail-prop payload exceeds supported bounds"));
        }
        Ok(data)
    }

    pub fn validate_lighting(
        &self,
        ldr: Option<&[DetailPropLighting]>,
        hdr: Option<&[DetailPropLighting]>,
    ) -> io::Result<()> {
        let total = ldr
            .map_or(0, <[DetailPropLighting]>::len)
            .checked_add(hdr.map_or(0, <[DetailPropLighting]>::len));
        if self.props.len() > MAX_DETAIL_PROPS
            || total.is_none_or(|count| count > MAX_DETAIL_PROP_LIGHTING)
            || ldr
                .into_iter()
                .chain(hdr)
                .flatten()
                .any(|record| record.style > 63)
            || ldr.zip(hdr).is_some_and(|(ldr, hdr)| {
                hdr.len() != ldr.len() || hdr.iter().zip(ldr).any(|(a, b)| a.style != b.style)
            })
        {
            return Err(invalid(
                "detail-prop lighting exceeds its total bound or channel style layout differs",
            ));
        }
        for prop in &self.props {
            if prop.light_style_count > 64 {
                return Err(invalid(
                    "detail-prop lighting range exceeds its bounded style array",
                ));
            }
            if prop.light_style_count == 0 {
                continue;
            }
            let start = prop.light_styles as usize;
            let end = start
                .checked_add(usize::from(prop.light_style_count))
                .filter(|&end| end <= MAX_DETAIL_PROP_LIGHTING)
                .ok_or_else(|| {
                    invalid("detail-prop lighting range exceeds its bounded style array")
                })?;
            for records in ldr.into_iter().chain(hdr) {
                let active = records.get(start..end).ok_or_else(|| {
                    invalid("detail-prop light-style range exceeds its lighting lump")
                })?;
                if active.windows(2).any(|pair| pair[0].style >= pair[1].style) {
                    return Err(invalid(
                        "detail-prop receiver styles must be distinct and increasing",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Bsp {
    fn check_detail_prop_lighting(&self, props: &DetailProps) -> io::Result<()> {
        let ldr = self.detail_prop_lighting(false)?;
        let hdr = self.detail_prop_lighting(true)?;
        props.validate_lighting(ldr.as_deref(), hdr.as_deref())?;
        if ldr.is_none()
            && hdr.is_none()
            && props.props.iter().any(|prop| prop.light_style_count != 0)
        {
            return Err(invalid("detail-prop light styles require a lighting lump"));
        }
        Ok(())
    }

    pub fn detail_props(&self) -> io::Result<Option<DetailProps>> {
        let mut lumps = self.game_lumps.iter().filter(|lump| lump.id == *b"dprp");
        let Some(lump) = lumps.next() else {
            return Ok(None);
        };
        if lumps.next().is_some() || lump.flags != 0 {
            return Err(invalid("detail-prop game lump is duplicated or compressed"));
        }
        let props = DetailProps::parse(lump.version, &lump.data, self.records::<Leaf>()?.len())?;
        self.check_detail_prop_lighting(&props)?;
        Ok(Some(props))
    }

    pub fn set_detail_props(&mut self, props: &DetailProps) -> io::Result<()> {
        let data = props.to_bytes(self.records::<Leaf>()?.len())?;
        self.check_detail_prop_lighting(props)?;
        self.game_lumps.retain(|lump| lump.id != *b"dprp");
        self.game_lumps.push(GameLump {
            id: *b"dprp",
            flags: 0,
            version: 4,
            data,
        });
        Ok(())
    }

    pub fn set_detail_props_with_lighting(
        &mut self,
        props: &DetailProps,
        ldr: &[DetailPropLighting],
        hdr: Option<&[DetailPropLighting]>,
    ) -> io::Result<()> {
        props.validate_lighting(Some(ldr), hdr)?;
        let data = props.to_bytes(self.records::<Leaf>()?.len())?;
        let mut ldr_data = Vec::new();
        append(&mut ldr_data, ldr);
        let hdr_data = hdr.map(|records| {
            let mut data = Vec::new();
            append(&mut data, records);
            data
        });
        self.game_lumps
            .retain(|lump| ![*b"dprp", *b"dplt", *b"dplh"].contains(&lump.id));
        self.game_lumps.push(GameLump {
            id: *b"dprp",
            flags: 0,
            version: 4,
            data,
        });
        self.game_lumps.push(GameLump {
            id: *b"dplt",
            flags: 0,
            version: 0,
            data: ldr_data,
        });
        if let Some(data) = hdr_data {
            self.game_lumps.push(GameLump {
                id: *b"dplh",
                flags: 0,
                version: 0,
                data,
            });
        }
        Ok(())
    }

    pub fn detail_prop_lighting(&self, hdr: bool) -> io::Result<Option<Vec<DetailPropLighting>>> {
        let id = if hdr { *b"dplh" } else { *b"dplt" };
        let mut lumps = self.game_lumps.iter().filter(|lump| lump.id == id);
        let Some(lump) = lumps.next() else {
            return Ok(None);
        };
        if lumps.next().is_some()
            || lump.version != 0
            || lump.flags != 0
            || lump.data.len() > MAX_DETAIL_PROP_BYTES
        {
            return Err(invalid("unsupported detail-prop lighting game lump"));
        }
        let mut data = lump.data.as_slice();
        let result = records(&mut data, MAX_DETAIL_PROP_LIGHTING)?;
        if !data.is_empty() {
            return Err(invalid("detail-prop lighting payload has trailing bytes"));
        }
        Ok(Some(result))
    }
}
