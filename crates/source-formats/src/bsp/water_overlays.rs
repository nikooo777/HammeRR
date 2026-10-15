use std::io;

use bytemuck::{Pod, Zeroable};

use super::overlays::{MAX_TEXINFOS, bounded_data};
use super::{Bsp, Face, MAX_LIGHTING_FACES, Overlay, OverlayData, TexInfo, invalid, lump};

pub const MAX_WATER_OVERLAYS: usize = 1024;
pub const MAX_WATER_OVERLAY_FACES: usize = 256;
const RENDER_ORDER_MASK: u16 = 0xc000;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct WaterOverlay {
    pub id: i32,
    pub texinfo: i16,
    pub face_count_order: u16,
    pub faces: [i32; MAX_WATER_OVERLAY_FACES],
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub uv_points: [[f32; 3]; 4],
    pub origin: [f32; 3],
    pub basis_normal: [f32; 3],
}

impl Default for WaterOverlay {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl super::Record for WaterOverlay {
    const LUMP: usize = lump::WATEROVERLAYS;
}

impl WaterOverlay {
    pub fn face_count(&self) -> usize {
        usize::from(self.face_count_order & !RENDER_ORDER_MASK)
    }

    pub fn render_order(&self) -> u16 {
        self.face_count_order >> 14
    }

    pub fn with_face_count(mut self, count: usize) -> io::Result<Self> {
        if count > MAX_WATER_OVERLAY_FACES {
            return Err(invalid(
                "water overlay face count exceeds its 256-face budget",
            ));
        }
        self.face_count_order = (self.face_count_order & RENDER_ORDER_MASK) | count as u16;
        Ok(self)
    }

    pub fn with_render_order(mut self, order: u16) -> io::Result<Self> {
        if order > 3 {
            return Err(invalid(
                "water overlay render order must be between 0 and 3",
            ));
        }
        self.face_count_order = (self.face_count_order & !RENDER_ORDER_MASK) | (order << 14);
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaterOverlayData {
    pub overlays: Vec<WaterOverlay>,
}

impl WaterOverlayData {
    pub fn validate(&self, face_count: usize, texinfo_count: usize) -> io::Result<()> {
        if self.overlays.len() > MAX_WATER_OVERLAYS {
            return Err(invalid(
                "water overlay array exceeds its 1024-record allocation budget",
            ));
        }
        let mut projections = OverlayData::default();
        for overlay in &self.overlays {
            let count = overlay.face_count();
            if count > MAX_WATER_OVERLAY_FACES
                || overlay.faces[..count]
                    .iter()
                    .any(|&face| face < 0 || face as usize >= face_count)
            {
                return Err(invalid("water overlay active face references are invalid"));
            }
            projections.overlays.push(Overlay {
                id: overlay.id,
                texinfo: overlay.texinfo,
                face_count_order: overlay.face_count_order & RENDER_ORDER_MASK,
                u: overlay.u,
                v: overlay.v,
                uv_points: overlay.uv_points,
                origin: overlay.origin,
                basis_normal: overlay.basis_normal,
                ..Overlay::default()
            });
        }
        projections.validate(face_count, texinfo_count)
    }
}

impl Bsp {
    pub fn water_overlay_data(&self) -> io::Result<Option<WaterOverlayData>> {
        let overlays = bounded_data::<WaterOverlay>(self, lump::WATEROVERLAYS, MAX_WATER_OVERLAYS)?;
        if overlays.is_empty() {
            return Ok(None);
        }
        let faces = bounded_data::<Face>(self, lump::FACES, MAX_LIGHTING_FACES)?;
        let texinfos = bounded_data::<TexInfo>(self, lump::TEXINFO, MAX_TEXINFOS)?;
        let data = WaterOverlayData {
            overlays: bytemuck::pod_collect_to_vec(overlays),
        };
        data.validate(
            faces.len() / size_of::<Face>(),
            texinfos.len() / size_of::<TexInfo>(),
        )?;
        Ok(Some(data))
    }

    pub fn set_water_overlay_data(&mut self, data: &WaterOverlayData) -> io::Result<()> {
        let faces = bounded_data::<Face>(self, lump::FACES, MAX_LIGHTING_FACES)?;
        let texinfos = bounded_data::<TexInfo>(self, lump::TEXINFO, MAX_TEXINFOS)?;
        data.validate(
            faces.len() / size_of::<Face>(),
            texinfos.len() / size_of::<TexInfo>(),
        )?;
        self.set_records(&data.overlays);
        Ok(())
    }
}
