use std::{collections::BTreeSet, path::Path};

use source_formats::{
    gamefs::{GameFs, asset_relative},
    mdl::MAX_MODEL_BYTES,
    model::Model,
    vmt::{Material, material_path},
};

const CAST_TEXTURE_SHADOWS: u32 = 0x0004_0000;

fn read(fs: &GameFs, path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    fs.read_limited(path, MAX_MODEL_BYTES)?
        .ok_or_else(|| format!("Missing native fixture asset {path}").into())
}

fn write(root: &Path, path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let target = root.join(asset_relative(path)?);
    std::fs::create_dir_all(target.parent().unwrap())?;
    std::fs::write(target, bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [gameinfo, path, destination] = arguments.as_slice() else {
        return Err(
            "usage: prop_shadow_fixture <gameinfo.txt> <models/path.mdl> <private-game-directory>"
                .into(),
        );
    };
    let fs = GameFs::from_gameinfo(gameinfo)?;
    let path = asset_relative(path)?;
    let stem = path.strip_suffix(".mdl").ok_or("Expected a .mdl asset")?;
    let mut mdl = read(&fs, &path)?;
    let vvd = read(&fs, &format!("{stem}.vvd"))?;
    let vtx = read(&fs, &format!("{stem}.dx90.vtx"))?;
    let model = Model::parse(&mdl, &vvd, &vtx)?;
    if !model.header.is_static_prop() {
        return Err("Native fixture requires a model authored as $staticprop".into());
    }
    let mut materials = BTreeSet::new();
    let skin = model.skins.first().ok_or("Model has no default skin")?;
    for part in model.parts_for_body(0) {
        let material = &model.materials[skin[part.material]];
        let name = material.strip_suffix(".vmt").unwrap_or(material);
        let candidates = model
            .material_directories
            .iter()
            .map(|directory| {
                if directory.is_empty() {
                    name.to_owned()
                } else {
                    format!("{directory}/{name}")
                }
            })
            .chain(std::iter::once(name.to_owned()));
        let mut resolved = None;
        for candidate in candidates {
            if Material::load_limited(&fs, &candidate, 64 * 1024)?.is_some() {
                resolved = Some(material_path(&candidate));
                break;
            }
        }
        materials
            .insert(resolved.ok_or_else(|| format!("Model material {material} was not found"))?);
    }
    let destination = Path::new(destination);
    std::fs::create_dir_all(destination)?;
    let destination = destination.canonicalize()?;
    let installation = fs.game_dir.canonicalize()?;
    let installation = installation
        .parent()
        .ok_or("Invalid installed game directory")?;
    if destination.starts_with(installation) {
        return Err("The fixture destination must be outside the installed game directory".into());
    }
    mdl[152..156].copy_from_slice(&(model.header.flags | CAST_TEXTURE_SHADOWS).to_le_bytes());
    write(&destination, &path, &mdl)?;
    write(&destination, &format!("{stem}.vvd"), &vvd)?;
    write(&destination, &format!("{stem}.dx90.vtx"), &vtx)?;
    if let Some(phy) = fs.read_limited(&format!("{stem}.phy"), MAX_MODEL_BYTES)? {
        write(&destination, &format!("{stem}.phy"), &phy)?;
    }
    for material in &materials {
        write(&destination, material, b"VertexLitGeneric { $model 1 $basetexture hammerr/prop_cutout $alphatest 1 $alphatestreference 0.5 $nocull 1 }\n")?;
    }
    println!(
        "{}",
        serde_json::json!({
            "model": path,
            "bounds": model.bounds,
            "vertices": model.vertices.len(),
            "triangles": model.parts_for_body(0).map(|part| part.indices.len() / 3).sum::<usize>(),
            "materials": materials,
            "original_flags": model.header.flags,
            "private_game": destination,
        })
    );
    Ok(())
}
