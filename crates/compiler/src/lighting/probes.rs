use glam::DVec3;
use source_formats::bsp::{
    Bsp, Edge, Face, Leaf, LeafAmbient, LeafAmbientIndex, LeafAmbientSample, LightingKind, Model,
    TexInfo, contents, surf,
};

use super::bake::Budget;
use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use super::{Occluder, encode_color};
use crate::map::Map;
use crate::tree::Tree;
use crate::visibility::Visibility;
use crate::{Diagnostic, Report};

const SURFACE_HIT_BIAS: f64 = 0.05;
const CANDIDATE_FRACTIONS: [u8; 4] = [64, 128, 192, 224];
const CUBE_AXES: [DVec3; 6] = [
    DVec3::X,
    DVec3::NEG_X,
    DVec3::Y,
    DVec3::NEG_Y,
    DVec3::Z,
    DVec3::NEG_Z,
];

pub(super) struct Context<'a> {
    pub map: &'a Map<'a>,
    pub tree: &'a Tree,
    pub visibility: &'a Visibility,
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<Triangle>,
    pub directions: &'a [DVec3],
    pub sky_radiance: DVec3,
}

pub(super) struct Stats {
    pub samples: usize,
    pub bytes: usize,
}

struct Surface {
    triangle: Triangle,
    radiance: DVec3,
    sky: bool,
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
    radiance: DVec3,
    sky: bool,
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
        return Err("Model ambient surface count exceeds the bounded triangle limit".into());
    }
    surfaces.push(Surface {
        triangle: Triangle::new(points)?,
        radiance,
        sky,
    });
    Ok(())
}

fn surfaces(
    ctx: &Context<'_>,
    bsp: &Bsp,
    budget: &mut Budget<'_>,
) -> Result<ShadowIndex<Surface>, String> {
    let world = bsp
        .records::<Model>()
        .map_err(|error| error.to_string())?
        .first()
        .copied()
        .ok_or("Model ambient needs a world model")?;
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
    for face in &faces[world.first_face as usize..(world.first_face + world.face_count) as usize] {
        budget.consume(1)?;
        let info = infos[face.texinfo as usize];
        let sky = info.flags & (surf::SKY | surf::SKY2D) != 0;
        let radiance = if sky {
            ctx.sky_radiance
        } else {
            let average = lighting
                .as_ref()
                .map(|lighting| lighting.face(face, &info))
                .transpose()
                .map_err(|error| error.to_string())?
                .flatten()
                .and_then(|light| light.average(0))
                .map_or(DVec3::ZERO, |color| DVec3::from_array(color.linear()));
            let material = ctx
                .map
                .materials
                .get(ctx.map.texdata[info.texdata as usize]);
            let reflectivity = DVec3::from_array(material.reflectivity.map(f64::from));
            if !reflectivity.is_finite() || reflectivity.min_element() < 0.0 {
                return Err(format!(
                    "Material {} has invalid ambient reflectivity",
                    material.name
                ));
            }
            average * reflectivity / 255.0
        };
        if !radiance.is_finite() || radiance.min_element() < 0.0 {
            return Err("Model ambient surface radiance is nonfinite or negative".into());
        }
        if face.dispinfo >= 0 {
            let terrain = &ctx.map.terrains[face.dispinfo as usize];
            budget.consume(terrain.vertices.len())?;
            let points = terrain.native_positions()?;
            for indices in &terrain.indices {
                add_surface(
                    &mut surfaces,
                    indices.map(|index| DVec3::from_array(points[index].map(f64::from))),
                    radiance,
                    sky,
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
                    radiance,
                    sky,
                    budget,
                )?;
            }
        }
    }
    ShadowIndex::new(surfaces, &mut |work| budget.consume(work))
}

fn candidate(
    ctx: &Context<'_>,
    node: usize,
    leaf: &Leaf,
    budget: &mut Budget<'_>,
) -> Result<Option<LeafAmbientSample>, String> {
    let volume = &ctx.tree.nodes[node].volume;
    let count = volume
        .sides
        .iter()
        .map(|side| side.winding.0.len())
        .sum::<usize>();
    budget.consume(count + 1)?;
    if count == 0 {
        return Ok(None);
    }
    let center = volume
        .sides
        .iter()
        .flat_map(|side| &side.winding.0)
        .copied()
        .sum::<DVec3>()
        / count as f64;
    let encoded = std::array::from_fn(|axis| {
        let min = f64::from(leaf.mins[axis]);
        let span = f64::from(leaf.maxs[axis]) - min;
        if span > 0.0 {
            ((center[axis] - min) * 255.0 / span)
                .round()
                .clamp(0.0, 255.0) as u8
        } else {
            0
        }
    });
    let candidates =
        std::iter::once(encoded).chain((0..CANDIDATE_FRACTIONS.len().pow(3)).map(|index| {
            std::array::from_fn(|axis| {
                CANDIDATE_FRACTIONS[(index / CANDIDATE_FRACTIONS.len().pow(axis as u32))
                    % CANDIDATE_FRACTIONS.len()]
            })
        }));
    for position in candidates {
        budget.consume(volume.sides.len() + 1)?;
        let sample = LeafAmbientSample {
            position,
            ..Default::default()
        };
        let point = DVec3::from_array(
            sample
                .world_position(leaf)
                .map_err(|error| error.to_string())?,
        );
        if volume.sides.iter().any(|side| {
            let plane = ctx.map.planes.get(side.plane);
            plane.normal.dot(point) - plane.dist > 0.0001
        }) {
            continue;
        }
        let blocked = ctx
            .shadows
            .nearest(
                point,
                point,
                &mut |work| budget.consume(work),
                &mut |hull| Ok(hull.entry(point, point)?.map(|hit| (hit.0, ()))),
            )?
            .is_some();
        if blocked {
            continue;
        }
        let above = point + DVec3::Z * super::sky::TRACE_LENGTH;
        if ctx
            .terrain
            .nearest(
                point,
                above,
                &mut |work| budget.consume(work),
                &mut |triangle| {
                    Ok(triangle
                        .fraction(point, above)?
                        .map(|fraction| (fraction, ())))
                },
            )?
            .is_some()
        {
            continue;
        }
        return Ok(Some(sample));
    }
    Ok(None)
}

fn gather(
    directions: &[DVec3],
    radiance: &mut impl FnMut(DVec3) -> Result<DVec3, String>,
) -> Result<[DVec3; 6], String> {
    let mut totals = [0.0; 6];
    let mut colors = [DVec3::ZERO; 6];
    for &direction in directions {
        let color = radiance(direction)?;
        for (axis, normal) in CUBE_AXES.iter().enumerate() {
            let weight = normal.dot(direction).max(0.0);
            totals[axis] += weight;
            colors[axis] += color * weight;
        }
    }
    for (color, total) in colors.iter_mut().zip(totals) {
        if total <= 0.0 {
            return Err("Model ambient sampling has an empty hemisphere".into());
        }
        *color /= total;
    }
    Ok(colors)
}

pub(super) fn bake(
    ctx: &Context<'_>,
    bsp: &mut Bsp,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Stats, String> {
    let surfaces = surfaces(ctx, bsp, budget)?;
    let leaves = bsp.records::<Leaf>().map_err(|error| error.to_string())?;
    let mut nodes = vec![None; ctx.visibility.count];
    for (node, cluster) in ctx.visibility.clusters.iter().enumerate() {
        budget.consume(1)?;
        if let Some(cluster) = cluster {
            nodes[*cluster] = Some(node);
        }
    }
    let mut indices = vec![LeafAmbientIndex::default(); leaves.len()];
    let mut samples = Vec::new();
    let mut missing = 0;
    for (index, leaf) in leaves.iter().enumerate() {
        budget.consume(1)?;
        if leaf.cluster < 0 || leaf.contents & contents::SOLID != 0 {
            continue;
        }
        let node = nodes
            .get(leaf.cluster as usize)
            .copied()
            .flatten()
            .ok_or("Model ambient leaf has no world volume")?;
        let Some(mut sample) = candidate(ctx, node, leaf, budget)? else {
            missing += 1;
            continue;
        };
        let point = DVec3::from_array(
            sample
                .world_position(leaf)
                .map_err(|error| error.to_string())?,
        );
        let colors = gather(ctx.directions, &mut |direction| {
            budget.consume(7)?;
            let end = point + direction * super::sky::TRACE_LENGTH;
            let surface = surfaces.nearest(
                point,
                end,
                &mut |work| budget.consume(work),
                &mut |surface| {
                    Ok(surface
                        .triangle
                        .fraction(point, end)?
                        .map(|fraction| (fraction, (surface.radiance, surface.sky))))
                },
            )?;
            let Some((fraction, (color, sky))) = surface else {
                return Ok(DVec3::ZERO);
            };
            let blocker = if sky {
                let Some(hit) = ctx
                    .shadows
                    .sky_fraction(point, end, &mut |work| budget.consume(work))?
                else {
                    return Ok(DVec3::ZERO);
                };
                Some((hit, ()))
            } else {
                ctx.shadows
                    .nearest(point, end, &mut |work| budget.consume(work), &mut |hull| {
                        Ok(hull.entry(point, end)?.map(|hit| (hit.0, ())))
                    })?
            };
            if blocker.is_some_and(|(blocked, _)| {
                (fraction - blocked) * super::sky::TRACE_LENGTH > SURFACE_HIT_BIAS
            }) {
                return Ok(DVec3::ZERO);
            }
            Ok(color)
        })?;
        for (slot, color) in sample.cube.iter_mut().zip(colors) {
            *slot = encode_color(color)?;
        }
        indices[index] = LeafAmbientIndex {
            sample_count: 1,
            first_sample: u16::try_from(samples.len())
                .map_err(|_| "Model ambient sample offset exceeds Source limits")?,
        };
        samples.push(sample);
    }
    if missing != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("{missing} open leaves have no safe quantized ambient probe; model ambient lighting there is missing"), None, None));
    }
    let stats = Stats {
        samples: samples.len(),
        bytes: indices.len() * size_of::<LeafAmbientIndex>()
            + samples.len() * size_of::<LeafAmbientSample>(),
    };
    bsp.set_leaf_ambient(
        LightingKind::Ldr,
        &LeafAmbient::Indexed { indices, samples },
    )
    .map_err(|error| error.to_string())?;
    report.diagnostics.push(Diagnostic::info("Model ambient uses one quantized probe per open world leaf, sampled from reflected face averages and sky radiance. Local lightmap sampling, adaptive probe placement, static-prop shadows and bounced world lighting remain unfinished", None, None));
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directional_cube_normalization_keeps_uniform_radiance_and_its_hemispheres() {
        let directions = super::super::sky::ambient_directions(crate::SkyQuality::Fast);
        let cube = gather(&directions, &mut |_| Ok(DVec3::new(0.5, 0.25, 0.125))).unwrap();
        for color in cube {
            assert_eq!(color, DVec3::new(0.5, 0.25, 0.125));
        }
        let cube = gather(&directions, &mut |direction| {
            Ok(if direction.z > 0.0 {
                DVec3::Z
            } else {
                DVec3::ZERO
            })
        })
        .unwrap();
        assert_eq!(cube[4], DVec3::Z);
        assert_eq!(cube[5], DVec3::ZERO);
        assert!(gather(&directions, &mut |_| Err("cancelled".into())).is_err());
    }
}
