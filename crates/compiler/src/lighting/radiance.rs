use glam::{DVec2, DVec3};
use source_formats::bsp::{Bsp, Edge, Face, LightingKind, Model, RgbExp32, TexInfo, surf};

use super::Occluder;
use super::bake::Budget;
use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use crate::map::Map;

const SURFACE_HIT_BIAS: f64 = 0.05;
const LOCAL_FILTER_START: f64 = 20.0;
const LOCAL_FILTER_END: f64 = 40.0;

struct Surface {
    triangle: Triangle,
    normal: DVec3,
    source: usize,
    coordinates: Option<[DVec2; 3]>,
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
    surfaces.push(Surface {
        triangle: Triangle::new(points)?,
        normal: (points[0] - points[1])
            .cross(points[2] - points[1])
            .normalize(),
        source,
        coordinates,
    });
    Ok(())
}

pub(super) fn build(
    map: &Map<'_>,
    bsp: &Bsp,
    sky_radiance: DVec3,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(map, bsp, sky_radiance, false, budget)
}

pub(super) fn build_styled(
    map: &Map<'_>,
    bsp: &Bsp,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(map, bsp, DVec3::ZERO, true, budget)
}

fn build_with_styles(
    map: &Map<'_>,
    bsp: &Bsp,
    sky_radiance: DVec3,
    styled: bool,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    let world = bsp
        .records::<Model>()
        .map_err(|error| error.to_string())?
        .first()
        .copied()
        .ok_or("Surface radiance needs a world model")?;
    let faces = bsp.records::<Face>().map_err(|error| error.to_string())?;
    let infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    let lighting = bsp
        .lighting(LightingKind::Ldr)
        .map_err(|error| error.to_string())?;
    let mut surfaces = Vec::new();
    let mut radiance = Vec::new();
    for face in &faces[world.first_face as usize..(world.first_face + world.face_count) as usize] {
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
            let reflectivity = DVec3::from_array(material.reflectivity.map(f64::from));
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
        let mut extras = Vec::new();
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
                    budget,
                )?;
            }
        }
    }
    Ok(SurfaceIndex {
        geometry: ShadowIndex::new(surfaces, &mut |work| budget.consume(work))?,
        radiance,
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
                ),
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
                    .intersection(point, end)?
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
                            (surface.source, uv, surface.normal.dot(-direction) > 0.0),
                        )
                    }))
            },
        )?;
        let Some((fraction, (source, uv, front))) = hit else {
            return Ok(None);
        };
        let radiance = &self.radiance[source];
        let blocker = if radiance.sky {
            let Some(hit) = shadows.sky_fraction(point, end, &mut |work| budget.consume(work))?
            else {
                return Ok(None);
            };
            Some((hit, ()))
        } else {
            shadows.nearest(point, end, &mut |work| budget.consume(work), &mut |hull| {
                Ok(hull.entry(point, end)?.map(|hit| (hit.0, ())))
            })?
        };
        if blocker.is_some_and(|(blocked, _)| {
            (fraction - blocked) * super::sky::TRACE_LENGTH > SURFACE_HIT_BIAS
        }) {
            return Ok(None);
        }
        let distance = fraction * super::sky::TRACE_LENGTH;
        let position = point + direction * distance;
        Ok(Some(Hit {
            color: radiance.at(position, uv, distance, cone),
            point: position,
            sky: radiance.sky,
            front,
            source,
            uv,
            distance,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
