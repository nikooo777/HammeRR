use std::io;

use source_formats::vhv::{Bgra8, File, Limits, Mesh, MeshLayout, inspect};

fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 1024];
    bytes[..40].copy_from_slice(&[
        2, 0, 0, 0, 0x78, 0x56, 0x34, 0x92, 4, 0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    bytes[40..68].copy_from_slice(&[
        1, 0, 0, 0, 2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    bytes[68..96].copy_from_slice(&[
        0, 0, 0, 0, 1, 0, 0, 0, 8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    bytes[512..524].copy_from_slice(&[
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
    ]);
    bytes
}

fn file() -> File {
    File {
        checksum: 0x9234_5678,
        meshes: vec![
            Mesh {
                layout: MeshLayout {
                    lod: 1,
                    vertex_count: 2,
                },
                colors: vec![
                    Bgra8 {
                        blue: 0x11,
                        green: 0x22,
                        red: 0x33,
                        alpha: 0x44,
                    },
                    Bgra8 {
                        blue: 0x55,
                        green: 0x66,
                        red: 0x77,
                        alpha: 0x88,
                    },
                ],
            },
            Mesh {
                layout: MeshLayout {
                    lod: 0,
                    vertex_count: 1,
                },
                colors: vec![Bgra8 {
                    blue: 0x99,
                    green: 0xaa,
                    red: 0xbb,
                    alpha: 0xcc,
                }],
            },
        ],
    }
}

fn put_word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn assert_invalid(bytes: &[u8], limits: Limits) {
    let error = inspect(bytes, limits).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let error = File::parse(bytes, limits).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn literal_v2_header_descriptors_offsets_and_bgra_channels() {
    let bytes = fixture();
    let metadata = inspect(&bytes, Limits::default()).unwrap();
    assert_eq!(metadata.checksum, 0x9234_5678);
    assert_eq!(metadata.mesh_count, 2);
    assert_eq!(metadata.vertex_count, 3);
    assert_eq!(
        metadata.decoded_bytes,
        size_of::<File>() + 2 * size_of::<Mesh>() + 3 * size_of::<Bgra8>(),
    );
    assert_eq!(File::parse(&bytes, Limits::default()).unwrap(), file());
}

#[test]
fn writer_matches_literal_bytes_and_preserves_descriptor_order() {
    let source = file();
    assert_eq!(source.encoded_len(Limits::default()).unwrap(), 1024);
    let bytes = source.to_bytes(Limits::default()).unwrap();
    assert_eq!(bytes, fixture());
    assert_eq!(source.to_bytes(Limits::default()).unwrap(), bytes);
    source
        .validate_layout(
            0x9234_5678,
            &[
                MeshLayout {
                    lod: 1,
                    vertex_count: 2,
                },
                MeshLayout {
                    lod: 0,
                    vertex_count: 1,
                },
            ],
        )
        .unwrap();
}

#[test]
fn rejects_unsupported_header_fields_lods_and_unbounded_counts() {
    for (offset, value) in [
        (0, 1),
        (0, 3),
        (8, 0),
        (8, 5),
        (12, 3),
        (12, 8),
        (16, u32::MAX),
        (20, u32::MAX),
        (20, i32::MAX as u32),
        (40, 8),
        (40, u32::MAX),
        (44, u32::MAX),
    ] {
        let mut bytes = fixture();
        put_word(&mut bytes, offset, value);
        assert_invalid(&bytes, Limits::default());
    }
}

#[test]
fn rejects_reserved_words_and_nonzero_alignment_padding() {
    for offset in [24, 39, 52, 67, 80, 95, 96, 511, 524, 1023] {
        let mut bytes = fixture();
        bytes[offset] = 1;
        assert_invalid(&bytes, Limits::default());
    }
}

#[test]
fn rejects_overlapping_reordered_gapped_and_out_of_file_sections() {
    for (offset, value) in [
        (48, 40),
        (48, 511),
        (48, 516),
        (48, u32::MAX),
        (76, 512),
        (76, 516),
        (76, 524),
        (76, 1024),
        (76, u32::MAX),
    ] {
        let mut bytes = fixture();
        put_word(&mut bytes, offset, value);
        assert_invalid(&bytes, Limits::default());
    }
    for count in [2, 4] {
        let mut bytes = fixture();
        put_word(&mut bytes, 16, count);
        assert_invalid(&bytes, Limits::default());
    }
    let mut bytes = fixture();
    put_word(&mut bytes, 72, 127);
    put_word(&mut bytes, 16, 129);
    assert_invalid(&bytes, Limits::default());
}

#[test]
fn rejects_truncated_headers_payloads_and_missing_or_extra_sector_padding() {
    let bytes = fixture();
    for length in [
        0, 3, 20, 39, 40, 67, 68, 95, 96, 511, 512, 519, 520, 523, 524, 1023,
    ] {
        assert_invalid(&bytes[..length], Limits::default());
    }
    for length in [1025, 1536] {
        let mut extended = bytes.clone();
        extended.resize(length, 0);
        assert_invalid(&extended, Limits::default());
    }
}

#[test]
fn read_and_write_admit_exact_budgets_and_reject_each_excess() {
    let exact = Limits {
        max_bytes: 1024,
        max_meshes: 2,
        max_vertices: 3,
    };
    let bytes = fixture();
    inspect(&bytes, exact).unwrap();
    assert_eq!(File::parse(&bytes, exact).unwrap(), file());
    assert_eq!(file().encoded_len(exact).unwrap(), 1024);
    assert_eq!(file().to_bytes(exact).unwrap(), bytes);
    for limits in [
        Limits {
            max_bytes: 1023,
            ..exact
        },
        Limits {
            max_meshes: 1,
            ..exact
        },
        Limits {
            max_vertices: 2,
            ..exact
        },
        Limits {
            max_meshes: i32::MAX as usize + 1,
            ..exact
        },
    ] {
        assert_invalid(&bytes, limits);
        assert!(file().encoded_len(limits).is_err());
        assert!(file().to_bytes(limits).is_err());
    }
    let mut bytes = bytes;
    put_word(&mut bytes, 16, 2);
    let error = inspect(
        &bytes,
        Limits {
            max_vertices: 2,
            ..exact
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("aggregate vertex count"));
}

#[test]
fn writer_rejects_missing_colors_and_unsupported_lods_before_sizing() {
    let mut missing = file();
    missing.meshes[0].colors.pop();
    let mut extra = file();
    extra.meshes[0].colors.push(Bgra8::default());
    let mut bad_lod = file();
    bad_lod.meshes[0].layout.lod = 8;
    for invalid in [missing, extra, bad_lod] {
        assert!(invalid.encoded_len(Limits::default()).is_err());
        assert!(invalid.to_bytes(Limits::default()).is_err());
    }
}

#[test]
fn expected_layout_requires_checksum_complete_count_and_exact_order() {
    let source = file();
    let expected: Vec<_> = source.meshes.iter().map(|mesh| mesh.layout).collect();
    assert!(
        source
            .validate_layout(source.checksum ^ 1, &expected)
            .is_err()
    );
    assert!(
        source
            .validate_layout(source.checksum, &expected[..1])
            .is_err()
    );
    assert!(
        source
            .validate_layout(source.checksum, &[expected[1], expected[0]])
            .is_err()
    );
    let mut count_mismatch = expected.clone();
    count_mismatch[0].vertex_count += 1;
    assert!(
        source
            .validate_layout(source.checksum, &count_mismatch)
            .is_err()
    );
    let mut incomplete = source;
    incomplete.meshes[0].colors.pop();
    assert!(
        incomplete
            .validate_layout(incomplete.checksum, &expected)
            .is_err()
    );
}

#[test]
fn sector_boundaries_depend_on_directory_and_total_payload_not_each_mesh() {
    let color = Bgra8 {
        blue: 1,
        green: 2,
        red: 3,
        alpha: 4,
    };
    for (vertices, expected_bytes) in [(128, 1024), (129, 1536)] {
        let source = File {
            checksum: 0,
            meshes: vec![Mesh {
                layout: MeshLayout {
                    lod: 0,
                    vertex_count: vertices,
                },
                colors: vec![color; vertices as usize],
            }],
        };
        let bytes = source.to_bytes(Limits::default()).unwrap();
        assert_eq!(bytes.len(), expected_bytes);
        assert_eq!(&bytes[48..52], &[0, 2, 0, 0]);
        inspect(&bytes, Limits::default()).unwrap();
    }
    let source = File {
        checksum: 0,
        meshes: vec![
            Mesh {
                layout: MeshLayout {
                    lod: 7,
                    vertex_count: 0
                },
                colors: Vec::new(),
            };
            17
        ],
    };
    let bytes = source.to_bytes(Limits::default()).unwrap();
    assert_eq!(bytes.len(), 1024);
    for index in 0..17 {
        assert_eq!(&bytes[48 + index * 28..52 + index * 28], &[0, 4, 0, 0]);
    }
    assert_eq!(File::parse(&bytes, Limits::default()).unwrap(), source);
}

#[test]
fn empty_file_has_one_zero_padded_sector_and_zero_counts() {
    let source = File {
        checksum: 0,
        meshes: Vec::new(),
    };
    let limits = Limits {
        max_bytes: 512,
        max_meshes: 0,
        max_vertices: 0,
    };
    let bytes = source.to_bytes(limits).unwrap();
    let mut literal = vec![0; 512];
    literal[0] = 2;
    literal[8] = 4;
    literal[12] = 4;
    assert_eq!(bytes, literal);
    assert_eq!(File::parse(&literal, limits).unwrap(), source);
}
