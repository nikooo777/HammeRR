use std::{collections::BTreeSet, io};

use glam::{DQuat, DVec3};
use source_formats::{
    gamefs::GameFs,
    mdl::{MAX_MODEL_BYTES, ModelHeader},
    model::Model,
    vmt::{Material, material_path},
};

use crate::{Control, map::MAX_COMPILE_COORD, static_props};

const MAX_READ_BYTES: usize = 512 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_MATERIAL_LOOKUPS: usize = 65_536;

#[derive(Clone, Copy)]
pub(super) struct ModelBounds {
    header: ModelHeader,
    mesh: [[f32; 3]; 2],
}

impl ModelBounds {
    pub(super) fn check_transform(&self, origin: DVec3, rotation: DQuat) -> Result<(), String> {
        static_props::bounds(self.header, origin, rotation)?;
        for corner in 0..8 {
            let local = DVec3::from_array(std::array::from_fn(|axis| {
                f64::from(self.mesh[usize::from(corner & (1 << axis) != 0)][axis])
            }));
            let point = origin + rotation * local;
            if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
                return Err(format!(
                    "Transformed detail model mesh bounds must fit ±{MAX_COMPILE_COORD} units"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct ModelValidator {
    bytes: usize,
    lookups: usize,
    materials: BTreeSet<String>,
}

impl ModelValidator {
    fn read(
        &mut self,
        fs: &GameFs,
        path: &str,
        maximum: usize,
        control: &Control,
    ) -> io::Result<Option<Vec<u8>>> {
        control.checkpoint().map_err(io::Error::other)?;
        let maximum = maximum.min(MAX_READ_BYTES.saturating_sub(self.bytes));
        let data = fs.read_limited(path, maximum)?;
        if let Some(data) = &data {
            self.bytes += data.len();
        }
        Ok(data)
    }

    pub(super) fn bounds(
        &mut self,
        fs: &GameFs,
        path: &str,
        control: &Control,
    ) -> Result<ModelBounds, String> {
        let data = self
            .read(fs, path, MAX_MODEL_BYTES, control)
            .map_err(|error| format!("Detail model {path}: {error}"))?
            .ok_or_else(|| format!("Detail model {path} was not found"))?;
        let header = ModelHeader::parse(&data).map_err(|error| format!("{path}: {error}"))?;
        if !header.is_static_prop() {
            return Err(format!(
                "Detail model {path} was not built with $staticprop"
            ));
        }
        let stem = path
            .strip_suffix(".mdl")
            .ok_or("Detail model must end in .mdl")?;
        let mut companion = |extension| {
            let companion = format!("{stem}.{extension}");
            self.read(fs, &companion, MAX_MODEL_BYTES, control)
                .map_err(|error| format!("{companion}: {error}"))?
                .ok_or_else(|| format!("Detail model companion {companion} was not found"))
        };
        let vvd = companion("vvd")?;
        let vtx = companion("dx90.vtx")?;
        let model = Model::parse(&data, &vvd, &vtx)
            .map_err(|error| format!("Detail model {path}: {error}"))?;
        control.checkpoint()?;
        if model.materials.is_empty() {
            return Err(format!("Detail model {path} has no material definitions"));
        }
        for name in &model.materials {
            let name = name.strip_suffix(".vmt").unwrap_or(name);
            let mut found = false;
            for candidate in model
                .material_directories
                .iter()
                .map(|directory| {
                    if directory.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{directory}/{name}")
                    }
                })
                .chain(std::iter::once(name.to_owned()))
            {
                control.checkpoint()?;
                if self.materials.contains(&candidate) {
                    found = true;
                    break;
                }
                let material =
                    Material::load_with_reader(&material_path(&candidate), &mut |path| {
                        self.lookups += 1;
                        if self.lookups > MAX_MATERIAL_LOOKUPS {
                            return Err(io::Error::other(
                                "Detail model materials exceed 65,536 lookup attempts",
                            ));
                        }
                        self.read(fs, path, MAX_MATERIAL_BYTES, control)
                    })
                    .map_err(|error| {
                        format!("Detail model {path}, material {candidate}: {error}")
                    })?;
                let Some(material) = material else {
                    continue;
                };
                if material.shader != "unlitgeneric" {
                    return Err(format!(
                        "Detail model {path}, material {candidate} uses {}; only UnlitGeneric model materials are supported because the engine substitutes an error model for vertex-lit detail props",
                        material.shader
                    ));
                }
                self.materials.insert(candidate);
                found = true;
                break;
            }
            if !found {
                return Err(format!("Detail model {path} material {name} was not found"));
            }
        }
        Ok(ModelBounds {
            header,
            mesh: model.bounds,
        })
    }
}
