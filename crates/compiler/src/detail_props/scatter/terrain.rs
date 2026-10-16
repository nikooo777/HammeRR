use glam::{DVec2, DVec3};

use super::{Budget, Builder, DENSITY_AREA, Definition, Random, place, selected_model};
use crate::{Control, faces::Face, map::Map};

const BASE_EPSILON: f64 = 0.02;
const WEIGHT_EPSILON: f64 = 1e-9;
const ALPHA_RANGE: f64 = 255.0;

struct Triangle {
    coordinates: [DVec2; 3],
    positions: [DVec3; 3],
    alphas: [f64; 3],
    normal: DVec3,
}

struct Surface {
    cells: usize,
    triangles: Vec<Triangle>,
    lookup: Vec<Vec<usize>>,
}

struct Sample {
    point: DVec3,
    normal: DVec3,
    center: DVec3,
    alpha: f64,
}

impl Surface {
    fn new(
        terrain: &crate::map::Terrain,
        budget: &mut Budget,
        control: &Control,
    ) -> Result<Self, String> {
        let size = terrain
            .info
            .grid_size()
            .map_err(|error| error.to_string())?;
        let cells = size - 1;
        budget.terrain_work(terrain.vertices.len() + terrain.indices.len(), control)?;
        let positions = terrain.native_positions()?;
        let mut triangles = Vec::with_capacity(terrain.indices.len());
        let mut lookup = vec![Vec::new(); cells * cells];
        for &indices in &terrain.indices {
            control.checkpoint()?;
            let coordinates =
                indices.map(|index| DVec2::new((index % size) as f64, (index / size) as f64));
            let positions = indices.map(|index| DVec3::from_array(positions[index].map(f64::from)));
            let alphas = indices.map(|index| f64::from(terrain.vertices[index].alpha));
            let normal = (positions[2] - positions[0])
                .cross(positions[1] - positions[0])
                .try_normalize()
                .ok_or("Terrain detail surface has a collapsed native triangle")?;
            let min = coordinates[0].min(coordinates[1]).min(coordinates[2]);
            let max = coordinates[0].max(coordinates[1]).max(coordinates[2]);
            if (coordinates[1] - coordinates[0])
                .perp_dot(coordinates[2] - coordinates[0])
                .abs()
                <= WEIGHT_EPSILON
            {
                return Err("Terrain detail surface has a collapsed base triangle".into());
            }
            for row in min.y as usize..max.y as usize {
                for col in min.x as usize..max.x as usize {
                    budget.terrain_work(1, control)?;
                    lookup[row * cells + col].push(triangles.len());
                }
            }
            triangles.push(Triangle {
                coordinates,
                positions,
                alphas,
                normal,
            });
        }
        Ok(Self {
            cells,
            triangles,
            lookup,
        })
    }

    fn sample(&self, uv: DVec2, budget: &mut Budget, control: &Control) -> Result<Sample, String> {
        let uv = uv * self.cells as f64;
        let cell = uv.y.floor() as usize * self.cells + uv.x.floor() as usize;
        for &index in &self.lookup[cell] {
            budget.terrain_work(1, control)?;
            let triangle = &self.triangles[index];
            let [a, b, c] = triangle.coordinates;
            let determinant = (b - a).perp_dot(c - a);
            let u = (uv - a).perp_dot(c - a) / determinant;
            let v = (b - a).perp_dot(uv - a) / determinant;
            let weights = [1.0 - u - v, u, v];
            if weights
                .iter()
                .any(|weight| !(-WEIGHT_EPSILON..=1.0 + WEIGHT_EPSILON).contains(weight))
            {
                continue;
            }
            let weights = weights.map(|weight| weight.clamp(0.0, 1.0));
            let total: f64 = weights.iter().sum();
            let weights = weights.map(|weight| weight / total);
            let point = triangle
                .positions
                .iter()
                .zip(weights)
                .map(|(&position, weight)| position * weight)
                .sum();
            let alpha = triangle
                .alphas
                .iter()
                .zip(weights)
                .map(|(&alpha, weight)| alpha * weight)
                .sum::<f64>()
                / ALPHA_RANGE;
            return Ok(Sample {
                point,
                alpha,
                normal: triangle.normal,
                center: triangle.positions.iter().sum::<DVec3>() / 3.0,
            });
        }
        Err("Terrain detail UV does not resolve to a final compiled triangle".into())
    }
}

pub(super) fn emit(
    map: &Map,
    face: &Face,
    index: usize,
    definition: &Definition,
    builder: &mut Builder,
    budget: &mut Budget,
    control: &Control,
) -> Result<(), String> {
    let terrain = map
        .terrains
        .get(index)
        .ok_or("Terrain detail face has no compiled surface")?;
    let base = terrain.base.map(crate::emit::stored_vertex);
    if (base[0] + base[2] - base[1] - base[3]).length() > BASE_EPSILON {
        return Err("Terrain detail scattering requires a parallelogram base quad; skewed terrain bases are unsupported".into());
    }
    if definition
        .groups
        .iter()
        .all(|group| group.models.is_empty() && group.error.is_none())
    {
        return Ok(());
    }
    let area = ((base[1] - base[0]).cross(base[2] - base[0]).length()
        + (base[2] - base[0]).cross(base[3] - base[0]).length())
        * 0.5;
    let count = budget.samples((area * definition.density / DENSITY_AREA).floor())?;
    if count == 0 {
        return Ok(());
    }
    let surface = Surface::new(terrain, budget, control)?;
    let mut random = Random::face(face)?;
    for _ in 0..count {
        control.checkpoint()?;
        let sample = surface.sample(DVec2::new(random.unit(), random.unit()), budget, control)?;
        let models = definition.selected_models(sample.alpha, &mut random)?;
        let Some(model) = selected_model(models, random.unit()) else {
            continue;
        };
        place(
            model,
            sample.point,
            sample.normal,
            sample.center,
            builder,
            &mut random,
            control,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface() -> Surface {
        Surface {
            cells: 1,
            triangles: vec![Triangle {
                coordinates: [DVec2::ZERO, DVec2::X, DVec2::Y],
                positions: [DVec3::ZERO, DVec3::X, DVec3::Y],
                alphas: [0.0, 0.0, 255.0],
                normal: DVec3::Z,
            }],
            lookup: vec![vec![0]],
        }
    }

    #[test]
    fn terrain_lookup_honors_cancellation_and_shared_work_exhaustion() {
        let cancelled = Control::default();
        cancelled.cancel().unwrap();
        assert!(
            surface()
                .sample(DVec2::splat(0.25), &mut Budget::default(), &cancelled)
                .is_err()
        );
        let mut budget = Budget {
            terrain_work: super::super::MAX_TERRAIN_WORK,
            ..Default::default()
        };
        let error = surface()
            .sample(DVec2::splat(0.25), &mut budget, &Control::default())
            .err()
            .unwrap();
        assert!(error.contains("four-million-step lookup budget"));
    }
}
