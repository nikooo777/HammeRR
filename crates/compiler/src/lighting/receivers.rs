use glam::DVec3;

use super::Occluder;
use super::bake::{Budget, Emitter};
use super::culling::Candidates;
use super::shadows::{ShadowIndex, Triangle};
use crate::{map::Map, tree::Tree, visibility::Visibility};

pub(super) const STYLE_COUNT: usize = 64;

pub(super) struct Context<'a> {
    pub lights: &'a [Emitter],
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<Triangle>,
    pub props: &'a ShadowIndex<super::props::PropTriangle>,
    pub sun_directions: &'a [DVec3],
    pub ambient_directions: &'a [DVec3],
}

pub(super) struct Sample {
    pub point: DVec3,
    pub normal: DVec3,
    pub origin: DVec3,
}

pub(super) fn cluster(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    point: DVec3,
    open_error: &str,
    budget: &mut Budget<'_>,
) -> Result<usize, String> {
    let mut node = 0;
    while let Some((plane, children)) = tree.nodes[node].split {
        budget.consume(1)?;
        node = children[usize::from(map.planes.get(plane).distance(point) < 0.0)];
    }
    budget.consume(1)?;
    visibility.clusters[node]
        .filter(|_| !tree.nodes[node].solid())
        .ok_or_else(|| open_error.into())
}

pub(super) fn direct(
    ctx: &Context<'_>,
    candidates: &mut Candidates,
    cluster: usize,
    sample: Sample,
    sky_ambient: bool,
    budget: &mut Budget<'_>,
) -> Result<[DVec3; STYLE_COUNT], String> {
    direct_optional(ctx, candidates, Some(cluster), sample, sky_ambient, budget)
}

pub(super) fn direct_optional(
    ctx: &Context<'_>,
    candidates: &mut Candidates,
    cluster: Option<usize>,
    sample: Sample,
    sky_ambient: bool,
    budget: &mut Budget<'_>,
) -> Result<[DVec3; STYLE_COUNT], String> {
    let mut colors = [DVec3::ZERO; STYLE_COUNT];
    for &candidate in candidates.for_cluster(cluster, ctx.lights, budget)? {
        budget.consume(1)?;
        let light = &ctx.lights[candidate];
        if super::culling::outside_range(light, sample.point) {
            continue;
        }
        match light {
            Emitter::Point(light) => {
                let value = light.contribution(sample.point, sample.normal)?;
                if value != DVec3::ZERO
                    && !ctx
                        .shadows
                        .blocked(sample.origin, light.origin, &mut |work| {
                            budget.consume(work)
                        })?
                    && !ctx
                        .terrain
                        .blocked(sample.origin, light.origin, &mut |work| {
                            budget.consume(work)
                        })?
                    && !ctx
                        .props
                        .blocked(sample.origin, light.origin, &mut |work| {
                            budget.consume(work)
                        })?
                {
                    colors[usize::from(light.style)] += value;
                }
            }
            Emitter::Sky(light) => {
                colors[0] += light.sun_contribution(
                    sample.normal,
                    ctx.sun_directions,
                    &mut |direction| {
                        super::bake::sky_visible(
                            direction,
                            sample.origin,
                            ctx.shadows,
                            ctx.terrain,
                            ctx.props,
                            &None,
                            budget,
                        )
                    },
                )?;
                if sky_ambient && light.has_ambient() {
                    budget.consume(ctx.ambient_directions.len())?;
                    colors[0] += light.ambient_contribution(
                        sample.normal,
                        ctx.ambient_directions,
                        &mut |direction| {
                            super::bake::sky_visible(
                                direction,
                                sample.origin,
                                ctx.shadows,
                                ctx.terrain,
                                ctx.props,
                                &None,
                                budget,
                            )
                        },
                    )?;
                }
            }
        }
    }
    Ok(colors)
}
