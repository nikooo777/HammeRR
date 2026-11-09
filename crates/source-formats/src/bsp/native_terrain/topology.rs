use std::io;

use super::NativeTerrainSource;
use crate::bsp::invalid;

pub const NATIVE_TERRAIN_MESH_GROUP_ELEMENTS: usize = 8192;

pub struct NativeTerrainTopologyReadback {
    pub vertex_base: u32,
    pub active_vertices: [u32; 10],
    pub allowed_vertices: [u32; 10],
    pub indices: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTerrainTopologyComparison {
    pub active_allowed_mismatched_bits: u32,
    pub active_compiled_mismatched_bits: u32,
    pub allowed_compiled_mismatched_bits: u32,
    pub ordered_triangle_mismatches: usize,
}

impl NativeTerrainTopologyComparison {
    pub const fn matches(self) -> bool {
        self.active_allowed_mismatched_bits == 0
            && self.active_compiled_mismatched_bits == 0
            && self.allowed_compiled_mismatched_bits == 0
            && self.ordered_triangle_mismatches == 0
    }
}

pub struct NativeTerrainTopology {
    vertex_base: u32,
    vertex_count: usize,
    active_vertices: [u32; 10],
    allowed_vertices: [u32; 10],
    compiled_allowed_vertices: [u32; 10],
    indices: Vec<u16>,
    triangles: Vec<[u16; 3]>,
    expected_triangles: Vec<[u16; 3]>,
    comparison: NativeTerrainTopologyComparison,
}

impl NativeTerrainTopology {
    pub const fn vertex_base(&self) -> u32 {
        self.vertex_base
    }

    pub const fn active_vertices(&self) -> &[u32; 10] {
        &self.active_vertices
    }

    pub const fn allowed_vertices(&self) -> &[u32; 10] {
        &self.allowed_vertices
    }

    pub const fn compiled_allowed_vertices(&self) -> &[u32; 10] {
        &self.compiled_allowed_vertices
    }

    pub fn active_vertex_count(&self) -> usize {
        vertex_bits(&self.active_vertices, self.vertex_count)
    }

    pub fn allowed_vertex_count(&self) -> usize {
        vertex_bits(&self.allowed_vertices, self.vertex_count)
    }

    pub fn compiled_allowed_vertex_count(&self) -> usize {
        vertex_bits(&self.compiled_allowed_vertices, self.vertex_count)
    }

    pub fn active_padding_bit_count(&self) -> usize {
        bit_count(&self.active_vertices) - self.active_vertex_count()
    }

    pub fn allowed_padding_bit_count(&self) -> usize {
        bit_count(&self.allowed_vertices) - self.allowed_vertex_count()
    }

    pub fn compiled_allowed_padding_bit_count(&self) -> usize {
        bit_count(&self.compiled_allowed_vertices) - self.compiled_allowed_vertex_count()
    }

    pub fn indices(&self) -> &[u16] {
        &self.indices
    }

    pub fn triangles(&self) -> &[[u16; 3]] {
        &self.triangles
    }

    pub fn expected_triangles(&self) -> &[[u16; 3]] {
        &self.expected_triangles
    }

    pub const fn comparison(&self) -> NativeTerrainTopologyComparison {
        self.comparison
    }

    pub(super) fn heap_bytes(&self) -> usize {
        self.indices.capacity() * size_of::<u16>()
            + self.triangles.capacity() * size_of::<[u16; 3]>()
            + self.expected_triangles.capacity() * size_of::<[u16; 3]>()
    }

    pub(super) fn decode_controlled(
        source: &NativeTerrainSource,
        readback: &NativeTerrainTopologyReadback,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        check()?;
        if !(2..=4).contains(&source.power()) {
            return Err(invalid("Unsupported native terrain topology power"));
        }
        let vertex_count = source.vertex_count();
        let maximum_indices = source.info().triangle_count()? * 3;
        if readback
            .vertex_base
            .checked_add(vertex_count as u32)
            .is_none_or(|end| end as usize > NATIVE_TERRAIN_MESH_GROUP_ELEMENTS)
            || !readback.indices.len().is_multiple_of(3)
            || readback.indices.len() > maximum_indices
        {
            return Err(invalid(
                "Native terrain topology base or index count exceeds its bounds",
            ));
        }
        let mut triangles = Vec::with_capacity(readback.indices.len() / 3);
        for (ordinal, raw) in readback.indices.as_chunks::<3>().0.iter().enumerate() {
            if ordinal.is_multiple_of(64) {
                check()?;
            }
            let local = raw.map(|index| index.wrapping_sub(readback.vertex_base as u16));
            if local
                .iter()
                .any(|index| usize::from(*index) >= vertex_count)
            {
                return Err(invalid(
                    "Native terrain topology index is outside its displacement",
                ));
            }
            triangles.push(local);
        }
        check()?;
        let expected_triangles: Vec<_> = source
            .info()
            .allowed_triangle_indices()?
            .into_iter()
            .map(|triangle| triangle.map(|index| index as u16))
            .collect();
        let compiled_allowed_vertices = source.info().allowed_vertices;
        let comparison = NativeTerrainTopologyComparison {
            active_allowed_mismatched_bits: mismatched_bits(
                &readback.active_vertices,
                &readback.allowed_vertices,
            ),
            active_compiled_mismatched_bits: mismatched_bits(
                &readback.active_vertices,
                &compiled_allowed_vertices,
            ),
            allowed_compiled_mismatched_bits: mismatched_bits(
                &readback.allowed_vertices,
                &compiled_allowed_vertices,
            ),
            ordered_triangle_mismatches: triangles
                .iter()
                .zip(&expected_triangles)
                .filter(|(observed, expected)| observed != expected)
                .count()
                + triangles.len().abs_diff(expected_triangles.len()),
        };
        check()?;
        Ok(Self {
            vertex_base: readback.vertex_base,
            vertex_count,
            active_vertices: readback.active_vertices,
            allowed_vertices: readback.allowed_vertices,
            compiled_allowed_vertices,
            indices: readback.indices.clone(),
            triangles,
            expected_triangles,
            comparison,
        })
    }
}

fn mismatched_bits(left: &[u32; 10], right: &[u32; 10]) -> u32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

fn bit_count(bits: &[u32; 10]) -> usize {
    bits.iter().map(|word| word.count_ones() as usize).sum()
}

fn vertex_bits(bits: &[u32; 10], vertex_count: usize) -> usize {
    (0..vertex_count)
        .filter(|index| bits[index / 32] & (1 << (index % 32)) != 0)
        .count()
}
