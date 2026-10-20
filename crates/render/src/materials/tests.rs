use super::*;
use source_formats::gamefs::{Mount, MountedPath};

fn texture(width: u16, height: u16, pixel: [u8; 4]) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (at, value) in [(4, 7u32), (8, 2), (12, 80), (57, u32::MAX)] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[16..18].copy_from_slice(&width.to_le_bytes());
    bytes[18..20].copy_from_slice(&height.to_le_bytes());
    for at in [24, 63] {
        bytes[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    bytes[56] = 1;
    for _ in 0..usize::from(width) * usize::from(height) {
        bytes.extend(pixel);
    }
    bytes
}

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    for (name, width, height, pixel) in [
        ("raw", 2, 1, [210, 25, 15, 255]),
        ("albedo", 4, 2, [12, 170, 35, 96]),
        ("second", 3, 1, [20, 40, 220, 255]),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/{name}.vtf")),
            texture(width, height, pixel),
        )
        .unwrap();
    }
    let fs = GameFs {
        game_dir: directory.path().into(),
        base_dir: directory.path().into(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().into()),
        }],
    };
    (directory, fs)
}

fn write_material(directory: &std::path::Path, shader: &str, extra: &str) {
    std::fs::write(
        directory.join("materials/test.vmt"),
        format!("{shader} {{ $basetexture raw $bumpmap missing_normal $albedo albedo {extra} }}"),
    )
    .unwrap();
}

#[test]
fn effective_base_supplies_real_display_pixels_dimensions_and_retains_blend() {
    let (directory, fs) = fixture();
    for shader in [
        "LightmappedGeneric",
        "WorldVertexTransition",
        "WorldTwoTextureBlend",
    ] {
        write_material(directory.path(), shader, "$basetexture2 second");
        let material = MaterialInfo::load(&fs, "test");
        assert!(material.error.is_none() && !material.missing);
        assert_eq!(
            material.display_texture.as_deref(),
            Some("materials/albedo.vtf")
        );
        assert_eq!((material.width, material.height), (4, 2));
        let image = material
            .base
            .as_ref()
            .unwrap()
            .decode_rgba8(0, 0, 0)
            .unwrap();
        let (pixels, remainder) = image.pixels.as_chunks::<4>();
        assert_eq!(pixels.len(), 8);
        assert!(remainder.is_empty());
        assert!(pixels.iter().all(|pixel| *pixel == [12, 170, 35, 96]));
        if shader == "WorldVertexTransition" {
            let blend = material.blend.unwrap();
            assert_eq!((blend.width, blend.height), (3, 1));
            assert_eq!(
                &blend.decode_rgba8(0, 0, 0).unwrap().pixels[..4],
                &[20, 40, 220, 255]
            );
        }
        assert!(
            !directory
                .path()
                .join("materials/missing_normal.vtf")
                .exists()
        );
    }
}

#[test]
fn intentional_tool_display_priority_and_literal_suppression_remain_distinct() {
    let (directory, fs) = fixture();
    for (extra, expected, size) in [
        ("%tooltexture second", "materials/second.vtf", (3, 1)),
        ("%tooltexture missing_tool", "materials/albedo.vtf", (4, 2)),
        ("$nodiffusebumplighting 1", "materials/raw.vtf", (2, 1)),
    ] {
        write_material(directory.path(), "LightmappedGeneric", extra);
        let material = MaterialInfo::load(&fs, "test");
        assert!(material.error.is_none() && !material.missing);
        assert_eq!(material.display_texture.as_deref(), Some(expected));
        assert_eq!((material.width, material.height), size);
    }
}

#[test]
fn missing_or_malformed_selected_albedo_never_succeeds_with_existing_raw_base() {
    let (directory, fs) = fixture();
    write_material(directory.path(), "LightmappedGeneric", "");
    std::fs::remove_file(directory.path().join("materials/albedo.vtf")).unwrap();
    let missing = MaterialInfo::load(&fs, "test");
    assert!(missing.missing && missing.base.is_none());
    assert!(missing.error.unwrap().contains("selected $albedo"));
    let mut library = MaterialLibrary::default();
    assert!(library.load_limited(&fs, "test", [1024, 4096]).is_err());
    assert!(library.find("test").is_none());
    std::fs::write(directory.path().join("materials/albedo.vtf"), b"malformed").unwrap();
    let malformed = MaterialInfo::load(&fs, "test");
    assert!(malformed.missing && malformed.base.is_none());
    assert!(malformed.error.unwrap().contains("materials/albedo.vtf"));
    assert!(directory.path().join("materials/raw.vtf").exists());
}

#[test]
fn reload_keeps_ids_while_refreshing_effective_source_dimensions_and_identity() {
    let (directory, fs) = fixture();
    write_material(directory.path(), "LightmappedGeneric", "");
    let mut library = MaterialLibrary::default();
    let id = library.load(&fs, "test");
    let identity = library.get(id).source_identity();
    let generation = library.generation();
    write_material(
        directory.path(),
        "LightmappedGeneric",
        "$nodiffusebumplighting 1",
    );
    assert_eq!(
        library.get(id).display_texture.as_deref(),
        Some("materials/albedo.vtf")
    );
    assert_eq!(library.reload(&fs).failed, 0);
    assert_eq!(library.load(&fs, "test"), id);
    assert_ne!(library.generation(), generation);
    assert_ne!(library.get(id).source_identity(), identity);
    assert_eq!(
        library.get(id).display_texture.as_deref(),
        Some("materials/raw.vtf")
    );
    assert_eq!((library.get(id).width, library.get(id).height), (2, 1));
    write_material(directory.path(), "LightmappedGeneric", "");
    std::fs::remove_file(directory.path().join("materials/albedo.vtf")).unwrap();
    assert_eq!(library.reload(&fs).failed, 1);
    assert!(library.get(id).missing && library.get(id).base.is_none());
    std::fs::write(
        directory.path().join("materials/albedo.vtf"),
        texture(4, 2, [2, 3, 4, 255]),
    )
    .unwrap();
    assert_eq!(library.reload(&fs).failed, 0);
    assert_eq!(
        library.get(id).display_texture.as_deref(),
        Some("materials/albedo.vtf")
    );
    assert_eq!(
        &library
            .get(id)
            .base
            .as_ref()
            .unwrap()
            .decode_rgba8(0, 0, 0)
            .unwrap()
            .pixels[..4],
        &[2, 3, 4, 255]
    );
}
