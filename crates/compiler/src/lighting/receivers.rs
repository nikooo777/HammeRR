use glam::DVec3;

use super::bake::{Budget, Emitter};
use super::culling::Candidates;
use super::shadows::ShadowIndex;
use super::terrain_shadows::TerrainTriangle;
use super::transport::Scene;
use super::{Occluder, ReceiverAngularMode};
use crate::{map::Map, tree::Tree, visibility::Visibility};

pub(super) const STYLE_COUNT: usize = 64;

pub(super) struct Context<'a> {
    pub emission: Option<&'a super::emission::Prepared>,
    pub kind: source_formats::bsp::LightingKind,
    pub lights: &'a [Emitter],
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<TerrainTriangle>,
    pub props: &'a ShadowIndex<super::props::PropTriangle>,
    pub sun_directions: &'a [DVec3],
    pub ambient_directions: &'a [DVec3],
}

pub(super) struct Sample {
    pub point: DVec3,
    pub normal: DVec3,
    pub origin: DVec3,
}

pub(super) struct Reception {
    pub angular: ReceiverAngularMode,
    pub skip_owner: Option<u32>,
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
    direct_skipping_owner(ctx, candidates, cluster, sample, sky_ambient, None, budget)
}

pub(super) fn direct_skipping_owner(
    ctx: &Context<'_>,
    candidates: &mut Candidates,
    cluster: Option<usize>,
    sample: Sample,
    sky_ambient: bool,
    skip_owner: Option<u32>,
    budget: &mut Budget<'_>,
) -> Result<[DVec3; STYLE_COUNT], String> {
    direct_with_reception(
        ctx,
        candidates,
        cluster,
        sample,
        sky_ambient,
        Reception {
            angular: ReceiverAngularMode::Normal,
            skip_owner,
        },
        budget,
    )
}

pub(super) fn direct_with_reception(
    ctx: &Context<'_>,
    candidates: &mut Candidates,
    cluster: Option<usize>,
    sample: Sample,
    sky_ambient: bool,
    reception: Reception,
    budget: &mut Budget<'_>,
) -> Result<[DVec3; STYLE_COUNT], String> {
    let context = budget.context();
    let mut colors = [DVec3::ZERO; STYLE_COUNT];
    let scene = Scene {
        shadows: ctx.shadows,
        terrain: ctx.terrain,
        props: ctx.props,
        model: None,
    };
    for &candidate in candidates.for_cluster(cluster, ctx.lights, budget)? {
        let light = &ctx.lights[candidate];
        budget.emitter(light);
        budget.consume(1)?;
        if super::culling::outside_range(light, sample.point) {
            continue;
        }
        match light {
            Emitter::Point(light) => {
                let value =
                    light.contribution_with_mode(sample.point, sample.normal, reception.angular)?;
                if value != DVec3::ZERO {
                    let visibility = scene.visibility(
                        sample.origin,
                        light.origin,
                        reception.skip_owner,
                        budget,
                    )?;
                    colors[usize::from(light.style)] += value * visibility;
                }
            }
            Emitter::Sky(light) => {
                colors[0] += light.sun_contribution_with_mode_visibility(
                    sample.normal,
                    ctx.sun_directions,
                    reception.angular,
                    &mut |direction| {
                        super::sky_visibility_skipping_owner(
                            direction,
                            sample.origin,
                            ctx.shadows,
                            ctx.terrain,
                            ctx.props,
                            &None,
                            reception.skip_owner,
                            budget,
                        )
                    },
                )?;
                if sky_ambient && light.has_ambient() {
                    budget.consume(ctx.ambient_directions.len())?;
                    colors[0] += light.ambient_contribution_with_mode_visibility(
                        sample.normal,
                        ctx.ambient_directions,
                        reception.angular,
                        &mut |direction| {
                            super::sky_visibility_skipping_owner(
                                direction,
                                sample.origin,
                                ctx.shadows,
                                ctx.terrain,
                                ctx.props,
                                &None,
                                reception.skip_owner,
                                budget,
                            )
                        },
                    )?;
                }
            }
        }
    }
    if let Some(emission) = ctx.emission {
        budget.clear_emitter();
        colors[0] += emission.gather(
            ctx.kind,
            super::emission::Sample {
                point: sample.point,
                normal: sample.normal,
                origin: sample.origin,
                face: None,
                angular: reception.angular,
            },
            super::emission::Context {
                shadows: ctx.shadows,
                terrain: ctx.terrain,
                props: ctx.props,
                model: None,
                skip_owner: reception.skip_owner,
            },
            budget,
        )?;
    }
    budget.restore(context);
    Ok(colors)
}
