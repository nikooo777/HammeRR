use glam::{DQuat, DVec3, EulerRot};
use source_formats::bsp::{Bsp, DetailProp, DetailProps, LightingKind};

use super::bake::{Budget, Emitter};
use super::culling::Candidates;
use super::shadows::{ShadowIndex, Triangle};
use super::{Occluder, encode_color};
use crate::map::{MAX_COMPILE_COORD, Map};
use crate::tree::Tree;
use crate::visibility::Visibility;
use crate::{Diagnostic, Report};

const LIGHTING_SCALE: f64 = 255.0;

pub(super) struct Context<'a> {
    pub map: &'a Map<'a>,
    pub tree: &'a Tree,
    pub visibility: &'a Visibility,
    pub kind: LightingKind,
    pub model_centers: &'a [DVec3],
    pub lights: &'a [Emitter],
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<Triangle>,
    pub props: &'a ShadowIndex<super::props::PropTriangle>,
    pub directions: &'a [DVec3],
    pub sun_directions: &'a [DVec3],
    pub sky_radiance: DVec3,
}

fn receiver(
    prop: &DetailProp,
    details: &DetailProps,
    model_centers: &[DVec3],
) -> Result<(DVec3, DVec3), String> {
    let local = match prop.kind {
        0 => *model_centers
            .get(usize::from(prop.model))
            .ok_or("Detail lighting is missing its validated model hull midpoint")?,
        1 => {
            let sprite = details
                .sprites
                .get(usize::from(prop.model))
                .ok_or("Detail lighting sprite dictionary index is invalid")?;
            DVec3::new(
                0.0,
                (f64::from(sprite.upper_left[0]) + f64::from(sprite.lower_right[0])) * 0.5,
                (f64::from(sprite.upper_left[1]) + f64::from(sprite.lower_right[1])) * 0.5,
            ) * f64::from(prop.scale)
        }
        _ => return Err("Baked detail lighting supports models and type-one sprites".into()),
    };
    let angles = prop.angles.map(|value| f64::from(value).to_radians());
    let rotation = DQuat::from_euler(EulerRot::ZYX, angles[1], angles[0], angles[2]);
    let center = DVec3::from_array(prop.origin.map(f64::from)) + rotation * local;
    let normal = rotation * DVec3::Z;
    if !center.is_finite() || center.abs().max_element() > MAX_COMPILE_COORD || !normal.is_finite()
    {
        return Err("Detail lighting center must fit finite Source coordinates".into());
    }
    Ok((center, normal))
}

fn cluster(ctx: &Context<'_>, point: DVec3, budget: &mut Budget<'_>) -> Result<usize, String> {
    let mut node = 0;
    while let Some((plane, children)) = ctx.tree.nodes[node].split {
        budget.consume(1)?;
        node = children[usize::from(ctx.map.planes.get(plane).distance(point) < 0.0)];
    }
    budget.consume(1)?;
    ctx.visibility.clusters[node]
        .filter(|_| !ctx.tree.nodes[node].solid())
        .ok_or_else(|| {
            "Detail lighting hull or sprite midpoint must lie in open visible world space".into()
        })
}

fn style_error(index: usize, style: u8) -> String {
    format!(
        "Detail receiver {index} receives nonzero light style {style}; animated and switchable detail lighting is unsupported"
    )
}

fn direct(
    ctx: &Context<'_>,
    candidates: &mut Candidates,
    index: usize,
    point: DVec3,
    normal: DVec3,
    budget: &mut Budget<'_>,
) -> Result<DVec3, String> {
    let cluster = cluster(ctx, point, budget)?;
    let mut color = DVec3::ZERO;
    for &candidate in candidates.for_cluster(Some(cluster), ctx.lights, budget)? {
        budget.consume(1)?;
        let light = &ctx.lights[candidate];
        if super::culling::outside_range(light, point) {
            continue;
        }
        match light {
            Emitter::Point(light) => {
                let value = light.contribution(point, normal)?;
                if value != DVec3::ZERO
                    && !ctx
                        .shadows
                        .blocked(point, light.origin, &mut |work| budget.consume(work))?
                    && !ctx
                        .terrain
                        .blocked(point, light.origin, &mut |work| budget.consume(work))?
                    && !ctx
                        .props
                        .blocked(point, light.origin, &mut |work| budget.consume(work))?
                {
                    if light.style != 0 {
                        return Err(style_error(index, light.style));
                    }
                    color += value;
                }
            }
            Emitter::Sky(light) => {
                color += light.sun_contribution(normal, ctx.sun_directions, &mut |direction| {
                    super::bake::sky_visible(
                        direction,
                        point,
                        ctx.shadows,
                        ctx.terrain,
                        ctx.props,
                        &None,
                        budget,
                    )
                })?;
            }
        }
    }
    Ok(color)
}

pub(super) fn bake(
    ctx: &Context<'_>,
    bsp: &mut Bsp,
    candidates: &mut Candidates,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let Some(lump) = bsp.game_lumps.iter().find(|lump| lump.id == *b"dprp") else {
        return Ok(());
    };
    if lump.data == [0; 12] {
        return Ok(());
    }
    let leaf_bytes = bsp.lumps[source_formats::bsp::lump::LEAFS].data.len();
    let _records = budget.reserve_bytes(lump.data.len() * 3 + leaf_bytes * 2)?;
    budget.consume(lump.data.len() + leaf_bytes * 2)?;
    let Some(mut details) = bsp.detail_props().map_err(|error| error.to_string())? else {
        return Ok(());
    };
    if details.props.is_empty() {
        return Ok(());
    }
    let surfaces =
        super::radiance::build_styled_sky(ctx.map, bsp, ctx.kind, ctx.sky_radiance, budget)?;
    if ctx.directions.is_empty() {
        return Err("Detail ambient lighting requires sphere directions".into());
    }
    let cone = (1.0 - 2.0 / ctx.directions.len() as f64).acos().tan();
    for index in 0..details.props.len() {
        budget.consume(1)?;
        let prop = details.props[index];
        if prop.light_style_count != 0 {
            return Err(
                "Existing styled detail records cannot be rebaked by static detail lighting".into(),
            );
        }
        let (point, normal) = receiver(&prop, &details, ctx.model_centers)?;
        let mut color = direct(ctx, candidates, index, point, normal, budget)?;
        let mut ambient = DVec3::ZERO;
        for &direction in ctx.directions {
            budget.consume(1)?;
            if let Some(hit) =
                surfaces.trace(point, direction, cone, ctx.shadows, ctx.props, budget)?
            {
                for (style, value) in surfaces.styled_colors(&hit, cone, budget)? {
                    if style == 0 {
                        ambient += value;
                    } else if style != 255 && value != DVec3::ZERO {
                        return Err(style_error(index, style));
                    }
                }
            }
        }
        color += ambient * (LIGHTING_SCALE / ctx.directions.len() as f64);
        details.props[index].lighting = encode_color(color)?;
    }
    budget.consume(details.props.len())?;
    bsp.set_detail_props(&details)
        .map_err(|error| error.to_string())?;
    report.diagnostics.push(Diagnostic::info(
        format!(
            "Baked {} detail model/sprite colors from static direct light and filtered spherical world/terrain/sky radiance ({:?} shared base); nonzero styles at detail receivers are unsupported",
            details.props.len(), ctx.kind,
        ),
        None,
        None,
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::bsp::DetailSprite;

    #[test]
    fn sprite_lighting_uses_scaled_anchor_midpoint_and_stored_angles() {
        let details = DetailProps {
            sprites: vec![DetailSprite {
                upper_left: [-4.0, 24.0],
                lower_right: [12.0, -8.0],
                ..Default::default()
            }],
            ..Default::default()
        };
        let prop = DetailProp {
            kind: 1,
            scale: 1.5,
            origin: [10.0, 20.0, 30.0],
            angles: [0.0, 90.0, 0.0],
            ..Default::default()
        };
        let (point, normal) = receiver(&prop, &details, &[]).unwrap();
        assert!((point - DVec3::new(4.0, 20.0, 42.0)).length() < 1e-12);
        assert!((normal - DVec3::Z).length() < 1e-12);
    }

    #[test]
    fn detail_receiver_checks_share_cancellation_work_and_live_memory_limits() {
        let game = crate::test_common::empty_game();
        let mut report = Report::default();
        let map = Map::load(&crate::test_common::room(), &game, &mut report).unwrap();
        let tree = Tree {
            nodes: vec![crate::tree::Node {
                split: None,
                volume: crate::tree::Polyhedron { sides: Vec::new() },
                contents: 0,
                brushes: Vec::new(),
            }],
        };
        let visibility = Visibility {
            clusters: vec![Some(0)],
            count: 1,
            portals: 0,
            data: Vec::new(),
            sky: Vec::new(),
            work: 0,
            removed_pairs: 0,
            workers: 1,
        };
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let shadows = ShadowIndex::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
        let terrain = ShadowIndex::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
        let props = ShadowIndex::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
        let ctx = Context {
            map: &map,
            tree: &tree,
            visibility: &visibility,
            kind: LightingKind::Ldr,
            model_centers: &[DVec3::ZERO],
            lights: &[],
            shadows: &shadows,
            terrain: &terrain,
            props: &props,
            directions: &[DVec3::Z],
            sun_directions: &[DVec3::Z],
            sky_radiance: DVec3::ZERO,
        };
        control.cancel().unwrap();
        assert!(cluster(&ctx, DVec3::ZERO, &mut budget).is_err());
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        budget.consume(100_000_000).unwrap();
        assert!(
            cluster(&ctx, DVec3::ZERO, &mut budget)
                .unwrap_err()
                .contains("shared 100-million")
        );
        let mut budget = Budget::new(&control);
        let _held = budget
            .reserve_bytes(super::super::bake::MAX_LIGHTING_LIVE_BYTES)
            .unwrap();
        let mut bsp = Bsp::default();
        bsp.set_records(&[source_formats::bsp::Leaf::default()]);
        bsp.set_detail_props(&DetailProps {
            models: vec![DetailProps::model_record("models/props/test.mdl").unwrap()],
            props: vec![DetailProp::default()],
            ..Default::default()
        })
        .unwrap();
        let before = bsp.to_bytes();
        let mut candidates = Candidates::new(&[], &visibility, &mut budget).unwrap();
        assert!(
            bake(&ctx, &mut bsp, &mut candidates, &mut report, &mut budget)
                .unwrap_err()
                .contains("512 MiB")
        );
        assert_eq!(bsp.to_bytes(), before);
    }
}
