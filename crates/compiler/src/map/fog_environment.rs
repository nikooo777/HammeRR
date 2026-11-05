use std::collections::BTreeSet;

use map_core::Plane;
use source_formats::bsp::contents;

use super::Map;
use crate::asset_sources::BuildMaterialSources;
use crate::{Control, Report, portals, tree};

pub const MAX_FOG_ENVIRONMENT_NODES: usize = 8191;
pub const MAX_FOG_ENVIRONMENT_LIQUIDS: usize = 256;

const LIQUID_CONTENTS: i32 = contents::WATER | contents::SLIME;
const TREE_LIMITS: tree::Limits = tree::Limits {
    nodes: MAX_FOG_ENVIRONMENT_NODES,
    pending: 4096,
    points: 800_000,
    work: 16_000_000,
};
const PORTAL_LIMITS: portals::Limits = portals::Limits {
    portals: 32_768,
    pending: 8192,
    points: 800_000,
    work: 16_000_000,
};

#[derive(Debug)]
pub struct FogLiquidVolume {
    pub solid: u32,
    pub source_solids: Vec<u32>,
    pub contents: i32,
    pub material: String,
    pub surface_material: String,
    pub surface: f32,
}

#[derive(Debug)]
pub struct FogEnvironmentNode {
    pub split: Option<(Plane, [usize; 2])>,
    pub liquid: Option<usize>,
}

#[derive(Debug, Default)]
pub struct FogEnvironmentGeometry {
    pub liquids: Vec<FogLiquidVolume>,
    pub nodes: Vec<FogEnvironmentNode>,
    pub liquid_brushes: usize,
    pub solid_brushes: usize,
}

impl Map<'_> {
    pub fn fog_environment_geometry(
        &mut self,
        sources: &BuildMaterialSources,
        report: &mut Report,
        control: &Control,
    ) -> Result<FogEnvironmentGeometry, String> {
        let mut geometry = FogEnvironmentGeometry::default();
        let mut candidates = Vec::new();
        for brush in &self.brushes {
            control.checkpoint()?;
            if brush.contents & LIQUID_CONTENTS == 0 {
                geometry.solid_brushes += usize::from(
                    brush.entity == 0
                        && brush.structural()
                        && brush.contents & contents::SOLID != 0,
                );
                continue;
            }
            if brush.entity != 0 || !brush.structural() {
                return Err(format!(
                    "Fog environment liquid brush {} requires structural world geometry",
                    brush.id
                ));
            }
            if brush.contents & LIQUID_CONTENTS == LIQUID_CONTENTS {
                return Err(format!(
                    "Fog environment brush {} mixes WATER and SLIME contents",
                    brush.id
                ));
            }
            let top = crate::water::horizontal_surface(self, brush).ok_or_else(|| format!(
                "Fog environment liquid brush {} needs a supported visible horizontal top surface", brush.id
            ))?;
            candidates.push((brush.id, top.texinfo, brush.contents & LIQUID_CONTENTS));
            if candidates.len() > MAX_FOG_ENVIRONMENT_LIQUIDS {
                return Err("Fog environment exceeds 256 structural liquid brushes".into());
            }
        }
        geometry.liquid_brushes = candidates.len();
        if candidates.is_empty() {
            return Ok(geometry);
        }
        crate::water::validate_static_slime_materials(self, sources, report, control)?;
        crate::water::prepare_materials(self, report, control)?;
        if report.has_errors() {
            return Err("Fog environment liquid material preparation failed".into());
        }
        let structural = self.entities[0]
            .brushes
            .iter()
            .copied()
            .filter(|&index| self.brushes[index].structural())
            .collect::<Vec<_>>();
        let tree = tree::Tree::build_limited_with_hints(
            &self.brushes,
            &structural,
            &mut self.planes,
            &self.hints,
            control,
            TREE_LIMITS,
        )?;
        let portals = portals::build_limited(&tree, &self.planes, control, PORTAL_LIMITS)?;
        let water = crate::water::build(self, &tree, &portals, report, control)?;
        if report.has_errors() {
            return Err("Fog environment liquid region preparation failed".into());
        }
        if water.records.len() > MAX_FOG_ENVIRONMENT_LIQUIDS {
            return Err("Fog environment exceeds 256 liquid leaf records".into());
        }
        let mut owners = vec![BTreeSet::new(); water.records.len()];
        for (index, &record) in water.leaves.iter().enumerate() {
            let Ok(record) = usize::try_from(record) else {
                continue;
            };
            for &brush in &tree.nodes[index].brushes {
                control.checkpoint()?;
                let brush = &self.brushes[brush];
                if brush.contents & LIQUID_CONTENTS != 0 {
                    owners[record].insert(brush.id);
                }
            }
        }
        for (record_index, record) in water.records.iter().enumerate() {
            control.checkpoint()?;
            let texinfo = usize::try_from(record.surface_texinfo)
                .map_err(|_| "Fog environment liquid texinfo is invalid")?;
            let solid = *owners[record_index]
                .first()
                .ok_or("Fog environment liquid source brushes are missing")?;
            let contents = self
                .brushes
                .iter()
                .find(|brush| brush.id == solid)
                .ok_or("Fog environment liquid source brush is missing")?
                .contents
                & LIQUID_CONTENTS;
            let top_info = self
                .texinfos
                .infos
                .get(texinfo)
                .ok_or("Fog environment top texinfo is missing")?;
            let top_material = *self
                .texdata
                .get(
                    usize::try_from(top_info.texdata)
                        .map_err(|_| "Fog environment top material index is invalid")?,
                )
                .ok_or("Fog environment top material is missing")?;
            let top = self.materials.get(top_material);
            let bottom_material = if contents == contents::SLIME {
                top_material
            } else {
                let bottom_texinfo = *self.water_bottom_texinfos.get(&texinfo)
                    .ok_or_else(|| format!("Fog environment material {:?} needs an explicit supported $bottommaterial", top.name))?;
                let bottom_info = self
                    .texinfos
                    .infos
                    .get(bottom_texinfo)
                    .ok_or("Fog environment bottom texinfo is missing")?;
                *self
                    .texdata
                    .get(
                        usize::try_from(bottom_info.texdata)
                            .map_err(|_| "Fog environment bottom material index is invalid")?,
                    )
                    .ok_or("Fog environment bottom material is missing")?
            };
            let bottom = self.materials.get(bottom_material);
            if !bottom.found || !bottom.unsupported_flags.is_empty() {
                return Err(format!(
                    "Fog environment underwater material {:?} is missing or unsupported",
                    bottom.name
                ));
            }
            geometry.liquids.push(FogLiquidVolume {
                solid,
                source_solids: owners[record_index].iter().copied().collect(),
                contents,
                material: bottom.name.clone(),
                surface_material: top.name.clone(),
                surface: record.surface_z,
            });
        }
        geometry.nodes = tree
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let liquid = water.leaves[index];
                FogEnvironmentNode {
                    split: node.split.map(|(index, children)| {
                        let plane = self.planes.get(index);
                        (
                            Plane {
                                normal: plane.normal.as_vec3().as_dvec3(),
                                dist: f64::from(plane.dist as f32),
                            },
                            children,
                        )
                    }),
                    liquid: usize::try_from(liquid).ok(),
                }
            })
            .collect();
        Ok(geometry)
    }
}
