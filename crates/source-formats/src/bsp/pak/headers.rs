use std::collections::BTreeSet;
use std::io;
use std::ops::Range;

use super::{PakEntry, PakLimits, invalid, reserve};

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const DESCRIPTOR: u32 = 0x0807_4b50;
const END_BYTES: usize = 22;

pub(super) struct Directory<'a> {
    pub entries: Vec<Entry<'a>>,
}

pub(super) struct Entry<'a> {
    pub name: &'a str,
    pub size: usize,
    pub compressed_size: usize,
    pub crc32: u32,
    pub method: u16,
    pub header_start: usize,
    pub data_start: usize,
    pub local_end: usize,
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn range(data: &[u8], start: usize, length: usize) -> io::Result<Range<usize>> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| invalid("PAK ZIP span overflow"))?;
    if end > data.len() {
        return Err(invalid("PAK ZIP span exceeds the archive"));
    }
    Ok(start..end)
}

pub(super) fn name(name: &str, limits: PakLimits) -> io::Result<()> {
    if name.is_empty()
        || name.len() > limits.max_name_bytes
        || name.len() > u16::MAX as usize
        || name.chars().any(|character| character.is_control())
        || name.contains(['\\', ':'])
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid(
            "PAK file name must be a bounded relative UTF-8 asset path",
        ));
    }
    Ok(())
}

fn extra_fields(data: &[u8]) -> io::Result<()> {
    let mut position = 0;
    while position < data.len() {
        range(data, position, 4)?;
        let kind = u16_at(data, position);
        let length = u16_at(data, position + 2) as usize;
        if matches!(kind, 1 | 0x9901) {
            return Err(invalid("PAK ZIP64 and AES extra fields are unsupported"));
        }
        position = range(data, position + 4, length)?.end;
    }
    Ok(())
}

fn footer(data: &[u8], check: &mut dyn FnMut() -> io::Result<()>) -> io::Result<usize> {
    if data.len() < END_BYTES {
        return Err(invalid("PAK ZIP end record is truncated"));
    }
    let first = data.len().saturating_sub(END_BYTES + u16::MAX as usize);
    for position in (first..=data.len() - END_BYTES).rev() {
        if position % 1024 == 0 {
            check()?;
        }
        if u32_at(data, position) == END
            && position
                .checked_add(END_BYTES)
                .and_then(|end| end.checked_add(u16_at(data, position + 20) as usize))
                == Some(data.len())
        {
            return Ok(position);
        }
    }
    Err(invalid("PAK ZIP end record or comment length is invalid"))
}

pub(super) fn preflight<'a>(
    data: &'a [u8],
    limits: PakLimits,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Directory<'a>> {
    check()?;
    limits.validate()?;
    if data.len() > limits.max_archive_bytes {
        return Err(invalid("PAK ZIP exceeds its encoded-byte budget"));
    }
    let end = footer(data, check)?;
    let count = u16_at(data, end + 10) as usize;
    let central_size = u32_at(data, end + 12) as usize;
    let central_start = u32_at(data, end + 16) as usize;
    if u16_at(data, end + 4) != 0
        || u16_at(data, end + 6) != 0
        || u16_at(data, end + 8) as usize != count
        || count == u16::MAX as usize
        || central_size == u32::MAX as usize
        || central_start == u32::MAX as usize
    {
        return Err(invalid(
            "PAK multidisk and ZIP64 directories are unsupported",
        ));
    }
    if count > limits.max_entries {
        return Err(invalid("PAK ZIP exceeds its entry-count budget"));
    }
    if range(data, central_start, central_size)?.end != end {
        return Err(invalid(
            "PAK ZIP central directory has inconsistent offsets",
        ));
    }
    let mut entries = Vec::new();
    reserve(&mut entries, count)?;
    let mut spans = Vec::new();
    reserve(&mut spans, count)?;
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    let mut position = central_start;
    for _ in 0..count {
        check()?;
        if range(data, position, 46)?.end > end {
            return Err(invalid("PAK ZIP central header exceeds its directory"));
        }
        if u32_at(data, position) != CENTRAL {
            return Err(invalid("PAK ZIP central header signature is invalid"));
        }
        let version = u16_at(data, position + 6);
        let flags = u16_at(data, position + 8);
        let method = u16_at(data, position + 10);
        if !(10..=20).contains(&version)
            || !matches!(method, 0 | 8)
            || flags & !0x080e != 0
            || method == 0 && flags & 6 != 0
        {
            return Err(invalid(
                "PAK requires unencrypted ZIP32 STORE or DEFLATE entries",
            ));
        }
        let crc32 = u32_at(data, position + 16);
        let compressed_size = u32_at(data, position + 20) as usize;
        let size = u32_at(data, position + 24) as usize;
        let header_start = u32_at(data, position + 42) as usize;
        if [compressed_size, size, header_start].contains(&(u32::MAX as usize)) {
            return Err(invalid("PAK ZIP64 entry fields are unsupported"));
        }
        if size > limits.max_entry_bytes || compressed_size > limits.max_archive_bytes {
            return Err(invalid("PAK ZIP entry exceeds its byte budget"));
        }
        if method == 0 && size != compressed_size {
            return Err(invalid("PAK stored entry has inconsistent sizes"));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("PAK decoded-size overflow"))?;
        if total > limits.max_total_bytes {
            return Err(invalid(
                "PAK ZIP exceeds its total decompressed-byte budget",
            ));
        }
        let name_bytes = u16_at(data, position + 28) as usize;
        let extra_bytes = u16_at(data, position + 30) as usize;
        let comment_bytes = u16_at(data, position + 32) as usize;
        let name_span = range(data, position + 46, name_bytes)?;
        let name = std::str::from_utf8(&data[name_span.clone()])
            .map_err(|_| invalid("PAK ZIP asset name is not UTF-8"))?;
        self::name(name, limits)?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(invalid(
                "PAK ZIP contains duplicate ASCII-case-folded asset names",
            ));
        }
        let extra_span = range(data, name_span.end, extra_bytes)?;
        extra_fields(&data[extra_span.clone()])?;
        let next = range(data, extra_span.end, comment_bytes)?.end;
        if next > end || u16_at(data, position + 34) != 0 {
            return Err(invalid("PAK ZIP central entry exceeds its directory"));
        }
        let attributes = u32_at(data, position + 38);
        let mode = attributes >> 16 & 0o170000;
        if attributes & 0x10 != 0 || !matches!(mode, 0 | 0o100000) {
            return Err(invalid("PAK entries must be regular asset files"));
        }
        let mut entry = Entry {
            name,
            size,
            compressed_size,
            crc32,
            method,
            header_start,
            data_start: 0,
            local_end: 0,
        };
        let (span, data_start) = local_span(data, &entry, version, flags, central_start)?;
        entry.data_start = data_start;
        entry.local_end = span.end;
        spans.push(span);
        entries.push(entry);
        position = next;
    }
    if position != end {
        return Err(invalid(
            "PAK ZIP central size or entry count is inconsistent",
        ));
    }
    spans.sort_unstable_by_key(|span| span.start);
    if spans.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(invalid("PAK ZIP local entry spans overlap"));
    }
    check()?;
    Ok(Directory { entries })
}

fn local_span(
    data: &[u8],
    entry: &Entry<'_>,
    version: u16,
    flags: u16,
    central_start: usize,
) -> io::Result<(Range<usize>, usize)> {
    let start = entry.header_start;
    if range(data, start, 30)?.end > central_start {
        return Err(invalid(
            "PAK ZIP local header overlaps its central directory",
        ));
    }
    if u32_at(data, start) != LOCAL
        || u16_at(data, start + 4) != version
        || u16_at(data, start + 6) != flags
        || u16_at(data, start + 8) != entry.method
    {
        return Err(invalid("PAK ZIP local and central headers disagree"));
    }
    let crc32 = u32_at(data, start + 14);
    let compressed = u32_at(data, start + 18);
    let decoded = u32_at(data, start + 22);
    let expected = [entry.crc32, entry.compressed_size as u32, entry.size as u32];
    let actual = [crc32, compressed, decoded];
    if actual
        .iter()
        .zip(expected)
        .any(|(&actual, expected)| actual != expected && (flags & 8 == 0 || actual != 0))
    {
        return Err(invalid(
            "PAK ZIP local CRC or sizes disagree with its directory",
        ));
    }
    let name_bytes = u16_at(data, start + 26) as usize;
    let extra_bytes = u16_at(data, start + 28) as usize;
    let name_span = range(data, start + 30, name_bytes)?;
    if &data[name_span.clone()] != entry.name.as_bytes() {
        return Err(invalid("PAK ZIP local and central file names disagree"));
    }
    let extra_span = range(data, name_span.end, extra_bytes)?;
    extra_fields(&data[extra_span.clone()])?;
    let mut end = range(data, extra_span.end, entry.compressed_size)?.end;
    if flags & 8 != 0 {
        let signed = data
            .get(end..end.saturating_add(4))
            .is_some_and(|signature| signature == DESCRIPTOR.to_le_bytes());
        let offset = if signed && descriptor_matches(data, end + 4, expected) {
            4
        } else {
            0
        };
        if !descriptor_matches(data, end + offset, expected) {
            return Err(invalid("PAK ZIP data descriptor CRC or sizes disagree"));
        }
        end = range(data, end, offset + 12)?.end;
    }
    if end > central_start {
        return Err(invalid("PAK ZIP entry overlaps its central directory"));
    }
    Ok((start..end, extra_span.end))
}

fn descriptor_matches(data: &[u8], start: usize, expected: [u32; 3]) -> bool {
    range(data, start, 12).is_ok()
        && expected
            .into_iter()
            .enumerate()
            .all(|(index, expected)| u32_at(data, start + index * 4) == expected)
}

pub(super) fn writer_preflight<'a>(
    entries: &'a [PakEntry],
    limits: PakLimits,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<(Vec<&'a PakEntry>, usize)> {
    check()?;
    limits.validate()?;
    if entries.len() > limits.max_entries {
        return Err(invalid("PAK ZIP exceeds its entry-count budget"));
    }
    let mut sorted = Vec::new();
    reserve(&mut sorted, entries.len())?;
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    let mut encoded = END_BYTES;
    for entry in entries {
        check()?;
        name(&entry.name, limits)?;
        if !names.insert(entry.name.to_ascii_lowercase()) {
            return Err(invalid(
                "PAK ZIP contains duplicate ASCII-case-folded asset names",
            ));
        }
        if entry.data.len() > limits.max_entry_bytes {
            return Err(invalid("PAK ZIP entry exceeds its byte budget"));
        }
        total = total
            .checked_add(entry.data.len())
            .ok_or_else(|| invalid("PAK decoded-size overflow"))?;
        if total > limits.max_total_bytes {
            return Err(invalid(
                "PAK ZIP exceeds its total decompressed-byte budget",
            ));
        }
        encoded = encoded
            .checked_add(76)
            .and_then(|size| size.checked_add(entry.name.len().checked_mul(2)?))
            .and_then(|size| size.checked_add(entry.data.len()))
            .ok_or_else(|| invalid("PAK stored ZIP output size overflow"))?;
        if encoded > limits.max_archive_bytes {
            return Err(invalid("PAK stored ZIP exceeds its encoded-byte budget"));
        }
        sorted.push(entry);
    }
    if encoded > limits.max_archive_bytes {
        return Err(invalid("PAK stored ZIP exceeds its encoded-byte budget"));
    }
    sorted.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    check()?;
    Ok((sorted, encoded))
}
