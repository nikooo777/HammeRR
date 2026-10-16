use std::path::Path;

fn integer(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn write(directory: &Path, name: &str) {
    let mut mdl = vec![0; 1024];
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
        integer(&mut mdl, offset, value);
    }
    for offset in [116, 140] {
        float(&mut mdl, offset, 1.0);
        float(&mut mdl, offset + 4, 1.0);
    }
    mdl[390..392].copy_from_slice(&1u16.to_le_bytes());
    for (offset, text) in [(720, "one"), (736, "two"), (752, "models/test/")] {
        mdl[offset..offset + text.len()].copy_from_slice(text.as_bytes());
    }
    let mut vvd = vec![0; 256];
    vvd[..4].copy_from_slice(b"IDSV");
    for (offset, value) in [(4, 4), (8, 7), (12, 1), (16, 4), (56, 64)] {
        integer(&mut vvd, offset, value);
    }
    for (index, point) in [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ]
    .into_iter()
    .enumerate()
    {
        let offset = 64 + index * 48;
        for (axis, value) in point.into_iter().enumerate() {
            float(&mut vvd, offset + 16 + axis * 4, value);
        }
        float(&mut vvd, offset + 36, 1.0);
        float(&mut vvd, offset + 40, point[0]);
        float(&mut vvd, offset + 44, point[1]);
    }
    let mut vtx = vec![0; 173];
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
        integer(&mut vtx, offset, value);
    }
    for index in 0..4 {
        vtx[98 + index * 9 + 4..98 + index * 9 + 6].copy_from_slice(&(index as u16).to_le_bytes());
    }
    for (index, value) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
        vtx[134 + index * 2..136 + index * 2].copy_from_slice(&value.to_le_bytes());
    }
    vtx[164] = 1;
    std::fs::create_dir_all(directory.join("models/props")).unwrap();
    std::fs::create_dir_all(directory.join("materials/models/test")).unwrap();
    for (extension, data) in [("mdl", mdl), ("vvd", vvd), ("dx90.vtx", vtx)] {
        std::fs::write(
            directory.join(format!("models/props/{name}.{extension}")),
            data,
        )
        .unwrap();
    }
    for material in ["one", "two"] {
        std::fs::write(
            directory.join(format!("materials/models/test/{material}.vmt")),
            "UnlitGeneric { }",
        )
        .unwrap();
    }
}
