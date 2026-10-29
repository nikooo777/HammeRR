use glam::DVec3;
use source_formats::bsp::{Bsp, Face, Lighting, LightingKind, RgbExp32, TexInfo};

use super::Occluder;
use super::bake::{Budget, OriginContext, Plan};
use super::shadows::ShadowIndex;
use crate::SkyQuality;
use crate::map::Map;

const LIGHTMAP_SCALE: f64 = 255.0;
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

pub(super) struct Context<'a> {
    pub instances: &'a crate::instances::Prepared<'a>,
    pub report: &'a mut crate::Report,
    pub brush_textures: Option<&'a super::brush_alpha::Prepared>,
    pub face_sources: &'a [std::sync::Arc<[u32]>],
    pub kind: LightingKind,
    pub map: &'a Map<'a>,
    pub plans: &'a [Plan],
    pub model_scopes: &'a [super::models::Scope],
    pub faces: &'a mut [Face],
    pub infos: &'a [TexInfo],
    pub prop_shadows: &'a ShadowIndex<super::props::PropTriangle>,
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<super::terrain_shadows::TerrainTriangle>,
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
    for plan in ctx
        .plans
        .iter()
        .filter(|plan| ctx.model_scopes[plan.model].reflected_source)
    {
        budget.consume(1)?;
        let info = ctx.infos[ctx.faces[plan.face].texinfo as usize];
        let material = ctx
            .map
            .materials
            .get(ctx.map.texdata[info.texdata as usize]);
        let metrics = material.texture_metrics()?;
        if ctx.measured_sources && !metrics.reflectivity_source.is_measured_or_authored() {
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
    let surfaces = super::radiance::build_styled(
        ctx.map,
        bsp,
        ctx.model_scopes,
        ctx.kind,
        ctx.face_sources,
        ctx.brush_textures,
        ctx.terrain,
        ctx.shadows,
        budget,
    )?;
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
        let scene = super::transport::Scene {
            shadows: ctx.shadows,
            terrain: ctx.terrain,
            props: ctx.prop_shadows,
            model: ctx.model_shadows[plan.model].as_ref(),
        };
        let origin_context = OriginContext {
            map: ctx.map,
            face_sources: ctx.face_sources,
            scene: &scene,
        };
        for t in 0..height {
            for s in 0..width {
                let (point, normal) = plan.sample(s, t, budget)?;
                let origin = plan
                    .origin(&origin_context, [s, t], point, normal, budget)
                    .map_err(|message| {
                        let diagnostic = super::bake::channel_failure(
                            ctx.instances,
                            ctx.map,
                            ctx.face_sources,
                            plan,
                            point,
                            std::iter::empty(),
                            message,
                        );
                        let message = diagnostic.message.clone();
                        ctx.report.diagnostics.push(diagnostic);
                        message
                    })?;
                let normals = plan.shading_normals(normal, budget)?;
                let mut colors = [[DVec3::ZERO; super::bump::MAPS]; 4];
                for (map, &normal) in normals.iter().enumerate().take(plan.map_count()) {
                    let bounce = gather(normal, &directions, &mut |direction| {
                        stats.rays += 1;
                        let mut total = DVec3::ZERO;
                        surfaces.trace(
                            origin,
                            direction,
                            cone,
                            &scene,
                            None,
                            budget,
                            &mut |hit, budget| {
                                if hit.sky || !hit.front {
                                    return Ok(());
                                }
                                let styles = surfaces.styled_colors(hit, cone, budget)?;
                                for &(style, color) in
                                    styles.iter().skip(1).filter(|(style, _)| *style != 255)
                                {
                                    let mut contribution = [DVec3::ZERO; super::bump::MAPS];
                                    contribution[map] =
                                        color * (LIGHTMAP_SCALE / directions.len() as f64);
                                    channels
                                        .add(style, None, &contribution, &mut colors, budget)
                                        .map_err(|message| {
                                            let sources = channels
                                                .sources()
                                                .filter(|(present, _)| *present != style)
                                                .chain(std::iter::once((style, None)));
                                            let diagnostic = super::bake::channel_failure(
                                                ctx.instances,
                                                ctx.map,
                                                ctx.face_sources,
                                                plan,
                                                point,
                                                sources,
                                                message,
                                            );
                                            let message = diagnostic.message.clone();
                                            ctx.report.diagnostics.push(diagnostic);
                                            message
                                        })?;
                                }
                                total += hit.color * LIGHTMAP_SCALE;
                                Ok(())
                            },
                        )?;
                        Ok(total)
                    })?;
                    let mut contribution = [DVec3::ZERO; super::bump::MAPS];
                    contribution[map] = bounce;
                    channels.add(0, None, &contribution, &mut colors, budget)?;
                }
                let positive = channels.finish_bounce_sample(t * width + s, &colors, budget)?;
                budget.finish_sample();
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
