use std::collections::BTreeMap;
use std::io;

use bytemuck::Pod;

use super::{Area, AreaPortal, Bsp, Leaf, Plane, invalid, lump};

pub const MAX_AREAS: usize = 256;
pub const MAX_AREA_PORTALS: usize = 1024;
// CS:S transmits 24 bytes of keyed portal state; key zero remains reserved.
pub const MAX_AREA_PORTAL_KEY: usize = 24 * 8 - 1;
pub const MAX_AREA_PORTAL_VERTICES: usize = 32;
pub const MAX_CLIP_PORTAL_VERTICES: usize = 128_000;
const MAX_PLANES: usize = 65_536;
const MAX_LEAVES: usize = 65_536;
type FragmentKey = (usize, usize, i32, Vec<[u32; 3]>);

fn canonical_ring(vertices: &[[f32; 3]]) -> Vec<[u32; 3]> {
    let points = vertices
        .iter()
        .map(|point| point.map(|value| if value == 0.0 { 0 } else { value.to_bits() }))
        .collect::<Vec<_>>();
    (0..points.len())
        .flat_map(|start| {
            [false, true].map(|reverse| {
                (0..points.len())
                    .map(|step| {
                        let index = if reverse {
                            (start + points.len() - step) % points.len()
                        } else {
                            (start + step) % points.len()
                        };
                        points[index]
                    })
                    .collect::<Vec<_>>()
            })
        })
        .min()
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct AreaData {
    pub areas: Vec<Area>,
    pub portals: Vec<AreaPortal>,
    pub clip_vertices: Vec<[f32; 3]>,
}

impl AreaData {
    pub fn validate(&self, plane_count: usize, leaves: &[Leaf]) -> io::Result<()> {
        if self.areas.is_empty() || self.areas.len() > MAX_AREAS {
            return Err(invalid("area count must be between 1 and 256"));
        }
        if self.portals.is_empty() || self.portals.len() > MAX_AREA_PORTALS {
            return Err(invalid("areaportal count must be between 1 and 1024"));
        }
        if self.portals[0] != AreaPortal::default() || self.areas[0].portal_count != 0 {
            return Err(invalid("area and portal zero must remain reserved"));
        }
        if self.clip_vertices.len() > MAX_CLIP_PORTAL_VERTICES
            || self
                .clip_vertices
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(invalid("clip portal vertices exceed finite native bounds"));
        }
        if plane_count > MAX_PLANES || leaves.len() > MAX_LEAVES {
            return Err(invalid("area reference arrays exceed native bounds"));
        }
        if leaves
            .iter()
            .any(|leaf| leaf.area() as usize >= self.areas.len())
        {
            return Err(invalid("leaf area is outside the area array"));
        }
        let mut owners = vec![None; self.portals.len()];
        for (owner, area) in self.areas.iter().enumerate() {
            let first = usize::try_from(area.first_portal)
                .map_err(|_| invalid("negative area portal offset"))?;
            let count = usize::try_from(area.portal_count)
                .map_err(|_| invalid("negative area portal count"))?;
            let end = first
                .checked_add(count)
                .filter(|&end| end <= self.portals.len())
                .ok_or_else(|| invalid("area portal range is outside the portal array"))?;
            for (portal, slot) in self.portals[first..end].iter().zip(&mut owners[first..end]) {
                if portal.portal_key == 0 || slot.replace(owner).is_some() {
                    return Err(invalid(
                        "area portal ranges overlap or contain the reserved portal",
                    ));
                }
            }
        }
        let mut keys: BTreeMap<u16, Vec<(usize, &AreaPortal)>> = BTreeMap::new();
        for (index, portal) in self.portals.iter().enumerate().skip(1) {
            let owner = owners[index].ok_or_else(|| invalid("areaportal has no owning area"))?;
            if usize::from(portal.portal_key) > MAX_AREA_PORTAL_KEY {
                return Err(invalid(format!(
                    "areaportal control key exceeds the CS:S limit of {MAX_AREA_PORTAL_KEY}"
                )));
            }
            if portal.portal_key == 0
                || portal.other_area == 0
                || usize::from(portal.other_area) >= self.areas.len()
                || usize::from(portal.other_area) == owner
            {
                return Err(invalid("areaportal key or other-area reference is invalid"));
            }
            let plane =
                usize::try_from(portal.plane).map_err(|_| invalid("negative areaportal plane"))?;
            if plane >= plane_count {
                return Err(invalid("areaportal plane is outside the plane array"));
            }
            let first = usize::from(portal.first_clip_vert);
            let count = usize::from(portal.clip_vert_count);
            if !(3..=MAX_AREA_PORTAL_VERTICES).contains(&count) {
                return Err(invalid(format!(
                    "areaportal clip polygons require 3–{MAX_AREA_PORTAL_VERTICES} vertices"
                )));
            }
            if first
                .checked_add(count)
                .is_none_or(|end| end > self.clip_vertices.len())
            {
                return Err(invalid(
                    "areaportal clip polygon is outside the vertex array",
                ));
            }
            keys.entry(portal.portal_key)
                .or_default()
                .push((owner, portal));
        }
        for entries in keys.values() {
            let mut planes = BTreeMap::new();
            let mut fragments: BTreeMap<FragmentKey, i32> = BTreeMap::new();
            for &(owner, portal) in entries {
                if planes
                    .insert(owner, portal.plane)
                    .is_some_and(|plane| plane != portal.plane)
                {
                    return Err(invalid(
                        "areaportal fragments in one area must share one directed plane",
                    ));
                }
                let other = usize::from(portal.other_area);
                let first = usize::from(portal.first_clip_vert);
                let end = first + usize::from(portal.clip_vert_count);
                let ring = canonical_ring(&self.clip_vertices[first..end]);
                let (key, direction) = if owner < other {
                    ((owner, other, portal.plane, ring), 1)
                } else {
                    ((other, owner, portal.plane ^ 1, ring), -1)
                };
                *fragments.entry(key).or_default() += direction;
            }
            if planes.len() != 2 || fragments.values().any(|&count| count != 0) {
                return Err(invalid(
                    "areaportal key must connect two areas with reciprocal planes and clip fragments",
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn bounded_data<T: Pod>(
    bsp: &Bsp,
    slot: usize,
    limit: usize,
    version: i32,
) -> io::Result<&[u8]> {
    let metadata = &bsp.lumps[slot];
    if metadata.version != version || metadata.fourcc != [0; 4] || bsp.is_compressed(slot) {
        return Err(invalid(format!("unsupported BSP lump {slot} metadata")));
    }
    if metadata.data.len() / size_of::<T>() > limit {
        return Err(invalid(format!(
            "BSP lump {slot} exceeds its native count limit"
        )));
    }
    bsp.checked(slot, size_of::<T>())
}

fn records<T: Pod>(bsp: &Bsp, slot: usize, limit: usize, version: i32) -> io::Result<Vec<T>> {
    Ok(bytemuck::pod_collect_to_vec(bounded_data::<T>(
        bsp, slot, limit, version,
    )?))
}

impl Bsp {
    pub fn area_data(&self) -> io::Result<Option<AreaData>> {
        if [lump::AREAS, lump::AREAPORTALS, lump::CLIPPORTALVERTS]
            .iter()
            .all(|&slot| self.lumps[slot].data.is_empty())
        {
            return Ok(None);
        }
        let data = AreaData {
            areas: records(self, lump::AREAS, MAX_AREAS, 0)?,
            portals: records(self, lump::AREAPORTALS, MAX_AREA_PORTALS, 0)?,
            clip_vertices: records(self, lump::CLIPPORTALVERTS, MAX_CLIP_PORTAL_VERTICES, 0)?,
        };
        let leaf_version = self.lumps[lump::LEAFS].version;
        if ![0, 1].contains(&leaf_version) {
            return Err(invalid("unsupported area-related leaf version"));
        }
        bounded_data::<Plane>(self, lump::PLANES, MAX_PLANES, 0)?;
        self.checked(
            lump::LEAFS,
            if leaf_version == 0 {
                56
            } else {
                size_of::<Leaf>()
            },
        )?;
        let plane_count = self.lumps[lump::PLANES].data.len() / size_of::<Plane>();
        let leaf_count = self.lumps[lump::LEAFS].data.len()
            / if leaf_version == 0 {
                56
            } else {
                size_of::<Leaf>()
            };
        if plane_count > MAX_PLANES || leaf_count > MAX_LEAVES {
            return Err(invalid("area reference arrays exceed native bounds"));
        }
        data.validate(plane_count, &self.records::<Leaf>()?)?;
        Ok(Some(data))
    }

    pub fn set_area_data(&mut self, data: &AreaData) -> io::Result<()> {
        let leaf_version = self.lumps[lump::LEAFS].version;
        if ![0, 1].contains(&leaf_version) {
            return Err(invalid("unsupported area-related leaf version"));
        }
        let planes = bounded_data::<Plane>(self, lump::PLANES, MAX_PLANES, 0)?;
        let stride = if leaf_version == 0 {
            56
        } else {
            size_of::<Leaf>()
        };
        let bytes = self.checked(lump::LEAFS, stride)?;
        if bytes.len() / stride > MAX_LEAVES {
            return Err(invalid("area-related leaf count exceeds native bounds"));
        }
        data.validate(planes.len() / size_of::<Plane>(), &self.records::<Leaf>()?)?;
        self.set_records(&data.areas);
        self.set_records(&data.portals);
        self.set_values(lump::CLIPPORTALVERTS, &data.clip_vertices);
        Ok(())
    }
}

impl Leaf {
    pub fn with_area(mut self, area: i16) -> io::Result<Self> {
        if !(0..MAX_AREAS as i16).contains(&area) {
            return Err(invalid("leaf area must be between 0 and 255"));
        }
        self.area_flags = (self.area_flags & !0x1ff) | area;
        Ok(self)
    }
}
