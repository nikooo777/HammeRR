use std::fs;

use source_formats::bsp::{Bsp, Leaf, lump};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vpk::Vpk;
use source_formats::vtf::Vtf;

fn game(root: &std::path::Path) -> GameFs {
    GameFs {
        game_dir: root.into(),
        base_dir: root.parent().unwrap().into(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(root.into()),
        }],
    }
}

#[test]
fn bsp_rejects_signed_ranges_and_impossible_game_directories() {
    for (offset, length) in [(-1, 1), (0, -1), (i32::MAX, i32::MAX)] {
        let mut bytes = Bsp::new().to_bytes();
        bytes[8..12].copy_from_slice(&i32::to_le_bytes(offset));
        bytes[12..16].copy_from_slice(&i32::to_le_bytes(length));
        assert!(Bsp::parse(&bytes).is_err());
    }
    for count in [-1, 1, i32::MAX] {
        let mut bytes = Bsp::new().to_bytes();
        let start = bytes.len() as i32;
        let entry = 8 + lump::GAME_LUMP * 16;
        bytes[entry..entry + 4].copy_from_slice(&start.to_le_bytes());
        bytes[entry + 4..entry + 8].copy_from_slice(&4i32.to_le_bytes());
        bytes.extend_from_slice(&i32::to_le_bytes(count));
        assert!(Bsp::parse(&bytes).is_err());
    }
    let mut bsp = Bsp::new();
    bsp.game_lumps.push(source_formats::bsp::GameLump {
        id: *b"sprp",
        flags: 0,
        version: 4,
        data: vec![1],
    });
    let mut bytes = bsp.to_bytes();
    let entry = 8 + lump::GAME_LUMP * 16;
    let start = i32::from_le_bytes(bytes[entry..entry + 4].try_into().unwrap()) as usize;
    bytes[start + 12..start + 16].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(Bsp::parse(&bytes).is_err());
}

#[test]
fn leaf_v0_water_precedes_the_ambient_cube() {
    let mut bsp = Bsp::new();
    let mut data = vec![0; 56 * 3];
    for (leaf, water) in data.as_chunks_mut::<56>().0.iter_mut().zip([-1i16, 0, 123]) {
        leaf[28..30].copy_from_slice(&water.to_le_bytes());
        leaf[30..54].fill(255);
    }
    bsp.lumps[lump::LEAFS].data = data;
    assert_eq!(
        bsp.records::<Leaf>()
            .unwrap()
            .iter()
            .map(|l| l.water_data)
            .collect::<Vec<_>>(),
        [-1, 0, 123]
    );
}

fn texture() -> Vec<u8> {
    let mut bytes = vec![0; 84];
    bytes[..4].copy_from_slice(b"VTF\0");
    bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&80u32.to_le_bytes());
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&1u16.to_le_bytes());
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    bytes
}

#[test]
fn vtf_validates_mips_dimensions_resource_counts_and_payload_ranges() {
    assert_eq!(
        Vtf::parse(texture())
            .unwrap()
            .image_data(0, 0, 0)
            .unwrap()
            .len(),
        4
    );
    for mips in [0, 2, 33, 255] {
        let mut bytes = texture();
        bytes[56] = mips;
        assert!(Vtf::parse(bytes).is_err());
    }
    let mut bytes = texture();
    bytes[16..18].fill(0);
    assert!(Vtf::parse(bytes).is_err());
    for header in [0u32, 64, u32::MAX] {
        let mut bytes = texture();
        bytes[12..16].copy_from_slice(&header.to_le_bytes());
        assert!(Vtf::parse(bytes).is_err());
    }
    let mut bytes = texture();
    bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Vtf::parse(bytes).is_err());
    let mut bytes = texture();
    bytes[24..26].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(Vtf::parse(bytes).is_err());
}

#[test]
fn vpk_validates_tree_and_entry_lengths_before_reserving_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_dir.vpk");
    for version in [1u32, 2] {
        let bytes = [0x55aa_1234u32, version, u32::MAX]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        fs::write(&path, bytes).unwrap();
        assert!(Vpk::open(&path).is_err());
    }
    let mut tree = b"txt\0 \0payload\0".to_vec();
    tree.extend_from_slice(&0u32.to_le_bytes());
    tree.extend_from_slice(&0u16.to_le_bytes());
    tree.extend_from_slice(&0x7fffu16.to_le_bytes());
    tree.extend_from_slice(&0u32.to_le_bytes());
    tree.extend_from_slice(&u32::MAX.to_le_bytes());
    tree.extend_from_slice(&0xffffu16.to_le_bytes());
    tree.extend_from_slice(&[0, 0, 0]);
    let mut bytes = [0x55aa_1234u32, 1, tree.len() as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    bytes.extend(tree);
    fs::write(&path, bytes).unwrap();
    assert!(Vpk::open(&path).unwrap().read("payload.txt").is_err());
}

#[test]
fn asset_paths_are_relative_and_empty_listing_stays_in_the_mount() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    fs::create_dir_all(root.join("Materials")).unwrap();
    fs::write(dir.path().join("sentinel.txt"), "outside").unwrap();
    fs::write(root.join("Materials/Inside.txt"), "inside").unwrap();
    let game = game(&root);
    for path in [
        "../sentinel.txt",
        "materials/../../sentinel.txt",
        "..\\sentinel.txt",
        "/sentinel.txt",
        "C:\\sentinel.txt",
    ] {
        assert!(game.read(path).is_err(), "{path}");
        assert!(game.list(path, ".txt").is_err(), "{path}");
    }
    assert_eq!(
        game.read("Materials\\inside.txt").unwrap().unwrap(),
        b"inside"
    );
    assert_eq!(game.list("", ".txt").unwrap(), ["materials/inside.txt"]);
    assert_eq!(
        game.list("materials", ".txt").unwrap(),
        ["materials/inside.txt"]
    );
}

#[cfg(unix)]
#[test]
fn symlinked_mounts_work_but_nested_links_cannot_escape_or_cycle() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("content");
    fs::create_dir_all(root.join("materials")).unwrap();
    fs::write(root.join("materials/test.vmt"), "test").unwrap();
    fs::write(dir.path().join("sentinel.vmt"), "outside").unwrap();
    symlink(&root, dir.path().join("mounted")).unwrap();
    symlink(&root, root.join("materials/cycle")).unwrap();
    symlink(dir.path(), root.join("external")).unwrap();
    symlink(dir.path().join("sentinel.vmt"), root.join("escape.vmt")).unwrap();
    let game = game(&dir.path().join("mounted"));
    assert_eq!(game.read("materials/test.vmt").unwrap().unwrap(), b"test");
    assert!(game.read("external/sentinel.vmt").unwrap().is_none());
    assert!(game.read("escape.vmt").unwrap().is_none());
    assert_eq!(game.list("", ".vmt").unwrap(), ["materials/test.vmt"]);
}
