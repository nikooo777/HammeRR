use std::io;
use std::ops::Range;

use bytemuck::Pod;
use glam::DVec3;

use super::{
    Bsp, DisplacementInfo, Displacements, Edge, Face, LightingKind, Model, Plane, invalid, lump,
};

const FACE_PLANE_EPSILON: f64 = 0.02;
const NORMAL_LENGTH_EPSILON: f64 = 0.001;
const SOURCE_COORDINATE_LIMIT: f32 = 16_384.0;
const MAX_REFERENCE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct FaceWinding {
    pub model: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub normal: [f32; 3],
}

#[derive(Debug)]
pub struct FaceTerrain {
    pub model: usize,
    pub info: DisplacementInfo,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[usize; 3]>,
    pub grid_uvs: Vec<[f32; 2]>,
    pub alphas: Vec<f32>,
}

pub struct FaceGeometry {
    faces: Vec<Face>,
    models: Vec<Model>,
    face_models: Vec<usize>,
    planes: Vec<Plane>,
    surfedges: Vec<i32>,
    edges: Vec<Edge>,
    vertices: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    normal_indices: Vec<u16>,
    displacements: Option<Displacements>,
}

pub fn checked_model_face_range(model: &Model, faces: usize) -> io::Result<Range<usize>> {
    let first = usize::try_from(model.first_face)
        .map_err(|_| invalid("Compiled model has a negative first face"))?;
    let count = usize::try_from(model.face_count)
        .map_err(|_| invalid("Compiled model has a negative face count"))?;
    let end = first
        .checked_add(count)
        .filter(|&end| end <= faces)
        .ok_or_else(|| invalid("Compiled model face range exceeds this channel's face table"))?;
    Ok(first..end)
}

fn references<T: Pod>(bsp: &Bsp, index: usize) -> io::Result<Vec<T>> {
    let data = bsp.lump(index);
    if data.version != 0 || data.fourcc != [0; 4] || data.data.len() > MAX_REFERENCE_BYTES {
        return Err(invalid(
            "Unsupported compiled face geometry metadata or size",
        ));
    }
    bsp.values(index)
}

fn same_terrain_face(selected: &Face, shared: &Face) -> bool {
    selected.plane == shared.plane
        && selected.side == shared.side
        && selected.first_edge == shared.first_edge
        && selected.edge_count == shared.edge_count
        && selected.texinfo == shared.texinfo
        && selected.dispinfo == shared.dispinfo
        && selected.area.to_bits() == shared.area.to_bits()
        && selected.lightmap_mins == shared.lightmap_mins
        && selected.lightmap_size == shared.lightmap_size
}

fn point(value: [f32; 3]) -> io::Result<DVec3> {
    let point = DVec3::from_array(value.map(f64::from));
    if !point.is_finite() || point.abs().max_element() > f64::from(SOURCE_COORDINATE_LIMIT) {
        return Err(invalid(
            "Compiled face vertices must be finite and inside Source bounds",
        ));
    }
    Ok(point)
}

fn normal(value: [f32; 3]) -> io::Result<DVec3> {
    let normal = DVec3::from_array(value.map(f64::from));
    if !normal.is_finite() || (normal.length_squared() - 1.0).abs() > NORMAL_LENGTH_EPSILON {
        return Err(invalid("Compiled face has an invalid native normal"));
    }
    Ok(normal)
}

impl FaceGeometry {
    pub fn new(bsp: &Bsp, kind: LightingKind) -> io::Result<Self> {
        Self::new_controlled(bsp, kind, &mut || Ok(()))
    }

    pub fn new_controlled(
        bsp: &Bsp,
        kind: LightingKind,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        check()?;
        let faces = bsp.lighting_faces(kind)?;
        check()?;
        let models = references::<Model>(bsp, lump::MODELS)?;
        let world = models
            .first()
            .ok_or_else(|| invalid("Compiled Model0 is unavailable"))?;
        if world.origin != [0.0; 3] {
            return Err(invalid("Compiled Model0 has a nonzero or invalid origin"));
        }
        let mut face_models = vec![usize::MAX; faces.len()];
        for (index, model) in models.iter().enumerate() {
            check()?;
            for face in checked_model_face_range(model, faces.len())? {
                if face_models[face] != usize::MAX {
                    return Err(invalid("Compiled face belongs to overlapping model ranges"));
                }
                face_models[face] = index;
            }
        }
        check()?;
        let planes = references(bsp, lump::PLANES)?;
        check()?;
        let surfedges = references::<i32>(bsp, lump::SURFEDGES)?;
        check()?;
        let edges = references(bsp, lump::EDGES)?;
        check()?;
        let vertices = references(bsp, lump::VERTEXES)?;
        check()?;
        let normals = references(bsp, lump::VERTNORMALS)?;
        check()?;
        let normal_indices = references::<u16>(bsp, lump::VERTNORMALINDICES)?;
        check()?;
        let displacements = bsp.displacements()?;
        check()?;
        if displacements.is_some() {
            let shared_kind = if bsp.lump(lump::FACES).data.is_empty() {
                LightingKind::Hdr
            } else {
                LightingKind::Ldr
            };
            let shared = bsp.lighting_faces(shared_kind)?;
            if shared.len() != faces.len() {
                return Err(invalid(
                    "Selected terrain channel has a different face layout",
                ));
            }
            for (selected, shared) in faces.iter().zip(shared) {
                check()?;
                if (selected.dispinfo >= 0 || shared.dispinfo >= 0)
                    && !same_terrain_face(selected, &shared)
                {
                    return Err(invalid(
                        "Selected terrain channel disagrees with shared displacement geometry or lightmap layout",
                    ));
                }
            }
        }
        for face in &faces {
            check()?;
            if face.dispinfo < -1
                || face.dispinfo >= 0
                    && displacements
                        .as_ref()
                        .is_none_or(|data| face.dispinfo as usize >= data.infos.len())
            {
                return Err(invalid("Compiled face has an invalid displacement index"));
            }
        }
        Ok(Self {
            faces,
            models,
            face_models,
            planes,
            surfedges,
            edges,
            vertices,
            normals,
            normal_indices,
            displacements,
        })
    }

    pub fn faces(&self) -> &[Face] {
        &self.faces
    }

    pub fn models(&self) -> &[Model] {
        &self.models
    }

    pub fn retained_bytes(&self) -> usize {
        self.faces.len() * size_of::<Face>()
            + self.models.len() * size_of::<Model>()
            + self.face_models.len() * size_of::<usize>()
            + self.planes.len() * size_of::<Plane>()
            + self.surfedges.len() * size_of::<i32>()
            + self.edges.len() * size_of::<Edge>()
            + self.vertices.len() * size_of::<[f32; 3]>()
            + self.normals.len() * size_of::<[f32; 3]>()
            + self.normal_indices.len() * size_of::<u16>()
            + self.displacements.as_ref().map_or(0, |data| {
                data.infos.len() * size_of::<DisplacementInfo>()
                    + data.vertices.len() * size_of::<super::DisplacementVertex>()
                    + data.triangles.len() * size_of::<super::DisplacementTriangle>()
            })
    }

    pub fn face(&self, ordinal: usize) -> io::Result<FaceWinding> {
        self.winding(ordinal, true)
    }

    pub fn positions(&self, ordinal: usize) -> io::Result<(usize, Vec<[f32; 3]>)> {
        if let Some(terrain) = self.terrain(ordinal)? {
            return Ok((terrain.model, terrain.positions));
        }
        let winding = self.winding(ordinal, false)?;
        Ok((winding.model, winding.positions))
    }

    fn winding(&self, ordinal: usize, with_normals: bool) -> io::Result<FaceWinding> {
        let face = self
            .faces
            .get(ordinal)
            .ok_or_else(|| invalid("Compiled face index exceeds this channel's face count"))?;
        if self.face_models[ordinal] == usize::MAX {
            return Err(invalid("Compiled face is not covered by a model range"));
        }
        let plane = self
            .planes
            .get(usize::from(face.plane))
            .ok_or_else(|| invalid("Compiled face plane is out of bounds"))?;
        let plane_normal = normal(plane.normal)?;
        if !plane.dist.is_finite() || face.side > 1 || !face.area.is_finite() || face.area <= 0.0 {
            return Err(invalid("Compiled face has an invalid plane, side or area"));
        }
        let first = usize::try_from(face.first_edge)
            .map_err(|_| invalid("Compiled face has a negative first surfedge"))?;
        let count = usize::try_from(face.edge_count)
            .ok()
            .filter(|&count| count >= 3)
            .ok_or_else(|| invalid("Compiled face requires at least three surfedges"))?;
        let end = first
            .checked_add(count)
            .ok_or_else(|| invalid("Compiled face surfedge range overflow"))?;
        let ring = self
            .surfedges
            .get(first..end)
            .ok_or_else(|| invalid("Compiled face surfedge range is out of bounds"))?;
        let mut first_vertex = None;
        let mut previous_end = None;
        let mut positions = Vec::with_capacity(count);
        let mut normals = Vec::with_capacity(count);
        let mut area = DVec3::ZERO;
        if with_normals && self.normal_indices.len() != self.surfedges.len() {
            return Err(invalid(
                "Compiled native normal indices do not match surfedges",
            ));
        }
        for (corner, &surfedge) in ring.iter().enumerate() {
            let index = usize::try_from(surfedge.unsigned_abs())
                .map_err(|_| invalid("Compiled edge index is too large"))?;
            let edge = self
                .edges
                .get(index)
                .ok_or_else(|| invalid("Compiled surfedge references an out-of-range edge"))?
                .0;
            let [start, end] = if surfedge < 0 {
                [edge[1], edge[0]]
            } else {
                edge
            };
            if start == end || previous_end.is_some_and(|previous| previous != start) {
                return Err(invalid(
                    "Compiled face surfedges do not form a connected nonzero edge chain",
                ));
            }
            let mut points = [DVec3::ZERO; 2];
            for (position, index) in points.iter_mut().zip([start, end]) {
                *position =
                    point(*self.vertices.get(usize::from(index)).ok_or_else(|| {
                        invalid("Compiled edge references an out-of-range vertex")
                    })?)?;
                if (plane_normal.dot(*position) - f64::from(plane.dist)).abs() > FACE_PLANE_EPSILON
                {
                    return Err(invalid(
                        "Compiled face vertices do not lie on its flat plane",
                    ));
                }
            }
            if points[0] == points[1] {
                return Err(invalid("Compiled face contains a zero-length edge"));
            }
            if with_normals {
                let index = self.normal_indices[first + corner];
                let corner_normal = *self
                    .normals
                    .get(usize::from(index))
                    .ok_or_else(|| invalid("Compiled corner normal index is out of bounds"))?;
                normal(corner_normal)?;
                normals.push(corner_normal);
            }
            first_vertex.get_or_insert(start);
            previous_end = Some(end);
            positions.push(points[0].as_vec3().to_array());
            area += points[0].cross(points[1]);
        }
        if first_vertex != previous_end || area.length_squared() == 0.0 {
            return Err(invalid(
                "Compiled face requires a closed nondegenerate edge chain",
            ));
        }
        // Emitted faces can already reference the oriented odd plane in a pair.
        let outward = if area.dot(plane_normal) > 0.0 {
            (-plane_normal).as_vec3().to_array()
        } else {
            plane.normal
        };
        Ok(FaceWinding {
            model: self.face_models[ordinal],
            positions,
            normals,
            normal: outward,
        })
    }

    pub fn terrain(&self, ordinal: usize) -> io::Result<Option<FaceTerrain>> {
        let winding = self.winding(ordinal, false)?;
        let face = &self.faces[ordinal];
        if face.dispinfo == -1 {
            return Ok(None);
        }
        let index = face.dispinfo as usize;
        let data = self
            .displacements
            .as_ref()
            .ok_or_else(|| invalid("Compiled displacement data is missing"))?;
        let info = *data
            .infos
            .get(index)
            .ok_or_else(|| invalid("Compiled displacement index is out of bounds"))?;
        if usize::from(info.face) != ordinal {
            return Err(invalid("Compiled displacement references a different face"));
        }
        let quad = winding
            .positions
            .try_into()
            .map_err(|_| invalid("Compiled displacement base face must be a quad"))?;
        let positions = data.positions(index, quad)?;
        for &position in &positions {
            point(position)?;
        }
        let normals = info.vertex_normals(
            &positions
                .iter()
                .map(|position| position.map(f64::from))
                .collect::<Vec<_>>(),
        )?;
        let normals = normals
            .into_iter()
            .map(|value| {
                let value = value.map(|component| component as f32);
                normal(value)?;
                Ok(value)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let triangles = info.allowed_triangle_indices()?;
        let size = info.grid_size()?;
        let grid_uvs = (0..positions.len())
            .map(|index| {
                [
                    (index % size) as f32 / (size - 1) as f32,
                    (index / size) as f32 / (size - 1) as f32,
                ]
            })
            .collect();
        let first = usize::try_from(info.vertex_start)
            .map_err(|_| invalid("Compiled displacement has a negative first vertex"))?;
        let end = first
            .checked_add(positions.len())
            .ok_or_else(|| invalid("Compiled displacement vertex range overflows"))?;
        let vertices = data
            .vertices
            .get(first..end)
            .ok_or_else(|| invalid("Compiled displacement vertices are out of bounds"))?;
        let alphas = vertices
            .iter()
            .map(|vertex| {
                if !(0.0..=255.0).contains(&vertex.alpha) {
                    return Err(invalid("Compiled displacement alpha is outside [0,255]"));
                }
                Ok(vertex.alpha)
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Some(FaceTerrain {
            model: winding.model,
            info,
            positions,
            normals,
            triangles,
            grid_uvs,
            alphas,
        }))
    }
}
