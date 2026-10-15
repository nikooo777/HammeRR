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
use crate::tree::Tree;
use crate::winding::Winding;
use crate::{Control, Diagnostic, Report};

const MAX_LEAF_WORK: usize = 16_000_000;
const GEOMETRY_EPSILON: f64 = 0.02;
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

#[derive(Debug)]
pub struct Terrain {
    pub solid: u32,
    pub side: u32,
    pub plane: usize,
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
        let data = bsp::Displacements {
            infos: vec![self.info],
            vertices: self.vertices.clone(),
            triangles: self.triangles.clone(),
        };
        data.positions(
            0,
            self.base
                .map(|point| stored_vertex(point).as_vec3().to_array()),
        )
        .map_err(|error| error.to_string())
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
            if disp.normals.is_some() != disp.distances.is_some() {
                return Err("Displacement normals and distances must be provided together".into());
            }
            if self.terrains.len() + terrains.len() >= bsp::MAX_DISPLACEMENTS {
                return Err("Native displacement patch limit exceeded".into());
            }
            if disp.flags.unwrap_or(0) & !14 != 0 {
                return Err("Bumped/unknown displacement flags are not supported yet".into());
            }
            let base: [DVec3; 4] = face
                .vertices
                .clone()
                .try_into()
                .map_err(|_| "Displacements need a convex quad base face")?;
            if !disp.start_position.is_finite()
                || base
                    .iter()
                    .all(|point| point.distance(disp.start_position) > GEOMETRY_EPSILON)
            {
                return Err("Displacement start position must match a base corner".into());
            }
            let mesh = DisplacementMesh::build(face, disp).map_err(|e| e.to_string())?;
            if mesh.positions.iter().any(|point| {
                !point.is_finite() || point.abs().max_element() > super::MAX_COMPILE_COORD
            }) || mesh.alphas.iter().any(|alpha| !(*alpha as f32).is_finite())
            {
                return Err("Displacement vertices/alphas must be finite and positions must fit Source compile bounds".into());
            }
            let material = self.materials.load(&side.material)?;
            let material_info = self.materials.get(material);
            if !material_info.draws
                || material_info.contents & !contents::DETAIL != contents::SOLID
                || material_info.surface & (surf::SKY | surf::SKY2D | surf::WARP) != 0
            {
                return Err(
                    "Displacements currently require a visible ordinary solid material".into(),
                );
            }
            let terrain_contents = material_info.contents | extra_contents;
            let plane = support::find_authored(&mut self.planes, geometry, face, control)?
                .ok_or("Cannot preserve displacement base plane support")?;
            let source_texinfo = self.texinfo(side, material, DVec3::ZERO);
            let mut info = self.texinfos.infos[source_texinfo];
            if info
                .texture_vecs
                .iter()
                .chain(&info.lightmap_vecs)
                .flatten()
                .any(|value| !value.is_finite())
            {
                return Err(
                    "Displacement texture/lightmap projections must fit finite native floats"
                        .into(),
                );
            }
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
            let vertices = mesh
                .positions
                .iter()
                .zip(&mesh.base_positions)
                .zip(&mesh.alphas)
                .map(|((&point, &base), &alpha)| {
                    let offset = point - base;
                    let distance = offset.length();
                    DisplacementVertex {
                        direction: if distance == 0.0 {
                            [0.0; 3]
                        } else {
                            (offset / distance).as_vec3().to_array()
                        },
                        distance: distance as f32,
                        alpha: alpha as f32,
                    }
                })
                .collect::<Vec<_>>();
            let indices = info.triangle_indices().map_err(|error| error.to_string())?;
            let triangles = map_core::displacement::triangles::native_tags(disp, &mesh)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|tags| DisplacementTriangle { tags })
                .collect();
            if indices.iter().any(|&[a, b, c]| {
                (mesh.positions[b] - mesh.positions[a])
                    .cross(mesh.positions[c] - mesh.positions[a])
                    .length_squared()
                    < 1e-12
            }) {
                return Err("Displacement contains collapsed triangles".into());
            }
            terrains.push(Terrain {
                solid: solid.id,
                side: side.id,
                plane,
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
                displacement: Some(terrain_index),
            });
        }
        if !self.terrains.is_empty() {
            report.diagnostics.push(Diagnostic::warning("World and func_detail terrain surfaces do not seal the map. Native full/whole-to-half links include compatible mixed powers, LOD masks and snapping; corner-only contacts have reciprocal lists. Other fractional spans and removed triangles remain unfinished. CS:S engine physics adds terrain contact bodies", None, None));
        }
        Ok(faces)
    }
}
