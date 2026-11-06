use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::{bsp, vmf::DispInfo};

use super::{
    CUT_EPSILON, Exact, FIELD_WORK, PaintField, PlanarField, Region, Work, certify_height_normals,
    certify_normals, field::BasePlane, invalid, labels, orient, same, uv,
};
use crate::clip::{Keep, TerrainTexture};
use crate::{EditError, Face, Plane, displacement::DisplacementMesh};

#[derive(Clone, Copy)]
pub(super) struct State<'a> {
    pub(super) data: &'a bsp::Displacements,
    pub(super) positions: &'a [Vec<DVec3>],
    pub(super) ideal_positions: &'a [DVec3],
    pub(super) indices: &'a [[usize; 3]],
    pub(super) active: &'a [bool],
    pub(super) mask: &'a [u32; 10],
}

pub(super) struct Fields {
    field: PlanarField,
    uniform: UniformFields,
}

pub(super) struct UniformFields {
    paint: PaintField,
    label: labels::UniformLabel,
    texture: TerrainTexture,
}

impl Fields {
    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        work.consume(FIELD_WORK).map_err(invalid)?;
        Ok(self.field.matches(&other.field)? && self.uniform.matches(&other.uniform, work)?)
    }
}

impl UniformFields {
    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        Ok(self.label == other.label
            && self.texture == other.texture
            && self.paint.matches(&other.paint, work)?)
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

fn uniform_fields(
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: &State<'_>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<UniformFields, EditError> {
    let alpha = mesh.alphas[0];
    if mesh.alphas.iter().any(|&value| value != alpha) {
        return Err(invalid(
            "Retiled native LOD currently requires constant terrain paint",
        ));
    }
    let size = mesh.size;
    let corners = [0, (size - 1) * size, size * size - 1, size - 1];
    let paint = PaintField::new(
        corners.map(|index| mesh.base_positions[index]),
        [alpha; 4],
        work,
    )?;
    paint.certify(mesh, state.data, work)?;
    let label = labels::uniform(info, mesh, texture.has_secondary_surface, work)?;
    Ok(UniformFields {
        paint,
        label,
        texture,
    })
}

fn preserve_positions(
    mesh: &DisplacementMesh,
    state: &State<'_>,
    work: &mut Work,
) -> Result<(), EditError> {
    work.consume((state.positions.len() + 1) * mesh.positions.len())
        .map_err(invalid)?;
    for (index, &expected) in mesh.positions.iter().enumerate() {
        if !same(state.ideal_positions[index], expected)
            || state
                .positions
                .iter()
                .any(|positions| !same(positions[index], expected))
        {
            return Err(invalid(
                "Actual native LOD geometry must preserve every original fine-grid sample under every reconstruction profile",
            ));
        }
    }
    Ok(())
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
    let uniform = uniform_fields(info, mesh, &state, texture, work)?;
    preserve_positions(mesh, &state, work)?;
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
        uniform,
    })
}

pub(super) fn certify_uniform(
    face: &Face,
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: State<'_>,
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<UniformFields, EditError> {
    validate_state(info, mesh, &state, work)?;
    let uniform = uniform_fields(info, mesh, &state, texture, work)?;
    preserve_positions(mesh, &state, work)?;
    uv(state.data, face, mesh.positions.len(), texture, work)?;
    work.consume(FIELD_WORK).map_err(invalid)?;
    let axis = BasePlane::new([
        mesh.base_positions[0],
        mesh.base_positions[(mesh.size - 1) * mesh.size],
        mesh.base_positions[mesh.size * mesh.size - 1],
    ])?
    .axis();
    certify_height_normals(state.data, state.positions, axis, work)?;
    Ok(uniform)
}

pub(super) fn certify_discarded(
    face: &Face,
    info: &DispInfo,
    mesh: &DisplacementMesh,
    state: State<'_>,
    cut: (Plane, Keep),
    texture: TerrainTexture,
    work: &mut Work,
) -> Result<UniformFields, EditError> {
    let (plane, keep) = cut;
    if keep == Keep::Both {
        return Err(invalid(
            "Curved source LOD requires a cut that discards every changed triangle",
        ));
    }
    if !plane.normal.is_finite() || plane.normal == DVec3::ZERO || !plane.dist.is_finite() {
        return Err(invalid(
            "Terrain LOD requires a finite noncollapsed cut plane",
        ));
    }
    let uniform = certify_uniform(face, info, mesh, state, texture, work)?;
    let native_info = state.data.infos[0];
    let count = native_info
        .triangle_count()
        .map_err(|error| invalid(error.to_string()))?;
    work.consume(FIELD_WORK * (count + state.indices.len()))
        .map_err(invalid)?;
    let fine = native_info
        .triangle_indices()
        .map_err(|error| invalid(error.to_string()))?;
    let fine = triangle_set(&fine)?;
    let maximal = triangle_set(state.indices)?;
    work.consume(FIELD_WORK * (fine.len() + maximal.len()) * 3 * (state.positions.len() + 1))
        .map_err(invalid)?;
    let normal = [
        Exact::float(plane.normal.x)?,
        Exact::float(plane.normal.y)?,
        Exact::float(plane.normal.z)?,
    ];
    let distance = Exact::float(plane.dist)?;
    let epsilon = Exact::float(CUT_EPSILON)?;
    for triangle in fine.symmetric_difference(&maximal) {
        for positions in
            std::iter::once(state.ideal_positions).chain(state.positions.iter().map(Vec::as_slice))
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
                    return Err(invalid(
                        "Every changed native LOD triangle must lie strictly in the discarded half beyond cut precision under every reconstruction profile",
                    ));
                }
            }
        }
    }
    Ok(uniform)
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
