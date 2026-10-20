use std::io;

use super::{MAX_VERTICES, Vertex, hardware::Budget, invalid, reader::Reader};

pub struct Vertices {
    pub vertices: Vec<Vertex>,
    pub lods: usize,
    pub counts: [usize; super::MAX_LODS],
}

pub fn parse(
    data: &[u8],
    checksum: i32,
    reference: Option<&[glam::DAffine3]>,
    budget: &mut Budget<'_>,
) -> io::Result<Vertices> {
    let reader = Reader::new(data, 64)?;
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
    reader.bytes(start, count * 48)?;
    let fixups = reader.table(0, 48, 52, 12, MAX_VERTICES)?;
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
    for (work, index) in order.into_iter().enumerate() {
        budget.checkpoint(work)?;
        let offset = start + index * 48;
        let position = reader.vector(offset + 16)?;
        if position.iter().any(|value| value.abs() > 65536.0) {
            return Err(invalid("model vertices must fit ±65536 units"));
        }
        let normal = reader.vector(offset + 28)?;
        let length = normal.iter().map(|value| value * value).sum::<f32>();
        if !length.is_finite() || length < 1e-12 {
            return Err(invalid("model vertex normals must be nonzero"));
        }
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
    Ok(Vertices {
        vertices,
        lods,
        counts,
    })
}
