use glam::DVec3;
use source_formats::{bsp, vmf::DispInfo};

use super::DisplacementMesh;
use crate::{brush::Face, checks::SOURCE_COORDINATE_LIMIT};

const VERTEX_PRECISION: f64 = 10_000.0;
pub const GEOMETRY_EPSILON: f64 = 0.02;

pub fn vertex_key(point: DVec3) -> [i64; 3] {
    (point * VERTEX_PRECISION).round().as_i64vec3().to_array()
}

pub fn vertex_position(key: [i64; 3]) -> [f32; 3] {
    key.map(|value| (value as f64 / VERTEX_PRECISION) as f32)
}

pub fn stored_vertex(point: DVec3) -> DVec3 {
    DVec3::from_array(vertex_position(vertex_key(point)).map(f64::from))
}

pub fn checked_mesh(
    face: &Face,
    disp: &DispInfo,
) -> Result<([DVec3; 4], DisplacementMesh), String> {
    if disp.flags.unwrap_or(0) & !14 != 0 {
        return Err("Bumped/unknown displacement flags are not supported yet".into());
    }
    let base: [DVec3; 4] = face
        .vertices
        .as_slice()
        .try_into()
        .map_err(|_| "Displacements need a convex quad base face")?;
    if !disp.start_position.is_finite()
        || base
            .iter()
            .all(|point| point.distance(disp.start_position) > GEOMETRY_EPSILON)
    {
        return Err("Displacement start position must match a base corner".into());
    }
    let mesh = DisplacementMesh::build(face, disp).map_err(|error| error.to_string())?;
    if mesh
        .positions
        .iter()
        .any(|point| !point.is_finite() || point.abs().max_element() > SOURCE_COORDINATE_LIMIT)
    {
        return Err("Displacement positions must fit Source compile bounds".into());
    }
    if mesh.triangles().any(|[a, b, c]| {
        (mesh.positions[b] - mesh.positions[a])
            .cross(mesh.positions[c] - mesh.positions[a])
            .length_squared()
            < 1e-12
    }) {
        return Err("Displacement contains collapsed triangles".into());
    }
    Ok((base, mesh))
}

pub fn validate_material(draws: bool, material_contents: i32, surface: i32) -> Result<(), String> {
    if !draws
        || material_contents & !bsp::contents::DETAIL != bsp::contents::SOLID
        || surface & (bsp::surf::SKY | bsp::surf::SKY2D | bsp::surf::WARP) != 0
    {
        return Err("Displacements currently require a visible ordinary solid material".into());
    }
    Ok(())
}

pub fn vertices(mesh: &DisplacementMesh) -> Vec<bsp::DisplacementVertex> {
    mesh.positions
        .iter()
        .zip(&mesh.base_positions)
        .zip(&mesh.alphas)
        .map(|((&point, &base), &alpha)| {
            let offset = point - base;
            let distance = offset.length();
            bsp::DisplacementVertex {
                direction: if distance == 0.0 {
                    [0.0; 3]
                } else {
                    (offset / distance).as_vec3().to_array()
                },
                distance: distance as f32,
                alpha: alpha as f32,
            }
        })
        .collect()
}

pub fn positions(
    base: [DVec3; 4],
    info: bsp::DisplacementInfo,
    vertices: Vec<bsp::DisplacementVertex>,
) -> Result<Vec<[f32; 3]>, String> {
    let data = bsp::Displacements {
        infos: vec![info],
        vertices,
        triangles: Vec::new(),
    };
    data.positions(
        0,
        base.map(|point| stored_vertex(point).as_vec3().to_array()),
    )
    .map_err(|error| error.to_string())
}
