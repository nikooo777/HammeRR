use std::collections::{BTreeMap, BTreeSet};

use compiler::asset_sources::BuildMaterialSources;
use render::{
    MaterialId,
    models::{Asset, Instance},
};
use source_formats::{
    gamefs::GameFs,
    vmt::{Material, material_path, texture_path},
};

use super::*;

pub(super) const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
const MAX_ASSET_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_MATERIALS: usize = 128;
const MAX_MODEL_ASSETS: usize = 64;
const MAX_PATH_BYTES: usize = 1024;
const MAX_MOUNTS: usize = 128;

fn observe(
    sources: &BuildMaterialSources,
    game: &GameFs,
    path: &str,
    maximum: usize,
    control: &Control,
) -> std::io::Result<Option<Vec<u8>>> {
    control.checkpoint().map_err(std::io::Error::other)?;
    if path.len() > MAX_PATH_BYTES {
        return Err(std::io::Error::other(
            "Compiled detail preview paths exceed 1024 bytes",
        ));
    }
    let asset = sources.read_asset(game, path, maximum)?;
    Ok(asset.map(|asset| asset.bytes().to_vec()))
}

impl Session {
    fn prepare_detail_material(
        &mut self,
        name: &str,
        sources: &BuildMaterialSources,
        control: &Control,
    ) -> Result<MaterialId, String> {
        if name.len() > MAX_PATH_BYTES {
            return Err("Compiled detail material paths exceed 1024 bytes".into());
        }
        let material = Material::load_with_reader(&material_path(name), &mut |path| {
            observe(sources, &self.game, path, MAX_MATERIAL_BYTES, control)
        })
        .map_err(|error| format!("Detail material {name}: {error}"))?
        .ok_or_else(|| format!("Detail material {name} is missing"))?;
        if material.shader != "unlitgeneric"
            || !material.blocks.is_empty()
            || material.params.iter().any(|(key, _)| {
                ![
                    "$basetexture",
                    "$model",
                    "$surfaceprop",
                    "$translucent",
                    "$alphatest",
                    "$nocull",
                    "$vertexcolor",
                    "$vertexalpha",
                ]
                .contains(&key.as_str())
            })
        {
            return Err(format!(
                "Detail material {name} needs a static UnlitGeneric base texture; effects, overrides and other parameters are unsupported by this preview"
            ));
        }
        let texture = material
            .get("$basetexture")
            .filter(|path| !path.is_empty())
            .ok_or_else(|| format!("Detail material {name} has no base texture"))?;
        if texture.len() > MAX_PATH_BYTES {
            return Err("Compiled detail texture paths exceed 1024 bytes".into());
        }
        let texture = texture_path(texture);
        observe(sources, &self.game, &texture, MAX_ASSET_FILE_BYTES, control)
            .map_err(|error| format!("Detail texture {texture}: {error}"))?
            .ok_or_else(|| format!("Detail texture {texture} is missing"))?;
        let id = self.materials.reload_limited(
            &self.game,
            name,
            [MAX_MATERIAL_BYTES, MAX_ASSET_FILE_BYTES],
        )?;
        let info = self.materials.get(id);
        if info.error.is_some()
            || info.missing
            || !info.matches_material(&material)
            || info.display_texture.as_deref() != Some(texture.as_str())
            || info.base.as_ref().is_none_or(|texture| {
                texture.frames != 1
                    || texture.faces != 1
                    || texture.depth != 1
                    || texture.width > 4096
                    || texture.height > 4096
            })
        {
            return Err(format!(
                "Detail material {name} has an unavailable or unsupported static 2D display texture"
            ));
        }
        control.checkpoint()?;
        Ok(id)
    }

    pub(super) fn prepare_detail_assets(
        &mut self,
        records: &DetailProps,
        atlas: &str,
        control: &Control,
    ) -> Result<(SceneMesh, AssetSourceGuard, usize), String> {
        if self.game.mounts.len() > MAX_MOUNTS {
            return Err("Compiled detail previews support at most 128 mounted search paths".into());
        }
        let sources = BuildMaterialSources::bounded(MAX_ASSET_BYTES, 512, 4096, 8192);
        let mut materials = BTreeMap::new();
        let mut assets = BTreeMap::<String, Arc<Asset>>::new();
        let mut instances = Vec::new();
        instances
            .try_reserve_exact(records.props.len())
            .map_err(|_| "Detail instance allocation failed")?;
        let mut parts = Vec::new();
        parts
            .try_reserve_exact(records.props.len())
            .map_err(|_| "Compiled detail spatial allocation failed")?;
        let mut vertices = 0usize;
        let mut indices = 0usize;
        let mut asset_bytes = 0usize;
        let mut texture_bytes = 0usize;
        let mut textures = BTreeSet::new();
        let empty = render::compiled_details::model_scene(&self.game, &mut self.materials, &[]);
        for (ordinal, prop) in records
            .props
            .iter()
            .enumerate()
            .filter(|(_, prop)| prop.kind == 0)
        {
            control.checkpoint()?;
            if prop.orientation != 0 {
                return Err(format!(
                    "Detail record {ordinal}: camera-facing model orientations are unsupported"
                ));
            }
            let name = records
                .model_name(usize::from(prop.model))
                .map_err(|error| error.to_string())?;
            if !assets.contains_key(name) {
                if assets.len() >= MAX_MODEL_ASSETS {
                    return Err("Compiled details exceed 64 unique model assets".into());
                }
                let stem = name
                    .strip_suffix(".mdl")
                    .ok_or("Compiled detail model must end in .mdl")?;
                let paths = [
                    name.to_owned(),
                    format!("{stem}.vvd"),
                    format!("{stem}.dx90.vtx"),
                ];
                let mut inputs = Vec::with_capacity(paths.len());
                for path in paths {
                    inputs.push(
                        observe(&sources, &self.game, &path, MAX_ASSET_FILE_BYTES, control)
                            .map_err(|error| format!("Detail model {name}: {error}"))?
                            .ok_or_else(|| format!("Detail model asset {path} is missing"))?,
                    );
                }
                let identity = render::models::source_identity(&inputs[0], &inputs[1], &inputs[2]);
                let asset = self
                    .models
                    .borrow_mut()
                    .get_verified(&self.game, name, identity)
                    .map_err(|error| format!("Detail model {name}: {error}"))?;
                drop(inputs);
                asset_bytes = asset_bytes
                    .checked_add(asset.memory_bytes())
                    .ok_or("Detail model asset size overflow")?;
                if asset_bytes > MAX_ASSET_BYTES {
                    return Err("Compiled detail model assets exceed 64 MiB".into());
                }
                let bindings = asset.materials()?;
                let skin = asset
                    .model
                    .skins
                    .first()
                    .ok_or_else(|| format!("Detail model {name} has no skin 0"))?;
                for part in asset.model.parts_for_body(0) {
                    let material = skin
                        .get(part.material)
                        .ok_or("Detail model skin material is invalid")?;
                    let name = bindings
                        .get(*material)
                        .ok_or("Detail model material binding is invalid")?;
                    if !materials.contains_key(name) {
                        if materials.len() >= MAX_MATERIALS {
                            return Err("Compiled details exceed 128 materials".into());
                        }
                        let id = self.prepare_detail_material(name, &sources, control)?;
                        charge_texture(self.materials.get(id), &mut textures, &mut texture_bytes)?;
                        materials.insert(name.clone(), id);
                    }
                }
                drop(bindings);
                assets.insert(name.to_owned(), asset);
            }
            let instance =
                Instance::compiled_detail(assets[name].clone(), prop.origin, prop.angles)
                    .map_err(|error| format!("Detail record {ordinal}, {name}: {error}"))?;
            if !instance.has_geometry() {
                return Err(format!(
                    "Detail record {ordinal}, {name} has no body-0 geometry"
                ));
            }
            let first_vertex = vertices;
            let first_wire = indices * 2;
            let bounds = instance.bounds();
            vertices = vertices
                .checked_add(instance.asset.model.vertices.len())
                .ok_or("Detail vertex count overflow")?;
            indices = indices
                .checked_add(
                    instance
                        .asset
                        .model
                        .parts_for_body(0)
                        .map(|part| part.indices.len())
                        .sum::<usize>(),
                )
                .ok_or("Detail index count overflow")?;
            empty.check_compiled_detail_capacity(vertices, indices)?;
            parts.push(render::compiled_details::Part {
                ordinal,
                vertices: first_vertex..vertices,
                wires: first_wire..indices * 2,
                billboards: 0..0,
                bounds: [bounds.0, bounds.1],
            });
            instances.push(instance);
        }
        if records.props.iter().any(|prop| prop.kind != 0) {
            if !materials.contains_key(atlas) && materials.len() >= MAX_MATERIALS {
                return Err("Compiled details exceed 128 materials".into());
            }
            let id = self.prepare_detail_material(atlas, &sources, control)?;
            charge_texture(self.materials.get(id), &mut textures, &mut texture_bytes)?;
            materials.insert(atlas.to_owned(), id);
        }
        let mut mesh =
            render::compiled_details::model_scene(&self.game, &mut self.materials, &instances);
        mesh.check_compiled_detail_capacity(0, 0)?;
        if mesh.vertices.len() != vertices
            || mesh.indices.len() != indices
            || mesh.wire_lines.len() != indices * 2
        {
            return Err("Compiled detail model rendering ranges changed during preparation".into());
        }
        mesh.detail_parts = parts;
        for (ordinal, prop) in records
            .props
            .iter()
            .enumerate()
            .filter(|(_, prop)| prop.kind != 0)
        {
            control.checkpoint()?;
            let first_vertex = mesh.vertices.len();
            let first_wire = mesh.wire_lines.len();
            let first_billboard = mesh.detail_billboards.len();
            mesh.add_compiled_detail_card(
                materials[atlas],
                prop,
                records.sprites[usize::from(prop.model)],
            )
            .map_err(|error| format!("Detail record {ordinal}: {error}"))?;
            let bounds = if mesh.detail_billboards.len() != first_billboard {
                mesh.detail_billboards[first_billboard].bounds()
            } else {
                mesh.vertices[first_vertex..]
                    .iter()
                    .map(|vertex| glam::DVec3::from_array(vertex.position.map(f64::from)))
                    .fold(
                        [
                            glam::DVec3::splat(f64::INFINITY),
                            glam::DVec3::splat(f64::NEG_INFINITY),
                        ],
                        |[low, high], point| [low.min(point), high.max(point)],
                    )
            };
            mesh.detail_parts.push(render::compiled_details::Part {
                ordinal,
                vertices: first_vertex..mesh.vertices.len(),
                wires: first_wire..mesh.wire_lines.len(),
                billboards: first_billboard..mesh.detail_billboards.len(),
                bounds,
            });
        }
        if mesh.compiled_detail_bytes() > render::compiled_details::MAX_BYTES {
            return Err("Compiled detail retained geometry exceeds 32 MiB".into());
        }
        let guard = sources
            .original_source_guard(control, MAX_METADATA_BYTES)
            .map_err(|error| error.to_string())?;
        control.checkpoint()?;
        Ok((mesh, guard, asset_bytes + texture_bytes))
    }
}

fn charge_texture(
    info: &render::materials::MaterialInfo,
    textures: &mut BTreeSet<String>,
    bytes: &mut usize,
) -> Result<(), String> {
    let name = info
        .display_texture
        .as_ref()
        .ok_or("Detail material has no display texture identity")?;
    if textures.insert(name.clone()) {
        let texture = info
            .base
            .as_ref()
            .ok_or("Detail material has no display texture")?;
        let decoded = (texture.width as usize)
            .checked_mul(texture.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("Detail texture size overflow")?;
        *bytes = bytes
            .checked_add(
                decoded
                    .checked_mul(2)
                    .ok_or("Detail texture mip size overflow")?,
            )
            .ok_or("Detail texture total overflow")?;
        if *bytes > MAX_ASSET_BYTES {
            return Err("Compiled detail texture payloads exceed 64 MiB".into());
        }
    }
    Ok(())
}
