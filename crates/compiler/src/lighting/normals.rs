use std::collections::HashMap;
use std::sync::Arc;

use glam::DVec3;
use source_formats::bsp::{Bsp, Edge, Face, lump};

use super::bake::Budget;

mod junctions;

const HARD_GROUPS: u32 = 0xff00_0000;
const MAX_NORMALS: usize = u16::MAX as usize + 1;
const NORMAL_EPSILON: f64 = 1e-9;

pub(super) struct Field {
    center: DVec3,
    points: Vec<DVec3>,
    corners: Vec<DVec3>,
}

impl Field {
    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    pub fn sample(&self, point: DVec3, flat: DVec3) -> DVec3 {
        let points = &self.points;
        let offset = point - self.center;
        for index in 0..points.len() {
            let next = (index + 1) % points.len();
            let a = points[index] - self.center;
            let b = points[next] - self.center;
            let aa = a.length_squared();
            let bb = b.length_squared();
            let ab = a.dot(b);
            let determinant = aa * bb - ab * ab;
            if determinant <= aa * bb * 1e-12 {
                continue;
            }
            let u = (bb * a.dot(offset) - ab * b.dot(offset)) / determinant;
            let v = (aa * b.dot(offset) - ab * a.dot(offset)) / determinant;
            if u < -NORMAL_EPSILON || v < -NORMAL_EPSILON || u + v > 1.0 + NORMAL_EPSILON {
                continue;
            }
            let u = u.clamp(0.0, 1.0);
            let v = v.clamp(0.0, 1.0 - u);
            let normal = flat * (1.0 - u - v) + self.corners[index] * u + self.corners[next] * v;
            return normal
                .try_normalize()
                .filter(|normal| normal.dot(flat) > NORMAL_EPSILON)
                .unwrap_or(flat);
        }
        flat
    }
}

#[derive(Default)]
pub(super) struct Plan {
    pub fields: HashMap<usize, Arc<Field>>,
    pub faces: usize,
    pub corners: usize,
}

fn normal_key(normal: [f32; 3]) -> [u32; 3] {
    normal.map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

struct Source {
    center: DVec3,
    points: Vec<DVec3>,
    normal: DVec3,
    groups: u32,
}

fn compatible(source: &Source, other: &Source, threshold: Option<f64>) -> bool {
    if source.groups == 0 && other.groups == 0 {
        threshold.is_some_and(|threshold| source.normal.dot(other.normal) + 1e-6 >= threshold)
    } else {
        let common = source.groups & other.groups;
        common != 0 && common & HARD_GROUPS == 0
    }
}

struct Context<'a> {
    retained_points: usize,
    faces: &'a [Face],
    bindings: &'a [Option<Arc<crate::faces::SmoothingSource>>],
    planes: Vec<source_formats::bsp::Plane>,
    vertices: Vec<[f32; 3]>,
    edges: Vec<Edge>,
    surfedges: Vec<i32>,
    indices: Vec<u16>,
    normals: Vec<[f32; 3]>,
    interned: HashMap<[u32; 3], u16>,
}

pub(super) fn build(
    bsp: &mut Bsp,
    faces: &[Face],
    bindings: &[Option<Arc<crate::faces::SmoothingSource>>],
    scopes: &[super::models::Scope],
    smoothing_angle: u8,
    budget: &mut Budget<'_>,
) -> Result<Plan, String> {
    if bindings.len() != faces.len() {
        return Err("Brush smoothing source bindings do not match emitted faces".into());
    }
    if smoothing_angle == 0
        && !faces
            .iter()
            .any(|face| face.dispinfo < 0 && face.smoothing_groups != 0)
    {
        budget.consume(faces.len())?;
        return Ok(Plan::default());
    }
    let mut context = Context {
        retained_points: 0,
        faces,
        bindings,
        planes: bsp
            .records::<source_formats::bsp::Plane>()
            .map_err(|error| error.to_string())?,
        vertices: bsp.vertices().map_err(|error| error.to_string())?,
        edges: bsp.records::<Edge>().map_err(|error| error.to_string())?,
        surfedges: bsp.surfedges().map_err(|error| error.to_string())?,
        indices: bsp
            .values::<u16>(lump::VERTNORMALINDICES)
            .map_err(|error| error.to_string())?,
        normals: bsp
            .values::<[f32; 3]>(lump::VERTNORMALS)
            .map_err(|error| error.to_string())?,
        interned: HashMap::new(),
    };
    if context.indices.len() != context.surfedges.len() || context.normals.len() > MAX_NORMALS {
        return Err("Brush smoothing has invalid native normal arrays".into());
    }
    budget.consume(context.normals.len())?;
    for (index, &normal) in context.normals.iter().enumerate() {
        context
            .interned
            .entry(normal_key(normal))
            .or_insert(index as u16);
    }
    let mut result = Plan::default();
    for scope in scopes {
        let plan = context.scope(scope.faces.clone(), smoothing_angle, budget)?;
        result.faces += plan.faces;
        result.corners += plan.corners;
        result.fields.extend(plan.fields);
    }
    if result.faces != 0 {
        bsp.set_values(lump::VERTNORMALS, &context.normals);
        bsp.set_values(lump::VERTNORMALINDICES, &context.indices);
    }
    Ok(result)
}

impl Context<'_> {
    fn scope(
        &mut self,
        range: std::ops::Range<usize>,
        smoothing_angle: u8,
        budget: &mut Budget<'_>,
    ) -> Result<Plan, String> {
        let Self {
            retained_points,
            faces,
            bindings,
            planes,
            vertices,
            edges,
            surfedges,
            indices,
            normals,
            interned,
        } = self;
        let world = faces
            .get(range.clone())
            .ok_or("Brush smoothing scope has an invalid face range")?;
        budget.consume(world.len())?;
        if smoothing_angle == 0
            && !world
                .iter()
                .any(|face| face.dispinfo < 0 && face.smoothing_groups != 0)
        {
            return Ok(Plan::default());
        }
        let mut source_ids = HashMap::new();
        let mut sources = Vec::new();
        let mut targets = Vec::new();
        for (local, face) in world.iter().enumerate() {
            budget.consume(1)?;
            let index = range.start + local;
            let Some(source) = &bindings[index] else {
                continue;
            };
            if face.dispinfo >= 0 || face.smoothing_groups == 0 && smoothing_angle == 0 {
                continue;
            }
            let key = Arc::as_ptr(source) as usize;
            let source_id = if let Some(&source_id) = source_ids.get(&key) {
                source_id
            } else {
                let source_id = sources.len();
                budget.consume(source.winding.0.len())?;
                let normal = planes
                    .get(source.plane)
                    .ok_or("Brush smoothing source has an invalid plane")?
                    .normal;
                let normal = DVec3::from_array(normal.map(f64::from))
                    .try_normalize()
                    .ok_or("Brush smoothing has a degenerate source normal")?;
                let points = source
                    .winding
                    .0
                    .iter()
                    .map(|&point| crate::emit::stored_vertex(point).as_vec3().to_array())
                    .collect::<Vec<_>>();
                let points = points
                    .into_iter()
                    .map(|point| DVec3::from_array(point.map(f64::from)))
                    .collect::<Vec<_>>();
                sources.push(Source {
                    center: points.iter().sum::<DVec3>() / points.len() as f64,
                    points,
                    normal,
                    groups: source.groups,
                });
                source_ids.insert(key, source_id);
                source_id
            };
            targets.push((index, source_id));
        }
        let threshold =
            (smoothing_angle != 0).then(|| f64::from(smoothing_angle).to_radians().cos());
        junctions::insert(&mut sources, threshold, retained_points, budget)?;
        let mut references: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
        for (source_id, source) in sources.iter().enumerate() {
            budget.consume(source.points.len())?;
            for &point in &source.points {
                let neighbors = references
                    .entry(normal_key(point.as_vec3().to_array()))
                    .or_default();
                if neighbors.last() != Some(&source_id) {
                    neighbors.push(source_id);
                }
            }
        }
        let mut fields = Vec::with_capacity(sources.len());
        for source in &sources {
            let mut shading = Vec::with_capacity(source.points.len());
            let mut changed = false;
            for &point in &source.points {
                let neighbors = &references[&normal_key(point.as_vec3().to_array())];
                budget.consume(neighbors.len())?;
                let mut sum = DVec3::ZERO;
                for &neighbor in neighbors {
                    let other = &sources[neighbor];
                    if std::ptr::eq(source, other) || compatible(source, other, threshold) {
                        sum += other.normal;
                    }
                }
                let normal = sum
                    .try_normalize()
                    .filter(|normal| normal.dot(source.normal) > NORMAL_EPSILON)
                    .unwrap_or(source.normal);
                changed |= normal.distance_squared(source.normal) > NORMAL_EPSILON * NORMAL_EPSILON;
                shading.push(normal);
            }
            fields.push(changed.then(|| {
                Arc::new(Field {
                    center: source.center,
                    points: source.points.clone(),
                    corners: shading,
                })
            }));
        }
        let mut result = Plan::default();
        for (index, source_id) in targets {
            let Some(field) = &fields[source_id] else {
                continue;
            };
            let face = &faces[index];
            let base = sources[source_id].normal;
            let first = usize::try_from(face.first_edge)
                .map_err(|_| "Brush smoothing has a negative edge offset")?;
            let count = usize::try_from(face.edge_count)
                .map_err(|_| "Brush smoothing has a negative edge count")?;
            let face_edges = surfedges
                .get(first..first + count)
                .ok_or("Brush smoothing face exceeds its edges")?;
            let mut changed = 0;
            for (corner, &edge) in face_edges.iter().enumerate() {
                budget.consume(field.point_count() + 1)?;
                let pair = edges
                    .get(edge.unsigned_abs() as usize)
                    .ok_or("Brush smoothing has an invalid edge")?
                    .0;
                let point = vertices
                    .get(pair[usize::from(edge < 0)] as usize)
                    .ok_or("Brush smoothing has an invalid vertex")?;
                let normal = field.sample(DVec3::from_array(point.map(f64::from)), base);
                changed +=
                    usize::from(normal.distance_squared(base) > NORMAL_EPSILON * NORMAL_EPSILON);
                let stored = normal.as_vec3().to_array();
                let key = normal_key(stored);
                let id = if let Some(&id) = interned.get(&key) {
                    id
                } else {
                    let id = u16::try_from(normals.len()).map_err(
                        |_| "Brush smoothing exceeds the 65,536 native vertex-normal budget",
                    )?;
                    normals.push(stored);
                    interned.insert(key, id);
                    id
                };
                indices[first + corner] = id;
            }
            result.fields.insert(index, field.clone());
            result.faces += 1;
            result.corners += changed;
        }
        Ok(result)
    }
}
