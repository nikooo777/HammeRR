use std::io;

use serde::Serialize;

use super::process::StoppedProcess;
use crate::util::invalid;

const MODEL_LOADER_POINTER: u64 = 0x00a0_0d50;
const MODEL_LOADER_INSTANCE: u64 = 0x00b9_d480;
const MODEL_LOADER_WORLD_MODEL: u64 = 0x90;
const MODEL_LOADER_WORLD: u64 = 0x98;
const MODEL_LOADER_PUBLICATION: u64 = 0x2a0;
const MODEL_LOADER_SIZE: usize = 0x2a1;
const LOAD_DEPTH: u64 = 0x00b9_ce10;
const WORLD_MODEL: u64 = 0x00c5_8090;
const WORLD: u64 = 0x00c5_8098;
const MODEL_NAME: u64 = 8;
const MODEL_FLAGS: u64 = 0x10;
const MODEL_TYPE: u64 = 0x20;
const MODEL_WORLD: u64 = 0x48;
const MODEL_SIZE: usize = 0x50;
const MODEL_LOADED: u32 = 1;
const BRUSH_MODEL: u32 = 1;
const MAP_NAME_LIMIT: usize = 260;
const WORLD_DISPLACEMENT_COUNT: u64 = 0xc8;
const WORLD_DISPLACEMENT_HANDLE: u64 = 0xd0;
const HANDLE_BASE: u64 = 8;
const HANDLE_COUNT: u64 = 0x10;
const HANDLE_SIZE: usize = 0x18;
pub(super) const DISPLACEMENT_STRIDE: usize = 0x310;
pub(super) const MAX_DISPLACEMENTS: usize = 2048;

#[derive(Debug)]
pub(super) enum Readiness {
    Pending(PendingReason),
    Ready(ReadyWorld),
}

#[derive(Debug)]
pub(super) enum PendingReason {
    Loading { depth: u32 },
    Unpublished,
    WorldUnavailable,
    Retiring { model_flags: u32 },
}

impl std::fmt::Display for PendingReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading { depth } => write!(formatter, "native map load depth is {depth}"),
            Self::Unpublished => formatter.write_str("native terrain is not published"),
            Self::WorldUnavailable => formatter.write_str("native world is not available"),
            Self::Retiring { model_flags } => {
                write!(
                    formatter,
                    "native world is retiring (flags {model_flags:#x})"
                )
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ReadyWorld {
    pub load_bias: u64,
    pub loader: u64,
    pub model: u64,
    pub world: u64,
    pub handle: u64,
    pub displacement_base: u64,
    pub displacement_count: usize,
    pub map_name: String,
    pub publication_address: u64,
    pub publication: u8,
    pub load_depth_address: u64,
    pub load_depth: u32,
    pub model_flags_address: u64,
    pub model_flags: u32,
    pub model_type: u32,
}

pub(super) fn inspect(
    process: &mut StoppedProcess,
    load_bias: u64,
    expected_map: &str,
) -> io::Result<Readiness> {
    if expected_map.is_empty()
        || expected_map.len() >= MAP_NAME_LIMIT
        || !expected_map.is_ascii()
        || expected_map.as_bytes().contains(&0)
    {
        return Err(invalid("Invalid private terrain snapshot map name"));
    }
    let loader_pointer = address(load_bias, MODEL_LOADER_POINTER)?;
    process.require_range(loader_pointer, size_of::<u64>(), true)?;
    let loader = process.read_u64(loader_pointer)?;
    if loader != address(load_bias, MODEL_LOADER_INSTANCE)? {
        return Err(invalid("Native terrain model-loader identity changed"));
    }
    process.require_range(loader, MODEL_LOADER_SIZE, true)?;
    let load_depth_address = address(load_bias, LOAD_DEPTH)?;
    process.require_range(load_depth_address, size_of::<u32>(), true)?;
    let load_depth = process.read_u32(load_depth_address)?;
    if load_depth != 0 {
        return Ok(Readiness::Pending(PendingReason::Loading {
            depth: load_depth,
        }));
    }
    let publication_address = address(loader, MODEL_LOADER_PUBLICATION)?;
    let publication = process.read_u8(publication_address)?;
    match publication {
        0 => return Ok(Readiness::Pending(PendingReason::Unpublished)),
        1 => {}
        _ => return Err(invalid("Invalid native terrain publication flag")),
    }
    let model_pointer = address(load_bias, WORLD_MODEL)?;
    let world_pointer = address(load_bias, WORLD)?;
    process.require_range(model_pointer, size_of::<u64>(), true)?;
    process.require_range(world_pointer, size_of::<u64>(), true)?;
    let model = process.read_u64(model_pointer)?;
    let world = process.read_u64(world_pointer)?;
    if model == 0 || world == 0 {
        return Ok(Readiness::Pending(PendingReason::WorldUnavailable));
    }
    process.require_range(model, MODEL_SIZE, true)?;
    let model_flags_address = address(model, MODEL_FLAGS)?;
    let model_flags = process.read_u32(model_flags_address)?;
    if model_flags & MODEL_LOADED == 0 {
        return Ok(Readiness::Pending(PendingReason::Retiring { model_flags }));
    }
    let model_type = process.read_u32(address(model, MODEL_TYPE)?)?;
    if model_type != BRUSH_MODEL
        || world != address(loader, MODEL_LOADER_WORLD)?
        || process.read_u64(address(loader, MODEL_LOADER_WORLD_MODEL)?)? != model
        || process.read_u64(address(model, MODEL_WORLD)?)? != world
    {
        return Err(invalid("Native terrain world-model binding changed"));
    }
    let name_pointer = process.read_u64(address(model, MODEL_NAME)?)?;
    if name_pointer == 0 {
        return Err(invalid("Native terrain world model has no map name"));
    }
    let map_name = process.read_c_string(name_pointer, MAP_NAME_LIMIT)?;
    if map_name != expected_map {
        return Err(invalid(format!(
            "Native terrain loaded {map_name:?}, expected private map {expected_map:?}"
        )));
    }
    let world_count = process.read_u32(address(world, WORLD_DISPLACEMENT_COUNT)?)?;
    let displacement_count = usize::try_from(world_count)
        .map_err(|_| invalid("Native terrain displacement count exceeds address space"))?;
    if displacement_count > MAX_DISPLACEMENTS {
        return Err(invalid("Native terrain displacement count exceeds 2048"));
    }
    let handle = process.read_u64(address(world, WORLD_DISPLACEMENT_HANDLE)?)?;
    if handle == 0 {
        return Err(invalid(
            "Published native terrain has no displacement handle",
        ));
    }
    process.require_range(handle, HANDLE_SIZE, true)?;
    if process.read_u32(address(handle, HANDLE_COUNT)?)? != world_count {
        return Err(invalid("Native terrain displacement counts disagree"));
    }
    let displacement_base = process.read_u64(address(handle, HANDLE_BASE)?)?;
    if displacement_count == 0 {
        if displacement_base != 0 {
            return Err(invalid("Empty native terrain has a displacement array"));
        }
    } else {
        process.checked_array(
            displacement_base,
            displacement_count,
            DISPLACEMENT_STRIDE,
            MAX_DISPLACEMENTS,
        )?;
    }
    Ok(Readiness::Ready(ReadyWorld {
        load_bias,
        loader,
        model,
        world,
        handle,
        displacement_base,
        displacement_count,
        map_name,
        publication_address,
        publication,
        load_depth_address,
        load_depth,
        model_flags_address,
        model_flags,
        model_type,
    }))
}

fn address(base: u64, offset: u64) -> io::Result<u64> {
    base.checked_add(offset)
        .ok_or_else(|| invalid("Native terrain readiness address overflow"))
}
