use glam::DVec3;
use source_formats::{bsp, vmf::DispInfo};

use super::{
    FIELD_WORK, PaintField, PlanarField, Region, Work, certify_normals, invalid, labels, orient,
    same, uv,
};
use crate::clip::TerrainTexture;
use crate::{EditError, Face, displacement::DisplacementMesh};

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
    paint: PaintField,
    label: labels::UniformLabel,
    texture: TerrainTexture,
}

impl Fields {
    pub(super) fn matches(&self, other: &Self, work: &mut Work) -> Result<bool, EditError> {
        work.consume(FIELD_WORK).map_err(invalid)?;
        Ok(self.label == other.label
            && self.texture == other.texture
            && self.field.matches(&other.field)?
            && self.paint.matches(&other.paint, work)?)
    }
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
    let alpha = mesh.alphas[0];
    if mesh.alphas.iter().any(|&value| value != alpha) {
        return Err(invalid(
            "Retiled native LOD currently requires constant terrain paint",
        ));
    }
    let corners = [0, (size - 1) * size, count - 1, size - 1];
    let paint = PaintField::new(
        corners.map(|index| mesh.base_positions[index]),
        [alpha; 4],
        work,
    )?;
    paint.certify(mesh, state.data, work)?;
    let label = labels::uniform(info, mesh, texture.has_secondary_surface, work)?;
    work.consume((2 * FIELD_WORK + state.positions.len() + 1) * count)
        .map_err(invalid)?;
    for (index, &base) in mesh.base_positions.iter().enumerate() {
        let expected = field.position(base)?;
        if !same(field.base_position(expected)?, base)
            || !same(mesh.positions[index], expected)
            || !same(state.ideal_positions[index], expected)
            || state
                .positions
                .iter()
                .any(|positions| !same(positions[index], expected))
        {
            return Err(invalid(
                "Actual native LOD geometry must preserve its complete exact affine mapping under every reconstruction profile",
            ));
        }
    }
    uv(state.data, face, count, texture, work)?;
    certify_normals(state.data, state.positions, work)?;
    work.consume(4 * FIELD_WORK).map_err(invalid)?;
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
        paint,
        label,
        texture,
    })
}
