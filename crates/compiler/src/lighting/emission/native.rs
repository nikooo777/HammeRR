use std::f64::consts::PI;

use glam::DVec3;
use source_formats::bsp::{
    Bsp, LightingKind, MAX_WORLD_LIGHTS, Model, WorldLight, world_light_kind,
};

use super::super::bake::Budget;
use super::{NativePatch, Prepared};

mod cluster;

use cluster::Clusters;

const CACHE_CELL_EXTENT: DVec3 = DVec3::new(32.0, 32.0, 128.0);
const NATIVE_RADIUS_OPERATIONS: f64 = 32.0;
const RADIUS_BOUND_OPERATIONS: f64 = 32.0;
const NATIVE_PATCH_WORK: usize = 128;

fn radius(world: &Model, origin: [f32; 3]) -> Result<f32, String> {
    let mins = DVec3::from_array(world.mins.map(f64::from));
    let maxs = DVec3::from_array(world.maxs.map(f64::from));
    let origin = DVec3::from_array(origin.map(f64::from));
    if !mins.is_finite() || !maxs.is_finite() || !mins.cmple(maxs).all() || !origin.is_finite() {
        return Err("Native emission needs finite ordered Model0 bounds and origin".into());
    }
    let mins = mins - CACHE_CELL_EXTENT;
    let maxs = maxs + CACHE_CELL_EXTENT;
    let scale = mins.abs().max(maxs.abs()).max(origin.abs()).max(DVec3::ONE);
    let native_roundoff = NATIVE_RADIUS_OPERATIONS * f64::from(f32::EPSILON);
    let calculation_roundoff = RADIUS_BOUND_OPERATIONS * f64::EPSILON;
    let allowance = scale * (native_roundoff + calculation_roundoff);
    let distances = (origin - mins).abs().max((origin - maxs).abs()) + allowance;
    let bound = (distances.length() * (1.0 + native_roundoff + calculation_roundoff)).max(1.0);
    if !bound.is_finite() || bound > f64::from(f32::MAX) {
        return Err("Native emission radius exceeds finite native bounds".into());
    }
    let mut radius = bound as f32;
    if f64::from(radius) < bound {
        radius = radius.next_up();
    }
    if !radius.is_finite() || radius < 1.0 || !(radius * radius).is_finite() {
        return Err("Native emission radius exceeds finite native squared distance".into());
    }
    Ok(radius)
}

fn record(
    clusters: &mut Clusters,
    patch: &NativePatch<'_>,
    budget: &mut Budget<'_>,
) -> Result<WorldLight, String> {
    budget.consume(NATIVE_PATCH_WORK)?;
    let [a, b, c] = *patch.points;
    if patch.points.iter().any(|point| !point.is_finite())
        || !patch.radiance.is_finite()
        || patch.radiance.min_element() < 0.0
        || patch.radiance == DVec3::ZERO
    {
        return Err(
            "Native emission needs finite geometry and nonzero nonnegative radiance".into(),
        );
    }
    let cross = (b - a).cross(c - a);
    let length = cross.length();
    if !length.is_finite() || length <= 0.0 {
        return Err("Native emission source triangle must have finite positive area".into());
    }
    let centroid = (a + b + c) / 3.0;
    let normal = cross / length;
    let intensity = patch.radiance * (length * 0.5 / PI);
    if !centroid.is_finite() || !normal.is_finite() || !intensity.is_finite() {
        return Err("Native emission source geometry or intensity overflow".into());
    }
    let origin = centroid.as_vec3();
    let normal = normal.as_vec3();
    let native_intensity = intensity.as_vec3();
    if !origin.is_finite()
        || !normal.is_finite()
        || normal.length_squared() == 0.0
        || !native_intensity.is_finite()
        || native_intensity.min_element() < 0.0
        || native_intensity == glam::Vec3::ZERO
        || (patch.radiance.cmpgt(DVec3::ZERO) & native_intensity.as_dvec3().cmpeq(DVec3::ZERO))
            .any()
    {
        return Err(
            "Native emission loses finite geometry or positive intensity in f32 conversion".into(),
        );
    }
    let origin = origin.to_array();
    let normal = normal.to_array();
    let radius = radius(clusters.world(), origin)?;
    let cluster = clusters.cluster(
        patch,
        origin,
        DVec3::from_array(normal.map(f64::from)),
        budget,
    )?;
    let texinfo = i32::try_from(patch.texinfo)
        .map_err(|_| "Native emission texinfo exceeds its native integer field")?;
    Ok(WorldLight {
        origin,
        intensity: native_intensity.to_array(),
        normal,
        cluster,
        kind: world_light_kind::SURFACE,
        style: 0,
        stop_dot: 0.0,
        stop_dot2: 0.0,
        exponent: 0.0,
        radius,
        constant_attenuation: 0.0,
        linear_attenuation: 0.0,
        quadratic_attenuation: 0.0,
        flags: 0,
        texinfo,
        owner: -1,
    })
}

pub(in crate::lighting) fn append(
    prepared: &Prepared,
    kind: LightingKind,
    bsp: &Bsp,
    expected: usize,
    lights: &mut Vec<WorldLight>,
    budget: &mut Budget<'_>,
) -> Result<usize, String> {
    if expected == 0 {
        return Ok(0);
    }
    if !prepared.native_enabled()
        || lights
            .len()
            .checked_add(expected)
            .is_none_or(|count| count > MAX_WORLD_LIGHTS)
    {
        return Err("Native emission admission exceeds the shared WorldLight limit".into());
    }
    let mut clusters = Clusters::new(bsp, budget)?;
    let mut emitted = 0;
    prepared.visit_native_patches(kind, budget, |patch, budget| {
        if emitted == expected {
            return Err("Native emission source count changed after admission".into());
        }
        let light = record(&mut clusters, &patch, budget).map_err(|message| {
            format!(
                "centroid {:?}: {message}",
                (patch.points[0] + patch.points[1] + patch.points[2]) / 3.0,
            )
        })?;
        lights.push(light);
        emitted += 1;
        Ok(())
    })?;
    if emitted != expected {
        return Err("Native emission source count changed after admission".into());
    }
    Ok(emitted)
}
