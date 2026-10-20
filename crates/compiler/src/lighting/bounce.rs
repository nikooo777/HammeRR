use glam::DVec3;
use source_formats::bsp::{Bsp, Face, Lighting, LightingKind, RgbExp32, TexInfo};

use super::Occluder;
use super::bake::{Budget, Plan};
use super::shadows::ShadowIndex;
use crate::SkyQuality;
use crate::map::Map;

const LIGHTMAP_SCALE: f64 = 255.0;
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

pub(super) struct Context<'a> {
    pub face_sources: &'a [std::sync::Arc<[u32]>],
    pub kind: LightingKind,
    pub map: &'a Map<'a>,
    pub plans: &'a [Plan],
    pub faces: &'a mut [Face],
    pub infos: &'a [TexInfo],
    pub prop_shadows: &'a ShadowIndex<super::props::PropTriangle>,
    pub shadows: &'a ShadowIndex<Occluder>,
    pub model_shadows: &'a [Option<ShadowIndex<Occluder>>],
    pub quality: SkyQuality,
    pub measured_sources: bool,
}

pub(super) struct Stats {
    pub rays: usize,
    pub samples: usize,
    pub model_faces: usize,
    pub model_samples: usize,
}

pub(super) fn directions(count: usize) -> Vec<DVec3> {
    (0..count)
        .map(|index| {
            let fraction = (index as f64 + 0.5) / count as f64;
            let angle = index as f64 * GOLDEN_ANGLE;
            let radius = fraction.sqrt();
            DVec3::new(
                radius * angle.cos(),
                radius * angle.sin(),
                (1.0 - fraction).sqrt(),
            )
        })
        .collect()
}

pub(super) fn gather(
    normal: DVec3,
    directions: &[DVec3],
    sample: &mut impl FnMut(DVec3) -> Result<DVec3, String>,
) -> Result<DVec3, String> {
    let normal = normal
        .try_normalize()
        .ok_or("Bounce receiver needs a finite nonzero normal")?;
    let reference = if normal.z.abs() < 0.999 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let tangent = reference.cross(normal).normalize();
    let bitangent = normal.cross(tangent);
    let mut total = DVec3::ZERO;
    for direction in directions {
        total += sample(tangent * direction.x + bitangent * direction.y + normal * direction.z)?;
    }
    // Cosine-weighted directions already include the diffuse cosine and pi factors.
    Ok(total / directions.len() as f64)
}

pub(super) struct Direct {
    faces: Vec<Face>,
    colors: Vec<u8>,
    _allocation: super::bake::Allocation,
}

impl Direct {
    pub(super) fn new(
        faces: &[Face],
        colors: &[RgbExp32],
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        budget.consume(faces.len() + colors.len())?;
        let allocation =
            budget.reserve_bytes(std::mem::size_of_val(faces) + std::mem::size_of_val(colors))?;
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(colors));
        for color in colors {
            bytes.extend_from_slice(&color.rgb);
            bytes.push(color.exponent as u8);
        }
        Ok(Self {
            faces: faces.to_vec(),
            colors: bytes,
            _allocation: allocation,
        })
    }
}

pub(super) fn bake(
    ctx: &mut Context<'_>,
    bsp: &Bsp,
    direct: Option<&Direct>,
    colors: &mut Vec<RgbExp32>,
    budget: &mut Budget<'_>,
) -> Result<(Stats, super::radiance::SurfaceIndex), String> {
    for plan in ctx.plans.iter().filter(|plan| plan.model == 0) {
        budget.consume(1)?;
        let info = ctx.infos[ctx.faces[plan.face].texinfo as usize];
        let material = ctx
            .map
            .materials
            .get(ctx.map.texdata[info.texdata as usize]);
        let metrics = material.texture_metrics()?;
        if ctx.measured_sources && !metrics.source.has_reflectivity_source() {
            return Err(format!(
                "One-bounce prop lighting needs measured VTF or authored material reflectivity for material {}; provide its selected texture and reflectivity source or use Direct only",
                material.name
            ));
        }
        if metrics
            .reflectivity
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(format!(
                "One diffuse bounce needs material {} reflectivity between 0 and 1; use Direct only for this material",
                material.name
            ));
        }
    }
    let surfaces = super::radiance::build_styled(ctx.map, bsp, ctx.kind, ctx.face_sources, budget)?;
    let _directions = budget.reserve::<DVec3>(ctx.quality.directions())?;
    budget.consume(ctx.quality.directions())?;
    let directions = directions(ctx.quality.directions());
    let cone = (1.0 - 1.0 / directions.len() as f64).acos().tan();
    let mut stats = Stats {
        rays: 0,
        samples: 0,
        model_faces: 0,
        model_samples: 0,
    };
    let lighting = match direct {
        Some(direct) => Lighting::parse(&direct.colors).map_err(|error| error.to_string())?,
        None => bsp
            .lighting(ctx.kind)
            .map_err(|error| error.to_string())?
            .ok_or("Bounce source lighting is missing")?,
    };
    let _emitted_allocation = budget.reserve_bytes(source_formats::bsp::MAX_LIGHTING_BYTES)?;
    let mut emitted =
        Vec::with_capacity(source_formats::bsp::MAX_LIGHTING_BYTES / size_of::<RgbExp32>());
    let _face_allocation = budget.reserve::<Face>(ctx.faces.len())?;
    let current_faces;
    let direct_faces = match direct {
        Some(direct) => &direct.faces,
        None => {
            current_faces = bsp
                .lighting_faces(ctx.kind)
                .map_err(|error| error.to_string())?;
            &current_faces
        }
    };
    for plan in ctx.plans {
        budget.consume(1)?;
        stats.model_faces += usize::from(plan.model != 0);
        let face = direct_faces[plan.face];
        let info = ctx.infos[face.texinfo as usize];
        let source = lighting
            .face(&face, &info)
            .map_err(|error| error.to_string())?
            .ok_or("Bounce receiver has no direct map")?;
        let mut channels = super::styles::Channels::restore(&source, emitted.len(), budget)?;
        let [width, height] = plan.dimensions();
        for t in 0..height {
            for s in 0..width {
                let (point, normal) = plan.sample(s, t, budget)?;
                let origin = plan.origin(point, normal);
                let normals = plan.shading_normals(normal, budget)?;
                let mut colors = [[DVec3::ZERO; super::bump::MAPS]; 4];
                for (map, &normal) in normals.iter().enumerate().take(plan.map_count()) {
                    let bounce = gather(normal, &directions, &mut |direction| {
                        stats.rays += 1;
                        let hit = surfaces
                            .trace(
                                origin,
                                direction,
                                cone,
                                ctx.shadows,
                                ctx.prop_shadows,
                                budget,
                            )?
                            .filter(|hit| !hit.sky && hit.front);
                        match hit {
                            Some(hit)
                                if !super::bake::model_blocked(
                                    &ctx.model_shadows[plan.model],
                                    origin,
                                    hit.point,
                                    budget,
                                )? =>
                            {
                                let styles = surfaces.styled_colors(&hit, cone, budget)?;
                                for &(style, color) in
                                    styles.iter().skip(1).filter(|(style, _)| *style != 255)
                                {
                                    let mut contribution = [DVec3::ZERO; super::bump::MAPS];
                                    contribution[map] =
                                        color * (LIGHTMAP_SCALE / directions.len() as f64);
                                    channels.add(style, &contribution, &mut colors, budget)?;
                                }
                                Ok(hit.color * LIGHTMAP_SCALE)
                            }
                            _ => Ok(DVec3::ZERO),
                        }
                    })?;
                    let mut contribution = [DVec3::ZERO; super::bump::MAPS];
                    contribution[map] = bounce;
                    channels.add(0, &contribution, &mut colors, budget)?;
                }
                let positive = channels.finish_bounce_sample(t * width + s, &colors, budget)?;
                stats.samples += usize::from(positive);
                stats.model_samples += usize::from(plan.model != 0 && positive);
            }
        }
        let (styles, offset, _) = channels.emit(&mut emitted)?;
        ctx.faces[plan.face].styles = styles;
        ctx.faces[plan.face].light_offset = offset;
    }
    *colors = emitted;
    Ok((stats, surfaces))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_snapshot_preserves_channel_samples_and_checks_cancellation() {
        let control = crate::Control::default();
        let mut budget = Budget::new(&control);
        let colors = [
            RgbExp32 {
                rgb: [3, 7, 19],
                exponent: -4,
            },
            RgbExp32 {
                rgb: [11, 0, 200],
                exponent: 12,
            },
        ];
        let direct = Direct::new(&[Face::default()], &colors, &mut budget).unwrap();
        assert_eq!(Lighting::parse(&direct.colors).unwrap().samples(), colors);
        assert_eq!(direct.faces.len(), 1);
        control.cancel().unwrap();
        assert!(Direct::new(&[], &colors, &mut budget).is_err());
    }

    #[test]
    fn cosine_gather_preserves_uniform_reflected_energy_in_every_frame() {
        for quality in [SkyQuality::Fast, SkyQuality::Balanced, SkyQuality::Detailed] {
            let directions = directions(quality.directions());
            for normal in [
                DVec3::X,
                DVec3::NEG_Z,
                DVec3::new(1.0, 2.0, 3.0).normalize(),
            ] {
                let color = DVec3::new(100.0, 50.0, 25.0);
                let result = gather(normal, &directions, &mut |direction| {
                    assert!(
                        (direction.length() - 1.0).abs() < 1e-12 && direction.dot(normal) > 0.0
                    );
                    Ok(color)
                })
                .unwrap();
                assert_eq!(result, color);
                assert!(gather(normal, &directions, &mut |_| Err("cancelled".into())).is_err());
            }
        }
    }
}
