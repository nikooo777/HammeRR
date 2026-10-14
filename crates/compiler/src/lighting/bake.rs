use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::{
    self, Bsp, Edge, Face, LightingKind, MAX_LIGHTING_BYTES, MAX_WORLD_LIGHTS, Model, RgbExp32,
    TexInfo, contents, surf,
};

use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use super::sky::SkyLight;
use super::terrain::TerrainPlan;
use super::{LuxelFrame, Occluder, PointLight};
use crate::map::Map;
use crate::tree::Tree;
use crate::visibility::Visibility;
use crate::{Control, Diagnostic, Report};

const MAX_LIGHTING_WORK: usize = 100_000_000;
pub(super) const SAMPLE_BIAS: f64 = 0.5;

#[derive(Debug, Default)]
pub(crate) struct BakeStats {
    pub lights: usize,
    pub light_styles: usize,
    pub styled_faces: usize,
    pub light_style_maps: usize,
    pub faces: usize,
    pub models: usize,
    pub model_faces: usize,
    pub samples: usize,
    pub bytes: usize,
    pub work: usize,
    pub ambient_samples: usize,
    pub ambient_bytes: usize,
    pub bounces: u8,
    pub bounce_rays: usize,
    pub bounce_samples: usize,
    pub bounce_work: usize,
    pub smoothed_faces: usize,
    pub smoothed_corners: usize,
    pub normal_work: usize,
    pub culling: super::culling::Stats,
}

pub(super) struct Plan {
    pub face: usize,
    pub model: usize,
    offset: DVec3,
    surface: Surface,
}

enum Surface {
    Plane {
        frame: LuxelFrame,
        normal: DVec3,
        points: Vec<DVec3>,
        normals: Option<std::sync::Arc<super::normals::Field>>,
    },
    Terrain(TerrainPlan),
}

impl Plan {
    pub(super) fn dimensions(&self) -> [usize; 2] {
        match &self.surface {
            Surface::Plane { frame, .. } => frame.dimensions(),
            Surface::Terrain(plan) => plan.dimensions,
        }
    }

    pub(super) fn sample_count(&self) -> usize {
        let [width, height] = self.dimensions();
        width * height
    }

    pub(super) fn sample(
        &self,
        s: usize,
        t: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(DVec3, DVec3), String> {
        match &self.surface {
            Surface::Plane {
                frame,
                normal,
                points,
                normals,
            } => {
                budget.consume(points.len() + 1)?;
                let point = inside_or_closest(frame.position(s, t)?, points, *normal);
                let shading = if let Some(normals) = normals {
                    budget.consume(normals.point_count())?;
                    normals.sample(point, *normal)
                } else {
                    *normal
                };
                Ok((point + self.offset, shading))
            }
            Surface::Terrain(plan) => {
                budget.consume(1)?;
                let sample = &plan.samples[t * plan.dimensions[0] + s];
                Ok((sample.point, sample.normal))
            }
        }
    }
    pub(super) fn origin(&self, point: DVec3, normal: DVec3) -> DVec3 {
        let bias = match &self.surface {
            Surface::Plane { normal, .. } => *normal,
            Surface::Terrain(_) => normal,
        };
        point + bias * SAMPLE_BIAS
    }
}

pub(super) struct Budget<'a> {
    work: usize,
    control: &'a Control,
}

impl Budget<'_> {
    pub(super) fn new(control: &Control) -> Budget<'_> {
        Budget { work: 0, control }
    }

    pub(super) fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Direct lighting work count overflow")?;
        if self.work > MAX_LIGHTING_WORK {
            return Err("Direct LDR exceeded its 100-million work-unit limit; choose Fast sampling, larger lightmap scales, Direct only or Fullbright".into());
        }
        Ok(())
    }
}

fn inside_or_closest(point: DVec3, points: &[DVec3], normal: DVec3) -> DVec3 {
    let mut inside = true;
    let mut closest = points[0];
    let mut distance = f64::INFINITY;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        let edge = b - a;
        if normal.dot(edge.cross(point - a)) > 1e-6 {
            inside = false;
        }
        let squared = edge.length_squared();
        let t = if squared > 0.0 {
            ((point - a).dot(edge) / squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let candidate = a + t * edge;
        let candidate_distance = candidate.distance_squared(point);
        if candidate_distance < distance {
            closest = candidate;
            distance = candidate_distance;
        }
    }
    if inside { point } else { closest }
}

pub(super) enum Emitter {
    Point(PointLight),
    Sky(SkyLight),
}

impl Emitter {
    fn style(&self) -> u8 {
        match self {
            Self::Point(light) => light.style,
            Self::Sky(_) => 0,
        }
    }

    fn world_lights(&self) -> [Option<bsp::WorldLight>; 2] {
        match self {
            Self::Point(light) => [Some(light.world_light()), None],
            Self::Sky(light) => [Some(light.world_light()), light.ambient_world_light()],
        }
    }
}

fn lights(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    report: &mut Report,
    control: &Control,
) -> Result<Vec<Emitter>, String> {
    let mut lights = Vec::new();
    let mut targets = map_core::light::Targets::new();
    let mut sky_present = false;
    for entity in &map.entities {
        control.checkpoint()?;
        if let (Some((_, name)), Some(origin)) = (
            entity
                .properties
                .iter()
                .find(|(key, value)| key.eq_ignore_ascii_case("targetname") && !value.is_empty()),
            entity.origin,
        ) {
            targets.entry(name.as_str()).or_default().push(origin);
        }
    }
    for entity in &map.entities {
        control.checkpoint()?;
        if entity.classname.eq_ignore_ascii_case("light_environment") {
            if lights.len() == MAX_WORLD_LIGHTS {
                return Err("Direct lighting exceeds the 8,192 world-light limit".into());
            }
            let light = if sky_present {
                Err("Direct LDR accepts one visible light_environment; remove duplicate sky emitters".into())
            } else {
                SkyLight::from_entity(entity, &targets)
            };
            sky_present = true;
            match light {
                Ok(light) => lights.push(Emitter::Sky(light)),
                Err(error) => report.diagnostics.push(Diagnostic::error(
                    error,
                    Some(format!("entity:{}", entity.id)),
                    entity.origin,
                )),
            }
            continue;
        }
        if entity.classname.eq_ignore_ascii_case("light")
            || entity.classname.eq_ignore_ascii_case("light_spot")
        {
            if lights.len() == MAX_WORLD_LIGHTS {
                return Err("Direct lighting exceeds the 8,192 world-light limit".into());
            }
            let leaf = entity
                .origin
                .map(|origin| tree.leaf_at(origin, &map.planes));
            let cluster = leaf.and_then(|leaf| visibility.clusters[leaf]);
            let light =
                PointLight::from_entity(entity, cluster.map(|value| value as i32).unwrap_or(-1))
                    .and_then(|light| {
                        if entity.classname.eq_ignore_ascii_case("light_spot") {
                            light.with_spotlight(entity, &targets)
                        } else {
                            Ok(light)
                        }
                    });
            match light {
                Ok(light) if cluster.is_some() => lights.push(Emitter::Point(light)),
                Ok(_) => report.diagnostics.push(Diagnostic::warning("Light source is inside solid or unreachable space and does not illuminate the world", Some(format!("entity:{}", entity.id)), entity.origin)),
                Err(error) => report.diagnostics.push(Diagnostic::error(error, Some(format!("entity:{}", entity.id)), entity.origin)),
            }
        } else if entity.classname.to_ascii_lowercase().starts_with("light_") {
            report.diagnostics.push(Diagnostic::warning(
                format!(
                    "Direct LDR does not bake {} yet; static point/spot and direct sky emitters are supported",
                    entity.classname
                ),
                Some(format!("entity:{}", entity.id)),
                entity.origin,
            ));
        }
    }
    Ok(lights)
}

fn occluders(
    map: &Map,
    entity: usize,
    report: &mut Report,
    control: &Control,
) -> Result<Vec<Occluder>, String> {
    let mut result = Vec::new();
    let offset = map.entities[entity].origin.unwrap_or(DVec3::ZERO);
    for &index in &map.entities[entity].brushes {
        control.checkpoint()?;
        let brush = &map.brushes[index];
        if brush.contents & (contents::SOLID | contents::WINDOW | contents::GRATE) == 0 {
            continue;
        }
        if brush.contents & (contents::WINDOW | contents::TRANSLUCENT | contents::GRATE) != 0 {
            report.diagnostics.push(Diagnostic::warning("Direct LDR casts opaque shadows from translucent/grate brush hulls; material transmittance is unfinished", Some(format!("solid:{}", brush.id)), None));
        }
        let planes = brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| {
                let plane = map.planes.get(side.plane);
                Plane {
                    normal: plane.normal,
                    dist: plane.dist + plane.normal.dot(offset),
                }
            })
            .collect();
        let sky = brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| map.texinfos.infos[side.texinfo].flags & (surf::SKY | surf::SKY2D) != 0)
            .collect();
        result
            .push(Occluder::new(planes, brush.mins + offset, brush.maxs + offset)?.with_sky(sky)?);
    }
    Ok(result)
}

fn plans(
    map: &Map,
    bsp: &Bsp,
    faces: &mut [Face],
    infos: &mut Vec<TexInfo>,
    normals: &mut super::normals::Plan,
    scopes: &[super::models::Scope],
    budget: &mut Budget<'_>,
) -> Result<Vec<Plan>, String> {
    let planes = bsp
        .records::<bsp::Plane>()
        .map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    let mut plans = Vec::new();
    let mut sample_records: usize = 0;
    let mut terrain_fields =
        super::terrain::fields(&map.terrains, &mut |amount| budget.consume(amount))?;
    for (model, scope) in scopes.iter().enumerate() {
        for (local, face) in faces[scope.faces.clone()].iter_mut().enumerate() {
            let index = scope.faces.start + local;
            budget.consume(0)?;
            let info = infos[face.texinfo as usize];
            if face.dispinfo >= 0 {
                let terrain = &map.terrains[face.dispinfo as usize];
                let material = map.materials.get(map.texdata[info.texdata as usize]);
                if !material.lit() {
                    return Err(format!(
                        "Direct LDR terrain needs a lit material on solid:{} side:{}; use Fullbright for unlit terrain",
                        terrain.solid, terrain.side
                    ));
                }
                let dimensions = TerrainPlan::dimensions(terrain, &info).map_err(|error| {
                    format!("solid:{} side:{}: {error}", terrain.solid, terrain.side)
                })?;
                sample_records = sample_records
                    .checked_add(dimensions[0] * dimensions[1] + 1)
                    .filter(|&count| count <= MAX_LIGHTING_BYTES / size_of::<RgbExp32>())
                    .ok_or("Direct LDR lighting exceeds Source's 16 MiB limit")?;
                let terrain = TerrainPlan::new(
                    terrain,
                    &terrain_fields.positions[face.dispinfo as usize],
                    std::mem::take(&mut terrain_fields.normals[face.dispinfo as usize]),
                    dimensions,
                    &mut |amount| budget.consume(amount),
                )?;
                let texinfo = i16::try_from(infos.len())
                    .map_err(|_| "Terrain lighting texinfo count exceeds native short limits")?;
                infos.push(TexInfo {
                    flags: info.flags & !(surf::NOLIGHT | surf::BUMPLIGHT),
                    ..info
                });
                face.texinfo = texinfo;
                face.lightmap_size = dimensions.map(|value| (value - 1) as i32);
                if model != 0 {
                    return Err("Terrain lighting requires world-model faces".into());
                }
                plans.push(Plan {
                    face: index,
                    model,
                    offset: scope.offset,
                    surface: Surface::Terrain(terrain),
                });
                continue;
            }
            if info.flags & (surf::NOLIGHT | surf::SKY | surf::SKY2D | surf::WARP | surf::NODRAW)
                != 0
            {
                continue;
            }
            let plane = planes[face.plane as usize];
            let plane = Plane {
                normal: DVec3::from_array(plane.normal.map(f64::from)),
                dist: f64::from(plane.dist),
            };
            let frame = LuxelFrame::new(plane, &info, face.lightmap_mins, face.lightmap_size)?;
            sample_records = sample_records
                .checked_add(frame.sample_count() + 1)
                .ok_or("Direct lighting allocation count overflow")?;
            if sample_records > MAX_LIGHTING_BYTES / size_of::<RgbExp32>() {
                return Err("Direct LDR lighting exceeds Source's 16 MiB limit".into());
            }
            let points = surfedges
                [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
                .iter()
                .map(|&surfedge| {
                    let edge = edges[surfedge.unsigned_abs() as usize].0;
                    let vertex = edge[usize::from(surfedge < 0)];
                    DVec3::from_array(vertices[vertex as usize].map(f64::from))
                })
                .collect::<Vec<_>>();
            if points.len() < 3 || points.iter().any(|point| !point.is_finite()) {
                return Err("Direct lighting face has invalid corners".into());
            }
            plans.push(Plan {
                face: index,
                model,
                offset: scope.offset,
                surface: Surface::Plane {
                    frame,
                    normal: plane.normal.normalize(),
                    points,
                    normals: normals.fields.remove(&index),
                },
            });
        }
    }
    Ok(plans)
}

fn terrain_shadows(map: &Map, budget: &mut Budget<'_>) -> Result<ShadowIndex<Triangle>, String> {
    let mut triangles = Vec::new();
    for terrain in &map.terrains {
        budget.consume(terrain.vertices.len())?;
        let points = terrain.native_positions()?;
        for indices in &terrain.indices {
            budget.consume(1)?;
            if triangles.len() == Triangle::MAX_OBJECTS {
                return Err(format!(
                    "Direct LDR exceeds its {} terrain-shadow triangle limit",
                    Triangle::MAX_OBJECTS
                ));
            }
            triangles.push(Triangle::new(
                indices.map(|index| DVec3::from_array(points[index].map(f64::from))),
            )?);
        }
    }
    ShadowIndex::new(triangles, &mut |amount| budget.consume(amount))
}

fn sky_visible(
    direction: DVec3,
    origin: DVec3,
    shadows: &ShadowIndex<Occluder>,
    terrain: &ShadowIndex<Triangle>,
    model: &Option<ShadowIndex<Occluder>>,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    budget.consume(1)?;
    let end = origin + direction * super::sky::TRACE_LENGTH;
    if let Some(fraction) =
        shadows.sky_fraction(origin, end, &mut |amount| budget.consume(amount))?
    {
        let hit = origin.lerp(end, fraction) + direction * super::sky::SKY_HIT_BIAS;
        Ok(
            !terrain.blocked(origin, hit, &mut |amount| budget.consume(amount))?
                && !model_blocked(model, origin, hit, budget)?,
        )
    } else {
        Ok(false)
    }
}

fn model_blocked(
    model: &Option<ShadowIndex<Occluder>>,
    start: DVec3,
    end: DVec3,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    match model {
        Some(model) => model.blocked(start, end, &mut |amount| budget.consume(amount)),
        None => Ok(false),
    }
}

pub(crate) struct Settings<'a> {
    pub options: crate::Options,
    pub model_entities: &'a [usize],
    pub smoothing_sources: &'a [Option<std::sync::Arc<crate::faces::SmoothingSource>>],
}

pub(crate) fn bake(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    bsp: &mut Bsp,
    report: &mut Report,
    control: &Control,
    settings: Settings<'_>,
) -> Result<BakeStats, String> {
    let options = settings.options;
    let sky_quality = options.sky_quality;
    let lights = lights(map, tree, visibility, report, control)?;
    if report.has_errors() {
        return Ok(BakeStats::default());
    }
    if options.bounces != 0 && lights.iter().any(|light| light.style() != 0) {
        return Err("One diffuse bounce with switchable/animated light styles is unfinished; choose Direct only or Appearance Normal on unnamed lights".into());
    }
    let world_lights = lights
        .iter()
        .flat_map(Emitter::world_lights)
        .flatten()
        .collect::<Vec<_>>();
    if world_lights.len() > MAX_WORLD_LIGHTS {
        return Err(
            "Direct lighting exceeds the 8,192 world-light limit including sky ambient".into(),
        );
    }
    let world_hulls = occluders(map, 0, report, control)?;
    let mut faces = bsp.records::<Face>().map_err(|error| error.to_string())?;
    let mut infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let mut budget = Budget::new(control);
    let mut candidates = super::culling::Candidates::new(&lights, visibility, &mut budget)?;
    let model_records = bsp.records::<Model>().map_err(|error| error.to_string())?;
    let scopes = super::models::scopes(map, &model_records, settings.model_entities, faces.len())?;
    let normal_start = budget.work;
    let mut normals = super::normals::build(
        bsp,
        &faces,
        settings.smoothing_sources,
        &scopes,
        options.smoothing_angle,
        &mut budget,
    )?;
    let normal_work = budget.work - normal_start;
    let plans = plans(
        map,
        bsp,
        &mut faces,
        &mut infos,
        &mut normals,
        &scopes,
        &mut budget,
    )?;
    let mut has_receivers = vec![false; scopes.len()];
    let mut model_faces = 0;
    for plan in &plans {
        has_receivers[plan.model] = true;
        model_faces += usize::from(plan.model != 0);
    }
    let mut model_shadows = Vec::with_capacity(scopes.len());
    for (model, scope) in scopes.iter().enumerate() {
        let shadows = if model != 0 && has_receivers[model] {
            Some(ShadowIndex::new(
                occluders(map, scope.entity, report, control)?,
                &mut |amount| budget.consume(amount),
            )?)
        } else {
            None
        };
        model_shadows.push(shadows);
    }
    if model_faces != 0 && options.bounces != 0 {
        report.diagnostics.push(Diagnostic::warning(
            "Brush entities receive direct lightmaps; model bounce receivers remain unfinished",
            None,
            None,
        ));
    }
    if infos.iter().any(|info| info.flags & surf::BUMPLIGHT != 0) {
        report.diagnostics.push(Diagnostic::warning("Direct LDR disables bump-light sample maps; scoped world/model brush smoothing is supported, while bump-light baking remains unfinished", None, None));
        for info in &mut infos {
            info.flags &= !surf::BUMPLIGHT;
        }
    }

    if !map.terrains.is_empty() {
        report.diagnostics.push(Diagnostic::warning("Direct LDR sews terrain lighting normals across verified matching equal-power full edges; authored smoothing angles, mixed-power/partial-edge LOD and corner-only neighbors remain unfinished", None, None));
    }
    report.diagnostics.push(Diagnostic::info("Direct LDR bakes static, switchable and animated point/spot styles, sampled soft sun, sky ambient and model ambient with indexed opaque brush/terrain shadows; one optional diffuse world-light bounce is supported; brush entities receive pivot-aware direct maps with isolated self shadows; model bounces, multiple bounces, HDR and prop lightmaps remain unfinished", None, None));
    let count: usize = plans.iter().map(|plan| plan.sample_count() + 1).sum();
    let mut colors = Vec::with_capacity(count);
    let shadows = ShadowIndex::new(world_hulls, &mut |amount| budget.consume(amount))?;
    let terrain_shadows = terrain_shadows(map, &mut budget)?;
    let ambient_directions = super::sky::ambient_directions(sky_quality);
    let sun_directions = lights
        .iter()
        .find_map(|light| match light {
            Emitter::Sky(light) => Some(light.sun_directions(sky_quality)),
            Emitter::Point(_) => None,
        })
        .unwrap_or_default();
    if sun_directions.len() > 1 {
        budget.consume(sun_directions.len())?;
    }
    let mut used_styles = std::collections::BTreeSet::new();
    let mut styled_faces = 0;
    let mut style_maps = 0;
    for plan in &plans {
        budget.consume(1)?;
        let mut channels = super::styles::Channels::new(plan.sample_count(), colors.len())?;
        let [width, height] = plan.dimensions();
        for t in 0..height {
            for s in 0..width {
                let (point, normal) = plan.sample(s, t, &mut budget)?;
                let origin = plan.origin(point, normal);
                let mut color = [DVec3::ZERO; 4];
                let cluster = candidates.cluster(origin, map, tree, visibility, &mut budget)?;
                let culling_enabled = candidates.enabled();
                let mut evaluated = 0;
                let mut range_skipped = 0;
                for &index in candidates.for_cluster(cluster, &lights, &mut budget)? {
                    let light = &lights[index];
                    budget.consume(1)?;
                    if culling_enabled && super::culling::outside_range(light, point) {
                        range_skipped += 1;
                        continue;
                    }
                    evaluated += 1;
                    if let Emitter::Sky(light) = light
                        && light.has_ambient()
                    {
                        budget.consume(ambient_directions.len())?;
                        let contribution = light.ambient_contribution(
                            normal,
                            &ambient_directions,
                            &mut |direction| {
                                sky_visible(
                                    direction,
                                    origin,
                                    &shadows,
                                    &terrain_shadows,
                                    &model_shadows[plan.model],
                                    &mut budget,
                                )
                            },
                        )?;
                        channels.add(0, contribution, &mut color, &mut budget)?;
                    }
                    match light {
                        Emitter::Point(light) => {
                            let contribution = light.contribution(point, normal)?;
                            if contribution != DVec3::ZERO
                                && !shadows.blocked(origin, light.origin, &mut |amount| {
                                    budget.consume(amount)
                                })?
                                && !terrain_shadows.blocked(
                                    origin,
                                    light.origin,
                                    &mut |amount| budget.consume(amount),
                                )?
                                && !model_blocked(
                                    &model_shadows[plan.model],
                                    origin,
                                    light.origin,
                                    &mut budget,
                                )?
                            {
                                channels.add(light.style, contribution, &mut color, &mut budget)?;
                            }
                        }
                        Emitter::Sky(light) => {
                            let contribution = light.sun_contribution(
                                normal,
                                &sun_directions,
                                &mut |direction| {
                                    sky_visible(
                                        direction,
                                        origin,
                                        &shadows,
                                        &terrain_shadows,
                                        &model_shadows[plan.model],
                                        &mut budget,
                                    )
                                },
                            )?;
                            channels.add(0, contribution, &mut color, &mut budget)?;
                        }
                    }
                }
                candidates.stats.evaluated += evaluated;
                candidates.stats.range_skipped += range_skipped;
                channels.finish_sample(t * width + s, &color, &mut budget)?;
            }
        }
        let (styles, offset, channels) = channels
            .emit(&mut colors)
            .map_err(|error| format!("face:{}: {error}", plan.face))?;
        used_styles.extend(styles.into_iter().filter(|&style| style != 255));
        styled_faces += usize::from(channels > 1);
        style_maps += channels;
        faces[plan.face].styles = styles;
        faces[plan.face].light_offset = offset;
    }
    control.checkpoint()?;
    bsp.set_lighting(LightingKind::Ldr, &colors)
        .map_err(|error| error.to_string())?;
    bsp.set_world_lights(LightingKind::Ldr, &world_lights)
        .map_err(|error| error.to_string())?;
    bsp.set_records(&faces);
    bsp.set_records(&infos);
    let bounce_start = budget.work;
    let bounce = if options.bounces == 1 {
        let stats = super::bounce::bake(
            &super::bounce::Context {
                map,
                plans: &plans,
                faces: &faces,
                infos: &infos,
                shadows: &shadows,
                quality: sky_quality,
            },
            bsp,
            &mut colors,
            &mut budget,
        )?;
        bsp.set_lighting(LightingKind::Ldr, &colors)
            .map_err(|error| error.to_string())?;
        stats
    } else {
        super::bounce::Stats {
            rays: 0,
            samples: 0,
        }
    };
    let bounce_work = budget.work - bounce_start;
    if !map.terrains.is_empty() {
        let mut data = bsp
            .displacements()
            .map_err(|error| error.to_string())?
            .ok_or("Terrain records are missing")?;
        let mut bytes = Vec::new();
        let mut missing = 0;
        for plan in &plans {
            let Surface::Terrain(terrain) = &plan.surface else {
                continue;
            };
            budget.consume(terrain.samples.len())?;
            missing += terrain
                .samples
                .iter()
                .filter(|sample| sample.record.is_none())
                .count();
            let index = faces[plan.face].dispinfo as usize;
            data.infos[index].lightmap_sample_start = i32::try_from(bytes.len())
                .map_err(|_| "Terrain sample offset exceeds native limits")?;
            let records = terrain
                .samples
                .iter()
                .map(|sample| {
                    sample.record.unwrap_or(bsp::DisplacementLightmapSample {
                        triangle: 0,
                        weights: [0; 3],
                    })
                })
                .collect::<Vec<_>>();
            bytes.extend(
                data.infos[index]
                    .encode_lightmap_samples(&records)
                    .map_err(|error| error.to_string())?,
            );
        }
        if missing != 0 {
            report.diagnostics.push(Diagnostic::info(format!("{missing} power-4 terrain luxels use missing auxiliary records because native triangle 511 is unrepresentable; their lighting uses full-precision surface geometry"), None, None));
        }
        bsp.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data = bytes;
        bsp.set_displacements(&data)
            .map_err(|error| error.to_string())?;
    }
    let sky_radiance = lights
        .iter()
        .find_map(|light| match light {
            Emitter::Sky(light) => Some(light.ambient_radiance()),
            Emitter::Point(_) => None,
        })
        .unwrap_or(DVec3::ZERO);
    let ambient = super::probes::bake(
        &super::probes::Context {
            map,
            tree,
            visibility,
            shadows: &shadows,
            terrain: &terrain_shadows,
            directions: &ambient_directions,
            quality: sky_quality,
            sky_radiance,
        },
        bsp,
        report,
        &mut budget,
    )?;
    Ok(BakeStats {
        lights: world_lights.len(),
        light_styles: used_styles.len(),
        styled_faces,
        light_style_maps: style_maps,
        faces: plans.len(),
        models: has_receivers.iter().skip(1).filter(|&&lit| lit).count(),
        model_faces,
        samples: count - plans.len(),
        bytes: colors.len() * size_of::<RgbExp32>(),
        work: budget.work,
        ambient_samples: ambient.samples,
        ambient_bytes: ambient.bytes,
        bounces: options.bounces,
        bounce_rays: bounce.rays,
        bounce_samples: bounce.samples,
        bounce_work,
        smoothed_faces: normals.faces,
        smoothed_corners: normals.corners,
        normal_work,
        culling: candidates.stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_work_and_cancellation_stop_before_sampling() {
        let control = Control::default();
        let mut budget = Budget {
            work: MAX_LIGHTING_WORK,
            control: &control,
        };
        assert!(budget.consume(1).is_err());
        let mut budget = Budget {
            work: 0,
            control: &control,
        };
        control.cancel().unwrap();
        assert!(budget.consume(1).is_err());
        assert_eq!(budget.work, 0);
    }

    #[test]
    fn outside_samples_clamp_to_the_polygon_without_leaving_its_plane() {
        let points = [
            DVec3::ZERO,
            DVec3::Y * 16.0,
            DVec3::new(16.0, 16.0, 0.0),
            DVec3::X * 16.0,
        ];
        assert_eq!(
            inside_or_closest(DVec3::new(4.0, 8.0, 0.0), &points, DVec3::Z),
            DVec3::new(4.0, 8.0, 0.0)
        );
        assert_eq!(
            inside_or_closest(DVec3::new(-4.0, 8.0, 0.0), &points, DVec3::Z),
            DVec3::new(0.0, 8.0, 0.0)
        );
        assert_eq!(
            inside_or_closest(DVec3::new(20.0, 20.0, 0.0), &points, DVec3::Z),
            DVec3::new(16.0, 16.0, 0.0)
        );
    }
}
