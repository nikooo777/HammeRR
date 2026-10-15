use std::io;

use super::{
    MAX_BODY_PARTS, MAX_INDICES, MAX_MESHES, MAX_MODELS, MAX_VERTICES, Mesh, Part, invalid,
    reader::Reader,
};

const MAX_GROUPS: usize = 65_536;

pub struct Geometry {
    pub parts: Vec<Part>,
    pub models: Vec<Vec<std::ops::Range<usize>>>,
}

pub fn parse(data: &[u8], checksum: i32, meshes: &[Vec<Vec<Mesh>>]) -> io::Result<Geometry> {
    let reader = Reader::new(data, 36)?;
    if reader.int(0)? != 7 || reader.int(16)? != checksum {
        return Err(invalid("VTX version or model checksum does not match"));
    }
    if reader.count(20, 8)? == 0 {
        return Err(invalid("VTX requires at least one LOD"));
    }
    let bodies = reader.table(0, 28, 32, 8, MAX_BODY_PARTS)?;
    if bodies.len != meshes.len() {
        return Err(invalid("VTX body parts do not match MDL"));
    }
    let mut parts = Vec::new();
    let mut work = 0;
    let mut total_indices = 0;
    let mut model_ranges = Vec::with_capacity(bodies.len);
    for (body, body_meshes) in meshes.iter().enumerate() {
        let models = reader.table(bodies.at(body)?, 0, 4, 8, MAX_MODELS)?;
        if models.len != body_meshes.len() {
            return Err(invalid("VTX body models do not match MDL"));
        }
        let mut ranges = Vec::with_capacity(models.len);
        for (model_index, model_meshes) in body_meshes.iter().enumerate() {
            let first = parts.len();
            let lods = reader.table(models.at(model_index)?, 0, 4, 12, 8)?;
            if lods.len == 0 && model_meshes.is_empty() {
                ranges.push(first..first);
                continue;
            }
            let vtx_meshes = reader.table(lods.at(0)?, 0, 4, 9, MAX_MESHES)?;
            if vtx_meshes.len != model_meshes.len() {
                return Err(invalid("VTX LOD0 meshes do not match MDL"));
            }
            for (index, mesh) in model_meshes.iter().enumerate() {
                let groups = reader.table(vtx_meshes.at(index)?, 0, 4, 25, MAX_GROUPS)?;
                let mut output = Vec::new();
                for group in 0..groups.len {
                    let base = groups.at(group)?;
                    let vertices = reader.table(base, 0, 4, 9, MAX_VERTICES)?;
                    let indices = reader.table(base, 8, 12, 2, MAX_INDICES)?;
                    let strips = reader.table(base, 16, 20, 27, MAX_GROUPS)?;
                    work += 1 + vertices.len + indices.len + strips.len;
                    if work > MAX_VERTICES + MAX_INDICES {
                        return Err(invalid("VTX traversal exceeds its work limit"));
                    }
                    let mut mapped = Vec::with_capacity(vertices.len);
                    for vertex in 0..vertices.len {
                        let original = usize::from(reader.short(vertices.at(vertex)? + 4)?);
                        if original >= mesh.count {
                            return Err(invalid("VTX vertex is outside its MDL mesh"));
                        }
                        mapped.push((mesh.first + original) as u32);
                    }
                    for strip in 0..strips.len {
                        let strip = strips.at(strip)?;
                        let count = reader.count(strip, MAX_INDICES)?;
                        let first = reader.count(strip + 4, indices.len)?;
                        if first + count > indices.len {
                            return Err(invalid("VTX strip index range exceeds its group"));
                        }
                        let vertex_count = reader.count(strip + 8, vertices.len)?;
                        let vertex_start = reader.count(strip + 12, vertices.len)?;
                        if vertex_start + vertex_count > vertices.len {
                            return Err(invalid("VTX strip vertex range exceeds its group"));
                        }
                        reader.table(strip, 19, 23, 8, 512)?;
                        let mode = reader.byte(strip + 18)? & 3;
                        if mode != 1 && mode != 2 {
                            return Err(invalid("VTX strip must use a triangle list or strip"));
                        }
                        if mode == 1 && !count.is_multiple_of(3) {
                            return Err(invalid(
                                "VTX triangle-list index count must be divisible by three",
                            ));
                        }
                        let triangle_count = if mode == 1 {
                            count / 3
                        } else {
                            count.saturating_sub(2)
                        };
                        work += triangle_count;
                        if work > MAX_VERTICES + MAX_INDICES {
                            return Err(invalid("VTX traversal exceeds its work limit"));
                        }
                        if total_indices + triangle_count * 3 > MAX_INDICES {
                            return Err(invalid("VTX triangles exceed their resource limit"));
                        }
                        for triangle in 0..triangle_count {
                            let start = first + if mode == 1 { triangle * 3 } else { triangle };
                            let mut corners = [0; 3];
                            for (offset, corner) in corners.iter_mut().enumerate() {
                                let index = usize::from(reader.short(indices.at(start + offset)?)?);
                                *corner = *mapped.get(index).ok_or_else(|| {
                                    invalid("VTX index is outside its group vertices")
                                })?;
                            }
                            if mode == 2 && triangle % 2 != 0 {
                                corners.swap(0, 1);
                            }
                            if corners[0] == corners[1]
                                || corners[1] == corners[2]
                                || corners[2] == corners[0]
                            {
                                continue;
                            }
                            output.extend(corners);
                            total_indices += 3;
                        }
                    }
                }
                parts.push(Part {
                    material: mesh.material,
                    indices: output,
                });
            }
            ranges.push(first..parts.len());
        }
        model_ranges.push(ranges);
    }
    Ok(Geometry {
        parts,
        models: model_ranges,
    })
}
