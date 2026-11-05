use std::{
    collections::{BTreeSet, VecDeque},
    fmt,
};

use glam::{DVec3, Vec3};

use crate::{Plane, checks::SOURCE_COORDINATE_LIMIT};

pub const HULL_HALF_EXTENT: f64 = 2.0;
pub const CONTACT_EPSILON: f64 = 1.0 / 32.0;
pub const MAX_BRUSHES: usize = 4096;
pub const MAX_BRUSH_PLANES: usize = 1024;
pub const MAX_PLANES: usize = 131_072;
pub const MAX_TRIANGLES: usize = 65_536;
pub const MAX_TERRAIN_PATCHES: usize = 128;
pub const MAX_STATIONARY_CERTIFICATE_WORK: usize = MAX_TRIANGLES * MAX_TERRAIN_PATCHES;
pub const MAX_STATIONARY_TREE_NODES: usize = 8191;
pub const MAX_STATIONARY_REFERENCES: usize = 262_144;
pub const MAX_STATIONARY_QUERY_LEAVES: usize = 1024;
pub const MAX_TRACE_WORK: usize = 16_000_000;

const UNIT_TOLERANCE: f64 = 1e-4;
const TERRAIN_FRACTION_TOLERANCE: f64 = 0.001;
const NATIVE_STAB_DISTANCE: f32 = 99_999.9;
const NATIVE_NORMAL_BIAS: f32 = 1e-10;
const MAX_NATIVE_NORMAL_Q: f32 = 1e30;
const NATIVE_SCALE_LOWER: f64 = 0.99;
const NATIVE_SCALE_UPPER: f64 = 1.01;
const NATIVE_ENDPOINT_ROUNDING: f64 = 1.0 / 64.0;
const DOT_ROUNDING_OPERATIONS: f64 = 16.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidGeometry(String),
    InvalidTrace,
    UncertifiedStationaryTerrain,
    StationaryNormalization,
    AmbiguousStationaryContact,
    ResourceLimit(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGeometry(message) => formatter.write_str(message),
            Self::InvalidTrace => {
                formatter.write_str("Rope hull traces need finite endpoints within ±65536 units")
            }
            Self::UncertifiedStationaryTerrain => formatter.write_str(
                "Stationary rope terrain volume is unavailable: the all-direction departure certificate failed; this terrain needs BSP leaf classification",
            ),
            Self::StationaryNormalization => formatter.write_str(
                "Rope stationary mixed terrain requires the qualified finite x86-64 normalization profile",
            ),
            Self::AmbiguousStationaryContact => formatter.write_str(
                "Stationary rope terrain volume has tied contact planes with different solid classifications",
            ),
            Self::ResourceLimit(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trace {
    pub fraction: f64,
    pub start_solid: bool,
    pub all_solid: bool,
    pub normal: DVec3,
    pub plane_distance: f64,
    pub end: DVec3,
}

impl Trace {
    pub fn clear(end: DVec3) -> Self {
        Self {
            fraction: 1.0,
            start_solid: false,
            all_solid: false,
            normal: DVec3::ZERO,
            plane_distance: 0.0,
            end,
        }
    }
}

#[derive(Debug)]
pub struct TraceBudget {
    work: usize,
    limit: usize,
}

impl Default for TraceBudget {
    fn default() -> Self {
        Self::limited(MAX_TRACE_WORK)
    }
}

impl TraceBudget {
    pub fn limited(limit: usize) -> Self {
        Self {
            work: 0,
            limit: limit.min(MAX_TRACE_WORK),
        }
    }

    pub fn work(&self) -> usize {
        self.work
    }

    fn consume(&mut self, amount: usize) -> Result<(), Error> {
        self.work = self
            .work
            .checked_add(amount)
            .filter(|&work| work <= self.limit)
            .ok_or(Error::ResourceLimit(
                "Rope hull traces exceeded their aggregate work limit",
            ))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct Brush {
    pub solid: u32,
    pub mins: DVec3,
    pub maxs: DVec3,
    /// Complete compiler-prepared outward collision planes, including bevels.
    pub planes: Vec<Plane>,
}

#[derive(Debug)]
pub struct Triangle {
    pub solid: u32,
    pub side: u32,
    pub positions: [DVec3; 3],
}

#[derive(Debug)]
pub struct TerrainPatch {
    pub solid: u32,
    pub side: u32,
    pub base: [DVec3; 4],
    pub mins: DVec3,
    pub maxs: DVec3,
    pub matches_mask: bool,
}

#[derive(Debug, Default)]
pub struct StationaryNode {
    pub split: Option<(Plane, [usize; 2])>,
    pub brushes: Vec<usize>,
    pub patches: Vec<usize>,
    pub solid: bool,
}

#[derive(Debug)]
pub struct StationaryTopology {
    pub nodes: Vec<StationaryNode>,
}

#[derive(Debug)]
struct Bounds {
    mins: DVec3,
    maxs: DVec3,
}

impl Bounds {
    fn overlaps(&self, other: &Self) -> bool {
        self.mins.cmple(other.maxs).all() && self.maxs.cmpge(other.mins).all()
    }
}

#[derive(Debug)]
struct Primitive {
    bounds: Bounds,
    planes: Vec<Plane>,
    terrain: Option<Terrain>,
    order: usize,
}

#[derive(Debug)]
struct Terrain {
    source: (u32, u32),
    normal: DVec3,
    native_normal: Option<[Interval; 3]>,
    positions: [DVec3; 3],
}

#[derive(Debug, Clone, Copy)]
struct Interval {
    min: f64,
    max: f64,
}

impl Interval {
    fn multiply(self, other: Self) -> Self {
        [
            self.min * other.min,
            self.min * other.max,
            self.max * other.min,
            self.max * other.max,
        ]
        .into_iter()
        .fold(
            Self {
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
            },
            |bounds, value| Self {
                min: bounds.min.min(value),
                max: bounds.max.max(value),
            },
        )
    }

    fn magnitude(self) -> f64 {
        self.min.abs().max(self.max.abs())
    }

    fn rounded(self) -> Self {
        let error = 2.0 * f64::from(f32::EPSILON) * self.magnitude() + f64::from(f32::MIN_POSITIVE);
        Self {
            min: self.min - error,
            max: self.max + error,
        }
    }
}

#[derive(Debug, Default)]
pub struct World {
    primitives: Vec<Primitive>,
    brushes: usize,
    triangles: usize,
    patches: Vec<TerrainPatch>,
    primitive_indices: Vec<usize>,
    patch_primitives: Vec<Vec<usize>>,
    stationary_topology: Option<StationaryTopology>,
    stationary_terrain_clear: bool,
}

struct StabState {
    start: DVec3,
    end: DVec3,
    delta: DVec3,
    trace: Trace,
    terrain_hit: bool,
    brushes: Vec<bool>,
    patches: Vec<bool>,
}

fn native_normal_bounds(first: Vec3, second: Vec3) -> Option<[Interval; 3]> {
    let cross = first.cross(second);
    let q = (cross.z * cross.z + NATIVE_NORMAL_BIAS) + (cross.y * cross.y + cross.x * cross.x);
    if !cross.is_finite() || !(NATIVE_NORMAL_BIAS..=MAX_NATIVE_NORMAL_Q).contains(&q) {
        return None;
    }
    let root = f64::from(q).sqrt();
    let scale = Interval {
        min: NATIVE_SCALE_LOWER / root,
        max: NATIVE_SCALE_UPPER / root,
    };
    Some(cross.to_array().map(|component| {
        let component = f64::from(component);
        Interval {
            min: component,
            max: component,
        }
        .multiply(scale)
        .rounded()
    }))
}

#[cfg(target_arch = "x86_64")]
fn native_stab_direction(patch: &TerrainPatch) -> Option<Vec3> {
    use std::arch::x86_64::{_mm_cvtss_f32, _mm_rsqrt_ss, _mm_set_ss};

    let [a, b, _, d] = patch.base.map(|point| point.as_vec3());
    let cross = (d - a).cross(b - a);
    let q = (cross.z * cross.z + NATIVE_NORMAL_BIAS) + (cross.y * cross.y + cross.x * cross.x);
    if !cross.is_finite() || !(NATIVE_NORMAL_BIAS..=MAX_NATIVE_NORMAL_Q).contains(&q) {
        return None;
    }
    let inverse = unsafe { _mm_cvtss_f32(_mm_rsqrt_ss(_mm_set_ss(q))) };
    let squared = inverse * inverse;
    let product = squared * q;
    let correction = (3.0 - product) * 0.5;
    let scale = inverse * correction;
    let direction = cross * scale;
    (direction.is_finite() && direction.length_squared() > 0.0).then_some(direction)
}

#[cfg(not(target_arch = "x86_64"))]
fn native_stab_direction(_patch: &TerrainPatch) -> Option<Vec3> {
    None
}

fn node_distance(plane: Plane, point: DVec3) -> f64 {
    let point = point.as_vec3();
    let normal = plane.normal.as_vec3();
    let z = point.z * normal.z - plane.dist as f32;
    let xy = point.y * normal.y + point.x * normal.x;
    f64::from(z + xy)
}

fn node_extent(plane: Plane, extent: f64) -> f64 {
    let normal = plane.normal.as_vec3();
    let extent = extent as f32;
    let xy = (normal.x * extent).abs() + (normal.y * extent).abs();
    f64::from(xy + (normal.z * extent).abs())
}

fn solid_at(point: DVec3) -> Trace {
    Trace {
        fraction: 0.0,
        start_solid: true,
        all_solid: true,
        normal: DVec3::ZERO,
        plane_distance: 0.0,
        end: point,
    }
}

fn stab_delta_bounds(patch: &TerrainPatch) -> Option<[Interval; 3]> {
    let [a, b, _, d] = patch.base.map(|point| point.as_vec3());
    let normal = native_normal_bounds(d - a, b - a)?;
    Some(std::array::from_fn(|axis| {
        let distance = f64::from(NATIVE_STAB_DISTANCE);
        let step = normal[axis]
            .multiply(Interval {
                min: distance,
                max: distance,
            })
            .rounded();
        if axis == 2 {
            step
        } else {
            Interval {
                min: step.min - NATIVE_ENDPOINT_ROUNDING,
                max: step.max + NATIVE_ENDPOINT_ROUNDING,
            }
        }
    }))
}

fn certifies_departure(normal: [Interval; 3], delta: [Interval; 3]) -> bool {
    let products = std::array::from_fn::<_, 3, _>(|axis| normal[axis].multiply(delta[axis]));
    let minimum: f64 = products.iter().map(|product| product.min).sum();
    let magnitude: f64 = products.iter().map(|product| product.magnitude()).sum();
    let error = DOT_ROUNDING_OPERATIONS * f64::from(f32::EPSILON) * magnitude
        + f64::from(f32::MIN_POSITIVE);
    minimum - error > CONTACT_EPSILON
}

fn valid_point(point: DVec3) -> bool {
    point.is_finite() && point.abs().max_element() <= SOURCE_COORDINATE_LIMIT
}

fn checked_plane(plane: Plane) -> Result<Plane, Error> {
    let length = plane.normal.length();
    if !plane.normal.is_finite() || !plane.dist.is_finite() || (length - 1.0).abs() > UNIT_TOLERANCE
    {
        return Err(Error::InvalidGeometry(
            "Rope collision planes must have finite unit normals and distances".into(),
        ));
    }
    Ok(Plane {
        normal: plane.normal / length,
        dist: plane.dist / length,
    })
}

fn triangle_planes(triangle: Triangle) -> Result<Primitive, Error> {
    let invalid = || {
        Error::InvalidGeometry(format!(
            "Terrain face {} on brush {} has invalid rope collision triangles",
            triangle.side, triangle.solid
        ))
    };
    if triangle.positions.iter().any(|point| !valid_point(*point)) {
        return Err(invalid());
    }
    let [a, b, c] = triangle.positions;
    let normal = (c - a).cross(b - a).try_normalize().ok_or_else(invalid)?;
    let mins = a.min(b).min(c);
    let maxs = a.max(b).max(c);
    let mut planes = Vec::with_capacity(16);
    for axis in (0..3).rev() {
        let mut direction = DVec3::ZERO;
        direction[axis] = 1.0;
        planes.push(Plane {
            normal: -direction,
            // Keep the geometric sign; native displacement traces report +min.
            dist: -mins[axis],
        });
        planes.push(Plane {
            normal: direction,
            dist: maxs[axis],
        });
    }
    for (axis, direction) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate() {
        for (on, next, off) in [(a, b, c), (b, c, a), (c, a, b)] {
            let Some(mut normal) = (next - on).cross(direction).try_normalize() else {
                continue;
            };
            if normal[(axis + 1) % 3] == 0.0 || normal[(axis + 2) % 3] == 0.0 {
                continue;
            }
            let mut distance = normal.dot(on);
            let off_distance = normal.dot(off);
            if (off_distance - distance).abs() >= CONTACT_EPSILON && off_distance > distance {
                normal = -normal;
                distance = -distance;
            }
            planes.push(Plane {
                normal,
                dist: distance,
            });
        }
    }
    let distance = normal.dot(a);
    planes.push(Plane {
        normal,
        dist: distance,
    });
    Ok(Primitive {
        bounds: Bounds { mins, maxs },
        planes,
        terrain: Some(Terrain {
            source: (triangle.solid, triangle.side),
            normal,
            native_normal: native_normal_bounds(
                c.as_vec3() - a.as_vec3(),
                b.as_vec3() - a.as_vec3(),
            ),
            positions: triangle.positions,
        }),
        order: 0,
    })
}

impl World {
    pub fn new(brushes: Vec<Brush>, triangles: Vec<Triangle>) -> Result<Self, Error> {
        if brushes.len() > MAX_BRUSHES || triangles.len() > MAX_TRIANGLES {
            return Err(Error::ResourceLimit(
                "Rope collision exceeds 4096 brushes or 65536 terrain triangles",
            ));
        }
        let mut world = Self {
            primitives: Vec::with_capacity(brushes.len() + triangles.len()),
            brushes: brushes.len(),
            triangles: triangles.len(),
            patches: Vec::new(),
            primitive_indices: Vec::new(),
            patch_primitives: Vec::new(),
            stationary_topology: None,
            stationary_terrain_clear: triangles.is_empty(),
        };
        let mut plane_count = 0usize;
        for brush in brushes {
            if !valid_point(brush.mins)
                || !valid_point(brush.maxs)
                || !brush.mins.cmplt(brush.maxs).all()
                || !(6..=MAX_BRUSH_PLANES).contains(&brush.planes.len())
            {
                return Err(Error::InvalidGeometry(format!(
                    "Brush {} needs bounded complete prepared collision geometry",
                    brush.solid
                )));
            }
            plane_count = plane_count.saturating_add(brush.planes.len());
            if plane_count > MAX_PLANES {
                return Err(Error::ResourceLimit(
                    "Rope collision exceeds 131072 prepared brush planes",
                ));
            }
            let planes = brush
                .planes
                .into_iter()
                .map(checked_plane)
                .collect::<Result<Vec<_>, _>>()?;
            world.primitives.push(Primitive {
                bounds: Bounds {
                    mins: brush.mins,
                    maxs: brush.maxs,
                },
                planes,
                terrain: None,
                order: world.primitives.len(),
            });
        }
        for triangle in triangles {
            let mut primitive = triangle_planes(triangle)?;
            primitive.order = world.primitives.len();
            world.primitives.push(primitive);
        }
        world.primitives.sort_by(|first, second| {
            first
                .bounds
                .mins
                .x
                .total_cmp(&second.bounds.mins.x)
                .then_with(|| first.order.cmp(&second.order))
        });
        world.primitive_indices = vec![0; world.primitives.len()];
        for (index, primitive) in world.primitives.iter().enumerate() {
            world.primitive_indices[primitive.order] = index;
        }
        Ok(world)
    }

    pub fn brushes(&self) -> usize {
        self.brushes
    }

    pub fn triangles(&self) -> usize {
        self.triangles
    }

    pub fn with_terrain_patches(mut self, patches: Vec<TerrainPatch>) -> Result<Self, Error> {
        if patches.len() > MAX_TERRAIN_PATCHES {
            return Err(Error::ResourceLimit(
                "Rope collision exceeds 128 native terrain patches",
            ));
        }
        let mut sources = BTreeSet::new();
        for patch in &patches {
            if !sources.insert((patch.solid, patch.side))
                || patch.base.iter().any(|point| !valid_point(*point))
                || !patch.mins.is_finite()
                || !patch.maxs.is_finite()
                || !patch.mins.cmplt(patch.maxs).all()
                || patch.mins.abs().max(patch.maxs.abs()).max_element()
                    > SOURCE_COORDINATE_LIMIT + 1.0
            {
                return Err(Error::InvalidGeometry(
                    "Rope terrain patch identities or base coordinates are invalid".into(),
                ));
            }
        }
        if self.triangles == 0 {
            self.stationary_terrain_clear = true;
            self.patches = patches;
            return Ok(self);
        }
        if self
            .primitives
            .iter()
            .filter_map(|primitive| primitive.terrain.as_ref())
            .any(|terrain| !sources.contains(&terrain.source))
        {
            return Err(Error::InvalidGeometry(
                "Rope collision requires the native base of every terrain patch".into(),
            ));
        }
        self.stationary_terrain_clear = patches.iter().all(|patch| {
            let Some(delta) = stab_delta_bounds(patch) else {
                return false;
            };
            self.primitives
                .iter()
                .filter_map(|primitive| primitive.terrain.as_ref())
                .all(|terrain| {
                    terrain
                        .native_normal
                        .is_some_and(|normal| certifies_departure(normal, delta))
                })
        });
        self.patch_primitives = patches
            .iter()
            .map(|patch| {
                let mut primitives = self
                    .primitives
                    .iter()
                    .enumerate()
                    .filter_map(|(index, primitive)| {
                        primitive
                            .terrain
                            .as_ref()
                            .filter(|terrain| terrain.source == (patch.solid, patch.side))
                            .map(|_| index)
                    })
                    .collect::<Vec<_>>();
                primitives.sort_by_key(|&index| self.primitives[index].order);
                primitives
            })
            .collect();
        self.patches = patches;
        Ok(self)
    }

    pub fn terrain_patches(&self) -> usize {
        self.patches.len()
    }

    pub fn terrain_patch_metadata(&self) -> &[TerrainPatch] {
        &self.patches
    }

    pub fn stationary_topology_required(&self) -> bool {
        self.triangles != 0 && !self.stationary_terrain_clear && self.stationary_topology.is_none()
    }

    pub fn stationary_tree_nodes(&self) -> usize {
        self.stationary_topology
            .as_ref()
            .map_or(0, |topology| topology.nodes.len())
    }

    pub fn stationary_terrain_profile(&self) -> &'static str {
        if self.triangles == 0 {
            "no_active_terrain"
        } else if self.stationary_terrain_clear {
            "certified_all_stab_directions_depart"
        } else if self.stationary_topology.is_some() {
            "prepared_bsp_leaf_stab"
        } else {
            "requires_bsp_leaf_classification"
        }
    }

    pub fn with_stationary_topology(mut self, topology: StationaryTopology) -> Result<Self, Error> {
        if topology.nodes.is_empty() || topology.nodes.len() > MAX_STATIONARY_TREE_NODES {
            return Err(Error::ResourceLimit(
                "Rope stationary topology exceeds its BSP node limit",
            ));
        }
        let mut references = 0usize;
        for (index, node) in topology.nodes.iter().enumerate() {
            if let Some((plane, children)) = node.split {
                checked_plane(plane)?;
                if children
                    .iter()
                    .any(|&child| child <= index || child >= topology.nodes.len())
                    || !node.brushes.is_empty()
                    || !node.patches.is_empty()
                {
                    return Err(Error::InvalidGeometry(
                        "Rope stationary BSP child references are invalid".into(),
                    ));
                }
            }
            if node.brushes.iter().any(|&brush| brush >= self.brushes)
                || node
                    .patches
                    .iter()
                    .any(|&patch| patch >= self.patches.len())
                || node.brushes.windows(2).any(|pair| pair[0] >= pair[1])
                || node.patches.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(Error::InvalidGeometry(
                    "Rope stationary BSP leaf references are invalid".into(),
                ));
            }
            references = references
                .saturating_add(node.brushes.len())
                .saturating_add(node.patches.len());
            if references > MAX_STATIONARY_REFERENCES {
                return Err(Error::ResourceLimit(
                    "Rope stationary BSP leaf references exceed their limit",
                ));
            }
        }
        self.stationary_topology = Some(topology);
        Ok(self)
    }

    pub fn trace(&self, start: DVec3, end: DVec3) -> Result<Trace, Error> {
        self.trace_bounded(start, end, &mut TraceBudget::default())
    }

    pub fn trace_bounded(
        &self,
        start: DVec3,
        end: DVec3,
        budget: &mut TraceBudget,
    ) -> Result<Trace, Error> {
        if !valid_point(start) || !valid_point(end) {
            return Err(Error::InvalidTrace);
        }
        let extent = DVec3::splat(HULL_HALF_EXTENT + CONTACT_EPSILON);
        let swept = Bounds {
            mins: start.min(end) - extent,
            maxs: start.max(end) + extent,
        };
        let mut trace = Trace::clear(end);
        let mut nearest_order = usize::MAX;
        let mut terrain_hit = false;
        let candidates = self
            .primitives
            .partition_point(|primitive| primitive.bounds.mins.x <= swept.maxs.x);
        for terrain in [false, true] {
            if terrain && (self.triangles == 0 || trace.start_solid || trace.fraction == 0.0) {
                break;
            }
            for primitive in &self.primitives[..candidates] {
                budget.consume(1)?;
                if primitive.terrain.is_some() != terrain || !primitive.bounds.overlaps(&swept) {
                    continue;
                }
                let Some(hit) = primitive.trace(start, end, budget)? else {
                    continue;
                };
                trace.start_solid |= hit.start_solid;
                trace.all_solid |= hit.all_solid;
                if hit.fraction < trace.fraction
                    || (hit.fraction == trace.fraction && primitive.order < nearest_order)
                {
                    trace.fraction = hit.fraction;
                    trace.normal = hit.normal;
                    trace.plane_distance = hit.plane_distance;
                    nearest_order = primitive.order;
                    terrain_hit = terrain;
                }
            }
        }
        if terrain_hit && trace.normal.dot(end - start) > 0.0 {
            trace.start_solid = true;
            trace.all_solid = true;
        }
        if start == end
            && self.triangles != 0
            && !trace.start_solid
            && !self.stationary_terrain_clear
        {
            return self.trace_stationary_terrain(start, budget);
        }
        trace.end = start.lerp(end, trace.fraction);
        Ok(trace)
    }

    fn trace_stationary_terrain(
        &self,
        point: DVec3,
        budget: &mut TraceBudget,
    ) -> Result<Trace, Error> {
        let topology = self
            .stationary_topology
            .as_ref()
            .ok_or(Error::UncertifiedStationaryTerrain)?;
        let leaves = topology.query_leaves(point, budget)?;
        let mut open = false;
        for index in leaves {
            budget.consume(1)?;
            let leaf = &topology.nodes[index];
            open |= !leaf.solid;
            let Some(&first) = leaf.patches.first() else {
                continue;
            };
            let mut selected = first;
            for &index in &leaf.patches {
                budget.consume(1)?;
                let patch = &self.patches[index];
                if patch.matches_mask {
                    let center = point.as_vec3();
                    let extent = Vec3::splat(HULL_HALF_EXTENT as f32);
                    if center.cmpge(patch.mins.as_vec3() - extent).all()
                        && center.cmple(patch.maxs.as_vec3() + extent).all()
                    {
                        selected = index;
                        break;
                    }
                }
            }
            let direction = native_stab_direction(&self.patches[selected])
                .ok_or(Error::StationaryNormalization)?;
            let start = point.as_vec3();
            let step = direction * NATIVE_STAB_DISTANCE;
            let end = start + step;
            let delta = Vec3::new(end.x - start.x, end.y - start.y, step.z);
            let (hit, terrain) = self.trace_stab(
                topology,
                start.as_dvec3(),
                end.as_dvec3(),
                delta.as_dvec3(),
                budget,
            )?;
            if terrain && hit.start_solid {
                return Ok(solid_at(point));
            }
        }
        Ok(if open {
            Trace::clear(point)
        } else {
            solid_at(point)
        })
    }

    fn trace_stab(
        &self,
        topology: &StationaryTopology,
        start: DVec3,
        end: DVec3,
        delta: DVec3,
        budget: &mut TraceBudget,
    ) -> Result<(Trace, bool), Error> {
        let maximum =
            SOURCE_COORDINATE_LIMIT + f64::from(NATIVE_STAB_DISTANCE) * NATIVE_SCALE_UPPER + 1.0;
        if !start.is_finite()
            || !end.is_finite()
            || !delta.is_finite()
            || end.abs().max_element() > maximum
        {
            return Err(Error::InvalidTrace);
        }
        let mut state = StabState {
            start,
            end,
            delta,
            trace: Trace::clear(end),
            terrain_hit: false,
            brushes: vec![false; self.brushes],
            patches: vec![false; self.patches.len()],
        };
        let mut pending = vec![(0usize, 0.0f32, 1.0f32, start.as_vec3(), end.as_vec3())];
        while let Some((mut index, first_fraction, last_fraction, first, last)) = pending.pop() {
            if state.trace.fraction <= f64::from(first_fraction) {
                continue;
            }
            loop {
                budget.consume(1)?;
                let node = &topology.nodes[index];
                let Some((plane, children)) = node.split else {
                    self.trace_stab_leaf(node, &mut state, budget)?;
                    break;
                };
                let first_distance = node_distance(plane, first.as_dvec3()) as f32;
                let last_distance = node_distance(plane, last.as_dvec3()) as f32;
                let extent = node_extent(plane, HULL_HALF_EXTENT) as f32;
                if first_distance > extent && last_distance > extent {
                    index = children[0];
                    continue;
                }
                if first_distance < -extent && last_distance < -extent {
                    index = children[1];
                    continue;
                }
                let (side, near, far) = if first_distance < last_distance {
                    let inverse = (first_distance - last_distance).recip();
                    (
                        1,
                        (first_distance - extent - CONTACT_EPSILON as f32) * inverse,
                        (first_distance + extent + CONTACT_EPSILON as f32) * inverse,
                    )
                } else if first_distance > last_distance {
                    let inverse = (first_distance - last_distance).recip();
                    (
                        0,
                        (first_distance + extent + CONTACT_EPSILON as f32) * inverse,
                        (first_distance - extent - CONTACT_EPSILON as f32) * inverse,
                    )
                } else {
                    (0, 1.0, 0.0)
                };
                let near = near.clamp(0.0, 1.0);
                let far = far.clamp(0.0, 1.0);
                let near_fraction = first_fraction + (last_fraction - first_fraction) * near;
                let far_fraction = first_fraction + (last_fraction - first_fraction) * far;
                pending.push((
                    children[side ^ 1],
                    far_fraction,
                    last_fraction,
                    first + (last - first) * far,
                    last,
                ));
                pending.push((
                    children[side],
                    first_fraction,
                    near_fraction,
                    first,
                    first + (last - first) * near,
                ));
                if pending.len() > MAX_STATIONARY_TREE_NODES {
                    return Err(Error::ResourceLimit(
                        "Rope stationary BSP sweep exceeded its pending limit",
                    ));
                }
                break;
            }
        }
        state.trace.end = start.lerp(end, state.trace.fraction);
        Ok((state.trace, state.terrain_hit))
    }

    fn trace_stab_leaf(
        &self,
        node: &StationaryNode,
        state: &mut StabState,
        budget: &mut TraceBudget,
    ) -> Result<(), Error> {
        let StabState {
            start,
            end,
            delta,
            trace,
            terrain_hit,
            brushes,
            patches,
        } = state;
        for &brush in &node.brushes {
            budget.consume(1)?;
            if std::mem::replace(&mut brushes[brush], true) {
                continue;
            }
            let primitive = &self.primitives[self.primitive_indices[brush]];
            if let Some(hit) = primitive.trace(*start, *end, budget)? {
                trace.start_solid |= hit.start_solid;
                trace.all_solid |= hit.all_solid;
                if hit.fraction < trace.fraction {
                    *terrain_hit = false;
                    trace.fraction = hit.fraction;
                    trace.normal = hit.normal;
                    trace.plane_distance = hit.plane_distance;
                }
            }
            if trace.fraction == 0.0 {
                return Ok(());
            }
        }
        if trace.start_solid {
            return Ok(());
        }
        let terrain_end = (start.as_vec3() + delta.as_vec3()).as_dvec3();
        for &patch in &node.patches {
            budget.consume(1)?;
            if std::mem::replace(&mut patches[patch], true) || !self.patches[patch].matches_mask {
                continue;
            }
            for &index in &self.patch_primitives[patch] {
                let primitive = &self.primitives[index];
                if let Some(hit) = primitive.trace_terrain(
                    *start,
                    terrain_end,
                    primitive.terrain.as_ref().unwrap().normal,
                    budget,
                )? {
                    if *terrain_hit
                        && hit.fraction == trace.fraction
                        && (hit.normal.dot(*delta) > 0.0) != (trace.normal.dot(*delta) > 0.0)
                    {
                        return Err(Error::AmbiguousStationaryContact);
                    }
                    if hit.fraction < trace.fraction {
                        *terrain_hit = true;
                        trace.fraction = hit.fraction;
                        trace.normal = hit.normal;
                        trace.plane_distance = hit.plane_distance;
                    }
                }
            }
            if trace.fraction == 0.0 {
                break;
            }
        }
        if *terrain_hit && trace.normal.dot(*delta) > 0.0 {
            trace.start_solid = true;
            trace.all_solid = true;
        }
        Ok(())
    }
}

impl StationaryTopology {
    fn query_leaves(&self, center: DVec3, budget: &mut TraceBudget) -> Result<Vec<usize>, Error> {
        let mut leaves = Vec::new();
        let mut pending = VecDeque::new();
        let mut index = 0;
        loop {
            budget.consume(1)?;
            if let Some((plane, children)) = self.nodes[index].split {
                let distance = node_distance(plane, center);
                let extent = node_extent(plane, HULL_HALF_EXTENT + 1.0);
                if distance >= extent {
                    index = children[0];
                } else if distance < -extent {
                    index = children[1];
                } else {
                    pending.push_back(children[0]);
                    if pending.len() > MAX_STATIONARY_QUERY_LEAVES {
                        return Err(Error::ResourceLimit(
                            "Rope stationary query exceeded its pending leaf limit",
                        ));
                    }
                    index = children[1];
                }
            } else {
                leaves.push(index);
                if leaves.len() > MAX_STATIONARY_QUERY_LEAVES {
                    return Err(Error::ResourceLimit(
                        "Rope stationary query exceeded 1024 BSP leaves",
                    ));
                }
                let Some(next) = pending.pop_front() else {
                    return Ok(leaves);
                };
                index = next;
            }
        }
    }
}

impl Primitive {
    fn trace(
        &self,
        start: DVec3,
        end: DVec3,
        budget: &mut TraceBudget,
    ) -> Result<Option<Trace>, Error> {
        if let Some(terrain) = &self.terrain {
            if start == end {
                return terrain.overlap(start, budget);
            }
            return self.trace_terrain(start, end, terrain.normal, budget);
        }
        let mut enter = f64::NEG_INFINITY;
        let mut leave = 1.0f64;
        let mut starts_outside = false;
        let mut ends_outside = false;
        let mut contact = None;
        for &plane in &self.planes {
            budget.consume(1)?;
            let distance = plane.dist + HULL_HALF_EXTENT * plane.normal.abs().element_sum();
            let first = plane.normal.dot(start) - distance;
            let second = plane.normal.dot(end) - distance;
            starts_outside |= first > 0.0;
            ends_outside |= second > 0.0;
            if first > 0.0 && second > 0.0 {
                return Ok(None);
            }
            if first <= 0.0 && second <= 0.0 {
                continue;
            }
            if first > second {
                let fraction = (first - CONTACT_EPSILON).max(0.0) / (first - second);
                if fraction > enter {
                    enter = fraction;
                    contact = Some(plane);
                }
            } else {
                let fraction = (first + CONTACT_EPSILON) / (first - second);
                leave = leave.min(fraction);
            }
            if enter > leave {
                return Ok(None);
            }
        }
        if !starts_outside {
            let mut trace = Trace::clear(end);
            trace.start_solid = true;
            trace.all_solid = !ends_outside;
            if trace.all_solid {
                trace.fraction = 0.0;
                trace.end = start;
            }
            return Ok(Some(trace));
        }
        if enter < 0.0 || enter >= leave || enter >= 1.0 {
            return Ok(None);
        }
        let Some(plane) = contact else {
            return Ok(None);
        };
        Ok(Some(Trace {
            fraction: enter,
            start_solid: false,
            all_solid: false,
            normal: plane.normal,
            plane_distance: plane.dist,
            end: start.lerp(end, enter),
        }))
    }

    fn trace_terrain(
        &self,
        start: DVec3,
        end: DVec3,
        normal: DVec3,
        budget: &mut TraceBudget,
    ) -> Result<Option<Trace>, Error> {
        if normal.dot(end - start) > CONTACT_EPSILON {
            return Ok(None);
        }
        let mut enter = f64::NEG_INFINITY;
        let mut leave = 1.0f64;
        let mut contact = None;
        for &plane in &self.planes {
            budget.consume(1)?;
            let distance = plane.dist + HULL_HALF_EXTENT * plane.normal.abs().element_sum();
            let first = plane.normal.dot(start) - distance;
            let second = plane.normal.dot(end) - distance;
            if first > 0.0 && second > 0.0 {
                return Ok(None);
            }
            if first < 0.0 && second < 0.0 {
                continue;
            }
            let denominator = first - second;
            if first >= 0.0 && second <= 0.0 {
                let fraction = if denominator == 0.0 {
                    0.0
                } else {
                    (first - CONTACT_EPSILON) / denominator
                };
                if fraction > enter {
                    enter = fraction;
                    contact = Some(plane);
                }
            } else {
                let fraction = if denominator == 0.0 {
                    0.0
                } else {
                    (first + CONTACT_EPSILON) / denominator
                };
                leave = leave.min(fraction);
            }
        }
        if !(enter < leave || (enter - leave).abs() < TERRAIN_FRACTION_TOLERANCE) || enter >= 1.0 {
            return Ok(None);
        }
        let Some(plane) = contact else {
            return Ok(None);
        };
        let fraction = enter.max(0.0);
        Ok(Some(Trace {
            fraction,
            start_solid: false,
            all_solid: false,
            normal: plane.normal,
            plane_distance: plane.dist,
            end: start.lerp(end, fraction),
        }))
    }
}

impl Terrain {
    fn overlap(&self, center: DVec3, budget: &mut TraceBudget) -> Result<Option<Trace>, Error> {
        for axis in [DVec3::X, DVec3::Y, DVec3::Z, self.normal] {
            if !self.overlaps_axis(center, axis, budget)? {
                return Ok(None);
            }
        }
        let [a, b, c] = self.positions;
        for edge in [b - a, c - b, a - c] {
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                if !self.overlaps_axis(center, edge.cross(axis), budget)? {
                    return Ok(None);
                }
            }
        }
        Ok(Some(Trace {
            fraction: 0.0,
            start_solid: true,
            all_solid: true,
            normal: DVec3::ZERO,
            plane_distance: 0.0,
            end: center,
        }))
    }

    fn overlaps_axis(
        &self,
        center: DVec3,
        axis: DVec3,
        budget: &mut TraceBudget,
    ) -> Result<bool, Error> {
        budget.consume(1)?;
        let [min, max] = self
            .positions
            .map(|point| axis.dot(point - center))
            .into_iter()
            .fold(
                [f64::INFINITY, f64::NEG_INFINITY],
                |[min, max], position| [min.min(position), max.max(position)],
            );
        let extent = HULL_HALF_EXTENT * axis.abs().element_sum();
        Ok(min <= extent && max >= -extent)
    }
}
