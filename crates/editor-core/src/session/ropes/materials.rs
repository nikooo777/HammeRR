use std::collections::BTreeMap;
use std::io;

use compiler::Control;
use compiler::asset_sources::{AssetSourceGuard, BuildMaterialSources};
use render::MaterialLibrary;
use render::ropes::Preview;
use render::ropes::materials::{self, Asset, Prepared};
use source_formats::gamefs::GameFs;
use source_formats::vmt::material_path;

const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_LOGICAL_PATHS: usize = 2048;
const MAX_PHYSICAL_PATHS: usize = 4096;
const MAX_LOOKUP_ROWS: usize = 8192;
const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_MOUNTS: usize = 128;

#[derive(Debug)]
pub(in crate::session) struct Assets {
    by_material: BTreeMap<String, Asset>,
    by_source: BTreeMap<u32, String>,
    guard: AssetSourceGuard,
}

fn observe(
    sources: &BuildMaterialSources,
    game: &GameFs,
    path: &str,
    maximum: usize,
    control: &Control,
) -> io::Result<Option<Vec<u8>>> {
    control.checkpoint().map_err(io::Error::other)?;
    if path.len() > materials::MAX_PATH_BYTES {
        return Err(io::Error::other("Rope asset path exceeds 256 bytes"));
    }
    let asset = sources.read_asset(game, path, maximum)?;
    Ok(asset.map(|asset| asset.bytes().to_vec()))
}

pub(in crate::session) fn prepare(
    game: &GameFs,
    preview: &Preview,
    control: &Control,
) -> Result<Assets, String> {
    control.checkpoint()?;
    if game.mounts.len() > MAX_MOUNTS {
        return Err("Rope material preview supports at most 128 mounted search paths".into());
    }
    let sources = BuildMaterialSources::bounded(
        MAX_SOURCE_BYTES,
        MAX_LOGICAL_PATHS,
        MAX_PHYSICAL_PATHS,
        MAX_LOOKUP_ROWS,
    );
    let mut by_material = BTreeMap::new();
    let mut by_source = BTreeMap::new();
    let mut decoded_bytes = 0usize;
    let mut retained_bytes = 0usize;
    for link in &preview.links {
        control.checkpoint()?;
        let name = material_path(&link.material);
        if !by_material.contains_key(&name) {
            if by_material.len() >= materials::MAX_MATERIALS {
                return Err("Rope appearance snapshot exceeds 128 materials".into());
            }
            let asset = Asset::load(&link.material, &mut |path, maximum| {
                observe(&sources, game, path, maximum, control)
            })
            .map_err(|error| format!("entity:{}: {error}", link.source))?;
            decoded_bytes = decoded_bytes
                .checked_add(asset.decoded_bytes)
                .ok_or("Rope decoded texture total overflow")?;
            retained_bytes = retained_bytes
                .checked_add(asset.retained_bytes)
                .ok_or("Rope retained texture total overflow")?;
            if decoded_bytes > materials::MAX_DECODED_BYTES || retained_bytes > MAX_SOURCE_BYTES {
                return Err(
                    "Rope appearance snapshot exceeds 64 MiB decoded or retained texture data"
                        .into(),
                );
            }
            by_material.insert(name.clone(), asset);
        }
        by_source.insert(link.source, name);
    }
    let guard = sources
        .source_guard(game, control)
        .map_err(|error| format!("Rope material sources: {error}"))?;
    guard
        .verify_original_metadata(control, MAX_METADATA_BYTES)
        .map_err(|error| format!("Rope material sources: {error}"))?;
    control.checkpoint()?;
    Ok(Assets {
        by_material,
        by_source,
        guard,
    })
}

impl Assets {
    pub(in crate::session) fn verify_sources(
        &self,
        game: &GameFs,
        control: &Control,
    ) -> Result<(), String> {
        self.guard
            .verify_original_metadata(control, MAX_METADATA_BYTES)
            .map_err(|error| format!("Rope material sources: {error}"))?;
        self.guard
            .verify_sources(game, control)
            .map_err(|error| format!("Rope material sources: {error}"))
    }

    pub(in crate::session) fn bind(
        self,
        library: &mut MaterialLibrary,
    ) -> Result<(Prepared, AssetSourceGuard), String> {
        for asset in self.by_material.values() {
            asset.check_binding(library)?;
        }
        if self
            .by_source
            .values()
            .any(|name| !self.by_material.contains_key(name))
        {
            return Err("Rope source lost its admitted material".into());
        }
        let mut appearances = BTreeMap::new();
        for (name, asset) in self.by_material {
            appearances.insert(name, asset.bind(library));
        }
        let mut by_source = BTreeMap::new();
        for (source, name) in self.by_source {
            let appearance = appearances[&name].clone();
            by_source.insert(source, appearance);
        }
        Ok((
            Prepared {
                by_source,
                generation: library.generation(),
                ..Prepared::default()
            },
            self.guard,
        ))
    }
}
