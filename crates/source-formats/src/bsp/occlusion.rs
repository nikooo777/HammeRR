use std::io;

use bytemuck::{Pod, Zeroable};

use super::{Area, Bsp, Lump, Plane, invalid, lump};

pub const MAX_OCCLUDERS: usize = 1024;
pub const MAX_OCCLUSION_POLYGONS: usize = 65_536;
pub const MAX_OCCLUSION_VERTEX_INDICES: usize = 262_144;
pub const OCCLUDER_INACTIVE: i32 = 1;
const MAX_REFERENCE_RECORDS: usize = 65_536;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Occluder {
    pub flags: i32,
    pub first_polygon: i32,
    pub polygon_count: i32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub area: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct OccluderPolygon {
    pub first_vertex_index: i32,
    pub vertex_count: i32,
    pub plane: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Occlusion {
    pub occluders: Vec<Occluder>,
    pub polygons: Vec<OccluderPolygon>,
    pub vertex_indices: Vec<i32>,
}

fn range(first: i32, count: i32, length: usize) -> io::Result<std::ops::Range<usize>> {
    let first = usize::try_from(first).map_err(|_| invalid("negative occlusion offset"))?;
    let count = usize::try_from(count).map_err(|_| invalid("negative occlusion count"))?;
    let end = first
        .checked_add(count)
        .filter(|&end| end <= length)
        .ok_or_else(|| invalid("occlusion range exceeds its reference array"))?;
    Ok(first..end)
}

impl Occlusion {
    pub fn validate(
        &self,
        plane_count: usize,
        vertex_count: usize,
        area_count: usize,
    ) -> io::Result<()> {
        if self.occluders.len() > MAX_OCCLUDERS
            || self.polygons.len() > MAX_OCCLUSION_POLYGONS
            || self.vertex_indices.len() > MAX_OCCLUSION_VERTEX_INDICES
        {
            return Err(invalid("occlusion arrays exceed their allocation caps"));
        }
        for occluder in &self.occluders {
            range(
                occluder.first_polygon,
                occluder.polygon_count,
                self.polygons.len(),
            )?;
            if occluder.polygon_count == 0 && occluder.area == -1 {
                continue;
            }
            if occluder.area < 0 || occluder.area as usize >= area_count {
                return Err(invalid("occluder area is outside the area array"));
            }
            if (0..3).any(|axis| {
                !occluder.mins[axis].is_finite()
                    || !occluder.maxs[axis].is_finite()
                    || occluder.mins[axis] > occluder.maxs[axis]
            }) {
                return Err(invalid("occluder bounds must be finite and ordered"));
            }
        }
        for polygon in &self.polygons {
            range(
                polygon.first_vertex_index,
                polygon.vertex_count,
                self.vertex_indices.len(),
            )?;
            if polygon.vertex_count < 3
                || polygon.plane < 0
                || polygon.plane as usize >= plane_count
            {
                return Err(invalid(
                    "occluder polygon needs three vertices and a valid plane",
                ));
            }
        }
        if self
            .vertex_indices
            .iter()
            .any(|&index| index < 0 || index as usize >= vertex_count)
        {
            return Err(invalid(
                "occluder vertex index is outside the global vertex array",
            ));
        }
        Ok(())
    }
}

fn take_records<T: Pod>(input: &mut &[u8], limit: usize) -> io::Result<Vec<T>> {
    let count_bytes = input
        .get(..4)
        .ok_or_else(|| invalid("truncated occlusion count"))?;
    let count = i32::from_le_bytes(count_bytes.try_into().unwrap());
    let count = usize::try_from(count).map_err(|_| invalid("negative occlusion array count"))?;
    if count > limit {
        return Err(invalid("occlusion array count exceeds its allocation cap"));
    }
    let end = count
        .checked_mul(size_of::<T>())
        .and_then(|length| length.checked_add(4))
        .ok_or_else(|| invalid("occlusion array size overflow"))?;
    let bytes = input
        .get(4..end)
        .ok_or_else(|| invalid("truncated occlusion array"))?;
    let result = bytemuck::pod_collect_to_vec(bytes);
    *input = &input[end..];
    Ok(result)
}

fn reference_count<T: Pod>(bsp: &Bsp, slot: usize, limit: usize) -> io::Result<usize> {
    let metadata = &bsp.lumps[slot];
    if metadata.version != 0 || metadata.fourcc != [0; 4] {
        return Err(invalid("unsupported occlusion reference lump metadata"));
    }
    let count = bsp.checked(slot, size_of::<T>())?.len() / size_of::<T>();
    if count > limit {
        return Err(invalid(
            "occlusion reference array exceeds its allocation cap",
        ));
    }
    Ok(count)
}

fn references(bsp: &Bsp) -> io::Result<(usize, usize, usize)> {
    Ok((
        reference_count::<Plane>(bsp, lump::PLANES, MAX_REFERENCE_RECORDS)?,
        reference_count::<[f32; 3]>(bsp, lump::VERTEXES, MAX_REFERENCE_RECORDS)?,
        reference_count::<Area>(bsp, lump::AREAS, super::MAX_AREAS)?,
    ))
}

fn finite_references(bsp: &Bsp, data: &Occlusion) -> io::Result<()> {
    for polygon in &data.polygons {
        let offset = polygon.plane as usize * size_of::<Plane>();
        let plane: Plane = bytemuck::pod_read_unaligned(
            &bsp.lumps[lump::PLANES].data[offset..offset + size_of::<Plane>()],
        );
        if !plane.dist.is_finite() || plane.normal.iter().any(|value| !value.is_finite()) {
            return Err(invalid("occluder references a nonfinite plane"));
        }
    }
    for &index in &data.vertex_indices {
        let offset = index as usize * size_of::<[f32; 3]>();
        let point: [f32; 3] = bytemuck::pod_read_unaligned(
            &bsp.lumps[lump::VERTEXES].data[offset..offset + size_of::<[f32; 3]>()],
        );
        if point.iter().any(|value| !value.is_finite()) {
            return Err(invalid("occluder references a nonfinite vertex"));
        }
    }
    Ok(())
}

fn append_records<T: Pod>(bytes: &mut Vec<u8>, records: &[T]) {
    bytes.extend_from_slice(&(records.len() as i32).to_le_bytes());
    bytes.extend_from_slice(bytemuck::cast_slice(records));
}

impl Bsp {
    pub fn occlusion(&self) -> io::Result<Option<Occlusion>> {
        let metadata = &self.lumps[lump::OCCLUSION];
        if metadata.fourcc != [0; 4] || self.is_compressed(lump::OCCLUSION) {
            return Err(invalid("compressed occlusion is unsupported"));
        }
        if metadata.data.is_empty() {
            return Ok(None);
        }
        let mut input = metadata.data.as_slice();
        let occluders = match metadata.version {
            1 => take_records::<[u32; 9]>(&mut input, MAX_OCCLUDERS)?
                .into_iter()
                .map(|record| Occluder {
                    flags: record[0] as i32,
                    first_polygon: record[1] as i32,
                    polygon_count: record[2] as i32,
                    mins: [record[3], record[4], record[5]].map(f32::from_bits),
                    maxs: [record[6], record[7], record[8]].map(f32::from_bits),
                    area: 1,
                })
                .collect(),
            2 => take_records(&mut input, MAX_OCCLUDERS)?,
            _ => return Err(invalid("unsupported occlusion lump version")),
        };
        let data = Occlusion {
            occluders,
            polygons: take_records(&mut input, MAX_OCCLUSION_POLYGONS)?,
            vertex_indices: take_records(&mut input, MAX_OCCLUSION_VERTEX_INDICES)?,
        };
        if !input.is_empty() {
            return Err(invalid("trailing occlusion bytes"));
        }
        let (planes, vertices, areas) = references(self)?;
        data.validate(planes, vertices, areas)?;
        finite_references(self, &data)?;
        Ok(Some(data))
    }

    pub fn set_occlusion(&mut self, data: &Occlusion) -> io::Result<()> {
        let (planes, vertices, areas) = references(self)?;
        data.validate(planes, vertices, areas)?;
        finite_references(self, data)?;
        let mut bytes = Vec::new();
        append_records(&mut bytes, &data.occluders);
        append_records(&mut bytes, &data.polygons);
        append_records(&mut bytes, &data.vertex_indices);
        self.lumps[lump::OCCLUSION] = Lump {
            version: 2,
            fourcc: [0; 4],
            data: bytes,
        };
        Ok(())
    }
}
