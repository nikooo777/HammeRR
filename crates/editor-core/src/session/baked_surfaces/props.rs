use std::{cell::Cell, collections::BTreeMap, io, sync::Arc};

use compiler::{
    Control,
    asset_sources::{AssetSourceGuard, BuildMaterialSources},
    entity_origins::Origin,
    prop_lighting_assets::{self, PropLightingManifest},
    static_prop_origins::StaticPropOrigins,
};
use render::{
    LightingChannel,
    compiled_faces::{
        MAX_CPU_BYTES,
        materials::{self, Budget, Material},
        props::{Input, Scene},
    },
};
use sha2::{Digest, Sha256};
use source_formats::{
    bsp::{Bsp, PakArchive, PakLimits},
    gamefs::{GameFs, asset_relative},
    model::{HardwareLayout, HardwareLimits, Model},
};

use super::{MAX_METADATA_BYTES, assets};

const MAX_MODELS: usize = 128;
const MAX_MATERIALS: usize = 1024;
const MAX_RECEIVERS: usize = 1024;
const MAX_HARDWARE_VERTICES: usize = 4_000_000;
const MAX_MODEL_WORK: usize = 32_000_000;
const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MATERIAL_SCRATCH_BYTES: usize = MAX_MATERIAL_BYTES * 128;
const MAX_PATH_BYTES: usize = 1024;
const MODEL_METADATA_BYTES: usize = 256;
const MAX_ASSET_LOOKUPS: usize = 16_384;

pub(super) struct Prepared {
    pub(super) scene: Scene,
    pub(super) materials: Vec<Option<Arc<Material>>>,
    pub(super) guard: AssetSourceGuard,
    pub(super) replacements: Vec<(u32, Origin)>,
    pub(super) bytes: usize,
}

impl Prepared {
    pub(super) fn verify(&self, control: &Control) -> Result<(), String> {
        self.guard
            .verify_original_metadata(control, MAX_METADATA_BYTES)
            .map_err(|error| format!("Compiled prop display inputs changed: {error}"))
    }
}

struct ModelData {
    model: Model,
    layout: HardwareLayout,
}

struct Inputs<'a> {
    game: &'a GameFs,
    pak: &'a PakArchive,
    manifest: &'a PropLightingManifest,
    sources: BuildMaterialSources,
    control: &'a Control,
    lookups: Cell<usize>,
}

impl Inputs<'_> {
    fn read(&self, path: &str, exact: bool, maximum: usize) -> io::Result<Option<Vec<u8>>> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let lookups = self
            .lookups
            .get()
            .checked_add(1)
            .filter(|&count| count <= MAX_ASSET_LOOKUPS)
            .ok_or_else(|| {
                io::Error::other("Compiled prop asset lookups exceed their working budget")
            })?;
        self.lookups.set(lookups);
        let path = asset_relative(path)?;
        if path.len() > MAX_PATH_BYTES {
            return Err(io::Error::other(
                "Compiled prop asset path exceeds 1024 bytes",
            ));
        }
        let data = materials::read_asset(
            &path,
            self.pak,
            &mut |path| {
                self.control.checkpoint().map_err(io::Error::other)?;
                let asset = self.sources.read_asset(self.game, path, maximum)?;
                Ok(asset.map(|asset| asset.bytes().to_vec()))
            },
            &mut || self.control.checkpoint(),
            maximum,
        )?;
        if exact {
            match (
                &data,
                self.manifest
                    .sources
                    .iter()
                    .find(|source| source.path == path),
            ) {
                (Some(data), Some(source))
                    if data.len() == source.bytes
                        && format!("{:x}", Sha256::digest(data)) == source.sha256 => {}
                (None, None) if self.manifest.missing_sources.contains(&path) => {}
                _ => {
                    return Err(io::Error::other(format!(
                        "Compiled prop input {path} differs from its published lighting source"
                    )));
                }
            }
        }
        Ok(data)
    }

    fn required(&self, path: &str, maximum: usize) -> Result<Vec<u8>, String> {
        self.read(path, true, maximum)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Required compiled prop input {path} is missing"))
    }
}

pub(super) fn prepare(
    bsp: &Bsp,
    game: &GameFs,
    published: &AssetSourceGuard,
    origins: &StaticPropOrigins,
    other_cpu_bytes: usize,
    control: &Control,
) -> Result<Prepared, String> {
    control.checkpoint()?;
    if game.mounts.len() > 128 {
        return Err("Compiled props exceed the 128 mounted-source working limit".into());
    }
    let bsp_bytes = bsp
        .lumps
        .iter()
        .map(|lump| lump.data.len())
        .chain(bsp.game_lumps.iter().map(|lump| lump.data.len()))
        .try_fold(0usize, usize::checked_add)
        .ok_or("Compiled prop BSP input size overflow")?;
    remaining(&[
        other_cpu_bytes,
        bsp_bytes,
        PakLimits::default().max_total_bytes,
        assets::MAX_ASSET_BYTES,
        MAX_METADATA_BYTES,
    ])?;
    let validated = prop_lighting_assets::read_validated_prop_lighting(bsp, control)
        .map_err(|error| error.to_string())?;
    validated
        .verify_published_sources(published, game, control)
        .map_err(|error| format!("Published prop lighting sources changed: {error}"))?;
    let manifest = validated
        .manifest()
        .ok_or("The owned build has no generated static-prop lighting manifest")?;
    if manifest.receivers.is_empty() || manifest.receivers.len() > MAX_RECEIVERS {
        return Err("Compiled props require 1–1024 validated native receivers".into());
    }
    let native = validated
        .static_props()
        .ok_or("The validated native static-prop table is missing")?;
    let associations = origins
        .associations(bsp, native.props.len())
        .map_err(str::to_owned)?;
    let pak = validated.archive();
    let pak_bytes = pak
        .entries
        .iter()
        .try_fold(0usize, |total, entry| {
            total
                .checked_add(entry.data.len())?
                .checked_add(entry.name.len())?
                .checked_add(size_of_val(entry))
        })
        .ok_or("Compiled prop PAK size overflow")?;
    let source_limits = assets::MAX_ASSET_BYTES;
    let base_bytes = add(&[
        other_cpu_bytes,
        bsp_bytes,
        pak_bytes,
        source_limits,
        MAX_METADATA_BYTES,
        MATERIAL_SCRATCH_BYTES,
    ])?;
    remaining(&[base_bytes])?;
    let inputs = Inputs {
        game,
        pak,
        manifest,
        sources: BuildMaterialSources::bounded(source_limits, 4096, 16_384, 32_768),
        control,
        lookups: Cell::new(0),
    };
    let channels = manifest
        .channels
        .iter()
        .map(|channel| match channel {
            prop_lighting_assets::Channel::Ldr => LightingChannel::Ldr,
            prop_lighting_assets::Channel::Hdr => LightingChannel::Hdr,
        })
        .collect();
    let mut scene = Scene::new(channels)?;
    let mut model_indices = BTreeMap::<String, usize>::new();
    let mut models = Vec::<ModelData>::new();
    let mut model_bytes = 0usize;
    let mut model_work = 0usize;
    let mut total_vertices = 0usize;
    let mut slots = BTreeMap::<(usize, usize), usize>::new();
    let mut materials = Vec::new();
    let mut replacements = Vec::new();
    replacements
        .try_reserve_exact(manifest.receivers.len())
        .map_err(|_| "Compiled prop replacement allocation failed")?;
    let mut retained_metadata = manifest
        .receivers
        .len()
        .checked_mul(size_of::<(u32, Origin)>())
        .ok_or("Compiled prop replacement size overflow")?;
    let mut budget = Budget {
        max_asset_bytes: assets::MAX_ASSET_FILE_BYTES,
        max_texture_axis: 4096,
        max_decoded_bytes: 0,
        decoded_bytes: 0,
    };
    for receiver in &manifest.receivers {
        control.checkpoint()?;
        let prop = native
            .props
            .get(receiver.ordinal as usize)
            .ok_or("Validated compiled prop ordinal is unavailable")?;
        let path = native
            .model_name(usize::from(prop.model))
            .map_err(|error| error.to_string())?;
        let model_index = if let Some(&index) = model_indices.get(path) {
            index
        } else {
            if models.len() >= MAX_MODELS {
                return Err("Compiled props exceed 128 unique model inputs".into());
            }
            let stem = path
                .strip_suffix(".mdl")
                .ok_or("Compiled prop model must end in .mdl")?;
            let first_max = remaining(&[
                base_bytes,
                scene.retained_bytes(),
                model_bytes,
                budget.decoded_bytes,
                retained_metadata,
            ])? / 2;
            let mdl = inputs.required(path, first_max.min(assets::MAX_ASSET_FILE_BYTES))?;
            if mdl.get(377).copied() != Some(0) {
                return Err("Compiled props require the published authored root LOD 0".into());
            }
            let second_max = remaining(&[
                base_bytes,
                scene.retained_bytes(),
                model_bytes,
                budget.decoded_bytes,
                retained_metadata,
                mdl.len(),
            ])? / 2;
            let vvd = inputs.required(
                &format!("{stem}.vvd"),
                second_max.min(assets::MAX_ASSET_FILE_BYTES),
            )?;
            let third_max = remaining(&[
                base_bytes,
                scene.retained_bytes(),
                model_bytes,
                budget.decoded_bytes,
                retained_metadata,
                mdl.len(),
                vvd.len(),
            ])? / 2;
            let vtx = inputs.required(
                &format!("{stem}.dx90.vtx"),
                third_max.min(assets::MAX_ASSET_FILE_BYTES),
            )?;
            let parse_limit = remaining(&[
                base_bytes,
                scene.retained_bytes(),
                model_bytes,
                budget.decoded_bytes,
                retained_metadata,
                mdl.len(),
                vvd.len(),
                vtx.len(),
            ])?;
            let mut parsed_bytes = 0usize;
            let (model, layout) = Model::parse_with_hardware(
                &mdl,
                &vvd,
                &vtx,
                HardwareLimits {
                    sections: prop_lighting_assets::MAX_RECEIVER_MESHES,
                    vertices: MAX_HARDWARE_VERTICES,
                    bytes: parse_limit.min(source_formats::model::MAX_HARDWARE_BYTES),
                    work: MAX_MODEL_WORK.saturating_sub(model_work),
                },
                &mut |charge| {
                    control.checkpoint().map_err(io::Error::other)?;
                    parsed_bytes = parsed_bytes
                        .checked_add(charge.bytes)
                        .filter(|&bytes| bytes <= parse_limit)
                        .ok_or_else(|| {
                            io::Error::other(
                                "Compiled prop model parsing exceeds the combined CPU budget",
                            )
                        })?;
                    model_work = model_work
                        .checked_add(charge.work)
                        .filter(|&work| work <= MAX_MODEL_WORK)
                        .ok_or_else(|| {
                            io::Error::other("Compiled prop model parsing exceeds its work budget")
                        })?;
                    Ok(())
                },
            )
            .map_err(|error| format!("Compiled prop {path}: {error}"))?;
            if layout.checksum as u32 != receiver.checksum {
                return Err(
                    "Compiled prop model checksum differs from published VHV identity".into(),
                );
            }
            model_bytes = add(&[
                model_bytes,
                model.memory_bytes(),
                layout.memory_bytes(),
                path.len(),
                MODEL_METADATA_BYTES,
            ])?;
            let index = models.len();
            models
                .try_reserve_exact(1)
                .map_err(|_| "Compiled prop model allocation failed")?;
            models.push(ModelData { model, layout });
            model_indices.insert(path.to_owned(), index);
            index
        };
        let data = &models[model_index];
        total_vertices = total_vertices
            .checked_add(data.layout.vertices.len())
            .filter(|&count| count <= MAX_HARDWARE_VERTICES)
            .ok_or("Compiled props exceed four million hardware colour vertices")?;
        let skin = usize::try_from(prop.skin)
            .ok()
            .and_then(|skin| data.model.skins.get(skin))
            .ok_or("Compiled prop selected skin is unavailable")?;
        let mut section_materials = Vec::new();
        section_materials
            .try_reserve_exact(data.layout.sections.len())
            .map_err(|_| "Compiled prop section material allocation failed")?;
        for section in &data.layout.sections {
            control.checkpoint()?;
            let material_index = data
                .model
                .parts
                .get(section.part)
                .and_then(|part| skin.get(part.material))
                .copied()
                .ok_or("Compiled prop selected hardware material is unavailable")?;
            let key = (model_index, material_index);
            let slot = if let Some(&slot) = slots.get(&key) {
                slot
            } else {
                if materials.len() >= MAX_MATERIALS {
                    return Err("Compiled props exceed 1024 selected material inputs".into());
                }
                let name = data
                    .model
                    .materials
                    .get(material_index)
                    .ok_or("Compiled prop material name is unavailable")?;
                let name = name.strip_suffix(".vmt").unwrap_or(name);
                let mut selected = None;
                for candidate in source_formats::model::material_candidates(
                    name,
                    &data.model.material_directories,
                ) {
                    control.checkpoint()?;
                    let path = format!("materials/{candidate}.vmt");
                    let material = compiler::load_prop_vertex_material(&path, &mut |path| {
                        inputs.read(path, true, MAX_MATERIAL_BYTES)
                    })?;
                    if let Some(material) = material {
                        selected = Some((candidate, material));
                        break;
                    }
                }
                let (name, resolved) = selected.ok_or(
                    "Compiled prop selected material was not found in its exact published closure",
                )?;
                budget.max_decoded_bytes = remaining(&[
                    base_bytes,
                    scene.retained_bytes(),
                    model_bytes,
                    retained_metadata,
                ])?
                .min(assets::MAX_DECODED_BYTES);
                let material = materials::load_static_prop_resolved(
                    &name,
                    &resolved,
                    pak,
                    &mut |path| inputs.read(path, false, assets::MAX_ASSET_FILE_BYTES),
                    &mut || control.checkpoint(),
                    &mut budget,
                )?;
                let slot = materials.len();
                retained_metadata = add(&[
                    retained_metadata,
                    size_of::<Option<Arc<Material>>>(),
                    size_of::<Material>(),
                    name.len(),
                    MODEL_METADATA_BYTES,
                ])?;
                remaining(&[
                    base_bytes,
                    scene.retained_bytes(),
                    model_bytes,
                    retained_metadata,
                    budget.decoded_bytes,
                ])?;
                materials
                    .try_reserve_exact(1)
                    .map_err(|_| "Compiled prop material allocation failed")?;
                materials.push(Some(Arc::new(material)));
                slots.insert(key, slot);
                slot
            };
            section_materials.push(slot);
        }
        let channel_data = [
            prop_lighting_assets::Channel::Ldr,
            prop_lighting_assets::Channel::Hdr,
        ]
        .map(|channel| {
            validated
                .asset(receiver.ordinal, channel)
                .map(|(_, bytes)| bytes)
        });
        let maximum = remaining(&[
            base_bytes,
            model_bytes,
            retained_metadata,
            budget.decoded_bytes,
            section_materials.len() * size_of::<usize>(),
        ])?;
        scene.push(
            Input {
                ordinal: receiver.ordinal,
                model_name: path,
                prop,
                model: &data.model,
                layout: &data.layout,
                materials: &section_materials,
                channels: channel_data,
            },
            maximum,
            &mut || control.checkpoint().map_err(io::Error::other),
        )?;
        let origin = associations
            .get(receiver.ordinal as usize)
            .ok_or("Compiled prop source association is unavailable")?;
        if replacements.iter().any(|(_, existing)| existing == origin) {
            return Err("Compiled prop replacement source identity is ambiguous".into());
        }
        retained_metadata = add(&[retained_metadata, origin.source_bytes()])?;
        remaining(&[
            base_bytes,
            scene.retained_bytes(),
            model_bytes,
            retained_metadata,
            budget.decoded_bytes,
        ])?;
        replacements.push((receiver.ordinal, origin.try_clone()?));
    }
    drop(models);
    drop(model_indices);
    drop(slots);
    validated
        .verify_published_sources(published, game, control)
        .map_err(|error| {
            format!("Published prop lighting sources changed during preparation: {error}")
        })?;
    let guard = inputs
        .sources
        .source_guard(game, control)
        .map_err(|error| error.to_string())?;
    let prepared = Prepared {
        scene,
        materials,
        guard,
        replacements,
        bytes: add(&[retained_metadata, budget.decoded_bytes])?,
    };
    prepared.verify(control)?;
    control.checkpoint()?;
    Ok(prepared)
}

fn add(values: &[usize]) -> Result<usize, String> {
    values
        .iter()
        .try_fold(0usize, |total, &bytes| total.checked_add(bytes))
        .ok_or_else(|| "Compiled prop byte accounting overflow".into())
}

fn remaining(values: &[usize]) -> Result<usize, String> {
    MAX_CPU_BYTES.checked_sub(add(values)?).ok_or_else(|| {
        "Compiled props and surfaces exceed the combined 256 MiB CPU working budget".into()
    })
}
