use std::io;

use super::{
    HardwareLayout, HardwareSection, MAX_BODY_PARTS, MAX_INDICES, MAX_LODS, MAX_MESHES, MAX_MODELS,
    MAX_VERTICES, Mesh, Part, hardware::Budget, invalid, reader::Reader,
};

const MAX_GROUPS: usize = 65_536;
const HARDWARE_SKINNED: u8 = 2;

pub struct Geometry {
    pub parts: Vec<Part>,
    pub models: Vec<Vec<std::ops::Range<usize>>>,
    pub hardware: Option<HardwareLayout>,
}

struct Traversal<'a, 'b, 'c> {
    reader: Reader<'a>,
    budget: &'b mut Budget<'c>,
    work: usize,
    total_indices: usize,
}

impl Traversal<'_, '_, '_> {
    fn consume(&mut self, work: usize) -> io::Result<()> {
        self.work = self
            .work
            .checked_add(work)
            .ok_or_else(|| invalid("VTX traversal work count overflow"))?;
        let maximum = if self.budget.enabled() {
            super::MAX_HARDWARE_WORK
        } else {
            MAX_VERTICES + MAX_INDICES
        };
        if self.work > maximum {
            return Err(invalid("VTX traversal exceeds its work limit"));
        }
        self.budget.charge(0, work)
    }

    fn group(
        &mut self,
        base: usize,
        mesh: &Mesh,
        lod: usize,
        mut output: Option<&mut Vec<u32>>,
        hardware: &mut Option<HardwareLayout>,
    ) -> io::Result<(std::ops::Range<usize>, Vec<u32>)> {
        let vertices = self.reader.table(base, 0, 4, 9, MAX_VERTICES)?;
        let indices = self.reader.table(base, 8, 12, 2, MAX_INDICES)?;
        let strips = self.reader.table(base, 16, 20, 27, MAX_GROUPS)?;
        self.consume(1 + vertices.len + indices.len + strips.len)?;
        if self.budget.enabled() && self.reader.byte(base + 24)? != HARDWARE_SKINNED {
            return Err(invalid(
                "VTX hardware layout requires ordinary hardware-skinned groups",
            ));
        }
        self.budget.group(vertices.len)?;
        self.budget.array::<u32>(vertices.len)?;
        let mut mapped = Vec::with_capacity(vertices.len);
        for vertex in 0..vertices.len {
            self.budget.checkpoint(vertex)?;
            let original = usize::from(self.reader.short(vertices.at(vertex)? + 4)?);
            if original >= mesh.count || (self.budget.enabled() && original >= mesh.lod_counts[lod])
            {
                return Err(invalid("VTX vertex is outside its MDL mesh"));
            }
            mapped.push((mesh.first + original) as u32);
        }
        if self.budget.enabled() {
            for index in 0..indices.len {
                self.budget.checkpoint(index)?;
                if usize::from(self.reader.short(indices.at(index)?)?) >= vertices.len {
                    return Err(invalid("VTX index is outside its group vertices"));
                }
            }
        }
        let mut local_triangles = Vec::new();
        for strip in 0..strips.len {
            self.budget.checkpoint(strip)?;
            let strip = strips.at(strip)?;
            let count = self.reader.count(strip, MAX_INDICES)?;
            let first = self.reader.count(strip + 4, indices.len)?;
            if first.checked_add(count).is_none_or(|end| end > indices.len) {
                return Err(invalid("VTX strip index range exceeds its group"));
            }
            let vertex_count = self.reader.count(strip + 8, vertices.len)?;
            let vertex_start = self.reader.count(strip + 12, vertices.len)?;
            if vertex_start
                .checked_add(vertex_count)
                .is_none_or(|end| end > vertices.len)
            {
                return Err(invalid("VTX strip vertex range exceeds its group"));
            }
            let changes = self.reader.table(strip, 19, 23, 8, 512)?;
            if self.budget.enabled() {
                self.consume(changes.len)?;
            }
            let flags = self.reader.byte(strip + 18)?;
            let mode = flags & 3;
            if (mode != 1 && mode != 2) || (self.budget.enabled() && flags != mode) {
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
            self.consume(triangle_count)?;
            if output.is_none() && !self.budget.enabled() {
                continue;
            }
            let triangle_indices = triangle_count
                .checked_mul(3)
                .ok_or_else(|| invalid("VTX triangle index count overflow"))?;
            if output.is_some()
                && self
                    .total_indices
                    .checked_add(triangle_indices)
                    .is_none_or(|n| n > MAX_INDICES)
            {
                return Err(invalid("VTX triangles exceed their resource limit"));
            }
            if self.budget.enabled() {
                self.budget
                    .reserve(&mut local_triangles, triangle_indices)?;
                if let Some(output) = output.as_deref_mut() {
                    self.budget.reserve(output, triangle_indices)?;
                }
            }
            for triangle in 0..triangle_count {
                self.budget.checkpoint(triangle)?;
                let start = first + if mode == 1 { triangle * 3 } else { triangle };
                let mut corners = [0; 3];
                for (offset, corner) in corners.iter_mut().enumerate() {
                    let index = usize::from(self.reader.short(indices.at(start + offset)?)?);
                    if index >= mapped.len() {
                        return Err(invalid("VTX index is outside its group vertices"));
                    }
                    *corner = index as u32;
                }
                if mode == 2 && triangle % 2 != 0 {
                    corners.swap(0, 1);
                }
                let root_corners = corners.map(|index| mapped[index as usize]);
                if root_corners[0] == root_corners[1]
                    || root_corners[1] == root_corners[2]
                    || root_corners[2] == root_corners[0]
                {
                    continue;
                }
                if self.budget.enabled() {
                    local_triangles.extend(corners);
                }
                if let Some(output) = output.as_deref_mut() {
                    output.extend(root_corners);
                    self.total_indices += 3;
                }
            }
        }
        let Some(hardware) = hardware else {
            return Ok((0..0, local_triangles));
        };
        let first = hardware.vertices.len();
        self.budget.reserve(&mut hardware.vertices, mapped.len())?;
        hardware.vertices.extend(mapped);
        Ok((first..hardware.vertices.len(), local_triangles))
    }
}

fn replacements(reader: &Reader<'_>, lods: usize, budget: &mut Budget<'_>) -> io::Result<()> {
    let start = reader.relative(0, 24)?;
    reader.bytes(start, lods * 8)?;
    budget.charge(0, lods)?;
    for lod in 0..lods {
        let table = reader.table(start + lod * 8, 0, 4, 6, super::MAX_MATERIALS)?;
        if table.len != 0 {
            return Err(invalid(
                "VTX hardware layout does not support LOD material replacements",
            ));
        }
    }
    Ok(())
}

pub fn parse(
    data: &[u8],
    checksum: i32,
    vvd_lods: usize,
    root_lod: usize,
    meshes: &[Vec<Vec<Mesh>>],
    budget: &mut Budget<'_>,
) -> io::Result<Geometry> {
    let reader = Reader::new(data, 36)?;
    if reader.int(0)? != 7 || reader.int(16)? != checksum {
        return Err(invalid("VTX version or model checksum does not match"));
    }
    let lod_count = reader.count(20, MAX_LODS)?;
    if lod_count == 0 {
        return Err(invalid("VTX requires at least one LOD"));
    }
    if budget.enabled() {
        if lod_count != vvd_lods {
            return Err(invalid("VTX and VVD declared LOD counts do not match"));
        }
        replacements(&reader, lod_count, budget)?;
    }
    let bodies = reader.table(0, 28, 32, 8, MAX_BODY_PARTS)?;
    if bodies.len != meshes.len() {
        return Err(invalid("VTX body parts do not match MDL"));
    }
    let mut hardware = budget.enabled().then(|| HardwareLayout {
        checksum,
        lods: lod_count,
        root_lod: root_lod.min(lod_count - 1),
        sections: Vec::new(),
        vertices: Vec::new(),
    });
    budget.array::<Vec<std::ops::Range<usize>>>(bodies.len)?;
    let mut traversal = Traversal {
        reader,
        budget,
        work: 0,
        total_indices: 0,
    };
    let mut parts = Vec::new();
    let mut model_ranges = Vec::with_capacity(bodies.len);
    for (body, body_meshes) in meshes.iter().enumerate() {
        let models = traversal
            .reader
            .table(bodies.at(body)?, 0, 4, 8, MAX_MODELS)?;
        if models.len != body_meshes.len() {
            return Err(invalid("VTX body models do not match MDL"));
        }
        if traversal.budget.enabled() {
            traversal.consume(models.len)?;
        }
        traversal
            .budget
            .array::<std::ops::Range<usize>>(models.len)?;
        let mut ranges = Vec::with_capacity(models.len);
        for (model_index, model_meshes) in body_meshes.iter().enumerate() {
            let first = parts.len();
            let lods = traversal
                .reader
                .table(models.at(model_index)?, 0, 4, 12, MAX_LODS)?;
            if lods.len == 0 && model_meshes.is_empty() && !traversal.budget.enabled() {
                ranges.push(first..first);
                continue;
            }
            if traversal.budget.enabled() && lods.len != lod_count {
                return Err(invalid(
                    "VTX model LOD count does not match its file header",
                ));
            }
            let retained_lods = if traversal.budget.enabled() {
                lods.len
            } else {
                1
            };
            for lod in 0..retained_lods {
                let lod_base = lods.at(lod)?;
                let vtx_meshes = traversal.reader.table(lod_base, 0, 4, 9, MAX_MESHES)?;
                if vtx_meshes.len != model_meshes.len() {
                    return Err(invalid("VTX LOD meshes do not match MDL"));
                }
                if traversal.budget.enabled() {
                    traversal.reader.float(lod_base + 8)?;
                }
                if traversal.budget.enabled() {
                    traversal.consume(vtx_meshes.len)?;
                }
                if lod == 0 && traversal.budget.enabled() {
                    traversal.budget.reserve(&mut parts, model_meshes.len())?;
                }
                for (index, mesh) in model_meshes.iter().enumerate() {
                    let mesh_base = vtx_meshes.at(index)?;
                    if traversal.budget.enabled() && traversal.reader.byte(mesh_base + 8)? != 0 {
                        return Err(invalid(
                            "VTX hardware layout does not support specialized mesh flags",
                        ));
                    }
                    let groups = traversal.reader.table(mesh_base, 0, 4, 25, MAX_GROUPS)?;
                    let mut output = Vec::new();
                    for group in 0..groups.len {
                        let (vertices, triangle_indices) = traversal.group(
                            groups.at(group)?,
                            mesh,
                            lod,
                            (lod == 0).then_some(&mut output),
                            &mut hardware,
                        )?;
                        if let Some(layout) = &mut hardware {
                            traversal.budget.reserve(&mut layout.sections, 1)?;
                            layout.sections.push(HardwareSection {
                                body_part: body,
                                model: model_index,
                                lod,
                                mesh: index,
                                group,
                                part: first + index,
                                vertices,
                                triangle_indices,
                            });
                        }
                    }
                    if lod == 0 {
                        parts.push(Part {
                            material: mesh.material,
                            indices: output,
                        });
                    }
                }
            }
            ranges.push(first..parts.len());
        }
        model_ranges.push(ranges);
    }
    if let Some(layout) = &mut hardware {
        traversal.budget.charge(0, layout.sections.len() * 16)?;
        layout.sections.sort_unstable_by_key(|section| {
            (
                section.lod,
                section.body_part,
                section.model,
                section.mesh,
                section.group,
            )
        });
    }
    Ok(Geometry {
        parts,
        models: model_ranges,
        hardware,
    })
}
