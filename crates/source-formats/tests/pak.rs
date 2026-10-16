use std::io;

use source_formats::bsp::{
    Bsp, GameLump, PakArchive, PakEntry, PakLimits, lump, replace_bsp_pak,
    replace_bsp_pak_controlled,
};

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn get32(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}

struct File<'a> {
    name: &'a [u8],
    data: &'a [u8],
    deflate: Option<&'a [u8]>,
    descriptor: Option<bool>,
}

fn stored<'a>(name: &'a str, data: &'a [u8]) -> File<'a> {
    File {
        name: name.as_bytes(),
        data,
        deflate: None,
        descriptor: None,
    }
}

fn legacy_zip(files: &[File<'_>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offsets = Vec::new();
    for file in files {
        offsets.push(bytes.len());
        let encoded = file.deflate.unwrap_or(file.data);
        let version = if file.deflate.is_some() { 20 } else { 10 };
        let mut header = [0; 30];
        put32(&mut header, 0, 0x0403_4b50);
        put16(&mut header, 4, version);
        put16(
            &mut header,
            6,
            if file.descriptor.is_some() { 8 } else { 0 },
        );
        put16(&mut header, 8, if file.deflate.is_some() { 8 } else { 0 });
        if file.descriptor.is_none() {
            put32(&mut header, 14, crc32fast::hash(file.data));
            put32(&mut header, 18, encoded.len() as u32);
            put32(&mut header, 22, file.data.len() as u32);
        }
        put16(&mut header, 26, file.name.len() as u16);
        bytes.extend(header);
        bytes.extend(file.name);
        bytes.extend(encoded);
        if let Some(signed) = file.descriptor {
            if signed {
                bytes.extend(0x0807_4b50u32.to_le_bytes());
            }
            bytes.extend(crc32fast::hash(file.data).to_le_bytes());
            bytes.extend((encoded.len() as u32).to_le_bytes());
            bytes.extend((file.data.len() as u32).to_le_bytes());
        }
    }
    let directory = bytes.len();
    for (file, offset) in files.iter().zip(offsets) {
        let encoded = file.deflate.unwrap_or(file.data);
        let version = if file.deflate.is_some() { 20 } else { 10 };
        let mut header = [0; 46];
        put32(&mut header, 0, 0x0201_4b50);
        put16(&mut header, 4, version);
        put16(&mut header, 6, version);
        put16(
            &mut header,
            8,
            if file.descriptor.is_some() { 8 } else { 0 },
        );
        put16(&mut header, 10, if file.deflate.is_some() { 8 } else { 0 });
        put32(&mut header, 16, crc32fast::hash(file.data));
        put32(&mut header, 20, encoded.len() as u32);
        put32(&mut header, 24, file.data.len() as u32);
        put16(&mut header, 28, file.name.len() as u16);
        put32(&mut header, 42, offset as u32);
        bytes.extend(header);
        bytes.extend(file.name);
    }
    let mut footer = [0; 22];
    put32(&mut footer, 0, 0x0605_4b50);
    put16(&mut footer, 8, files.len() as u16);
    put16(&mut footer, 10, files.len() as u16);
    put32(&mut footer, 12, (bytes.len() - directory) as u32);
    put32(&mut footer, 16, directory as u32);
    bytes.extend(footer);
    bytes
}

fn archive() -> PakArchive {
    PakArchive {
        entries: vec![
            PakEntry {
                name: "unknown/preserve.txt".into(),
                data: b"hello".to_vec(),
            },
            PakEntry {
                name: "materials/maps/test/c0_0_0.vtf".into(),
                data: vec![1, 2, 3],
            },
        ],
    }
}

#[test]
fn reads_legacy_stored_deflate_descriptors_and_native_comment() {
    let limits = PakLimits::default();
    assert!(
        PakArchive::parse(&legacy_zip(&[]), limits)
            .unwrap()
            .entries
            .is_empty()
    );
    let mut native = legacy_zip(&[stored("assets/hello.txt", b"hello"), stored("empty", b"")]);
    let end = native.len() - 22;
    put16(&mut native, end + 20, 32);
    native.extend(b"XZP1 0");
    native.resize(native.len() + 26, 0);
    let parsed = PakArchive::parse(&native, limits).unwrap();
    assert_eq!(parsed.entries[0].data, b"hello");
    assert!(parsed.entries[1].data.is_empty());
    assert_eq!(crc32fast::hash(b"hello"), 0x3610_a686);
    for descriptor in [None, Some(false), Some(true)] {
        let zipped = legacy_zip(&[File {
            name: b"hello",
            data: b"hello",
            deflate: Some(&[0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]),
            descriptor,
        }]);
        assert_eq!(
            PakArchive::parse(&zipped, limits).unwrap().entries[0].data,
            b"hello"
        );
    }
}

#[test]
fn stored_writer_is_deterministic_and_preserves_unknown_contents() {
    let limits = PakLimits::default();
    let first = archive();
    let mut reversed = first.clone();
    reversed.entries.reverse();
    let bytes = first.to_bytes(limits).unwrap();
    assert_eq!(bytes, reversed.to_bytes(limits).unwrap());
    let mut decoded = PakArchive::parse(&bytes, limits).unwrap();
    decoded.entries.sort_by(|a, b| a.name.cmp(&b.name));
    let mut expected = first;
    expected.entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(decoded, expected);
    assert_eq!(&bytes[..4], b"PK\x03\x04");
    assert_eq!(&bytes[8..12], &[0, 0, 0, 0]);
    assert_eq!(&bytes[12..14], &[33, 0]);
    let directory = get32(&bytes, bytes.len() - 6);
    assert_eq!(&bytes[directory + 10..directory + 12], &[0, 0]);
    assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
    assert_eq!(
        PakArchive::default().to_bytes(limits).unwrap(),
        legacy_zip(&[])
    );
}

#[test]
fn rejects_unsafe_non_utf8_duplicate_and_special_names() {
    let limits = PakLimits::default();
    for name in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a/./b",
        "a//b",
        "a\\b",
        "C:/a",
        "a\0b",
        "a\nb",
        "directory/",
    ] {
        assert!(
            PakArchive::parse(&legacy_zip(&[stored(name, b"x")]), limits).is_err(),
            "{name:?}"
        );
        let invalid = PakArchive {
            entries: vec![PakEntry {
                name: name.into(),
                data: vec![],
            }],
        };
        assert!(invalid.to_bytes(limits).is_err(), "{name:?}");
    }
    let non_utf8 = legacy_zip(&[File {
        name: b"a\xff",
        data: b"x",
        deflate: None,
        descriptor: None,
    }]);
    assert!(PakArchive::parse(&non_utf8, limits).is_err());
    for names in [["same", "same"], ["Materials/Asset", "materials/asset"]] {
        let duplicate = legacy_zip(&[stored(names[0], b"a"), stored(names[1], b"b")]);
        assert!(PakArchive::parse(&duplicate, limits).is_err());
        let duplicate = PakArchive {
            entries: names
                .map(|name| PakEntry {
                    name: name.into(),
                    data: vec![],
                })
                .into(),
        };
        assert!(duplicate.to_bytes(limits).is_err());
    }
    let unicode = legacy_zip(&[stored("materials/café.vmt", b"x")]);
    assert_eq!(
        PakArchive::parse(&unicode, limits).unwrap().entries[0].name,
        "materials/café.vmt"
    );
    for attributes in [0x10, 0o120777 << 16, 0o040755 << 16] {
        let mut bytes = legacy_zip(&[stored("asset", b"x")]);
        let directory = get32(&bytes, bytes.len() - 6);
        put32(&mut bytes, directory + 38, attributes);
        assert!(PakArchive::parse(&bytes, limits).is_err());
    }
}

#[test]
fn rejects_layout_dependent_native_preload_metadata() {
    let limits = PakLimits::default();
    for name in ["__preload_section.pre", "__PRELOAD_SECTION.PRE"] {
        let bytes = legacy_zip(&[stored(
            name,
            b"offsets and remap indexes depend on ZIP layout",
        )]);
        let error = PakArchive::parse(&bytes, limits).unwrap_err();
        assert!(error.to_string().contains("native preload metadata"));
        let archive = PakArchive {
            entries: vec![PakEntry {
                name: name.into(),
                data: vec![],
            }],
        };
        assert!(
            archive
                .to_bytes(limits)
                .unwrap_err()
                .to_string()
                .contains("native preload metadata")
        );
        let mut bsp = Bsp::new();
        bsp.lumps[lump::PAKFILE].data = bytes;
        assert!(replace_bsp_pak(&bsp.to_bytes(), &PakArchive::default(), limits).is_err());
    }
    let ordinary = legacy_zip(&[stored("ordinary/__preload_section.pre", b"opaque asset")]);
    assert!(PakArchive::parse(&ordinary, limits).is_ok());
}

#[test]
fn rejects_bad_offsets_flags_zip64_headers_crc_and_truncation() {
    let limits = PakLimits::default();
    let original = legacy_zip(&[stored("asset", b"hello")]);
    for end in 0..original.len() {
        assert!(
            PakArchive::parse(&original[..end], limits).is_err(),
            "prefix {end}"
        );
    }
    let directory = get32(&original, original.len() - 6);
    for (offset, value) in [
        (directory + 20, u32::MAX),
        (directory + 24, u32::MAX),
        (directory + 42, u32::MAX),
        (original.len() - 6, u32::MAX),
        (directory + 42, directory as u32),
    ] {
        let mut bytes = original.clone();
        put32(&mut bytes, offset, value);
        assert!(
            PakArchive::parse(&bytes, limits).is_err(),
            "offset {offset}"
        );
    }
    for flags in [1, 0x40, 0x2000] {
        let mut bytes = original.clone();
        put16(&mut bytes, 6, flags);
        put16(&mut bytes, directory + 8, flags);
        assert!(PakArchive::parse(&bytes, limits).is_err());
    }
    for (local, central, value) in [(8, directory + 10, 14), (4, directory + 6, 45)] {
        let mut bytes = original.clone();
        put16(&mut bytes, local, value);
        put16(&mut bytes, central, value);
        assert!(PakArchive::parse(&bytes, limits).is_err());
    }
    for offset in [14, 30, 35] {
        let mut bytes = original.clone();
        bytes[offset] ^= 1;
        assert!(PakArchive::parse(&bytes, limits).is_err());
    }
    let mut wrong_crc = original.clone();
    put32(&mut wrong_crc, 14, 0);
    put32(&mut wrong_crc, directory + 16, 0);
    assert!(PakArchive::parse(&wrong_crc, limits).is_err());
    let mut bytes = original.clone();
    bytes.push(0);
    assert!(PakArchive::parse(&bytes, limits).is_err());
    for descriptor in [false, true] {
        let mut bytes = legacy_zip(&[File {
            name: b"a",
            data: b"x",
            deflate: None,
            descriptor: Some(descriptor),
        }]);
        bytes[32 + if descriptor { 4 } else { 0 }] ^= 1;
        assert!(PakArchive::parse(&bytes, limits).is_err());
    }
}

#[test]
fn reads_embedded_zip_signatures_as_asset_bytes_without_footer_fallback() {
    let mut payload = b"opaque existing asset".to_vec();
    let mut alternate = [0; 22];
    put32(&mut alternate, 0, 0x0605_4b50);
    put16(&mut alternate, 8, u16::MAX);
    put16(&mut alternate, 10, u16::MAX);
    put32(&mut alternate, 12, u32::MAX);
    put32(&mut alternate, 16, u32::MAX);
    payload.extend(alternate);
    payload.extend(b"PK\x06\x06PK\x06\x07");
    let bytes = legacy_zip(&[stored("unknown/embedded.zip.bin", &payload)]);
    assert_eq!(
        PakArchive::parse(&bytes, PakLimits::default())
            .unwrap()
            .entries[0]
            .data,
        payload
    );
    let mut malformed = bytes;
    put32(&mut malformed, 14, 1);
    let central = get32(&malformed, malformed.len() - 6);
    put32(&mut malformed, central + 16, 1);
    assert!(PakArchive::parse(&malformed, PakLimits::default()).is_err());
}

#[test]
fn rejects_overlapping_entries_even_when_their_headers_match() {
    let limits = PakLimits::default();
    let inner = legacy_zip(&[stored("inner", b"x")]);
    let end = get32(&inner, inner.len() - 6);
    let mut bytes = legacy_zip(&[stored("outer", &inner[..end]), stored("inner", b"x")]);
    let central = get32(&bytes, bytes.len() - 6);
    put32(&mut bytes, central + 46 + 5 + 42, 35);
    let error = PakArchive::parse(&bytes, limits).unwrap_err();
    assert!(error.to_string().contains("overlap"), "{error}");
}

#[test]
fn enforces_encoded_entry_decoded_name_and_count_budgets() {
    let archive = archive();
    let bytes = archive.to_bytes(PakLimits::default()).unwrap();
    let boundary = PakLimits {
        max_archive_bytes: bytes.len(),
        max_entries: archive.entries.len(),
        max_entry_bytes: 5,
        max_total_bytes: 8,
        max_name_bytes: archive
            .entries
            .iter()
            .map(|entry| entry.name.len())
            .max()
            .unwrap(),
        ..PakLimits::default()
    };
    assert!(archive.to_bytes(boundary).is_ok());
    assert!(PakArchive::parse(&bytes, boundary).is_ok());
    for limits in [
        PakLimits {
            max_archive_bytes: boundary.max_archive_bytes - 1,
            ..boundary
        },
        PakLimits {
            max_entries: boundary.max_entries - 1,
            ..boundary
        },
        PakLimits {
            max_entry_bytes: boundary.max_entry_bytes - 1,
            ..boundary
        },
        PakLimits {
            max_total_bytes: boundary.max_total_bytes - 1,
            ..boundary
        },
        PakLimits {
            max_name_bytes: boundary.max_name_bytes - 1,
            ..boundary
        },
    ] {
        assert!(archive.to_bytes(limits).is_err(), "{limits:?}");
        assert!(PakArchive::parse(&bytes, limits).is_err(), "{limits:?}");
    }
    for limits in [
        PakLimits {
            max_archive_bytes: usize::MAX,
            ..boundary
        },
        PakLimits {
            max_entries: usize::MAX,
            ..boundary
        },
        PakLimits {
            max_entry_bytes: usize::MAX,
            ..boundary
        },
        PakLimits {
            max_total_bytes: usize::MAX,
            ..boundary
        },
        PakLimits {
            max_name_bytes: usize::MAX,
            ..boundary
        },
        PakLimits {
            max_bsp_bytes: usize::MAX,
            ..boundary
        },
    ] {
        assert!(PakArchive::default().to_bytes(limits).is_err());
        assert!(PakArchive::parse(&legacy_zip(&[]), limits).is_err());
    }
    let empty_only = PakLimits {
        max_entries: 0,
        max_entry_bytes: 0,
        max_total_bytes: 0,
        max_name_bytes: 0,
        max_archive_bytes: 22,
        ..boundary
    };
    assert!(PakArchive::default().to_bytes(empty_only).is_ok());
    assert!(PakArchive::parse(&legacy_zip(&[]), empty_only).is_ok());
}

#[test]
fn bounds_deflate_output_even_when_advertised_sizes_agree() {
    let original = legacy_zip(&[File {
        name: b"hello",
        data: b"hello",
        deflate: Some(&[0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]),
        descriptor: None,
    }]);
    let central = get32(&original, original.len() - 6);
    for advertised in [0, 4, 6, u32::MAX] {
        let mut bytes = original.clone();
        put32(&mut bytes, 22, advertised);
        put32(&mut bytes, central + 24, advertised);
        assert!(PakArchive::parse(&bytes, PakLimits::default()).is_err());
    }
    let limits = PakLimits {
        max_entry_bytes: 4,
        max_total_bytes: 4,
        ..PakLimits::default()
    };
    assert!(PakArchive::parse(&original, limits).is_err());
}

#[test]
fn rejects_truncated_deflate_and_trailing_junk_or_another_stream() {
    let stream = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];
    for end in 0..stream.len() {
        let bytes = legacy_zip(&[File {
            name: b"hello",
            data: b"hello",
            deflate: Some(&stream[..end]),
            descriptor: None,
        }]);
        assert!(
            PakArchive::parse(&bytes, PakLimits::default()).is_err(),
            "prefix {end}"
        );
    }
    for suffix in [b"junk".as_slice(), &stream] {
        let mut encoded = stream.to_vec();
        encoded.extend(suffix);
        let bytes = legacy_zip(&[File {
            name: b"hello",
            data: b"hello",
            deflate: Some(&encoded),
            descriptor: None,
        }]);
        assert!(PakArchive::parse(&bytes, PakLimits::default()).is_err());
    }
    for encoded in [&[3, 0][..], &[1, 0, 0, 255, 255][..]] {
        let bytes = legacy_zip(&[File {
            name: b"empty",
            data: b"",
            deflate: Some(encoded),
            descriptor: None,
        }]);
        assert!(
            PakArchive::parse(&bytes, PakLimits::default())
                .unwrap()
                .entries[0]
                .data
                .is_empty()
        );
    }
}

#[test]
fn rejects_zip64_aes_and_truncated_extra_fields_before_decoding() {
    for extra in [
        vec![1, 0, 0, 0],
        vec![1, 0x99, 0, 0],
        vec![2, 0, 2, 0, 0],
        vec![0],
    ] {
        let original = legacy_zip(&[stored("a", b"x")]);
        let central = get32(&original, original.len() - 6);
        let mut local = original.clone();
        local.splice(31..31, extra.iter().copied());
        put16(&mut local, 28, extra.len() as u16);
        let end = local.len() - 22;
        put32(&mut local, end + 16, (central + extra.len()) as u32);
        assert!(PakArchive::parse(&local, PakLimits::default()).is_err());
        let mut directory = original;
        directory.splice(central + 47..central + 47, extra.iter().copied());
        put16(&mut directory, central + 30, extra.len() as u16);
        let end = directory.len() - 22;
        put32(&mut directory, end + 12, (47 + extra.len()) as u32);
        assert!(PakArchive::parse(&directory, PakLimits::default()).is_err());
    }
}

fn bsp_fixture() -> Vec<u8> {
    let mut bsp = Bsp::new();
    bsp.map_revision = 71;
    bsp.lumps[lump::ENTITIES].data = b"{\n\"classname\" \"worldspawn\"\n}\0".to_vec();
    bsp.lumps[lump::LIGHTING].data = b"LZMA\x0f\0\0\0opaque compressed lighting".to_vec();
    bsp.lumps[lump::LIGHTING].fourcc = 15u32.to_le_bytes();
    bsp.lumps[63].version = 92;
    bsp.lumps[63].data = b"unknown lump bytes".to_vec();
    bsp.game_lumps.push(GameLump {
        id: *b"sprp",
        flags: 0,
        version: 6,
        data: b"opaque static props with absolute offset".to_vec(),
    });
    bsp.game_lumps.push(GameLump {
        id: *b"test",
        flags: 1,
        version: 19,
        data: vec![1, 3, 5, 7],
    });
    bsp.lumps[lump::PAKFILE].data = legacy_zip(&[stored("unknown/preserve.txt", b"hello")]);
    let mut bytes = bsp.to_bytes();
    bytes.extend(b"retained trailing data\0");
    bytes
}

#[test]
fn append_only_replacement_preserves_non_pak_bytes_and_absolute_game_offsets() {
    let original = bsp_fixture();
    let header = 8 + lump::PAKFILE * 16;
    let replacement = archive();
    let replaced = replace_bsp_pak(&original, &replacement, PakLimits::default()).unwrap();
    assert_eq!(&replaced[..header], &original[..header]);
    assert_eq!(
        &replaced[header + 16..original.len()],
        &original[header + 16..]
    );
    let start = get32(&replaced, header);
    assert_eq!(start % 4, 0);
    assert_eq!(start, original.len().next_multiple_of(4));
    assert_eq!(get32(&replaced, header + 4), replaced.len() - start);
    assert!(
        replaced[original.len()..start]
            .iter()
            .all(|&byte| byte == 0)
    );
    assert_eq!(&replaced[header + 8..header + 16], &[0; 8]);
    let before = Bsp::parse(&original).unwrap();
    let after = Bsp::parse(&replaced).unwrap();
    assert_eq!(after.game_lumps, before.game_lumps);
    assert_eq!(after.map_revision, before.map_revision);
    for index in 0..64 {
        if index != lump::PAKFILE {
            assert_eq!(after.lumps[index], before.lumps[index], "lump {index}");
        }
    }
    let mut expected = replacement;
    expected.entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(after.pak_data(PakLimits::default()).unwrap(), expected);
    for version in [19, 21] {
        let mut bytes = original.clone();
        put32(&mut bytes, 4, version);
        assert!(replace_bsp_pak(&bytes, &archive(), PakLimits::default()).is_ok());
    }
}

#[test]
fn replacement_checks_outer_spans_old_pak_and_retained_byte_budget() {
    let original = bsp_fixture();
    let limits = PakLimits::default();
    let replacement = archive();
    let needed = replace_bsp_pak(&original, &replacement, limits)
        .unwrap()
        .len();
    assert!(
        replace_bsp_pak(
            &original,
            &replacement,
            PakLimits {
                max_bsp_bytes: needed,
                ..limits
            }
        )
        .is_ok()
    );
    for budget in [original.len() - 1, original.len(), needed - 1] {
        assert!(
            replace_bsp_pak(
                &original,
                &replacement,
                PakLimits {
                    max_bsp_bytes: budget,
                    ..limits
                }
            )
            .is_err()
        );
    }
    for end in [0, 4, 1035] {
        assert!(replace_bsp_pak(&original[..end], &replacement, limits).is_err());
    }
    let pak_header = 8 + lump::PAKFILE * 16;
    let entity_offset = get32(&original, 8);
    for (offset, value) in [
        (4, 22),
        (8, u32::MAX),
        (12, u32::MAX),
        (8, 8),
        (12, original.len() as u32),
        (8 + 63 * 16, entity_offset as u32),
        (pak_header + 8, 1),
        (pak_header + 12, 1),
    ] {
        let mut bytes = original.clone();
        put32(&mut bytes, offset, value);
        assert!(
            replace_bsp_pak(&bytes, &replacement, limits).is_err(),
            "header {offset}"
        );
    }
    let mut corrupt_old_pak = original.clone();
    let old_pak = get32(&original, pak_header);
    corrupt_old_pak[old_pak + 30 + "unknown/preserve.txt".len()] ^= 1;
    assert!(replace_bsp_pak(&corrupt_old_pak, &replacement, limits).is_err());
    let mut absent = Bsp::new();
    assert!(absent.pak_data(limits).unwrap().entries.is_empty());
    assert!(replace_bsp_pak(&absent.to_bytes(), &replacement, limits).is_ok());
    absent.lumps.pop();
    assert!(absent.set_pak_data(&replacement, limits).is_err());
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancel this PAK operation")
}

fn raw_stored_deflate(data: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    let chunks = data.chunks(16 * 1024);
    let count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        encoded.push(u8::from(index + 1 == count));
        let length = chunk.len() as u16;
        encoded.extend(length.to_le_bytes());
        encoded.extend((!length).to_le_bytes());
        encoded.extend(chunk);
    }
    encoded
}

#[test]
fn controlled_codec_propagates_cancellation_during_headers_and_chunk_io() {
    let archive = PakArchive {
        entries: vec![PakEntry {
            name: "sound/large.wav".into(),
            data: vec![0xa5; 96 * 1024],
        }],
    };
    let limits = PakLimits::default();
    let mut write_steps = 0;
    let bytes = archive
        .to_bytes_controlled(limits, &mut || {
            write_steps += 1;
            Ok(())
        })
        .unwrap();
    for stop in 1..=write_steps {
        let mut step = 0;
        let error = archive
            .to_bytes_controlled(limits, &mut || {
                step += 1;
                if step >= stop {
                    Err(cancelled())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(
            error.kind(),
            io::ErrorKind::Interrupted,
            "write step {stop}"
        );
        assert_eq!(error.to_string(), "cancel this PAK operation");
    }
    let deflate = raw_stored_deflate(&archive.entries[0].data);
    let compressed = legacy_zip(&[File {
        name: archive.entries[0].name.as_bytes(),
        data: &archive.entries[0].data,
        deflate: Some(&deflate),
        descriptor: Some(true),
    }]);
    for bytes in [bytes, compressed] {
        let mut read_steps = 0;
        PakArchive::parse_controlled(&bytes, limits, &mut || {
            read_steps += 1;
            Ok(())
        })
        .unwrap();
        for stop in 1..=read_steps {
            let mut step = 0;
            let error = PakArchive::parse_controlled(&bytes, limits, &mut || {
                step += 1;
                if step >= stop {
                    Err(cancelled())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Interrupted, "read step {stop}");
        }
    }
}

#[test]
fn cancelled_setter_and_raw_replacement_leave_original_state_unchanged() {
    let bytes = bsp_fixture();
    let original = Bsp::parse(&bytes).unwrap();
    let archive = archive();
    let limits = PakLimits::default();
    let mut steps = 0;
    let mut measured = original.clone();
    measured
        .set_pak_data_controlled(&archive, limits, &mut || {
            steps += 1;
            Ok(())
        })
        .unwrap();
    for stop in 1..=steps {
        let mut bsp = original.clone();
        let mut step = 0;
        let error = bsp
            .set_pak_data_controlled(&archive, limits, &mut || {
                step += 1;
                if step >= stop {
                    Err(cancelled())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(bsp, original);
    }
    let before = bytes.clone();
    let mut steps = 0;
    replace_bsp_pak_controlled(&bytes, &archive, limits, &mut || {
        steps += 1;
        Ok(())
    })
    .unwrap();
    for stop in [1, steps / 2, steps] {
        let mut step = 0;
        let error = replace_bsp_pak_controlled(&bytes, &archive, limits, &mut || {
            step += 1;
            if step >= stop {
                Err(cancelled())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(bytes, before);
    }
}
