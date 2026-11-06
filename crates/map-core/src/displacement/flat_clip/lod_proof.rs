use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::{bsp, vmf::DispInfo};

use super::{
    CUT_EPSILON, Exact, FIELD_WORK, PlanarField, Region, Work, certify_normals, convex,
    field::BasePlane, invalid, labels, orient, paint, same, uv,
};
use crate::clip::{Keep, TerrainTexture};
use crate::{EditError, Face, Plane, displacement::DisplacementMesh};

#[derive(Clone, Copy)]
pub(super) struct State<'a> {
    pub(super) data: &'a bsp::Displacements,
    pub(super) original_positions: &'a [Vec<DVec3>],
    pub(super) positions: &'a [Vec<DVec3>],
    pub(super) ideal_positions: &'a [DVec3],
    pub(super) indices: &'a [[usize; 3]],
    pub(super) active: &'a [bool],
    pub(super) mask: &'a [u32; 10],
}

pub(super) struct Fields {
    field: PlanarField,
    samples: SampleFields,
}

pub(super) struct SampleFields {
    physical: labels::PhysicalField,
    texture: TerrainTexture,
}

impl Fields {
    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        work.consume(FIELD_WORK).map_err(invalid)?;
        Ok(self.field.matches(&other.field)? && self.samples.matches(&other.samples, work)?)
    }
}

impl SampleFields {
    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        Ok(self.texture == other.texture && self.physical.matches(&other.physical, work)?)
    }
}

fn validate_state(
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: &State<'_>,
    work: &mut Work,
) -> Result<bsp::DisplacementInfo, EditError> {
    let [native_info] = state.data.infos.as_slice() else {
        return Err(invalid(
            "Terrain LOD proof requires exactly one native patch",
        ));
    };
    let size = native_info
        .grid_size()
        .map_err(|error| invalid(error.to_string()))?;
    let count = size * size;
    work.consume(4 * count + state.indices.len())
        .map_err(invalid)?;
    if size != mesh.size
        || native_info.power != info.power as i32
        || native_info.allowed_vertices != *state.mask
        || state.data.vertices.len() != count
        || mesh.base_positions.len() != count
        || mesh.positions.len() != count
        || mesh.alphas.len() != count
        || state.active.len() != count
        || state.ideal_positions.len() != count
        || state.original_positions.len() != bsp::NativeDisplacementRounding::ALL.len() + 1
        || state
            .original_positions
            .iter()
            .any(|positions| positions.len() != count)
        || state.positions.len() != bsp::NativeDisplacementRounding::ALL.len() + 1
        || state
            .positions
            .iter()
            .any(|positions| positions.len() != count)
        || state.indices.len() > 2 * (size - 1).pow(2)
    {
        return Err(invalid(
            "Terrain LOD proof lost its matching native grid or reconstruction profiles",
        ));
    }
    let expected = if *state.mask == [u32::MAX; 10] {
        native_info.triangle_indices()
    } else {
        native_info.allowed_triangle_indices()
    }
    .map_err(|error| invalid(error.to_string()))?;
    if state.indices != expected.as_slice() {
        return Err(invalid(
            "Terrain maximal LOD triangles differ from their native allowed mask",
        ));
    }
    let mut active = vec![false; count];
    for triangle in state.indices {
        for &index in triangle {
            if index >= count || !native_info.vertex_allowed(index) {
                return Err(invalid(
                    "Terrain maximal LOD triangle references a forbidden vertex",
                ));
            }
            active[index] = true;
        }
    }
    if active.as_slice() != state.active
        || [
            0,
            size - 1,
            size * (size - 1),
            count - 1,
            size * (size / 2) + size / 2,
        ]
        .into_iter()
        .any(|index| !active[index] || !native_info.vertex_allowed(index))
    {
        return Err(invalid(
            "Terrain LOD proof requires exact active triangles, roots and corners",
        ));
    }
    Ok(*native_info)
}

fn sample_fields(
    mesh: &DisplacementMesh,
    state: &State<'_>,
    cut: Option<(Plane, Keep)>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<SampleFields, EditError> {
    let physical = labels::PhysicalField::new(
        mesh,
        state.data,
        state.indices,
        texture.has_secondary_surface,
        work,
    )?;
    let native_info = state.data.infos[0];
    let count = native_info
        .triangle_count()
        .map_err(|error| invalid(error.to_string()))?;
    work.consume(FIELD_WORK * (count + state.indices.len()))
        .map_err(invalid)?;
    let fine = triangle_set(
        &native_info
            .triangle_indices()
            .map_err(|error| invalid(error.to_string()))?,
    )?;
    let maximal = triangle_set(state.indices)?;
    if !all_discarded(mesh, &fine, &maximal, *state, cut, work)? {
        paint::certify_lod(mesh, &fine, &maximal, work)?;
    }
    Ok(SampleFields { physical, texture })
}

fn preserve_positions(
    mesh: &DisplacementMesh,
    state: &State<'_>,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<(), EditError> {
    work.consume(
        (state.original_positions.len() + state.positions.len() + 1) * mesh.positions.len(),
    )
    .map_err(invalid)?;
    let moved = mesh.positions.iter().enumerate().any(|(index, &expected)| {
        !same(state.ideal_positions[index], expected)
            || state
                .original_positions
                .iter()
                .chain(state.positions)
                .any(|positions| !same(positions[index], expected))
    });
    if !moved {
        return Ok(());
    }
    let fine = triangle_set(
        &state.data.infos[0]
            .triangle_indices()
            .map_err(|error| invalid(error.to_string()))?,
    )?;
    let maximal = triangle_set(state.indices)?;
    if all_discarded(mesh, &fine, &maximal, *state, cut, work)? {
        return Ok(());
    }
    Err(invalid(
        "Actual native LOD geometry must preserve every retained fine-grid sample under every reconstruction profile; all changed LOD triangles and triangles incident to moved samples must lie strictly in the discarded half",
    ))
}

pub(super) fn certify(
    face: &Face,
    info: &DispInfo,
    mesh: &DisplacementMesh,
    field: &PlanarField,
    state: State<'_>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<Fields, EditError> {
    let native_info = validate_state(info, mesh, &state, work)?;
    let samples = sample_fields(mesh, &state, None, texture, work)?;
    preserve_positions(mesh, &state, None, work)?;
    let size = mesh.size;
    let count = size * size;
    work.consume(2 * FIELD_WORK * count).map_err(invalid)?;
    for (index, &base) in mesh.base_positions.iter().enumerate() {
        let expected = field.position(base)?;
        if !same(field.base_position(expected)?, base) || !same(mesh.positions[index], expected) {
            return Err(invalid(
                "Actual native LOD geometry must preserve its complete exact affine mapping under every reconstruction profile",
            ));
        }
    }
    uv(state.data, face, count, texture, work)?;
    certify_normals(state.data, state.positions, work)?;
    work.consume(4 * FIELD_WORK).map_err(invalid)?;
    let corners = [0, (size - 1) * size, count - 1, size - 1];
    let ring = corners
        .map(|index| field.position(mesh.base_positions[index]))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let axis = field.world_axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let sign = orient(ring[0], ring[1], ring[2], axes)?;
    if sign == 0 {
        return Err(invalid("Terrain LOD analytic coverage domain is collapsed"));
    }
    let fine = native_info
        .collision_triangle_indices()
        .map_err(|error| invalid(error.to_string()))?;
    for positions in state.positions {
        let mut maximal = Region::new(&ring, axes, sign, None, Some(field));
        maximal.add(positions, state.indices, size, Some(state.active), work)?;
        maximal.finish()?;
        let mut collision = Region::new(&ring, axes, sign, None, Some(field));
        collision.add(positions, &fine, size, None, work)?;
        collision.finish()?;
    }
    Ok(Fields {
        field: *field,
        samples,
    })
}

fn samples(
    face: &Face,
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: State<'_>,
    cut: Option<(Plane, Keep)>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<SampleFields, EditError> {
    validate_state(info, mesh, &state, work)?;
    let samples = sample_fields(mesh, &state, cut, texture, work)?;
    preserve_positions(mesh, &state, cut, work)?;
    uv(state.data, face, mesh.positions.len(), texture, work)?;
    Ok(samples)
}

pub(super) fn certify_samples(
    face: &Face,
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: State<'_>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<SampleFields, EditError> {
    let samples = samples(face, info, mesh, state, None, texture, work)?;
    geometry(mesh, state, None, work)?;
    Ok(samples)
}

pub(super) fn certify_curved(
    face: &Face,
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: State<'_>,
    cut: Option<(Plane, Keep)>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<SampleFields, EditError> {
    let samples = samples(face, info, mesh, state, cut, texture, work)?;
    geometry(mesh, state, cut, work)?;
    Ok(samples)
}

pub(super) fn certify_geometry(
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: State<'_>,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<(), EditError> {
    validate_state(info, mesh, &state, work)?;
    preserve_positions(mesh, &state, cut, work)?;
    geometry(mesh, state, cut, work)
}

fn geometry(
    mesh: &DisplacementMesh,
    state: State<'_>,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<(), EditError> {
    let native_info = state.data.infos[0];
    let count = native_info
        .triangle_count()
        .map_err(|error| invalid(error.to_string()))?;
    work.consume(FIELD_WORK * (count + state.indices.len()))
        .map_err(invalid)?;
    let fine = native_info
        .triangle_indices()
        .map_err(|error| invalid(error.to_string()))?;
    let fine_set = triangle_set(&fine)?;
    let maximal = triangle_set(state.indices)?;
    if all_discarded(mesh, &fine_set, &maximal, state, cut, work)? {
        return Ok(());
    }
    equivalent(mesh, state, &fine, &fine_set, &maximal, work)
}

fn all_discarded(
    mesh: &DisplacementMesh,
    fine: &BTreeSet<[usize; 3]>,
    maximal: &BTreeSet<[usize; 3]>,
    state: State<'_>,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<bool, EditError> {
    let Some((plane, keep)) = cut else {
        return Ok(false);
    };
    if !plane.normal.is_finite() || plane.normal == DVec3::ZERO || !plane.dist.is_finite() {
        return Err(invalid(
            "Terrain LOD requires a finite noncollapsed cut plane",
        ));
    }
    if keep == Keep::Both {
        return Ok(false);
    }
    work.consume(
        FIELD_WORK * (fine.len() + maximal.len())
            + (state.original_positions.len() + state.positions.len() + 1) * mesh.positions.len(),
    )
    .map_err(invalid)?;
    let moved = mesh
        .positions
        .iter()
        .enumerate()
        .map(|(index, &point)| {
            !same(state.ideal_positions[index], point)
                || state
                    .original_positions
                    .iter()
                    .chain(state.positions)
                    .any(|positions| !same(positions[index], point))
        })
        .collect::<Vec<_>>();
    let affected = fine
        .union(maximal)
        .filter(|triangle| {
            fine.contains(*triangle) != maximal.contains(*triangle)
                || triangle.iter().any(|&index| moved[index])
        })
        .collect::<Vec<_>>();
    work.consume(
        FIELD_WORK
            * affected.len()
            * 3
            * (state.original_positions.len() + state.positions.len() + 2),
    )
    .map_err(invalid)?;
    let normal = [
        Exact::float(plane.normal.x)?,
        Exact::float(plane.normal.y)?,
        Exact::float(plane.normal.z)?,
    ];
    let distance = Exact::float(plane.dist)?;
    let epsilon = Exact::float(CUT_EPSILON)?;
    for triangle in affected {
        for positions in std::iter::once(mesh.positions.as_slice())
            .chain(state.original_positions.iter().map(Vec::as_slice))
            .chain(std::iter::once(state.ideal_positions))
            .chain(state.positions.iter().map(Vec::as_slice))
        {
            for &index in triangle {
                let mut side = Exact::new(0, 0);
                for (&coefficient, coordinate) in normal.iter().zip(positions[index].to_array()) {
                    side = side.add(coefficient.mul(Exact::float(coordinate)?)?)?;
                }
                let side = side.sub(distance)?;
                let discarded = match keep {
                    Keep::Front => side.add(epsilon)?.value < 0,
                    Keep::Back => side.sub(epsilon)?.value > 0,
                    Keep::Both => false,
                };
                if !discarded {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn equivalent(
    mesh: &DisplacementMesh,
    state: State<'_>,
    fine_indices: &[[usize; 3]],
    fine: &BTreeSet<[usize; 3]>,
    maximal: &BTreeSet<[usize; 3]>,
    work: &mut Work,
) -> Result<(), EditError> {
    let size = mesh.size;
    let ring =
        [0, (size - 1) * size, size * size - 1, size - 1].map(|index| mesh.base_positions[index]);
    work.consume(FIELD_WORK * (5 + mesh.base_positions.len()))
        .map_err(invalid)?;
    let plane = BasePlane::new([ring[0], ring[1], ring[2]])?;
    let axis = plane.axis();
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let sign = orient(ring[0], ring[1], ring[2], axes)?;
    convex(&ring, axes, sign)?;
    for &base in &mesh.base_positions {
        if !plane.contains(base)? {
            return Err(invalid(
                "Terrain LOD base coverage requires one exact plane",
            ));
        }
    }
    let mut fine_cover = Region::new(&ring, axes, sign, None, None);
    fine_cover.add(&mesh.base_positions, fine_indices, size, None, work)?;
    fine_cover.finish()?;
    let mut maximal_cover = Region::new(&ring, axes, sign, None, None);
    maximal_cover.add(
        &mesh.base_positions,
        state.indices,
        size,
        Some(state.active),
        work,
    )?;
    maximal_cover.finish()?;
    if fine == maximal {
        return Ok(());
    }
    let removed_count = fine.difference(maximal).count();
    let added_count = maximal.difference(fine).count();
    if removed_count == 0 || added_count == 0 {
        return Err(invalid(
            "Terrain LOD changed domains require both original fine and maximal triangles",
        ));
    }
    work.consume(
        17 * FIELD_WORK * (removed_count + added_count)
            + labels::AABB_WORK * removed_count * added_count
            + removed_count,
    )
    .map_err(invalid)?;
    let mut removed = fine
        .difference(maximal)
        .map(|&triangle| {
            let base = triangle.map(|index| mesh.base_positions[index]);
            Ok((
                labels::Projected::new(base, axes)?,
                PlanarField::triangle(base, triangle.map(|index| mesh.positions[index]))?,
                false,
            ))
        })
        .collect::<Result<Vec<_>, EditError>>()?;
    for &triangle in maximal.difference(fine) {
        let base = triangle.map(|index| mesh.base_positions[index]);
        let projected = labels::Projected::new(base, axes)?;
        let field = PlanarField::triangle(base, triangle.map(|index| mesh.positions[index]))?;
        let mut matched = false;
        for (source, surface, covered) in &mut removed {
            if !projected.overlaps(source, axes, work)? {
                continue;
            }
            work.consume(FIELD_WORK).map_err(invalid)?;
            if !field.matches(surface)? {
                return Err(invalid(
                    "Native LOD triangle changed an overlapping original affine surface field",
                ));
            }
            *covered = true;
            matched = true;
        }
        if !matched {
            return Err(invalid(
                "Native LOD triangle has no positive-area original fine coverage",
            ));
        }
    }
    if removed.iter().any(|(_, _, covered)| !*covered) {
        return Err(invalid(
            "Original fine triangle has no positive-area native LOD coverage",
        ));
    }
    Ok(())
}

fn triangle_set(triangles: &[[usize; 3]]) -> Result<BTreeSet<[usize; 3]>, EditError> {
    let mut result = BTreeSet::new();
    for &triangle in triangles {
        let first = (0..3).min_by_key(|&index| triangle[index]).unwrap();
        let mut canonical = triangle;
        canonical.rotate_left(first);
        if !result.insert(canonical) {
            return Err(invalid(
                "Terrain LOD proof requires unique oriented native triangles",
            ));
        }
    }
    Ok(result)
}
