use source_formats::bsp::{
    Bsp, CollisionHeader, MAX_PHYSICS_KEY_BYTES, MAX_PHYSICS_SOLIDS, Model, PhysicsCollision,
    PhysicsModel, PhysicsSolid, lump,
};

fn blob(version: u16, kind: u16) -> Vec<u8> {
    let header_size = if kind == 0 { 28 } else { 12 };
    let mut bytes = vec![0; header_size + 4];
    bytes[..4].copy_from_slice(b"VPHY");
    bytes[4..6].copy_from_slice(&version.to_le_bytes());
    bytes[6..8].copy_from_slice(&kind.to_le_bytes());
    bytes[8..12].copy_from_slice(&4_u32.to_le_bytes());
    bytes[header_size..].copy_from_slice(&[1, 2, 3, 4]);
    bytes
}

fn fixture() -> Vec<u8> {
    let convex = blob(0x100, 0);
    let terrain = blob(0x100, 1);
    PhysicsCollision {
        models: vec![PhysicsModel {
            index: 0,
            solids: vec![
                PhysicsSolid::parse(&convex).unwrap(),
                PhysicsSolid::parse(&terrain).unwrap(),
            ],
            key_data: b"staticsolid { index 0 contents 1 }\0",
        }],
    }
    .to_bytes(1)
    .unwrap()
}

fn replace(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn checked_physics_framing_round_trips_known_and_opaque_body_types() {
    let mut convex = blob(0x100, 0);
    convex[12..16].copy_from_slice(&0.25_f32.to_le_bytes());
    convex[24..28].copy_from_slice(&17_u32.to_le_bytes());
    let terrain = blob(0x100, 1);
    let future_kind = blob(0x100, 2);
    let future_version = blob(0x101, 0);
    let solids: Vec<_> = [&convex, &terrain, &future_kind, &future_version]
        .into_iter()
        .map(|bytes| PhysicsSolid::parse(bytes).unwrap())
        .collect();
    assert_eq!(
        solids[0].header(),
        CollisionHeader::Convex {
            orthographic_areas: [0.25, 0.0, 0.0],
            reserved: 17,
        }
    );
    assert_eq!(solids[1].header(), CollisionHeader::Null);
    assert_eq!(solids[0].body(), Some([1, 2, 3, 4].as_slice()));
    assert_eq!(solids[1].body(), Some([1, 2, 3, 4].as_slice()));
    assert_eq!(
        solids[2].header(),
        CollisionHeader::Unsupported {
            version: 0x100,
            kind: 2
        }
    );
    assert!(solids[2].body().is_none() && solids[3].body().is_none());
    let physics = PhysicsCollision {
        models: vec![
            PhysicsModel {
                index: 0,
                solids,
                key_data: b"staticsolid {}\0",
            },
            PhysicsModel {
                index: 2,
                solids: vec![],
                key_data: b"\xff\0",
            },
        ],
    };
    let mut bsp = Bsp::new();
    bsp.set_records(&[Model::default(); 3]);
    bsp.set_physics(&physics).unwrap();
    let again = Bsp::parse(&bsp.to_bytes()).unwrap();
    assert_eq!(again.physics().unwrap().unwrap(), physics);
    let bytes = physics.to_bytes(3).unwrap();
    assert_eq!(
        &bytes[bytes.len() - 16..],
        &[-1_i32, -1, 0, 0].map(i32::to_le_bytes).concat()
    );
    assert_eq!(
        PhysicsCollision::parse(&bytes, 3)
            .unwrap()
            .to_bytes(3)
            .unwrap(),
        bytes
    );
    assert!(PhysicsCollision::parse(&bytes, 2).is_err());
}

#[test]
fn malformed_physics_sizes_counts_references_and_end_markers_are_rejected() {
    let bytes = fixture();
    for length in 0..bytes.len() {
        assert!(
            PhysicsCollision::parse(&bytes[..length], 1).is_err(),
            "length {length}"
        );
    }
    for (offset, value) in [
        (0, -1),
        (0, 1),
        (0, i32::MAX),
        (4, -1),
        (4, 0),
        (4, i32::MAX),
        (8, -1),
        (8, 0),
        (8, MAX_PHYSICS_KEY_BYTES as i32 + 1),
        (12, -1),
        (12, 0),
        (12, MAX_PHYSICS_SOLIDS as i32 + 1),
        (16, -1),
        (16, 12),
        (16, i32::MAX),
        (28, 0),
        (28, i32::MAX),
        (bytes.len() - 12, 0),
    ] {
        let mut corrupt = bytes.clone();
        replace(&mut corrupt, offset, value);
        assert!(
            PhysicsCollision::parse(&corrupt, 1).is_err(),
            "{offset}: {value}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(PhysicsCollision::parse(&trailing, 1).is_err());
    let mut duplicate = bytes[..bytes.len() - 16].to_vec();
    duplicate.extend_from_slice(&bytes);
    assert!(PhysicsCollision::parse(&duplicate, 2).is_err());
    for kind in [0, 1] {
        let mut bytes = blob(0x100, kind);
        bytes[0] = 0;
        assert!(PhysicsSolid::parse(&bytes).is_err());
        bytes[0] = b'V';
        replace(&mut bytes, 8, -1);
        assert!(PhysicsSolid::parse(&bytes).is_err());
    }
    let mut bad_keys = bytes.clone();
    bad_keys[bytes.len() - 17] = 1;
    assert!(PhysicsCollision::parse(&bad_keys, 1).is_err());
    bad_keys = bytes.clone();
    bad_keys[72] = 0;
    assert!(PhysicsCollision::parse(&bad_keys, 1).is_err());
}

#[test]
fn rejected_physics_writes_preserve_previous_lump_and_unsupported_storage_fails() {
    let bytes = fixture();
    let physics = PhysicsCollision::parse(&bytes, 1).unwrap();
    let mut bsp = Bsp::new();
    assert!(bsp.physics().unwrap().is_none());
    bsp.set_records(&[Model::default()]);
    bsp.set_physics(&physics).unwrap();
    let before = bsp.lump(lump::PHYSCOLLIDE).clone();
    let mut invalid = physics.clone();
    invalid.models[0].key_data = b"unterminated";
    assert!(bsp.set_physics(&invalid).is_err());
    invalid = physics.clone();
    invalid.models[0].index = 1;
    assert!(bsp.set_physics(&invalid).is_err());
    invalid = physics.clone();
    invalid.models.push(invalid.models[0].clone());
    assert!(bsp.set_physics(&invalid).is_err());
    assert_eq!(bsp.lump(lump::PHYSCOLLIDE), &before);
    bsp.lumps[lump::PHYSCOLLIDE].version = 1;
    assert!(bsp.physics().is_err());
    bsp.lumps[lump::PHYSCOLLIDE] = before.clone();
    bsp.lumps[lump::PHYSCOLLIDE].fourcc = 4_u32.to_le_bytes();
    assert!(bsp.physics().is_err());
    bsp.lumps[lump::PHYSCOLLIDE] = before;
    bsp.lumps[lump::MODELS].data.push(0);
    assert!(bsp.physics().is_err());
    assert!(bsp.set_physics(&physics).is_err());
}
