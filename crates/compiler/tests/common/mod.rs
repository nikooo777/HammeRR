use glam::DVec3;
use map_core::{Document, edit};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

pub fn room() -> Vmf {
    let mut document = Document::empty();
    document
        .edit("test room", |tx| {
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
            for (classname, origin) in [
                ("info_player_terrorist", [-64.0, 0.0, 8.0]),
                ("info_player_counterterrorist", [64.0, 0.0, 8.0]),
            ] {
                edit::create_entity(
                    tx,
                    classname,
                    Some(DVec3::from_array(origin)),
                    &[],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

pub fn empty_game() -> GameFs {
    GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    }
}
