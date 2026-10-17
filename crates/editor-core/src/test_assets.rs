pub mod model_lighting;

use source_formats::bsp::Bsp;
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

pub fn document(session: crate::runtime::SharedSession) -> crate::runtime::DocumentHandle {
    crate::runtime::DocumentHandle::start(session).unwrap()
}

pub struct NativeActivityFixture {
    _authority: crate::runtime::native_activity::Authority,
    permit: Option<crate::runtime::native_activity::Permit>,
}

impl NativeActivityFixture {
    pub fn fail_cleanup(&mut self, error: &str) {
        let mut permit = self
            .permit
            .take()
            .expect("fixture owns its native activity");
        permit.failed_cleanup(error);
    }
}

pub fn native_activity(session: &crate::runtime::SharedSession) -> NativeActivityFixture {
    let authority = crate::runtime::native_activity::Authority::isolated();
    let client = authority.client(crate::runtime::DocumentId::allocate().unwrap());
    let permit = client
        .reserve(crate::runtime::native_activity::Phase::Capture, None, None)
        .unwrap();
    crate::runtime::lock(session).use_application_native_activity(client);
    NativeActivityFixture {
        _authority: authority,
        permit: Some(permit),
    }
}

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

pub fn instance_map(leak: bool) -> (tempfile::TempDir, GameFs, PathBuf, PathBuf) {
    use glam::DVec3;
    use map_core::{Document, edit};
    use source_formats::vmf::Entity;
    let (directory, game, _) = game_process(false);
    let source = directory.path().join("instances.vmf");
    let child_path = directory.path().join("child.vmf");
    let mut child = Document::empty();
    child
        .edit("instance brushes", |tx| {
            edit::create_block(
                tx,
                DVec3::new(0.0, 0.0, 32.0),
                DVec3::new(16.0, 16.0, 48.0),
                "dev/dev_measuregeneric01",
            )?;
            edit::create_block(
                tx,
                DVec3::new(24.0, 0.0, 32.0),
                DVec3::new(40.0, 16.0, 48.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let mut child = child.vmf().clone();
    let detail_id = child.object_ids().chain(child.side_ids()).max().unwrap() + 1;
    let mut detail = Entity::new(detail_id, "func_detail");
    detail.solids.push(child.world.solids.pop().unwrap());
    child.entities.push(detail);
    std::fs::write(&child_path, child.write(&Default::default())).unwrap();
    let mut parent = room();
    if leak {
        parent.world.solids.pop();
    }
    let id = parent.object_ids().chain(parent.side_ids()).max().unwrap() + 1;
    let mut instance = Entity::new(id, "func_instance");
    instance.properties.set("file", "child.vmf");
    instance.properties.set("origin", "0 0 0");
    instance.properties.set("angles", "0 0 0");
    parent.entities.push(instance);
    std::fs::write(&source, parent.write(&Default::default())).unwrap();
    std::fs::write(source.with_extension("bsp"), b"previous BSP").unwrap();
    std::fs::write(source.with_extension("lin"), b"previous pointfile").unwrap();
    (directory, game, source, child_path)
}

pub use crate::session::clipboard::fixtures::fixture as clipboard;

pub use crate::session::prefabs::fixtures::fixture as prefabs;

pub use crate::session::prefabs::fixtures::textured_fixture as textured_prefabs;

pub use crate::session::placement::fixtures::fixture as placement;

pub use crate::session::sounds::fixtures::fixture as sounds;

pub use crate::session::repeat::fixtures::fixture as repeat;

pub use crate::session::particles::fixtures::fixture as particles;

pub fn instance_extraction_map() -> (tempfile::TempDir, GameFs, PathBuf, Vec<map_core::ObjectId>) {
    use glam::DVec3;
    use map_core::{Document, ObjectId, edit};
    let (directory, game, _) = game_process(false);
    let parent = directory.path().join("extraction-parent.vmf");
    let mut document = Document::new(room());
    let selected = document
        .edit("assembly", |tx| {
            let world = edit::create_block(
                tx,
                DVec3::new(32.0, 40.0, 8.0),
                DVec3::new(48.0, 56.0, 24.0),
                "test/checker",
            )?;
            let ObjectId::Solid(world_id) = world else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(world_id)?.sides {
                side.u_axis.axis = DVec3::new(1.0, 0.5, 0.0);
                side.u_axis.scale = -0.25;
                side.u_axis.offset = 7.5;
                side.v_axis.axis = DVec3::new(0.0, 1.0, 0.25);
                side.v_axis.scale = 0.5;
                side.v_axis.offset = -3.25;
                side.smoothing_groups = None;
            }
            let a = edit::create_block(
                tx,
                DVec3::new(64.0, 40.0, 8.0),
                DVec3::new(72.0, 56.0, 24.0),
                "test/checker",
            )?;
            let b = edit::create_block(
                tx,
                DVec3::new(80.0, 40.0, 8.0),
                DVec3::new(88.0, 56.0, 24.0),
                "test/checker",
            )?;
            let brush_id = |id| match id {
                ObjectId::Solid(id) => id,
                _ => unreachable!(),
            };
            let detail = edit::tie_to_entity(tx, &[brush_id(a), brush_id(b)], "func_detail", &[])?;
            edit::create_entity(
                tx,
                "info_target",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[],
                Vec::new(),
            )?;
            Ok(vec![world, detail])
        })
        .unwrap();
    std::fs::write(&parent, document.vmf().write(&Default::default())).unwrap();
    std::fs::write(parent.with_extension("bsp"), b"prior BSP").unwrap();
    std::fs::write(parent.with_extension("lin"), b"prior LIN").unwrap();
    (directory, game, parent, selected)
}
