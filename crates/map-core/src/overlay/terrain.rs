use std::cell::OnceCell;

use glam::{DVec2, DVec3};
use source_formats::{
    bsp::DisplacementInfo,
    vmf::{DispInfo, Vmf},
};

use super::{BASIS_TOLERANCE, QUAD_AREA_EPSILON};
use crate::{
    brush::Face,
    checks::SOURCE_COORDINATE_LIMIT,
    displacement::{DisplacementMesh, lod, native, smoothing, surface, triangles},
    plane::ON_EPSILON,
    polygon,
    surface::FaceId,
};

pub(super) struct Target<'a> {
    pub base: [DVec3; 4],
    pub planar: Option<Planar<'a>>,
}

pub(super) struct Planar<'a> {
    pub normal: DVec3,
    pub corners: [DVec3; 4],
    prepared: &'a [DVec3],
    native: Option<Vec<DVec3>>,
}

impl Planar<'_> {
    pub(super) fn samples(&self) -> impl Iterator<Item = &DVec3> {
        self.prepared.iter().chain(self.native.iter().flatten())
    }
}

pub(super) fn target<'a>(
    vmf: &Vmf,
    face: FaceId,
    target: &Face,
    info: &DispInfo,
    prepared: &'a OnceCell<surface::Prepared>,
) -> Result<Target<'a>, String> {
    if vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) {
        return Err("Terrain overlay authoring requires terrain topology from the map; hide visible instances in saved map visibility or use an imported overlay. Temporary editor hiding does not qualify".into());
    }
    let (cage, mesh) = native::checked_mesh(target, info)?;
    smoothing::angle(info)?;
    triangles::native_tags(info, &mesh).map_err(|error| error.to_string())?;
    let prepared = prepared.get_or_init(|| surface::prepare(vmf));
    let current = prepared.get(&face).ok_or_else(|| {
        prepared
            .issues
            .get(&face)
            .cloned()
            .unwrap_or_else(|| "Overlay target has no prepared terrain surface".into())
    })?;
    let facing = target.plane.normal;
    let base = native::stored_base_corners(&mesh);
    validate_base(&base, facing)?;
    for (id, other) in &prepared.meshes {
        if *id == face {
            continue;
        }
        let other_base = native::stored_base_corners(other);
        if other_base
            .iter()
            .all(|point| facing.dot(*point - base[0]).abs() <= ON_EPSILON)
            && overlapping_interiors(&base, &other_base, facing)
        {
            return Err(format!(
                "Overlay terrain base overlaps brush {} face {}; separate overlapping supports",
                id.solid, id.side
            ));
        }
    }
    let native = native_positions(cage, &mesh, current, info.power)?;
    validate_grid(current, facing, &base, &current.positions)?;
    let native = if native != current.positions {
        validate_grid(current, facing, &base, &native)?;
        Some(native)
    } else {
        None
    };
    let planar =
        planar_quad(&current.positions, current.size, facing).and_then(|(normal, corners)| {
            if let Some(native) = &native {
                planar_quad(native, current.size, normal)?;
                if native
                    .iter()
                    .any(|point| normal.dot(*point - corners[0]).abs() > ON_EPSILON)
                    || corners
                        .iter()
                        .zip(corner_indices(current.size))
                        .any(|(corner, index)| !corner.abs_diff_eq(native[index], ON_EPSILON))
                {
                    return None;
                }
            }
            Some(Planar {
                normal,
                corners,
                prepared: &current.positions,
                native,
            })
        });
    Ok(Target { base, planar })
}

fn corner_indices(size: usize) -> [usize; 4] {
    [0, (size - 1) * size, size * size - 1, size - 1]
}

fn validate_base(base: &[DVec3; 4], normal: DVec3) -> Result<(), String> {
    for index in 0..base.len() {
        let [a, b, c] =
            [index, (index + 1) % base.len(), (index + 2) % base.len()].map(|index| base[index]);
        if normal.dot(a - base[0]).abs() > ON_EPSILON
            || (a - b).cross(c - b).dot(normal) < QUAD_AREA_EPSILON
        {
            return Err("Overlay terrain requires a convex native base quad".into());
        }
    }
    Ok(())
}

fn native_positions(
    cage: [DVec3; 4],
    raw: &DisplacementMesh,
    current: &DisplacementMesh,
    power: u32,
) -> Result<Vec<DVec3>, String> {
    let info = DisplacementInfo {
        start: native::stored_vertex(raw.base_positions[0])
            .as_vec3()
            .to_array(),
        power: power as i32,
        ..Default::default()
    };
    let mut vertices = native::vertices(raw);
    let positions = native::positions(cage, info, vertices.clone())?
        .into_iter()
        .map(|point| DVec3::from_array(point.map(f64::from)))
        .collect::<Vec<_>>();
    if positions == current.positions {
        return Ok(positions);
    }
    for ((vertex, &before), &after) in vertices.iter_mut().zip(&positions).zip(&current.positions) {
        if before != after {
            *vertex = lod::encode_snapped_vertex(*vertex, before, after);
        }
    }
    let positions = native::positions(cage, info, vertices)?
        .into_iter()
        .map(|point| DVec3::from_array(point.map(f64::from)))
        .collect::<Vec<_>>();
    if positions
        .iter()
        .zip(&current.positions)
        .any(|(native, prepared)| !native.abs_diff_eq(*prepared, ON_EPSILON))
    {
        return Err(
            "Overlay terrain differs from native surface precision after terrain LOD".into(),
        );
    }
    Ok(positions)
}

fn validate_grid(
    mesh: &DisplacementMesh,
    normal: DVec3,
    base: &[DVec3; 4],
    positions: &[DVec3],
) -> Result<(), String> {
    if positions
        .iter()
        .any(|point| !point.is_finite() || point.abs().max_element() > SOURCE_COORDINATE_LIMIT)
    {
        return Err(format!(
            "Overlay terrain must fit finite Source coordinates within ±{SOURCE_COORDINATE_LIMIT} units"
        ));
    }
    if mesh.triangles().any(|[a, b, c]| {
        (positions[a] - positions[b])
            .cross(positions[c] - positions[b])
            .dot(normal)
            < QUAD_AREA_EPSILON
    }) {
        return Err(
            "Overlay terrain contains collapsed or folded triangles in its projection".into(),
        );
    }
    let u = (base[3] - base[0]).normalize();
    let v = normal.cross(u);
    let outline = mesh
        .outline()
        .into_iter()
        .map(|index| {
            let local = positions[index] - base[0];
            DVec2::new(local.dot(u), local.dot(v))
        })
        .collect::<Vec<_>>();
    let count = outline.len();
    for i in 0..count {
        for j in (i + 2)..count {
            if (j + 1) % count != i
                && polygon::intersects(
                    outline[i],
                    outline[i + 1],
                    outline[j],
                    outline[(j + 1) % count],
                )
            {
                return Err("Overlay terrain boundary crosses itself in its projection".into());
            }
        }
    }
    Ok(())
}

fn planar_quad(positions: &[DVec3], size: usize, facing: DVec3) -> Option<(DVec3, [DVec3; 4])> {
    let corners = corner_indices(size).map(|index| positions[index]);
    let normal = (corners[3] - corners[0])
        .cross(corners[1] - corners[0])
        .try_normalize()?;
    if normal.dot(facing) <= BASIS_TOLERANCE
        || positions
            .iter()
            .any(|point| normal.dot(*point - corners[0]).abs() > ON_EPSILON)
    {
        return None;
    }
    for row in 0..size {
        for column in 0..size {
            let point = positions[row * size + column];
            for edge in 0..4 {
                let a = corners[edge];
                let b = corners[(edge + 1) % 4];
                if a.distance_squared(b) < QUAD_AREA_EPSILON
                    || (a - b).cross(point - a).dot(normal) / a.distance(b) < -ON_EPSILON
                {
                    return None;
                }
            }
            let edge = if column == 0 {
                Some((corners[0], corners[1]))
            } else if column == size - 1 {
                Some((corners[3], corners[2]))
            } else if row == 0 {
                Some((corners[0], corners[3]))
            } else if row == size - 1 {
                Some((corners[1], corners[2]))
            } else {
                None
            };
            if edge.is_some_and(|(a, b)| {
                let edge = b - a;
                let t = ((point - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
                !point.abs_diff_eq(a + edge * t, ON_EPSILON)
            }) {
                return None;
            }
        }
    }
    if crate::displacement::grid_triangles(size).any(|[a, b, c]| {
        (positions[a] - positions[b])
            .cross(positions[c] - positions[b])
            .dot(normal)
            < QUAD_AREA_EPSILON
    }) {
        return None;
    }
    Some((normal, corners))
}

pub(super) fn overlapping_interiors(first: &[DVec3], second: &[DVec3], normal: DVec3) -> bool {
    for polygon in [first, second] {
        for i in 0..polygon.len() {
            let Some(axis) = (polygon[(i + 1) % polygon.len()] - polygon[i])
                .cross(normal)
                .try_normalize()
            else {
                continue;
            };
            let first = interval(first, axis);
            let second = interval(second, axis);
            if first[1].min(second[1]) - first[0].max(second[0]) <= ON_EPSILON {
                return false;
            }
        }
    }
    true
}

fn interval(vertices: &[DVec3], axis: DVec3) -> [f64; 2] {
    vertices
        .iter()
        .fold([f64::INFINITY, f64::NEG_INFINITY], |range, vertex| {
            let point = vertex.dot(axis);
            [range[0].min(point), range[1].max(point)]
        })
}
