use std::collections::BTreeMap;
use std::ffi::c_void;

use glam::DVec3;

use super::{Pointer, Shape, Vector};

pub(super) type Constructor = unsafe extern "C" fn(Pointer, *const c_void) -> Pointer;

#[repr(C)]
struct IndexedLine {
    vertices: [u16; 2],
}

#[repr(C)]
struct LineReference {
    line: u16,
    endpoint: u8,
    padding: u8,
}

#[repr(C)]
struct IndexedPolygon {
    first: u16,
    count: u16,
    normal: Vector,
}

#[repr(C)]
struct PolyhedronInput {
    vertices: *mut Vector,
    lines: *mut IndexedLine,
    indices: *mut LineReference,
    polygons: *mut IndexedPolygon,
    vertex_count: u16,
    line_count: u16,
    index_count: u16,
    polygon_count: u16,
}

const _: () = {
    assert!(size_of::<Vector>() == 12 && align_of::<Vector>() == 4);
    assert!(size_of::<IndexedLine>() == 4 && align_of::<IndexedLine>() == 2);
    assert!(size_of::<LineReference>() == 4 && align_of::<LineReference>() == 2);
    assert!(std::mem::offset_of!(LineReference, endpoint) == 2);
    assert!(size_of::<IndexedPolygon>() == 16 && align_of::<IndexedPolygon>() == 4);
    assert!(std::mem::offset_of!(IndexedPolygon, normal) == 4);
    assert!(size_of::<PolyhedronInput>() == 40 && align_of::<PolyhedronInput>() == 8);
    assert!(std::mem::offset_of!(PolyhedronInput, vertices) == 0);
    assert!(std::mem::offset_of!(PolyhedronInput, lines) == 8);
    assert!(std::mem::offset_of!(PolyhedronInput, indices) == 16);
    assert!(std::mem::offset_of!(PolyhedronInput, polygons) == 24);
    assert!(std::mem::offset_of!(PolyhedronInput, vertex_count) == 32);
    assert!(std::mem::offset_of!(PolyhedronInput, line_count) == 34);
    assert!(std::mem::offset_of!(PolyhedronInput, index_count) == 36);
    assert!(std::mem::offset_of!(PolyhedronInput, polygon_count) == 38);
};

unsafe extern "C" {
    fn hammerr_convex_from_polyhedron(
        construct: Constructor,
        collision: Pointer,
        input: *const PolyhedronInput,
        output: *mut Pointer,
    ) -> i32;
}

struct Edge {
    line: u16,
    endpoint: u8,
    paired: bool,
}

struct Input {
    vertices: Vec<Vector>,
    lines: Vec<IndexedLine>,
    indices: Vec<LineReference>,
    polygons: Vec<IndexedPolygon>,
}

fn count(value: usize) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| "Physics topology exceeds native 16-bit counts".into())
}

impl Input {
    fn new(shape: &Shape) -> Result<Self, String> {
        super::super::topology::validate(shape)?;
        let index_count = shape.faces.iter().try_fold(0_usize, |total, face| {
            total
                .checked_add(face.len())
                .ok_or("Physics topology count overflow")
        })?;
        count(index_count)?;
        count(shape.corners.len())?;
        count(shape.planes.len())?;
        let mut input = Self {
            vertices: shape.corners.iter().copied().map(Vector).collect(),
            lines: Vec::with_capacity(index_count / 2),
            indices: Vec::with_capacity(index_count),
            polygons: Vec::with_capacity(shape.planes.len()),
        };
        let mut edges = BTreeMap::<[u16; 2], Edge>::new();
        for (face, plane) in shape.faces.iter().zip(&shape.planes) {
            let normal = DVec3::new(
                f64::from(plane[0]),
                f64::from(plane[1]),
                f64::from(plane[2]),
            )
            .normalize();
            if !normal.is_finite() {
                return Err("Physics topology requires finite outward normals".into());
            }
            input.polygons.push(IndexedPolygon {
                first: count(input.indices.len())?,
                count: count(face.len())?,
                normal: Vector(normal.to_array().map(|value| value as f32)),
            });
            for index in 0..face.len() {
                let a = face[index];
                let b = face[(index + 1) % face.len()];
                let vertices = [a.min(b), a.max(b)];
                let endpoint = u8::from(a != vertices[0]);
                let line = if let Some(edge) = edges.get_mut(&vertices) {
                    if edge.paired || edge.endpoint == endpoint {
                        return Err("Physics topology edges require two opposite references".into());
                    }
                    edge.paired = true;
                    edge.line
                } else {
                    let line = count(input.lines.len())?;
                    input.lines.push(IndexedLine { vertices });
                    edges.insert(
                        vertices,
                        Edge {
                            line,
                            endpoint,
                            paired: false,
                        },
                    );
                    line
                };
                input.indices.push(LineReference {
                    line,
                    endpoint,
                    padding: 0,
                });
            }
        }
        if edges.values().any(|edge| !edge.paired)
            || input.lines.len().checked_mul(2) != Some(input.indices.len())
            || input.indices.len() != index_count
        {
            return Err("Physics topology has an open native edge".into());
        }
        Ok(input)
    }

    fn abi(&mut self) -> Result<PolyhedronInput, String> {
        Ok(PolyhedronInput {
            vertices: self.vertices.as_mut_ptr(),
            lines: self.lines.as_mut_ptr(),
            indices: self.indices.as_mut_ptr(),
            polygons: self.polygons.as_mut_ptr(),
            vertex_count: count(self.vertices.len())?,
            line_count: count(self.lines.len())?,
            index_count: count(self.indices.len())?,
            polygon_count: count(self.polygons.len())?,
        })
    }
}

pub(super) fn construct(
    construct: Constructor,
    collision: Pointer,
    shape: &Shape,
) -> Result<(Pointer, i32), String> {
    let mut input = Input::new(shape)?;
    let abi = input.abi()?;
    let mut output = std::ptr::null_mut();
    // The pinned slot 9 copies every array before the C++ object is destroyed.
    let status = unsafe { hammerr_convex_from_polyhedron(construct, collision, &abi, &mut output) };
    Ok((output, status))
}

pub(super) fn check_status(status: i32) -> Result<(), String> {
    match status {
        0 => Ok(()),
        1 => Err("Native physics polyhedron ABI layout does not match the verified build".into()),
        2 => Err("Native physics unexpectedly released its borrowed polyhedron input".into()),
        3 => Err("Native physics polyhedron construction raised a C++ exception".into()),
        _ => Err("Native physics polyhedron construction returned an invalid status".into()),
    }
}
