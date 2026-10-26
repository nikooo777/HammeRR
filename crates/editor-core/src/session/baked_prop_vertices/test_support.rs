use glam::DVec3;
use map_core::{Document, edit};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Entity, Vmf};

fn int(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn fixture() -> (tempfile::TempDir, GameFs, Vmf) {
    let (directory, game) = crate::test_assets::model_lighting::game();
    std::fs::create_dir(directory.path().join("maps")).unwrap();
    let mdl_path = directory.path().join("models/test/receiver.mdl");
    let mut mdl = std::fs::read(&mdl_path).unwrap();
    int(&mut mdl, 628, 4);
    int(&mut mdl, 632, 4);
    int(&mut mdl, 212, 2);
    int(&mut mdl, 216, 768);
    int(&mut mdl, 768, 780);
    int(&mut mdl, 772, 752);
    mdl[780..796].copy_from_slice(b"models/missing/\0");
    std::fs::write(mdl_path, mdl).unwrap();
    let vvd_path = directory.path().join("models/test/receiver.vvd");
    let mut vvd = std::fs::read(&vvd_path).unwrap();
    int(&mut vvd, 12, 2);
    int(&mut vvd, 20, 4);
    std::fs::write(vvd_path, vvd).unwrap();
    let mut vtx = vec![0u8; 3025];
    for (offset, value) in [
        (0, 7),
        (16, 7),
        (20, 2),
        (24, 160),
        (28, 1),
        (32, 36),
        (36, 1),
        (40, 8),
        (44, 2),
        (48, 8),
        (52, 1),
        (56, 28),
        (64, 1),
        (68, 25),
        (80, 1),
        (84, 28),
        (89, 1),
        (93, 44),
        (108, 300),
        (112, 84),
        (116, 6),
        (120, 2784),
        (124, 1),
        (128, 2796),
        (133, 6),
        (137, 2799),
        (141, 6),
        (145, 2853),
        (149, 1),
        (153, 2865),
        (2904, 6),
        (2912, 300),
        (2998, 6),
        (3006, 6),
    ] {
        int(&mut vtx, offset, value);
    }
    vtx[72..76].copy_from_slice(&16f32.to_le_bytes());
    vtx[132] = 2;
    vtx[157] = 2;
    vtx[2922] = 1;
    vtx[3016] = 1;
    for (start, count) in [(192, 300), (2932, 6)] {
        for index in 0..count {
            let original = [2u16, 0, 2, 1, 3, 3][index % 6];
            vtx[start + index * 9 + 4..start + index * 9 + 6]
                .copy_from_slice(&original.to_le_bytes());
        }
    }
    for start in [2892, 2986] {
        for (index, vertex) in [1u16, 3, 0, 1, 0, 4].into_iter().enumerate() {
            vtx[start + index * 2..start + index * 2 + 2].copy_from_slice(&vertex.to_le_bytes());
        }
    }
    std::fs::write(directory.path().join("models/test/receiver.dx90.vtx"), vtx).unwrap();
    let mut document = Document::empty();
    document
        .edit("sealed baked prop fixture", |tx| {
            for (minimum, maximum) in [
                ([-272., -272., -16.], [272., 272., 0.]),
                ([-272., -272., 256.], [272., 272., 272.]),
                ([-272., -272., 0.], [-256., 272., 256.]),
                ([256., -272., 0.], [272., 272., 256.]),
                ([-256., -272., 0.], [256., -256., 256.]),
                ([-256., 256., 0.], [256., 272., 256.]),
            ] {
                edit::create_block(
                    tx,
                    DVec3::from_array(minimum),
                    DVec3::from_array(maximum),
                    "test/checker",
                )?;
            }
            edit::create_entity(
                tx,
                "info_player_terrorist",
                Some(DVec3::new(-128., 0., 8.)),
                &[],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
    }
    let mut light = Entity::new(9001, "light");
    for (key, value) in [
        ("origin", "0 0 160"),
        ("_light", "255 0 0 255"),
        ("_lightHDR", "0 0 255 255"),
        ("_constant_attn", "1"),
    ] {
        light.properties.set(key, value);
    }
    vmf.entities.push(light);
    for (id, origin, disabled) in [
        (9100, "-64 0 32", true),
        (9101, "0 0 32", false),
        (9102, "64 0 32", false),
    ] {
        let mut prop = Entity::new(id, "prop_static");
        for (key, value) in [
            ("origin", origin),
            ("model", "models/test/receiver.mdl"),
            ("skin", "0"),
            ("solid", "0"),
            ("disableshadows", "1"),
        ] {
            prop.properties.set(key, value);
        }
        if disabled {
            prop.properties.set("disablevertexlighting", "1");
        }
        vmf.entities.push(prop);
    }
    (directory, game, vmf)
}

pub fn leaf_membership_fixture() -> (tempfile::TempDir, GameFs, Vmf) {
    let (directory, game, vmf) = fixture();
    let mut document = Document::new(vmf);
    document
        .edit("prop leaf membership partition", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-8., -256., 0.),
                DVec3::new(8., 256., 256.),
                "test/checker",
            )?;
            edit::create_entity(
                tx,
                "info_player_terrorist",
                Some(DVec3::new(128., 0., 8.)),
                &[],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    (directory, game, document.into_vmf())
}
