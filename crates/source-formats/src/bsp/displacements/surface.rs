use std::io;

use glam::{DVec2, DVec3};

use super::{DisplacementInfo, DisplacementLightmapSample, DisplacementSurfaceSample};
use crate::bsp::invalid;

const EPSILON: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplacementGeometricSample {
    pub triangle: u16,
    pub weights: [f64; 3],
    pub surface: DisplacementSurfaceSample,
}

impl DisplacementGeometricSample {
    pub fn lightmap_record(self) -> Option<DisplacementLightmapSample> {
        (self.triangle <= 510).then(|| DisplacementLightmapSample {
            triangle: self.triangle,
            weights: self.weights.map(|weight| (weight * 255.9) as u8),
        })
    }
}

struct Triangle {
    coordinates: [DVec2; 3],
    points: [DVec3; 3],
    normal: DVec3,
}

pub struct DisplacementSurfaceSampler {
    cells: usize,
    triangles: Vec<Triangle>,
    lookup: Vec<[u16; 2]>,
}

impl DisplacementSurfaceSampler {
    pub const MAX_SAMPLE_WORK: usize = 8;
    pub fn new(info: DisplacementInfo, positions: &[[f32; 3]]) -> io::Result<Self> {
        let size = info.grid_size()?;
        let cells = size - 1;
        if positions.len() != size * size || positions.iter().flatten().any(|x| !x.is_finite()) {
            return Err(invalid("displacement surface grid is invalid"));
        }
        let indices = info.lightmap_triangle_indices()?;
        let mut triangles = Vec::with_capacity(indices.len());
        let mut lookup = vec![[u16::MAX; 2]; cells * cells];
        for (index, indices) in indices.into_iter().enumerate() {
            let coordinates = indices.map(|i| DVec2::new((i % size) as f64, (i / size) as f64));
            let points = indices.map(|i| DVec3::from_array(positions[i].map(f64::from)));
            let normal = (points[2] - points[0])
                .cross(points[1] - points[0])
                .try_normalize()
                .ok_or_else(|| invalid("displacement surface triangle is collapsed"))?;
            let min = coordinates[0].min(coordinates[1]).min(coordinates[2]);
            let entry = &mut lookup[min.y as usize * cells + min.x as usize];
            let slot = entry.iter_mut().find(|i| **i == u16::MAX).unwrap();
            *slot = index as u16;
            triangles.push(Triangle {
                coordinates,
                points,
                normal,
            });
        }
        Ok(Self {
            cells,
            triangles,
            lookup,
        })
    }

    pub fn sample(&self, uv: [f64; 2]) -> io::Result<DisplacementGeometricSample> {
        if uv
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(invalid(
                "displacement surface UV must be finite and between zero and one",
            ));
        }
        let uv = DVec2::from_array(uv) * self.cells as f64;
        let bounds = [uv.x, uv.y].map(|value| {
            let (low, high) = if (value - value.round()).abs() <= EPSILON {
                let boundary = value.round() as usize;
                (
                    boundary.saturating_sub(1).min(self.cells - 1),
                    boundary.min(self.cells - 1),
                )
            } else {
                let cell = value.floor() as usize;
                (cell, cell)
            };
            low..=high
        });
        let mut found: Option<DisplacementGeometricSample> = None;
        for y in bounds[1].clone() {
            for x in bounds[0].clone() {
                for index in self.lookup[y * self.cells + x] {
                    if found.is_some_and(|sample| sample.triangle <= index) {
                        continue;
                    }
                    let triangle = &self.triangles[usize::from(index)];
                    let [a, b, c] = triangle.coordinates;
                    let determinant = (b - a).perp_dot(c - a);
                    let u = (uv - a).perp_dot(c - a) / determinant;
                    let v = (b - a).perp_dot(uv - a) / determinant;
                    let weights = [1.0 - u - v, u, v];
                    if weights
                        .iter()
                        .any(|&weight| !(-EPSILON..=1.0 + EPSILON).contains(&weight))
                    {
                        continue;
                    }
                    let weights = weights.map(|weight| weight.clamp(0.0, 1.0));
                    let total: f64 = weights.iter().sum();
                    let weights = weights.map(|weight| weight / total);
                    let position = triangle
                        .points
                        .iter()
                        .zip(weights)
                        .map(|(&point, weight)| point * weight)
                        .sum::<DVec3>();
                    found = Some(DisplacementGeometricSample {
                        triangle: index,
                        weights,
                        surface: DisplacementSurfaceSample {
                            position: position.to_array(),
                            normal: triangle.normal.to_array(),
                        },
                    });
                }
            }
        }
        found.ok_or_else(|| invalid("displacement UV does not resolve to a native triangle"))
    }
}
