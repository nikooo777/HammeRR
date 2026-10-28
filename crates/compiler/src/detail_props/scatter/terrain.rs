use std::collections::BTreeMap;

use glam::{DVec2, DVec3};

use super::{Budget, Builder, DENSITY_AREA, Definition, Random, place, selected_model};
use crate::{Control, faces::Face, map::Map};

const BASE_EPSILON: f64 = 0.02;
const WEIGHT_EPSILON: f64 = 1e-9;
const AREA_REL_EPSILON: f64 = 1e-8;
const ALPHA_RANGE: f64 = 255.0;

struct Base {
    area: f64,
    chart: Option<Chart>,
}

struct Chart {
    corners: [DVec3; 4],
    normal: DVec3,
}

impl Base {
    fn new(base: [DVec3; 4], start: DVec3) -> Result<Self, String> {
        if base.iter().any(|point| !point.is_finite()) || !start.is_finite() {
            return Err("Terrain detail base chart must be finite".into());
        }
        if (base[0] + base[2] - base[1] - base[3]).length() <= BASE_EPSILON {
            let area = ((base[1] - base[0]).cross(base[2] - base[0]).length()
                + (base[2] - base[0]).cross(base[3] - base[0]).length())
                * 0.5;
            return Ok(Self { area, chart: None });
        }
        let area_vector = (base[3] - base[0]).cross(base[2] - base[0])
            + (base[2] - base[0]).cross(base[1] - base[0]);
        let normal = area_vector
            .try_normalize()
            .ok_or("Terrain detail base chart has a collapsed area")?;
        let first = (0..4)
            .min_by(|&a, &b| {
                base[a]
                    .distance_squared(start)
                    .total_cmp(&base[b].distance_squared(start))
            })
            .expect("four base corners");
        let [a, b, c, d] = [0, 1, 2, 3].map(|offset| base[(first + offset) % 4]);
        let column = d - a;
        let row = b - a;
        let skew = a + c - b - d;
        if base
            .iter()
            .any(|point| (*point - a).dot(normal).abs() > BASE_EPSILON)
        {
            return Err("Terrain detail base chart must be planar within native precision".into());
        }
        let jacobian = [
            column.cross(row).dot(normal),
            column.cross(skew).dot(normal),
            skew.cross(row).dot(normal),
        ];
        let [constant, u, v] = jacobian;
        if [constant, constant + u, constant + v, constant + u + v]
            .iter()
            .any(|value| !value.is_finite() || *value <= WEIGHT_EPSILON)
        {
            return Err("Terrain detail base chart must have positive finite corner areas".into());
        }
        Ok(Self {
            area: area_vector.length() * 0.5,
            chart: Some(Chart {
                corners: [a, b, c, d],
                normal,
            }),
        })
    }
}

impl Chart {
    fn triangle_area(&self, coordinates: [DVec2; 3]) -> Result<f64, String> {
        let [a, b, c, d] = self.corners;
        let points = coordinates.map(|uv| a.lerp(b, uv.y).lerp(d.lerp(c, uv.y), uv.x));
        let area = (points[2] - points[0])
            .cross(points[1] - points[0])
            .dot(self.normal)
            * 0.5;
        if !area.is_finite() || area <= WEIGHT_EPSILON {
            return Err("Terrain detail surface has a collapsed or reversed base triangle".into());
        }
        Ok(area)
    }
}

fn add_edge(
    edges: &mut BTreeMap<(usize, usize), (u8, i8)>,
    a: usize,
    b: usize,
) -> Result<(), String> {
    let (key, direction) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
    let edge = edges.entry(key).or_default();
    edge.0 += 1;
    edge.1 += direction;
    if edge.0 > 2 || (edge.0 == 2 && edge.1 != 0) {
        return Err("Terrain detail base triangles overlap at a shared edge".into());
    }
    Ok(())
}

fn check_edges(
    edges: BTreeMap<(usize, usize), (u8, i8)>,
    cells: usize,
    budget: &mut Budget,
    control: &Control,
) -> Result<(), String> {
    let size = cells + 1;
    let mut boundary = vec![false; cells * 4];
    for ((a, b), (count, _)) in edges {
        budget.terrain_work(1, control)?;
        if count == 2 {
            continue;
        }
        let (col_a, row_a) = (a % size, a / size);
        let (col_b, row_b) = (b % size, b / size);
        let (offset, start, end) = if row_a == row_b && (row_a == 0 || row_a == cells) {
            (
                usize::from(row_a == cells) * cells,
                col_a.min(col_b),
                col_a.max(col_b),
            )
        } else if col_a == col_b && (col_a == 0 || col_a == cells) {
            (
                (2 + usize::from(col_a == cells)) * cells,
                row_a.min(row_b),
                row_a.max(row_b),
            )
        } else {
            return Err("Terrain detail base triangles have an open interior edge".into());
        };
        for covered in &mut boundary[offset + start..offset + end] {
            budget.terrain_work(1, control)?;
            if *covered {
                return Err("Terrain detail base triangles overlap at the quad boundary".into());
            }
            *covered = true;
        }
    }
    if boundary.iter().any(|covered| !covered) {
        return Err("Terrain detail base triangles do not cover the quad boundary".into());
    }
    Ok(())
}

struct Triangle {
    coordinates: [DVec2; 3],
    positions: [DVec3; 3],
    alphas: [f64; 3],
    normal: DVec3,
}

impl Triangle {
    fn sample(&self, weights: [f64; 3]) -> Sample {
        let point = self
            .positions
            .iter()
            .zip(weights)
            .map(|(&position, weight)| position * weight)
            .sum();
        let alpha = self
            .alphas
            .iter()
            .zip(weights)
            .map(|(&alpha, weight)| alpha * weight)
            .sum::<f64>()
            / ALPHA_RANGE;
        Sample {
            point,
            alpha,
            normal: self.normal,
            center: self.positions.iter().sum::<DVec3>() / 3.0,
        }
    }
}

struct Surface {
    cells: usize,
    triangles: Vec<Triangle>,
    lookup: Vec<Vec<usize>>,
    areas: Vec<f64>,
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
        base: &Base,
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
        let mut areas = Vec::new();
        let mut edges = BTreeMap::new();
        for &indices in &terrain.indices {
            control.checkpoint()?;
            if base.chart.is_some()
                && indices
                    .iter()
                    .any(|&index| index >= positions.len() || index >= terrain.vertices.len())
            {
                return Err("Terrain detail base triangle has an invalid vertex index".into());
            }
            let coordinates =
                indices.map(|index| DVec2::new((index % size) as f64, (index / size) as f64));
            if let Some(chart) = &base.chart {
                budget.terrain_work(4, control)?;
                let area = chart.triangle_area(coordinates.map(|uv| uv / cells as f64))?;
                let total = areas.last().copied().unwrap_or(0.0) + area;
                if !total.is_finite() || total <= areas.last().copied().unwrap_or(0.0) {
                    return Err(
                        "Terrain detail base triangle area sum must increase finitely".into(),
                    );
                }
                areas.push(total);
                for [a, b] in [
                    [indices[0], indices[1]],
                    [indices[1], indices[2]],
                    [indices[2], indices[0]],
                ] {
                    add_edge(&mut edges, a, b)?;
                }
            }
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
        if base.chart.is_some() {
            check_edges(edges, cells, budget, control)?;
            let area = areas.last().copied().unwrap_or(0.0);
            if (area - base.area).abs() > base.area * AREA_REL_EPSILON {
                return Err(
                    "Terrain detail base triangles do not cover the whole projected quad area"
                        .into(),
                );
            }
        }
        Ok(Self {
            cells,
            triangles,
            lookup,
            areas,
        })
    }

    fn sample_area(
        &self,
        choice: DVec2,
        budget: &mut Budget,
        control: &Control,
    ) -> Result<Sample, String> {
        budget.terrain_work(self.areas.len().max(1).ilog2() as usize + 1, control)?;
        let target = choice.x
            * self
                .areas
                .last()
                .ok_or("Terrain detail surface has no base areas")?;
        let index = self.areas.partition_point(|&area| area <= target);
        let triangle = self
            .triangles
            .get(index)
            .ok_or("Terrain detail area sample is outside the base surface")?;
        let previous = index
            .checked_sub(1)
            .map(|index| self.areas[index])
            .unwrap_or(0.0);
        let residual = ((target - previous) / (self.areas[index] - previous)).clamp(0.0, 1.0);
        let radial = residual.sqrt();
        Ok(triangle.sample([1.0 - radial, radial * (1.0 - choice.y), radial * choice.y]))
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
            return Ok(triangle.sample(weights));
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
    let base = Base::new(
        terrain.base.map(crate::emit::stored_vertex),
        DVec3::from_array(terrain.info.start.map(f64::from)),
    )?;
    if definition
        .groups
        .iter()
        .all(|group| group.models.is_empty() && group.error.is_none())
    {
        return Ok(());
    }
    let count = budget.samples((base.area * definition.density / DENSITY_AREA).floor())?;
    if count == 0 {
        return Ok(());
    }
    let surface = Surface::new(terrain, &base, budget, control)?;
    let mut random = Random::face(face)?;
    for _ in 0..count {
        control.checkpoint()?;
        let choice = DVec2::new(random.unit(), random.unit());
        let sample = if base.chart.is_some() {
            surface.sample_area(choice, budget, control)?
        } else {
            surface.sample(choice, budget, control)?
        };
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
