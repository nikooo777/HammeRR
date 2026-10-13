use std::io;

use bytemuck::{Pod, Zeroable};
use glam::DVec3;

use super::{Bsp, Edge, Face, Record, invalid, lump};

mod samples;
pub use samples::{DisplacementLightmapSample, DisplacementSurfaceSample};

pub const MAX_DISPLACEMENTS: usize = 2048;
pub const MAX_DISPLACEMENT_VERTICES: usize = MAX_DISPLACEMENTS * 289;
pub const MAX_DISPLACEMENT_TRIANGLES: usize = MAX_DISPLACEMENTS * 512;
pub const INVALID_DISPLACEMENT_NEIGHBOR: u16 = u16::MAX;
const MAX_AUXILIARY_BYTES: usize = 64 * 1024 * 1024;
const MAX_REFERENCE_BYTES: usize = 16 * 1024 * 1024;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DisplacementNeighbor {
    pub index: u16,
    pub orientation: u8,
    pub span: u8,
    pub neighbor_span: u8,
    pub padding: u8,
}

impl Default for DisplacementNeighbor {
    fn default() -> Self {
        Self {
            index: INVALID_DISPLACEMENT_NEIGHBOR,
            orientation: 0,
            span: 0,
            neighbor_span: 0,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct DisplacementCorner {
    pub neighbors: [u16; 4],
    pub count: u8,
    pub padding: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DisplacementInfo {
    pub start: [f32; 3],
    pub vertex_start: i32,
    pub triangle_start: i32,
    pub power: i32,
    pub min_tessellation: i32,
    pub smoothing_angle: f32,
    pub contents: i32,
    pub face: u16,
    pub padding: u16,
    pub lightmap_alpha_start: i32,
    pub lightmap_sample_start: i32,
    pub edges: [[DisplacementNeighbor; 2]; 4],
    pub corners: [DisplacementCorner; 4],
    pub allowed_vertices: [u32; 10],
}

impl Default for DisplacementInfo {
    fn default() -> Self {
        Self {
            start: [0.0; 3],
            vertex_start: 0,
            triangle_start: 0,
            power: 2,
            min_tessellation: 0,
            smoothing_angle: 0.0,
            contents: super::contents::SOLID,
            face: 0,
            padding: 0,
            lightmap_alpha_start: 0,
            lightmap_sample_start: 0,
            edges: [[DisplacementNeighbor::default(); 2]; 4],
            corners: [DisplacementCorner::default(); 4],
            allowed_vertices: [u32::MAX; 10],
        }
    }
}

impl DisplacementInfo {
    pub fn flags(self) -> Option<u32> {
        (self.min_tessellation < 0).then_some(self.min_tessellation as u32 & i32::MAX as u32)
    }

    pub fn grid_size(self) -> io::Result<usize> {
        if !(2..=4).contains(&self.power) {
            return Err(invalid("displacement power must be 2, 3 or 4"));
        }
        Ok((1 << self.power) + 1)
    }

    pub fn vertex_count(self) -> io::Result<usize> {
        let size = self.grid_size()?;
        Ok(size * size)
    }

    pub fn triangle_count(self) -> io::Result<usize> {
        let cells = self.grid_size()? - 1;
        Ok(cells * cells * 2)
    }

    pub fn triangle_indices(self) -> io::Result<Vec<[usize; 3]>> {
        self.ordered_triangles(false)
    }

    pub fn lightmap_triangle_indices(self) -> io::Result<Vec<[usize; 3]>> {
        self.ordered_triangles(true)
    }

    fn ordered_triangles(self, lightmap: bool) -> io::Result<Vec<[usize; 3]>> {
        let size = self.grid_size()?;
        let mut pending = vec![(0, 0, size - 1)];
        let mut triangles = Vec::with_capacity(self.triangle_count()?);
        while let Some((x, y, cells)) = pending.pop() {
            if cells > 2 {
                let half = cells / 2;
                let children = if lightmap {
                    [(half, 0), (0, 0), (0, half), (half, half)]
                } else {
                    [(half, half), (0, half), (half, 0), (0, 0)]
                };
                for (dx, dy) in children {
                    pending.push((x + dx, y + dy, half));
                }
            } else {
                let center = (y + 1) * size + x + 1;
                let ring = if lightmap {
                    [
                        (2, 0),
                        (1, 0),
                        (0, 0),
                        (0, 1),
                        (0, 2),
                        (1, 2),
                        (2, 2),
                        (2, 1),
                        (2, 0),
                    ]
                } else {
                    [
                        (0, 1),
                        (0, 0),
                        (1, 0),
                        (2, 0),
                        (2, 1),
                        (2, 2),
                        (1, 2),
                        (0, 2),
                        (0, 1),
                    ]
                }
                .map(|(dx, dy)| (y + dy) * size + x + dx);
                for pair in ring.windows(2) {
                    triangles.push(if lightmap {
                        [pair[0], pair[1], center]
                    } else {
                        [pair[1], pair[0], center]
                    });
                }
            }
        }
        Ok(triangles)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct DisplacementVertex {
    pub direction: [f32; 3],
    pub distance: f32,
    pub alpha: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct DisplacementTriangle {
    pub tags: u16,
}

impl Record for DisplacementInfo {
    const LUMP: usize = lump::DISPINFO;
}
impl Record for DisplacementVertex {
    const LUMP: usize = lump::DISP_VERTS;
}
impl Record for DisplacementTriangle {
    const LUMP: usize = lump::DISP_TRIS;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Displacements {
    pub infos: Vec<DisplacementInfo>,
    pub vertices: Vec<DisplacementVertex>,
    pub triangles: Vec<DisplacementTriangle>,
}

fn range(start: i32, count: usize, maximum: usize) -> io::Result<std::ops::Range<usize>> {
    let start =
        usize::try_from(start).map_err(|_| invalid("negative displacement record offset"))?;
    let end = start
        .checked_add(count)
        .filter(|&end| end <= maximum)
        .ok_or_else(|| invalid("displacement record range is invalid"))?;
    Ok(start..end)
}

fn auxiliary(bsp: &Bsp, index: usize, offset: i32) -> io::Result<()> {
    let data = &bsp.lumps[index];
    if data.version != 0 || data.fourcc != [0; 4] || data.data.len() > MAX_AUXILIARY_BYTES {
        return Err(invalid(
            "unsupported displacement auxiliary version/compression/size",
        ));
    }
    if data.data.is_empty() {
        if offset != 0 && offset != -1 {
            return Err(invalid("displacement auxiliary offset needs data"));
        }
    } else if offset < 0 || offset as usize >= data.data.len() {
        return Err(invalid("displacement auxiliary offset is out of range"));
    }
    Ok(())
}

fn references<T: Pod>(bsp: &Bsp, index: usize) -> io::Result<Vec<T>> {
    if bsp.lumps[index].data.len() > MAX_REFERENCE_BYTES {
        return Err(invalid(
            "displacement geometry references exceed supported bounds",
        ));
    }
    bsp.values(index)
}

fn faces(bsp: &Bsp) -> io::Result<Vec<Face>> {
    let index = if bsp.lumps[lump::FACES].data.is_empty() {
        lump::FACES_HDR
    } else {
        lump::FACES
    };
    references(bsp, index)
}

impl Displacements {
    pub fn validate(&self, bsp: &Bsp) -> io::Result<()> {
        if self.infos.len() > MAX_DISPLACEMENTS
            || self.vertices.len() > MAX_DISPLACEMENT_VERTICES
            || self.triangles.len() > MAX_DISPLACEMENT_TRIANGLES
        {
            return Err(invalid(
                "displacement record counts exceed supported bounds",
            ));
        }
        let faces = faces(bsp)?;
        let mut sample_work = 0usize;
        for (index, info) in self.infos.iter().enumerate() {
            let face = faces
                .get(usize::from(info.face))
                .ok_or_else(|| invalid("displacement base face is missing"))?;
            if face.dispinfo != index as i16 || face.edge_count != 4 {
                return Err(invalid(
                    "displacement base face reference or quad topology is invalid",
                ));
            }
            range(info.vertex_start, info.vertex_count()?, self.vertices.len())?;
            range(
                info.triangle_start,
                info.triangle_count()?,
                self.triangles.len(),
            )?;
            if info.start.iter().any(|v| !v.is_finite()) || !info.smoothing_angle.is_finite() {
                return Err(invalid(
                    "displacement start and smoothing angle must be finite",
                ));
            }
            for neighbor in info
                .edges
                .iter()
                .flatten()
                .filter(|neighbor| neighbor.index != INVALID_DISPLACEMENT_NEIGHBOR)
            {
                if usize::from(neighbor.index) >= self.infos.len()
                    || neighbor.orientation > 3
                    || neighbor.span > 2
                    || neighbor.neighbor_span > 2
                {
                    return Err(invalid(
                        "displacement edge neighbor reference or orientation/span is invalid",
                    ));
                }
            }
            for corner in info.corners {
                if corner.count > 4
                    || corner.neighbors[..usize::from(corner.count)]
                        .iter()
                        .any(|&index| usize::from(index) >= self.infos.len())
                {
                    return Err(invalid(
                        "displacement corner neighbor count or reference is invalid",
                    ));
                }
            }
            auxiliary(bsp, lump::DISP_LIGHTMAP_ALPHAS, info.lightmap_alpha_start)?;
            auxiliary(
                bsp,
                lump::DISP_LIGHTMAP_SAMPLE_POSITIONS,
                info.lightmap_sample_start,
            )?;
            if let Some(mut samples) = samples::stream(bsp, *info, face)? {
                sample_work = sample_work
                    .checked_add(samples.count())
                    .filter(|&work| work <= MAX_AUXILIARY_BYTES / 4)
                    .ok_or_else(|| {
                        invalid("displacement sample validation exceeds its work limit")
                    })?;
                while samples.remaining() != 0 {
                    samples.read()?;
                }
            }
        }
        if self.vertices.iter().any(|vertex| {
            vertex
                .direction
                .into_iter()
                .chain([vertex.distance, vertex.alpha])
                .any(|v| !v.is_finite())
        }) {
            return Err(invalid("displacement vertex fields must be finite"));
        }
        for face in faces {
            if face.dispinfo < -1
                || face.dispinfo >= 0 && face.dispinfo as usize >= self.infos.len()
            {
                return Err(invalid("face refers to a missing displacement"));
            }
        }
        Ok(())
    }

    pub fn positions(&self, index: usize, quad: [[f32; 3]; 4]) -> io::Result<Vec<[f32; 3]>> {
        let info = *self
            .infos
            .get(index)
            .ok_or_else(|| invalid("displacement index is invalid"))?;
        let size = info.grid_size()?;
        let vertices = &self.vertices[range(info.vertex_start, size * size, self.vertices.len())?];
        let quad = quad.map(|point| DVec3::from_array(point.map(f64::from)));
        if quad.iter().any(|point| !point.is_finite()) {
            return Err(invalid("displacement base quad must be finite"));
        }
        let start = DVec3::from_array(info.start.map(f64::from));
        if !start.is_finite() {
            return Err(invalid("displacement start must be finite"));
        }
        let first = (0..4)
            .min_by(|&a, &b| {
                quad[a]
                    .distance_squared(start)
                    .total_cmp(&quad[b].distance_squared(start))
            })
            .unwrap();
        let [a, b, c, d] = [0, 1, 2, 3].map(|offset| quad[(first + offset) % 4]);
        let mut result = Vec::with_capacity(size * size);
        for row in 0..size {
            let t = row as f64 / (size - 1) as f64;
            let left = a.lerp(b, t);
            let right = d.lerp(c, t);
            for col in 0..size {
                let vertex = vertices[row * size + col];
                let point = left.lerp(right, col as f64 / (size - 1) as f64)
                    + DVec3::from_array(vertex.direction.map(f64::from))
                        * f64::from(vertex.distance);
                let point = point.to_array().map(|v| v as f32);
                if point.iter().any(|v| !v.is_finite()) {
                    return Err(invalid(
                        "displacement position exceeds finite native floats",
                    ));
                }
                result.push(point);
            }
        }
        Ok(result)
    }
}

fn checked_records<T: Record>(bsp: &Bsp, maximum: usize) -> io::Result<Vec<T>> {
    let data = &bsp.lumps[T::LUMP];
    if data.version != 0 || data.fourcc != [0; 4] || data.data.len() > maximum * size_of::<T>() {
        return Err(invalid(
            "unsupported displacement record version/compression/size",
        ));
    }
    bsp.records()
}

impl Bsp {
    pub fn displacement_quad(&self, index: usize) -> io::Result<[[f32; 3]; 4]> {
        let infos = checked_records::<DisplacementInfo>(self, MAX_DISPLACEMENTS)?;
        let info = infos
            .get(index)
            .ok_or_else(|| invalid("displacement index is invalid"))?;
        let faces = faces(self)?;
        let face = faces
            .get(usize::from(info.face))
            .ok_or_else(|| invalid("displacement base face is missing"))?;
        if face.edge_count != 4 || face.dispinfo != index as i16 {
            return Err(invalid("displacement base face must be a quad"));
        }
        let surfedges = references::<i32>(self, lump::SURFEDGES)?;
        let edges = references::<Edge>(self, lump::EDGES)?;
        let vertices = references::<[f32; 3]>(self, lump::VERTEXES)?;
        let face_edges = &surfedges[range(face.first_edge, 4, surfedges.len())?];
        let mut corners = [0; 4];
        let mut ends = [0; 4];
        for (corner, &index) in face_edges.iter().enumerate() {
            let edge_index = index
                .checked_abs()
                .ok_or_else(|| invalid("invalid signed displacement edge"))?
                as usize;
            let edge = edges
                .get(edge_index)
                .ok_or_else(|| invalid("displacement edge is missing"))?;
            corners[corner] = edge.0[usize::from(index < 0)];
            ends[corner] = edge.0[usize::from(index >= 0)];
        }
        let mut quad = [[0.0; 3]; 4];
        for corner in 0..4 {
            if ends[corner] != corners[(corner + 1) % 4] {
                return Err(invalid("displacement face edges do not form a closed quad"));
            }
            quad[corner] = *vertices
                .get(usize::from(corners[corner]))
                .ok_or_else(|| invalid("displacement base vertex is missing"))?;
            if quad[corner].iter().any(|v| !v.is_finite()) {
                return Err(invalid("displacement base vertices must be finite"));
            }
        }
        Ok(quad)
    }

    pub fn displacements(&self) -> io::Result<Option<Displacements>> {
        let infos = checked_records::<DisplacementInfo>(self, MAX_DISPLACEMENTS)?;
        let vertices = checked_records::<DisplacementVertex>(self, MAX_DISPLACEMENT_VERTICES)?;
        let triangles = checked_records::<DisplacementTriangle>(self, MAX_DISPLACEMENT_TRIANGLES)?;
        if infos.is_empty() && vertices.is_empty() && triangles.is_empty() {
            return Ok(None);
        }
        let data = Displacements {
            infos,
            vertices,
            triangles,
        };
        data.validate(self)?;
        Ok(Some(data))
    }

    pub fn set_displacements(&mut self, data: &Displacements) -> io::Result<()> {
        data.validate(self)?;
        self.set_records(&data.infos);
        self.set_records(&data.vertices);
        self.set_records(&data.triangles);
        Ok(())
    }
}
