use glam::DVec3;
use map_core::displacement::neighbors::{self, NeighborPair, Span, SurfaceGrid};
use sha2::{Digest, Sha256};
use source_formats::bsp::{DisplacementLightmapSample, DisplacementSurfaceSampler, TexInfo};

use super::bake::{Allocation, Budget};
use crate::map::{Map, Terrain};
use crate::{NativeTerrainNormalInputs, TerrainLightingNormals, TerrainNormalInputs};

const MAX_EXTENT: usize = 125;

pub(super) struct Fields {
    pub positions: Vec<Vec<[f32; 3]>>,
    pub normals: Vec<Vec<[f64; 3]>>,
    _allocation: Option<Allocation>,
}

fn authored_fields(
    terrains: &[Terrain],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Fields, String> {
    let mut positions = Vec::with_capacity(terrains.len());
    for terrain in terrains {
        consume(terrain.triangles.len())?;
        positions.push(terrain.native_positions()?);
    }
    let doubles = positions
        .iter()
        .map(|positions| {
            positions
                .iter()
                .map(|point| DVec3::from_array(point.map(f64::from)))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let grids = terrains
        .iter()
        .zip(&doubles)
        .map(|(terrain, positions)| {
            let size = terrain.mesh.size;
            SurfaceGrid {
                power: terrain.info.power as u32,
                corners: [0, (size - 1) * size, size * size - 1, size - 1]
                    .map(|index| crate::emit::stored_vertex(terrain.mesh.base_positions[index])),
                positions,
            }
        })
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for (first, terrain) in terrains.iter().enumerate() {
        for (edge, neighbors) in terrain.info.edges.iter().enumerate() {
            for neighbor in neighbors {
                if neighbor.index == u16::MAX || neighbor.index as usize <= first {
                    continue;
                }
                let second_edge = (edge + neighbor.orientation as usize + 2) % 4;
                pairs.push(NeighborPair {
                    first,
                    first_edge: edge,
                    second: neighbor.index as usize,
                    second_edge,
                    first_span: Span::from_native(neighbor.span, edge)?,
                    second_span: Span::from_native(neighbor.neighbor_span, second_edge)?,
                });
            }
        }
    }
    let angles = terrains
        .iter()
        .map(|terrain| terrain.smoothing_angle)
        .collect::<Vec<_>>();
    let corners = terrains
        .iter()
        .map(|terrain| terrain.info.corners)
        .collect::<Vec<_>>();
    let normals =
        neighbors::vertex_normals_with_corners(&grids, &pairs, &corners, &angles, consume)?;
    Ok(Fields {
        positions,
        normals,
        _allocation: None,
    })
}

pub(super) fn fields(
    map: &Map<'_>,
    bsp: &source_formats::bsp::Bsp,
    mode: TerrainLightingNormals,
    budget: &mut Budget<'_>,
) -> Result<(Fields, TerrainNormalInputs), String> {
    let mut report = TerrainNormalInputs {
        mode,
        status: if map.terrains.is_empty() {
            "not_applied"
        } else {
            "applied"
        },
        sampling: "hammerr_lightmap_triangle_barycentric_then_normalize",
        displacement_count: map.terrains.len(),
        sample_count: map
            .terrains
            .iter()
            .map(|terrain| terrain.vertices.len())
            .sum(),
        native: None,
    };
    if mode == TerrainLightingNormals::Authored || map.terrains.is_empty() {
        return Ok((
            authored_fields(&map.terrains, &mut |amount| budget.consume(amount))?,
            report,
        ));
    }
    use source_formats::bsp::native_terrain::NativeTerrainVectorStatus;
    use source_formats::bsp::native_terrain::basis::{
        MAX_NATIVE_BASIS_SCRATCH_BYTES, MAX_NATIVE_BASIS_WORK, NativeTerrainBasisProfile,
        predict_native_terrain_basis_budgeted_controlled,
    };
    use source_formats::bsp::{LightingKind, NativeDisplacementRounding, surf};
    for terrain in &map.terrains {
        budget.consume(1)?;
        let material = map.materials.get(terrain.material);
        if !material.lit() || material.surface & surf::BUMPLIGHT != 0 {
            return Err(format!(
                "solid:{} side:{}: native client terrain normals require an ordinary unbumped lit terrain material",
                terrain.solid, terrain.side
            ));
        }
    }
    let field_bytes = report
        .sample_count
        .checked_mul(size_of::<[f32; 3]>() + size_of::<[f64; 3]>())
        .and_then(|bytes| {
            bytes.checked_add(
                map.terrains.len() * (size_of::<Vec<[f32; 3]>>() + size_of::<Vec<[f64; 3]>>()),
            )
        })
        .ok_or("Native client terrain normal field allocation size overflows")?;
    let allocation = budget.reserve_bytes(field_bytes)?;
    let _kernel_allocation = budget.reserve_bytes(MAX_NATIVE_BASIS_SCRATCH_BYTES)?;
    let profile = NativeTerrainBasisProfile::current_cpu(
        NativeDisplacementRounding::NearestEven,
        NativeDisplacementRounding::NearestEven,
        true,
        true,
    )
    .map_err(|error| format!("Native client terrain normal profile: {error}"))?;
    let prediction = predict_native_terrain_basis_budgeted_controlled(
        bsp,
        LightingKind::Ldr,
        profile,
        &mut |amount| budget.consume(amount).map_err(std::io::Error::other),
    )
    .map_err(|error| format!("Native client terrain normal catalog: {error}"))?;
    if prediction.snapshot().displacements().len() != map.terrains.len() {
        return Err(
            "Native client terrain normal catalog count differs from compiler terrain".into(),
        );
    }
    let mut positions = Vec::with_capacity(map.terrains.len());
    let mut normals = Vec::with_capacity(map.terrains.len());
    let mut normal_digest = Sha256::new();
    for (ordinal, (terrain, predicted)) in map
        .terrains
        .iter()
        .zip(prediction.snapshot().displacements())
        .enumerate()
    {
        budget.consume(terrain.vertices.len())?;
        let invalid = |reason: &str| {
            format!(
                "solid:{} side:{}: native client terrain normals {reason}",
                terrain.solid, terrain.side
            )
        };
        let source = predicted.source();
        if usize::from(predicted.ordinal()) != ordinal
            || predicted.power() != terrain.info.power
            || source.info().edges != terrain.info.edges
            || source.info().corners != terrain.info.corners
            || source.info().allowed_vertices != terrain.info.allowed_vertices
        {
            return Err(invalid(
                "catalog identity or native neighbor/LOD fields differ",
            ));
        }
        let actual = terrain
            .native_positions()
            .map_err(|error| invalid(&error))?;
        if actual.len() != predicted.samples().len()
            || actual
                .iter()
                .zip(predicted.samples())
                .any(|(position, sample)| position.map(f32::to_bits) != sample.position_bits())
        {
            return Err(invalid("positions differ from the exact compiled geometry"));
        }
        let mut field = Vec::with_capacity(actual.len());
        normal_digest.update(predicted.ordinal().to_le_bytes());
        for sample in predicted.samples() {
            if sample.normal().status() != NativeTerrainVectorStatus::FiniteNonzero {
                return Err(invalid("require finite nonzero native load normals"));
            }
            let bits = sample.normal().bits();
            for component in bits {
                normal_digest.update(component.to_le_bytes());
            }
            field.push(bits.map(|bits| f64::from(f32::from_bits(bits))));
        }
        positions.push(actual);
        normals.push(field);
    }
    let receipt = prediction.receipt();
    let profile = prediction.profile();
    report.native = Some(NativeTerrainNormalInputs {
        implementation: receipt.implementation,
        engine_sha256: receipt.engine_sha256,
        cpu_vendor: profile.cpu.vendor.clone(),
        cpu_brand: profile.cpu.brand.clone(),
        cpu_signature: profile.cpu.signature,
        rsqrt_probe_bits: profile.cpu.rsqrt_probe_bits,
        position_rounding: "nearest_even",
        basis_rounding: "nearest_even",
        flush_to_zero: profile.flush_to_zero,
        denormals_are_zero: profile.denormals_are_zero,
        caller_mxcsr: receipt.caller_mxcsr,
        selected_mxcsr: receipt.selected_mxcsr,
        local_digest: receipt.local_digest,
        t_junction_digest: receipt.t_junction_digest,
        corner_digest: receipt.corner_digest,
        final_digest: receipt.final_digest,
        normal_sha256: format!("{:x}", normal_digest.finalize()),
        work_units: receipt.work_units,
        kernel_work_limit_units: MAX_NATIVE_BASIS_WORK,
        scratch_reserved_bytes: receipt.scratch_reserved_bytes,
        caller_scratch_reserved_bytes: MAX_NATIVE_BASIS_SCRATCH_BYTES,
        retained_bytes: receipt.retained_bytes,
    });
    Ok((
        Fields {
            positions,
            normals,
            _allocation: Some(allocation),
        },
        report,
    ))
}

pub(super) struct TerrainSample {
    pub point: DVec3,
    pub normal: DVec3,
    pub record: Option<DisplacementLightmapSample>,
}

pub(super) struct TerrainPlan {
    pub patch: usize,
    pub dimensions: [usize; 2],
    pub samples: Vec<TerrainSample>,
}

impl TerrainPlan {
    pub(super) fn dimensions(terrain: &Terrain, info: &TexInfo) -> Result<[usize; 2], String> {
        let scale = DVec3::from_array(
            [
                info.lightmap_vecs[0][0],
                info.lightmap_vecs[0][1],
                info.lightmap_vecs[0][2],
            ]
            .map(f64::from),
        )
        .length()
        .recip();
        if !scale.is_finite() || scale < 1.0 || scale > f64::from(i32::MAX) {
            return Err("Terrain lightmap scale must fit a positive native integer".into());
        }
        let scale = scale.round();
        let size = terrain.mesh.size;
        let points = &terrain.mesh.base_positions;
        let [a, b, c, d] = [
            points[0],
            points[(size - 1) * size],
            points[size * size - 1],
            points[size - 1],
        ];
        let extents = [
            a.distance(d).max(b.distance(c)),
            a.distance(b).max(d.distance(c)),
        ];
        Ok(extents.map(|length| ((length / scale).floor() as usize + 1).min(MAX_EXTENT) + 1))
    }

    pub(super) fn new(
        terrain: &Terrain,
        patch: usize,
        positions: &[[f32; 3]],
        normals: Vec<[f64; 3]>,
        dimensions: [usize; 2],
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Self, String> {
        consume(terrain.vertices.len() + terrain.triangles.len())?;
        let sampler =
            DisplacementSurfaceSampler::with_vertex_normals(terrain.info, positions, normals)
                .map_err(|error| error.to_string())?;
        let mut samples = Vec::with_capacity(dimensions[0] * dimensions[1]);
        for t in 0..dimensions[1] {
            for s in 0..dimensions[0] {
                consume(DisplacementSurfaceSampler::MAX_SAMPLE_WORK)?;
                let sample = sampler
                    .sample_smoothed([
                        s as f64 / (dimensions[0] - 1) as f64,
                        t as f64 / (dimensions[1] - 1) as f64,
                    ])
                    .map_err(|error| error.to_string())?;
                samples.push(TerrainSample {
                    point: DVec3::from_array(sample.surface.position),
                    normal: DVec3::from_array(sample.surface.normal),
                    record: sample.lightmap_record(),
                });
            }
        }
        Ok(Self {
            patch,
            dimensions,
            samples,
        })
    }
}
