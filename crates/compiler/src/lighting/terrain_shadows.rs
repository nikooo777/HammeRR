use std::sync::Arc;

use glam::DVec3;
use map_core::displacement::neighbors::{self, Span};
use source_formats::bsp;

use super::bake::Budget;
use super::brush_alpha::{Face, Prepared};
use super::shadows::{ShadowIndex, ShadowObject, TRIANGLE_EPSILON, Triangle, Visit};
use super::transport::Coverage;
use crate::map::Map;

const OWNERSHIP_WORK: usize = 48;
const TEXTURE_WORK: usize = 96;
const EDGE_SCRATCH_VERTICES: usize = 17;

struct Patch {
    first_vertex: usize,
    first_triangle: usize,
    size: usize,
    corners: [DVec3; 4],
    alpha: Option<Face>,
}

struct Topology {
    positions: Vec<DVec3>,
    indices: Vec<[u32; 3]>,
    groups: Vec<u32>,
    offsets: Vec<u32>,
    incidence: Vec<u32>,
    patches: Vec<Patch>,
}

#[derive(Clone)]
pub(super) struct TerrainTriangle {
    triangle: Triangle,
    topology: Arc<Topology>,
    slot: u32,
    patch: u16,
}

impl TerrainTriangle {
    pub(super) fn source(&self) -> (usize, usize) {
        let patch = usize::from(self.patch);
        (
            patch,
            self.slot as usize - self.topology.patches[patch].first_triangle,
        )
    }

    pub(super) fn point(&self, weights: [f64; 3]) -> DVec3 {
        self.triangle.point(weights)
    }

    pub(super) fn texture_point(&self, weights: [f64; 3]) -> DVec3 {
        let patch = &self.topology.patches[usize::from(self.patch)];
        let [a, b, c, d] = patch.corners;
        self.topology.indices[self.slot as usize]
            .into_iter()
            .zip(weights)
            .map(|(index, weight)| {
                let vertex = index as usize - patch.first_vertex;
                let s = (vertex % patch.size) as f64 / (patch.size - 1) as f64;
                let t = (vertex / patch.size) as f64 / (patch.size - 1) as f64;
                a.lerp(b, t).lerp(d.lerp(c, t), s) * weight
            })
            .sum()
    }

    pub(super) fn point_contact(&self, point: DVec3) -> Result<bool, String> {
        self.triangle.point_contact(point)
    }

    pub(super) fn intersection(
        &self,
        start: DVec3,
        end: DVec3,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        self.intersection_with_endpoints(start, end, TRIANGLE_EPSILON)
    }

    pub(super) fn source_intersection(
        &self,
        start: DVec3,
        end: DVec3,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        self.intersection_with_endpoints(start, end, 0.0)
    }

    fn intersection_with_endpoints(
        &self,
        start: DVec3,
        end: DVec3,
        endpoint: f64,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        let Some(hit) = self
            .triangle
            .topological_intersection(start, end, endpoint)?
        else {
            return Ok(None);
        };
        let vertices = self.topology.indices[self.slot as usize];
        let groups = vertices.map(|vertex| self.topology.groups[vertex as usize]);
        let mut feature = [0; 2];
        let mut count = 0;
        for (group, weight) in groups.into_iter().zip(hit.1) {
            if weight != 0.0 {
                if count == feature.len() {
                    return Ok(Some(hit));
                }
                feature[count] = group;
                count += 1;
            }
        }
        if count == 0 {
            return Err("Terrain shadow crossing has no support vertex".into());
        }
        let first = feature[0] as usize;
        let range =
            self.topology.offsets[first] as usize..self.topology.offsets[first + 1] as usize;
        // Exact edge-function zeros identify shared topology without merging nearby crossings.
        for &candidate in &self.topology.incidence[range] {
            if candidate >= self.slot {
                break;
            }
            let indices = self.topology.indices[candidate as usize];
            let groups = indices.map(|vertex| self.topology.groups[vertex as usize]);
            if !feature[..count].iter().all(|group| groups.contains(group)) {
                continue;
            }
            let triangle =
                Triangle::new(indices.map(|vertex| self.topology.positions[vertex as usize]))?;
            let Some((_, weights)) = triangle.topological_intersection(start, end, endpoint)?
            else {
                continue;
            };
            let same_feature = groups
                .into_iter()
                .zip(weights)
                .all(|(group, weight)| (weight != 0.0) == feature[..count].contains(&group));
            if same_feature {
                return Ok(None);
            }
        }
        Ok(Some(hit))
    }

    fn sampled_coverage(&self, weights: [f64; 3]) -> Result<u8, String> {
        match &self.topology.patches[usize::from(self.patch)].alpha {
            Some(alpha) => alpha.coverage(self.texture_point(weights)),
            None => Ok(u8::MAX),
        }
    }
}

impl ShadowObject for TerrainTriangle {
    const MAX_OBJECTS: usize = bsp::MAX_DISPLACEMENT_TRIANGLES;

    fn bounds(&self) -> (DVec3, DVec3) {
        self.triangle.bounds()
    }

    fn work(&self) -> usize {
        let neighbors = self.topology.indices[self.slot as usize]
            .into_iter()
            .map(|vertex| self.topology.groups[vertex as usize] as usize)
            .map(|group| self.topology.offsets[group + 1] - self.topology.offsets[group])
            .max()
            .unwrap_or(0) as usize;
        self.triangle.work()
            + OWNERSHIP_WORK * (neighbors + 1)
            + if self.topology.patches[usize::from(self.patch)]
                .alpha
                .is_some()
            {
                TEXTURE_WORK
            } else {
                0
            }
    }

    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        match self.intersection(start, end)? {
            Some((_, weights)) => Ok(self.sampled_coverage(weights)? != 0),
            None => Ok(false),
        }
    }

    fn fractional(&self) -> bool {
        self.topology.patches[usize::from(self.patch)]
            .alpha
            .as_ref()
            .is_some_and(Face::fractional)
    }
}

impl ShadowIndex<TerrainTriangle> {
    pub(super) fn accumulate_coverage(
        &self,
        start: DVec3,
        end: DVec3,
        coverage: &mut Coverage,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<(), String> {
        self.visit(start, end, consume, &mut |triangle| {
            if coverage.opaque() {
                return Ok(Visit::Stop);
            }
            if let Some((_, weights)) = triangle.intersection(start, end)? {
                coverage.add(triangle.sampled_coverage(weights)?);
            }
            Ok(if coverage.opaque() {
                Visit::Stop
            } else {
                Visit::Continue(1.0)
            })
        })?;
        Ok(())
    }

    pub(super) fn accumulate_sky_coverage(
        &self,
        start: DVec3,
        end: DVec3,
        sky_fraction: f64,
        coverage: &mut Coverage,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<(), String> {
        if !sky_fraction.is_finite() || !(0.0..=1.0).contains(&sky_fraction) {
            return Err("Terrain sky endpoint must be a finite segment fraction".into());
        }
        self.visit(start, end, consume, &mut |triangle| {
            if coverage.opaque() {
                return Ok(Visit::Stop);
            }
            if let Some((fraction, weights)) = triangle.intersection(start, end)?
                && fraction <= sky_fraction + TRIANGLE_EPSILON
            {
                coverage.add(triangle.sampled_coverage(weights)?);
            }
            Ok(if coverage.opaque() {
                Visit::Stop
            } else {
                Visit::Continue(sky_fraction)
            })
        })?;
        Ok(())
    }
}

fn counts(map: &Map<'_>) -> Result<(usize, usize), String> {
    if map.terrains.len() > bsp::MAX_DISPLACEMENTS {
        return Err("Terrain shadows exceed the native displacement patch limit".into());
    }
    let mut vertices = 0usize;
    let mut triangles = 0usize;
    for terrain in &map.terrains {
        let count = terrain
            .info
            .vertex_count()
            .map_err(|error| error.to_string())?;
        if terrain.vertices.len() != count {
            return Err("Terrain shadow vertices do not match the native grid".into());
        }
        vertices = vertices
            .checked_add(count)
            .ok_or("Terrain shadow vertex count overflow")?;
        triangles = triangles
            .checked_add(terrain.indices.len())
            .ok_or("Terrain shadow triangle count overflow")?;
    }
    if vertices > bsp::MAX_DISPLACEMENT_VERTICES || triangles > TerrainTriangle::MAX_OBJECTS {
        return Err("Terrain shadows exceed the native displacement geometry limits".into());
    }
    Ok((vertices, triangles))
}

pub(super) fn allocation_bytes(map: &Map<'_>) -> Result<usize, String> {
    let (vertices, triangles) = counts(map)?;
    let topology = vertices
        .checked_mul(size_of::<DVec3>() + 2 * size_of::<u32>())
        .and_then(|bytes| bytes.checked_add(triangles * 6 * size_of::<u32>()))
        .and_then(|bytes| bytes.checked_add(map.terrains.len() * size_of::<Patch>()))
        .and_then(|bytes| {
            bytes.checked_add(size_of::<Topology>() + 2 * size_of::<usize>() + size_of::<u32>())
        })
        .ok_or("Terrain shadow topology allocation overflow")?;
    ShadowIndex::<TerrainTriangle>::allocation_bytes(triangles)?
        .checked_add(topology)
        .ok_or_else(|| "Terrain shadow allocation overflow".into())
}

fn root(groups: &mut [u32], mut vertex: usize, budget: &mut Budget<'_>) -> Result<usize, String> {
    while groups[vertex] as usize != vertex {
        budget.consume(1)?;
        groups[vertex] = groups[groups[vertex] as usize];
        vertex = groups[vertex] as usize;
    }
    Ok(vertex)
}

fn join(
    topology: &mut Topology,
    first: usize,
    second: usize,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    budget.consume(1)?;
    if topology.positions[first] != topology.positions[second] {
        return Ok(());
    }
    let a = root(&mut topology.groups, first, budget)?;
    let b = root(&mut topology.groups, second, budget)?;
    topology.groups[a.max(b)] = a.min(b) as u32;
    Ok(())
}

fn certify_edges(
    map: &Map<'_>,
    topology: &mut Topology,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    for (index, terrain) in map.terrains.iter().enumerate() {
        for (edge, neighbors) in terrain.info.edges.iter().enumerate() {
            for neighbor in neighbors {
                budget.consume(1)?;
                let other = usize::from(neighbor.index);
                if neighbor.index == bsp::INVALID_DISPLACEMENT_NEIGHBOR || other <= index {
                    continue;
                }
                let other_terrain = map
                    .terrains
                    .get(other)
                    .ok_or("Terrain shadow neighbor references a missing patch")?;
                if neighbor.orientation >= 4 {
                    return Err("Terrain shadow neighbor has invalid orientation".into());
                }
                let other_edge = (usize::from(neighbor.orientation) + edge + 2) % 4;
                let reciprocal = other_terrain.info.edges[other_edge].iter().any(|back| {
                    usize::from(back.index) == index
                        && usize::from(back.orientation) == (edge + 6 - other_edge) % 4
                        && back.span == neighbor.neighbor_span
                        && back.neighbor_span == neighbor.span
                });
                if !reciprocal {
                    return Err("Terrain shadow neighbor lacks a reciprocal native span".into());
                }
                let first_span = Span::from_native(neighbor.span, edge)?;
                let second_span = Span::from_native(neighbor.neighbor_span, other_edge)?;
                for (a, b) in neighbors::common_span_indices(
                    (terrain.info.power as u32, edge, first_span),
                    (other_terrain.info.power as u32, other_edge, second_span),
                )? {
                    let first = topology.patches[index].first_vertex + a;
                    let second = topology.patches[other].first_vertex + b;
                    join(topology, first, second, budget)?;
                }
            }
        }
    }
    Ok(())
}

fn certify_corners(
    map: &Map<'_>,
    topology: &mut Topology,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    for (index, terrain) in map.terrains.iter().enumerate() {
        for (corner, neighbors) in terrain.info.corners.iter().enumerate() {
            let list = neighbors
                .neighbors
                .get(..usize::from(neighbors.count))
                .ok_or("Terrain shadow corner exceeds its native neighbor limit")?;
            for &neighbor in list {
                budget.consume(1)?;
                let other = usize::from(neighbor);
                if other <= index {
                    continue;
                }
                let other_terrain = map
                    .terrains
                    .get(other)
                    .ok_or("Terrain shadow corner references a missing patch")?;
                let patch = &topology.patches[index];
                let first_vertex = patch.first_vertex
                    + [
                        0,
                        (patch.size - 1) * patch.size,
                        patch.size * patch.size - 1,
                        patch.size - 1,
                    ][corner];
                let base = patch.corners[corner];
                for (other_corner, back) in other_terrain.info.corners.iter().enumerate() {
                    budget.consume(1)?;
                    let back_list = back
                        .neighbors
                        .get(..usize::from(back.count))
                        .ok_or("Terrain shadow corner exceeds its native neighbor limit")?;
                    if !back_list.contains(&(index as u16)) {
                        continue;
                    }
                    let other_patch = &topology.patches[other];
                    if base != other_patch.corners[other_corner] {
                        continue;
                    }
                    let size = other_patch.size;
                    let second_vertex = other_patch.first_vertex
                        + [0, (size - 1) * size, size * size - 1, size - 1][other_corner];
                    join(topology, first_vertex, second_vertex, budget)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn build(
    map: &Map<'_>,
    prepared: Option<&Prepared>,
    budget: &mut Budget<'_>,
) -> Result<ShadowIndex<TerrainTriangle>, String> {
    let (vertex_count, triangle_count) = counts(map)?;
    let _temporary = budget.reserve_bytes(
        vertex_count * size_of::<u32>() + EDGE_SCRATCH_VERTICES * 8 * size_of::<usize>(),
    )?;
    budget.consume(vertex_count + triangle_count + map.terrains.len())?;
    let mut topology = Topology {
        positions: Vec::with_capacity(vertex_count),
        indices: Vec::with_capacity(triangle_count),
        groups: (0..vertex_count as u32).collect(),
        offsets: vec![0; vertex_count + 1],
        incidence: vec![0; 3 * triangle_count],
        patches: Vec::with_capacity(map.terrains.len()),
    };
    for terrain in &map.terrains {
        let _native_temporary = budget.reserve_bytes(
            terrain.vertices.len() * (size_of::<bsp::DisplacementVertex>() + size_of::<[f32; 3]>()),
        )?;
        budget.consume(terrain.vertices.len() + terrain.indices.len())?;
        let points = terrain.native_positions()?;
        let info = map
            .texinfos
            .infos
            .get(terrain.texinfo)
            .ok_or("Terrain shadow texture vectors are unavailable")?;
        let alpha =
            prepared.and_then(|prepared| prepared.face(terrain.material, info.texture_vecs));
        let first_vertex = topology.positions.len();
        let first_triangle = topology.indices.len();
        topology.patches.push(Patch {
            first_vertex,
            first_triangle,
            size: terrain
                .info
                .grid_size()
                .map_err(|error| error.to_string())?,
            corners: terrain
                .info
                .start_oriented_quad(terrain.base.map(|point| {
                    map_core::displacement::native::stored_vertex(point)
                        .as_vec3()
                        .to_array()
                }))
                .map_err(|error| error.to_string())?
                .map(|point| DVec3::from_array(point.map(f64::from))),
            alpha,
        });
        topology.positions.extend(
            points
                .into_iter()
                .map(|point| DVec3::from_array(point.map(f64::from))),
        );
        for indices in &terrain.indices {
            if indices.iter().any(|&index| index >= terrain.vertices.len()) {
                return Err("Terrain shadow triangle references a missing vertex".into());
            }
            topology
                .indices
                .push(indices.map(|index| (first_vertex + index) as u32));
        }
    }
    certify_edges(map, &mut topology, budget)?;
    certify_corners(map, &mut topology, budget)?;
    for vertex in 0..vertex_count {
        budget.consume(1)?;
        topology.groups[vertex] = root(&mut topology.groups, vertex, budget)? as u32;
    }
    for indices in &topology.indices {
        budget.consume(3)?;
        for &vertex in indices {
            topology.offsets[topology.groups[vertex as usize] as usize + 1] += 1;
        }
    }
    for index in 1..topology.offsets.len() {
        budget.consume(1)?;
        topology.offsets[index] += topology.offsets[index - 1];
    }
    let mut cursor = topology.offsets[..vertex_count].to_vec();
    for (slot, indices) in topology.indices.iter().enumerate() {
        budget.consume(3)?;
        for &vertex in indices {
            let group = topology.groups[vertex as usize] as usize;
            topology.incidence[cursor[group] as usize] = slot as u32;
            cursor[group] += 1;
        }
    }
    drop(cursor);
    let topology = Arc::new(topology);
    let mut triangles = Vec::with_capacity(triangle_count);
    for (patch, terrain) in map.terrains.iter().enumerate() {
        for triangle in 0..terrain.indices.len() {
            budget.consume(1)?;
            let slot = topology.patches[patch].first_triangle + triangle;
            triangles.push(TerrainTriangle {
                triangle: Triangle::new(
                    topology.indices[slot].map(|vertex| topology.positions[vertex as usize]),
                )?,
                topology: Arc::clone(&topology),
                slot: slot as u32,
                patch: patch as u16,
            });
        }
    }
    ShadowIndex::new(triangles, &mut |amount| budget.consume(amount))
}
