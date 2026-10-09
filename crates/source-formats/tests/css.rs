//! Tests against a local Counter-Strike: Source install. `CSS_DIR` overrides
//! the default Steam location; tests are skipped when it isn't installed.

use std::path::PathBuf;

use source_formats::gamefs::{GameFs, Mount};

fn css_gameinfo() -> Option<PathBuf> {
    let dir = match std::env::var_os("CSS_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?)
            .join(".local/share/Steam/steamapps/common/Counter-Strike Source"),
    };
    let gameinfo = dir.join("cstrike/gameinfo.txt");
    gameinfo.is_file().then_some(gameinfo)
}

#[test]
fn mounts_css_search_paths() {
    let Some(gameinfo) = css_gameinfo() else {
        eprintln!("CS:S not installed; skipping");
        return;
    };
    let fs = GameFs::from_gameinfo(&gameinfo).unwrap();
    for m in &fs.mounts {
        eprintln!("{:<28} {}", m.ids.join("+"), m.mount.location().display());
    }

    let names: Vec<String> = fs
        .mounts
        .iter()
        .map(|m| {
            m.mount
                .location()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let position = |name: &str| names.iter().position(|n| n == name);
    let pak = position("cstrike_pak_dir.vpk").expect("cstrike pak mounted");
    let textures = position("hl2_textures_dir.vpk").expect("hl2 textures mounted");
    let loose = position("cstrike").expect("cstrike folder mounted");
    assert!(
        pak < textures && textures < loose,
        "mount order follows gameinfo.txt"
    );
    assert_eq!(
        names.iter().filter(|n| *n == "cstrike").count(),
        1,
        "|gameinfo_path|. and cstrike are one mount"
    );

    let vmt = fs
        .read("Materials\\DE_CBBLE\\CobbleRoad01.vmt")
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&vmt).to_ascii_lowercase();
    assert!(text.contains("$basetexture"), "{text}");
    assert!(fs.read("materials/does/not/exist.vmt").unwrap().is_none());
}

#[test]
fn pak_contents_match_their_crcs() {
    let Some(gameinfo) = css_gameinfo() else {
        eprintln!("CS:S not installed; skipping");
        return;
    };
    let fs = GameFs::from_gameinfo(&gameinfo).unwrap();
    let mut checked = 0;
    for mounted in &fs.mounts {
        let Mount::Pak(vpk) = &mounted.mount else {
            continue;
        };
        let mut paths: Vec<&str> = vpk.paths().collect();
        paths.sort_unstable();
        for path in paths.iter().step_by((paths.len() / 50).max(1)) {
            let data = vpk.read(path).unwrap().unwrap();
            assert_eq!(
                crc32fast::hash(&data),
                vpk.crc(path).unwrap(),
                "{path} in {}",
                vpk.path().display()
            );
            checked += 1;
        }
    }
    eprintln!("{checked} files verified");
    assert!(checked > 100);
}
