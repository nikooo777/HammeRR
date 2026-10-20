use glam::{DQuat, DVec3, EulerRot};
use source_formats::bsp::{
    Bsp, DetailProp, DetailPropLighting, DetailProps, LightingKind, MAX_DETAIL_PROP_LIGHTING,
    RgbExp32,
};

use super::bake::{Budget, Emitter};
use super::culling::Candidates;
use super::receivers::STYLE_COUNT;
use super::shadows::{ShadowIndex, Triangle};
use super::{Occluder, encode_color};
use crate::detail_props::Tint;
use crate::map::{MAX_COMPILE_COORD, Map};
use crate::tree::Tree;
use crate::visibility::Visibility;
use crate::{Diagnostic, Report};

const LIGHTING_SCALE: f64 = 255.0;
const MAX_AUTHORED_STYLE_GAIN: f32 = 550.0 * (1.0 / 264.0);
const OPEN_RECEIVER: &str =
    "Detail lighting hull or sprite midpoint must lie in open visible world space";

pub(super) struct Channel {
    ranges: Vec<std::ops::Range<usize>>,
    records: Vec<DetailPropLighting>,
    _allocation: super::bake::Allocation,
}

pub(super) struct Context<'a> {
    pub face_sources: &'a [std::sync::Arc<[u32]>],
    pub map: &'a Map<'a>,
    pub tree: &'a Tree,
    pub visibility: &'a Visibility,
    pub kind: LightingKind,
    pub model_centers: &'a [DVec3],
    pub lighting_origins: &'a [Option<DVec3>],
    pub lighting_tints: &'a [Tint],
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
    lighting_origin: Option<DVec3>,
) -> Result<(DVec3, DVec3), String> {
    if lighting_origin.is_some() && prop.kind != 0 {
        return Err("Named detail lighting origins require an explicit model record".into());
    }
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
        2 | 3 => DVec3::ZERO,
        _ => return Err("Baked detail lighting encountered an unsupported native type".into()),
    };
    let angles = prop.angles.map(|value| f64::from(value).to_radians());
    let rotation = DQuat::from_euler(EulerRot::ZYX, angles[1], angles[0], angles[2]);
    let center = lighting_origin
        .unwrap_or_else(|| DVec3::from_array(prop.origin.map(f64::from)) + rotation * local);
    let normal = rotation * DVec3::Z;
    if !center.is_finite() || center.abs().max_element() > MAX_COMPILE_COORD || !normal.is_finite()
    {
        return Err("Detail lighting center must fit finite Source coordinates".into());
    }
    Ok((center, normal))
}

fn cluster(ctx: &Context<'_>, point: DVec3, budget: &mut Budget<'_>) -> Result<usize, String> {
    super::receivers::cluster(
        ctx.map,
        ctx.tree,
        ctx.visibility,
        point,
        OPEN_RECEIVER,
        budget,
    )
}

fn direct(
    ctx: &Context<'_>,
    candidates: &mut Candidates,
    point: DVec3,
    normal: DVec3,
    budget: &mut Budget<'_>,
) -> Result<[DVec3; STYLE_COUNT], String> {
    let cluster = cluster(ctx, point, budget)?;
    super::receivers::direct(
        &super::receivers::Context {
            lights: ctx.lights,
            shadows: ctx.shadows,
            terrain: ctx.terrain,
            props: ctx.props,
            sun_directions: ctx.sun_directions,
            ambient_directions: ctx.directions,
        },
        candidates,
        cluster,
        super::receivers::Sample {
            point,
            normal,
            origin: point,
        },
        false,
        budget,
    )
}

fn ambient(
    surfaces: &super::radiance::SurfaceIndex,
    point: DVec3,
    cone: f64,
    directions: &[DVec3],
    shadows: &ShadowIndex<Occluder>,
    props: &ShadowIndex<super::props::PropTriangle>,
    budget: &mut Budget<'_>,
) -> Result<[DVec3; STYLE_COUNT], String> {
    let mut colors = [DVec3::ZERO; STYLE_COUNT];
    for &direction in directions {
        budget.consume(1)?;
        if let Some(hit) = surfaces.trace(point, direction, cone, shadows, props, budget)? {
            for (style, value) in surfaces.styled_colors(&hit, cone, budget)? {
                if style != 255 {
                    let slot = colors
                        .get_mut(usize::from(style))
                        .ok_or("Detail lighting style exceeds native range 1–63")?;
                    *slot += value;
                }
            }
        }
    }
    Ok(colors)
}

pub(super) fn bake(
    ctx: &Context<'_>,
    bsp: &mut Bsp,
    candidates: &mut Candidates,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Option<Channel>, String> {
    let Some(lump) = bsp.game_lumps.iter().find(|lump| lump.id == *b"dprp") else {
        return Ok(None);
    };
    if lump.data == [0; 12] {
        return Ok(None);
    }
    let leaf_bytes = bsp.lumps[source_formats::bsp::lump::LEAFS].data.len();
    let _records = budget.reserve_bytes(lump.data.len() * 3 + leaf_bytes * 2)?;
    budget.consume(lump.data.len() + leaf_bytes * 2)?;
    let Some(mut details) = bsp.detail_props().map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    if ctx.lighting_origins.len() != details.props.len() {
        return Err(
            "Detail lighting origin metadata does not match the sorted native records".into(),
        );
    }
    if ctx.lighting_tints.len() != details.props.len() {
        return Err(
            "Detail lighting tint metadata does not match the sorted native records".into(),
        );
    }
    if details.props.is_empty() {
        return Ok(None);
    }
    let surfaces = super::radiance::build_styled_sky(
        ctx.map,
        bsp,
        ctx.kind,
        ctx.sky_radiance,
        ctx.face_sources,
        budget,
    )?;
    if ctx.directions.is_empty() {
        return Err("Detail ambient lighting requires sphere directions".into());
    }
    let cone = (1.0 - 2.0 / ctx.directions.len() as f64).acos().tan();
    let allocation = budget.reserve_bytes(
        details.props.len() * size_of::<std::ops::Range<usize>>()
            + MAX_DETAIL_PROP_LIGHTING * size_of::<DetailPropLighting>(),
    )?;
    let mut ranges = Vec::with_capacity(details.props.len());
    let mut records = Vec::with_capacity(MAX_DETAIL_PROP_LIGHTING);
    for index in 0..details.props.len() {
        budget.consume(1)?;
        let prop = details.props[index];
        if prop.light_style_count != 0 {
            return Err(
                "Existing styled detail records cannot be rebaked by static detail lighting".into(),
            );
        }
        let lighting_origin = ctx.lighting_origins[index];
        let tint = ctx.lighting_tints[index];
        let (point, normal) = receiver(&prop, &details, ctx.model_centers, lighting_origin)?;
        let mut colors = direct(ctx, candidates, point, normal, budget).map_err(|error| {
            if lighting_origin.is_some() && error==OPEN_RECEIVER {
                "Named detail lighting origin must lie in open visible world space; helpers outside a sealed compile region remain referenced but cannot be sampled. Enlarge the region or disable it".into()
            } else {error}
        })?;
        let ambient = ambient(
            &surfaces,
            point,
            cone,
            ctx.directions,
            ctx.shadows,
            ctx.props,
            budget,
        )?;
        budget.consume(STYLE_COUNT)?;
        let ambient_scale = LIGHTING_SCALE / ctx.directions.len() as f64;
        details.props[index].lighting =
            encode_color(tint.apply(colors[0] + ambient[0] * ambient_scale))?;
        let start = records.len();
        for (style, (color, ambient)) in colors.iter_mut().zip(ambient).enumerate().skip(1) {
            *color += ambient * ambient_scale;
            if *color != DVec3::ZERO {
                if records.len() == MAX_DETAIL_PROP_LIGHTING {
                    return Err(
                        "Detail lighting exceeds its 65,536 total style-record budget".into(),
                    );
                }
                records.push(DetailPropLighting {
                    style: style as u8,
                    lighting: encode_color(tint.apply(*color * 0.5))?,
                });
            }
        }
        ranges.push(start..records.len());
    }
    budget.consume(details.props.len())?;
    bsp.set_detail_props(&details)
        .map_err(|error| error.to_string())?;
    report.diagnostics.push(Diagnostic::info(
        format!(
            "Baked {} detail model/sprite colors and {} light-style contributions from direct light and filtered spherical world/terrain/sky radiance ({:?} shared base)",
            details.props.len(), records.len(), ctx.kind,
        ),
        None,
        None,
    ));
    Ok(Some(Channel {
        ranges,
        records,
        _allocation: allocation,
    }))
}

pub(super) fn finish(
    bsp: &mut Bsp,
    ldr: Option<&Channel>,
    hdr: Option<&Channel>,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let Some(ldr) = ldr else {
        return Ok(());
    };
    if ldr.records.is_empty() && hdr.is_none_or(|channel| channel.records.is_empty()) {
        return Ok(());
    }
    let raw = bsp
        .game_lumps
        .iter()
        .find(|lump| lump.id == *b"dprp")
        .ok_or("Detail style receiver records are missing")?;
    let leaf_bytes = bsp.lumps[source_formats::bsp::lump::LEAFS].data.len();
    let _output = budget.reserve_bytes(
        raw.data.len() * 3
            + leaf_bytes * 2
            + MAX_DETAIL_PROP_LIGHTING * size_of::<DetailPropLighting>() * 4,
    )?;
    budget.consume(raw.data.len() + leaf_bytes * 2)?;
    let mut details = bsp
        .detail_props()
        .map_err(|error| error.to_string())?
        .ok_or("Detail style receiver records are missing")?;
    if ldr.ranges.len() != details.props.len()
        || hdr.is_some_and(|channel| channel.ranges.len() != details.props.len())
    {
        return Err("Detail lighting channel receiver counts differ".into());
    }
    let channels = 1 + usize::from(hdr.is_some());
    let mut ldr_records = Vec::new();
    let mut hdr_records = Vec::new();
    for (index, prop) in details.props.iter_mut().enumerate() {
        budget.consume(1)?;
        let left = &ldr.records[ldr.ranges[index].clone()];
        let right = hdr.map_or(&[][..], |channel| {
            &channel.records[channel.ranges[index].clone()]
        });
        let (mut a, mut b) = (0, 0);
        let start = ldr_records.len();
        while a < left.len() || b < right.len() {
            budget.consume(1)?;
            if (ldr_records.len() + 1) * channels > MAX_DETAIL_PROP_LIGHTING {
                return Err("Detail lighting exceeds its 65,536 total style-record budget".into());
            }
            let style = left
                .get(a)
                .map_or(255, |value| value.style)
                .min(right.get(b).map_or(255, |value| value.style));
            let mut ldr_value = DetailPropLighting {
                style,
                ..Default::default()
            };
            let mut hdr_value = ldr_value;
            if left.get(a).is_some_and(|value| value.style == style) {
                ldr_value = left[a];
                a += 1;
            }
            if right.get(b).is_some_and(|value| value.style == style) {
                hdr_value = right[b];
                b += 1;
            }
            ldr_records.push(ldr_value);
            if hdr.is_some() {
                hdr_records.push(hdr_value);
            }
        }
        let count = ldr_records.len() - start;
        if count > 63 {
            return Err("Detail receiver exceeds 63 native light styles".into());
        }
        if count != 0 && prop.kind == 1 {
            crate::detail_props::check_native_sprite_transform(
                prop.scale,
                DVec3::from_array(prop.origin.map(f64::from)),
            )?;
        }
        budget.consume((count * 3 + 3) * channels)?;
        check_native_sum(prop.lighting, &ldr_records[start..])?;
        if hdr.is_some() {
            check_native_sum(prop.lighting, &hdr_records[start..])?;
        }
        prop.light_styles = if count == 0 { 0 } else { start as u32 };
        prop.light_style_count = count as u8;
    }
    budget.consume(ldr_records.len() + hdr_records.len())?;
    bsp.set_detail_props_with_lighting(&details, &ldr_records, hdr.map(|_| hdr_records.as_slice()))
        .map_err(|error| error.to_string())
}

fn native_color(color: RgbExp32) -> [f32; 3] {
    let scale = (2.0_f64.powi(i32::from(color.exponent)) / LIGHTING_SCALE) as f32;
    color.rgb.map(|value| f32::from(value) * scale)
}

fn check_native_sum(base: RgbExp32, extras: &[DetailPropLighting]) -> Result<(), String> {
    let mut color = native_color(base);
    for extra in extras {
        let scale = (2.0_f64.powi(i32::from(extra.lighting.exponent)) / LIGHTING_SCALE) as f32;
        for (total, value) in color.iter_mut().zip(extra.lighting.rgb) {
            *total += (f32::from(value) * MAX_AUTHORED_STYLE_GAIN) * scale;
            if !total.is_finite() {
                return Err("Detail lighting overflows native linear color when authored light styles reach their maximum a–z gain".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_ambient_retains_fractional_base_and_extra_style_radiance_once() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let (surfaces, shadows, props) =
            super::super::radiance::receiver_tests::transmitted_source(&mut budget);
        let colors = ambient(
            &surfaces,
            DVec3::Z * 16.0,
            0.0,
            &[-DVec3::Z],
            &shadows,
            &props,
            &mut budget,
        )
        .unwrap();
        assert!((colors[0] * 255.0).abs_diff_eq(
            DVec3::new(191.0, 95.87450980392157, 47.937254901960784),
            1e-12
        ));
        assert!((colors[7] * 255.0).abs_diff_eq(
            DVec3::new(47.937254901960784, 95.87450980392157, 143.81176470588235),
            1e-12
        ));
        assert!(
            colors
                .iter()
                .enumerate()
                .all(|(index, color)| index == 0 || index == 7 || *color == DVec3::ZERO)
        );
    }
    use source_formats::bsp::DetailSprite;

    #[test]
    fn named_model_receiver_replaces_only_the_point_and_preserves_stored_up_normal() {
        let prop = DetailProp {
            origin: [10.0, 20.0, 30.0],
            angles: [90.0, 0.0, 0.0],
            ..Default::default()
        };
        let target = DVec3::new(-40.125, 50.25, 60.5);
        let (point, normal) = receiver(
            &prop,
            &DetailProps::default(),
            &[DVec3::splat(15.0)],
            Some(target),
        )
        .unwrap();
        assert_eq!(point, target);
        assert!((normal - DVec3::X).length() < 1e-12);
        assert!(
            receiver(
                &DetailProp { kind: 1, ..prop },
                &DetailProps::default(),
                &[],
                Some(target)
            )
            .is_err()
        );
    }

    fn channel(budget: &mut Budget<'_>, props: usize, records: &[DetailPropLighting]) -> Channel {
        let allocation = budget
            .reserve_bytes(props * size_of::<std::ops::Range<usize>>() + size_of_val(records))
            .unwrap();
        Channel {
            ranges: vec![0..records.len(); props],
            records: records.to_vec(),
            _allocation: allocation,
        }
    }

    fn style_bsp(details: &DetailProps) -> Bsp {
        let mut bsp = Bsp::new();
        bsp.set_records(&[source_formats::bsp::Leaf::default()]);
        bsp.set_detail_props(details).unwrap();
        bsp
    }

    #[test]
    fn styled_sprite_native_transform_and_aggregate_union_cap_reject_atomically() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let records = [DetailPropLighting {
            style: 32,
            ..Default::default()
        }];
        let style = channel(&mut budget, 1, &records);
        let mut details = DetailProps {
            sprites: vec![DetailSprite {
                upper_left: [-10.0, 20.0],
                lower_right: [10.0, 0.0],
                ..Default::default()
            }],
            props: vec![DetailProp {
                kind: 1,
                orientation: 2,
                scale: f32::from_bits(103 << 23) * 0.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut bsp = style_bsp(&details);
        let before = bsp.to_bytes();
        let empty = channel(&mut budget, 1, &[]);
        finish(&mut bsp, Some(&empty), None, &mut budget).unwrap();
        assert_eq!(bsp.to_bytes(), before);
        assert!(
            finish(&mut bsp, Some(&style), None, &mut budget)
                .unwrap_err()
                .contains("half precision")
        );
        assert_eq!(bsp.to_bytes(), before);
        for (scale, origin, expected) in [
            (65505.0, [0.0; 3], "maximum"),
            (1.0, [0.0, 0.0, (MAX_COMPILE_COORD + 1.0) as f32], "origin"),
        ] {
            details.props[0].scale = scale;
            details.props[0].origin = origin;
            let mut bsp = style_bsp(&details);
            let before = bsp.to_bytes();
            finish(&mut bsp, Some(&empty), None, &mut budget).unwrap();
            assert_eq!(bsp.to_bytes(), before);
            assert!(
                finish(&mut bsp, Some(&style), None, &mut budget)
                    .unwrap_err()
                    .contains(expected)
            );
            assert_eq!(bsp.to_bytes(), before);
        }
        details.props[0].scale = f32::from_bits(103 << 23);
        details.props[0].origin = [0.0; 3];
        let mut bsp = style_bsp(&details);
        finish(&mut bsp, Some(&style), None, &mut budget).unwrap();
        assert_eq!(
            bsp.detail_props().unwrap().unwrap().props[0].light_style_count,
            1
        );

        let count = source_formats::bsp::MAX_DETAIL_PROPS;
        let records = (1..=3)
            .map(|style| DetailPropLighting {
                style,
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let ldr = channel(&mut budget, count, &records);
        let hdr = channel(&mut budget, count, &records);
        let details = DetailProps {
            models: vec![DetailProps::model_record("models/test.mdl").unwrap()],
            props: vec![DetailProp::default(); count],
            ..Default::default()
        };
        let mut bsp = style_bsp(&details);
        let before = bsp.to_bytes();
        assert!(
            finish(&mut bsp, Some(&ldr), Some(&hdr), &mut budget)
                .unwrap_err()
                .contains("65,536")
        );
        assert_eq!(bsp.to_bytes(), before);
    }

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
        let (point, normal) = receiver(&prop, &details, &[], None).unwrap();
        assert!((point - DVec3::new(4.0, 20.0, 42.0)).length() < 1e-12);
        assert!((normal - DVec3::Z).length() < 1e-12);
        for kind in [2, 3] {
            let prop = DetailProp {
                kind,
                angles: [90.0, 0.0, 0.0],
                shape_angle: 80,
                shape_size: 200,
                ..prop
            };
            let (point, normal) = receiver(&prop, &details, &[], None).unwrap();
            assert_eq!(point, DVec3::new(10.0, 20.0, 30.0));
            assert!((normal - DVec3::X).length() < 1e-12);
        }
    }

    #[test]
    fn both_style_arrays_check_final_shared_hdr_base_for_native_float_overflow() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let ldr = channel(
            &mut budget,
            1,
            &[DetailPropLighting {
                style: 32,
                lighting: RgbExp32 {
                    rgb: [128; 3],
                    exponent: 127,
                },
            }],
        );
        let hdr = channel(
            &mut budget,
            1,
            &[DetailPropLighting {
                style: 32,
                ..Default::default()
            }],
        );
        let details = DetailProps {
            models: vec![DetailProps::model_record("models/test.mdl").unwrap()],
            props: vec![DetailProp {
                lighting: RgbExp32 {
                    rgb: [255; 3],
                    exponent: 127,
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            native_color(details.props[0].lighting)
                .into_iter()
                .all(f32::is_finite)
        );
        assert!(check_native_sum(details.props[0].lighting, &hdr.records).is_ok());
        assert!(check_native_sum(RgbExp32::default(), &ldr.records).is_ok());
        let mut bsp = style_bsp(&details);
        let before = bsp.to_bytes();
        assert!(
            finish(&mut bsp, Some(&ldr), Some(&hdr), &mut budget)
                .unwrap_err()
                .contains("overflows native linear")
        );
        assert_eq!(bsp.to_bytes(), before);
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
            face_sources: &[],
            map: &map,
            tree: &tree,
            visibility: &visibility,
            kind: LightingKind::Ldr,
            model_centers: &[DVec3::ZERO],
            lighting_origins: &[],
            lighting_tints: &[],
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
                .err()
                .unwrap()
                .contains("512 MiB")
        );
        assert_eq!(bsp.to_bytes(), before);
    }
}
