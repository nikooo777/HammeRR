use source_formats::bsp::Bsp;
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

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

pub fn alpha_game() -> (tempfile::TempDir, GameFs) {
    let (directory, game) = game();
    let path = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&path).unwrap();
    let colors = [
        [255, 0, 0, 0],
        [0, 255, 0, 64],
        [0, 0, 255, 128],
        [255, 255, 0, 255],
    ];
    for (index, pixel) in bytes[80..].as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&colors[index % 64 / 16]);
    }
    std::fs::write(path, bytes).unwrap();
    (directory, game)
}

pub fn room() -> source_formats::vmf::Vmf {
    use glam::DVec3;
    use map_core::{Document, edit};
    let mut document = Document::empty();
    document
        .edit("room", |tx| {
            for (mins, maxs) in [
                ([-272., -272., -16.], [272., 272., 0.]),
                ([-272., -272., 256.], [272., 272., 272.]),
                ([-272., -272., 0.], [-256., 272., 256.]),
                ([256., -272., 0.], [272., 272., 256.]),
                ([-256., -272., 0.], [256., -256., 256.]),
                ([-256., 256., 0.], [256., 272., 256.]),
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
                Some(DVec3::new(0., 0., 8.)),
                &[],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

pub fn game_process(persistent: bool) -> (tempfile::TempDir, GameFs, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let game = directory.path().join("cstrike");
    std::fs::create_dir_all(game.join("maps")).unwrap();
    let executable = directory.path().join("cstrike_linux64");
    let ending = if persistent {
        "exec sleep 60"
    } else {
        "sleep 0.1\nexit 23"
    };
    let script = format!(
        r#"#!/bin/sh
insecure=false
while [ "$#" -gt 0 ]; do
case "$1" in
    -insecure) insecure=true ;;
    +con_logfile) shift; logfile=$1 ;;
    +map) shift; map=$1 ;;
esac
shift
done
test "$insecure" = true || exit 10
test -f "cstrike/maps/$map.bsp" || exit 9
mkdir -p cstrike/downloadlists
printf '%s\n' "$map" > "cstrike/downloadlists/$map.lst"
printf 'loaded %s\n' "$map" > "cstrike/$logfile"
echo launcher-output
{ending}
"#
    );
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bsp = directory.path().join("source.bsp");
    std::fs::write(&bsp, Bsp::new().to_bytes()).unwrap();
    std::fs::write(game.join("maps/stock.bsp"), b"stock map").unwrap();
    let fs = GameFs {
        game_dir: game,
        base_dir: directory.path().to_owned(),
        mounts: Vec::new(),
    };
    (directory, fs, bsp)
}

pub use crate::session::clipboard::fixtures::fixture as clipboard;

pub use crate::session::prefabs::fixtures::fixture as prefabs;

pub use crate::session::prefabs::fixtures::textured_fixture as textured_prefabs;

pub use crate::session::placement::fixtures::fixture as placement;

pub use crate::session::sounds::fixtures::fixture as sounds;

pub use crate::session::repeat::fixtures::fixture as repeat;

pub use crate::session::particles::fixtures::fixture as particles;
