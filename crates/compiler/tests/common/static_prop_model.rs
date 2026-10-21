fn int(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn model() -> [Vec<u8>; 3] {
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
        (580, -160),
        (584, 4),
        (628, 4),
    ] {
        int(&mut mdl, offset, value);
    }
    for offset in [104, 128] {
        for axis in 0..3 {
            float(&mut mdl, offset + axis * 4, -8.0);
            float(&mut mdl, offset + 12 + axis * 4, 8.0);
        }
    }
    mdl[390..392].copy_from_slice(&1_u16.to_le_bytes());
    for (offset, value) in [(720, "one"), (736, "two"), (752, "models/test/")] {
        mdl[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    }
    let mut vvd = vec![0; 256];
    vvd[..4].copy_from_slice(b"IDSV");
    for (offset, value) in [(4, 4), (8, 7), (12, 1), (16, 4), (56, 64)] {
        int(&mut vvd, offset, value);
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
        for (axis, value) in point.into_iter().enumerate() {
            float(&mut vvd, 64 + index * 48 + 16 + axis * 4, value);
        }
        float(&mut vvd, 64 + index * 48 + 36, 1.0);
    }
    let mut vtx = vec![0; 349];
    for (offset, value) in [
        (0, 7),
        (16, 7),
        (20, 1),
        (24, 173),
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
        (73, 6),
        (77, 183),
        (81, 3),
        (85, 237),
        (89, 1),
        (93, 249),
        (322, 3),
        (330, 6),
    ] {
        int(&mut vtx, offset, value);
    }
    vtx[97] = 2;
    vtx[340] = 1;
    for (index, original) in [2_u16, 0, 2, 1, 3, 3].into_iter().enumerate() {
        vtx[260 + index * 9..262 + index * 9].copy_from_slice(&original.to_le_bytes());
    }
    for (index, vertex) in [1_u16, 3, 0].into_iter().enumerate() {
        vtx[310 + index * 2..312 + index * 2].copy_from_slice(&vertex.to_le_bytes());
    }
    [mdl, vvd, vtx]
}
