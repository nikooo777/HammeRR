use glam::DVec3;
use source_formats::bsp::{
    Bsp, Leaf, LeafAmbient, LeafAmbientIndex, LeafAmbientSample, LightingKind, contents,
};

use super::bake::Budget;
use super::shadows::ShadowIndex;
use super::terrain_shadows::TerrainTriangle;
use super::{Occluder, encode_color};
use crate::map::Map;
use crate::tree::Tree;
use crate::{Diagnostic, Report};

mod refinement;

const MIN_PROBE_SPACING: f64 = 128.0;
const MAX_AMBIENT_SAMPLES: usize = u16::MAX as usize + 1;
const CANDIDATE_FRACTIONS: [u8; 4] = [64, 128, 192, 224];
const BOUNDARY_FRACTIONS: [u8; 4] = [16, 240, 1, 254];
const MAX_CANDIDATE_POOL: usize = 1 + CANDIDATE_FRACTIONS.len().pow(3);
const GRID_GENERATION_WORK: usize = 32;
const MAX_MISSING_PROBE_DETAILS: usize = 8;
const CUBE_AXES: [DVec3; 6] = [
    DVec3::X,
    DVec3::NEG_X,
    DVec3::Y,
    DVec3::NEG_Y,
    DVec3::Z,
    DVec3::NEG_Z,
];
const EXTRA_DIRECTION_WORK: usize = CUBE_AXES.len() * 16;
const EXTRA_CUBE_WORK: usize = CUBE_AXES.len() * 32;

pub(super) struct Context<'a> {
    pub model_scopes: &'a [super::models::Scope],
    pub emission: Option<&'a super::emission::Prepared>,
    pub brush_textures: Option<&'a super::brush_alpha::Prepared>,
    pub prop_sources: Option<&'a super::prop_vertices::SourceSnapshot>,
    pub face_sources: &'a [std::sync::Arc<[u32]>],
    pub kind: LightingKind,
    pub map: &'a Map<'a>,
    pub tree: &'a Tree,
    pub world_leaf_nodes: &'a [Option<usize>],
    pub prop_shadows: &'a ShadowIndex<super::props::PropTriangle>,
    pub prop_bounds: &'a ShadowIndex<super::props::ProbeBounds>,
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<TerrainTriangle>,
    pub directions: &'a [DVec3],
    pub quality: crate::SkyQuality,
    pub sky_radiance: DVec3,
    pub refine: bool,
}

pub(super) struct Stats {
    pub samples: usize,
    pub bytes: usize,
}

#[derive(Default)]
struct Rejections {
    attempts: usize,
    outside: usize,
    brushes: usize,
    props: usize,
    terrain: usize,
}

struct Candidates {
    probes: Vec<LeafAmbientSample>,
    reference: DVec3,
    rejected: Rejections,
}

fn candidate_grid(fractions: &[u8]) -> impl Iterator<Item = [u8; 3]> + '_ {
    let width = fractions.len();
    (0..width.pow(3)).map(move |index| {
        std::array::from_fn(|axis| fractions[(index / width.pow(axis as u32)) % width])
    })
}

fn candidates(
    ctx: &Context<'_>,
    node: usize,
    leaf: &Leaf,
    limit: usize,
    budget: &mut Budget<'_>,
) -> Result<Candidates, String> {
    let volume = &ctx.tree.nodes[node].volume;
    let count = volume
        .sides
        .iter()
        .map(|side| side.winding.0.len())
        .sum::<usize>();
    budget.consume(count + 1)?;
    if count == 0 {
        return Err("Ambient probe world cell has no boundary vertices".into());
    }
    let center = volume
        .sides
        .iter()
        .flat_map(|side| &side.winding.0)
        .copied()
        .sum::<DVec3>()
        / count as f64;
    if !center.is_finite() {
        return Err("Ambient probe world cell has a nonfinite reference point".into());
    }
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
    budget.consume(GRID_GENERATION_WORK)?;
    let mut extended = [0; CANDIDATE_FRACTIONS.len() + BOUNDARY_FRACTIONS.len()];
    let (primary, boundary) = extended.split_at_mut(CANDIDATE_FRACTIONS.len());
    primary.copy_from_slice(&CANDIDATE_FRACTIONS);
    boundary.copy_from_slice(&BOUNDARY_FRACTIONS);
    let candidates = std::iter::once(encoded)
        .chain(candidate_grid(&CANDIDATE_FRACTIONS))
        .map(|position| (0, position))
        .chain(
            (CANDIDATE_FRACTIONS.len() + 2..=extended.len())
                .step_by(2)
                .flat_map(|width| {
                    candidate_grid(&extended[..width]).map(move |position| (width, position))
                }),
        );
    let mut valid = Vec::new();
    let mut rejected = Rejections::default();
    let mut charged_width = 0;
    if ctx.refine {
        valid
            .try_reserve_exact(MAX_CANDIDATE_POOL)
            .map_err(|_| "Cannot reserve ambient refinement candidates")?;
    }
    for (width, position) in candidates {
        let boundary = width != 0;
        if boundary {
            if !valid.is_empty() {
                break;
            }
            if charged_width != width {
                budget.consume(width.pow(3) * GRID_GENERATION_WORK)?;
                charged_width = width;
            }
            if position == encoded
                || position
                    .iter()
                    .all(|fraction| extended[..width - 2].contains(fraction))
            {
                continue;
            }
        }
        budget.consume(volume.sides.len() + 8)?;
        rejected.attempts += 1;
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
            rejected.outside += 1;
            continue;
        }
        let blocked = ctx
            .shadows
            .nearest(
                point,
                point,
                &mut |work| budget.consume(work),
                &mut |hull| {
                    if !hull.occupies_world() {
                        return Ok(None);
                    }
                    Ok(hull.entry(point, point)?.map(|hit| (hit.0, ())))
                },
            )?
            .is_some();
        if blocked {
            rejected.brushes += 1;
            continue;
        }
        if ctx
            .prop_bounds
            .contains_point(point, &mut |work| budget.consume(work))?
        {
            rejected.props += 1;
            continue;
        }
        let above = point + DVec3::Z * super::sky::TRACE_LENGTH;
        if let Some((terrain_fraction, ())) = ctx.terrain.nearest(
            point,
            above,
            &mut |work| budget.consume(work),
            &mut |triangle| {
                Ok(triangle
                    .intersection(point, above)?
                    .map(|(fraction, _)| (fraction, ())))
            },
        )? {
            let roof = ctx.shadows.nearest(
                point,
                above,
                &mut |work| budget.consume(work),
                &mut |hull| {
                    if !hull.occupies_world() {
                        return Ok(None);
                    }
                    let Some(entry) = hull.optical_boundaries(point, above)?[0] else {
                        return Ok(None);
                    };
                    Ok((entry.fraction < terrain_fraction
                        && entry.non_sky_coverage.opaque()
                        && !entry.sky)
                        .then_some((entry.fraction, ())))
                },
            )?;
            if roof.is_none() {
                rejected.terrain += 1;
                continue;
            }
        }
        valid.push((sample, point));
        if limit == 1 || boundary {
            break;
        }
    }
    let mut selected = Vec::new();
    if ctx.refine {
        selected
            .try_reserve_exact(limit)
            .map_err(|_| "Cannot reserve ambient refinement pool")?;
    }
    if !valid.is_empty() {
        if ctx.refine {
            budget.consume(valid.len())?;
        }
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
        if ctx.refine {
            budget.consume(valid.len() - index)?;
        }
        selected.push(valid.remove(index));
    }
    let probes = if ctx.refine {
        let mut probes = Vec::new();
        probes
            .try_reserve_exact(limit)
            .map_err(|_| "Cannot reserve ambient refinement records")?;
        budget.consume(selected.len())?;
        probes.extend(selected.into_iter().map(|(sample, _)| sample));
        probes
    } else {
        selected.into_iter().map(|(sample, _)| sample).collect()
    };
    Ok(Candidates {
        probes,
        reference: center,
        rejected,
    })
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

fn reserve_refinement(budget: &Budget<'_>) -> Result<super::bake::Allocation, String> {
    budget.reserve_bytes(
        (MAX_CANDIDATE_POOL + refinement::MAX_POOL) * size_of::<(LeafAmbientSample, DVec3)>()
            + refinement::MAX_POOL * size_of::<LeafAmbientSample>()
            + refinement::SCRATCH_BYTES,
    )
}

pub(super) fn bake(
    ctx: &Context<'_>,
    bsp: &mut Bsp,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Stats, String> {
    let channel = match ctx.kind {
        LightingKind::Ldr => "LDR",
        LightingKind::Hdr => "HDR",
    };
    let surfaces = super::radiance::build(
        ctx.map,
        bsp,
        ctx.model_scopes,
        ctx.kind,
        ctx.sky_radiance,
        ctx.face_sources,
        ctx.brush_textures,
        ctx.terrain,
        ctx.shadows,
        ctx.prop_sources,
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
        let limit = match ctx.quality {
            crate::SkyQuality::Fast => 1,
            crate::SkyQuality::Balanced => 4,
            crate::SkyQuality::Detailed => 8,
        };
        let _refinement_allocation = ctx.refine.then(|| reserve_refinement(budget)).transpose()?;
        let Candidates {
            mut probes,
            reference,
            rejected,
        } = candidates(
            ctx,
            node,
            leaf,
            if ctx.refine { limit * 2 } else { limit },
            budget,
        )
        .map_err(|error| format!("{channel} ambient leaf {index}: {error}"))?;
        if probes.is_empty() {
            missing += 1;
            if missing <= MAX_MISSING_PROBE_DETAILS {
                budget.consume(64)?;
                report.diagnostics.push(Diagnostic::warning(
                    format!("{channel} ambient leaf {index} (cluster {}): no accepted probe after {} candidate attempts. First rejections: {} outside cell, {} in world brushes, {} in prop bounds, {} with terrain above. Model ambient is missing. Recorded position is the cell reference, not a probe.", leaf.cluster, rejected.attempts, rejected.outside, rejected.brushes, rejected.props, rejected.terrain),
                    None,
                    Some(reference),
                ).with_world_position());
            }
            continue;
        }
        let emitted = probes.len().min(limit);
        if samples.len() + emitted > MAX_AMBIENT_SAMPLES {
            return Err("Model ambient sample count exceeds the bounded Source index limit".into());
        }
        indices[index] = LeafAmbientIndex {
            sample_count: emitted as u16,
            first_sample: u16::try_from(samples.len())
                .map_err(|_| "Model ambient sample offset exceeds Source limits")?,
        };
        budget.plan_samples(probes.len())?;
        for (probe_index, sample) in probes.iter_mut().enumerate() {
            if ctx.refine && probe_index >= emitted {
                budget.consume(ctx.directions.len() * EXTRA_DIRECTION_WORK + EXTRA_CUBE_WORK)?;
            }
            let point = DVec3::from_array(
                sample
                    .world_position(leaf)
                    .map_err(|error| error.to_string())?,
            );
            let mut colors = gather(ctx.directions, &mut |direction| {
                let mut total = DVec3::ZERO;
                surfaces.trace(
                    point,
                    direction,
                    cone,
                    &super::transport::Scene {
                        shadows: ctx.shadows,
                        terrain: ctx.terrain,
                        props: ctx.prop_shadows,
                        model: None,
                    },
                    None,
                    budget,
                    &mut |hit, _| {
                        total += hit.color;
                        Ok(())
                    },
                )?;
                Ok(total)
            })?;
            if let Some(emission) = ctx.emission.filter(|emission| !emission.native_enabled()) {
                for (color, normal) in colors.iter_mut().zip(CUBE_AXES) {
                    *color += emission.gather(
                        ctx.kind,
                        super::emission::Sample {
                            point,
                            normal,
                            origin: point,
                            face: None,
                            angular: super::ReceiverAngularMode::Normal,
                        },
                        super::emission::Context {
                            shadows: ctx.shadows,
                            terrain: ctx.terrain,
                            props: ctx.prop_shadows,
                            model: None,
                            skip_owner: None,
                        },
                        budget,
                    )? / 255.0;
                }
            }
            for (slot, color) in sample.cube.iter_mut().zip(colors) {
                *slot = encode_color(color)?;
            }
            budget.finish_sample();
        }
        if ctx.refine && probes.len() > emitted {
            let replacement = refinement::choose(leaf, &probes, emitted, budget)?;
            budget.consume(1)?;
            probes.swap(emitted - 1, replacement);
            probes.truncate(emitted);
        }
        samples.extend(probes);
    }
    budget.samples_planned_complete();
    if missing != 0 {
        let omitted = missing.saturating_sub(MAX_MISSING_PROBE_DETAILS);
        let mut message =
            format!("{channel}: model ambient lighting is missing in {missing} open leaves.");
        if omitted != 0 {
            message.push_str(&format!(" Additional leaf locations omitted: {omitted}."));
        }
        report
            .diagnostics
            .push(Diagnostic::warning(message, None, None));
    }
    let stats = Stats {
        samples: samples.len(),
        bytes: indices.len() * size_of::<LeafAmbientIndex>()
            + samples.len() * size_of::<LeafAmbientSample>(),
    };
    bsp.set_leaf_ambient(ctx.kind, &LeafAmbient::Indexed { indices, samples })
        .map_err(|error| error.to_string())?;
    Ok(stats)
}
