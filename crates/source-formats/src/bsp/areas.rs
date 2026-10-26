use std::collections::BTreeMap;
use std::io;

use bytemuck::Pod;

use super::{Area, AreaPortal, Bsp, Leaf, Plane, invalid, lump};

pub const MAX_AREAS: usize = 256;
pub const MAX_AREA_PORTALS: usize = 1024;
pub const MAX_CLIP_PORTAL_VERTICES: usize = 128_000;
const MAX_PLANES: usize = 65_536;
const MAX_LEAVES: usize = 65_536;

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
            if count < 3
                || first
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
            let &[(first, a), (second, b)] = entries.as_slice() else {
                return Err(invalid("areaportal key must have two reciprocal records"));
            };
            if usize::from(a.other_area) != second
                || usize::from(b.other_area) != first
                || a.plane ^ 1 != b.plane
            {
                return Err(invalid("areaportal areas and planes are not reciprocal"));
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
