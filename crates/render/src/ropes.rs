use std::collections::{HashMap, HashSet};

use glam::DVec3;
use map_core::ObjectId;
use map_core::query::{Hit, Ray};
use source_formats::vmf::{Entity, Vmf};

pub const MAX_LINKS: usize = 512;
pub const MAX_CURVE_SEGMENTS: usize = 64;
pub const TUBE_SIDES: usize = 6;
pub const MAX_EXTENT: f64 = 65_536.0;
pub const MAX_SLACK: f64 = 65_536.0;
pub const MAX_WIDTH: f64 = 64.0;
pub const MAX_IDENTIFIER_BYTES: usize = 256;
pub const MAX_DIAGNOSTIC_EXCERPT_BYTES: usize = 64;
const LENGTH_ITERATIONS: usize = 32;

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
    pub target: u32,
    pub points: Vec<DVec3>,
    pub width: f64,
    pub slack: f64,
}

#[derive(Debug, Default)]
pub struct Preview {
    pub links: Vec<Link>,
    pub issues: Vec<Issue>,
    pub omitted: usize,
}

fn property<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = entity
        .properties
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key));
    let value = values.next().map(|(_, value)| value.trim());
    if values.next().is_some() {
        return Err(format!(
            "{key} has ambiguous case-insensitive duplicate keys"
        ));
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

fn identifier<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let value = property(entity, key)?;
    if let Some(value) = value
        && value.len() > MAX_IDENTIFIER_BYTES
    {
        return Err(format!(
            "{key} identifier exceeds {MAX_IDENTIFIER_BYTES} bytes ({} bytes; starts with '{}')",
            value.len(),
            excerpt(value),
        ));
    }
    Ok(value)
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

fn endpoint(entity: &Entity) -> Result<DVec3, String> {
    property(entity, "classname")?;
    identifier(entity, "targetname")?;
    if !entity.solids.is_empty() {
        return Err("Rope endpoints must be point entities".into());
    }
    if property(entity, "parentname")?.is_some_and(|value| !value.is_empty()) {
        return Err("Parented rope endpoints and parent attachments are unsupported".into());
    }
    for key in ["StartAttachment", "EndAttachment", "parentattachment"] {
        if property(entity, key)?.is_some_and(|value| !value.is_empty() && value != "0") {
            return Err(format!("{key} attachment binding is unsupported"));
        }
    }
    const ORIGIN_ERROR: &str = "Rope endpoint needs exactly three finite origin coordinates";
    let mut values = property(entity, "origin")?
        .ok_or(ORIGIN_ERROR)?
        .split_whitespace();
    let mut coordinates = [0.0; 3];
    for coordinate in &mut coordinates {
        *coordinate = values
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .ok_or(ORIGIN_ERROR)?;
    }
    if values.next().is_some() {
        return Err(ORIGIN_ERROR.into());
    }
    let origin = DVec3::from_array(coordinates);
    if !origin.is_finite() || origin.abs().max_element() > MAX_EXTENT {
        return Err("Rope endpoint coordinates must be finite and within ±65536".into());
    }
    Ok(origin)
}

fn sampled_curve(start: DVec3, end: DVec3, sag: f64, segments: usize) -> Vec<DVec3> {
    (0..=segments)
        .map(|index| {
            let t = index as f64 / segments as f64;
            start.lerp(end, t) - DVec3::Z * (4.0 * sag * t * (1.0 - t))
        })
        .collect()
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

    pub fn triangles(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ {
        let right = (self.points[self.points.len() - 1] - self.points[0])
            .cross(DVec3::Z)
            .try_normalize()
            .unwrap_or(DVec3::X)
            * (self.width * 0.5);
        let ring = move |index: usize, angle: f64| {
            let before = self.points[index.saturating_sub(1)];
            let after = self.points[(index + 1).min(self.points.len() - 1)];
            let up = (after - before).normalize_or_zero().cross(right);
            self.points[index] + right * angle.cos() + up * angle.sin()
        };
        (0..self.points.len() - 1).flat_map(move |index| {
            (0..TUBE_SIDES).flat_map(move |side| {
                let angle = side as f64 * std::f64::consts::TAU / TUBE_SIDES as f64;
                let next = (side + 1) as f64 * std::f64::consts::TAU / TUBE_SIDES as f64;
                let a = ring(index, angle);
                let b = ring(index, next);
                let c = ring(index + 1, next);
                let d = ring(index + 1, angle);
                [[a, c, b], [a, d, c]]
            })
        })
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

fn link(source: &Entity, target: &Entity) -> Result<Link, String> {
    let start = endpoint(source)?;
    let end = endpoint(target).map_err(|error| format!("NextKey endpoint: {error}"))?;
    let distance = start.distance(end);
    if distance < 0.001 {
        return Err("Rope endpoints must be distinct".into());
    }
    let slack = number(source, "Slack", 25.0)?;
    if !(0.0..=MAX_SLACK).contains(&slack) {
        return Err("Slack must be between 0 and 65536".into());
    }
    if slack > 0.0 && (end - start).truncate().length() < 0.001 {
        return Err("Vertical endpoints with positive Slack need a physics pose; no static curve is inferred".into());
    }
    let width = number(source, "Width", 2.0)?;
    if !(1.0..=MAX_WIDTH).contains(&width) {
        return Err("Width must be between 1 and 64".into());
    }
    let subdiv = number(source, "Subdiv", 2.0)?;
    if !(0.0..=8.0).contains(&subdiv) || subdiv.fract() != 0.0 {
        return Err("Subdiv must be an integer from 0 to 8".into());
    }
    if number(source, "Type", 0.0)? != 0.0 {
        return Err("Only Type 0 (Rope) has a static slack preview".into());
    }
    if number(source, "Dangling", 0.0)? != 0.0 {
        return Err("Dangling endpoints are unsupported by the static linked preview".into());
    }
    if source
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("move_rope"))
        && number(source, "PositionInterpolator", 2.0)? != 2.0
    {
        return Err("Only PositionInterpolator 2 (Rope) is supported".into());
    }
    let segments = 8 + subdiv as usize * 7;
    let mut low = 0.0;
    let mut high = distance + slack;
    if slack > 0.0 {
        for _ in 0..LENGTH_ITERATIONS {
            let sag = (low + high) * 0.5;
            let points = sampled_curve(start, end, sag, segments);
            let length: f64 = points
                .windows(2)
                .map(|pair| pair[0].distance(pair[1]))
                .sum();
            if length < distance + slack {
                low = sag;
            } else {
                high = sag;
            }
        }
    } else {
        high = 0.0;
    }
    let points = sampled_curve(start, end, high, segments);
    if points
        .iter()
        .any(|point| point.abs().max_element() + width * 0.5 > MAX_EXTENT)
    {
        return Err("Static rope curve exceeds the ±65536 preview bounds".into());
    }
    if points
        .windows(2)
        .any(|pair| pair[0].distance_squared(pair[1]) < 1e-12)
    {
        return Err("Static rope curve contains a degenerate segment".into());
    }
    Ok(Link {
        source: source.id,
        target: target.id,
        points,
        width,
        slack,
    })
}

pub fn prepare(vmf: &Vmf, include: impl Fn(ObjectId) -> bool) -> Preview {
    let mut preview = Preview::default();
    let mut sources = Vec::new();
    for source in vmf
        .entities
        .iter()
        .filter(|entity| !entity.hidden && is_rope(entity) && include(ObjectId::Entity(entity.id)))
    {
        if sources.len() == MAX_LINKS {
            preview.omitted += 1;
        } else {
            sources.push(source);
        }
    }
    let inspected = sources
        .iter()
        .map(|entity| entity.id)
        .collect::<HashSet<_>>();
    let mut names = HashMap::new();
    for source in &sources {
        if let Ok(Some(next)) = identifier(source, "NextKey")
            && !next.is_empty()
        {
            names.insert(next.to_ascii_lowercase(), Target::Missing);
        }
    }
    if !names.is_empty() {
        for entity in &vmf.entities {
            for (key, value) in entity.properties.iter() {
                if !key.eq_ignore_ascii_case("targetname") {
                    continue;
                }
                let value = value.trim();
                if value.len() <= MAX_IDENTIFIER_BYTES
                    && let Some(target) = names.get_mut(&value.to_ascii_lowercase())
                {
                    *target = match target {
                        Target::Missing => Target::Unique(entity),
                        Target::Unique(_) | Target::Ambiguous => Target::Ambiguous,
                    };
                }
            }
        }
    }
    for source in sources {
        let result = (|| {
            identifier(source, "targetname")?;
            let next = identifier(source, "NextKey")?.unwrap_or("");
            if next.is_empty() {
                if source
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("move_rope"))
                {
                    return Err("move_rope needs an explicit NextKey endpoint".into());
                }
                return Ok(None);
            }
            if next.contains(['*', '?', '!']) {
                return Err("NextKey needs an exact authored targetname".into());
            }
            let target = match names.get(&next.to_ascii_lowercase()) {
                Some(Target::Unique(target)) => *target,
                Some(Target::Ambiguous) => {
                    return Err(format!(
                        "NextKey '{}' matches multiple targetname entries",
                        excerpt(next),
                    ));
                }
                Some(Target::Missing) | None => {
                    return Err(format!(
                        "NextKey '{}' has no authored target",
                        excerpt(next),
                    ));
                }
            };
            if source.id == target.id {
                return Err("NextKey links to the same entity".into());
            }
            if !is_rope(target) {
                return Err("NextKey must reference move_rope or keyframe_rope".into());
            }
            if target.hidden || !include(ObjectId::Entity(target.id)) {
                return Err("NextKey endpoint is hidden or outside the working region".into());
            }
            if !inspected.contains(&target.id) {
                return Err(
                    "NextKey endpoint exceeds the bounded rope-node inspection limit".into(),
                );
            }
            link(source, target).map(Some)
        })();
        match result {
            Ok(Some(link)) => preview.links.push(link),
            Ok(None) => (),
            Err(message) => preview.issues.push(Issue {
                entity: source.id,
                message,
            }),
        }
    }
    let edges: HashMap<_, _> = preview
        .links
        .iter()
        .map(|link| (link.source, link.target))
        .collect();
    let mut cycles = HashSet::new();
    let mut completed = HashSet::new();
    for source in edges.keys() {
        let mut path = Vec::new();
        let mut positions = HashMap::new();
        let mut current = *source;
        while let Some(&next) = edges.get(&current) {
            if completed.contains(&current) {
                break;
            }
            if let Some(&index) = positions.get(&current) {
                cycles.extend(path[index..].iter().copied());
                break;
            }
            positions.insert(current, path.len());
            path.push(current);
            current = next;
        }
        completed.extend(path);
    }
    preview.links.retain(|link| {
        if cycles.contains(&link.source) {
            preview.issues.push(Issue {
                entity: link.source,
                message: "NextKey is part of a cyclic rope link".into(),
            });
            false
        } else {
            true
        }
    });
    preview
}
