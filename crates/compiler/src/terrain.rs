use glam::DVec3;
use map_core::brush::BrushGeometry;
use map_core::displacement::DisplacementMesh;
use source_formats::bsp::{
    self, DisplacementInfo, DisplacementTriangle, DisplacementVertex, contents, surf,
};
use source_formats::vmf::Solid;

use super::{Map, support};
use crate::emit::stored_vertex;
use crate::faces::Face;
use crate::materials::MaterialInfo;
use crate::tree::Tree;
use crate::winding::Winding;
use crate::{Control, Diagnostic, Report};

const MAX_LEAF_WORK: usize = 16_000_000;
use map_core::displacement::native::GEOMETRY_EPSILON;
pub(crate) use map_core::displacement::native::checked_mesh;
#[path = "terrain/lod.rs"]
mod lod;

pub(crate) struct NeighborStats {
    pub pairs: usize,
    pub corner_pairs: usize,
    pub mixed_pairs: usize,
    pub half_pairs: usize,
    pub disabled_vertices: usize,
    pub snapped_vertices: usize,
}
#[path = "terrain/neighbors.rs"]
mod neighbors;

pub(super) struct MaterialPolicy {
    sources: crate::asset_sources::BuildMaterialSources,
    checked: std::collections::BTreeSet<usize>,
}

impl MaterialPolicy {
    pub(super) fn new(sources: &crate::asset_sources::BuildMaterialSources) -> Self {
        Self {
            sources: sources.clone(),
            checked: std::collections::BTreeSet::new(),
        }
    }

    fn validate(
        &mut self,
        material: usize,
        info: &MaterialInfo,
        control: &Control,
    ) -> Result<(), String> {
        if !crate::brush_shadow_casters::texture_alpha(info) {
            return Err(format!(
                "Terrain material {} requires visible ordinary SOLID contents or supported static LightmappedGeneric texture alpha with Brush texture shadows enabled",
                info.name,
            ));
        }
        if !self.checked.contains(&material) {
            crate::brush_shadow_casters::validate_material(
                &info.name.to_ascii_lowercase(),
                &self.sources,
                control,
                true,
            )?;
            self.checked.insert(material);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Terrain {
    pub solid: u32,
    pub side: u32,
    pub plane: usize,
    pub material: usize,
    pub texinfo: usize,
    pub smoothing_groups: u32,
    pub smoothing_angle: Option<f64>,
    pub base: [DVec3; 4],
    pub mesh: DisplacementMesh,
    pub info: DisplacementInfo,
    pub vertices: Vec<DisplacementVertex>,
    pub triangles: Vec<DisplacementTriangle>,
    pub indices: Vec<[usize; 3]>,
}

impl Terrain {
    pub(crate) fn native_positions(&self) -> Result<Vec<[f32; 3]>, String> {
        map_core::displacement::native::positions(self.base, self.info, self.vertices.clone())
    }
}

impl Map<'_> {
    pub(crate) fn connect_terrain(
        &mut self,
        report: &mut Report,
        control: &Control,
    ) -> Result<NeighborStats, String> {
        neighbors::connect(&mut self.terrains, report, control)
    }
    pub(super) fn add_terrain(
        &mut self,
        solid: &Solid,
        geometry: &BrushGeometry,
        entity: usize,
        extra_contents: i32,
        report: &mut Report,
        control: &Control,
    ) -> Result<(), String> {
        let result = self.prepare_terrain(solid, geometry, entity, extra_contents, control);
        match result {
            Ok(terrain) => self.terrains.extend(terrain),
            Err(error) => {
                control.checkpoint()?;
                report.diagnostics.push(Diagnostic::error(
                    error,
                    Some(format!("solid:{}", solid.id)),
                    Some(geometry.center()),
                ));
            }
        }
        Ok(())
    }

    fn prepare_terrain(
        &mut self,
        solid: &Solid,
        geometry: &BrushGeometry,
        entity: usize,
        extra_contents: i32,
        control: &Control,
    ) -> Result<Vec<Terrain>, String> {
        if entity != 0 || extra_contents & !contents::DETAIL != 0 {
            return Err("Only world and func_detail terrain surfaces are compiled; detach this terrain from its runtime brush entity".into());
        }
        let mut terrains = Vec::new();
        for face in &geometry.faces {
            control.checkpoint()?;
            let side = &solid.sides[face.side];
            let Some(disp) = &side.displacement else {
                continue;
            };
            if self.terrains.len() + terrains.len() >= bsp::MAX_DISPLACEMENTS {
                return Err("Native displacement patch limit exceeded".into());
            }
            let (base, mesh) = checked_mesh(face, disp)?;
            let material = self.materials.load(&side.material)?;
            let material_info = self.materials.get(material);
            let material_validation = map_core::displacement::native::validate_material(
                material_info.draws,
                material_info.contents,
                material_info.surface,
            );
            if let Err(error) = material_validation {
                let Some(policy) = &mut self.terrain_materials else {
                    return Err(error);
                };
                policy
                    .validate(material, material_info, control)
                    .map_err(|error| format!("side:{}: {error}", side.id))?;
            }
            let terrain_contents = material_info.contents | extra_contents;
            let plane = support::find_authored(&mut self.planes, geometry, face, control)?
                .ok_or("Cannot preserve displacement base plane support")?;
            let source_texinfo = self.texinfo(side, material, DVec3::ZERO)?;
            let mut info = self.texinfos.infos[source_texinfo];
            info.flags = (info.flags | surf::NOLIGHT) & !surf::BUMPLIGHT;
            let texinfo = self.texinfos.add(info);
            let first = mesh.base_positions[0];
            let smoothing_angle = map_core::displacement::smoothing::angle(disp)?;
            let info = DisplacementInfo {
                start: stored_vertex(first).as_vec3().to_array(),
                power: disp.power as i32,
                min_tessellation: i32::MIN | disp.flags.unwrap_or(0) as i32,
                smoothing_angle: smoothing_angle.unwrap_or(0.0) as f32,
                contents: terrain_contents,
                ..Default::default()
            };
            let vertices = map_core::displacement::native::vertices(&mesh);
            let indices = info.triangle_indices().map_err(|error| error.to_string())?;
            let triangles = map_core::displacement::triangles::native_tags(disp, &mesh)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|tags| DisplacementTriangle { tags })
                .collect();
            terrains.push(Terrain {
                solid: solid.id,
                side: side.id,
                plane,
                material,
                texinfo,
                smoothing_groups: side.smoothing_groups.unwrap_or(0),
                smoothing_angle,
                base,
                mesh,
                info,
                vertices,
                triangles,
                indices,
            });
        }
        Ok(terrains)
    }

    pub(crate) fn terrain_faces(
        &self,
        tree: &Tree,
        report: &mut Report,
        control: &Control,
    ) -> Result<Vec<Face>, String> {
        let mut faces = Vec::with_capacity(self.terrains.len());
        let mut work = 0;
        for (terrain_index, terrain) in self.terrains.iter().enumerate() {
            let (mins, maxs) = terrain
                .mesh
                .positions
                .iter()
                .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &point| {
                    (lo.min(point), hi.max(point))
                });
            let mut leaves = Vec::new();
            let mut pending = vec![0];
            while let Some(index) = pending.pop() {
                control.checkpoint()?;
                work += 1;
                if work > MAX_LEAF_WORK {
                    return Err("Displacement leaf membership exceeded its work budget".into());
                }
                let node = &tree.nodes[index];
                if let Some((plane, children)) = node.split {
                    let plane = self.planes.get(plane);
                    let center = (mins + maxs) * 0.5;
                    let extent = plane.normal.abs().dot((maxs - mins) * 0.5);
                    let distance = plane.distance(center);
                    if distance + extent >= -GEOMETRY_EPSILON {
                        pending.push(children[0]);
                    }
                    if distance - extent <= GEOMETRY_EPSILON {
                        pending.push(children[1]);
                    }
                } else if !node.solid() {
                    leaves.push(index);
                }
            }
            if leaves.is_empty() {
                report.diagnostics.push(Diagnostic::error(
                    "Displacement does not touch open world space",
                    Some(format!("solid:{}", terrain.solid)),
                    Some((mins + maxs) * 0.5),
                ));
            }
            faces.push(Face {
                smoothing_source: None,
                source_sides: vec![terrain.side].into(),
                plane: terrain.plane,
                texinfo: terrain.texinfo,
                smoothing_groups: terrain.smoothing_groups,
                winding: Winding(terrain.base.to_vec()),
                node: None,
                leaves,
                water_region: None,
                displacement: Some(terrain_index),
            });
        }
        if !self.terrains.is_empty() {
            report.diagnostics.push(Diagnostic::warning("World and func_detail terrain surfaces do not seal the map. Native full/whole-to-half links include compatible mixed powers, LOD masks and snapping; corner-only contacts have reciprocal lists. Other fractional spans and removed triangles remain unfinished. CS:S engine physics adds terrain contact bodies", None, None));
        }
        Ok(faces)
    }
}
