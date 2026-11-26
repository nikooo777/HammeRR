use glam::{DVec2, DVec3, Vec3};
use map_core::brush::Face;
use map_core::{BrushGeometry, Plane, surface::FaceId};
use source_formats::{gamefs::GameFs, vmf::Solid};

use super::{Batch, SceneIssue, SceneMesh, Vertex, WATER_CAPTURE_BRUSH};
use crate::Camera;
use crate::materials::{MaterialId, MaterialLibrary};

pub(crate) const CLIP_BIAS: f32 = 2.0;
pub(crate) const NATIVE_NEAR: f32 = 7.0;
pub(crate) const NATIVE_FAR: f32 = 28_377.92;
const MAX_SCENE_TRIANGLES: usize = 250_000;
const MAX_CAPTURE_RANGES: usize = 512;
const CUBOID_EPSILON: f64 = 1e-6;
const MIRROR_EYE_CLEARANCE: f32 = 12.0;
const CEILING_MARGIN: f64 = 1.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pool {
    pub face: FaceId,
    pub material: MaterialId,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub tangent: [f32; 4],
}

#[derive(Debug)]
pub(crate) struct Captures {
    pub pool: Pool,
    pub refraction: Vec<Batch>,
    pub reflection: Option<Mirror>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Mirror {
    pub batches: Vec<Batch>,
    ceilings: Vec<Ceiling>,
}

#[derive(Debug, Clone, Copy)]
struct Ceiling {
    height: f64,
    mins: DVec2,
    maxs: DVec2,
}

impl Mirror {
    pub(crate) fn camera(
        &self,
        pool: Pool,
        camera: &Camera,
        aspect: f32,
    ) -> Result<Camera, &'static str> {
        if camera.position.z - pool.maxs.z < MIRROR_EYE_CLEARANCE {
            return Err("Water reflection needs the camera at least 12 units above the pool");
        }
        let mut mirrored = *camera;
        mirrored.position.z = 2.0 * pool.maxs.z - camera.position.z;
        mirrored.pitch_degrees = -camera.pitch_degrees;
        let eye = mirrored.position.as_dvec3();
        let forward = mirrored.forward().as_dvec3();
        let half = (f64::from(mirrored.fov_y_degrees).to_radians() * 0.5).tan();
        let right = mirrored.right().as_dvec3() * half * f64::from(aspect);
        let up = mirrored.up().as_dvec3() * half;
        let rays = [
            forward - right - up,
            forward - right + up,
            forward + right - up,
            forward + right + up,
        ];
        if !self
            .ceilings
            .iter()
            .any(|ceiling| ceiling.covers(eye, rays))
        {
            return Err("Water reflection needs one opaque ceiling covering the mirrored view");
        }
        Ok(mirrored)
    }
}

impl Ceiling {
    fn covers(self, eye: DVec3, rays: [DVec3; 4]) -> bool {
        rays.into_iter().all(|ray| {
            if ray.z <= 0.0 {
                return false;
            }
            let distance = (self.height - eye.z) / ray.z;
            if !(f64::from(NATIVE_NEAR) + CEILING_MARGIN..=f64::from(NATIVE_FAR) - CEILING_MARGIN)
                .contains(&distance)
            {
                return false;
            }
            let point = (eye + distance * ray).truncate();
            point.cmpge(self.mins + DVec2::splat(CEILING_MARGIN)).all()
                && point.cmple(self.maxs - DVec2::splat(CEILING_MARGIN)).all()
        })
    }

    fn from_triangles(
        first: [u32; 3],
        second: [u32; 3],
        vertices: &[Vertex],
        pool_height: f32,
    ) -> Option<Self> {
        if first[0] != second[0] || first[2] != second[1] {
            return None;
        }
        let vertices =
            [first[0], first[1], first[2], second[2]].map(|index| &vertices[index as usize]);
        let height = vertices[0].position[2];
        if height <= pool_height
            || vertices
                .iter()
                .any(|vertex| vertex.position[2] != height || vertex.normal != [0.0, 0.0, -1.0])
        {
            return None;
        }
        let points = vertices.map(|vertex| Vec3::from_array(vertex.position).as_dvec3().truncate());
        let mins = points[0].min(points[1]).min(points[2]).min(points[3]);
        let maxs = points[0].max(points[1]).max(points[2]).max(points[3]);
        if (maxs - mins).min_element() <= 2.0 * CEILING_MARGIN {
            return None;
        }
        let mut corners = 0_u8;
        for point in points {
            if !(point.x == mins.x || point.x == maxs.x)
                || !(point.y == mins.y || point.y == maxs.y)
            {
                return None;
            }
            let corner = usize::from(point.x == maxs.x) + 2 * usize::from(point.y == maxs.y);
            corners |= 1 << corner;
        }
        (corners == 0b1111).then_some(Self {
            height: f64::from(height),
            mins,
            maxs,
        })
    }
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
    ) -> Result<Captures, String> {
        if mesh.indices.len() / 3 > MAX_SCENE_TRIANGLES {
            return Err("Water preview is limited to scenes with at most 250,000 triangles".into());
        }
        crate::materials::water::verify_nodraw(fs)?;
        let mut refraction: Vec<Batch> = Vec::new();
        let asset = library
            .get(self.material)
            .water
            .ready()
            .ok_or("Water preview material was not retained")?;
        let mut reflection = asset.reflection.map(|_| Mirror::default());
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
            let triangles = mesh.indices[start..end].as_chunks::<3>().0;
            for (triangle_index, triangle) in triangles.iter().enumerate() {
                let vertices = triangle.map(|index| &mesh.vertices[index as usize]);
                let [a, b, c] = vertices.map(|vertex| Vec3::from(vertex.position));
                let mins = a.min(b).min(c);
                let maxs = a.max(b).max(c);
                let submerged = mins.cmple(self.maxs).all()
                    && maxs.cmpge(self.mins).all()
                    && intersects([a, b, c], &planes);
                let in_reflection_band = reflection.is_some() && maxs.z >= self.maxs.z - CLIP_BIAS;
                if in_reflection_band
                    && maxs.z < self.maxs.z
                    && submerged
                    && !(mins.cmpge(self.mins).all() && maxs.cmple(self.maxs).all())
                {
                    return Err(format!(
                        "Water reflection cannot classify a submerged shoreline face on {}",
                        info.name
                    ));
                }
                let reflected = in_reflection_band && (maxs.z >= self.maxs.z || !submerged);
                if !submerged && !reflected {
                    continue;
                }
                if vertices.iter().any(|vertex| {
                    vertex.position.iter().any(|&value| {
                        !value.is_finite()
                            || f64::from(value).abs() > map_core::checks::SOURCE_COORDINATE_LIMIT
                    }) || vertex
                        .uv
                        .iter()
                        .any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
                }) {
                    return Err(format!(
                        "Water capture needs finite Source positions and UVs within ±1000000 on {}",
                        info.name
                    ));
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
                        "Water {} needs plain opaque world or func_detail brush faces; {} is unsupported",
                        if reflected {
                            "reflection"
                        } else {
                            "refraction"
                        },
                        info.name,
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
                if submerged {
                    add_triangle(&mut refraction, *batch, first_index)?;
                }
                if let Some(mirror) = reflection.as_mut().filter(|_| reflected) {
                    add_triangle(&mut mirror.batches, *batch, first_index)?;
                    if let Some(&next) = triangles.get(triangle_index + 1)
                        && let Some(ceiling) =
                            Ceiling::from_triangles(*triangle, next, &mesh.vertices, self.maxs.z)
                    {
                        if mirror.ceilings.len() == MAX_CAPTURE_RANGES {
                            return Err("Water reflection exceeds 512 opaque ceiling faces".into());
                        }
                        mirror.ceilings.push(ceiling);
                    }
                }
            }
        }
        if refraction.is_empty() {
            return Err("Water refraction needs visible submerged brush geometry".into());
        }
        if reflection
            .as_ref()
            .is_some_and(|mirror| mirror.ceilings.is_empty())
        {
            return Err("Water reflection needs a horizontal opaque ceiling above the pool".into());
        }
        Ok(Captures {
            pool: self,
            refraction,
            reflection,
        })
    }
}

fn add_triangle(draws: &mut Vec<Batch>, batch: Batch, first_index: u32) -> Result<(), String> {
    if let Some(last) = draws.last_mut()
        && last.material == batch.material
        && last.first_index + last.index_count == first_index
    {
        last.index_count += 3;
    } else {
        if draws.len() == MAX_CAPTURE_RANGES {
            return Err("Water preview exceeds 512 draw ranges in a capture".into());
        }
        draws.push(Batch {
            first_index,
            index_count: 3,
            ..batch
        });
    }
    Ok(())
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
