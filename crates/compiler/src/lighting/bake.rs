use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::{
    self, Bsp, Edge, Face, LightingKind, MAX_LIGHTING_BYTES, MAX_WORLD_LIGHTS, Model, RgbExp32,
    TexInfo, contents, surf,
};

use super::progress::Contributor;
use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use super::sky::SkyLight;
use super::terrain::TerrainPlan;
use super::{
    LightingChannel, LightingContributor, LightingPhase, LightingProgress, LuxelFrame,
    MAX_LIGHTING_CONTRIBUTORS, Occluder, PointLight,
};
use crate::brush_model_pose::Pose;
use crate::map::{MAX_COMPILE_COORD, Map};
use crate::tree::Tree;
use crate::visibility::Visibility;
use crate::{Control, Diagnostic, Report};

pub(super) const MAX_LIGHTING_LIVE_BYTES: usize = 512 * 1024 * 1024;
const MAX_TOTAL_LIGHTING_BYTES: usize = 2 * MAX_LIGHTING_BYTES;
const MAX_CHANNEL_AMBIENT_BYTES: usize =
    65_536 * (size_of::<bsp::LeafAmbientIndex>() + size_of::<bsp::LeafAmbientSample>());
const MAX_CHANNEL_SAMPLES: usize = MAX_LIGHTING_BYTES / size_of::<RgbExp32>();
pub(super) const SAMPLE_BIAS: f64 = 0.5;

#[derive(Debug, Default)]
pub(crate) struct BakeStats {
    pub brush_shadow_models: usize,
    pub prop_lighting_assets: Option<crate::prop_lighting_assets::PropLightingPlan>,
    pub prop_vertex_lighting: Option<crate::PropVertexLightingStats>,
    pub hdr: Option<crate::HdrLightingStats>,
    pub total_work: usize,
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
    pub model_bounce_faces: usize,
    pub model_bounce_samples: usize,
    pub bump_faces: usize,
    pub bump_lightmaps: usize,
    pub smoothed_faces: usize,
    pub smoothed_corners: usize,
    pub normal_work: usize,
    pub prop_shadows: super::props::Stats,
    pub culling: super::culling::Stats,
}

pub(super) struct Plan {
    pub face: usize,
    pub model: usize,
    pose: Pose,
    surface: Surface,
    bump: Option<super::bump::Basis>,
}

enum Surface {
    Plane {
        frame: LuxelFrame,
        normal: DVec3,
        world_normal: DVec3,
        points: Vec<DVec3>,
        normals: Option<std::sync::Arc<super::normals::Field>>,
    },
    Terrain(TerrainPlan),
}

impl Plan {
    fn apply_pose(&mut self, pose: Pose, budget: &mut Budget<'_>) -> Result<(), String> {
        let Some(rotation) = pose.rotation else {
            return Ok(());
        };
        let Surface::Plane {
            normal,
            world_normal,
            points,
            ..
        } = &mut self.surface
        else {
            return Err("Rotated brush-model lighting cannot contain terrain".into());
        };
        for &point in points.iter() {
            budget.consume(1)?;
            checked_model_point(pose.point(point))?;
        }
        budget.consume(1)?;
        let transformed_normal = pose.normal(*normal);
        if !transformed_normal.is_finite() || transformed_normal.length_squared() == 0.0 {
            return Err("Rotated brush-model lighting needs a finite nonzero normal".into());
        }
        if let Some(basis) = &mut self.bump {
            budget.consume(1)?;
            basis.rotate(rotation);
        }
        *world_normal = transformed_normal;
        self.pose = pose;
        Ok(())
    }

    pub(super) fn map_count(&self) -> usize {
        if self.bump.is_some() {
            super::bump::MAPS
        } else {
            1
        }
    }

    pub(super) fn shading_normals(
        &self,
        normal: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<[DVec3; super::bump::MAPS], String> {
        match &self.bump {
            Some(basis) => {
                budget.consume(12)?;
                basis.normals(normal)
            }
            None => Ok([normal; super::bump::MAPS]),
        }
    }

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
                ..
            } => {
                budget.consume(points.len() + 1)?;
                let point = inside_or_closest(frame.position(s, t)?, points, *normal);
                let shading = if let Some(normals) = normals {
                    budget.consume(normals.point_count())?;
                    normals.sample(point, *normal)
                } else {
                    *normal
                };
                if self.pose.rotation.is_some() {
                    budget.consume(2)?;
                    let point = self.pose.point(point);
                    let shading = self.pose.normal(shading);
                    checked_model_point(point)?;
                    if !shading.is_finite() || shading.length_squared() == 0.0 {
                        return Err(
                            "Rotated brush-model lighting needs a finite nonzero shading normal"
                                .into(),
                        );
                    }
                    Ok((point, shading))
                } else {
                    Ok((point + self.pose.offset, shading))
                }
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
            Surface::Plane { world_normal, .. } => *world_normal,
            Surface::Terrain(_) => normal,
        };
        point + bias * SAMPLE_BIAS
    }
}

fn checked_model_point(point: DVec3) -> Result<(), String> {
    if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
        return Err(format!(
            "Rotated brush-model lighting geometry exceeds finite Source bounds ±{MAX_COMPILE_COORD}"
        ));
    }
    Ok(())
}

pub(super) struct Budget<'a> {
    work: usize,
    progress: LightingProgress,
    quality: crate::SkyQuality,
    dual: bool,
    contributor: Contributor,
    source: Option<(u32, &'static str)>,
    control: &'a Control,
    live_bytes: std::rc::Rc<std::cell::Cell<usize>>,
}

pub(super) struct Allocation {
    bytes: usize,
    live_bytes: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Allocation {
    pub(super) fn grow(&mut self, budget: &Budget<'_>, bytes: usize) -> Result<(), String> {
        if !std::rc::Rc::ptr_eq(&self.live_bytes, &budget.live_bytes) {
            return Err("Lighting allocation belongs to another budget".into());
        }
        let mut additional = budget.reserve_bytes(bytes)?;
        self.bytes += additional.bytes;
        additional.bytes = 0;
        Ok(())
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        self.live_bytes.set(self.live_bytes.get() - self.bytes);
    }
}

impl Budget<'_> {
    #[cfg(test)]
    pub(super) fn new(control: &Control) -> Budget<'_> {
        Self::with_options(control, crate::Options::default())
    }

    #[cfg(test)]
    pub(super) fn with_limit(control: &Control, limit: u64) -> Budget<'_> {
        Self::with_options(
            control,
            crate::Options {
                lighting_work_limit: limit,
                ..Default::default()
            },
        )
    }

    fn with_options(control: &Control, options: crate::Options) -> Budget<'_> {
        Budget {
            work: 0,
            progress: LightingProgress::new(options.lighting_work_limit, options.sky_quality),
            quality: options.sky_quality,
            dual: options.lighting == crate::LightingMode::DirectLdrHdr,
            contributor: Contributor::Unattributed,
            source: None,
            control,
            live_bytes: Default::default(),
        }
    }

    fn channel(&mut self, kind: LightingKind) {
        self.progress.current_channel = match kind {
            LightingKind::Ldr => LightingChannel::Ldr,
            LightingKind::Hdr => LightingChannel::Hdr,
        };
        self.phase(LightingPhase::Preparation);
    }

    pub(super) fn phase(&mut self, phase: LightingPhase) {
        self.progress.current_phase = phase;
        self.clear_emitter();
    }

    pub(super) fn context(&self) -> (LightingPhase, Option<(u32, &'static str)>) {
        (self.progress.current_phase, self.source)
    }

    pub(super) fn restore(&mut self, context: (LightingPhase, Option<(u32, &'static str)>)) {
        self.phase(context.0);
        if let Some((id, classname)) = context.1 {
            self.source(id, classname);
        }
    }

    pub(super) fn emitter(&mut self, emitter: &Emitter) {
        match emitter {
            Emitter::Point(light) => self.source(
                light.source_id,
                if light.cone.is_some() {
                    "light_spot"
                } else {
                    "light"
                },
            ),
            Emitter::Sky(light) => self.source(light.source_id, "light_environment"),
        }
    }

    fn source(&mut self, id: u32, classname: &'static str) {
        self.source = Some((id, classname));
        let channel = self.progress.current_channel;
        let phase = self.progress.current_phase;
        let index = self.progress.contributors.iter().position(|entry| {
            entry.entity_id == id && entry.channel == channel && entry.phase == phase
        });
        let index = index.or_else(|| {
            if self.progress.contributors.len() == MAX_LIGHTING_CONTRIBUTORS {
                self.progress.contributors_truncated = true;
                return None;
            }
            let index = self.progress.contributors.len();
            self.progress.contributors.push(LightingContributor {
                entity_id: id,
                compiled_entity_id: id,
                instance: None,
                classname,
                channel,
                phase,
                work: 0,
            });
            Some(index)
        });
        self.contributor = index.map_or(Contributor::Other, Contributor::Retained);
    }

    pub(super) fn clear_emitter(&mut self) {
        self.source = None;
        self.contributor = Contributor::Unattributed;
    }

    fn plan_lightmap_samples(&mut self, count: usize) -> Result<(), String> {
        let count = count as u64;
        let ldr = self
            .progress
            .ldr_lightmap_samples_planned
            .checked_add(count)
            .ok_or("Lighting planned sample count overflow")?;
        let hdr = self
            .progress
            .hdr_lightmap_samples_planned
            .checked_add(if self.dual { count } else { 0 })
            .ok_or("Lighting planned sample count overflow")?;
        self.progress.lightmap_samples_planned = ldr
            .checked_add(hdr)
            .ok_or("Lighting planned sample count overflow")?;
        self.progress.ldr_lightmap_samples_planned = ldr;
        self.progress.hdr_lightmap_samples_planned = hdr;
        for phase in &mut self.progress.phases {
            if phase.phase == LightingPhase::Direct {
                phase.samples_planned = match phase.channel {
                    LightingChannel::Ldr => ldr,
                    LightingChannel::Hdr => hdr,
                    LightingChannel::Preparation => 0,
                };
            }
        }
        Ok(())
    }

    pub(super) fn plan_samples(&mut self, count: usize) -> Result<(), String> {
        let index = self.progress.phase_index();
        let entry = &mut self.progress.phases[index];
        entry.samples_planned = entry
            .samples_planned
            .checked_add(count as u64)
            .ok_or("Lighting planned sample count overflow")?;
        Ok(())
    }

    pub(super) fn finish_sample(&mut self) {
        let index = self.progress.phase_index();
        self.progress.phases[index].samples_completed += 1;
        if self.progress.current_phase == LightingPhase::Direct {
            self.progress.lightmap_samples_completed += 1;
            match self.progress.current_channel {
                LightingChannel::Ldr => self.progress.ldr_lightmap_samples_completed += 1,
                LightingChannel::Hdr => self.progress.hdr_lightmap_samples_completed += 1,
                LightingChannel::Preparation => {}
            }
        }
    }

    pub(super) fn samples_planned_complete(&mut self) {
        let index = self.progress.phase_index();
        self.progress.phases[index].samples_planned_complete = true;
    }

    pub(super) fn reserve<T>(&self, count: usize) -> Result<Allocation, String> {
        let bytes = count
            .checked_mul(size_of::<T>())
            .ok_or("Lighting allocation size overflow")?;
        self.reserve_bytes(bytes)
    }

    pub(super) fn reserve_bytes(&self, bytes: usize) -> Result<Allocation, String> {
        self.control.checkpoint()?;
        let live = self
            .live_bytes
            .get()
            .checked_add(bytes)
            .filter(|&live| live <= MAX_LIGHTING_LIVE_BYTES)
            .ok_or("Lighting exceeds its shared 512 MiB live channel-allocation budget")?;
        self.live_bytes.set(live);
        Ok(Allocation {
            bytes,
            live_bytes: self.live_bytes.clone(),
        })
    }

    #[cfg(test)]
    pub(super) fn live_bytes(&self) -> usize {
        self.live_bytes.get()
    }

    pub(super) fn work(&self) -> usize {
        self.work
    }

    pub(super) fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Direct lighting work count overflow")?;
        self.progress.record(amount as u64, self.contributor)?;
        if self.progress.work > self.progress.limit {
            let sampling = if self.quality == crate::SkyQuality::Fast {
                ""
            } else {
                "choose Fast sampling, "
            };
            let allowance = if self.progress.limit < crate::MAX_LIGHTING_WORK_LIMIT {
                format!(
                    "increase the lighting work limit (maximum {}), ",
                    crate::MAX_LIGHTING_WORK_LIMIT
                )
            } else {
                String::new()
            };
            return Err(format!(
                "Lighting exceeded its shared {} work-unit limit after {} attempted units ({:?}/{:?}, {:?} sampling); {}{}use fewer bounces, point-light distance limits, larger lightmap scales or Fullbright",
                self.progress.limit,
                self.progress.work,
                self.progress.current_channel,
                self.progress.current_phase,
                self.quality,
                allowance,
                sampling,
            ));
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
    kind: LightingKind,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Vec<Emitter>, String> {
    let mut lights = Vec::new();
    let mut targets = map_core::light::Targets::new();
    let mut sky_present = false;
    for entity in &map.entities {
        budget.consume(1)?;
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
        budget.consume(1)?;
        if entity.classname.eq_ignore_ascii_case("light_environment") {
            if lights.len() == MAX_WORLD_LIGHTS {
                return Err("Direct lighting exceeds the 8,192 world-light limit".into());
            }
            let light = if sky_present {
                Err("Direct LDR accepts one visible light_environment; remove duplicate sky emitters".into())
            } else {
                match kind {
                    LightingKind::Ldr => SkyLight::from_entity(entity, &targets),
                    LightingKind::Hdr => SkyLight::from_entity_kind(entity, &targets, kind),
                }
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
            let light = PointLight::from_entity_kind(
                entity,
                cluster.map(|value| value as i32).unwrap_or(-1),
                kind,
            )
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

#[cfg(test)]
pub(super) fn occluders(
    map: &Map,
    entity: usize,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Vec<Occluder>, String> {
    posed_occluders(
        map,
        entity,
        Pose {
            offset: map.entities[entity].origin.unwrap_or(DVec3::ZERO),
            rotation: None,
        },
        report,
        budget,
        None,
    )
}

fn posed_occluders(
    map: &Map,
    entity: usize,
    pose: Pose,
    report: &mut Report,
    budget: &mut Budget<'_>,
    brush_textures: Option<&super::brush_alpha::Prepared>,
) -> Result<Vec<Occluder>, String> {
    let mut result = Vec::new();
    for &index in &map.entities[entity].brushes {
        budget.consume(1)?;
        let brush = &map.brushes[index];
        if brush.contents & (contents::SOLID | contents::WINDOW | contents::GRATE) == 0 {
            continue;
        }
        if brush.contents & (contents::WINDOW | contents::TRANSLUCENT | contents::GRATE) != 0
            && brush_textures.is_none_or(|textures| {
                !brush
                    .sides
                    .iter()
                    .any(|side| textures.contains(side.material))
            })
        {
            report.diagnostics.push(Diagnostic::warning("Direct LDR casts opaque shadows from translucent/grate brush hulls; material transmittance is unfinished", Some(format!("solid:{}", brush.id)), None));
        }
        budget.consume(brush.sides.len())?;
        let planes = brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| {
                let plane = map.planes.get(side.plane);
                if pose.rotation.is_some() {
                    budget.consume(1)?;
                }
                let normal = pose.normal(plane.normal);
                Ok(Plane {
                    normal,
                    dist: plane.dist + normal.dot(pose.offset),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let sky = brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| map.texinfos.infos[side.texinfo].flags & (surf::SKY | surf::SKY2D) != 0)
            .collect();
        let sources = brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| super::HullSideSource {
                plane: side.plane,
                side: side.source_id,
            })
            .collect();
        let (mins, maxs) = if pose.rotation.is_some() {
            budget.consume(16)?;
            pose.raw_and_stored_bounds(brush.mins, brush.maxs)?
        } else {
            (brush.mins + pose.offset, brush.maxs + pose.offset)
        };
        let mut hull = Occluder::new(planes, mins, maxs)?
            .with_sky(sky)?
            .with_source(index, sources)?;
        if let Some(textures) = brush_textures {
            if entity != 0 {
                budget.consume(brush.sides.len())?;
            }
            let faces = brush
                .sides
                .iter()
                .filter(|side| !side.bevel)
                .map(|side| {
                    textures.face(side.material, map.texinfos.infos[side.texinfo].texture_vecs)
                })
                .collect();
            hull = hull.with_optical(faces, (entity != 0).then_some(pose))?;
        }
        result.push(hull);
    }
    Ok(result)
}

fn global_casters(
    map: &Map,
    scopes: &[super::models::Scope],
    hulls: &mut Vec<Occluder>,
    report: &mut Report,
    budget: &mut Budget<'_>,
    textures: Option<&super::brush_alpha::Prepared>,
) -> Result<(usize, Option<Allocation>), String> {
    let caster_brushes = scopes
        .iter()
        .filter(|scope| {
            scope.entity != 0
                && crate::brush_shadow_casters::enabled(&map.entities[scope.entity].properties)
        })
        .try_fold(0usize, |count, scope| {
            count.checked_add(map.entities[scope.entity].brushes.len())
        })
        .ok_or("Brush global shadows hull count overflow")?;
    let allocation = if caster_brushes == 0 {
        None
    } else {
        Some(
            budget.reserve_bytes(
                caster_brushes
                    .checked_mul(4 * size_of::<Occluder>() + size_of::<super::OpticalHull>())
                    .ok_or("Brush global shadows allocation overflow")?,
            )?,
        )
    };
    let mut brush_shadow_models = 0;
    if caster_brushes != 0 {
        budget.consume(caster_brushes)?;
        hulls
            .try_reserve_exact(caster_brushes)
            .map_err(|error| format!("Brush global shadows hull allocation: {error}"))?;
        for scope in scopes.iter().filter(|scope| scope.entity != 0) {
            budget.consume(1)?;
            if !crate::brush_shadow_casters::enabled(&map.entities[scope.entity].properties) {
                continue;
            }
            let textures = if let Some(textures) = textures
                && textures.has_global_alpha(scope.entity, budget)?
            {
                Some(textures)
            } else {
                None
            };
            let prepared =
                posed_occluders(map, scope.entity, scope.pose, report, budget, textures)?;
            if prepared.len() != map.entities[scope.entity].brushes.len() || prepared.is_empty() {
                return Err(
                    "Brush global shadows: requested model has no complete caster hull set".into(),
                );
            }
            for hull in prepared {
                budget.consume(1)?;
                hulls.push(hull.into_global_caster());
            }
            brush_shadow_models += 1;
        }
        report.diagnostics.push(Diagnostic::info(format!("Brush global shadows: {brush_shadow_models} stationary brush models cast into global baked lighting; supported binary cutouts use brush texture shadows, world occupancy and model radiance sources are unchanged"), None, None));
    }
    Ok((brush_shadow_models, allocation))
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
                let info = TexInfo {
                    flags: info.flags | (material.surface & surf::BUMPLIGHT),
                    ..info
                };
                let dimensions = TerrainPlan::dimensions(terrain, &info).map_err(|error| {
                    format!("solid:{} side:{}: {error}", terrain.solid, terrain.side)
                })?;
                let bump = super::bump::Basis::new(
                    &info,
                    DVec3::from_array(planes[face.plane as usize].normal.map(f64::from)),
                )?;
                let maps = if bump.is_some() { super::bump::MAPS } else { 1 };
                sample_records = sample_records
                    .checked_add(dimensions[0] * dimensions[1] * maps + 1)
                    .filter(|&count| count <= MAX_LIGHTING_BYTES / size_of::<RgbExp32>())
                    .ok_or("Direct LDR lighting exceeds Source's 16 MiB limit")?;
                budget.plan_lightmap_samples(dimensions[0] * dimensions[1])?;
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
                    flags: info.flags & !surf::NOLIGHT,
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
                    pose: scope.pose,
                    surface: Surface::Terrain(terrain),
                    bump,
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
            let bump = super::bump::Basis::new(&info, plane.normal)?;
            let maps = if bump.is_some() { super::bump::MAPS } else { 1 };
            sample_records = sample_records
                .checked_add(frame.sample_count() * maps + 1)
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
            let normal = plane.normal.normalize();
            budget.plan_lightmap_samples(frame.sample_count())?;
            plans.push(Plan {
                face: index,
                model,
                pose: scope.pose,
                surface: Surface::Plane {
                    frame,
                    normal,
                    world_normal: normal,
                    points,
                    normals: normals.fields.remove(&index),
                },
                bump,
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

#[cfg(test)]
pub(super) fn sky_visible(
    direction: DVec3,
    origin: DVec3,
    shadows: &ShadowIndex<Occluder>,
    terrain: &ShadowIndex<Triangle>,
    props: &ShadowIndex<super::props::PropTriangle>,
    model: &Option<ShadowIndex<Occluder>>,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    super::sky_visible_skipping_owner(
        direction, origin, shadows, terrain, props, model, None, budget,
    )
}

pub(super) fn sky_visibility(
    direction: DVec3,
    origin: DVec3,
    shadows: &ShadowIndex<Occluder>,
    terrain: &ShadowIndex<Triangle>,
    props: &ShadowIndex<super::props::PropTriangle>,
    model: &Option<ShadowIndex<Occluder>>,
    budget: &mut Budget<'_>,
) -> Result<f64, String> {
    super::sky_visibility_skipping_owner(
        direction, origin, shadows, terrain, props, model, None, budget,
    )
}

pub(super) fn model_blocked(
    model: &Option<ShadowIndex<Occluder>>,
    start: DVec3,
    end: DVec3,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    match model {
        Some(model) => model.optical_blocked(start, end, &mut |amount| budget.consume(amount)),
        None => Ok(false),
    }
}

fn mapped_colors(
    base: DVec3,
    normals: &[DVec3; super::bump::MAPS],
    maps: usize,
    budget: &mut Budget<'_>,
    gather: &mut impl FnMut(DVec3, &mut Budget<'_>) -> Result<DVec3, String>,
) -> Result<[DVec3; super::bump::MAPS], String> {
    let mut colors = [DVec3::ZERO; super::bump::MAPS];
    colors[0] = base;
    if base != DVec3::ZERO {
        for map in 1..maps {
            budget.consume(1)?;
            colors[map] = gather(normals[map], budget)?;
        }
    }
    Ok(colors)
}

pub(crate) struct Settings<'a> {
    pub control: &'a Control,
    pub fs: &'a source_formats::gamefs::GameFs,
    pub vmf: &'a source_formats::vmf::Vmf,
    pub options: crate::Options,
    pub model_entities: &'a [usize],
    pub prop_sources: &'a [usize],
    pub prop_material_sources: Option<super::asset_tracking::SourceReads>,
    pub brush_material_sources: Option<&'a crate::asset_sources::BuildMaterialSources>,
    pub world_leaf_nodes: &'a [Option<usize>],
    pub detail_model_centers: &'a [DVec3],
    pub detail_lighting_origins: &'a [Option<DVec3>],
    pub detail_lighting_tints: &'a [crate::detail_props::Tint],
    pub smoothing_sources: &'a [Option<std::sync::Arc<crate::faces::SmoothingSource>>],
    pub face_sources: &'a [std::sync::Arc<[u32]>],
}

fn shared_geometry_allocation(
    map: &Map,
    bsp: &Bsp,
    smoothing: &[Option<std::sync::Arc<crate::faces::SmoothingSource>>],
    face_sources: &[std::sync::Arc<[u32]>],
    budget: &mut Budget<'_>,
) -> Result<Allocation, String> {
    let mut bytes = 0usize;
    for brush in &map.brushes {
        bytes = bytes
            .checked_add(
                2 * (size_of::<Occluder>()
                    + brush.sides.len()
                        * (size_of::<Plane>() + size_of::<super::HullSideSource>() + 1)),
            )
            .ok_or("Lighting shared hull allocation size overflow")?;
    }
    bytes = bytes
        .checked_add(ShadowIndex::<Occluder>::allocation_bytes(
            map.brushes.len(),
        )?)
        .ok_or("Lighting shared shadow allocation size overflow")?;
    let mut sources = std::collections::HashSet::new();
    let _contributor_lookup = budget.reserve::<usize>(
        face_sources
            .len()
            .checked_mul(4)
            .ok_or("Lighting contributor lookup allocation size overflow")?,
    )?;
    budget.consume(face_sources.len())?;
    let mut contributors = std::collections::HashSet::with_capacity(face_sources.len());
    bytes = bytes
        .checked_add(std::mem::size_of_val(face_sources))
        .ok_or("Lighting face contributor allocation size overflow")?;
    for ids in face_sources {
        if contributors.insert(std::sync::Arc::as_ptr(ids) as *const u32) {
            bytes = bytes
                .checked_add(std::mem::size_of_val(ids.as_ref()) + 2 * size_of::<usize>())
                .ok_or("Lighting face contributor allocation size overflow")?;
        }
    }
    for source in smoothing.iter().flatten() {
        if sources.insert(std::sync::Arc::as_ptr(source)) {
            bytes = bytes
                .checked_add(source.winding.0.len() * 4 * size_of::<DVec3>())
                .ok_or("Lighting smoothing allocation size overflow")?;
        }
    }
    for terrain in &map.terrains {
        bytes = bytes
            .checked_add(terrain.vertices.len() * 8 * size_of::<DVec3>())
            .and_then(|bytes| bytes.checked_add(terrain.indices.len() * 2 * size_of::<Triangle>()))
            .and_then(|bytes| {
                ShadowIndex::<Triangle>::allocation_bytes(terrain.indices.len())
                    .ok()
                    .and_then(|shadow| bytes.checked_add(shadow))
            })
            .ok_or("Lighting terrain allocation size overflow")?;
    }
    for index in [
        bsp::lump::PLANES,
        bsp::lump::TEXINFO,
        bsp::lump::VERTEXES,
        bsp::lump::EDGES,
        bsp::lump::SURFEDGES,
    ] {
        bytes = bytes
            .checked_add(bsp.lumps[index].data.len() * 4)
            .ok_or("Lighting shared geometry allocation size overflow")?;
    }
    budget.reserve_bytes(bytes)
}

pub(crate) fn bake(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    bsp: &mut Bsp,
    report: &mut Report,
    settings: Settings<'_>,
    progress: &mut Option<LightingProgress>,
) -> Result<BakeStats, String> {
    let mut budget = Budget::with_options(settings.control, settings.options);
    let result = bake_inner(map, tree, visibility, bsp, report, settings, &mut budget);
    budget.progress.completed = result.is_ok() && !report.has_errors();
    *progress = Some(budget.progress);
    result
}

fn bake_inner(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    bsp: &mut Bsp,
    report: &mut Report,
    settings: Settings<'_>,
    budget: &mut Budget<'_>,
) -> Result<BakeStats, String> {
    let control = budget.control;
    let options = settings.options.validate()?;
    let dual = options.lighting == crate::LightingMode::DirectLdrHdr;
    let _shared_geometry_allocation = shared_geometry_allocation(
        map,
        bsp,
        settings.smoothing_sources,
        settings.face_sources,
        budget,
    )?;
    let _detail_centers = budget.reserve::<DVec3>(settings.detail_model_centers.len())?;
    let _detail_origins =
        budget.reserve::<Option<DVec3>>(settings.detail_lighting_origins.len())?;
    let _detail_tints =
        budget.reserve::<crate::detail_props::Tint>(settings.detail_lighting_tints.len())?;
    let brush_textures = settings
        .brush_material_sources
        .map(|sources| super::brush_alpha::prepare(map, settings.fs, sources, budget))
        .transpose()?;
    if options.brush_texture_shadows && brush_textures.is_none() {
        return Err("Brush texture shadows is missing original material sources".into());
    }
    let mut visibility_hulls = posed_occluders(
        map,
        0,
        Pose {
            offset: map.entities[0].origin.unwrap_or(DVec3::ZERO),
            rotation: None,
        },
        report,
        budget,
        brush_textures.as_ref(),
    )?;
    let mut faces = bsp
        .lighting_faces(LightingKind::Ldr)
        .map_err(|error| error.to_string())?;
    let _geometry_allocation = budget.reserve::<Face>(faces.len())?;
    let channels = if dual { 2 } else { 1 };
    let channel_output_bytes = MAX_LIGHTING_BYTES
        + faces.len() * size_of::<Face>()
        + MAX_WORLD_LIGHTS * size_of::<bsp::WorldLight>()
        + MAX_CHANNEL_AMBIENT_BYTES;
    let _output_allocation = budget.reserve_bytes(channels * channel_output_bytes)?;
    let mut infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let model_records = bsp.records::<Model>().map_err(|error| error.to_string())?;
    let _scope_allocation = budget.reserve::<super::models::Scope>(model_records.len())?;
    let mut scopes =
        super::models::scopes(map, &model_records, settings.model_entities, faces.len())?;
    let normal_start = budget.work;
    let mut normals = super::normals::build(
        bsp,
        &faces,
        settings.smoothing_sources,
        &scopes,
        options.smoothing_angle,
        budget,
    )?;
    let normal_work = budget.work - normal_start;
    let terrain_plan_bytes =
        faces
            .iter()
            .filter(|face| face.dispinfo >= 0)
            .try_fold(0usize, |bytes, face| {
                let dimensions = TerrainPlan::dimensions(
                    &map.terrains[face.dispinfo as usize],
                    &infos[face.texinfo as usize],
                )?;
                bytes
                    .checked_add(
                        dimensions[0] * dimensions[1] * size_of::<super::terrain::TerrainSample>(),
                    )
                    .ok_or_else(|| "Terrain lighting plan allocation size overflow".to_string())
            })?;
    let _terrain_plan_allocation = budget.reserve_bytes(terrain_plan_bytes)?;
    let _plan_allocation = budget.reserve::<Plan>(faces.len())?;
    let mut plans = plans(
        map,
        bsp,
        &mut faces,
        &mut infos,
        &mut normals,
        &scopes,
        budget,
    )?;
    budget.progress.lightmap_samples_planned_complete = true;
    for phase in &mut budget.progress.phases {
        if phase.phase == LightingPhase::Direct && phase.channel != LightingChannel::Preparation {
            phase.samples_planned_complete = true;
        }
    }
    let mut has_receivers = vec![false; scopes.len()];
    let mut model_faces = 0;
    for plan in &plans {
        has_receivers[plan.model] = true;
        model_faces += usize::from(plan.model != 0);
    }
    super::models::admit_initial_poses(map, &mut scopes, &has_receivers, report, budget)?;
    if report.has_errors() {
        return Ok(BakeStats::default());
    }
    if let Some(textures) = &brush_textures {
        textures.qualify_models(map, &scopes, &has_receivers, budget)?;
        report.diagnostics.push(Diagnostic::info("Brush texture shadows samples binary alpha-test world/detail boundaries, admitted initial-pose same-model self shadows and explicitly requested stationary root global casters; geometry, occupancy and model radiance sources remain unchanged", None, None));
    }
    for plan in &mut plans {
        let scope = &scopes[plan.model];
        if scope.pose.rotation.is_some() {
            plan.apply_pose(scope.pose, budget)
                .map_err(|error| format!("entity:{}: {error}", map.entities[scope.entity].id))?;
        }
    }
    let (brush_shadow_models, _caster_allocation) = global_casters(
        map,
        &scopes,
        &mut visibility_hulls,
        report,
        budget,
        brush_textures.as_ref(),
    )?;
    let mut model_shadows = Vec::with_capacity(scopes.len());
    for (model, scope) in scopes.iter().enumerate() {
        let shadows = if model != 0
            && has_receivers[model]
            && !crate::brush_shadow_casters::enabled(&map.entities[scope.entity].properties)
        {
            Some(ShadowIndex::new(
                posed_occluders(
                    map,
                    scope.entity,
                    scope.pose,
                    report,
                    budget,
                    brush_textures
                        .as_ref()
                        .filter(|textures| textures.has_model_alpha()),
                )?,
                &mut |amount| budget.consume(amount),
            )?)
        } else {
            None
        };
        model_shadows.push(shadows);
    }

    if !map.terrains.is_empty() {
        report.diagnostics.push(Diagnostic::warning("Direct LDR shares terrain border normals across verified full/half edges, using each surface’s Automatic, hard-border or angle-limit choice; other fractional spans remain unfinished", None, None));
    }
    let model_shadow_scope = if brush_shadow_models == 0 {
        "isolated self shadows"
    } else {
        "self shadows and explicitly requested stationary global casters"
    };
    report.diagnostics.push(Diagnostic::info(format!("Direct LDR bakes native directional normal-map channels and static, switchable and animated point/spot styles, sampled soft sun, sky ambient and model ambient with indexed opaque brush/terrain shadows; up to four style-preserving diffuse world-light bounces are supported; brush entities receive pivot-aware direct and bounced maps with {model_shadow_scope}; reflected model sources and prop lightmaps remain unfinished"), None, None));
    let shadows = ShadowIndex::new(visibility_hulls, &mut |amount| budget.consume(amount))?;
    let terrain_shadows = terrain_shadows(map, budget)?;
    budget.phase(LightingPhase::PropShadows);
    let mut prop_assets = if options.bake_static_prop_vertices {
        Some(super::asset_tracking::Tracker::new(
            settings.fs,
            settings.prop_material_sources.ok_or(
                "Baked prop vertex lighting is missing its original world-material source capture",
            )?,
            budget,
        )?)
    } else {
        None
    };
    let super::props::Prepared {
        shadows: prop_shadows,
        bounds: prop_bounds,
        stats: prop_stats,
        allocations: _prop_allocations,
    } = super::props::build(
        bsp,
        settings.vmf,
        settings.fs,
        prop_assets.as_mut(),
        options.generate_reflections,
        report,
        budget,
    )?;
    budget.phase(LightingPhase::PropVertices);
    let mut prop_vertices = if options.bake_static_prop_vertices {
        super::prop_vertices::prepare(
            map,
            super::prop_vertices::Settings {
                vmf: settings.vmf,
                prop_sources: settings.prop_sources,
            },
            prop_assets
                .take()
                .ok_or("Static-prop source tracking is missing")?,
            bsp,
            report,
            budget,
        )?
    } else {
        None
    };
    budget.phase(LightingPhase::Preparation);
    bsp.set_records(&infos);
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
        // Sample validation reads dimensions from the stored faces.
        bsp.set_lighting_faces(LightingKind::Ldr, &faces)
            .map_err(|error| error.to_string())?;
        bsp.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data = bytes;
        bsp.set_displacements(&data)
            .map_err(|error| error.to_string())?;
    }
    if dual {
        // Both channels start from this unlit geometry, before either owns styles or offsets.
        bsp.set_lighting_faces(LightingKind::Hdr, &faces)
            .map_err(|error| error.to_string())?;
    }
    let ctx = ChannelContext {
        world_leaf_nodes: settings.world_leaf_nodes,
        map,
        tree,
        visibility,
        plans: &plans,
        geometry: &faces,
        infos: &infos,
        shadows: &shadows,
        terrain_shadows: &terrain_shadows,
        model_shadows: &model_shadows,
        prop_shadows: &prop_shadows,
        prop_bounds: &prop_bounds,
        model_count: has_receivers.iter().skip(1).filter(|&&lit| lit).count(),
        model_faces,
        detail_model_centers: settings.detail_model_centers,
        detail_lighting_origins: settings.detail_lighting_origins,
        detail_lighting_tints: settings.detail_lighting_tints,
        face_sources: settings.face_sources,
        brush_textures: brush_textures.as_ref(),
        options,
    };
    let mut detail_ldr = None;
    let mut detail_hdr = None;
    let mut stats = bake_channel(
        &ctx,
        LightingKind::Ldr,
        bsp,
        report,
        budget,
        &mut detail_ldr,
        &mut prop_vertices,
    )?;
    stats.work = budget.work;
    stats.smoothed_faces = normals.faces;
    stats.smoothed_corners = normals.corners;
    stats.normal_work = normal_work;
    stats.prop_shadows = prop_stats;
    if dual && !report.has_errors() {
        let hdr = bake_channel(
            &ctx,
            LightingKind::Hdr,
            bsp,
            report,
            budget,
            &mut detail_hdr,
            &mut prop_vertices,
        )?;
        stats.hdr = Some(crate::HdrLightingStats {
            world_lights: hdr.lights,
            light_styles: hdr.light_styles,
            styled_faces: hdr.styled_faces,
            light_style_maps: hdr.light_style_maps,
            lighting_faces: hdr.faces,
            lit_brush_models: hdr.models,
            model_lighting_faces: hdr.model_faces,
            lightmap_samples: hdr.samples,
            lighting_bytes: hdr.bytes,
            lighting_work: hdr.work,
            ambient_samples: hdr.ambient_samples,
            ambient_bytes: hdr.ambient_bytes,
            bounces: hdr.bounces,
            bounce_rays: hdr.bounce_rays,
            bounce_samples: hdr.bounce_samples,
            bounce_work: hdr.bounce_work,
            model_bounce_faces: hdr.model_bounce_faces,
            model_bounce_samples: hdr.model_bounce_samples,
            bump_faces: hdr.bump_faces,
            bump_lightmaps: hdr.bump_lightmaps,
            map_data_present: [
                bsp::lump::LIGHTING_HDR,
                bsp::lump::WORLDLIGHTS_HDR,
                bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
            ]
            .into_iter()
            .all(|index| !bsp.lumps[index].data.is_empty()),
        });
    }
    stats
        .bytes
        .checked_add(stats.hdr.as_ref().map_or(0, |hdr| hdr.lighting_bytes))
        .filter(|&bytes| bytes <= MAX_TOTAL_LIGHTING_BYTES)
        .ok_or("Dual-channel lighting exceeds its aggregate 32 MiB sample budget")?;
    budget.progress.current_channel = LightingChannel::Preparation;
    budget.phase(LightingPhase::Finalization);
    super::details::finish(bsp, detail_ldr.as_ref(), detail_hdr.as_ref(), budget)?;
    if !report.has_errors()
        && let Some(prepared) = prop_vertices
    {
        let (plan, prop_stats) =
            super::prop_vertices::finish(prepared, bsp, dual, settings.fs, control, budget)?;
        report.diagnostics.push(Diagnostic::info(format!(
            "Baked static-prop vertex colors for {} props, {} models, {} hardware sections and {} vertices in {} channel(s)",
            prop_stats.props, prop_stats.models, prop_stats.meshes, prop_stats.vertices,
            if dual { 2 } else { 1 }), None, None));
        stats.prop_lighting_assets = Some(plan);
        stats.prop_vertex_lighting = Some(prop_stats);
    }
    stats.work = (budget.progress.preparation_work + budget.progress.ldr_work) as usize;
    if let Some(hdr) = &mut stats.hdr {
        hdr.lighting_work = budget.progress.hdr_work as usize;
    }
    stats.total_work = budget.work;
    stats.brush_shadow_models = brush_shadow_models;
    Ok(stats)
}

struct ChannelContext<'a> {
    brush_textures: Option<&'a super::brush_alpha::Prepared>,
    face_sources: &'a [std::sync::Arc<[u32]>],
    world_leaf_nodes: &'a [Option<usize>],
    map: &'a Map<'a>,
    tree: &'a Tree,
    visibility: &'a Visibility,
    plans: &'a [Plan],
    geometry: &'a [Face],
    infos: &'a [TexInfo],
    shadows: &'a ShadowIndex<Occluder>,
    terrain_shadows: &'a ShadowIndex<Triangle>,
    model_shadows: &'a [Option<ShadowIndex<Occluder>>],
    prop_shadows: &'a ShadowIndex<super::props::PropTriangle>,
    prop_bounds: &'a ShadowIndex<super::props::ProbeBounds>,
    model_count: usize,
    model_faces: usize,
    detail_model_centers: &'a [DVec3],
    detail_lighting_origins: &'a [Option<DVec3>],
    detail_lighting_tints: &'a [crate::detail_props::Tint],
    options: crate::Options,
}

fn bake_channel(
    ctx: &ChannelContext<'_>,
    kind: LightingKind,
    bsp: &mut Bsp,
    report: &mut Report,
    budget: &mut Budget<'_>,
    detail_lighting: &mut Option<super::details::Channel>,
    prop_vertices: &mut Option<super::prop_vertices::Prepared>,
) -> Result<BakeStats, String> {
    budget.channel(kind);
    let start_work = budget.work;
    let ChannelContext {
        brush_textures,
        face_sources,
        world_leaf_nodes,
        map,
        tree,
        visibility,
        plans,
        geometry,
        infos,
        shadows,
        terrain_shadows,
        model_shadows,
        prop_shadows,
        prop_bounds,
        model_count,
        model_faces,
        detail_model_centers,
        detail_lighting_origins,
        detail_lighting_tints,
        options,
    } = *ctx;
    let sky_quality = options.sky_quality;
    let lights = lights(map, tree, visibility, kind, report, budget)?;
    if report.has_errors() {
        return Ok(BakeStats::default());
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
    let _candidate_allocation = budget.reserve_bytes(super::culling::allocation_bytes(
        visibility.count,
        lights.len(),
    )?)?;
    let mut candidates = super::culling::Candidates::new(&lights, visibility, budget)?;
    let _face_allocation = budget.reserve::<Face>(geometry.len())?;
    let _serialization_allocation = budget.reserve_bytes(
        MAX_LIGHTING_BYTES + std::mem::size_of_val(geometry) + MAX_CHANNEL_AMBIENT_BYTES,
    )?;
    let mut faces = geometry.to_vec();
    let _color_allocation = budget.reserve::<RgbExp32>(MAX_CHANNEL_SAMPLES)?;
    let mut colors = Vec::with_capacity(MAX_CHANNEL_SAMPLES);
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
    let prop_context = super::prop_vertices::Context {
        map,
        tree,
        visibility,
        direct: super::receivers::Context {
            lights: &lights,
            shadows,
            terrain: terrain_shadows,
            props: prop_shadows,
            sun_directions: &sun_directions,
            ambient_directions: &ambient_directions,
        },
    };
    if options.bounces == 0
        && let Some(prepared) = prop_vertices
    {
        budget.phase(LightingPhase::PropVertices);
        super::prop_vertices::bake_channel(
            prepared,
            &prop_context,
            None,
            kind,
            &mut candidates,
            report,
            budget,
        )?;
    }
    budget.phase(LightingPhase::Direct);
    let mut used_styles = std::collections::BTreeSet::new();
    let mut styled_faces = 0;
    let mut style_maps = 0;
    let mut bump_lightmaps = 0;
    for plan in plans {
        budget.consume(1)?;
        let mut channels = super::styles::Channels::new(
            plan.sample_count(),
            colors.len(),
            plan.map_count(),
            budget,
        )?;
        let [width, height] = plan.dimensions();
        for t in 0..height {
            for s in 0..width {
                budget.phase(LightingPhase::Direct);
                let (point, normal) = plan.sample(s, t, budget)?;
                let origin = plan.origin(point, normal);
                let normals = plan.shading_normals(normal, budget)?;
                let mut color = [[DVec3::ZERO; super::bump::MAPS]; 4];
                let cluster = candidates.cluster(origin, map, tree, visibility, budget)?;
                let culling_enabled = candidates.enabled();
                let mut evaluated = 0;
                let mut range_skipped = 0;
                for &index in candidates.for_cluster(cluster, &lights, budget)? {
                    let light = &lights[index];
                    budget.phase(match light {
                        Emitter::Point(_) => LightingPhase::PointSpot,
                        Emitter::Sky(_) => LightingPhase::Sun,
                    });
                    budget.emitter(light);
                    budget.consume(1)?;
                    if culling_enabled && super::culling::outside_range(light, point) {
                        range_skipped += 1;
                        continue;
                    }
                    evaluated += 1;
                    if let Emitter::Sky(light) = light
                        && light.has_ambient()
                    {
                        budget.phase(LightingPhase::SkyAmbient);
                        budget.emitter(&lights[index]);
                        budget.consume(ambient_directions.len())?;
                        let contribution = light.ambient_contribution_with_visibility(
                            normal,
                            &ambient_directions,
                            &mut |direction| {
                                sky_visibility(
                                    direction,
                                    origin,
                                    shadows,
                                    terrain_shadows,
                                    prop_shadows,
                                    &model_shadows[plan.model],
                                    budget,
                                )
                            },
                        )?;
                        let contributions = mapped_colors(
                            contribution,
                            &normals,
                            plan.map_count(),
                            budget,
                            &mut |normal, budget| {
                                budget.consume(ambient_directions.len())?;
                                light.ambient_contribution_with_visibility(
                                    normal,
                                    &ambient_directions,
                                    &mut |direction| {
                                        sky_visibility(
                                            direction,
                                            origin,
                                            shadows,
                                            terrain_shadows,
                                            prop_shadows,
                                            &model_shadows[plan.model],
                                            budget,
                                        )
                                    },
                                )
                            },
                        )?;
                        channels.add(0, &contributions, &mut color, budget)?;
                    }
                    budget.phase(match light {
                        Emitter::Point(_) => LightingPhase::PointSpot,
                        Emitter::Sky(_) => LightingPhase::Sun,
                    });
                    budget.emitter(light);
                    match light {
                        Emitter::Point(light) => {
                            let contribution = light.contribution(point, normal)?;
                            if contribution != DVec3::ZERO
                                && !shadows.optical_blocked(
                                    origin,
                                    light.origin,
                                    &mut |amount| budget.consume(amount),
                                )?
                                && !terrain_shadows.blocked(
                                    origin,
                                    light.origin,
                                    &mut |amount| budget.consume(amount),
                                )?
                            {
                                let visibility = prop_shadows.visibility_skipping_owner(
                                    origin,
                                    light.origin,
                                    None,
                                    &mut |amount| budget.consume(amount),
                                )?;
                                if visibility == 0.0
                                    || model_blocked(
                                        &model_shadows[plan.model],
                                        origin,
                                        light.origin,
                                        budget,
                                    )?
                                {
                                    continue;
                                }
                                let contributions = mapped_colors(
                                    contribution * visibility,
                                    &normals,
                                    plan.map_count(),
                                    budget,
                                    &mut |normal, _| {
                                        Ok(light.contribution(point, normal)? * visibility)
                                    },
                                )?;
                                channels.add(light.style, &contributions, &mut color, budget)?;
                            }
                        }
                        Emitter::Sky(light) => {
                            let contribution = light.sun_contribution_with_visibility(
                                normal,
                                &sun_directions,
                                &mut |direction| {
                                    sky_visibility(
                                        direction,
                                        origin,
                                        shadows,
                                        terrain_shadows,
                                        prop_shadows,
                                        &model_shadows[plan.model],
                                        budget,
                                    )
                                },
                            )?;
                            let contributions = mapped_colors(
                                contribution,
                                &normals,
                                plan.map_count(),
                                budget,
                                &mut |normal, budget| {
                                    light.sun_contribution_with_visibility(
                                        normal,
                                        &sun_directions,
                                        &mut |direction| {
                                            sky_visibility(
                                                direction,
                                                origin,
                                                shadows,
                                                terrain_shadows,
                                                prop_shadows,
                                                &model_shadows[plan.model],
                                                budget,
                                            )
                                        },
                                    )
                                },
                            )?;
                            channels.add(0, &contributions, &mut color, budget)?;
                        }
                    }
                }
                budget.phase(LightingPhase::Direct);
                candidates.stats.evaluated += evaluated;
                candidates.stats.range_skipped += range_skipped;
                channels.finish_sample(t * width + s, &color, budget)?;
                budget.finish_sample();
            }
        }
        budget.phase(LightingPhase::Direct);
        let (styles, offset, channels) = channels
            .emit(&mut colors)
            .map_err(|error| format!("face:{}: {error}", plan.face))?;
        used_styles.extend(styles.into_iter().filter(|&style| style != 255));
        styled_faces += usize::from(channels > 1);
        style_maps += channels * plan.map_count();
        bump_lightmaps += channels * (plan.map_count() - 1);
        faces[plan.face].styles = styles;
        faces[plan.face].light_offset = offset;
    }
    budget.consume(0)?;
    bsp.set_lighting(kind, &colors)
        .map_err(|error| error.to_string())?;
    bsp.set_world_lights(kind, &world_lights)
        .map_err(|error| error.to_string())?;
    bsp.set_lighting_faces(kind, &faces)
        .map_err(|error| error.to_string())?;
    budget.phase(LightingPhase::Bounce);
    budget.plan_samples(
        plans.iter().map(Plan::sample_count).sum::<usize>() * usize::from(options.bounces),
    )?;
    budget.samples_planned_complete();
    let bounce_start = budget.work;
    let mut prop_source = None;
    let bounce = if options.bounces > 0 {
        let direct = if options.bounces > 1 {
            Some(super::bounce::Direct::new(&faces, &colors, budget)?)
        } else {
            None
        };
        let mut final_stats = super::bounce::Stats {
            rays: 0,
            samples: 0,
            model_faces: 0,
            model_samples: 0,
        };
        let mut rays = 0;
        for index in 0..options.bounces {
            budget.consume(0)?;
            let (stats, surfaces) = super::bounce::bake(
                &mut super::bounce::Context {
                    brush_textures,
                    face_sources,
                    map,
                    kind,
                    plans,
                    faces: &mut faces,
                    infos,
                    shadows,
                    model_shadows,
                    prop_shadows,
                    quality: sky_quality,
                    measured_sources: options.bake_static_prop_vertices,
                },
                bsp,
                direct.as_ref(),
                &mut colors,
                budget,
            )?;
            final_stats = stats;
            if prop_vertices.is_some() && index + 1 == options.bounces {
                prop_source = Some(surfaces);
            } else {
                drop(surfaces);
            }
            rays += final_stats.rays;
            bsp.set_lighting(kind, &colors)
                .map_err(|error| error.to_string())?;
            bsp.set_lighting_faces(kind, &faces)
                .map_err(|error| error.to_string())?;
        }
        final_stats.rays = rays;
        used_styles.clear();
        styled_faces = 0;
        style_maps = 0;
        bump_lightmaps = 0;
        for plan in plans {
            let styles = faces[plan.face]
                .styles
                .into_iter()
                .filter(|&style| style != 255)
                .collect::<Vec<_>>();
            used_styles.extend(styles.iter().copied());
            styled_faces += usize::from(styles.len() > 1);
            style_maps += styles.len() * plan.map_count();
            bump_lightmaps += styles.len() * (plan.map_count() - 1);
        }
        final_stats
    } else {
        super::bounce::Stats {
            rays: 0,
            samples: 0,
            model_faces: 0,
            model_samples: 0,
        }
    };
    let bounce_work = budget.work - bounce_start;
    if options.bounces > 0
        && let Some(prepared) = prop_vertices
    {
        let surfaces = prop_source
            .as_ref()
            .ok_or("Bounced prop source lighting is missing")?;
        budget.phase(LightingPhase::PropVertices);
        super::prop_vertices::bake_channel(
            prepared,
            &prop_context,
            Some(super::prop_vertices::Indirect {
                surfaces,
                quality: sky_quality,
            }),
            kind,
            &mut candidates,
            report,
            budget,
        )?;
    }
    drop(prop_source);
    let sky_radiance = lights
        .iter()
        .find_map(|light| match light {
            Emitter::Sky(light) => Some(light.ambient_radiance()),
            Emitter::Point(_) => None,
        })
        .unwrap_or(DVec3::ZERO);
    budget.phase(LightingPhase::AmbientProbes);
    let ambient = super::probes::bake(
        &super::probes::Context {
            brush_textures,
            face_sources,
            world_leaf_nodes,
            map,
            kind,
            tree,
            shadows,
            terrain: terrain_shadows,
            prop_shadows,
            prop_bounds,
            directions: &ambient_directions,
            quality: sky_quality,
            sky_radiance,
            refine: options.refine_ambient_probes,
        },
        bsp,
        report,
        budget,
    )?;
    budget.phase(LightingPhase::DetailProps);
    *detail_lighting = super::details::bake(
        &super::details::Context {
            brush_textures,
            face_sources,
            map,
            tree,
            visibility,
            kind,
            model_centers: detail_model_centers,
            lighting_origins: detail_lighting_origins,
            lighting_tints: detail_lighting_tints,
            lights: &lights,
            shadows,
            terrain: terrain_shadows,
            props: prop_shadows,
            directions: &ambient_directions,
            sun_directions: &sun_directions,
            sky_radiance,
        },
        bsp,
        &mut candidates,
        report,
        budget,
    )?;
    Ok(BakeStats {
        lights: world_lights.len(),
        light_styles: used_styles.len(),
        styled_faces,
        light_style_maps: style_maps,
        faces: plans.len(),
        models: model_count,
        model_faces,
        samples: plans.iter().map(Plan::sample_count).sum(),
        bytes: colors.len() * size_of::<RgbExp32>(),
        work: budget.work - start_work,
        ambient_samples: ambient.samples,
        ambient_bytes: ambient.bytes,
        bounces: options.bounces,
        bounce_rays: bounce.rays,
        bounce_samples: bounce.samples,
        bounce_work,
        model_bounce_faces: bounce.model_faces,
        model_bounce_samples: bounce.model_samples,
        bump_faces: plans.iter().filter(|plan| plan.map_count() > 1).count(),
        bump_lightmaps,
        culling: candidates.stats,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane_plan() -> Plan {
        let info = TexInfo {
            lightmap_vecs: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
            ..Default::default()
        };
        Plan {
            face: 0,
            model: 1,
            pose: Pose {
                offset: DVec3::new(10.5, -20.25, 30.125),
                rotation: None,
            },
            surface: Surface::Plane {
                frame: LuxelFrame::new(
                    Plane {
                        normal: DVec3::Z,
                        dist: 4.0,
                    },
                    &info,
                    [0, 0],
                    [1, 1],
                )
                .unwrap(),
                normal: DVec3::Z,
                world_normal: DVec3::Z,
                points: vec![
                    DVec3::new(-2.0, -2.0, 4.0),
                    DVec3::new(-2.0, 2.0, 4.0),
                    DVec3::new(2.0, 2.0, 4.0),
                    DVec3::new(2.0, -2.0, 4.0),
                ],
                normals: None,
            },
            bump: None,
        }
    }

    fn pitch_quarter_turn(offset: DVec3) -> Pose {
        Pose {
            offset,
            rotation: Some(glam::DMat3::from_cols(DVec3::NEG_Z, DVec3::Y, DVec3::X)),
        }
    }

    #[test]
    fn translation_only_receiver_keeps_sample_bias_bits_and_work() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut plan = plane_plan();
        plan.apply_pose(plan.pose, &mut budget).unwrap();
        assert_eq!(budget.work(), 0);
        let (point, normal) = plan.sample(1, 1, &mut budget).unwrap();
        let expected = DVec3::new(1.0, 1.0, 4.0) + plan.pose.offset;
        assert_eq!(
            point.to_array().map(f64::to_bits),
            expected.to_array().map(f64::to_bits)
        );
        assert_eq!(
            normal.to_array().map(f64::to_bits),
            DVec3::Z.to_array().map(f64::to_bits)
        );
        let bias = plan.origin(point, DVec3::new(0.6, 0.0, 0.8));
        let expected_bias = expected + DVec3::Z * SAMPLE_BIAS;
        assert_eq!(
            bias.to_array().map(f64::to_bits),
            expected_bias.to_array().map(f64::to_bits)
        );
        assert_eq!(budget.work(), 5);
    }

    #[test]
    fn rotated_receiver_maps_local_samples_and_uses_flat_world_bias() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut plan = plane_plan();
        plan.apply_pose(pitch_quarter_turn(plan.pose.offset), &mut budget)
            .unwrap();
        assert_eq!(budget.work(), 5);
        let (point, normal) = plan.sample(1, 1, &mut budget).unwrap();
        assert_eq!(point, DVec3::new(14.5, -19.25, 29.125));
        assert_eq!(normal, DVec3::X);
        assert_eq!(
            plan.origin(point, DVec3::Y),
            DVec3::new(15.0, -19.25, 29.125)
        );
        assert_eq!(
            plan.shading_normals(normal, &mut budget).unwrap(),
            [DVec3::X; super::super::bump::MAPS]
        );
        assert_eq!(budget.work(), 12);
    }

    #[test]
    fn rotated_receiver_checks_emitted_points_and_additional_sampling_work() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut plan = plane_plan();
        let outside = pitch_quarter_turn(DVec3::new(MAX_COMPILE_COORD - 3.0, 0.0, 0.0));
        assert!(
            plan.apply_pose(outside, &mut budget)
                .unwrap_err()
                .contains("Source bounds")
        );
        assert!(plan.pose.rotation.is_none());
        plan.apply_pose(pitch_quarter_turn(plan.pose.offset), &mut budget)
            .unwrap();
        budget
            .consume(crate::DEFAULT_LIGHTING_WORK_LIMIT as usize - budget.work() - 5)
            .unwrap();
        assert!(
            plan.sample(1, 1, &mut budget)
                .unwrap_err()
                .contains("work-unit limit")
        );
        let mut budget = Budget::new(&control);
        control.cancel().unwrap();
        assert!(
            plane_plan()
                .apply_pose(pitch_quarter_turn(DVec3::ZERO), &mut budget)
                .unwrap_err()
                .contains("cancelled")
        );
        assert_eq!(budget.work(), 0);
    }

    #[test]
    fn global_no_receiver_hulls_prepay_memory_work_and_keep_source_identity() {
        let game = crate::test_common::empty_game();
        let mut map =
            Map::load(&crate::test_common::room(), &game, &mut Report::default()).unwrap();
        let entity = map.entities.len();
        map.entities.push(crate::map::MapEntity {
            id: 901,
            classname: "func_brush".into(),
            properties: vec![("vrad_brush_cast_shadows".into(), "1".into())],
            brushes: vec![0],
            origin: Some(DVec3::ZERO),
        });
        map.brushes[0].entity = entity;
        map.entities[0].brushes.retain(|&brush| brush != 0);
        let scopes = [super::super::models::Scope {
            entity,
            pose: Pose {
                offset: DVec3::ZERO,
                rotation: None,
            },
            faces: 0..0,
        }];
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let required = 4 * size_of::<Occluder>() + size_of::<super::super::OpticalHull>();
        let hold = budget
            .reserve_bytes(MAX_LIGHTING_LIVE_BYTES - required + 1)
            .unwrap();
        let mut hulls = Vec::new();
        let mut report = Report::default();
        assert!(global_casters(&map, &scopes, &mut hulls, &mut report, &mut budget, None).is_err());
        assert!(hulls.is_empty());
        assert_eq!(budget.work(), 0);
        drop(hold);
        let (count, allocation) =
            global_casters(&map, &scopes, &mut hulls, &mut report, &mut budget, None).unwrap();
        assert_eq!(count, 1);
        assert_eq!(hulls.len(), 1);
        assert_eq!(hulls[0].source.as_ref().unwrap().brush, 0);
        assert!(!hulls[0].occupies_world());
        assert_eq!(budget.live_bytes(), required);
        drop(hulls);
        drop(allocation);
        assert_eq!(budget.live_bytes(), 0);
        let mut exhausted = Budget::with_limit(&control, 1);
        let mut rejected = Vec::new();
        assert!(
            global_casters(
                &map,
                &scopes,
                &mut rejected,
                &mut report,
                &mut exhausted,
                None
            )
            .err()
            .unwrap()
            .contains("work-unit limit")
        );
        assert!(rejected.is_empty());
        control.cancel().unwrap();
        assert!(
            global_casters(&map, &scopes, &mut rejected, &mut report, &mut budget, None)
                .err()
                .unwrap()
                .contains("cancelled")
        );
        assert!(rejected.is_empty());
    }

    #[test]
    fn posed_own_hulls_rotate_planes_and_use_all_bounds_corners() {
        let game = crate::test_common::empty_game();
        let map = Map::load(&crate::test_common::room(), &game, &mut Report::default()).unwrap();
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut report = Report::default();
        let original = occluders(&map, 0, &mut report, &mut budget).unwrap();
        let c = std::f64::consts::FRAC_1_SQRT_2;
        let offset = DVec3::new(10.5, -20.25, 30.125);
        let pose = Pose {
            offset,
            rotation: Some(glam::DMat3::from_cols(
                DVec3::new(c, c, 0.0),
                DVec3::new(-c, c, 0.0),
                DVec3::Z,
            )),
        };
        let rotated = posed_occluders(&map, 0, pose, &mut report, &mut budget, None).unwrap();
        assert!(!original.is_empty());
        assert_eq!(original.len(), rotated.len());
        for (local, world) in original.iter().zip(&rotated) {
            let expected_min = DVec3::new(
                c * (local.mins.x - local.maxs.y),
                c * (local.mins.x + local.mins.y),
                local.mins.z,
            ) + offset;
            let expected_max = DVec3::new(
                c * (local.maxs.x - local.mins.y),
                c * (local.maxs.x + local.maxs.y),
                local.maxs.z,
            ) + offset;
            assert!(world.mins.distance(expected_min) < 1e-10);
            assert!(world.maxs.distance(expected_max) < 1e-10);
            assert_eq!(local.planes.len(), world.planes.len());
            for (local, world) in local.planes.iter().zip(&world.planes) {
                let normal = DVec3::new(
                    c * (local.normal.x - local.normal.y),
                    c * (local.normal.x + local.normal.y),
                    local.normal.z,
                );
                assert!(world.normal.distance(normal) < 1e-12);
                assert!((world.dist - (local.dist + normal.dot(offset))).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn rejected_work_and_cancellation_stop_before_sampling() {
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 100_000_000);
        budget.consume(100_000_000).unwrap();
        assert!(budget.consume(1).is_err());
        let mut budget = Budget::new(&control);
        control.cancel().unwrap();
        assert!(budget.consume(1).is_err());
        assert_eq!(budget.work, 0);
    }

    #[test]
    fn channel_phases_share_work_and_cancellation() {
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 100_000_000);
        budget.channel(LightingKind::Ldr);
        budget.consume(50_000_001).unwrap();
        budget.channel(LightingKind::Hdr);
        assert!(budget.consume(50_000_000).is_err());
        assert_eq!(budget.progress.ldr_work, 50_000_001);
        assert_eq!(budget.progress.hdr_work, 50_000_000);
        assert_eq!(budget.progress.work, 100_000_001);
        let mut budget = Budget::new(&control);
        budget.consume(17).unwrap();
        control.cancel().unwrap();
        assert!(budget.consume(1).is_err());
        assert_eq!(budget.work(), 17);
    }

    #[test]
    fn contributor_detail_is_bounded_without_losing_charged_work() {
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 1_000);
        budget.consume(3).unwrap();
        budget.channel(LightingKind::Ldr);
        budget.phase(LightingPhase::PointSpot);
        for id in 0..MAX_LIGHTING_CONTRIBUTORS as u32 + 5 {
            budget.source(id, "light");
            budget.consume(2).unwrap();
        }
        budget.source(0, "light");
        budget.consume(7).unwrap();
        budget.clear_emitter();
        budget.consume(11).unwrap();
        let progress = &budget.progress;
        assert_eq!(progress.contributors.len(), MAX_LIGHTING_CONTRIBUTORS);
        assert!(progress.contributors_truncated);
        assert_eq!(progress.other_contributors_work, 10);
        assert_eq!(progress.unattributed_work, 14);
        assert_eq!(progress.contributors[0].work, 9);
        assert_eq!(
            progress.work,
            progress
                .contributors
                .iter()
                .map(|entry| entry.work)
                .sum::<u64>()
                + progress.other_contributors_work
                + progress.unattributed_work
        );
        assert_eq!(
            progress.work,
            progress.preparation_work + progress.ldr_work + progress.hdr_work
        );
        assert_eq!(
            progress.work,
            progress.phases.iter().map(|phase| phase.work).sum::<u64>()
        );
    }

    #[test]
    fn explicit_work_failure_retains_the_rejected_request_and_active_quality() {
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 19);
        budget.channel(LightingKind::Hdr);
        budget.phase(LightingPhase::SkyAmbient);
        budget.source(42, "light_environment");
        budget.consume(17).unwrap();
        let error = budget.consume(5).unwrap_err();
        assert!(error.contains("19 work-unit limit") && error.contains("Fast sampling"));
        assert!(!error.contains("choose Fast"));
        assert_eq!(budget.progress.work, 22);
        assert_eq!(budget.progress.hdr_work, 22);
        assert_eq!(budget.progress.contributors[0].work, 22);
        assert!(!budget.progress.completed);
        control.cancel().unwrap();
        assert!(budget.consume(5).unwrap_err().contains("cancelled"));
        assert_eq!(budget.progress.work, 22);
    }

    #[test]
    fn live_channel_allocations_share_one_checked_releasing_limit() {
        let control = Control::default();
        let budget = Budget::new(&control);
        assert!(budget.reserve::<RgbExp32>(usize::MAX).is_err());
        let first = budget.reserve_bytes(MAX_LIGHTING_LIVE_BYTES / 2).unwrap();
        let second = budget.reserve_bytes(MAX_LIGHTING_LIVE_BYTES / 2).unwrap();
        assert!(budget.reserve_bytes(1).is_err());
        drop(second);
        let snapshot = budget.reserve_bytes(MAX_LIGHTING_LIVE_BYTES / 2).unwrap();
        drop(first);
        drop(snapshot);
        assert_eq!(budget.live_bytes.get(), 0);
        control.cancel().unwrap();
        assert!(budget.reserve_bytes(1).is_err());
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
