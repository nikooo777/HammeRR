use glam::{DVec2, DVec3};
use source_formats::bsp::{Bsp, Edge, LightingKind, Model, RgbExp32, TexInfo, surf};
use std::cell::Cell;
use std::ops::ControlFlow;
use std::sync::Arc;

use super::Occluder;
use super::bake::Budget;
use super::shadows::{ShadowIndex, ShadowObject, Triangle, Visit};
use super::terrain_shadows::TerrainTriangle;
use super::transport::{Coverage, Scene};
use crate::map::Map;

const LOCAL_FILTER_START: f64 = 20.0;
const LOCAL_FILTER_END: f64 = 40.0;
const SOURCE_ROUNDING_OPERATIONS: f64 = 64.0;
const SOURCE_INTERSECTION_WORK: usize = 48;
const SOURCE_FEATURE_WORK: usize = 64;
const SOURCE_TRANSPORT_WORK: usize = 24;
const SOURCE_SAMPLE_WORK: usize = 30;

struct Surface {
    geometry: Geometry,
    normal: DVec3,
    source: usize,
    ordinal: usize,
    coordinates: Option<[DVec2; 3]>,
    boundary_error: f64,
}

enum Geometry {
    Brush { fan: Arc<[Triangle]>, index: usize },
    Terrain(TerrainTriangle),
}

impl Geometry {
    fn intersection(
        &self,
        start: DVec3,
        end: DVec3,
        pending: &Cell<usize>,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        match self {
            Self::Brush { fan, index } => {
                let Some(hit) = fan[*index].topological_intersection(start, end, 0.0)? else {
                    return Ok(None);
                };
                if hit.1[2] == 0.0 {
                    let central = hit.1[1] == 0.0;
                    let first = if central { 0 } else { index.saturating_sub(1) };
                    for previous in &fan[first..*index] {
                        pending.set(
                            pending
                                .get()
                                .checked_add(SOURCE_INTERSECTION_WORK)
                                .ok_or("Radiance fan ownership work overflow")?,
                        );
                        if previous
                            .topological_intersection(start, end, 0.0)?
                            .is_some_and(|(_, weights)| {
                                weights[1] == 0.0 && (!central || weights[2] == 0.0)
                            })
                        {
                            return Ok(None);
                        }
                    }
                }
                Ok(Some(hit))
            }
            Self::Terrain(triangle) => triangle.source_intersection(start, end),
        }
    }

    fn point(&self, weights: [f64; 3]) -> DVec3 {
        match self {
            Self::Brush { fan, index } => fan[*index].point(weights),
            Self::Terrain(triangle) => triangle.point(weights),
        }
    }

    fn texture_point(&self, weights: [f64; 3]) -> DVec3 {
        match self {
            Self::Brush { fan, index } => fan[*index].point(weights),
            Self::Terrain(triangle) => triangle.texture_point(weights),
        }
    }
}

struct Radiance {
    average: DVec3,
    reflectivity: DVec3,
    samples: Vec<RgbExp32>,
    dimensions: [usize; 2],
    projection: [[f32; 4]; 2],
    mins: [i32; 2],
    sky: bool,
    extras: Vec<Layer>,
    boundary: Option<SourceBoundary>,
    alpha: Option<super::brush_alpha::Face>,
}

pub(super) struct SourceBoundary {
    pub(super) plane: usize,
    pub(super) equation: map_core::Plane,
    pub(super) sides: Arc<[u32]>,
    pub(super) owners: Vec<usize>,
}

impl SourceBoundary {
    #[allow(clippy::too_many_arguments)]
    fn crossing(
        &self,
        hull: &Occluder,
        interval: super::HullInterval,
        start: DVec3,
        end: DVec3,
        point: DVec3,
        error: f64,
    ) -> Result<(Option<(bool, DVec3)>, usize), String> {
        let Some(source) = &hull.source else {
            return Ok((None, 1));
        };
        let mut work = self.owners.len().checked_ilog2().unwrap_or(0) as usize + 1;
        if self.owners.binary_search(&source.brush).is_err() {
            return Ok((None, work));
        }
        for (identity, plane) in source.sides.iter().zip(&hull.planes) {
            work += self.sides.len().checked_ilog2().unwrap_or(0) as usize + 2;
            if identity.plane != self.plane
                || !identity
                    .side
                    .is_some_and(|side| self.sides.binary_search(&side).is_ok())
                || plane.normal != self.equation.normal
                || plane.dist != self.equation.dist
            {
                continue;
            }
            work += 24;
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            let distance = plane.distance_to(point);
            let scale = (plane.normal * point).abs().element_sum() + plane.dist.abs() + 1.0;
            let allowance = error + scale * (SOURCE_ROUNDING_OPERATIONS * f64::EPSILON);
            if !a.is_finite() || !b.is_finite() || !distance.is_finite() || !allowance.is_finite() {
                return Err("Radiance source crossing arithmetic must be finite".into());
            }
            if (a > 0.0) == (b > 0.0) || distance.abs() > allowance {
                continue;
            }
            let entering = a > 0.0;
            let fraction = a / (a - b);
            let boundary = if entering {
                interval.entering
            } else {
                interval.leaving
            };
            if fraction > 0.0
                && fraction < 1.0
                && (fraction - boundary).abs() <= super::shadows::TRIANGLE_EPSILON
            {
                return Ok((Some((entering, start.lerp(end, fraction))), work));
            }
        }
        Ok((None, work))
    }

    fn terminal(
        &self,
        hull: &Occluder,
        interval: super::HullInterval,
        start: DVec3,
        end: DVec3,
        error: f64,
    ) -> Result<(bool, usize), String> {
        let Some(source) = &hull.source else {
            return Ok((false, 1));
        };
        let mut work = self.owners.len().checked_ilog2().unwrap_or(0) as usize + 1;
        if interval.entering <= 0.0
            || interval.leaving <= interval.entering
            || self.owners.binary_search(&source.brush).is_err()
        {
            return Ok((false, work));
        }
        for (identity, plane) in source.sides.iter().zip(&hull.planes) {
            work += self.sides.len().checked_ilog2().unwrap_or(0) as usize + 2;
            if identity.plane != self.plane
                || !identity
                    .side
                    .is_some_and(|side| self.sides.binary_search(&side).is_ok())
            {
                continue;
            }
            if plane.normal != self.equation.normal || plane.dist != self.equation.dist {
                continue;
            }
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            let scale = (plane.normal * end).abs().element_sum() + plane.dist.abs() + 1.0;
            let allowance = error + scale * (SOURCE_ROUNDING_OPERATIONS * f64::EPSILON);
            if !a.is_finite() || !b.is_finite() || !allowance.is_finite() {
                return Err("Radiance source boundary arithmetic must be finite".into());
            }
            if a > 0.0 && b <= 0.0 && b.abs() <= allowance {
                return Ok((true, work));
            }
        }
        Ok((false, work))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn source_coverage(
    shadows: &ShadowIndex<Occluder>,
    start: DVec3,
    end: DVec3,
    boundary: Option<&SourceBoundary>,
    error: f64,
    coverage: &mut Coverage,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let pending = Cell::new(0usize);
    let result = shadows.visit(
        start,
        end,
        &mut |work| {
            budget.consume(
                work.checked_add(pending.replace(0))
                    .ok_or("Radiance source certificate work overflow")?,
            )
        },
        &mut |hull| {
            let Some(interval) = hull.interval(start, end)? else {
                return Ok(Visit::Continue(1.0));
            };
            pending.set(
                pending
                    .get()
                    .checked_add(hull.plane_count() + 1)
                    .ok_or("Radiance source interval work overflow")?,
            );
            let terminal = if let Some(boundary) = boundary {
                let (terminal, work) = boundary.terminal(hull, interval, start, end, error)?;
                pending.set(
                    pending
                        .get()
                        .checked_add(work)
                        .ok_or("Radiance source certificate work overflow")?,
                );
                terminal
            } else {
                false
            };
            for event in hull.optical_boundaries(start, end)?.into_iter().flatten() {
                if terminal && event.fraction == interval.entering {
                    continue;
                }
                coverage.add(event.coverage);
            }
            Ok(if coverage.opaque() {
                Visit::Stop
            } else {
                Visit::Continue(1.0)
            })
        },
    );
    budget.consume(pending.replace(0))?;
    result.map(|_| ())
}

struct Layer {
    style: u8,
    average: DVec3,
    samples: Vec<RgbExp32>,
}

impl Radiance {
    fn at(&self, point: DVec3, uv: Option<DVec2>, distance: f64, cone: f64) -> DVec3 {
        self.at_layer(&self.samples, self.average, point, uv, distance, cone)
    }

    fn at_layer(
        &self,
        samples: &[RgbExp32],
        average: DVec3,
        point: DVec3,
        uv: Option<DVec2>,
        distance: f64,
        cone: f64,
    ) -> DVec3 {
        let blend = ((distance * cone - LOCAL_FILTER_START)
            / (LOCAL_FILTER_END - LOCAL_FILTER_START))
            .clamp(0.0, 1.0);
        if samples.is_empty() || blend == 1.0 {
            return average;
        }
        let coordinates: [f64; 2] = std::array::from_fn(|axis| {
            let coordinate = if let Some(uv) = uv {
                uv[axis] * (self.dimensions[axis] - 1) as f64
            } else {
                let row = self.projection[axis];
                DVec3::from_array([row[0], row[1], row[2]].map(f64::from)).dot(point)
                    + f64::from(row[3])
                    - f64::from(self.mins[axis])
            };
            coordinate.clamp(0.0, (self.dimensions[axis] - 1) as f64)
        });
        let lower = coordinates.map(|value| value.floor() as usize);
        let upper: [usize; 2] =
            std::array::from_fn(|axis| (lower[axis] + 1).min(self.dimensions[axis] - 1));
        let color =
            |x: usize, y: usize| DVec3::from_array(samples[y * self.dimensions[0] + x].linear());
        let weight = [
            coordinates[0] - lower[0] as f64,
            coordinates[1] - lower[1] as f64,
        ];
        let near = color(lower[0], lower[1]).lerp(color(upper[0], lower[1]), weight[0]);
        let far = color(lower[0], upper[1]).lerp(color(upper[0], upper[1]), weight[0]);
        (near.lerp(far, weight[1]) * self.reflectivity).lerp(average, blend)
    }
}

pub(super) struct SurfaceIndex {
    geometry: ShadowIndex<Surface>,
    radiance: Vec<Radiance>,
    crossings: usize,
    _allocation: super::bake::Allocation,
}

impl ShadowObject for Surface {
    const MAX_OBJECTS: usize = Triangle::MAX_OBJECTS;
    fn bounds(&self) -> (DVec3, DVec3) {
        match &self.geometry {
            Geometry::Brush { fan, index } => fan[*index].bounds(),
            Geometry::Terrain(triangle) => triangle.bounds(),
        }
    }
    fn work(&self) -> usize {
        match &self.geometry {
            Geometry::Brush { .. } => SOURCE_INTERSECTION_WORK + 32,
            Geometry::Terrain(triangle) => triangle.work() + 32,
        }
    }
    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        Ok(self
            .geometry
            .intersection(start, end, &Cell::new(0))?
            .is_some())
    }
}

#[allow(clippy::too_many_arguments)]
fn add_surface(
    surfaces: &mut Vec<Surface>,
    points: [DVec3; 3],
    source: usize,
    coordinates: Option<[DVec2; 3]>,
    boundary: Option<&SourceBoundary>,
    geometry: Geometry,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    budget.consume(1)?;
    if (points[1] - points[0])
        .cross(points[2] - points[0])
        .length_squared()
        == 0.0
    {
        return Ok(());
    }
    if surfaces.len() == Surface::MAX_OBJECTS {
        return Err("Surface radiance surface count exceeds the bounded triangle limit".into());
    }
    let boundary_error = boundary.map_or(0.0, |boundary| {
        points
            .iter()
            .map(|&point| boundary.equation.distance_to(point).abs())
            .fold(0.0_f64, f64::max)
    });
    if !boundary_error.is_finite() {
        return Err("Radiance native source support must be finite".into());
    }
    surfaces.push(Surface {
        geometry,
        normal: (points[0] - points[1])
            .cross(points[2] - points[1])
            .normalize(),
        source,
        ordinal: surfaces.len(),
        coordinates,
        boundary_error,
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    sky_radiance: DVec3,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain: &ShadowIndex<TerrainTriangle>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(
        map,
        bsp,
        kind,
        sky_radiance,
        false,
        face_sources,
        brush_textures,
        terrain,
        budget,
    )
}

pub(super) fn build_styled(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain: &ShadowIndex<TerrainTriangle>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(
        map,
        bsp,
        kind,
        DVec3::ZERO,
        true,
        face_sources,
        brush_textures,
        terrain,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_styled_sky(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    sky_radiance: DVec3,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain: &ShadowIndex<TerrainTriangle>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(
        map,
        bsp,
        kind,
        sky_radiance,
        true,
        face_sources,
        brush_textures,
        terrain,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_with_styles(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    sky_radiance: DVec3,
    styled: bool,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain_shadows: &ShadowIndex<TerrainTriangle>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    let decoded_bytes = [
        source_formats::bsp::lump::FACES,
        source_formats::bsp::lump::FACES_HDR,
        source_formats::bsp::lump::TEXINFO,
        source_formats::bsp::lump::VERTEXES,
        source_formats::bsp::lump::EDGES,
        source_formats::bsp::lump::SURFEDGES,
        source_formats::bsp::lump::MODELS,
    ]
    .into_iter()
    .try_fold(0usize, |total, index| {
        total.checked_add(bsp.lumps[index].data.len())
    })
    .ok_or("Radiance geometry allocation size overflow")?;
    let _decoded_allocation = budget.reserve_bytes(decoded_bytes)?;
    let world = bsp
        .records::<Model>()
        .map_err(|error| error.to_string())?
        .first()
        .copied()
        .ok_or("Surface radiance needs a world model")?;
    let faces = bsp
        .lighting_faces(kind)
        .map_err(|error| error.to_string())?;
    if face_sources.len() != faces.len() {
        return Err("Radiance contributors do not match the emitted face count".into());
    }
    let infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    let lighting = bsp.lighting(kind).map_err(|error| error.to_string())?;
    let world_faces =
        &faces[world.first_face as usize..(world.first_face + world.face_count) as usize];
    let source_side_count = map.entities[0]
        .brushes
        .iter()
        .try_fold(0usize, |count, &index| {
            count.checked_add(map.brushes[index].sides.len())
        })
        .ok_or("Radiance source-side lookup size overflow")?;
    let _source_lookup = budget.reserve::<(u32, usize)>(source_side_count)?;
    budget.consume(source_side_count)?;
    let mut source_owners = Vec::with_capacity(source_side_count);
    for &owner in &map.entities[0].brushes {
        for side in &map.brushes[owner].sides {
            if !side.bevel
                && let Some(id) = side.source_id
            {
                source_owners.push((id, owner));
            }
        }
    }
    budget.consume(
        source_owners
            .len()
            .checked_mul(source_owners.len().checked_ilog2().unwrap_or(0) as usize + 1)
            .ok_or("Radiance source-side sort work overflow")?,
    )?;
    source_owners.sort_unstable_by_key(|&(side, _)| side);
    if source_owners.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("Radiance source-side ownership is ambiguous".into());
    }
    let contributor_count = face_sources
        [world.first_face as usize..(world.first_face + world.face_count) as usize]
        .iter()
        .try_fold(0usize, |count, sides| count.checked_add(sides.len()))
        .ok_or("Radiance contributor allocation size overflow")?;
    let triangles = world_faces
        .iter()
        .try_fold(0usize, |total, face| {
            let count = if face.dispinfo >= 0 {
                map.terrains[face.dispinfo as usize].indices.len()
            } else {
                (face.edge_count as usize).saturating_sub(2)
            };
            total
                .checked_add(count)
                .filter(|&total| total <= Surface::MAX_OBJECTS)
        })
        .ok_or("Surface radiance exceeds its bounded triangle allocation")?;
    let brush_triangles = world_faces
        .iter()
        .filter(|face| face.dispinfo < 0)
        .try_fold(0usize, |count, face| {
            count.checked_add((face.edge_count as usize).saturating_sub(2))
        })
        .ok_or("Radiance brush fan allocation size overflow")?;
    let bytes = ShadowIndex::<Surface>::allocation_bytes(triangles)?
        .checked_add(brush_triangles * size_of::<Triangle>())
        .and_then(|bytes| bytes.checked_add(world_faces.len() * 2 * size_of::<usize>()))
        .and_then(|bytes| {
            bytes.checked_add(world_faces.len() * (size_of::<Radiance>() + 3 * size_of::<Layer>()))
        })
        .and_then(|bytes| bytes.checked_add(source_formats::bsp::MAX_LIGHTING_BYTES))
        .and_then(|bytes| {
            contributor_count
                .checked_mul(size_of::<usize>())
                .and_then(|owners| bytes.checked_add(owners))
        })
        .ok_or("Surface radiance allocation size overflow")?;
    let allocation = budget.reserve_bytes(bytes)?;
    let mut surfaces = Vec::with_capacity(triangles);
    let mut radiance = Vec::with_capacity(world_faces.len());
    for (relative, face) in world_faces.iter().enumerate() {
        budget.consume(1)?;
        let info = infos[face.texinfo as usize];
        let sky = info.flags & (surf::SKY | surf::SKY2D) != 0;
        let light = lighting
            .as_ref()
            .map(|lighting| lighting.face(face, &info))
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        let reflectivity = if sky {
            DVec3::ONE
        } else {
            let material = map.materials.get(map.texdata[info.texdata as usize]);
            let reflectivity =
                DVec3::from_array(material.texture_metrics()?.reflectivity.map(f64::from));
            if !reflectivity.is_finite() || reflectivity.min_element() < 0.0 {
                return Err(format!(
                    "Material {} has invalid surface reflectivity",
                    material.name
                ));
            }
            reflectivity / 255.0
        };
        let average = if sky {
            sky_radiance
        } else {
            light
                .as_ref()
                .and_then(|light| light.average(0))
                .map_or(DVec3::ZERO, |color| DVec3::from_array(color.linear()))
                * reflectivity
        };
        if !average.is_finite() || average.min_element() < 0.0 {
            return Err("Surface radiance is nonfinite or negative".into());
        }
        let samples = if !sky {
            light
                .as_ref()
                .and_then(|light| light.samples(0, 0))
                .unwrap_or_default()
        } else {
            &[]
        };
        budget.consume(samples.len())?;
        let mut extras = Vec::with_capacity(3);
        if styled
            && !sky
            && let Some(light) = &light
        {
            for (index, &style) in light.styles().iter().enumerate().skip(1) {
                let samples = light
                    .samples(index, 0)
                    .ok_or("Styled radiance base map is missing")?;
                budget.consume(samples.len() + 1)?;
                let average = light
                    .average(index)
                    .ok_or("Styled radiance average is missing")?;
                extras.push(Layer {
                    style,
                    average: DVec3::from_array(average.linear()) * reflectivity,
                    samples: samples.to_vec(),
                });
            }
        }
        let boundary = if face.dispinfo < 0 {
            if face.side != (face.plane & 1) as u8 || usize::from(face.plane) >= map.planes.len() {
                return Err("Radiance source does not use its emitted outward plane".into());
            }
            let sides = &face_sources[world.first_face as usize + relative];
            budget.consume(sides.len() + 1)?;
            if sides.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err("Radiance source contributors must be sorted and unique".into());
            }
            let mut owners = Vec::with_capacity(sides.len());
            for &id in sides.iter() {
                budget.consume(source_owners.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                let index = source_owners
                    .binary_search_by_key(&id, |&(side, _)| side)
                    .map_err(|_| "Radiance source contributor has no world-brush owner")?;
                owners.push(source_owners[index].1);
            }
            budget.consume(
                owners
                    .len()
                    .checked_mul(owners.len().checked_ilog2().unwrap_or(0) as usize + 1)
                    .ok_or("Radiance contributor sort work overflow")?,
            )?;
            owners.sort_unstable();
            owners.dedup();
            let plane = map.planes.get(usize::from(face.plane));
            Some(SourceBoundary {
                plane: usize::from(face.plane),
                equation: map_core::Plane {
                    normal: plane.normal,
                    dist: plane.dist,
                },
                sides: sides.clone(),
                owners,
            })
        } else {
            None
        };
        let source = radiance.len();
        let material = if face.dispinfo >= 0 {
            map.terrains[face.dispinfo as usize].material
        } else {
            map.texdata[info.texdata as usize]
        };
        radiance.push(Radiance {
            alpha: brush_textures.and_then(|textures| textures.face(material, info.texture_vecs)),
            average,
            reflectivity,
            samples: samples.to_vec(),
            dimensions: light.as_ref().map_or([0; 2], |light| light.dimensions()),
            projection: info.lightmap_vecs,
            mins: face.lightmap_mins,
            sky,
            extras,
            boundary,
        });
        if face.dispinfo >= 0 {
            let terrain = &map.terrains[face.dispinfo as usize];
            budget.consume(terrain.vertices.len())?;
            let size = terrain
                .info
                .grid_size()
                .map_err(|error| error.to_string())?;
            for (index, indices) in terrain.indices.iter().enumerate() {
                let objects = terrain_shadows.objects();
                budget.consume(objects.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                let shadow = objects
                    .binary_search_by_key(&(face.dispinfo as usize, index), |triangle| {
                        triangle.source()
                    })
                    .map_err(|_| "Radiance terrain source has no prepared shadow geometry")?;
                let triangle = objects[shadow].clone();
                let points = [
                    triangle.point([1.0, 0.0, 0.0]),
                    triangle.point([0.0, 1.0, 0.0]),
                    triangle.point([0.0, 0.0, 1.0]),
                ];
                add_surface(
                    &mut surfaces,
                    points,
                    source,
                    Some(indices.map(|index| {
                        DVec2::new((index % size) as f64, (index / size) as f64) / (size - 1) as f64
                    })),
                    None,
                    Geometry::Terrain(triangle),
                    budget,
                )?;
            }
        } else {
            budget.consume(face.edge_count as usize)?;
            let points = surfedges
                [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
                .iter()
                .map(|&edge| {
                    let pair = edges[edge.unsigned_abs() as usize].0;
                    DVec3::from_array(vertices[pair[usize::from(edge < 0)] as usize].map(f64::from))
                })
                .collect::<Vec<_>>();
            let _fan_scratch = budget.reserve::<Triangle>(points.len().saturating_sub(2))?;
            let mut fan = Vec::with_capacity(points.len().saturating_sub(2));
            for index in 1..points.len().saturating_sub(1) {
                let points = [points[0], points[index], points[index + 1]];
                budget.consume(1)?;
                if (points[1] - points[0])
                    .cross(points[2] - points[0])
                    .length_squared()
                    == 0.0
                {
                    continue;
                }
                fan.push(Triangle::new(points)?);
            }
            let fan = Arc::<[Triangle]>::from(fan);
            for (index, triangle) in fan.iter().enumerate() {
                let points = [
                    triangle.point([1.0, 0.0, 0.0]),
                    triangle.point([0.0, 1.0, 0.0]),
                    triangle.point([0.0, 0.0, 1.0]),
                ];
                add_surface(
                    &mut surfaces,
                    points,
                    source,
                    None,
                    radiance[source].boundary.as_ref(),
                    Geometry::Brush {
                        fan: Arc::clone(&fan),
                        index,
                    },
                    budget,
                )?;
            }
        }
    }
    Ok(SurfaceIndex {
        crossings: surfaces.len(),
        geometry: ShadowIndex::new(surfaces, &mut |work| budget.consume(work))?,
        radiance,
        _allocation: allocation,
    })
}

pub(super) struct Hit {
    pub color: DVec3,
    pub point: DVec3,
    pub sky: bool,
    pub front: bool,
    source: usize,
    uv: Option<DVec2>,
    distance: f64,
    visibility: DVec3,
}

struct Crossing {
    fraction: f64,
    ordinal: usize,
    source: usize,
    uv: Option<DVec2>,
    front: bool,
    point: DVec3,
    boundary_error: f64,
    coverage: Coverage,
    weights: [f64; 3],
}

struct Feature {
    points: [DVec3; 2],
    count: usize,
}

impl Feature {
    fn shared(&self, other: &Self) -> bool {
        match (self.count, other.count) {
            (1, 1) => self.points[0] == other.points[0],
            (1, 2) => on_segment(self.points[0], other.points),
            (2, 1) => on_segment(other.points[0], self.points),
            (2, 2) => {
                (self.points[1] - self.points[0]).cross(other.points[1] - other.points[0])
                    == DVec3::ZERO
                    && (on_segment(self.points[0], other.points)
                        || on_segment(self.points[1], other.points)
                        || on_segment(other.points[0], self.points)
                        || on_segment(other.points[1], self.points))
            }
            _ => false,
        }
    }
}

fn on_segment(point: DVec3, [start, end]: [DVec3; 2]) -> bool {
    let edge = end - start;
    let offset = point - start;
    let distance = offset.dot(edge);
    offset.cross(edge) == DVec3::ZERO && distance >= 0.0 && distance <= edge.length_squared()
}

fn shares_owner(left: &SourceBoundary, right: &SourceBoundary) -> bool {
    let mut left = left.owners.iter().peekable();
    let mut right = right.owners.iter().peekable();
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        match a.cmp(b) {
            std::cmp::Ordering::Less => {
                left.next();
            }
            std::cmp::Ordering::Greater => {
                right.next();
            }
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

fn pending_work(pending: &Cell<usize>, work: usize) -> Result<(), String> {
    pending.set(
        pending
            .get()
            .checked_add(work)
            .ok_or("Radiance source work overflow")?,
    );
    Ok(())
}

fn transport_bytes(amount: DVec3) -> [u8; 3] {
    amount
        .to_array()
        .map(|channel| (channel * f64::from(u8::MAX)).round() as u8)
}

impl SurfaceIndex {
    fn crossing(
        &self,
        surface: &Surface,
        fraction: f64,
        weights: [f64; 3],
        delta: DVec3,
        pending: &Cell<usize>,
    ) -> Result<Crossing, String> {
        let radiance = &self.radiance[surface.source];
        let coverage = if !radiance.sky && radiance.alpha.is_some() && radiance.boundary.is_some() {
            Coverage::CLEAR
        } else if let Some(alpha) = &radiance.alpha {
            pending_work(pending, SOURCE_SAMPLE_WORK)?;
            alpha.coverage(surface.geometry.texture_point(weights))?
        } else {
            Coverage::OPAQUE
        };
        let uv = surface.coordinates.map(|coordinates| {
            coordinates
                .into_iter()
                .zip(weights)
                .map(|(point, weight)| point * weight)
                .sum::<DVec2>()
        });
        Ok(Crossing {
            fraction,
            ordinal: surface.ordinal,
            source: surface.source,
            uv,
            front: surface.normal.dot(-delta) > 0.0,
            point: surface.geometry.point(weights),
            boundary_error: surface.boundary_error * weights.into_iter().map(f64::abs).sum::<f64>(),
            coverage,
            weights,
        })
    }

    fn feature(&self, crossing: &Crossing) -> Option<Feature> {
        let geometry = &self.geometry.objects()[crossing.ordinal].geometry;
        if !matches!(geometry, Geometry::Brush { .. }) {
            return None;
        }
        let mut feature = Feature {
            points: [DVec3::ZERO; 2],
            count: 0,
        };
        for (corner, weight) in crossing.weights.into_iter().enumerate() {
            if weight == 0.0 {
                continue;
            }
            if feature.count == feature.points.len() {
                return None;
            }
            let mut weights = [0.0; 3];
            weights[corner] = 1.0;
            feature.points[feature.count] = geometry.point(weights);
            feature.count += 1;
        }
        Some(feature)
    }

    fn source_event(
        &self,
        shadows: &ShadowIndex<Occluder>,
        start: DVec3,
        end: DVec3,
        crossing: &Crossing,
        other: Option<&Crossing>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Coverage>, String> {
        let Some(boundary) = &self.radiance[crossing.source].boundary else {
            return Ok(None);
        };
        let pending = Cell::new(0usize);
        let mut coverage = None;
        let result = shadows.visit(
            start,
            end,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending.replace(0))
                        .ok_or("Radiance source event work overflow")?,
                )
            },
            &mut |hull| {
                let Some(interval) = hull.interval(start, end)? else {
                    return Ok(Visit::Continue(1.0));
                };
                let (event, work) = boundary.crossing(
                    hull,
                    interval,
                    start,
                    end,
                    crossing.point,
                    crossing.boundary_error,
                )?;
                pending_work(&pending, work)?;
                let Some((event, point)) = event else {
                    return Ok(Visit::Continue(1.0));
                };
                let (source, point) = if let Some(other) = other {
                    let Some(boundary) = &self.radiance[other.source].boundary else {
                        return Ok(Visit::Continue(1.0));
                    };
                    let (other_event, work) = boundary.crossing(
                        hull,
                        interval,
                        start,
                        end,
                        other.point,
                        other.boundary_error,
                    )?;
                    pending_work(&pending, work)?;
                    let Some((other_event, point)) = other_event else {
                        return Ok(Visit::Continue(1.0));
                    };
                    if other_event != event {
                        return Ok(Visit::Continue(1.0));
                    }
                    (other.source, point)
                } else {
                    (crossing.source, point)
                };
                coverage = Some(if let Some(alpha) = &self.radiance[source].alpha {
                    pending_work(&pending, SOURCE_SAMPLE_WORK)?;
                    alpha.coverage(point)?
                } else {
                    Coverage::OPAQUE
                });
                Ok(Visit::Stop)
            },
        );
        budget.consume(pending.replace(0))?;
        result?;
        Ok(coverage)
    }

    #[allow(clippy::too_many_arguments)]
    fn next_shared_crossing(
        &self,
        start: DVec3,
        end: DVec3,
        crossing: &Crossing,
        feature: &Feature,
        previous: Option<usize>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Crossing>, String> {
        let Some(boundary) = &self.radiance[crossing.source].boundary else {
            return Ok(None);
        };
        let pending = Cell::new(0usize);
        let mut first: Option<Crossing> = None;
        let result = self.geometry.visit_bounds(
            crossing.point,
            crossing.point,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending.replace(0))
                        .ok_or("Radiance source ownership work overflow")?,
                )
            },
            &mut |_, surface| {
                if surface.source == crossing.source
                    || previous.is_some_and(|previous| surface.ordinal <= previous)
                    || first
                        .as_ref()
                        .is_some_and(|first| surface.ordinal >= first.ordinal)
                {
                    return Ok(ControlFlow::Continue(()));
                }
                let radiance = &self.radiance[surface.source];
                let Some(other) = &radiance.boundary else {
                    return Ok(ControlFlow::Continue(()));
                };
                if radiance.sky {
                    return Ok(ControlFlow::Continue(()));
                }
                pending_work(
                    &pending,
                    boundary.owners.len() + other.owners.len() + surface.work(),
                )?;
                if !shares_owner(boundary, other) {
                    return Ok(ControlFlow::Continue(()));
                }
                let Some((fraction, weights)) =
                    surface.geometry.intersection(start, end, &pending)?
                else {
                    return Ok(ControlFlow::Continue(()));
                };
                let other = self.crossing(surface, fraction, weights, end - start, &pending)?;
                pending_work(&pending, SOURCE_FEATURE_WORK)?;
                let Some(other_feature) = self.feature(&other) else {
                    return Ok(ControlFlow::Continue(()));
                };
                if feature.shared(&other_feature) {
                    first = Some(other);
                }
                Ok(ControlFlow::Continue(()))
            },
        );
        budget.consume(pending.replace(0))?;
        let _ = result?;
        Ok(first)
    }

    fn owned_source_coverage(
        &self,
        shadows: &ShadowIndex<Occluder>,
        start: DVec3,
        end: DVec3,
        crossing: &Crossing,
        budget: &mut Budget<'_>,
    ) -> Result<Coverage, String> {
        budget.consume(SOURCE_FEATURE_WORK)?;
        let Some(feature) = self.feature(crossing) else {
            return Ok(crossing.coverage);
        };
        let amounts = transport_bytes(crossing.coverage.amount());
        let mut owned = amounts;
        let mut previous = None;
        for _ in 0..self.crossings {
            budget.consume(1)?;
            let Some(other) =
                self.next_shared_crossing(start, end, crossing, &feature, previous, budget)?
            else {
                break;
            };
            previous = Some(other.ordinal);
            if let Some(coverage) =
                self.source_event(shadows, start, end, crossing, Some(&other), budget)?
            {
                budget.consume(SOURCE_TRANSPORT_WORK)?;
                for (channel, amount) in transport_bytes(coverage.amount()).into_iter().enumerate()
                {
                    if amount > amounts[channel]
                        || (amount == amounts[channel] && other.ordinal < crossing.ordinal)
                    {
                        owned[channel] = 0;
                    }
                }
                if owned.iter().all(|&amount| amount == 0) {
                    break;
                }
            }
        }
        Ok(Coverage::from_channels(owned))
    }

    pub fn styled_colors(
        &self,
        hit: &Hit,
        cone: f64,
        budget: &mut Budget<'_>,
    ) -> Result<[(u8, DVec3); 4], String> {
        let mut colors = [(255, DVec3::ZERO); 4];
        colors[0] = (0, hit.color);
        let radiance = &self.radiance[hit.source];
        for (index, layer) in radiance.extras.iter().enumerate() {
            budget.consume(24)?;
            colors[index + 1] = (
                layer.style,
                radiance.at_layer(
                    &layer.samples,
                    layer.average,
                    hit.point,
                    hit.uv,
                    hit.distance,
                    cone,
                ) * hit.visibility,
            );
        }
        Ok(colors)
    }

    fn next_crossing(
        &self,
        point: DVec3,
        end: DVec3,
        previous: Option<(f64, bool, usize)>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Crossing>, String> {
        let mut first: Option<Crossing> = None;
        let pending_alpha = Cell::new(0usize);
        let result = self.geometry.visit(
            point,
            end,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending_alpha.replace(0))
                        .ok_or("Brush alpha source work overflow")?,
                )
            },
            &mut |surface| {
                let Some((fraction, weights)) =
                    surface.geometry.intersection(point, end, &pending_alpha)?
                else {
                    return Ok(Visit::Continue(
                        first.as_ref().map_or(1.0, |hit| hit.fraction),
                    ));
                };
                let sky = self.radiance[surface.source].sky;
                let key = (fraction, sky, surface.ordinal);
                if previous.is_some_and(|previous| key <= previous)
                    || first.as_ref().is_some_and(|hit| {
                        key >= (hit.fraction, self.radiance[hit.source].sky, hit.ordinal)
                    })
                {
                    return Ok(Visit::Continue(
                        first.as_ref().map_or(1.0, |hit| hit.fraction),
                    ));
                }
                let crossing =
                    self.crossing(surface, fraction, weights, end - point, &pending_alpha)?;
                let radiance = &self.radiance[surface.source];
                if crossing.coverage.clear()
                    && (radiance.sky || radiance.alpha.is_none() || radiance.boundary.is_none())
                {
                    return Ok(Visit::Continue(
                        first.as_ref().map_or(1.0, |hit| hit.fraction),
                    ));
                }
                first = Some(crossing);
                Ok(Visit::Continue(fraction))
            },
        );
        budget.consume(pending_alpha.replace(0))?;
        result?;
        Ok(first)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn trace(
        &self,
        point: DVec3,
        direction: DVec3,
        cone: f64,
        scene: &Scene<'_>,
        skipped_owner: Option<u32>,
        budget: &mut Budget<'_>,
        visitor: &mut impl FnMut(&Hit, &mut Budget<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        budget.consume(11)?;
        let end = point + direction * super::sky::TRACE_LENGTH;
        let mut previous = None;
        let mut tied_fraction = None;
        let mut tied_remaining = [u8::MAX; 3];
        for _ in 0..self.crossings {
            budget.consume(1)?;
            let Some(mut crossing) = self.next_crossing(point, end, previous, budget)? else {
                break;
            };
            let radiance = &self.radiance[crossing.source];
            previous = Some((crossing.fraction, radiance.sky, crossing.ordinal));
            if !radiance.sky && radiance.alpha.is_some() && radiance.boundary.is_some() {
                let Some(coverage) =
                    self.source_event(scene.shadows, point, end, &crossing, None, budget)?
                else {
                    continue;
                };
                crossing.coverage = coverage;
                if coverage.clear() {
                    continue;
                }
                crossing.coverage =
                    self.owned_source_coverage(scene.shadows, point, end, &crossing, budget)?;
                if crossing.coverage.clear() {
                    continue;
                }
            }
            let (position, fraction, remaining) = if radiance.sky {
                let Some(fraction) =
                    scene
                        .shadows
                        .optical_sky_fraction(point, end, &mut |work| budget.consume(work))?
                else {
                    break;
                };
                (
                    point.lerp(end, fraction),
                    fraction,
                    scene.sky_visibility(point, end, fraction, skipped_owner, budget)?,
                )
            } else {
                (
                    crossing.point,
                    crossing.fraction,
                    scene.visibility_to_source(
                        point,
                        crossing.point,
                        skipped_owner,
                        radiance.boundary.as_ref(),
                        crossing.boundary_error,
                        budget,
                    )?,
                )
            };
            budget.consume(SOURCE_TRANSPORT_WORK)?;
            let remaining = transport_bytes(remaining);
            if tied_fraction != Some(crossing.fraction) {
                tied_fraction = Some(crossing.fraction);
                tied_remaining = remaining;
            }
            let source = transport_bytes(crossing.coverage.amount());
            let covered = std::array::from_fn(|channel| {
                source[channel]
                    .min(remaining[channel])
                    .min(tied_remaining[channel])
            });
            tied_remaining =
                std::array::from_fn(|channel| tied_remaining[channel] - covered[channel]);
            let covered = Coverage::from_channels(covered);
            if !covered.clear() && (radiance.sky || crossing.front) {
                let visibility = covered.amount();
                budget.consume(24)?;
                let distance = fraction * super::sky::TRACE_LENGTH;
                visitor(
                    &Hit {
                        color: radiance.at(position, crossing.uv, distance, cone) * visibility,
                        point: position,
                        sky: radiance.sky,
                        front: crossing.front,
                        source: crossing.source,
                        uv: crossing.uv,
                        distance,
                        visibility,
                    },
                    budget,
                )?;
            }
            if radiance.sky || tied_remaining.iter().all(|&amount| amount == 0) {
                break;
            }
        }
        Ok(())
    }
}
