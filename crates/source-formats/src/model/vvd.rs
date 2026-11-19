use std::io;

use super::{MAX_VERTICES, Vertex, hardware::Budget, invalid, reader::Reader, reader::Table};

const HEADER_BYTES: usize = 64;
const VERTEX_BYTES: usize = 48;
const FIXUP_BYTES: usize = 12;
const TANGENT_BYTES: usize = 16;

pub struct Vertices {
    pub vertices: Vec<Vertex>,
    pub lods: usize,
    pub counts: [usize; super::MAX_LODS],
    pub tangents: Option<io::Result<Box<[[f32; 4]]>>>,
}

pub fn parse(
    data: &[u8],
    checksum: i32,
    reference: Option<&[glam::DAffine3]>,
    budget: &mut Budget<'_>,
    preview_tangents: bool,
) -> io::Result<Vertices> {
    let reader = Reader::new(data, HEADER_BYTES)?;
    if reader.bytes(0, 4)? != b"IDSV" || reader.int(4)? != 4 || reader.int(8)? != checksum {
        return Err(invalid(
            "VVD magic, version or model checksum does not match",
        ));
    }
    let lods = reader.count(12, super::MAX_LODS)?;
    if lods == 0 {
        return Err(invalid("VVD requires at least one LOD"));
    }
    let count = reader.count(16, MAX_VERTICES)?;
    if count == 0 {
        return Err(invalid("VVD has no vertices"));
    }
    let mut previous = count;
    let mut counts = [0; super::MAX_LODS];
    for (lod, count) in counts[..lods].iter_mut().enumerate() {
        let value = reader.count(16 + lod * 4, MAX_VERTICES)?;
        if value > previous {
            return Err(invalid("VVD LOD vertex counts must decrease"));
        }
        previous = value;
        *count = value;
    }
    let start = reader.count(56, data.len())?;
    reader.bytes(start, count * VERTEX_BYTES)?;
    let fixups = reader.table(0, 48, 52, FIXUP_BYTES, MAX_VERTICES)?;
    budget.array::<usize>(count)?;
    budget.array::<Vertex>(count)?;
    budget.charge(0, fixups.len)?;
    let mut order = Vec::with_capacity(count);
    let mut fixed_counts = [0usize; super::MAX_LODS];
    if fixups.len == 0 {
        order.extend(0..count);
    } else {
        for index in 0..fixups.len {
            budget.checkpoint(index)?;
            let fixup = fixups.at(index)?;
            let lod = reader.count(fixup, super::MAX_LODS - 1)?;
            if lod >= lods {
                return Err(invalid("VVD fixup references an unavailable LOD"));
            }
            let source = reader.count(fixup + 4, count)?;
            let size = reader.count(fixup + 8, count)?;
            let end = source
                .checked_add(size)
                .filter(|&end| end <= count)
                .ok_or_else(|| invalid("VVD fixup exceeds the vertex stream"))?;
            if order.len() + size > count {
                return Err(invalid("VVD fixups exceed the root vertex count"));
            }
            if budget.enabled() {
                for value in &mut fixed_counts[..=lod] {
                    *value = value
                        .checked_add(size)
                        .ok_or_else(|| invalid("VVD fixed-up LOD vertex count overflow"))?;
                }
            }
            order.extend(source..end);
        }
        if order.len() != count {
            return Err(invalid("VVD fixups do not cover the root vertex count"));
        }
        if budget.enabled() {
            for (lod, &fixed_count) in fixed_counts[..lods].iter().enumerate() {
                if fixed_count != reader.count(16 + lod * 4, MAX_VERTICES)? {
                    return Err(invalid(
                        "VVD fixups do not match declared LOD vertex counts",
                    ));
                }
            }
        }
    }
    let mut vertices = Vec::with_capacity(count);
    for (work, &index) in order.iter().enumerate() {
        budget.checkpoint(work)?;
        let offset = start + index * VERTEX_BYTES;
        let position = reader.vector(offset + 16)?;
        if position.iter().any(|value| value.abs() > 65536.0) {
            return Err(invalid("model vertices must fit ±65536 units"));
        }
        let normal = reader.vector(offset + 28)?;
        let (position, normal) = match reference {
            Some(transforms) => {
                super::bones::vertex(&reader, offset, transforms, position, normal)?
            }
            None => (position, normal),
        };
        vertices.push(Vertex {
            position,
            normal,
            uv: [reader.float(offset + 40)?, reader.float(offset + 44)?],
        });
    }
    let tangents =
        preview_tangents.then(|| read_tangents(&reader, start, &fixups, &order, reference));
    Ok(Vertices {
        vertices,
        lods,
        counts,
        tangents,
    })
}

fn tangent_start(reader: &Reader<'_>, vertex_start: usize, count: usize) -> io::Result<usize> {
    let start = usize::try_from(reader.int(60)?)
        .map_err(|_| invalid("VVD tangent stream has a negative offset"))?;
    if start == 0 {
        return Err(invalid("VVD has no tangent stream"));
    }
    let vertex_end = vertex_start
        .checked_add(count * VERTEX_BYTES)
        .ok_or_else(|| invalid("VVD vertex range overflow"))?;
    if vertex_start < HEADER_BYTES || start < vertex_end {
        return Err(invalid("VVD needs a complete separate tangent stream"));
    }
    reader.bytes(start, count * TANGENT_BYTES)?;
    Ok(start)
}

fn tangent(reader: &Reader<'_>, start: usize, index: usize) -> io::Result<[f32; 4]> {
    let offset = start + index * TANGENT_BYTES;
    let xyz = reader.vector(offset)?;
    let sign = reader.float(offset + 12)?;
    if !matches!(sign, -1.0 | 1.0) {
        return Err(invalid("VVD tangent handedness must be -1 or 1"));
    }
    Ok([xyz[0], xyz[1], xyz[2], sign])
}

fn read_tangents(
    reader: &Reader<'_>,
    vertex_start: usize,
    fixups: &Table,
    order: &[usize],
    reference: Option<&[glam::DAffine3]>,
) -> io::Result<Box<[[f32; 4]]>> {
    let count = order.len();
    let start = tangent_start(reader, vertex_start, count)?;
    if fixups.len != 0
        && start < fixups.start + fixups.len * FIXUP_BYTES
        && fixups.start < start + count * TANGENT_BYTES
    {
        return Err(invalid("VVD tangent stream overlaps the fixup table"));
    }
    let mut tangents = Vec::new();
    tangents
        .try_reserve_exact(count)
        .map_err(|_| invalid("VVD tangent preview allocation failed"))?;
    for &index in order {
        let tangent = tangent(reader, start, index)?;
        tangents.push(match reference {
            Some(transforms) => super::bones::tangent(
                reader,
                vertex_start + index * VERTEX_BYTES,
                transforms,
                tangent,
            )?,
            None => tangent,
        });
    }
    Ok(tangents.into_boxed_slice())
}

pub(super) fn validate_tangents(data: &[u8], budget: &mut Budget<'_>) -> io::Result<()> {
    let reader = Reader::new(data, HEADER_BYTES)?;
    let count = reader.count(16, MAX_VERTICES)?;
    let start = tangent_start(&reader, reader.count(56, data.len())?, count)?;
    budget.charge(0, count * 5)?;
    for index in 0..count {
        budget.checkpoint(index)?;
        let [x, y, z, _] = tangent(&reader, start, index)?;
        if (x * x + y * y + z * z - 1.0).abs() > 0.01 {
            return Err(invalid("Portable VVD tangents must have unit length"));
        }
    }
    Ok(())
}
