//! Tests against a local Counter-Strike: Source install. `CSS_DIR` overrides
//! the default Steam location; tests are skipped when it isn't installed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use source_formats::gamefs::{GameFs, Mount};
use source_formats::vmt::Material;
use source_formats::vtf::Vtf;

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
fn decodes_css_static_model_geometry() {
    let Some(gameinfo) = css_gameinfo() else {
        eprintln!("CS:S not installed; skipping");
        return;
    };
    let fs = GameFs::from_gameinfo(gameinfo).unwrap();
    for path in [
        "models/props_junk/wood_crate001a.mdl",
        "models/props/cs_office/file_cabinet1.mdl",
        "models/props_c17/oildrum001.mdl",
    ] {
        let stem = path.strip_suffix(".mdl").unwrap();
        let read = |path: &str| {
            fs.read_limited(path, source_formats::mdl::MAX_MODEL_BYTES)
                .unwrap()
                .unwrap()
        };
        let model = source_formats::model::Model::parse(
            &read(path),
            &read(&format!("{stem}.vvd")),
            &read(&format!("{stem}.dx90.vtx")),
        )
        .unwrap_or_else(|error| panic!("{path}: {error}"));
        assert!(!model.vertices.is_empty());
        assert!(model.parts.iter().any(|part| !part.indices.is_empty()));
        assert_eq!(model.skins.len(), model.header.skins);
        eprintln!(
            "{path}: {} vertices, {} meshes, {} skins",
            model.vertices.len(),
            model.parts.len(),
            model.skins.len()
        );
    }
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

#[test]
fn materials_resolve_to_decodable_textures() {
    let Some(gameinfo) = css_gameinfo() else {
        eprintln!("CS:S not installed; skipping");
        return;
    };
    let fs = GameFs::from_gameinfo(&gameinfo).unwrap();
    let mut materials: Vec<String> = fs
        .mounts
        .iter()
        .filter(|m| m.ids.iter().any(|id| id == "game"))
        .filter_map(|m| match &m.mount {
            Mount::Pak(vpk) => Some(vpk.paths().filter(|p| p.ends_with(".vmt"))),
            Mount::Directory(_) => None,
        })
        .flatten()
        .map(str::to_owned)
        .collect();
    materials.sort_unstable();
    materials.dedup();

    let mut problems: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut decoded = 0;
    for path in &materials {
        let name = path
            .trim_start_matches("materials/")
            .trim_end_matches(".vmt");
        let mut problem = |kind: String| problems.entry(kind).or_default().push(path.clone());
        let material = match Material::load(&fs, name) {
            Ok(Some(m)) => m,
            Ok(None) => unreachable!("listed material {path} is readable"),
            Err(e) => {
                problem(format!("vmt: {e}"));
                continue;
            }
        };
        let candidates = material.editor_textures();
        if candidates.is_empty() {
            continue;
        }
        let Some(bytes) = candidates.iter().find_map(|t| fs.read(t).unwrap()) else {
            problem("missing texture".into());
            continue;
        };
        let vtf = match Vtf::parse(bytes) {
            Ok(vtf) => vtf,
            Err(e) => {
                problem(format!("vtf: {e}"));
                continue;
            }
        };
        *formats.entry(format!("{:?}", vtf.format)).or_default() += 1;
        let mip = vtf.mip_for_size(64);
        match vtf.decode_rgba8(mip, 0, 0) {
            Ok(image) => {
                assert_eq!(
                    image.pixels.len(),
                    (image.width * image.height * 4) as usize
                );
                decoded += 1;
            }
            Err(e) => problem(format!("decode: {e}")),
        }
    }

    eprintln!("{} materials, {decoded} textures decoded", materials.len());
    eprintln!("formats: {formats:?}");
    for (kind, paths) in &problems {
        eprintln!(
            "{kind}: {} (e.g. {})",
            paths.len(),
            paths[..paths.len().min(3)].join(", ")
        );
    }
    let failures: usize = problems
        .iter()
        .filter(|(kind, _)| kind.as_str() != "missing texture")
        .map(|(_, p)| p.len())
        .sum();
    assert_eq!(failures, 0);
    assert!(decoded > 5000);
}
