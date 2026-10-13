use std::collections::BTreeSet;
use std::io;

use bytemuck::{Pod, Zeroable};

use super::{Bsp, Face, TexInfo, invalid, lump};

pub const MAX_OVERLAYS: usize = 1024;
pub const MAX_OVERLAY_FACES: usize = 64;
const MAX_FACES: usize = 65_536;
const MAX_TEXINFOS: usize = 32_768;
const RENDER_ORDER_MASK: u16 = 0xc000;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Overlay {
    pub id: i32,
    pub texinfo: i16,
    pub face_count_order: u16,
    pub faces: [i32; MAX_OVERLAY_FACES],
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub uv_points: [[f32; 3]; 4],
    pub origin: [f32; 3],
    pub basis_normal: [f32; 3],
}

impl Default for Overlay {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl Overlay {
    pub fn face_count(&self) -> usize {
        usize::from(self.face_count_order & !RENDER_ORDER_MASK)
    }

    pub fn render_order(&self) -> u16 {
        self.face_count_order >> 14
    }

    pub fn with_face_count(mut self, count: usize) -> io::Result<Self> {
        if count > MAX_OVERLAY_FACES {
            return Err(invalid("overlay face count exceeds 64"));
        }
        self.face_count_order = (self.face_count_order & RENDER_ORDER_MASK) | count as u16;
        Ok(self)
    }

    pub fn with_render_order(mut self, order: u16) -> io::Result<Self> {
        if order > 3 {
            return Err(invalid("overlay render order must be between 0 and 3"));
        }
        self.face_count_order = (self.face_count_order & !RENDER_ORDER_MASK) | (order << 14);
        Ok(self)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct OverlayFade {
    pub min_distance_squared: f32,
    pub max_distance_squared: f32,
}

impl super::Record for Overlay {
    const LUMP: usize = lump::OVERLAYS;
}

impl super::Record for OverlayFade {
    const LUMP: usize = lump::OVERLAY_FADES;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverlayData {
    pub overlays: Vec<Overlay>,
    pub fades: Vec<OverlayFade>,
}

impl OverlayData {
    pub fn validate(&self, face_count: usize, texinfo_count: usize) -> io::Result<()> {
        if self.overlays.len() > MAX_OVERLAYS || self.fades.len() > MAX_OVERLAYS {
            return Err(invalid(format!(
                "overlay arrays exceed their {MAX_OVERLAYS}-record allocation cap"
            )));
        }
        if !self.fades.is_empty() && self.fades.len() != self.overlays.len() {
            return Err(invalid("overlay fade count must match the overlay count"));
        }
        if face_count > MAX_FACES || texinfo_count > MAX_TEXINFOS {
            return Err(invalid("overlay reference arrays exceed bounded limits"));
        }
        let mut ids = BTreeSet::new();
        for overlay in &self.overlays {
            if overlay.id < 0 || !ids.insert(overlay.id) {
                return Err(invalid("overlay IDs must be unique and nonnegative"));
            }
            if overlay.texinfo < 0 || overlay.texinfo as usize >= texinfo_count {
                return Err(invalid("overlay texinfo is outside the texture-info array"));
            }
            let count = overlay.face_count();
            if count > MAX_OVERLAY_FACES
                || overlay.faces[..count]
                    .iter()
                    .any(|&face| face < 0 || face as usize >= face_count)
            {
                return Err(invalid(
                    "overlay face count or active face reference is invalid",
                ));
            }
            if overlay
                .u
                .iter()
                .chain(&overlay.v)
                .chain(overlay.uv_points.iter().flatten())
                .chain(&overlay.origin)
                .chain(&overlay.basis_normal)
                .any(|value| !value.is_finite())
            {
                return Err(invalid("overlay projection contains a non-finite value"));
            }
            let length_squared = overlay
                .basis_normal
                .iter()
                .map(|&value| f64::from(value).powi(2))
                .sum::<f64>();
            if (length_squared - 1.0).abs() > 0.01 {
                return Err(invalid("overlay basis normal must have unit length"));
            }
        }
        if self.fades.iter().any(|fade| {
            !fade.min_distance_squared.is_finite() || !fade.max_distance_squared.is_finite()
        }) {
            return Err(invalid("overlay fade contains a non-finite value"));
        }
        Ok(())
    }
}

fn bounded_data<T: Pod>(bsp: &Bsp, slot: usize, maximum: usize) -> io::Result<&[u8]> {
    let metadata = &bsp.lumps[slot];
    let version_ok = if slot == lump::FACES {
        [0, 1].contains(&metadata.version)
    } else {
        metadata.version == 0
    };
    if !version_ok || metadata.fourcc != [0; 4] || bsp.is_compressed(slot) {
        return Err(invalid(format!(
            "unsupported overlay-related lump {slot} metadata"
        )));
    }
    if metadata.data.len() / size_of::<T>() > maximum {
        return Err(invalid(format!(
            "overlay-related lump {slot} exceeds its allocation cap"
        )));
    }
    bsp.checked(slot, size_of::<T>())
}

impl Bsp {
    pub fn overlay_data(&self) -> io::Result<Option<OverlayData>> {
        let overlays = bounded_data::<Overlay>(self, lump::OVERLAYS, MAX_OVERLAYS)?;
        let fades = bounded_data::<OverlayFade>(self, lump::OVERLAY_FADES, MAX_OVERLAYS)?;
        if overlays.is_empty() && fades.is_empty() {
            return Ok(None);
        }
        let faces = bounded_data::<Face>(self, lump::FACES, MAX_FACES)?;
        let texinfos = bounded_data::<TexInfo>(self, lump::TEXINFO, MAX_TEXINFOS)?;
        let data = OverlayData {
            overlays: bytemuck::pod_collect_to_vec(overlays),
            fades: bytemuck::pod_collect_to_vec(fades),
        };
        data.validate(
            faces.len() / size_of::<Face>(),
            texinfos.len() / size_of::<TexInfo>(),
        )?;
        Ok(Some(data))
    }

    pub fn set_overlay_data(&mut self, data: &OverlayData) -> io::Result<()> {
        let faces = bounded_data::<Face>(self, lump::FACES, MAX_FACES)?;
        let texinfos = bounded_data::<TexInfo>(self, lump::TEXINFO, MAX_TEXINFOS)?;
        data.validate(
            faces.len() / size_of::<Face>(),
            texinfos.len() / size_of::<TexInfo>(),
        )?;
        self.set_records(&data.overlays);
        self.set_records(&data.fades);
        Ok(())
    }
}
