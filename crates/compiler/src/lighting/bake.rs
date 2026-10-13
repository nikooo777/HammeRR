use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::{
    self, Bsp, Edge, Face, LightingKind, MAX_LIGHTING_BYTES, MAX_WORLD_LIGHTS, Model, RgbExp32,
    TexInfo, contents, surf,
};

use super::shadows::ShadowIndex;
use super::{LuxelFrame, Occluder, PointLight, encode_color};
use crate::map::Map;
use crate::tree::Tree;
use crate::visibility::Visibility;
use crate::{Control, Diagnostic, Report};

const MAX_LIGHTING_WORK: usize = 100_000_000;
const SAMPLE_BIAS: f64 = 0.5;

#[derive(Debug, Default)]
pub(crate) struct BakeStats {
    pub lights: usize,
    pub faces: usize,
    pub samples: usize,
    pub bytes: usize,
    pub work: usize,
}

struct Plan {
    face: usize,
    frame: LuxelFrame,
    normal: DVec3,
    points: Vec<DVec3>,
}

struct Budget<'a> {
    work: usize,
    control: &'a Control,
}

impl Budget<'_> {
    fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Direct lighting work count overflow")?;
        if self.work > MAX_LIGHTING_WORK {
            return Err("Direct LDR exceeded its 100-million work-unit limit; use Fullbright while the accelerated lighting backend is unfinished".into());
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

fn lights(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    report: &mut Report,
    control: &Control,
) -> Result<Vec<PointLight>, String> {
    let mut lights = Vec::new();
    let mut targets = map_core::light::Targets::new();
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
                Ok(light) if cluster.is_some() => lights.push(light),
                Ok(_) => report.diagnostics.push(Diagnostic::warning("Light source is inside solid or unreachable space and does not illuminate the world", Some(format!("entity:{}", entity.id)), entity.origin)),
                Err(error) => report.diagnostics.push(Diagnostic::error(error, Some(format!("entity:{}", entity.id)), entity.origin)),
            }
        } else if entity.classname.to_ascii_lowercase().starts_with("light_") {
            report.diagnostics.push(Diagnostic::warning(
                format!(
                    "Direct LDR does not bake {} yet; only static point/spot lights are supported",
                    entity.classname
                ),
                Some(format!("entity:{}", entity.id)),
                entity.origin,
            ));
        }
    }
    Ok(lights)
}

fn occluders(map: &Map, report: &mut Report, control: &Control) -> Result<Vec<Occluder>, String> {
    let mut result = Vec::new();
    for brush in &map.brushes {
        control.checkpoint()?;
        if brush.entity != 0
            || brush.contents & (contents::SOLID | contents::WINDOW | contents::GRATE) == 0
        {
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
                    dist: plane.dist,
                }
            })
            .collect();
        result.push(Occluder::new(planes, brush.mins, brush.maxs)?);
    }
    Ok(result)
}

fn plans(
    bsp: &Bsp,
    faces: &[Face],
    infos: &[TexInfo],
    control: &Control,
) -> Result<Vec<Plan>, String> {
    let models = bsp.records::<Model>().map_err(|error| error.to_string())?;
    let world = models
        .first()
        .ok_or("Direct lighting needs a world model")?;
    let planes = bsp
        .records::<bsp::Plane>()
        .map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    let mut plans = Vec::new();
    let mut sample_records: usize = 0;
    for (index, face) in faces
        .iter()
        .enumerate()
        .skip(world.first_face as usize)
        .take(world.face_count as usize)
    {
        control.checkpoint()?;
        let info = &infos[face.texinfo as usize];
        if info.flags & (surf::NOLIGHT | surf::SKY | surf::SKY2D | surf::WARP | surf::NODRAW) != 0 {
            continue;
        }
        let plane = planes[face.plane as usize];
        let plane = Plane {
            normal: DVec3::from_array(plane.normal.map(f64::from)),
            dist: f64::from(plane.dist),
        };
        let frame = LuxelFrame::new(plane, info, face.lightmap_mins, face.lightmap_size)?;
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
            frame,
            normal: plane.normal.normalize(),
            points,
        });
    }
    Ok(plans)
}

pub(crate) fn bake(
    map: &Map,
    tree: &Tree,
    visibility: &Visibility,
    bsp: &mut Bsp,
    report: &mut Report,
    control: &Control,
) -> Result<BakeStats, String> {
    let lights = lights(map, tree, visibility, report, control)?;
    if report.has_errors() {
        return Ok(BakeStats::default());
    }
    let occluders = occluders(map, report, control)?;
    let mut faces = bsp.records::<Face>().map_err(|error| error.to_string())?;
    let mut infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let plans = plans(bsp, &faces, &infos, control)?;
    if bsp
        .records::<Model>()
        .map_err(|error| error.to_string())?
        .len()
        > 1
    {
        report.diagnostics.push(Diagnostic::warning("Direct LDR does not bake brush-model lighting yet; only world and detail faces receive samples", None, None));
    }
    if infos.iter().any(|info| info.flags & surf::BUMPLIGHT != 0) {
        report.diagnostics.push(Diagnostic::warning("Direct LDR uses flat face normals and disables bump-light sample maps; smoothing and bump-light baking are unfinished", None, None));
        for info in &mut infos {
            info.flags &= !surf::BUMPLIGHT;
        }
    }
    if faces.iter().any(|face| face.smoothing_groups != 0) {
        report.diagnostics.push(Diagnostic::warning("Direct LDR does not smooth lighting normals across smoothing groups yet; authored masks remain in the BSP", None, None));
    }
    report.diagnostics.push(Diagnostic::info("Direct LDR bakes static point/spot lights and indexed opaque brush shadows; bounced light, sky/ambient cubes, HDR, props, terrain and moving-model lighting remain unfinished", None, None));
    let count: usize = plans.iter().map(|plan| plan.frame.sample_count() + 1).sum();
    let mut colors = Vec::with_capacity(count);
    let mut budget = Budget { work: 0, control };
    let shadows = ShadowIndex::new(occluders, &mut |amount| budget.consume(amount))?;
    for plan in &plans {
        budget.consume(1)?;
        let average = colors.len();
        colors.push(RgbExp32::default());
        let mut total = DVec3::ZERO;
        let [width, height] = plan.frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                budget.consume(plan.points.len() + 1)?;
                let raw = plan.frame.position(s, t)?;
                let point = inside_or_closest(raw, &plan.points, plan.normal);
                let origin = point + plan.normal * SAMPLE_BIAS;
                let mut color = DVec3::ZERO;
                for light in &lights {
                    budget.consume(1)?;
                    let contribution = light.contribution(point, plan.normal)?;
                    if contribution == DVec3::ZERO {
                        continue;
                    }
                    let blocked = shadows
                        .blocked(origin, light.origin, &mut |amount| budget.consume(amount))?;
                    if !blocked {
                        color += contribution;
                    }
                }
                total += color;
                colors.push(encode_color(color)?);
            }
        }
        colors[average] = encode_color(total / plan.frame.sample_count() as f64)?;
        faces[plan.face].styles = [0, 255, 255, 255];
        faces[plan.face].light_offset = ((average + 1) * size_of::<RgbExp32>()) as i32;
    }
    control.checkpoint()?;
    bsp.set_lighting(LightingKind::Ldr, &colors)
        .map_err(|error| error.to_string())?;
    bsp.set_world_lights(
        LightingKind::Ldr,
        &lights
            .iter()
            .map(PointLight::world_light)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    bsp.set_records(&faces);
    bsp.set_records(&infos);
    Ok(BakeStats {
        lights: lights.len(),
        faces: plans.len(),
        samples: count - plans.len(),
        bytes: colors.len() * size_of::<RgbExp32>(),
        work: budget.work,
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
