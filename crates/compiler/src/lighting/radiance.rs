use glam::{DVec2, DVec3};
use source_formats::bsp::{Bsp, Edge, LightingKind, Model, RgbExp32, TexInfo, surf};
use std::cell::Cell;
use std::sync::Arc;

use super::Occluder;
use super::bake::Budget;
use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use crate::map::Map;

const LOCAL_FILTER_START: f64 = 20.0;
const LOCAL_FILTER_END: f64 = 40.0;
const SOURCE_ROUNDING_OPERATIONS: f64 = 64.0;

#[cfg(test)]
#[path = "radiance/source_boundary_tests.rs"]
mod source_boundary_tests;

#[cfg(test)]
#[path = "radiance/receiver_tests.rs"]
pub(super) mod receiver_tests;

struct Surface {
    triangle: Triangle,
    normal: DVec3,
    source: usize,
    coordinates: Option<[DVec2; 3]>,
    boundary_error: f64,
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
}

struct SourceBoundary {
    plane: usize,
    equation: map_core::Plane,
    sides: Arc<[u32]>,
    owners: Vec<usize>,
}

impl SourceBoundary {
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
    _allocation: super::bake::Allocation,
}

impl ShadowObject for Surface {
    const MAX_OBJECTS: usize = Triangle::MAX_OBJECTS;
    fn bounds(&self) -> (DVec3, DVec3) {
        self.triangle.bounds()
    }
    fn work(&self) -> usize {
        self.triangle.work()
    }
    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        self.triangle.intersects(start, end)
    }
}

fn add_surface(
    surfaces: &mut Vec<Surface>,
    points: [DVec3; 3],
    source: usize,
    coordinates: Option<[DVec2; 3]>,
    boundary: Option<&SourceBoundary>,
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
        triangle: Triangle::new(points)?,
        normal: (points[0] - points[1])
            .cross(points[2] - points[1])
            .normalize(),
        source,
        coordinates,
        boundary_error,
    });
    Ok(())
}

pub(super) fn build(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    sky_radiance: DVec3,
    face_sources: &[Arc<[u32]>],
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(map, bsp, kind, sky_radiance, false, face_sources, budget)
}

pub(super) fn build_styled(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    face_sources: &[Arc<[u32]>],
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(map, bsp, kind, DVec3::ZERO, true, face_sources, budget)
}

pub(super) fn build_styled_sky(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    sky_radiance: DVec3,
    face_sources: &[Arc<[u32]>],
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(map, bsp, kind, sky_radiance, true, face_sources, budget)
}

fn build_with_styles(
    map: &Map<'_>,
    bsp: &Bsp,
    kind: LightingKind,
    sky_radiance: DVec3,
    styled: bool,
    face_sources: &[Arc<[u32]>],
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
    let bytes = ShadowIndex::<Surface>::allocation_bytes(triangles)?
        .checked_add(world_faces.len() * (size_of::<Radiance>() + 3 * size_of::<Layer>()))
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
        radiance.push(Radiance {
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
            let points = terrain.native_positions()?;
            let size = terrain
                .info
                .grid_size()
                .map_err(|error| error.to_string())?;
            for indices in &terrain.indices {
                add_surface(
                    &mut surfaces,
                    indices.map(|index| DVec3::from_array(points[index].map(f64::from))),
                    source,
                    Some(indices.map(|index| {
                        DVec2::new((index % size) as f64, (index / size) as f64) / (size - 1) as f64
                    })),
                    None,
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
            for index in 1..points.len().saturating_sub(1) {
                add_surface(
                    &mut surfaces,
                    [points[0], points[index], points[index + 1]],
                    source,
                    None,
                    radiance[source].boundary.as_ref(),
                    budget,
                )?;
            }
        }
    }
    Ok(SurfaceIndex {
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
    visibility: f64,
}

impl SurfaceIndex {
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

    pub fn trace(
        &self,
        point: DVec3,
        direction: DVec3,
        cone: f64,
        shadows: &ShadowIndex<Occluder>,
        props: &ShadowIndex<super::props::PropTriangle>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Hit>, String> {
        self.trace_skipping_owner(point, direction, cone, shadows, props, None, budget)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn trace_skipping_owner(
        &self,
        point: DVec3,
        direction: DVec3,
        cone: f64,
        shadows: &ShadowIndex<Occluder>,
        props: &ShadowIndex<super::props::PropTriangle>,
        skipped_owner: Option<u32>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Hit>, String> {
        budget.consume(11)?;
        let end = point + direction * super::sky::TRACE_LENGTH;
        let hit = self.geometry.nearest(
            point,
            end,
            &mut |work| budget.consume(work),
            &mut |surface| {
                Ok(surface
                    .triangle
                    .source_intersection(point, end)?
                    .map(|(fraction, weights)| {
                        let uv = surface.coordinates.map(|coordinates| {
                            coordinates
                                .into_iter()
                                .zip(weights)
                                .map(|(point, weight)| point * weight)
                                .sum::<DVec2>()
                        });
                        (
                            fraction,
                            (
                                surface.source,
                                uv,
                                surface.normal.dot(-direction) > 0.0,
                                surface.triangle.point(weights),
                                surface.boundary_error
                                    * weights.into_iter().map(f64::abs).sum::<f64>(),
                            ),
                        )
                    }))
            },
        )?;
        let Some((fraction, (source, uv, front, position, boundary_error))) = hit else {
            return Ok(None);
        };
        let radiance = &self.radiance[source];
        if radiance.sky
            && shadows
                .sky_fraction(point, end, &mut |work| budget.consume(work))?
                .is_none()
        {
            return Ok(None);
        }
        let pending = Cell::new(0usize);
        let blocker = shadows.nearest(
            point,
            position,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending.replace(0))
                        .ok_or("Radiance source certificate work overflow")?,
                )
            },
            &mut |hull| {
                let Some(interval) = hull.interval(point, position)? else {
                    return Ok(None);
                };
                if let Some(boundary) = &radiance.boundary {
                    let (terminal, work) =
                        boundary.terminal(hull, interval, point, position, boundary_error)?;
                    pending.set(
                        pending
                            .get()
                            .checked_add(work)
                            .ok_or("Radiance source certificate work overflow")?,
                    );
                    if terminal {
                        return Ok(None);
                    }
                }
                Ok(Some((interval.entering, ())))
            },
        );
        budget.consume(pending.replace(0))?;
        if blocker?.is_some() {
            return Ok(None);
        }
        let distance = fraction * super::sky::TRACE_LENGTH;
        let visibility =
            props.visibility_skipping_owner(point, position, skipped_owner, &mut |amount| {
                budget.consume(amount)
            })?;
        if visibility == 0.0 {
            return Ok(None);
        }
        Ok(Some(Hit {
            color: radiance.at(position, uv, distance, cone) * visibility,
            point: position,
            sky: radiance.sky,
            front,
            source,
            uv,
            distance,
            visibility,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prop_occlusion_clips_indirect_rays_at_the_radiating_surface() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let color = DVec3::new(0.5, 0.25, 0.125);
        let allocation = budget
            .reserve_bytes(
                ShadowIndex::<Surface>::allocation_bytes(1).unwrap() + size_of::<Radiance>(),
            )
            .unwrap();
        let surfaces = SurfaceIndex {
            geometry: ShadowIndex::new(
                vec![Surface {
                    triangle: Triangle::new([
                        DVec3::new(-128.0, -128.0, 64.0),
                        DVec3::new(128.0, -128.0, 64.0),
                        DVec3::new(0.0, 128.0, 64.0),
                    ])
                    .unwrap(),
                    normal: DVec3::NEG_Z,
                    source: 0,
                    coordinates: None,
                    boundary_error: 0.0,
                }],
                &mut |work| budget.consume(work),
            )
            .unwrap(),
            radiance: vec![Radiance {
                average: color,
                reflectivity: DVec3::ONE,
                samples: Vec::new(),
                dimensions: [0; 2],
                projection: [[0.0; 4]; 2],
                mins: [0; 2],
                sky: false,
                extras: Vec::new(),
                boundary: None,
            }],
            _allocation: allocation,
        };
        let shadows =
            ShadowIndex::new(Vec::<Occluder>::new(), &mut |work| budget.consume(work)).unwrap();
        for height in [32.0, 64.0, 96.0] {
            let props = ShadowIndex::new(
                vec![
                    super::super::props::PropTriangle::new([
                        DVec3::new(-8.0, -8.0, height),
                        DVec3::new(8.0, -8.0, height),
                        DVec3::new(0.0, 8.0, height),
                    ])
                    .unwrap(),
                ],
                &mut |work| budget.consume(work),
            )
            .unwrap();
            let center = surfaces
                .trace(DVec3::ZERO, DVec3::Z, 0.0, &shadows, &props, &mut budget)
                .unwrap();
            assert_eq!(
                center.map(|hit| hit.color),
                if height < 64.0 { None } else { Some(color) }
            );
            let hole = surfaces
                .trace(
                    DVec3::X * 32.0,
                    DVec3::Z,
                    0.0,
                    &shadows,
                    &props,
                    &mut budget,
                )
                .unwrap()
                .unwrap();
            assert_eq!(hole.color, color);
            assert!(hole.front);
        }
    }

    #[test]
    fn local_radiance_interpolates_clamps_and_filters_distant_detail() {
        let field = Radiance {
            average: DVec3::new(96.0 / 255.0, 0.0, 0.0),
            reflectivity: DVec3::splat(1.0 / 255.0),
            samples: [0, 64, 128, 192]
                .map(|red| RgbExp32 {
                    rgb: [red, 0, 0],
                    exponent: 0,
                })
                .to_vec(),
            dimensions: [2, 2],
            projection: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
            mins: [0, 0],
            sky: false,
            extras: Vec::new(),
            boundary: None,
        };
        assert_eq!(field.at(DVec3::ZERO, None, 10.0, 0.1), DVec3::ZERO);
        assert_eq!(
            field.at(DVec3::new(0.5, 0.5, 0.0), None, 10.0, 0.1),
            field.average
        );
        assert_eq!(field.at(DVec3::ONE, None, 10.0, 0.1).x, 192.0 / 255.0);
        assert_eq!(
            field.at(DVec3::new(-100.0, 100.0, 0.0), None, 10.0, 0.1).x,
            128.0 / 255.0
        );
        assert_eq!(field.at(DVec3::ONE, None, 1000.0, 0.1), field.average);
        let middle = field.at(DVec3::ZERO, None, 300.0, 0.1);
        assert_eq!(middle, field.average * 0.5);
        let triangle = Triangle::new([
            DVec3::new(0.0, 0.0, 16.0),
            DVec3::new(128.0, 0.0, 32.0),
            DVec3::new(0.0, 128.0, 16.0),
        ])
        .unwrap();
        let (_, weights) = triangle
            .intersection(DVec3::new(32.0, 32.0, 128.0), DVec3::new(32.0, 32.0, -64.0))
            .unwrap()
            .unwrap();
        let coordinates = [DVec2::ZERO, DVec2::X, DVec2::Y];
        let uv = coordinates
            .into_iter()
            .zip(weights)
            .map(|(point, weight)| point * weight)
            .sum::<DVec2>();
        assert_eq!(uv, DVec2::splat(0.25));
        assert_eq!(
            field.at(DVec3::splat(1000.0), Some(uv), 10.0, 0.1).x,
            48.0 / 255.0
        );
    }
}
