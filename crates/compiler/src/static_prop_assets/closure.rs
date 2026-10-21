use std::collections::BTreeSet;
use std::io;

use source_formats::bsp::{self, Bsp, PakLimits, StaticProps};
use source_formats::gamefs::asset_relative;
use source_formats::model::{self, HardwareCharge, HardwareLimits};

use super::manifest::StaticPropPackedUse;
use super::{
    MAX_LOOKUPS, MAX_MATERIAL_BYTES, MAX_MATERIAL_TOTAL_BYTES, checkpoint, invalid, material,
};
use crate::Control;
use crate::asset_sources::hash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Roots {
    pub version: u16,
    pub sha256: String,
    pub models: Vec<String>,
    pub uses: Vec<StaticPropPackedUse>,
}

impl Roots {
    pub fn from_bsp(bsp: &Bsp) -> io::Result<Self> {
        if bsp.lumps.len() != bsp::LUMP_COUNT {
            return Err(invalid("BSP must have exactly 64 lump slots"));
        }
        let mut lumps = bsp.game_lumps.iter().filter(|lump| lump.id == *b"sprp");
        let Some(lump) = lumps.next() else {
            return Ok(Self {
                version: 0,
                sha256: hash(&[]),
                models: Vec::new(),
                uses: Vec::new(),
            });
        };
        if lumps.next().is_some() || lump.flags != 0 {
            return Err(invalid("Static-prop game lump is duplicated or compressed"));
        }
        let leaves = &bsp.lumps[bsp::lump::LEAFS];
        let stride = match leaves.version {
            0 => 56,
            1 => std::mem::size_of::<bsp::Leaf>(),
            _ => return Err(invalid("Unsupported static-prop leaf lump version")),
        };
        if leaves.fourcc != [0; 4]
            || leaves.data.starts_with(b"LZMA")
            || !leaves.data.len().is_multiple_of(stride)
        {
            return Err(invalid("Static-prop leaf lump is compressed or malformed"));
        }
        let props = StaticProps::parse(lump.version, &lump.data, leaves.data.len() / stride)?;
        let mut models = Vec::with_capacity(props.models.len());
        let mut unique = BTreeSet::new();
        for index in 0..props.models.len() {
            let path = props.model_name(index)?;
            canonical_model_path(path)?;
            if props.models[index] != StaticProps::model_record(path)?
                || !unique.insert(path.to_owned())
            {
                return Err(invalid(
                    "Static-prop dictionary must contain unique canonical zero-padded paths",
                ));
            }
            models.push(path.to_owned());
        }
        let mut uses = Vec::with_capacity(props.props.len());
        for prop in props.props {
            if prop.skin < 0 || !matches!(prop.solid, 0 | 2 | 6) {
                return Err(invalid(
                    "Static-prop packing supports nonnegative skins and solid 0, 2 or 6",
                ));
            }
            uses.push(StaticPropPackedUse {
                model: prop.model,
                skin: prop.skin,
                solid: prop.solid,
            });
        }
        Ok(Self {
            version: lump.version,
            sha256: hash(&lump.data),
            models,
            uses,
        })
    }
}

pub(super) fn canonical_model_path(path: &str) -> io::Result<()> {
    if asset_relative(path)? != path
        || !path.starts_with("models/")
        || !path.ends_with(".mdl")
        || path.len() >= bsp::STATIC_PROP_NAME_BYTES
    {
        return Err(invalid(format!(
            "Static-prop model path is not canonical: {path}"
        )));
    }
    Ok(())
}

pub(super) fn collect_closure(
    roots: &Roots,
    limits: PakLimits,
    control: &Control,
    read: &mut impl FnMut(&str, usize, bool) -> io::Result<Option<Vec<u8>>>,
) -> io::Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    let mut attempted = 0usize;
    let mut material_paths = BTreeSet::new();
    let mut texture_paths = BTreeSet::new();
    let mut material_bytes = 0usize;
    let mut charged = HardwareCharge::default();
    let hardware_limits = HardwareLimits::default();
    for (index, path) in roots.models.iter().enumerate() {
        checkpoint(control)?;
        let basename = path.strip_suffix(".mdl").unwrap();
        let mdl = required(
            path,
            limits.max_entry_bytes,
            true,
            &mut paths,
            &mut attempted,
            limits,
            control,
            read,
        )?;
        let vvd_path = format!("{basename}.vvd");
        let vtx_path = format!("{basename}.dx90.vtx");
        let vvd = required(
            &vvd_path,
            limits.max_entry_bytes,
            false,
            &mut paths,
            &mut attempted,
            limits,
            control,
            read,
        )?;
        let vtx = required(
            &vtx_path,
            limits.max_entry_bytes,
            false,
            &mut paths,
            &mut attempted,
            limits,
            control,
            read,
        )?;
        let model = model::validate_portable_static(
            path,
            &mdl,
            &vvd,
            &vtx,
            hardware_limits,
            &mut |charge| {
                checkpoint(control)?;
                charged.bytes = charged
                    .bytes
                    .checked_add(charge.bytes)
                    .filter(|bytes| *bytes <= hardware_limits.bytes)
                    .ok_or_else(|| {
                        invalid("Static-prop aggregate model parse allocation budget exceeded")
                    })?;
                charged.work = charged
                    .work
                    .checked_add(charge.work)
                    .filter(|work| *work <= hardware_limits.work)
                    .ok_or_else(|| {
                        invalid("Static-prop aggregate model parse work budget exceeded")
                    })?;
                Ok(())
            },
        )?;
        let mut physics = false;
        for usage in roots
            .uses
            .iter()
            .filter(|usage| usize::from(usage.model) == index)
        {
            checkpoint(control)?;
            if usage.skin as usize >= model.skins.len() {
                return Err(invalid(format!(
                    "Static-prop skin exceeds the original model families: {path}"
                )));
            }
            physics |= usage.solid == 6;
        }
        if physics {
            let phy_path = format!("{basename}.phy");
            let phy = required(
                &phy_path,
                limits.max_entry_bytes,
                true,
                &mut paths,
                &mut attempted,
                limits,
                control,
                read,
            )?;
            model::validate_portable_physics(&phy, model.header.checksum)?;
        }
        for name in &model.materials {
            checkpoint(control)?;
            let mut winner = None;
            for candidate in model::material_candidates(name, &model.material_directories) {
                let candidate = material::logical_path(&candidate, "vmt")?;
                reserve_lookup(&candidate, &mut attempted, limits)?;
                if material_paths.contains(&candidate) {
                    winner = Some((candidate, None));
                    break;
                }
                let maximum = MAX_MATERIAL_BYTES
                    .min(limits.max_entry_bytes)
                    .min(MAX_MATERIAL_TOTAL_BYTES.saturating_sub(material_bytes));
                if let Some(bytes) = read(&candidate, maximum, false)? {
                    if bytes.len() > maximum {
                        return Err(invalid(
                            "Static-prop material exceeds its aggregate byte budget",
                        ));
                    }
                    material_bytes += bytes.len();
                    insert_path(&candidate, &mut paths, limits)?;
                    winner = Some((candidate, Some(bytes)));
                    break;
                }
            }
            let (material_path, bytes) = winner.ok_or_else(|| {
                invalid(format!(
                    "Missing declared static-prop material: {name} in {path}"
                ))
            })?;
            let Some(bytes) = bytes else {
                continue;
            };
            let texture_path = material::basetexture(&material_path, &bytes)?;
            material_paths.insert(material_path);
            if texture_paths.insert(texture_path.clone()) {
                let bytes = required(
                    &texture_path,
                    limits.max_entry_bytes,
                    false,
                    &mut paths,
                    &mut attempted,
                    limits,
                    control,
                    read,
                )?;
                material::texture(&texture_path, bytes, control)?;
            }
        }
    }
    checkpoint(control)?;
    Ok(paths)
}

fn reserve_lookup(path: &str, attempted: &mut usize, limits: PakLimits) -> io::Result<()> {
    if path.len() > limits.max_name_bytes || *attempted >= MAX_LOOKUPS {
        return Err(invalid(
            "Static-prop packing exceeds its path or lookup budget",
        ));
    }
    *attempted += 1;
    Ok(())
}

fn insert_path(path: &str, paths: &mut BTreeSet<String>, limits: PakLimits) -> io::Result<()> {
    if paths.len() >= limits.max_entries.saturating_sub(1) && !paths.contains(path) {
        return Err(invalid("Static-prop packing exceeds its asset budget"));
    }
    paths.insert(path.to_owned());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn required(
    path: &str,
    maximum: usize,
    original: bool,
    paths: &mut BTreeSet<String>,
    attempted: &mut usize,
    limits: PakLimits,
    control: &Control,
    read: &mut impl FnMut(&str, usize, bool) -> io::Result<Option<Vec<u8>>>,
) -> io::Result<Vec<u8>> {
    checkpoint(control)?;
    reserve_lookup(path, attempted, limits)?;
    insert_path(path, paths, limits)?;
    let bytes = read(path, maximum, original)?
        .ok_or_else(|| invalid(format!("Missing static-prop source: {path}")))?;
    if bytes.len() > maximum {
        return Err(invalid(format!(
            "Static-prop source exceeds its byte budget: {path}"
        )));
    }
    Ok(bytes)
}
