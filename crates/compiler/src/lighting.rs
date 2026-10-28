use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::{
    LightingKind, MAX_LIGHTING_BYTES, RgbExp32, TexInfo, WorldLight, world_light_kind,
};

use crate::map::MapEntity;
pub(crate) mod asset_tracking;
mod bake;
mod bounce;
mod brush_alpha;
mod bump;
mod culling;
mod details;
pub(crate) mod emission;
mod falloff;
mod models;
mod normals;
pub mod preview;
mod probes;
mod progress;
pub use progress::{
    LightingChannel, LightingContributor, LightingInstanceSource, LightingPhase,
    LightingPhaseProgress, LightingProgress, MAX_LIGHTING_CONTRIBUTORS,
};
mod prop_vertices;
pub use prop_vertices::load_prop_vertex_material;
mod props;
mod radiance;
mod receivers;
mod sample_origins;
mod shadows;
mod sky;
pub(crate) mod styles;
mod terrain;
mod terrain_shadows;
mod transport;
pub(crate) use bake::{Settings, bake};

const PROJECTION_EPSILON: f64 = 1e-10;
const MIN_LIGHT_DISTANCE: f64 = 1.0;
const REFERENCE_DISTANCE: f64 = 100.0;
pub const IGNORE_NORMALS_FACTOR: f64 = 0.35;

pub(crate) fn validate_environment_light(entity: &MapEntity) -> Result<(), String> {
    let targets = map_core::light::Targets::new();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        sky::SkyLight::from_entity_kind(entity, &targets, kind)?;
    }
    Ok(())
}

pub(crate) fn validate_environment_light_template(entity: &MapEntity) -> Result<(), String> {
    let targets = map_core::light::Targets::new();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        sky::SkyLight::parse(entity, &targets, kind, true)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) enum ReceiverAngularMode {
    Normal,
    IgnoreNormals,
}

impl ReceiverAngularMode {
    fn direct_factor(self, normal: DVec3, direction: DVec3) -> f64 {
        match self {
            Self::Normal => normal.dot(direction).clamp(0.0, 1.0),
            Self::IgnoreNormals => IGNORE_NORMALS_FACTOR,
        }
    }

    fn ambient_weight(self, normal: DVec3, direction: DVec3) -> f64 {
        match self {
            Self::Normal => normal.dot(direction).max(0.0),
            Self::IgnoreNormals => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LuxelFrame {
    origin: DVec3,
    axes: [DVec3; 2],
    dimensions: [usize; 2],
}

impl LuxelFrame {
    pub fn new(
        plane: Plane,
        info: &TexInfo,
        mins: [i32; 2],
        size: [i32; 2],
    ) -> Result<Self, String> {
        let vectors = info
            .lightmap_vecs
            .map(|row| DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])));
        let normal = plane.normal;
        let determinant = vectors[0].dot(vectors[1].cross(normal));
        let scale = vectors[0].length() * vectors[1].length() * normal.length();
        if !plane.dist.is_finite()
            || !scale.is_finite()
            || scale == 0.0
            || !determinant.is_finite()
            || determinant.abs() <= PROJECTION_EPSILON * scale
        {
            return Err("Lightmap projection is singular or nonfinite".into());
        }
        let dual = [
            vectors[1].cross(normal) / determinant,
            normal.cross(vectors[0]) / determinant,
        ];
        let origin = dual[0] * (f64::from(mins[0]) - f64::from(info.lightmap_vecs[0][3]))
            + dual[1] * (f64::from(mins[1]) - f64::from(info.lightmap_vecs[1][3]))
            + vectors[0].cross(vectors[1]) * (plane.dist / determinant);
        let mut dimensions = [0; 2];
        for (dimension, extent) in dimensions.iter_mut().zip(size) {
            *dimension = usize::try_from(extent)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or("Lightmap extent is invalid")?;
        }
        let count = dimensions[0]
            .checked_mul(dimensions[1])
            .ok_or("Lightmap sample count overflow")?;
        if count > MAX_LIGHTING_BYTES / size_of::<RgbExp32>()
            || !origin.is_finite()
            || dual.iter().any(|axis| !axis.is_finite())
        {
            return Err("Lightmap samples exceed finite Source limits".into());
        }
        Ok(Self {
            origin,
            axes: dual,
            dimensions,
        })
    }

    pub fn dimensions(&self) -> [usize; 2] {
        self.dimensions
    }

    pub fn sample_count(&self) -> usize {
        self.dimensions[0] * self.dimensions[1]
    }

    pub fn position(&self, s: usize, t: usize) -> Result<DVec3, String> {
        if s >= self.dimensions[0] || t >= self.dimensions[1] {
            return Err("Luxel coordinates are outside the face".into());
        }
        let point = self.origin + self.axes[0] * s as f64 + self.axes[1] * t as f64;
        if !point.is_finite() {
            return Err("Luxel world position is nonfinite".into());
        }
        Ok(point)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    source_id: u32,
    pub origin: DVec3,
    style: u8,
    intensity: DVec3,
    falloff: falloff::Falloff,
    cluster: i32,
    cone: Option<SpotCone>,
}

#[derive(Debug, Clone, Copy)]
struct SpotCone {
    direction: DVec3,
    inner: f64,
    outer: f64,
    exponent: f64,
}

fn property<'a>(entity: &'a MapEntity, name: &str) -> Option<&'a str> {
    entity
        .properties
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn number(entity: &MapEntity, name: &str) -> Result<f64, String> {
    let value = property(entity, name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("0");
    nonnegative(value, name)
}

fn nonnegative(value: &str, name: &str) -> Result<f64, String> {
    let value = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Invalid light {name}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("Light {name} must be finite and nonnegative"));
    }
    Ok(value)
}

#[derive(Clone, Copy)]
struct LightColor {
    components: [f64; 3],
    brightness: f64,
}

impl LightColor {
    fn parse(value: &str, kind: LightingKind) -> Result<Self, String> {
        let values: Vec<f64> = value
            .split_whitespace()
            .take(9)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| "Invalid point-light color/brightness")?;
        let offset = usize::from(kind == LightingKind::Hdr && values.len() == 8) * 4;
        let components = match values.len() {
            1 => [values[0]; 3],
            3 | 4 | 8 => [values[offset], values[offset + 1], values[offset + 2]],
            _ => {
                return Err(
                    "Point light brightness needs one, three, four or eight numbers".into(),
                );
            }
        };
        let brightness = if values.len() >= 4 {
            values[offset + 3]
        } else {
            255.0
        };
        Ok(Self {
            components,
            brightness,
        })
    }

    fn intensity(self, gain: f64) -> DVec3 {
        DVec3::from_array(
            self.components
                .map(|value| (value / 255.0).powf(2.2) * self.brightness * gain),
        )
    }
}

fn hdr_color(
    entity: &MapEntity,
    name: &str,
    allow_sentinel: bool,
) -> Result<Option<LightColor>, String> {
    let Some(value) = property(entity, name).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let color = LightColor::parse(value, LightingKind::Hdr)
        .map_err(|error| format!("HDR light {name}: {error}"))?;
    if color.components.iter().any(|value| !value.is_finite()) || !color.brightness.is_finite() {
        return Err(format!(
            "HDR light {name} color and brightness must be finite"
        ));
    }
    if color.components.iter().any(|&value| value < 0.0) {
        if allow_sentinel {
            return Ok(None);
        }
        return Err(format!("HDR light {name} colors must be nonnegative"));
    }
    if color.brightness < 0.0 {
        return Err(format!("HDR light {name} brightness must be nonnegative"));
    }
    Ok(Some(color))
}

fn hdr_scale(
    entity: &MapEntity,
    name: &str,
    defer_parameters: bool,
) -> Result<Option<f64>, String> {
    let Some(value) = property(entity, name) else {
        return Ok(Some(1.0));
    };
    if unresolved_value(entity, name, defer_parameters) {
        return Ok(None);
    }
    nonnegative(value, name).map(Some)
}

fn checked_intensity(intensity: DVec3) -> Result<DVec3, String> {
    if !intensity.is_finite() || intensity.max_element() > f64::from(f32::MAX) {
        return Err("Point light intensity exceeds native world-light limits".into());
    }
    Ok(intensity)
}

fn point_light_pose_style(entity: &MapEntity) -> Result<(DVec3, u8), String> {
    let origin = entity.origin.ok_or("Point light needs an origin")?;
    if !origin.is_finite() || origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Point light origin must be finite and inside Source compile bounds".into());
    }
    let style = number(entity, "style")?;
    let named = property(entity, "targetname").is_some_and(|value| !value.is_empty());
    if style.fract() != 0.0 || style > 63.0 || (style >= 32.0) != named {
        return Err("Point/spot light styles require an authored Appearance from 0 to 31 or a compiler-assigned named style from 32 to 63".into());
    }
    Ok((origin, style as u8))
}

fn unresolved_value(entity: &MapEntity, key: &str, defer_parameters: bool) -> bool {
    defer_parameters
        && property(entity, key).is_some_and(|value| {
            map_core::light::value_parameter(&entity.classname, key, value).is_some()
        })
}

fn point_light_color(
    entity: &MapEntity,
    kind: LightingKind,
    defer_parameters: bool,
) -> Result<Option<LightColor>, String> {
    let color = match kind {
        LightingKind::Ldr => {
            if unresolved_value(entity, "_light", defer_parameters) {
                return Ok(None);
            }
            let color = LightColor::parse(
                property(entity, "_light").ok_or("Point light needs a brightness value")?,
                kind,
            )?;
            if color
                .components
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=255.0).contains(value))
                || !color.brightness.is_finite()
                || color.brightness < 0.0
            {
                return Err(
                    "Point light colors must be 0–255 with finite nonnegative brightness".into(),
                );
            }
            color
        }
        LightingKind::Hdr => {
            if unresolved_value(entity, "_lightHDR", defer_parameters) {
                return Ok(None);
            }
            match hdr_color(entity, "_lightHDR", true)? {
                Some(color) => color,
                None => {
                    if unresolved_value(entity, "_light", defer_parameters) {
                        return Ok(None);
                    }
                    hdr_color(entity, "_light", false)?
                        .ok_or("HDR point light needs _lightHDR or _light brightness")?
                }
            }
        }
    };
    Ok(Some(color))
}

pub(crate) fn validate_point_light_template(
    entity: &MapEntity,
    spotlight: bool,
    targets: &map_core::light::Targets<'_>,
) -> Result<(), String> {
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        PointLight::parse(entity, -1, kind, true)?;
    }
    if spotlight {
        spotlight_cone(
            entity,
            targets,
            entity.origin.ok_or("Point light needs an origin")?,
            true,
        )?;
    }
    Ok(())
}

enum SpotlightCone {
    Unresolved,
    Resolved(Option<SpotCone>),
}

fn spotlight_cone(
    entity: &MapEntity,
    targets: &map_core::light::Targets<'_>,
    origin: DVec3,
    defer_parameters: bool,
) -> Result<SpotlightCone, String> {
    let scalar = |key| {
        if unresolved_value(entity, key, defer_parameters) {
            Ok(None)
        } else {
            number(entity, key).map(Some)
        }
    };
    let inner = if property(entity, "_inner_cone").is_none() {
        Some(10.0)
    } else {
        scalar("_inner_cone")?
    };
    let inner = inner.map(|inner| if inner == 0.0 { 10.0 } else { inner });
    let outer = scalar("_cone")?;
    let outer = if outer == Some(0.0) { inner } else { outer };
    if inner == Some(180.0) && outer == Some(180.0) {
        return Ok(SpotlightCone::Resolved(None));
    }
    if inner
        .into_iter()
        .chain(outer)
        .any(|angle| angle > 90.0 && angle != 180.0)
        || matches!((inner, outer), (Some(inner), Some(outer)) if inner > 90.0 || outer > 90.0 || outer < inner)
    {
        return Err(
            "Spotlight cones need inner ≤ outer ≤ 90 degrees, or both 180 for a point light".into(),
        );
    }
    if inner.is_none_or(|inner| inner == 180.0) && outer.is_none_or(|outer| outer == 180.0) {
        return Ok(SpotlightCone::Unresolved);
    }
    let exponent = scalar("_exponent")?;
    if exponent.is_some_and(|exponent| exponent > f64::from(f32::MAX)) {
        return Err("Spotlight focus exceeds native float limits".into());
    }
    let target = map_core::light::target(property(entity, "target"), targets)?;
    let direction = if unresolved_value(entity, "pitch", defer_parameters) {
        map_core::light::template_direction(
            property(entity, "angles"),
            property(entity, "angle"),
            property(entity, "pitch"),
            origin,
            target,
        )?
    } else {
        Some(map_core::light::direction(
            property(entity, "angles"),
            property(entity, "angle"),
            property(entity, "pitch"),
            origin,
            target,
        )?)
    };
    let (Some(inner), Some(outer), Some(exponent), Some(direction)) =
        (inner, outer, exponent, direction)
    else {
        return Ok(SpotlightCone::Unresolved);
    };
    Ok(SpotlightCone::Resolved(Some(SpotCone {
        direction,
        inner: inner.to_radians().cos(),
        outer: outer.to_radians().cos(),
        exponent,
    })))
}

impl PointLight {
    pub fn from_entity(entity: &MapEntity, cluster: i32) -> Result<Self, String> {
        Self::from_entity_kind(entity, cluster, LightingKind::Ldr)
    }

    pub fn from_entity_kind(
        entity: &MapEntity,
        cluster: i32,
        kind: LightingKind,
    ) -> Result<Self, String> {
        Self::parse(entity, cluster, kind, false)?
            .ok_or_else(|| "Point light has an unresolved light-value parameter".into())
    }

    fn parse(
        entity: &MapEntity,
        cluster: i32,
        kind: LightingKind,
        defer_parameters: bool,
    ) -> Result<Option<Self>, String> {
        let (origin, style) = point_light_pose_style(entity)?;
        let color = point_light_color(entity, kind, defer_parameters)?;
        let falloff = falloff::Falloff::read(entity, defer_parameters)?;
        let scale = if kind == LightingKind::Hdr {
            hdr_scale(entity, "_lightscaleHDR", defer_parameters)?
        } else {
            Some(1.0)
        };
        let (Some(color), Some(scale), Some(falloff)) = (color, scale, falloff) else {
            return Ok(None);
        };
        let mut intensity = color.intensity(falloff.gain);
        if kind == LightingKind::Hdr {
            intensity *= scale;
        }
        let intensity = checked_intensity(intensity)?;
        Ok(Some(Self {
            source_id: entity.id,
            origin,
            style,
            intensity,
            falloff,
            cluster,
            cone: None,
        }))
    }

    pub fn with_spotlight(
        mut self,
        entity: &MapEntity,
        targets: &map_core::light::Targets<'_>,
    ) -> Result<Self, String> {
        match spotlight_cone(entity, targets, self.origin, false)? {
            SpotlightCone::Resolved(Some(cone)) => self.cone = Some(cone),
            SpotlightCone::Resolved(None) => {}
            SpotlightCone::Unresolved => {
                return Err("Spotlight has an unresolved light-value parameter".into());
            }
        }
        Ok(self)
    }

    pub fn contribution(&self, point: DVec3, normal: DVec3) -> Result<DVec3, String> {
        self.contribution_with_mode(point, normal, ReceiverAngularMode::Normal)
    }

    fn contribution_with_mode(
        &self,
        point: DVec3,
        normal: DVec3,
        mode: ReceiverAngularMode,
    ) -> Result<DVec3, String> {
        if !point.is_finite() || !normal.is_finite() {
            return Err("Light sample and normal must be finite".into());
        }
        let offset = self.origin - point;
        let distance = offset.length();
        if !distance.is_finite() {
            return Err("Point-light distance is nonfinite".into());
        }
        let normal = normal
            .try_normalize()
            .ok_or("Light sample needs a nonzero normal")?;
        if distance == 0.0 {
            return Ok(DVec3::ZERO);
        }
        let dot = mode.direct_factor(normal, offset / distance);
        let falloff = self.falloff.factor(distance)?;
        let cone = if let Some(cone) = self.cone {
            let angle = cone.direction.dot(-offset / offset.length());
            if angle <= cone.outer {
                0.0
            } else if angle >= cone.inner {
                angle
            } else {
                let fringe = ((angle - cone.outer) / (cone.inner - cone.outer)).clamp(0.0, 1.0);
                angle
                    * if cone.exponent == 0.0 || cone.exponent == 1.0 {
                        fringe
                    } else {
                        fringe.powf(cone.exponent)
                    }
            }
        } else {
            1.0
        };
        let result = self.intensity * (dot * cone * falloff);
        if !result.is_finite() {
            return Err("Point-light contribution is nonfinite".into());
        }
        Ok(result)
    }

    pub fn world_light(&self) -> WorldLight {
        let mut light = WorldLight {
            origin: self.origin.as_vec3().to_array(),
            intensity: (self.intensity / 255.0).as_vec3().to_array(),
            cluster: self.cluster,
            kind: world_light_kind::POINT,
            style: i32::from(self.style),
            radius: self.falloff.radius as f32,
            constant_attenuation: self.falloff.coefficients[0] as f32,
            linear_attenuation: self.falloff.coefficients[1] as f32,
            quadratic_attenuation: self.falloff.coefficients[2] as f32,
            texinfo: -1,
            owner: -1,
            ..WorldLight::default()
        };
        if let Some(cone) = self.cone {
            light.kind = world_light_kind::SPOT;
            light.normal = cone.direction.as_vec3().to_array();
            light.stop_dot = cone.inner as f32;
            light.stop_dot2 = cone.outer as f32;
            light.exponent = cone.exponent as f32;
        }
        light
    }
}

pub fn encode_color(color: DVec3) -> Result<RgbExp32, String> {
    if !color.is_finite() || color.min_element() < 0.0 {
        return Err("Light color must be finite and nonnegative".into());
    }
    let maximum = color.max_element();
    if maximum == 0.0 {
        return Ok(RgbExp32::default());
    }
    let exponent = (maximum.log2().floor() as i32 - 7).max(i32::from(i8::MIN));
    if exponent > i32::from(i8::MAX) {
        return Err("Light color exceeds RGBExp32 limits".into());
    }
    let scaled = color / 2.0_f64.powi(exponent);
    Ok(RgbExp32 {
        rgb: scaled.to_array().map(|value| value.min(255.0) as u8),
        exponent: exponent as i8,
    })
}

#[derive(Debug, Clone)]
pub struct Occluder {
    planes: Vec<Plane>,
    sky: Vec<bool>,
    source: Option<HullSource>,
    optical: Option<Box<OpticalHull>>,
    mins: DVec3,
    maxs: DVec3,
}

#[derive(Debug, Clone)]
struct OpticalHull {
    global_caster: bool,
    faces: Vec<Option<brush_alpha::Face>>,
    pose: Option<crate::brush_model_pose::Pose>,
}

pub(crate) fn validate_brush_cutout_material(
    material: &source_formats::vmt::Material,
) -> Result<(), String> {
    props::alpha::validate_brush(material)
}

#[derive(Debug, Clone)]
struct HullSource {
    brush: usize,
    sides: Vec<HullSideSource>,
}

#[derive(Debug, Clone, Copy)]
struct HullSideSource {
    plane: usize,
    side: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct HullInterval {
    entering: f64,
    leaving: f64,
    sky: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HullBoundary {
    pub(super) fraction: f64,
    pub(super) coverage: u8,
    pub(super) non_sky_coverage: u8,
    pub(super) sky: bool,
}

impl HullBoundary {
    fn add(&mut self, coverage: u8, sky: bool) {
        self.coverage = self.coverage.max(coverage);
        if sky {
            self.sky |= coverage != 0;
        } else {
            self.non_sky_coverage = self.non_sky_coverage.max(coverage);
        }
    }
}

impl Occluder {
    pub fn new(planes: Vec<Plane>, mins: DVec3, maxs: DVec3) -> Result<Self, String> {
        if planes.len() < 4
            || planes.iter().any(|plane| {
                !plane.normal.is_finite()
                    || !plane.normal.length_squared().is_finite()
                    || plane.normal.length_squared() == 0.0
                    || !plane.dist.is_finite()
            })
            || !mins.is_finite()
            || !maxs.is_finite()
            || !mins.cmplt(maxs).all()
        {
            return Err("Shadow hull needs finite convex planes and positive bounds".into());
        }
        Ok(Self {
            sky: vec![false; planes.len()],
            source: None,
            optical: None,
            planes,
            mins,
            maxs,
        })
    }

    pub fn plane_count(&self) -> usize {
        self.planes.len()
    }

    pub fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        Ok(self.entry(start, end)?.is_some())
    }

    pub(super) fn with_sky(mut self, flags: Vec<bool>) -> Result<Self, String> {
        if flags.len() != self.planes.len() {
            return Err("Sky flags must match shadow hull planes".into());
        }
        self.sky = flags;
        Ok(self)
    }

    pub(super) fn entry(&self, start: DVec3, end: DVec3) -> Result<Option<(f64, bool)>, String> {
        Ok(self
            .interval(start, end)?
            .map(|hit| (hit.entering, hit.sky)))
    }

    fn with_source(mut self, brush: usize, sides: Vec<HullSideSource>) -> Result<Self, String> {
        if sides.len() != self.planes.len() {
            return Err("Shadow hull source sides must match its planes".into());
        }
        self.source = Some(HullSource { brush, sides });
        Ok(self)
    }

    fn with_optical(
        mut self,
        faces: Vec<Option<brush_alpha::Face>>,
        pose: Option<crate::brush_model_pose::Pose>,
    ) -> Result<Self, String> {
        if faces.len() != self.planes.len() {
            return Err("Brush texture shadow boundaries must match hull planes".into());
        }
        if faces.iter().any(Option::is_some) {
            self.optical = Some(Box::new(OpticalHull {
                faces,
                pose,
                global_caster: false,
            }));
        }
        Ok(self)
    }

    fn into_global_caster(mut self) -> Self {
        if let Some(optical) = &mut self.optical {
            optical.global_caster = true;
        } else {
            self.optical = Some(Box::new(OpticalHull {
                faces: Vec::new(),
                pose: None,
                global_caster: true,
            }));
        }
        self
    }

    fn occupies_world(&self) -> bool {
        self.optical
            .as_ref()
            .is_none_or(|optical| !optical.global_caster)
    }

    fn optical_entry(&self, start: DVec3, end: DVec3) -> Result<Option<(f64, bool)>, String> {
        Ok(self
            .optical_boundaries(start, end)?
            .into_iter()
            .flatten()
            .find(|boundary| boundary.coverage != 0)
            .map(|boundary| {
                (
                    boundary.fraction,
                    boundary.sky && boundary.non_sky_coverage == 0,
                )
            }))
    }

    pub(super) fn optical_boundaries(
        &self,
        start: DVec3,
        end: DVec3,
    ) -> Result<[Option<HullBoundary>; 2], String> {
        let Some(interval) = self.interval(start, end)? else {
            return Ok([None; 2]);
        };
        let optical = self.optical.as_ref().filter(|hull| !hull.faces.is_empty());
        let mut boundaries = [None; 2];
        if optical.is_none() && interval.entering == 0.0 {
            boundaries[0] = Some(HullBoundary {
                fraction: 0.0,
                coverage: u8::MAX,
                non_sky_coverage: u8::MAX,
                sky: false,
            });
        }
        for (index, plane) in self.planes.iter().enumerate() {
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            if (a > 0.0) == (b > 0.0) {
                continue;
            }
            let fraction = a / (a - b);
            let (slot, boundary) = if a > 0.0 {
                (0, interval.entering)
            } else {
                (1, interval.leaving)
            };
            if (fraction - boundary).abs() > shadows::TRIANGLE_EPSILON
                || fraction <= 0.0
                || fraction >= 1.0
            {
                continue;
            }
            let coverage = if let Some(optical) = optical
                && let Some(face) = &optical.faces[index]
            {
                let world = start.lerp(end, fraction);
                let local = optical.pose.map_or(world, |pose| pose.local_point(world));
                face.coverage(local)?
            } else {
                u8::MAX
            };
            let event = boundaries[slot].get_or_insert(HullBoundary {
                fraction: boundary,
                coverage: 0,
                non_sky_coverage: 0,
                sky: false,
            });
            event.add(coverage, self.sky[index]);
        }
        Ok(boundaries)
    }

    fn interval(&self, start: DVec3, end: DVec3) -> Result<Option<HullInterval>, String> {
        if !start.is_finite() || !end.is_finite() {
            return Err("Shadow ray must be finite".into());
        }
        if start.min(end).cmpgt(self.maxs).any() || start.max(end).cmplt(self.mins).any() {
            return Ok(None);
        }
        let mut entering: f64 = 0.0;
        let mut leaving: f64 = 1.0;
        let mut sky = false;
        for (index, plane) in self.planes.iter().enumerate() {
            let is_sky = self.sky.get(index).copied().unwrap_or(false);
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            if !a.is_finite() || !b.is_finite() {
                return Err("Shadow-plane distance is nonfinite".into());
            }
            if a > 0.0 && b > 0.0 {
                return Ok(None);
            }
            if a <= 0.0 && b <= 0.0 {
                continue;
            }
            let fraction = a / (a - b);
            if a > 0.0 {
                if fraction > entering {
                    sky = is_sky;
                } else if fraction == entering {
                    sky &= is_sky;
                }
                entering = entering.max(fraction);
            } else {
                leaving = leaving.min(fraction);
            }
            if entering > leaving {
                return Ok(None);
            }
        }
        Ok(
            (entering < leaving && leaving > 0.0 && entering < 1.0).then_some(HullInterval {
                entering,
                leaving,
                sky,
            }),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn sky_visibility_skipping_owner(
    direction: DVec3,
    origin: DVec3,
    shadows: &shadows::ShadowIndex<Occluder>,
    terrain: &shadows::ShadowIndex<terrain_shadows::TerrainTriangle>,
    props: &shadows::ShadowIndex<props::PropTriangle>,
    model: &Option<shadows::ShadowIndex<Occluder>>,
    skip_owner: Option<u32>,
    budget: &mut bake::Budget<'_>,
) -> Result<f64, String> {
    budget.consume(1)?;
    let end = origin + direction * sky::TRACE_LENGTH;
    if let Some(fraction) =
        shadows.optical_sky_fraction(origin, end, &mut |work| budget.consume(work))?
    {
        transport::Scene {
            shadows,
            terrain,
            props,
            model: model.as_ref(),
        }
        .sky_visibility(origin, end, fraction, skip_owner, budget)
    } else {
        Ok(0.0)
    }
}
