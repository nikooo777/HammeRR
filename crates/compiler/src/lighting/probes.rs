use glam::DVec3;
use source_formats::bsp::{
    Bsp, Leaf, LeafAmbient, LeafAmbientIndex, LeafAmbientSample, LightingKind, contents,
};

use super::bake::Budget;
use super::shadows::{ShadowIndex, Triangle};
use super::{Occluder, encode_color};
use crate::map::Map;
use crate::tree::Tree;
use crate::{Diagnostic, Report};

const MIN_PROBE_SPACING: f64 = 128.0;
const MAX_AMBIENT_SAMPLES: usize = u16::MAX as usize + 1;
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
    pub face_sources: &'a [std::sync::Arc<[u32]>],
    pub kind: LightingKind,
    pub map: &'a Map<'a>,
    pub tree: &'a Tree,
    pub world_leaf_nodes: &'a [Option<usize>],
    pub prop_shadows: &'a ShadowIndex<super::props::PropTriangle>,
    pub prop_bounds: &'a ShadowIndex<super::props::ProbeBounds>,
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<Triangle>,
    pub directions: &'a [DVec3],
    pub quality: crate::SkyQuality,
    pub sky_radiance: DVec3,
}

pub(super) struct Stats {
    pub samples: usize,
    pub bytes: usize,
}

fn candidates(
    ctx: &Context<'_>,
    node: usize,
    leaf: &Leaf,
    budget: &mut Budget<'_>,
) -> Result<Vec<LeafAmbientSample>, String> {
    let volume = &ctx.tree.nodes[node].volume;
    let count = volume
        .sides
        .iter()
        .map(|side| side.winding.0.len())
        .sum::<usize>();
    budget.consume(count + 1)?;
    if count == 0 {
        return Ok(Vec::new());
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
    let limit = match ctx.quality {
        crate::SkyQuality::Fast => 1,
        crate::SkyQuality::Balanced => 4,
        crate::SkyQuality::Detailed => 8,
    };
    let mut valid = Vec::new();
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
        if blocked
            || ctx
                .prop_bounds
                .blocked(point, point, &mut |work| budget.consume(work))?
        {
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
        valid.push((sample, point));
        if limit == 1 {
            break;
        }
    }
    let mut selected = Vec::new();
    if !valid.is_empty() {
        selected.push(valid.remove(0));
    }
    while selected.len() < limit {
        let mut best = None;
        let mut distance = MIN_PROBE_SPACING * MIN_PROBE_SPACING;
        for (index, (_, point)) in valid.iter().enumerate() {
            budget.consume(selected.len())?;
            let separation = selected
                .iter()
                .map(|(_, chosen)| point.distance_squared(*chosen))
                .fold(f64::INFINITY, f64::min);
            if separation > distance {
                best = Some(index);
                distance = separation;
            }
        }
        let Some(index) = best else { break };
        selected.push(valid.remove(index));
    }
    Ok(selected.into_iter().map(|(sample, _)| sample).collect())
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
    let surfaces = super::radiance::build(
        ctx.map,
        bsp,
        ctx.kind,
        ctx.sky_radiance,
        ctx.face_sources,
        budget,
    )?;
    let cone = (1.0 - 2.0 / ctx.directions.len() as f64).acos().tan();
    let leaves = bsp.records::<Leaf>().map_err(|error| error.to_string())?;
    let _ambient_allocation = budget.reserve_bytes(
        leaves.len()
            * (size_of::<LeafAmbientIndex>() + size_of::<Leaf>() + size_of::<Option<usize>>())
            + MAX_AMBIENT_SAMPLES * size_of::<LeafAmbientSample>(),
    )?;
    if ctx.world_leaf_nodes.len() != leaves.len() {
        return Err("Model ambient world-leaf mapping disagrees with emitted leaves".into());
    }
    let mut indices = vec![LeafAmbientIndex::default(); leaves.len()];
    let mut samples = Vec::with_capacity(MAX_AMBIENT_SAMPLES);
    let mut missing = 0;
    for (index, leaf) in leaves.iter().enumerate() {
        budget.consume(1)?;
        if leaf.cluster < 0 || leaf.contents & contents::SOLID != 0 {
            continue;
        }
        let node = ctx
            .world_leaf_nodes
            .get(index)
            .copied()
            .flatten()
            .filter(|&node| {
                node < ctx.tree.nodes.len()
                    && ctx.tree.nodes[node].split.is_none()
                    && !ctx.tree.nodes[node].solid()
            })
            .ok_or("Model ambient leaf has no exact world volume")?;
        let probes = candidates(ctx, node, leaf, budget)?;
        if probes.is_empty() {
            missing += 1;
            continue;
        }
        if samples.len() + probes.len() > MAX_AMBIENT_SAMPLES {
            return Err("Model ambient sample count exceeds the bounded Source index limit".into());
        }
        indices[index] = LeafAmbientIndex {
            sample_count: probes.len() as u16,
            first_sample: u16::try_from(samples.len())
                .map_err(|_| "Model ambient sample offset exceeds Source limits")?,
        };
        budget.plan_samples(probes.len())?;
        for mut sample in probes {
            let point = DVec3::from_array(
                sample
                    .world_position(leaf)
                    .map_err(|error| error.to_string())?,
            );
            let colors = gather(ctx.directions, &mut |direction| {
                Ok(surfaces
                    .trace(
                        point,
                        direction,
                        cone,
                        ctx.shadows,
                        ctx.prop_shadows,
                        budget,
                    )?
                    .map_or(DVec3::ZERO, |hit| hit.color))
            })?;
            for (slot, color) in sample.cube.iter_mut().zip(colors) {
                *slot = encode_color(color)?;
            }
            samples.push(sample);
            budget.finish_sample();
        }
    }
    budget.samples_planned_complete();
    if missing != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("{missing} open leaves have no safe quantized ambient probe; model ambient lighting there is missing"), None, None));
    }
    let stats = Stats {
        samples: samples.len(),
        bytes: indices.len() * size_of::<LeafAmbientIndex>()
            + samples.len() * size_of::<LeafAmbientSample>(),
    };
    bsp.set_leaf_ambient(ctx.kind, &LeafAmbient::Indexed { indices, samples })
        .map_err(|error| error.to_string())?;
    report.diagnostics.push(Diagnostic::info("Model ambient uses up to 1/4/8 separated probes per open world leaf at Fast/Balanced/Detailed quality, sampled from filtered local world/terrain lightmaps and sky radiance. Static-prop opaque, cutout and requested translucent meshes attenuate probe rays; adaptive refinement and reflected prop sources remain unfinished", None, None));
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leaf_cube_gathers_transmitted_source_color() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let (surfaces, shadows, props) =
            super::super::radiance::receiver_tests::transmitted_source(&mut budget);
        let cube = gather(&CUBE_AXES, &mut |direction| {
            Ok(surfaces
                .trace(
                    DVec3::Z * 16.0,
                    direction,
                    0.0,
                    &shadows,
                    &props,
                    &mut budget,
                )?
                .map_or(DVec3::ZERO, |hit| hit.color * 255.0))
        })
        .unwrap();
        for color in &cube[..5] {
            assert_eq!(*color, DVec3::ZERO);
        }
        assert!(cube[5].abs_diff_eq(
            DVec3::new(191.0, 95.87450980392157, 47.937254901960784),
            1e-12
        ));
    }
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
