use std::collections::{BTreeMap, BTreeSet};

use source_formats::{gamefs::GameFs, keyvalues};

use super::{MAX_SURFACE_PROPERTIES, SurfaceData, valid_surface_property};
use crate::{Control, asset_sources::BuildMaterialSources};

const MANIFEST: &str = "scripts/surfaceproperties_manifest.txt";
const MAX_FILES: usize = 64;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_NODES: usize = 65_536;
const MAX_TOKEN_BYTES: usize = 511;
pub(super) const MAX_DEFINITIONS: usize = 4096;
pub(super) const MAX_MASS: f64 = 50_000.0;
const METERS_PER_INCH: f64 = 0.0254;

#[derive(Clone, Copy, Default)]
pub(super) struct Properties {
    pub density: f32,
    pub thickness: f32,
}

impl Properties {
    pub(super) fn validate(self, name: &str) -> Result<(), String> {
        if !self.density.is_finite() || self.density <= 0.0 {
            return Err(format!(
                "Mass surface property {name:?} needs finite positive density"
            ));
        }
        if !self.thickness.is_finite() || self.thickness < 0.0 {
            return Err(format!(
                "Mass surface property {name:?} needs finite nonnegative thickness"
            ));
        }
        Ok(())
    }

    pub(super) fn mass(self, volume: f32, area: f32) -> Result<f32, String> {
        self.validate("selected material")?;
        if !volume.is_finite() || volume <= 0.0 || !area.is_finite() || area <= 0.0 {
            return Err(
                "Brush-model mass needs finite positive native volume and emitted area".into(),
            );
        }
        let volume = if self.thickness == 0.0 {
            f64::from(volume)
        } else {
            f64::from(area) * f64::from(self.thickness)
        };
        let mass = volume * f64::from(self.density) * METERS_PER_INCH.powi(3);
        let mass = mass.min(MAX_MASS) as f32;
        if !mass.is_finite() || mass <= 0.0 {
            return Err(
                "Brush-model mass cannot be represented as a finite positive native float".into(),
            );
        }
        Ok(mass)
    }
}

pub(super) fn load(
    fs: &GameFs,
    sources: &BuildMaterialSources,
    control: &Control,
) -> Result<Vec<SurfaceData>, String> {
    control.checkpoint()?;
    let manifest = sources.read_asset(fs, MANIFEST, MAX_TEXT_BYTES).map_err(|error| error.to_string())?
        .ok_or("Brush-model mass requires scripts/surfaceproperties_manifest.txt on the GAME search path")?;
    let text =
        std::str::from_utf8(manifest.bytes()).map_err(|_| "Surface manifest must be UTF-8")?;
    let nodes = keyvalues::parse_bounded(text, 2, MAX_NODES).map_err(|error| error.to_string())?;
    let [root] = nodes.as_slice() else {
        return Err("Surface manifest needs one surfaceproperties_manifest block".into());
    };
    if !root.key.eq_ignore_ascii_case("surfaceproperties_manifest") {
        return Err("Surface manifest needs one surfaceproperties_manifest block".into());
    }
    let entries = root.children().ok_or("Surface manifest requires a block")?;
    if entries.is_empty() || entries.len() > MAX_FILES {
        return Err("Surface manifest requires 1–64 definition files".into());
    }
    let mut seen = BTreeSet::new();
    let mut bytes = manifest.bytes().len();
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        control.checkpoint()?;
        let name = entry
            .as_str()
            .filter(|_| entry.key.eq_ignore_ascii_case("file"))
            .ok_or("Surface manifest entries must be file paths")?;
        if name.len() > MAX_TOKEN_BYTES
            || !seen.insert(name.replace('\\', "/").to_ascii_lowercase())
        {
            return Err("Surface manifest needs unique bounded file paths".into());
        }
        let data = sources
            .read_asset(fs, name, MAX_TEXT_BYTES.saturating_sub(bytes))
            .map_err(|error| format!("Cannot read surface definitions {name:?}: {error}"))?
            .ok_or_else(|| format!("Surface manifest references missing definitions {name:?}"))?;
        bytes += data.bytes().len();
        let text = std::str::from_utf8(data.bytes())
            .map_err(|_| format!("Surface definitions {name:?} must be UTF-8"))?
            .to_owned();
        files.push(SurfaceData {
            name: name.to_owned(),
            text,
        });
    }
    resolve(&files)?;
    Ok(files)
}

pub(super) fn resolve(files: &[SurfaceData]) -> Result<BTreeMap<String, Properties>, String> {
    if files.len() > MAX_FILES {
        return Err("Surface definitions exceed their 64-file budget".into());
    }
    let mut properties = BTreeMap::<String, Properties>::new();
    let mut seen = BTreeSet::new();
    let mut bytes = 0;
    let mut definitions = 0;
    for file in files {
        bytes += file.text.len();
        if bytes > MAX_TEXT_BYTES
            || file.name.is_empty()
            || file.name.len() > MAX_TOKEN_BYTES
            || file.name.contains('\0')
            || !seen.insert(file.name.replace('\\', "/").to_ascii_lowercase())
        {
            return Err(
                "Surface definitions need unique bounded file names and at most 8 MiB of text"
                    .into(),
            );
        }
        let nodes = keyvalues::parse_bounded(&file.text, 2, MAX_NODES)
            .map_err(|error| format!("Surface definitions {:?}: {error}", file.name))?;
        if nodes.is_empty() {
            return Err(format!("Surface definitions {:?} are empty", file.name));
        }
        for node in nodes {
            definitions += 1;
            if definitions > MAX_DEFINITIONS || !valid_surface_property(&node.key) {
                return Err("Surface definitions exceed their 4,096-property budget or contain an unsupported property name".into());
            }
            let name = node.key.to_ascii_lowercase();
            let mut values = properties
                .get(&name)
                .or_else(|| properties.get("default"))
                .copied()
                .unwrap_or_default();
            let fields = node
                .children()
                .ok_or_else(|| format!("Surface property {name:?} requires a block"))?;
            for field in fields {
                let value = field
                    .as_str()
                    .ok_or_else(|| format!("Surface property {name:?} fields must be strings"))?;
                if value.len() > MAX_TOKEN_BYTES || field.key.len() > MAX_TOKEN_BYTES {
                    return Err("Surface definition tokens exceed their 511-byte budget".into());
                }
                match field.key.to_ascii_lowercase().as_str() {
                    "base" => {
                        values = *properties.get(&value.to_ascii_lowercase()).ok_or_else(|| {
                            format!("Surface property {name:?} references undefined base {value:?}")
                        })?;
                    }
                    "density" | "thickness" => {
                        let number = value.trim().parse::<f32>().map_err(|_| {
                            format!("Invalid surface property {name:?} {}", field.key)
                        })?;
                        if !number.is_finite() || number < 0.0 {
                            return Err(format!(
                                "Surface property {name:?} {} must be finite and nonnegative",
                                field.key
                            ));
                        }
                        if field.key.eq_ignore_ascii_case("density") {
                            values.density = number;
                        } else {
                            values.thickness = number;
                        }
                    }
                    _ => {}
                }
            }
            properties.insert(name, values);
        }
    }
    Ok(properties)
}

pub(super) fn dominant(
    areas: &[(String, f32)],
    hidden: Option<&str>,
) -> Result<(String, f32), String> {
    let mut groups = vec![(None, 1.0_f32)];
    if areas.is_empty() {
        groups.push((
            Some(
                hidden
                    .ok_or("Hidden brush-model mass needs a source surface property")?
                    .to_owned(),
            ),
            2.0,
        ));
    }
    for (name, area) in areas {
        if !valid_surface_property(name) || !area.is_finite() || *area <= 0.0 {
            return Err(
                "Brush-model mass requires safe surface properties and finite positive face areas"
                    .into(),
            );
        }
        if let Some((_, previous)) = groups
            .iter_mut()
            .find(|(property, _)| property.as_ref() == Some(name))
        {
            *previous += area;
        } else {
            groups.push((Some(name.clone()), *area));
        }
        if groups.len() > MAX_SURFACE_PROPERTIES + 1 {
            return Err("Brush-model mass exceeds its 127-property budget".into());
        }
    }
    let mut selected = &groups[0];
    let mut total = 0.0_f32;
    for group in &groups {
        total += group.1;
        if group.1 > selected.1 {
            selected = group;
        }
    }
    if !total.is_finite() || total <= 0.0 {
        return Err("Brush-model mass surface area exceeds finite native limits".into());
    }
    Ok((
        selected.0.clone().unwrap_or_else(|| "default".into()),
        total,
    ))
}
