use glam::DVec3;
use source_formats::bsp::{
    DisplacementLightmapSample, DisplacementSurfaceSampler, Displacements, TexInfo,
};

use crate::map::Terrain;

const MAX_EXTENT: usize = 125;

pub(super) struct TerrainSample {
    pub point: DVec3,
    pub normal: DVec3,
    pub record: Option<DisplacementLightmapSample>,
}

pub(super) struct TerrainPlan {
    pub dimensions: [usize; 2],
    pub samples: Vec<TerrainSample>,
}

pub(super) fn positions(terrain: &Terrain) -> Result<Vec<[f32; 3]>, String> {
    let size = terrain.mesh.size;
    let data = Displacements {
        infos: vec![terrain.info],
        vertices: terrain.vertices.clone(),
        triangles: terrain.triangles.clone(),
    };
    let base = &terrain.mesh.base_positions;
    let quad = [
        base[0],
        base[(size - 1) * size],
        base[size * size - 1],
        base[size - 1],
    ]
    .map(|point| point.as_vec3().to_array());
    data.positions(0, quad).map_err(|error| error.to_string())
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
        dimensions: [usize; 2],
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Self, String> {
        consume(terrain.vertices.len() + 2 * terrain.triangles.len())?;
        let positions = positions(terrain)?;
        let sampler = DisplacementSurfaceSampler::new(terrain.info, &positions)
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
            dimensions,
            samples,
        })
    }
}
