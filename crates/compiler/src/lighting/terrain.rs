use glam::{DVec2, DVec3};
use source_formats::bsp::{DisplacementLightmapSample, Displacements, TexInfo};

use crate::map::Terrain;

const MAX_EXTENT: usize = 125;
const BARYCENTRIC_EPSILON: f64 = 1e-9;

pub(super) struct TerrainSample {
    pub point: DVec3,
    pub normal: DVec3,
    pub record: DisplacementLightmapSample,
}

pub(super) struct TerrainPlan {
    pub dimensions: [usize; 2],
    pub samples: Vec<TerrainSample>,
}

impl TerrainPlan {
    pub(super) fn dimensions(terrain: &Terrain, info: &TexInfo) -> Result<[usize; 2], String> {
        if terrain.info.power == 4 {
            return Err("Direct LDR terrain currently supports powers 2 and 3; power 4 needs a verified native triangle-511 sample strategy. Use Fullbright for this patch".into());
        }
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
        let size = terrain.mesh.size;
        let cells = (size - 1) as f64;
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
        let positions = data.positions(0, quad).map_err(|error| error.to_string())?;
        let coordinates = terrain
            .indices
            .iter()
            .map(|indices| {
                indices.map(|index| DVec2::new((index % size) as f64, (index / size) as f64))
            })
            .collect::<Vec<_>>();
        let mut samples = Vec::with_capacity(dimensions[0] * dimensions[1]);
        for t in 0..dimensions[1] {
            for s in 0..dimensions[0] {
                let uv = DVec2::new(
                    s as f64 * cells / (dimensions[0] - 1) as f64,
                    t as f64 * cells / (dimensions[1] - 1) as f64,
                );
                let mut found = None;
                for (triangle, &[a, b, c]) in coordinates.iter().enumerate() {
                    consume(1)?;
                    let determinant = (b - a).perp_dot(c - a);
                    let u = (uv - a).perp_dot(c - a) / determinant;
                    let v = (b - a).perp_dot(uv - a) / determinant;
                    let weights = [1.0 - u - v, u, v];
                    if weights.iter().any(|&weight| {
                        weight < -BARYCENTRIC_EPSILON || weight > 1.0 + BARYCENTRIC_EPSILON
                    }) {
                        continue;
                    }
                    let record = DisplacementLightmapSample {
                        triangle: triangle as u16,
                        weights: weights.map(|weight| (weight.clamp(0.0, 1.0) * 255.9) as u8),
                    };
                    let sample = record
                        .surface(&terrain.indices, &positions)
                        .map_err(|error| error.to_string())?
                        .ok_or("Terrain luxel unexpectedly has no surface sample")?;
                    found = Some(TerrainSample {
                        point: DVec3::from_array(sample.position),
                        normal: DVec3::from_array(sample.normal),
                        record,
                    });
                    break;
                }
                samples.push(found.ok_or("Terrain luxel does not resolve to a native triangle")?);
            }
        }
        Ok(Self {
            dimensions,
            samples,
        })
    }
}
