use std::collections::HashMap;
use std::sync::Arc;

use glam::DVec3;
use map_core::ObjectId;
use map_core::query::{Hit, Ray};
use source_formats::vmf::{Entity, Vmf};

pub mod materials;
mod settings;
mod simulation;
mod types;

pub use simulation::Simulation;
pub use types::*;

pub const MAX_LINKS: usize = 512;
pub const MAX_CURVE_SEGMENTS: usize = 72;
pub const TUBE_SIDES: usize = 6;
pub const MAX_EXTENT: f64 = 65_536.0;
pub const MIN_SLACK: f64 = -2048.0;
pub const MAX_SLACK: f64 = 2047.0;
pub const MAX_WIDTH: f64 = 64.0;
pub const MAX_IDENTIFIER_BYTES: usize = 256;
pub const MAX_DIAGNOSTIC_EXCERPT_BYTES: usize = 64;

enum Target<'a> {
    Missing,
    Unique(&'a Entity),
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub entity: u32,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub source: u32,
    pub target: Option<u32>,
    pub points: Vec<DVec3>,
    pub width: f64,
    pub slack: f64,
    pub physical_nodes: usize,
    pub subdiv_authored: u8,
    pub subdiv: usize,
    pub material: String,
    pub texture_scale: f64,
    pub texture_length: f64,
    pub barbed: bool,
    pub breakable: bool,
    pub inert_keys: Vec<String>,
    pub scroll: f64,
}

#[derive(Debug, Default, Clone)]
pub struct Preview {
    pub links: Vec<Link>,
    pub issues: Vec<Issue>,
    pub omitted: usize,
    pub complete: bool,
    pub time: f64,
}

pub(crate) fn property<'a>(entity: &'a Entity, key: &'a str) -> Result<Option<&'a str>, String> {
    let mut values = entity.properties.get_all(key);
    let value = values.next().map(str::trim);
    if values.next().is_some() {
        return Err(format!(
            "{key} has ambiguous case-insensitive duplicate keys"
        ));
    }
    if value.is_some_and(|value| value.len() > MAX_IDENTIFIER_BYTES) {
        return Err(format!("{key} exceeds {MAX_IDENTIFIER_BYTES} bytes"));
    }
    Ok(value)
}

fn excerpt(value: &str) -> String {
    let mut end = value.len().min(MAX_DIAGNOSTIC_EXCERPT_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut excerpt = value[..end]
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if end < value.len() {
        excerpt.push('…');
    }
    excerpt
}

fn identifier<'a>(entity: &'a Entity, key: &'a str) -> Result<Option<&'a str>, String> {
    property(entity, key)
}

fn number(entity: &Entity, key: &str, default: f64) -> Result<f64, String> {
    let value = property(entity, key)?.map_or(Ok(default), |value| {
        value
            .parse::<f64>()
            .map_err(|_| format!("{key} must be a finite number"))
    })?;
    if !value.is_finite() {
        return Err(format!("{key} must be a finite number"));
    }
    Ok(value)
}

pub fn is_rope(entity: &Entity) -> bool {
    entity.classname().is_some_and(|class| {
        class.eq_ignore_ascii_case("move_rope") || class.eq_ignore_ascii_case("keyframe_rope")
    })
}

pub fn settings(entity: &Entity) -> Result<SpanSettings, String> {
    settings::parse(entity)
}

impl Link {
    pub fn bounds(&self) -> (DVec3, DVec3) {
        let (mins, maxs) = self.points.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(mins, maxs), point| (mins.min(*point), maxs.max(*point)),
        );
        let radius = DVec3::splat(self.width * 0.5);
        (mins - radius, maxs + radius)
    }

    pub fn length(&self) -> f64 {
        self.points
            .windows(2)
            .map(|pair| pair[0].distance(pair[1]))
            .sum()
    }

    fn vertex(&self, index: usize, side: usize) -> RopeVertex {
        let before = self.points[index.saturating_sub(1)];
        let after = self.points[(index + 1).min(self.points.len() - 1)];
        let tangent = (after - before)
            .try_normalize()
            .or_else(|| (after - self.points[index]).try_normalize())
            .or_else(|| (self.points[index] - before).try_normalize())
            .expect("Rope mesh vertices belong to a nonzero segment");
        let reference = if tangent.z.abs() > 0.95 {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let right = tangent.cross(reference).normalize();
        let up = tangent.cross(right);
        let u = side as f64 / TUBE_SIDES as f64;
        let angle = u * std::f64::consts::TAU;
        let normal = right * angle.cos() + up * angle.sin();
        let subdiv = if self.barbed { 3 } else { self.subdiv };
        let denominator = (self.physical_nodes - 1) * subdiv + 1;
        RopeVertex {
            position: self.points[index] + normal * (self.width * 0.5),
            normal,
            uv: [
                u,
                index as f64 * 4.0 * self.texture_length / self.texture_scale / denominator as f64,
            ],
        }
    }

    pub fn mesh_triangles(&self) -> impl Iterator<Item = [RopeVertex; 3]> + '_ {
        (0..self.points.len().saturating_sub(1))
            .filter(|&index| self.points[index] != self.points[index + 1])
            .flat_map(move |index| {
                (0..TUBE_SIDES).flat_map(move |side| {
                    let a = self.vertex(index, side);
                    let b = self.vertex(index, side + 1);
                    let c = self.vertex(index + 1, side + 1);
                    let d = self.vertex(index + 1, side);
                    [[a, c, b], [a, d, c]]
                })
            })
    }

    pub fn triangles(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ {
        self.mesh_triangles()
            .map(|triangle| triangle.map(|vertex| vertex.position))
    }

    pub fn pick(&self, ray: &Ray) -> Option<Hit> {
        if !ray.origin.is_finite() {
            return None;
        }
        let ray = Ray {
            origin: ray.origin,
            direction: ray.direction.try_normalize()?,
        };
        let (mins, maxs) = self.bounds();
        if !map_core::query::ray_intersects_bounds(&ray, mins, maxs) {
            return None;
        }
        let distance = self
            .triangles()
            .filter_map(|triangle| map_core::query::ray_triangle(&ray, triangle))
            .min_by(f64::total_cmp)?;
        Some(Hit {
            object: ObjectId::Entity(self.source),
            solid: None,
            side: None,
            distance,
            point: ray.origin + ray.direction * distance,
        })
    }
}

pub fn prepare_with_endpoints(
    vmf: &Vmf,
    include: impl Fn(ObjectId) -> bool,
    mut resolve: impl FnMut(&Entity, Option<&Entity>) -> Result<ResolvedEndpoints, String>,
) -> Prepared {
    let mut prepared = Prepared::default();
    let mut sources = Vec::new();
    for source in vmf
        .entities
        .iter()
        .filter(|entity| !entity.hidden && is_rope(entity) && include(ObjectId::Entity(entity.id)))
    {
        if sources.len() == MAX_LINKS {
            prepared.omitted += 1;
        } else {
            sources.push(source);
        }
    }
    let mut names = HashMap::new();
    for source in &sources {
        if let Ok(Some(next)) = identifier(source, "NextKey")
            && !next.is_empty()
        {
            names.insert(next.to_ascii_lowercase(), Target::Missing);
        }
    }
    for entity in &vmf.entities {
        for raw in entity.properties.get_all("targetname") {
            if raw.trim().len() <= MAX_IDENTIFIER_BYTES
                && let Some(target) = names.get_mut(&raw.trim().to_ascii_lowercase())
            {
                *target = match target {
                    Target::Missing => Target::Unique(entity),
                    Target::Unique(_) | Target::Ambiguous => Target::Ambiguous,
                };
            }
        }
    }
    for source in sources {
        let result = (|| {
            property(source, "classname")?;
            identifier(source, "targetname")?;
            let next = identifier(source, "NextKey")?.unwrap_or("");
            let settings = settings::parse(source)?;
            if next.is_empty() && !settings.dangling {
                if source
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("move_rope"))
                {
                    return Err("move_rope needs NextKey or Dangling 1".into());
                }
                return Ok(None);
            }
            if next.contains(['*', '?', '!']) {
                return Err("NextKey needs an exact authored targetname".into());
            }
            let target = if next.is_empty() {
                None
            } else {
                Some(match names.get(&next.to_ascii_lowercase()) {
                    Some(Target::Unique(target)) => *target,
                    Some(Target::Ambiguous) => {
                        return Err(format!(
                            "NextKey '{}' matches multiple targetname entries",
                            excerpt(next)
                        ));
                    }
                    Some(Target::Missing) | None => {
                        return Err(format!(
                            "NextKey '{}' has no authored target",
                            excerpt(next)
                        ));
                    }
                })
            };
            if let Some(target) = target
                && (target.hidden || !include(ObjectId::Entity(target.id)))
            {
                return Err("NextKey endpoint is hidden or outside the working region".into());
            }
            let endpoints = resolve(source, target)?;
            Ok(Some(Span {
                source: source.id,
                target: target.map(|target| target.id),
                endpoints,
                settings,
            }))
        })();
        match result {
            Ok(Some(span)) => prepared.spans.push(span),
            Ok(None) => (),
            Err(message) => prepared.issues.push(Issue {
                entity: source.id,
                message,
            }),
        }
    }
    prepared
}

pub fn prepare(vmf: &Vmf, include: impl Fn(ObjectId) -> bool) -> Preview {
    let mut prepared = prepare_with_endpoints(vmf, include, |source, target| {
        for entity in [Some(source), target].into_iter().flatten() {
            for key in ["StartAttachment", "EndAttachment", "parentattachment"] {
                if property(entity, key)?.is_some_and(|value| !value.is_empty() && value != "0") {
                    return Err(format!(
                        "{key} requires the prepared Session attachment resolver"
                    ));
                }
            }
            if property(entity, "parentname")?.is_some_and(|name| !name.is_empty())
                || !is_rope(entity)
            {
                return Err(
                    "This rope anchor requires the prepared Session attachment resolver".into(),
                );
            }
        }
        let position = |entity: &Entity| {
            map_core::measurements::entity_origin(vmf, ObjectId::Entity(entity.id))
        };
        let start = position(source)?;
        let end = target.map(position).transpose()?;
        let rope_length = end.map_or(0, |end| (end.as_vec3() - start.as_vec3()).length() as i32);
        Ok(ResolvedEndpoints {
            start: Endpoint {
                position: start,
                forward: DVec3::X,
            },
            end: Some(Endpoint {
                position: end.unwrap_or(DVec3::ZERO),
                forward: DVec3::X,
            }),
            rope_length,
        })
    });
    let result = (|| {
        let mut simulation = Simulation::new(Arc::new(prepared.clone()), Environment::default())?;
        loop {
            let progress = simulation.advance_to(
                0.0,
                MAX_BATCH_STEPS,
                &mut |_, _| {
                    Err("Rope collision requires the prepared Session world provider".into())
                },
                &mut || Ok(()),
            )?;
            if progress.complete {
                break;
            }
        }
        simulation.snapshot()
    })();
    match result {
        Ok(preview) => preview,
        Err(message) => {
            if prepared.issues.is_empty() {
                prepared.issues.push(Issue {
                    entity: prepared.spans.first().map_or(0, |span| span.source),
                    message,
                });
            }
            Preview {
                issues: prepared.issues,
                omitted: prepared.omitted,
                ..Preview::default()
            }
        }
    }
}
