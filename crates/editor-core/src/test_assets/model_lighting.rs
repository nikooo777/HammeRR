use super::*;

pub fn game() -> (tempfile::TempDir, GameFs) {
    let (directory, game) = super::game();
    let models = directory.path().join("models/test");
    let materials = directory.path().join("materials/models/test");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("one.vmt"),
        "VertexLitGeneric { $basetexture test/checker }",
    )
    .unwrap();
    std::fs::write(
        materials.join("two.vmt"),
        "VertexLitGeneric { $basetexture test/checker $phong 1 }",
    )
    .unwrap();
    let mut mdl = vec![0u8; 1024];
    mdl[..4].copy_from_slice(b"IDST");
    for (offset, value) in [
        (4, 48),
        (8, 7),
        (76, 1024),
        (152, 16),
        (204, 2),
        (208, 256),
        (212, 1),
        (216, 384),
        (220, 1),
        (224, 2),
        (228, 388),
        (232, 1),
        (236, 400),
        (256, 464),
        (320, 416),
        (384, 752),
        (404, 1),
        (408, 1),
        (412, 16),
        (488, 1),
        (492, 160),
        (496, 4),
        (500, 0),
        (576, 0),
        (580, -160),
        (584, 4),
        (588, 0),
    ] {
        mdl[offset..offset + 4].copy_from_slice(&i32::to_le_bytes(value));
    }
    for offset in [104, 108, 128, 132] {
        mdl[offset..offset + 4].copy_from_slice(&(-24f32).to_le_bytes());
    }
    for offset in [116, 120, 140, 144] {
        mdl[offset..offset + 4].copy_from_slice(&24f32.to_le_bytes());
    }
    mdl[390..392].copy_from_slice(&1u16.to_le_bytes());
    for (offset, value) in [(720, "one"), (736, "two"), (752, "models/test/")] {
        mdl[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    }
    let mut vvd = vec![0u8; 256];
    vvd[..4].copy_from_slice(b"IDSV");
    for (offset, value) in [(4, 4), (8, 7), (12, 1), (16, 4), (56, 64)] {
        vvd[offset..offset + 4].copy_from_slice(&i32::to_le_bytes(value));
    }
    for (index, point) in [
        [-24f32, -24., 0.],
        [24., -24., 0.],
        [24., 24., 0.],
        [-24., 24., 0.],
    ]
    .into_iter()
    .enumerate()
    {
        for (axis, value) in point.into_iter().enumerate() {
            let offset = 64 + index * 48 + 16 + axis * 4;
            vvd[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let offset = 64 + index * 48 + 36;
        vvd[offset..offset + 4].copy_from_slice(&1f32.to_le_bytes());
    }
    let mut vtx = vec![0u8; 173];
    for (offset, value) in [
        (0, 7),
        (16, 7),
        (20, 1),
        (28, 1),
        (32, 36),
        (36, 1),
        (40, 8),
        (44, 1),
        (48, 8),
        (52, 1),
        (56, 12),
        (64, 1),
        (68, 9),
        (73, 4),
        (77, 25),
        (81, 6),
        (85, 61),
        (89, 1),
        (93, 73),
        (146, 6),
        (150, 0),
        (154, 4),
        (158, 0),
    ] {
        vtx[offset..offset + 4].copy_from_slice(&i32::to_le_bytes(value));
    }
    for index in 0..4 {
        vtx[98 + index * 9 + 4..98 + index * 9 + 6].copy_from_slice(&(index as u16).to_le_bytes());
    }
    for (index, value) in [0u16, 2, 1, 0, 3, 2].into_iter().enumerate() {
        vtx[134 + index * 2..136 + index * 2].copy_from_slice(&value.to_le_bytes());
    }
    vtx[164] = 1;
    for (extension, data) in [("mdl", mdl), ("vvd", vvd), ("dx90.vtx", vtx)] {
        std::fs::write(models.join(format!("receiver.{extension}")), data).unwrap();
    }
    (directory, game)
}
