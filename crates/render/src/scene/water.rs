use glam::{DVec3, Vec3};
use map_core::brush::Face;
use map_core::{BrushGeometry, Plane, surface::FaceId};
use source_formats::{gamefs::GameFs, vmf::Solid};

use super::{Batch, SceneIssue, SceneMesh, WATER_CAPTURE_BRUSH};
use crate::materials::{MaterialId, MaterialLibrary};

pub(crate) const CLIP_BIAS: f32 = 2.0;
pub(crate) const NATIVE_NEAR: f32 = 7.0;
pub(crate) const NATIVE_FAR: f32 = 28_377.92;
const MAX_SCENE_TRIANGLES: usize = 250_000;
const MAX_CAPTURE_RANGES: usize = 512;
const CUBOID_EPSILON: f64 = 1e-6;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pool {
    pub face: FaceId,
    pub material: MaterialId,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub tangent: [f32; 4],
}

impl Pool {
    pub(super) fn from_brush(
        solid: &Solid,
        brush: &BrushGeometry,
        top: &Face,
        material: MaterialId,
        dimensions: [u32; 2],
    ) -> Result<Self, String> {
        brush.check_axis_aligned_cuboid(CUBOID_EPSILON)?;
        if top.plane.normal != DVec3::Z
            || solid.sides.len() != 6
            || solid.sides.iter().enumerate().any(|(index, side)| {
                side.displacement.is_some()
                    || index != top.side && !side.material.eq_ignore_ascii_case("tools/toolsnodraw")
            })
        {
            return Err("Water refraction needs one horizontal top and five nodraw sides".into());
        }
        if !brush.mins.is_finite()
            || !brush.maxs.is_finite()
            || brush
                .mins
                .abs()
                .max_element()
                .max(brush.maxs.abs().max_element())
                > map_core::checks::SOURCE_COORDINATE_LIMIT
            || (brush.maxs - brush.mins).min_element() < 1.0
        {
            return Err(
                "Water refraction needs finite Source bounds at least one unit wide".into(),
            );
        }
        let side = &solid.sides[top.side];
        let u = side.u_axis.axis.truncate().normalize_or_zero();
        let v = side.v_axis.axis.truncate().normalize_or_zero();
        if side.u_axis.axis.z != 0.0 || side.v_axis.axis.z != 0.0 || u.dot(v).abs() > CUBOID_EPSILON
        {
            return Err("Water refraction needs orthogonal horizontal U and V axes".into());
        }
        let tangent = crate::normal_maps::tangent(
            side,
            top.plane.normal,
            &top.vertices,
            dimensions,
            crate::materials::uv::IDENTITY_AXIS,
        )?;
        Ok(Self {
            face: FaceId {
                solid: solid.id,
                side: side.id,
            },
            material,
            mins: brush.mins.as_vec3(),
            maxs: brush.maxs.as_vec3(),
            tangent,
        })
    }

    pub(crate) fn issue(self, message: impl Into<String>) -> SceneIssue {
        SceneIssue::Surface {
            face: self.face,
            message: message.into(),
        }
    }

    pub(crate) fn captures(
        self,
        mesh: &SceneMesh,
        fs: &GameFs,
        library: &MaterialLibrary,
    ) -> Result<Vec<Batch>, String> {
        if mesh.indices.len() / 3 > MAX_SCENE_TRIANGLES {
            return Err(
                "Water refraction is limited to scenes with at most 250,000 triangles".into(),
            );
        }
        crate::materials::water::verify_nodraw(fs)?;
        let mut batches: Vec<Batch> = Vec::new();
        let mut verified = std::collections::HashSet::new();
        let planes: [Plane; 6] = std::array::from_fn(|index| {
            let axis = index / 2;
            let mut normal = DVec3::ZERO;
            normal[axis] = if index % 2 == 0 { 1.0 } else { -1.0 };
            let dist = if index % 2 == 0 {
                f64::from(self.maxs[axis])
            } else {
                -f64::from(self.mins[axis])
            };
            Plane { normal, dist }
        });
        for (index, batch) in mesh.batches.iter().enumerate() {
            let info = library.get(batch.material);
            if batch.material == self.material
                || batch.material == MaterialLibrary::EDITOR
                || info.tool && !info.sky
                || info.reference
            {
                continue;
            }
            let start = batch.first_index as usize;
            let end = start + batch.index_count as usize;
            for (triangle_index, triangle) in mesh.indices[start..end]
                .as_chunks::<3>()
                .0
                .iter()
                .enumerate()
            {
                let vertices = triangle.map(|index| &mesh.vertices[index as usize]);
                let [a, b, c] = vertices.map(|vertex| Vec3::from(vertex.position));
                if !a.min(b).min(c).cmple(self.maxs).all()
                    || !a.max(b).max(c).cmpge(self.mins).all()
                    || !intersects([a, b, c], &planes)
                {
                    continue;
                }
                if !info.water_receiver
                    || vertices
                        .iter()
                        .any(|v| v.texture_flags & WATER_CAPTURE_BRUSH == 0)
                    || batch.rope
                    || batch.model_translucent
                    || mesh.projected_overlay_batches.contains(&index)
                    || mesh.baked_face_batches.contains(&index)
                {
                    return Err(format!(
                        "Water refraction needs plain opaque world or func_detail brush faces inside the pool; {} is unsupported",
                        info.name
                    ));
                }
                if verified.insert(batch.material) {
                    let texture = info.base.as_ref().ok_or_else(|| {
                        format!("Water receiver {} has no base texture", info.name)
                    })?;
                    crate::materials::validate_ordinary_color_texture(texture, u32::from(u16::MAX))
                        .map_err(|error| format!("Water receiver {}: {error}", info.name))?;
                    crate::materials::water::verify_receiver(fs, info)?;
                }
                let first_index = batch.first_index + (triangle_index * 3) as u32;
                if let Some(last) = batches.last_mut()
                    && last.material == batch.material
                    && last.first_index + last.index_count == first_index
                {
                    last.index_count += 3;
                } else {
                    if batches.len() == MAX_CAPTURE_RANGES {
                        return Err("Water refraction exceeds 512 submerged draw ranges".into());
                    }
                    batches.push(Batch {
                        first_index,
                        index_count: 3,
                        ..*batch
                    });
                }
            }
        }
        if batches.is_empty() {
            return Err("Water refraction needs visible submerged brush geometry".into());
        }
        Ok(batches)
    }
}

fn intersects(triangle: [Vec3; 3], planes: &[Plane; 6]) -> bool {
    let triangle = triangle.map(|point| point.as_dvec3());
    if planes.iter().all(|plane| {
        triangle
            .iter()
            .all(|&point| plane.distance_to(point) <= 0.0)
    }) {
        return true;
    }
    let mut polygon = triangle.to_vec();
    for plane in planes {
        if polygon.iter().any(|&point| plane.distance_to(point) > 0.0) {
            polygon = plane.clip_behind_exact(&polygon);
            if polygon.len() < 3 {
                return false;
            }
        }
    }
    polygon[1..].windows(2).any(|edge| {
        (edge[0] - polygon[0])
            .cross(edge[1] - polygon[0])
            .length_squared()
            > 0.0
    })
}
