use std::{collections::BTreeSet, io};

use glam::{DQuat, DVec3};
use source_formats::{
    gamefs::{GameFs, asset_relative},
    mdl::{MAX_MODEL_BYTES, ModelHeader},
    model::Model,
    vmt::{Material, material_path, texture_path},
    vtf::Vtf,
};

use crate::{Control, asset_sources::BuildMaterialSources, map::MAX_COMPILE_COORD, static_props};

const MAX_READ_BYTES: usize = 512 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_MATERIAL_LOOKUPS: usize = 65_536;
const MAX_ATLAS_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATLAS_DIMENSION: u32 = 4096;

#[derive(Clone, Copy)]
pub(super) struct ModelBounds {
    header: ModelHeader,
    mesh: [[f32; 3]; 2],
}

impl ModelBounds {
    pub(super) fn lighting_center(self) -> DVec3 {
        (DVec3::from_array(self.header.hull_mins.map(f64::from))
            + DVec3::from_array(self.header.hull_maxs.map(f64::from)))
            * 0.5
    }

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

pub(super) struct ModelValidator<'a> {
    sources: &'a BuildMaterialSources,
    bytes: usize,
    lookups: usize,
    materials: BTreeSet<String>,
}

impl<'a> ModelValidator<'a> {
    pub(super) fn new(sources: &'a BuildMaterialSources) -> Self {
        Self {
            sources,
            bytes: 0,
            lookups: 0,
            materials: BTreeSet::new(),
        }
    }

    fn read(
        &mut self,
        fs: &GameFs,
        path: &str,
        maximum: usize,
        control: &Control,
    ) -> io::Result<Option<Vec<u8>>> {
        control.checkpoint().map_err(io::Error::other)?;
        let maximum = maximum.min(MAX_READ_BYTES.saturating_sub(self.bytes));
        let data = self.sources.read_asset(fs, path, maximum)?;
        if let Some(data) = &data {
            self.bytes += data.bytes().len();
        }
        Ok(data.map(|data| data.bytes().to_vec()))
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
            for candidate in
                source_formats::model::material_candidates(name, &model.material_directories)
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

    pub(super) fn sprite_atlas(
        &mut self,
        fs: &GameFs,
        raw: &str,
        control: &Control,
    ) -> Result<(), String> {
        if raw.trim() != raw {
            return Err(
                "Detail sprite atlas material path cannot contain surrounding whitespace".into(),
            );
        }
        let path = asset_relative(raw).map_err(|error| error.to_string())?;
        if path.is_empty() {
            return Err("Detail sprite atlas must name a mounted material".into());
        }
        let material = Material::load_with_reader(&material_path(&path), &mut |path| {
            self.lookups += 1;
            if self.lookups > MAX_MATERIAL_LOOKUPS {
                return Err(io::Error::other(
                    "Detail assets exceed 65,536 material lookup attempts",
                ));
            }
            self.read(fs, path, MAX_MATERIAL_BYTES, control)
        })
        .map_err(|error| format!("Detail sprite atlas {path}: {error}"))?
        .ok_or_else(|| format!("Detail sprite atlas material {path} was not found"))?;
        if material.shader != "unlitgeneric"
            || ["$translucent", "$nocull", "$vertexcolor", "$vertexalpha"]
                .iter()
                .any(|flag| !material.compile_flag(flag))
            || !material.blocks.is_empty()
            || material.params.iter().any(|(key, _)| {
                ![
                    "$basetexture",
                    "$translucent",
                    "$nocull",
                    "$vertexcolor",
                    "$vertexalpha",
                    "$surfaceprop",
                ]
                .contains(&key.as_str())
            })
        {
            return Err("Detail sprite atlas requires a static UnlitGeneric material with $translucent, $nocull, $vertexcolor and $vertexalpha enabled; material effects and overrides are unsupported".into());
        }
        let texture = material
            .get("$basetexture")
            .ok_or("Detail sprite atlas material needs $basetexture")?;
        if texture.is_empty() || texture.trim() != texture {
            return Err("Detail sprite atlas needs a nonempty $basetexture path without surrounding whitespace".into());
        }
        let texture = asset_relative(texture).map_err(|error| error.to_string())?;
        let texture = texture_path(&texture);
        let data = self
            .read(fs, &texture, MAX_ATLAS_BYTES, control)
            .map_err(|error| format!("Detail sprite atlas texture {texture}: {error}"))?
            .ok_or_else(|| format!("Detail sprite atlas texture {texture} was not found"))?;
        let atlas =
            Vtf::parse(data).map_err(|error| format!("Detail sprite atlas {texture}: {error}"))?;
        if atlas.width != atlas.height
            || atlas.width > MAX_ATLAS_DIMENSION
            || atlas.frames != 1
            || atlas.faces != 1
            || atlas.depth != 1
        {
            return Err("Detail sprite atlas needs one static square 2D texture no larger than 4096 pixels per side".into());
        }
        control.checkpoint()?;
        Ok(())
    }
}
