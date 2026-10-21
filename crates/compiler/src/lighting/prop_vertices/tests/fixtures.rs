use super::*;

use crate::test_static_prop_model::model;

fn int(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn game() -> (tempfile::TempDir, GameFs) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/608");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    for folder in ["models/props", "materials/models/test", "materials/shared"] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (suffix, data) in ["mdl", "vvd", "dx90.vtx"].into_iter().zip(model()) {
        std::fs::write(
            directory.path().join(format!("models/props/test.{suffix}")),
            data,
        )
        .unwrap();
    }
    for (path, text) in [
        ("one.vmt", "Patch { include \"materials/shared/base.vmt\" }"),
        ("shared/base.vmt", "VertexLitGeneric { $basetexture test }"),
        (
            "models/test/two.vmt",
            "VertexLitGeneric { $basetexture other }",
        ),
        ("sky.vmt", "UnlitGeneric { %compilesky 1 }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{path}")), text).unwrap();
    }
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

pub(super) fn scene(multiple: bool) -> Vmf {
    let mut document = Document::empty();
    document
        .edit("closed vertex receiver room", |tx| {
            for (mins, maxs) in [
                ([-272.0, -272.0, -16.0], [272.0, 272.0, 0.0]),
                ([-272.0, -272.0, 256.0], [272.0, 272.0, 272.0]),
                ([-272.0, -272.0, 0.0], [-256.0, 272.0, 256.0]),
                ([256.0, -272.0, 0.0], [272.0, 272.0, 256.0]),
                ([-256.0, -272.0, 0.0], [256.0, -256.0, 256.0]),
                ([-256.0, 256.0, 0.0], [256.0, 272.0, 256.0]),
            ] {
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?;
            }
            edit::create_entity(
                tx,
                "info_player_terrorist",
                Some(DVec3::new(-64.0, 0.0, 8.0)),
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
        ("origin", "0 0 100"),
        ("_light", "255 0 0 255"),
        ("_lightHDR", "0 0 255 255"),
        ("_constant_attn", "1"),
    ] {
        light.properties.set(key, value);
    }
    vmf.entities.push(light);
    for (id, position, disabled, hidden, skin) in if multiple {
        vec![
            (9100, "-32 0 50", false, true, "0"),
            (9101, "0 0 50", false, false, "0"),
            (9102, "32 0 50", true, false, "0"),
            (9103, "64 0 50", false, false, "1"),
        ]
    } else {
        vec![(9101, "0 0 50", false, false, "0")]
    } {
        let mut prop = Entity::new(id, "prop_static");
        prop.hidden = hidden;
        for (key, value) in [
            ("origin", position),
            ("model", "models/props/test.mdl"),
            ("solid", "0"),
            ("skin", skin),
        ] {
            prop.properties.set(key, value);
        }
        if disabled {
            prop.properties.set("disablevertexlighting", "1");
        }
        vmf.entities.push(prop);
    }
    vmf
}

pub(super) fn add_lod(directory: &std::path::Path) {
    let path = directory.join("models/props/test.mdl");
    let mut mdl = std::fs::read(&path).unwrap();
    int(&mut mdl, 632, 3);
    std::fs::write(path, mdl).unwrap();
    let path = directory.join("models/props/test.vvd");
    let mut vvd = std::fs::read(&path).unwrap();
    vvd.resize(280, 0);
    for (offset, value) in [
        (12, 2),
        (20, 3),
        (48, 2),
        (52, 256),
        (256, 1),
        (260, 0),
        (264, 3),
        (268, 0),
        (272, 3),
        (276, 1),
    ] {
        int(&mut vvd, offset, value);
    }
    std::fs::write(path, vvd).unwrap();
    let path = directory.join("models/props/test.dx90.vtx");
    let mut vtx = std::fs::read(&path).unwrap();
    vtx.resize(700, 0);
    for (offset, value) in [
        (20, 2),
        (44, 2),
        (48, 356),
        (400, 1),
        (404, -336),
        (412, 1),
        (416, 38),
        (450, 1),
        (454, 25),
        (475, 4),
        (479, 125),
        (483, 3),
        (487, 161),
        (491, 1),
        (495, 167),
        (642, 3),
        (650, 4),
    ] {
        int(&mut vtx, offset, value);
    }
    float(&mut vtx, 420, 16.0);
    vtx[499] = 2;
    vtx[660] = 1;
    for (index, original) in [2_u16, 0, 2, 1].into_iter().enumerate() {
        vtx[604 + index * 9..606 + index * 9].copy_from_slice(&original.to_le_bytes());
    }
    for (index, vertex) in [1_u16, 3, 0].into_iter().enumerate() {
        vtx[636 + index * 2..638 + index * 2].copy_from_slice(&vertex.to_le_bytes());
    }
    std::fs::write(path, vtx).unwrap();
}

pub(super) fn groups(directory: &std::path::Path, count: usize) {
    add_lod(directory);
    let path = directory.join("models/props/test.dx90.vtx");
    let mut vtx = std::fs::read(&path).unwrap();
    let first = vtx[73..98].to_vec();
    let second = vtx[475..500].to_vec();
    let root_count = count / 2;
    let lower_start = 800 + root_count * 25;
    vtx.resize(800 + count * 25, 0);
    for (offset, value) in [
        (64, root_count),
        (68, 736),
        (450, count - root_count),
        (454, lower_start - 450),
    ] {
        int(&mut vtx, offset, value as i32);
    }
    for (header, start, count, targets) in [
        (first, 800, root_count, [256, 310, 322]),
        (second, lower_start, count - root_count, [600, 636, 642]),
    ] {
        for group in 0..count {
            let base = start + group * 25;
            vtx[base..base + 25].copy_from_slice(&header);
            for (field, target) in [4, 12, 20].into_iter().zip(targets) {
                int(&mut vtx, base + field, target - base as i32);
            }
        }
    }
    std::fs::write(path, vtx).unwrap();
}

pub(super) fn caster(directory: &std::path::Path, alpha: u8) {
    for folder in ["models/casters", "materials/casters"] {
        std::fs::create_dir_all(directory.join(folder)).unwrap();
    }
    let [mut mdl, mut vvd, vtx] = model();
    int(&mut mdl, 152, 0x0004_0010);
    mdl[720..736].fill(0);
    mdl[720..724].copy_from_slice(b"cast");
    mdl[752..768].fill(0);
    mdl[752..760].copy_from_slice(b"casters/");
    for (index, point) in [
        [-16.0, -16.0, 0.0],
        [16.0, -16.0, 0.0],
        [0.0, 16.0, 0.0],
        [-16.0, 16.0, 0.0],
    ]
    .into_iter()
    .enumerate()
    {
        for (axis, value) in point.into_iter().enumerate() {
            float(&mut vvd, 64 + index * 48 + 16 + axis * 4, value);
        }
    }
    for (suffix, data) in ["mdl", "vvd", "dx90.vtx"].into_iter().zip([mdl, vvd, vtx]) {
        std::fs::write(
            directory.join(format!("models/casters/test.{suffix}")),
            data,
        )
        .unwrap();
    }
    for (path, text) in [
        (
            "materials/casters/cast.vmt",
            "Patch { include \"materials/casters/base.vmt\" }",
        ),
        (
            "materials/casters/base.vmt",
            "VertexLitGeneric { $basetexture casters/mask $alphatest 1 }",
        ),
    ] {
        std::fs::write(directory.join(path), text).unwrap();
    }
    std::fs::write(
        directory.join("materials/casters/mask.vtf"),
        caster_texture(alpha),
    )
    .unwrap();
}

pub(super) fn caster_texture(alpha: u8) -> Vec<u8> {
    let mut data = vec![0; 80];
    data[..4].copy_from_slice(b"VTF\0");
    int(&mut data, 4, 7);
    int(&mut data, 12, 80);
    data[16..18].copy_from_slice(&4_u16.to_le_bytes());
    data[18..20].copy_from_slice(&4_u16.to_le_bytes());
    int(&mut data, 20, 0x1000);
    data[24..26].copy_from_slice(&1_u16.to_le_bytes());
    data[56] = 1;
    int(&mut data, 57, -1);
    for _ in 0..16 {
        data.extend_from_slice(&[255, 255, 255, alpha]);
    }
    data
}

pub(super) fn caster_scene() -> Vmf {
    let mut vmf = scene(false);
    let mut caster = Entity::new(9200, "prop_static");
    for (key, value) in [
        ("origin", "0 0 75"),
        ("model", "models/casters/test.mdl"),
        ("solid", "0"),
        ("skin", "0"),
        ("disablevertexlighting", "1"),
    ] {
        caster.properties.set(key, value);
    }
    vmf.entities.push(caster);
    vmf
}

pub(super) fn normals(directory: &std::path::Path, normal: [f32; 3]) {
    let path = directory.join("models/props/test.vvd");
    let mut vvd = std::fs::read(&path).unwrap();
    for index in 0..4 {
        for (axis, value) in normal.into_iter().enumerate() {
            float(&mut vvd, 64 + index * 48 + 28 + axis * 4, value);
        }
    }
    std::fs::write(path, vvd).unwrap();
}

pub(super) fn grounded(height: f64) -> Vmf {
    let mut vmf = scene(false);
    for entity in &mut vmf.entities {
        if entity.classname() == Some("prop_static") {
            entity.properties.set("origin", format!("0 0 {height}"));
        } else if entity.classname() == Some("light") {
            entity.properties.set("origin", "8 0 8");
        }
    }
    vmf
}

pub(super) fn self_shadow_model(directory: &std::path::Path) {
    let [mut mdl, mut vvd, _] = model();
    for offset in [104, 128] {
        for axis in 0..2 {
            float(&mut mdl, offset + axis * 4, -16.0);
            float(&mut mdl, offset + 12 + axis * 4, 16.0);
        }
        float(&mut mdl, offset + 20, 10.0);
    }
    for offset in [496, 584, 628] {
        int(&mut mdl, offset, 7);
    }
    int(&mut vvd, 16, 7);
    vvd.resize(64 + 7 * 48, 0);
    for (index, point) in [[-16.0, -16.0, 10.0], [16.0, -16.0, 10.0], [0.0, 16.0, 10.0]]
        .into_iter()
        .enumerate()
    {
        for (axis, value) in point.into_iter().enumerate() {
            float(&mut vvd, 64 + (index + 4) * 48 + 16 + axis * 4, value);
        }
        float(&mut vvd, 64 + (index + 4) * 48 + 36, 1.0);
    }
    let mut vtx = vec![0; 376];
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
        (73, 9),
        (77, 183),
        (81, 6),
        (85, 264),
        (89, 1),
        (93, 276),
        (349, 6),
        (357, 9),
    ] {
        int(&mut vtx, offset, value);
    }
    vtx[97] = 2;
    vtx[367] = 1;
    for (index, original) in [2_u16, 0, 2, 1, 3, 3, 4, 5, 6].into_iter().enumerate() {
        vtx[260 + index * 9..262 + index * 9].copy_from_slice(&original.to_le_bytes());
    }
    for (index, vertex) in [1_u16, 3, 0, 6, 7, 8].into_iter().enumerate() {
        vtx[337 + index * 2..339 + index * 2].copy_from_slice(&vertex.to_le_bytes());
    }
    for (suffix, data) in ["mdl", "vvd", "dx90.vtx"].into_iter().zip([mdl, vvd, vtx]) {
        std::fs::write(directory.join(format!("models/props/test.{suffix}")), data).unwrap();
    }
}
