use super::*;

pub const MODEL: &str = "models/props/instance.mdl";

pub fn fixture() -> (tempfile::TempDir, GameFs, PathBuf, PathBuf) {
    let (directory, mut game, source, child) = instance_map(false);
    std::fs::create_dir_all(game.game_dir.join("models/props")).unwrap();
    std::fs::create_dir_all(game.game_dir.join("materials/tools")).unwrap();
    std::fs::write(
        game.game_dir.join("materials/tools/toolsblack.vmt"),
        "LightmappedGeneric { }",
    )
    .unwrap();
    std::fs::create_dir_all(directory.path().join("outputs")).unwrap();
    let mut model = vec![0; 240];
    model[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
        model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, values) in [
        (104, [-16.0_f32, -8.0, 0.0]),
        (116, [16.0, 8.0, 32.0]),
        (128, [-16.0, -8.0, 0.0]),
        (140, [16.0, 8.0, 32.0]),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            model[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(game.game_dir.join(MODEL), model).unwrap();
    game.mounts = vec![MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(game.game_dir.clone()),
    }];
    let mut document = map_core::Document::empty();
    document
        .edit("static child prop", |tx| {
            map_core::edit::create_entity(
                tx,
                "prop_static",
                Some(glam::DVec3::new(32.0, 32.0, 8.0)),
                &[
                    ("model".into(), MODEL.into()),
                    ("angles".into(), "0 0 0".into()),
                    ("solid".into(), "0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    std::fs::write(&child, document.vmf().write(&Default::default())).unwrap();
    (directory, game, source, child)
}
