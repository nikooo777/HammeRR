use source_formats::gamefs::{GameFs, Mount, MountedPath};

pub fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/test");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("checker.vmt"),
        "LightmappedGeneric { \"$basetexture\" \"test/checker\" }",
    )
    .unwrap();
    let mut image = vec![0u8; 80];
    image[..4].copy_from_slice(b"VTF\0");
    image[4..8].copy_from_slice(&7u32.to_le_bytes());
    image[8..12].copy_from_slice(&2u32.to_le_bytes());
    image[12..16].copy_from_slice(&80u32.to_le_bytes());
    image[16..18].copy_from_slice(&64u16.to_le_bytes());
    image[18..20].copy_from_slice(&32u16.to_le_bytes());
    image[24..26].copy_from_slice(&1u16.to_le_bytes());
    image[56] = 1;
    image[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    image[63..65].copy_from_slice(&1u16.to_le_bytes());
    for y in 0..32 {
        for x in 0..64 {
            image.extend_from_slice(if (x / 8 + y / 8) % 2 == 0 {
                &[72, 135, 175, 255]
            } else {
                &[210, 225, 234, 255]
            });
        }
    }
    std::fs::write(materials.join("checker.vtf"), image).unwrap();
    let game = GameFs {
        game_dir: directory.path().to_owned(),
        base_dir: directory.path().to_owned(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        }],
    };
    (directory, game)
}
