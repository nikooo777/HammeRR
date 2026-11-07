use std::collections::BTreeMap;

use glam::DVec3;
use map_core::ObjectId;
use map_core::displacement::{neighbors, triangles};
use source_formats::vmf::{Side, Vmf};

use super::{Admission, Limits, Stats, Surface, checked_total};
use crate::map::{PreparedTerrain, native_side_projections};

type SourceSides<'a> = BTreeMap<(u32, u32), (ObjectId, &'a Side)>;

pub(super) fn admit(
    patches: Vec<PreparedTerrain>,
    admission: &mut Admission<'_>,
    stats: &mut Stats,
) -> Result<Vec<PreparedTerrain>, String> {
    let mut admitted = Vec::new();
    for patch in patches {
        if admission.outside(patch.bounds)? {
            stats.excluded_terrain_patches += 1;
        } else if admission.contains(patch.bounds)? {
            admitted.push(patch);
        } else {
            return Err(format!(
                "solid:{} side:{}: complete authored/native terrain bounds cannot be certified inside or outside the selected sky_camera component",
                patch.source.solid, patch.source.side
            ));
        }
    }
    Ok(admitted)
}

fn source_sides<'a>(
    vmf: &'a Vmf,
    admission: &mut Admission<'_>,
) -> Result<SourceSides<'a>, String> {
    let mut sides = BTreeMap::new();
    for solid in &vmf.world.solids {
        admission.consume(1)?;
        if solid.hidden {
            continue;
        }
        admission.consume(1 + solid.sides.len())?;
        for side in solid
            .sides
            .iter()
            .filter(|side| side.displacement.is_some())
        {
            sides.insert((solid.id, side.id), (ObjectId::Solid(solid.id), side));
        }
    }
    for entity in &vmf.entities {
        admission.consume(1 + entity.solids.len())?;
        if entity.hidden || entity.classname() != Some("func_detail") {
            continue;
        }
        for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
            admission.consume(1 + solid.sides.len())?;
            for side in solid
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
            {
                sides.insert((solid.id, side.id), (ObjectId::Entity(entity.id), side));
            }
        }
    }
    Ok(sides)
}

pub(super) fn append(
    patches: Vec<PreparedTerrain>,
    vmf: &Vmf,
    include: &impl Fn(ObjectId) -> bool,
    surfaces: &mut Vec<Surface>,
    stats: &mut Stats,
    limits: Limits,
    admission: &mut Admission<'_>,
) -> Result<(), String> {
    if patches.is_empty() {
        return Ok(());
    }
    let source_sides = source_sides(vmf, admission)?;
    let grids = patches
        .iter()
        .map(|patch| {
            let size = patch.mesh.size;
            neighbors::SurfaceGrid {
                power: (size - 1).ilog2(),
                corners: [0, (size - 1) * size, size * size - 1, size - 1]
                    .map(|index| patch.mesh.base_positions[index].as_vec3().as_dvec3()),
                positions: &patch.mesh.positions,
            }
        })
        .collect::<Vec<_>>();
    let found = neighbors::find_spans(&grids, &mut |amount| admission.consume(amount))?;
    let angles = patches
        .iter()
        .map(|patch| patch.smoothing_angle)
        .collect::<Vec<_>>();
    let normals =
        neighbors::vertex_normals_with_smoothing(&grids, &found.pairs, &angles, &mut |amount| {
            admission.consume(amount)
        })?;
    drop(grids);
    for (patch, normals) in patches.into_iter().zip(normals) {
        let source = patch.source;
        let (_, side) = source_sides
            .get(&(source.solid, source.side))
            .filter(|(owner, _)| *owner == source.owner)
            .ok_or_else(|| {
                format!(
                    "side:{}: sky terrain lost its original visible world/detail surface",
                    source.side
                )
            })?;
        let info = side
            .displacement
            .as_ref()
            .ok_or("Sky terrain lost its displacement fields")?;
        admission.consume(2 * (patch.mesh.size - 1).pow(2) + patch.mesh.positions.len())?;
        triangles::native_tags(info, &patch.mesh)
            .map_err(|error| format!("solid:{} side:{}: {error}", source.solid, source.side))?;
        if normals.len() != patch.mesh.positions.len()
            || normals.iter().any(|normal| {
                let normal = DVec3::from_array(*normal);
                let packed = normal.as_vec3();
                !normal.is_finite()
                    || !normal.length_squared().is_finite()
                    || normal.length_squared() == 0.0
                    || !packed.is_finite()
                    || packed.length_squared() == 0.0
            })
        {
            return Err(format!(
                "solid:{} side:{}: sky terrain requires complete finite nonzero display normals",
                source.solid, source.side
            ));
        }
        if !include(source.owner) {
            continue;
        }
        let [texture_vectors, _] = native_side_projections(side, DVec3::ZERO)?;
        let count = 2 * (patch.mesh.size - 1).pow(2);
        let total = checked_total(
            surfaces.len(),
            count,
            limits.faces.faces,
            "terrain output surface",
        )?;
        let vertices = count
            .checked_mul(3)
            .ok_or("Sky terrain output vertex count overflow")?;
        stats.vertices = checked_total(stats.vertices, vertices, limits.vertices, "output vertex")?;
        surfaces
            .try_reserve_exact(total - surfaces.len())
            .map_err(|error| format!("Sky terrain output allocation failed: {error}"))?;
        for indices in patch.mesh.triangles() {
            admission.consume(3)?;
            let vertices = indices.map(|index| patch.mesh.positions[index]);
            let normal = (vertices[2] - vertices[0]).cross(vertices[1] - vertices[0]);
            let packed = vertices.map(|point| point.as_vec3().as_dvec3());
            let packed_normal = (packed[2] - packed[0]).cross(packed[1] - packed[0]);
            if !normal.is_finite()
                || normal.length_squared() < 1e-12
                || !packed_normal.is_finite()
                || packed_normal.length_squared() < 1e-12
                || normal.dot(packed_normal) <= 0.0
            {
                return Err(format!(
                    "solid:{} side:{}: sky terrain triangle collapses or reverses in display precision",
                    source.solid, source.side
                ));
            }
            surfaces.push(Surface {
                vertices: vertices.to_vec(),
                normal,
                material: side.material.clone(),
                texture_vectors,
                texture_points: Some(
                    indices
                        .map(|index| patch.mesh.base_positions[index])
                        .to_vec(),
                ),
                normals: Some(
                    indices
                        .map(|index| DVec3::from_array(normals[index]))
                        .to_vec(),
                ),
                flags: 0,
                source_sides: vec![source],
            });
        }
        stats.terrain_patches += 1;
        stats.terrain_triangles += count;
        stats.terrain_grid_vertices += patch.mesh.positions.len();
    }
    stats.surfaces = surfaces.len();
    Ok(())
}
