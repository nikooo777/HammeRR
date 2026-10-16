use std::io;
use std::ops::Range;

use super::super::{HEADER_SIZE, IDENT, LUMP_COUNT, i32_at};
use super::{CHUNK_BYTES, PakArchive, PakLimits, invalid, lump, reserve};

/// Append a checked PAK and update only its BSP lump header.
/// Existing bytes, including unreachable old PAK data and absolute game-lump offsets, remain intact.
pub fn replace_bsp_pak(
    data: &[u8],
    archive: &PakArchive,
    limits: PakLimits,
) -> io::Result<Vec<u8>> {
    replace_bsp_pak_controlled(data, archive, limits, &mut || Ok(()))
}

/// The controlled variant of [`replace_bsp_pak`]; performs no filesystem writes.
pub fn replace_bsp_pak_controlled(
    data: &[u8],
    archive: &PakArchive,
    limits: PakLimits,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    check()?;
    limits.validate()?;
    let original = preflight_bsp(data, limits, check)?;
    if !original.is_empty() {
        PakArchive::parse_controlled(&data[original], limits, check)?;
    }
    let pak = archive.to_bytes_controlled(limits, check)?;
    let padding = (4 - data.len() % 4) % 4;
    let start = data
        .len()
        .checked_add(padding)
        .ok_or_else(|| invalid("BSP PAK offset overflow"))?;
    let size = start
        .checked_add(pak.len())
        .ok_or_else(|| invalid("BSP PAK output size overflow"))?;
    if size > limits.max_bsp_bytes || size > i32::MAX as usize {
        return Err(invalid(
            "BSP with appended PAK exceeds its file-byte budget",
        ));
    }
    let mut bytes = Vec::new();
    reserve(&mut bytes, size)?;
    for chunk in data.chunks(CHUNK_BYTES) {
        check()?;
        bytes.extend_from_slice(chunk);
    }
    bytes.resize(start, 0);
    for chunk in pak.chunks(CHUNK_BYTES) {
        check()?;
        bytes.extend_from_slice(chunk);
    }
    check()?;
    let header = 8 + lump::PAKFILE * 16;
    bytes[header..header + 4].copy_from_slice(&(start as i32).to_le_bytes());
    bytes[header + 4..header + 8].copy_from_slice(&(pak.len() as i32).to_le_bytes());
    bytes[header + 8..header + 16].fill(0);
    Ok(bytes)
}

fn preflight_bsp(
    data: &[u8],
    limits: PakLimits,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Range<usize>> {
    if data.len() < HEADER_SIZE || data.len() > limits.max_bsp_bytes || data[..4] != IDENT {
        return Err(invalid(
            "BSP is truncated, oversized or missing its VBSP signature",
        ));
    }
    if !(19..=21).contains(&i32_at(data, 4)) {
        return Err(invalid("Unsupported BSP version for PAK replacement"));
    }
    let mut spans = Vec::new();
    reserve(&mut spans, LUMP_COUNT)?;
    let mut pak = 0..0;
    for index in 0..LUMP_COUNT {
        check()?;
        let header = 8 + index * 16;
        let start = i32_at(data, header);
        let length = i32_at(data, header + 4);
        if start < 0 || length < 0 {
            return Err(invalid("BSP lump has a negative offset or length"));
        }
        let start = start as usize;
        let end = start
            .checked_add(length as usize)
            .ok_or_else(|| invalid("BSP lump range overflow"))?;
        if end > data.len() || length != 0 && start < HEADER_SIZE {
            return Err(invalid("BSP lump exceeds the file or overlaps its header"));
        }
        if length != 0 {
            spans.push(start..end);
        }
        if index == lump::PAKFILE {
            if i32_at(data, header + 8) != 0 || data[header + 12..header + 16] != [0; 4] {
                return Err(invalid("BSP PAK lump requires uncompressed version 0"));
            }
            pak = start..end;
        }
    }
    spans.sort_unstable_by_key(|span| span.start);
    if spans.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(invalid("BSP lump spans overlap"));
    }
    Ok(pak)
}
