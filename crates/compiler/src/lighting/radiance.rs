use glam::{DVec2, DVec3};
use source_formats::bsp::{Bsp, Edge, LightingKind, Model, RgbExp32, TexInfo, surf};
use std::cell::Cell;
use std::sync::Arc;

use super::Occluder;
use super::bake::Budget;
use super::shadows::{ShadowIndex, ShadowObject, Triangle, Visit};
use super::terrain_shadows::TerrainTriangle;
use super::transport::{Coverage, Scene, SourceTarget};
use crate::brush_model_pose::Pose;
use crate::map::Map;

const LOCAL_FILTER_START: f64 = 20.0;
const LOCAL_FILTER_END: f64 = 40.0;
const SOURCE_ROUNDING_OPERATIONS: f64 = 64.0;
const SOURCE_INTERSECTION_WORK: usize = 48;
const SOURCE_TRANSPORT_WORK: usize = 24;
const SOURCE_SAMPLE_WORK: usize = 30;
const SOURCE_PLANE_WORK: usize = 8;
const SOURCE_MEMBERSHIP_WORK: usize = 24;

struct Surface {
    geometry: Geometry,
    mins: DVec3,
    maxs: DVec3,
    normal: DVec3,
    source: usize,
    ordinal: usize,
    coordinates: Option<[DVec2; 3]>,
    boundary_error: f64,
}

struct Frame {
    entity: usize,
    pose: Option<Pose>,
}

impl Frame {
    fn transform_work(&self) -> usize {
        self.pose
            .map_or(0, |pose| if pose.rotation.is_some() { 18 } else { 3 })
    }

    fn point(&self, point: DVec3) -> DVec3 {
        self.pose.map_or(point, |pose| pose.point(point))
    }

    fn local_point(&self, point: DVec3) -> DVec3 {
        self.pose.map_or(point, |pose| pose.local_point(point))
    }

    fn normal(&self, normal: DVec3) -> DVec3 {
        self.pose.map_or(normal, |pose| pose.normal(normal))
    }
}

struct BrushFan {
    triangles: Box<[Triangle]>,
    frame: Arc<Frame>,
}

enum Geometry {
    Brush { fan: Arc<BrushFan>, index: usize },
    Terrain(TerrainTriangle),
}

impl Geometry {
    fn intersection(
        &self,
        start: DVec3,
        end: DVec3,
        pending: &Cell<usize>,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        match self {
            Self::Brush { fan, index } => {
                let start = fan.frame.local_point(start);
                let end = fan.frame.local_point(end);
                let Some(hit) = fan.triangles[*index].topological_intersection(start, end, 0.0)?
                else {
                    return Ok(None);
                };
                if hit.1[2] == 0.0 {
                    let central = hit.1[1] == 0.0;
                    let first = if central { 0 } else { index.saturating_sub(1) };
                    for previous in &fan.triangles[first..*index] {
                        pending.set(
                            pending
                                .get()
                                .checked_add(SOURCE_INTERSECTION_WORK)
                                .ok_or("Radiance fan ownership work overflow")?,
                        );
                        if previous
                            .topological_intersection(start, end, 0.0)?
                            .is_some_and(|(_, weights)| {
                                weights[1] == 0.0 && (!central || weights[2] == 0.0)
                            })
                        {
                            return Ok(None);
                        }
                    }
                }
                Ok(Some(hit))
            }
            Self::Terrain(triangle) => triangle.source_intersection(start, end),
        }
    }

    fn point(&self, weights: [f64; 3]) -> DVec3 {
        match self {
            Self::Brush { fan, index } => fan.triangles[*index].point(weights),
            Self::Terrain(triangle) => triangle.point(weights),
        }
    }

    fn texture_point(&self, weights: [f64; 3]) -> DVec3 {
        match self {
            Self::Brush { fan, index } => fan.triangles[*index].point(weights),
            Self::Terrain(triangle) => triangle.texture_point(weights),
        }
    }
}

struct Radiance {
    frame: Arc<Frame>,
    average: DVec3,
    reflectivity: DVec3,
    samples: Vec<RgbExp32>,
    dimensions: [usize; 2],
    projection: [[f32; 4]; 2],
    mins: [i32; 2],
    sky: bool,
    extras: Vec<Layer>,
    boundary: Option<SourceBoundary>,
    alpha: Option<super::brush_alpha::Face>,
    casters: Vec<CasterBinding>,
    contributors: Vec<Contributor>,
}

struct SourceSide {
    id: u32,
    owner: usize,
    plane: usize,
    footprint: Option<Arc<Footprint>>,
}

struct Contributor {
    side: u32,
    owner: usize,
    caster: bool,
    footprint: Arc<Footprint>,
}

struct Footprint {
    plane: map_core::Plane,
    axes: [usize; 2],
    points: Box<[DVec2]>,
}

impl Footprint {
    fn contains(&self, point: DVec3, error: f64) -> Result<bool, String> {
        let distance = self.plane.distance_to(point);
        let scale = (self.plane.normal * point).abs().element_sum() + self.plane.dist.abs() + 1.0;
        let allowance = error + scale * (SOURCE_ROUNDING_OPERATIONS * f64::EPSILON);
        if !distance.is_finite() || !allowance.is_finite() {
            return Err("Native source attribution arithmetic must be finite".into());
        }
        if distance.abs() > allowance {
            return Ok(false);
        }
        let point = DVec2::new(point[self.axes[0]], point[self.axes[1]]);
        let mut inside = false;
        for (&a, &b) in self.points.iter().zip(self.points.iter().cycle().skip(1)) {
            let side = (a - point).perp_dot(b - point);
            if side == 0.0 && point.cmpge(a.min(b)).all() && point.cmple(a.max(b)).all() {
                return Ok(true);
            }
            if (a.y > point.y) != (b.y > point.y) && ((side > 0.0) == (b.y > a.y)) {
                inside = !inside;
            }
        }
        Ok(inside)
    }
}

fn prepare_footprint(
    map: &Map<'_>,
    source: &SourceSide,
    allocation: &mut super::bake::Allocation,
    budget: &mut Budget<'_>,
) -> Result<Footprint, String> {
    let brush = &map.brushes[source.owner];
    budget.consume(brush.sides.len())?;
    let side = brush
        .sides
        .iter()
        .find(|side| side.source_id == Some(source.id))
        .ok_or("Native source attribution lost its original side")?;
    let winding = side
        .winding
        .as_ref()
        .ok_or("Native source attribution needs its original finite winding")?;
    if winding.0.len() < 3 {
        return Err(format!(
            "Native source attribution side:{} has no finite polygon",
            source.id
        ));
    }
    let plane = map.planes.get(source.plane);
    let plane = map_core::Plane {
        normal: plane.normal,
        dist: plane.dist,
    };
    let normal = plane.normal.abs();
    let dropped = if normal.x >= normal.y && normal.x >= normal.z {
        0
    } else if normal.y >= normal.z {
        1
    } else {
        2
    };
    let axes = [(dropped + 1) % 3, (dropped + 2) % 3];
    let bytes = winding
        .0
        .len()
        .checked_mul(size_of::<DVec2>())
        .and_then(|bytes| bytes.checked_add(size_of::<Footprint>() + 2 * size_of::<usize>()))
        .ok_or("Native source attribution allocation overflow")?;
    allocation.grow(budget, bytes)?;
    budget.consume(
        winding
            .0
            .len()
            .checked_mul(SOURCE_MEMBERSHIP_WORK)
            .ok_or("Native source attribution work overflow")?,
    )?;
    let mut points = Vec::with_capacity(winding.0.len());
    for &point in &winding.0 {
        let point = crate::emit::stored_vertex(point);
        if !point.is_finite() {
            return Err("Native source attribution vertices must be finite".into());
        }
        points.push(DVec2::new(point[axes[0]], point[axes[1]]));
    }
    let origin = points[0];
    let area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(&a, &b)| (a - origin).perp_dot(b - origin))
        .sum::<f64>();
    if !area.is_finite() || area == 0.0 {
        return Err(format!(
            "Native source attribution side:{} has a collapsed polygon",
            source.id
        ));
    }
    Ok(Footprint {
        plane,
        axes,
        points: points.into_boxed_slice(),
    })
}

#[derive(Clone, Copy)]
struct Membership<'a> {
    contributors: &'a [Contributor],
    point: DVec3,
    error: f64,
}

impl Membership<'_> {
    fn allows(&self, owner: usize, side: u32) -> Result<(bool, usize), String> {
        if self.contributors.is_empty() {
            return Ok((true, 0));
        }
        let mut work = self.contributors.len().checked_ilog2().unwrap_or(0) as usize + 1;
        let Ok(index) = self
            .contributors
            .binary_search_by_key(&side, |contributor| contributor.side)
        else {
            return Ok((false, work));
        };
        let contributor = &self.contributors[index];
        if contributor.owner != owner {
            return Ok((false, work));
        }
        work += contributor.footprint.points.len() * SOURCE_MEMBERSHIP_WORK;
        Ok((
            contributor.footprint.contains(self.point, self.error)?,
            work,
        ))
    }
}

struct CasterBinding {
    slot: usize,
    owner: usize,
}

struct Ray<'a> {
    start: DVec3,
    end: DVec3,
    scene: &'a Scene<'a>,
    skip_owner: Option<u32>,
    terminal: Option<f64>,
}

struct HullEvent<'a> {
    hull: &'a Occluder,
    interval: super::HullInterval,
    entering: bool,
    point: DVec3,
    boundary: usize,
    value: super::HullBoundary,
}

pub(super) struct SourceBoundary {
    pub(super) plane: usize,
    pub(super) equation: map_core::Plane,
    pub(super) sides: Arc<[u32]>,
    pub(super) owners: Vec<usize>,
}

impl SourceBoundary {
    #[allow(clippy::too_many_arguments)]
    fn crossing(
        &self,
        hull: &Occluder,
        interval: super::HullInterval,
        start: DVec3,
        end: DVec3,
        point: DVec3,
        error: f64,
        membership: Membership<'_>,
    ) -> Result<(Option<(bool, DVec3)>, usize), String> {
        let Some(source) = &hull.source else {
            return Ok((None, 1));
        };
        let mut work = self.owners.len().checked_ilog2().unwrap_or(0) as usize + 1;
        if self.owners.binary_search(&source.brush).is_err() {
            return Ok((None, work));
        }
        for (identity, plane) in source.sides.iter().zip(&hull.planes) {
            work += self.sides.len().checked_ilog2().unwrap_or(0) as usize + 2;
            if identity.plane != self.plane
                || !identity
                    .side
                    .is_some_and(|side| self.sides.binary_search(&side).is_ok())
                || plane.normal != self.equation.normal
                || plane.dist != self.equation.dist
            {
                continue;
            }
            let side = identity
                .side
                .ok_or("Radiance source side identity is missing")?;
            let (allowed, membership_work) = membership.allows(source.brush, side)?;
            work += membership_work;
            if !allowed {
                continue;
            }
            work += 24;
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            let distance = plane.distance_to(point);
            let scale = (plane.normal * point).abs().element_sum() + plane.dist.abs() + 1.0;
            let allowance = error + scale * (SOURCE_ROUNDING_OPERATIONS * f64::EPSILON);
            if !a.is_finite() || !b.is_finite() || !distance.is_finite() || !allowance.is_finite() {
                return Err("Radiance source crossing arithmetic must be finite".into());
            }
            if (a > 0.0) == (b > 0.0) || distance.abs() > allowance {
                continue;
            }
            let entering = a > 0.0;
            let fraction = a / (a - b);
            let boundary = if entering {
                interval.entering
            } else {
                interval.leaving
            };
            if fraction > 0.0
                && fraction < 1.0
                && (fraction - boundary).abs() <= super::shadows::TRIANGLE_EPSILON
            {
                return Ok((Some((entering, start.lerp(end, fraction))), work));
            }
        }
        Ok((None, work))
    }

    fn terminal(
        &self,
        hull: &Occluder,
        interval: super::HullInterval,
        start: DVec3,
        end: DVec3,
        error: f64,
    ) -> Result<(bool, usize), String> {
        let Some(source) = &hull.source else {
            return Ok((false, 1));
        };
        let mut work = self.owners.len().checked_ilog2().unwrap_or(0) as usize + 1;
        if interval.entering <= 0.0
            || interval.leaving <= interval.entering
            || self.owners.binary_search(&source.brush).is_err()
        {
            return Ok((false, work));
        }
        for (identity, plane) in source.sides.iter().zip(&hull.planes) {
            work += self.sides.len().checked_ilog2().unwrap_or(0) as usize + 2;
            if identity.plane != self.plane
                || !identity
                    .side
                    .is_some_and(|side| self.sides.binary_search(&side).is_ok())
            {
                continue;
            }
            if plane.normal != self.equation.normal || plane.dist != self.equation.dist {
                continue;
            }
            let a = plane.distance_to(start);
            let b = plane.distance_to(end);
            let scale = (plane.normal * end).abs().element_sum() + plane.dist.abs() + 1.0;
            let allowance = error + scale * (SOURCE_ROUNDING_OPERATIONS * f64::EPSILON);
            if !a.is_finite() || !b.is_finite() || !allowance.is_finite() {
                return Err("Radiance source boundary arithmetic must be finite".into());
            }
            if a > 0.0 && b <= 0.0 && b.abs() <= allowance {
                return Ok((true, work));
            }
        }
        Ok((false, work))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn source_coverage(
    shadows: &ShadowIndex<Occluder>,
    start: DVec3,
    end: DVec3,
    boundary: Option<&SourceBoundary>,
    error: f64,
    coverage: &mut Coverage,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let pending = Cell::new(0usize);
    let result = shadows.visit(
        start,
        end,
        &mut |work| {
            budget.consume(
                work.checked_add(pending.replace(0))
                    .ok_or("Radiance source certificate work overflow")?,
            )
        },
        &mut |hull| {
            let Some(interval) = hull.interval(start, end)? else {
                return Ok(Visit::Continue(1.0));
            };
            pending.set(
                pending
                    .get()
                    .checked_add(hull.plane_count() + 1)
                    .ok_or("Radiance source interval work overflow")?,
            );
            let terminal = if let Some(boundary) = boundary {
                let (terminal, work) = boundary.terminal(hull, interval, start, end, error)?;
                pending.set(
                    pending
                        .get()
                        .checked_add(work)
                        .ok_or("Radiance source certificate work overflow")?,
                );
                terminal
            } else {
                false
            };
            for event in hull.optical_boundaries(start, end)?.into_iter().flatten() {
                if terminal && event.fraction == interval.entering {
                    continue;
                }
                coverage.add(event.coverage);
            }
            Ok(if coverage.opaque() {
                Visit::Stop
            } else {
                Visit::Continue(1.0)
            })
        },
    );
    budget.consume(pending.replace(0))?;
    result.map(|_| ())
}

struct Layer {
    style: u8,
    average: DVec3,
    samples: Vec<RgbExp32>,
}

impl Radiance {
    fn at(&self, point: DVec3, uv: Option<DVec2>, distance: f64, cone: f64) -> DVec3 {
        self.at_layer(&self.samples, self.average, point, uv, distance, cone)
    }

    fn at_layer(
        &self,
        samples: &[RgbExp32],
        average: DVec3,
        point: DVec3,
        uv: Option<DVec2>,
        distance: f64,
        cone: f64,
    ) -> DVec3 {
        let blend = ((distance * cone - LOCAL_FILTER_START)
            / (LOCAL_FILTER_END - LOCAL_FILTER_START))
            .clamp(0.0, 1.0);
        if samples.is_empty() || blend == 1.0 {
            return average;
        }
        let coordinates: [f64; 2] = std::array::from_fn(|axis| {
            let coordinate = if let Some(uv) = uv {
                uv[axis] * (self.dimensions[axis] - 1) as f64
            } else {
                let row = self.projection[axis];
                DVec3::from_array([row[0], row[1], row[2]].map(f64::from)).dot(point)
                    + f64::from(row[3])
                    - f64::from(self.mins[axis])
            };
            coordinate.clamp(0.0, (self.dimensions[axis] - 1) as f64)
        });
        let lower = coordinates.map(|value| value.floor() as usize);
        let upper: [usize; 2] =
            std::array::from_fn(|axis| (lower[axis] + 1).min(self.dimensions[axis] - 1));
        let color =
            |x: usize, y: usize| DVec3::from_array(samples[y * self.dimensions[0] + x].linear());
        let weight = [
            coordinates[0] - lower[0] as f64,
            coordinates[1] - lower[1] as f64,
        ];
        let near = color(lower[0], lower[1]).lerp(color(upper[0], lower[1]), weight[0]);
        let far = color(lower[0], upper[1]).lerp(color(upper[0], upper[1]), weight[0]);
        (near.lerp(far, weight[1]) * self.reflectivity).lerp(average, blend)
    }
}

pub(super) struct SurfaceIndex {
    geometry: ShadowIndex<Surface>,
    radiance: Vec<Radiance>,
    crossings: usize,
    shadow_count: usize,
    fractional_sources: bool,
    sheet_sources: bool,
    _allocation: super::bake::Allocation,
}

impl ShadowObject for Surface {
    const MAX_OBJECTS: usize = Triangle::MAX_OBJECTS;
    fn bounds(&self) -> (DVec3, DVec3) {
        (self.mins, self.maxs)
    }
    fn work(&self) -> usize {
        match &self.geometry {
            Geometry::Brush { fan, .. } => {
                SOURCE_INTERSECTION_WORK + 32 + 3 * fan.frame.transform_work()
            }
            Geometry::Terrain(triangle) => triangle.work() + 32,
        }
    }
    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        Ok(self
            .geometry
            .intersection(start, end, &Cell::new(0))?
            .is_some())
    }
}

#[allow(clippy::too_many_arguments)]
fn add_surface(
    surfaces: &mut Vec<Surface>,
    points: [DVec3; 3],
    source: usize,
    coordinates: Option<[DVec2; 3]>,
    boundary: Option<&SourceBoundary>,
    geometry: Geometry,
    frame: &Frame,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    budget.consume(1)?;
    if (points[1] - points[0])
        .cross(points[2] - points[0])
        .length_squared()
        == 0.0
    {
        return Ok(());
    }
    if surfaces.len() == Surface::MAX_OBJECTS {
        return Err("Surface radiance surface count exceeds the bounded triangle limit".into());
    }
    budget.consume(SOURCE_INTERSECTION_WORK + 4 * frame.transform_work())?;
    let world = points.map(|point| frame.point(point));
    if world.iter().any(|point| {
        !point.is_finite() || point.abs().max_element() > crate::map::MAX_COMPILE_COORD
    }) {
        return Err("Radiance source geometry exceeds finite Source coordinate bounds".into());
    }
    let (mins, maxs) = Triangle::new(world)?.bounds();
    let boundary_error = boundary.map_or(0.0, |boundary| {
        world
            .iter()
            .map(|&point| boundary.equation.distance_to(point).abs())
            .fold(0.0_f64, f64::max)
    });
    if !boundary_error.is_finite() {
        return Err("Radiance native source support must be finite".into());
    }
    surfaces.push(Surface {
        geometry,
        mins,
        maxs,
        normal: frame.normal(
            (points[0] - points[1])
                .cross(points[2] - points[1])
                .normalize(),
        ),
        source,
        ordinal: surfaces.len(),
        coordinates,
        boundary_error,
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    map: &Map<'_>,
    bsp: &Bsp,
    model_scopes: &[super::models::Scope],
    kind: LightingKind,
    sky_radiance: DVec3,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain: &ShadowIndex<TerrainTriangle>,
    shadows: &ShadowIndex<Occluder>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(
        map,
        bsp,
        model_scopes,
        kind,
        sky_radiance,
        false,
        face_sources,
        brush_textures,
        terrain,
        shadows,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_styled(
    map: &Map<'_>,
    bsp: &Bsp,
    model_scopes: &[super::models::Scope],
    kind: LightingKind,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain: &ShadowIndex<TerrainTriangle>,
    shadows: &ShadowIndex<Occluder>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(
        map,
        bsp,
        model_scopes,
        kind,
        DVec3::ZERO,
        true,
        face_sources,
        brush_textures,
        terrain,
        shadows,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_styled_sky(
    map: &Map<'_>,
    bsp: &Bsp,
    model_scopes: &[super::models::Scope],
    kind: LightingKind,
    sky_radiance: DVec3,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain: &ShadowIndex<TerrainTriangle>,
    shadows: &ShadowIndex<Occluder>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    build_with_styles(
        map,
        bsp,
        model_scopes,
        kind,
        sky_radiance,
        true,
        face_sources,
        brush_textures,
        terrain,
        shadows,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_with_styles(
    map: &Map<'_>,
    bsp: &Bsp,
    model_scopes: &[super::models::Scope],
    kind: LightingKind,
    sky_radiance: DVec3,
    styled: bool,
    face_sources: &[Arc<[u32]>],
    brush_textures: Option<&super::brush_alpha::Prepared>,
    terrain_shadows: &ShadowIndex<TerrainTriangle>,
    shadows: &ShadowIndex<Occluder>,
    budget: &mut Budget<'_>,
) -> Result<SurfaceIndex, String> {
    let decoded_bytes = [
        source_formats::bsp::lump::FACES,
        source_formats::bsp::lump::FACES_HDR,
        source_formats::bsp::lump::TEXINFO,
        source_formats::bsp::lump::VERTEXES,
        source_formats::bsp::lump::EDGES,
        source_formats::bsp::lump::SURFEDGES,
        source_formats::bsp::lump::MODELS,
    ]
    .into_iter()
    .try_fold(0usize, |total, index| {
        total.checked_add(bsp.lumps[index].data.len())
    })
    .ok_or("Radiance geometry allocation size overflow")?;
    let _decoded_allocation = budget.reserve_bytes(decoded_bytes)?;
    let models = bsp.records::<Model>().map_err(|error| error.to_string())?;
    let faces = bsp
        .lighting_faces(kind)
        .map_err(|error| error.to_string())?;
    if face_sources.len() != faces.len() {
        return Err("Radiance contributors do not match the emitted face count".into());
    }
    if models.len() != model_scopes.len()
        || model_scopes
            .first()
            .is_none_or(|scope| scope.entity != 0 || !scope.reflected_source)
    {
        return Err("Radiance source scopes do not match the emitted models".into());
    }
    for (model, scope) in models.iter().zip(model_scopes) {
        budget.consume(1)?;
        let first = usize::try_from(model.first_face)
            .map_err(|_| "Radiance model has a negative first face")?;
        let count = usize::try_from(model.face_count)
            .map_err(|_| "Radiance model has a negative face count")?;
        let end = first
            .checked_add(count)
            .filter(|&end| end <= faces.len())
            .ok_or("Radiance model exceeds emitted face bounds")?;
        if scope.faces != (first..end) || scope.entity >= map.entities.len() {
            return Err("Radiance source scope lost its final face or entity binding".into());
        }
    }
    let infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    let lighting = bsp.lighting(kind).map_err(|error| error.to_string())?;
    let _caster_lookup = budget.reserve::<(usize, usize)>(shadows.objects().len())?;
    budget.consume(shadows.objects().len())?;
    let mut caster_lookup = Vec::with_capacity(shadows.objects().len());
    for (slot, hull) in shadows.objects().iter().enumerate() {
        if let Some(source) = &hull.source {
            if source.brush >= map.brushes.len() {
                return Err("Radiance global caster has no original brush owner".into());
            }
            caster_lookup.push((source.brush, slot));
        }
    }
    budget.consume(
        caster_lookup
            .len()
            .checked_mul(caster_lookup.len().checked_ilog2().unwrap_or(0) as usize + 1)
            .ok_or("Radiance caster classification work overflow")?,
    )?;
    caster_lookup.sort_unstable_by_key(|&(owner, _)| owner);
    if caster_lookup.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("Radiance global caster ownership is ambiguous".into());
    }
    let source_faces = || {
        model_scopes
            .iter()
            .filter(|scope| scope.reflected_source)
            .flat_map(|scope| faces[scope.faces.clone()].iter())
    };
    let source_face_count = source_faces().count();
    let source_scope_count = model_scopes
        .iter()
        .filter(|scope| scope.reflected_source)
        .count();
    budget.consume(source_face_count)?;
    let source_side_count = model_scopes
        .iter()
        .filter(|scope| scope.reflected_source)
        .flat_map(|scope| map.entities[scope.entity].brushes.iter())
        .try_fold(0usize, |count, &index| {
            count.checked_add(map.brushes[index].sides.len())
        })
        .ok_or("Radiance source-side lookup size overflow")?;
    let _source_lookup = budget.reserve::<SourceSide>(source_side_count)?;
    budget.consume(source_side_count)?;
    let mut source_owners = Vec::with_capacity(source_side_count);
    for scope in model_scopes.iter().filter(|scope| scope.reflected_source) {
        for &owner in &map.entities[scope.entity].brushes {
            if map.brushes[owner].entity != scope.entity {
                return Err("Radiance source brush lost its final model ownership".into());
            }
            for side in &map.brushes[owner].sides {
                if !side.bevel
                    && let Some(id) = side.source_id
                {
                    source_owners.push(SourceSide {
                        id,
                        owner,
                        plane: side.plane,
                        footprint: None,
                    });
                }
            }
        }
    }
    budget.consume(
        source_owners
            .len()
            .checked_mul(source_owners.len().checked_ilog2().unwrap_or(0) as usize + 1)
            .ok_or("Radiance source-side sort work overflow")?,
    )?;
    source_owners.sort_unstable_by_key(|source| source.id);
    if source_owners
        .windows(2)
        .any(|pair| pair[0].id == pair[1].id)
    {
        return Err("Radiance source-side ownership is ambiguous".into());
    }
    let contributor_count = model_scopes
        .iter()
        .filter(|scope| scope.reflected_source)
        .flat_map(|scope| face_sources[scope.faces.clone()].iter())
        .try_fold(0usize, |count, sides| count.checked_add(sides.len()))
        .ok_or("Radiance contributor allocation size overflow")?;
    let triangles = source_faces()
        .try_fold(0usize, |total, face| {
            let count = if face.dispinfo >= 0 {
                map.terrains[face.dispinfo as usize].indices.len()
            } else {
                (face.edge_count as usize).saturating_sub(2)
            };
            total
                .checked_add(count)
                .filter(|&total| total <= Surface::MAX_OBJECTS)
        })
        .ok_or("Surface radiance exceeds its bounded triangle allocation")?;
    let brush_triangles = source_faces()
        .filter(|face| face.dispinfo < 0)
        .try_fold(0usize, |count, face| {
            count.checked_add((face.edge_count as usize).saturating_sub(2))
        })
        .ok_or("Radiance brush fan allocation size overflow")?;
    let bytes = ShadowIndex::<Surface>::allocation_bytes(triangles)?
        .checked_add(brush_triangles * size_of::<Triangle>())
        .and_then(|bytes| {
            bytes.checked_add(source_face_count * (size_of::<BrushFan>() + 2 * size_of::<usize>()))
        })
        .and_then(|bytes| {
            bytes.checked_add(source_scope_count * (size_of::<Frame>() + 2 * size_of::<usize>()))
        })
        .and_then(|bytes| {
            bytes.checked_add(source_face_count * (size_of::<Radiance>() + 3 * size_of::<Layer>()))
        })
        .and_then(|bytes| bytes.checked_add(source_formats::bsp::MAX_LIGHTING_BYTES))
        .and_then(|bytes| {
            contributor_count
                .checked_mul(size_of::<usize>() + size_of::<CasterBinding>())
                .and_then(|owners| bytes.checked_add(owners))
        })
        .ok_or("Surface radiance allocation size overflow")?;
    let mut allocation = budget.reserve_bytes(bytes)?;
    let mut surfaces = Vec::with_capacity(triangles);
    let mut radiance = Vec::with_capacity(source_face_count);
    for scope in model_scopes.iter().filter(|scope| scope.reflected_source) {
        let frame = Arc::new(Frame {
            entity: scope.entity,
            pose: (scope.entity != 0).then_some(scope.pose),
        });
        for index in scope.faces.clone() {
            let face = &faces[index];
            budget.consume(1)?;
            if scope.entity != 0 && face.dispinfo >= 0 {
                return Err("Radiance model sources cannot contain terrain".into());
            }
            let info = infos[face.texinfo as usize];
            let sky = info.flags & (surf::SKY | surf::SKY2D) != 0;
            let light = lighting
                .as_ref()
                .map(|lighting| lighting.face(face, &info))
                .transpose()
                .map_err(|error| error.to_string())?
                .flatten();
            let reflectivity = if sky {
                DVec3::ONE
            } else {
                let material = map.materials.get(map.texdata[info.texdata as usize]);
                let reflectivity =
                    DVec3::from_array(material.texture_metrics()?.reflectivity.map(f64::from));
                if !reflectivity.is_finite() || reflectivity.min_element() < 0.0 {
                    return Err(format!(
                        "Material {} has invalid surface reflectivity",
                        material.name
                    ));
                }
                if scope.entity != 0 && reflectivity.max_element() > 1.0 {
                    return Err(format!(
                        "Stationary reflected source entity:{} needs material {} reflectivity between 0 and 1",
                        map.entities[scope.entity].id, material.name,
                    ));
                }
                reflectivity / 255.0
            };
            let average = if sky {
                sky_radiance
            } else {
                light
                    .as_ref()
                    .and_then(|light| light.average(0))
                    .map_or(DVec3::ZERO, |color| DVec3::from_array(color.linear()))
                    * reflectivity
            };
            if !average.is_finite() || average.min_element() < 0.0 {
                return Err("Surface radiance is nonfinite or negative".into());
            }
            let samples = if !sky {
                light
                    .as_ref()
                    .and_then(|light| light.samples(0, 0))
                    .unwrap_or_default()
            } else {
                &[]
            };
            budget.consume(samples.len())?;
            let mut extras = Vec::with_capacity(3);
            if styled
                && !sky
                && let Some(light) = &light
            {
                for (index, &style) in light.styles().iter().enumerate().skip(1) {
                    let samples = light
                        .samples(index, 0)
                        .ok_or("Styled radiance base map is missing")?;
                    budget.consume(samples.len() + 1)?;
                    let average = light
                        .average(index)
                        .ok_or("Styled radiance average is missing")?;
                    extras.push(Layer {
                        style,
                        average: DVec3::from_array(average.linear()) * reflectivity,
                        samples: samples.to_vec(),
                    });
                }
            }
            let boundary = if face.dispinfo < 0 {
                if face.side != (face.plane & 1) as u8
                    || usize::from(face.plane) >= map.planes.len()
                {
                    return Err("Radiance source does not use its emitted outward plane".into());
                }
                let sides = &face_sources[index];
                budget.consume(sides.len() + 1)?;
                if sides.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err("Radiance source contributors must be sorted and unique".into());
                }
                if scope.entity != 0 && sides.is_empty() {
                    return Err("Radiance model source has no original side contributors".into());
                }
                let mut owners = Vec::with_capacity(sides.len());
                for &id in sides.iter() {
                    budget
                        .consume(source_owners.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                    let index = source_owners
                        .binary_search_by_key(&id, |source| source.id)
                        .map_err(|_| "Radiance source contributor has no admitted brush owner")?;
                    let source_side = &source_owners[index];
                    let owner = source_side.owner;
                    let plane = source_side.plane;
                    if map.brushes[owner].entity != scope.entity
                        || (scope.entity != 0 && plane != usize::from(face.plane))
                    {
                        return Err(
                            "Radiance source contributor lost its original model or plane identity"
                                .into(),
                        );
                    }
                    owners.push(owner);
                }
                budget.consume(
                    owners
                        .len()
                        .checked_mul(owners.len().checked_ilog2().unwrap_or(0) as usize + 1)
                        .ok_or("Radiance contributor sort work overflow")?,
                )?;
                owners.sort_unstable();
                owners.dedup();
                let plane = map.planes.get(usize::from(face.plane));
                let plane = map_core::Plane {
                    normal: plane.normal,
                    dist: plane.dist,
                };
                let equation = if let Some(pose) = frame.pose {
                    budget.consume(SOURCE_PLANE_WORK + frame.transform_work())?;
                    pose.plane(plane)
                } else {
                    plane
                };
                Some(SourceBoundary {
                    plane: usize::from(face.plane),
                    equation,
                    sides: sides.clone(),
                    owners,
                })
            } else {
                None
            };
            let source = radiance.len();
            let material = if face.dispinfo >= 0 {
                map.terrains[face.dispinfo as usize].material
            } else {
                map.texdata[info.texdata as usize]
            };
            let alpha =
                brush_textures.and_then(|textures| textures.face(material, info.texture_vecs));
            let mut casters = Vec::new();
            let mut contributors = Vec::new();
            if let Some(boundary) = &boundary {
                casters = Vec::with_capacity(boundary.owners.len());
                for &owner in &boundary.owners {
                    budget
                        .consume(caster_lookup.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                    if let Ok(index) =
                        caster_lookup.binary_search_by_key(&owner, |&(owner, _)| owner)
                    {
                        casters.push(CasterBinding {
                            slot: caster_lookup[index].1,
                            owner,
                        });
                    }
                }
                if (scope.entity != 0 || alpha.is_some())
                    && (casters.is_empty() || casters.len() != boundary.owners.len())
                {
                    return Err(
                        "Radiance source lost its complete prepared global caster bindings".into(),
                    );
                }
                if !casters.is_empty() && casters.len() != boundary.owners.len() {
                    if frame.pose.is_some() {
                        return Err(
                            "A model source cannot mix caster and noncaster contributors".into(),
                        );
                    }
                    allocation.grow(
                        budget,
                        boundary
                            .sides
                            .len()
                            .checked_mul(size_of::<Contributor>())
                            .ok_or("Native source contributor allocation overflow")?,
                    )?;
                    contributors = Vec::with_capacity(boundary.sides.len());
                    for &id in boundary.sides.iter() {
                        budget.consume(
                            source_owners.len().checked_ilog2().unwrap_or(0) as usize + 1,
                        )?;
                        let index = source_owners
                            .binary_search_by_key(&id, |source| source.id)
                            .map_err(|_| "Native source attribution lost its contributor")?;
                        if source_owners[index].plane != usize::from(face.plane) {
                            return Err(format!(
                                "Native source attribution side:{id} lost its original plane"
                            ));
                        }
                        let footprint = if let Some(footprint) = &source_owners[index].footprint {
                            Arc::clone(footprint)
                        } else {
                            let footprint = Arc::new(prepare_footprint(
                                map,
                                &source_owners[index],
                                &mut allocation,
                                budget,
                            )?);
                            source_owners[index].footprint = Some(Arc::clone(&footprint));
                            footprint
                        };
                        let owner = source_owners[index].owner;
                        budget.consume(casters.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                        contributors.push(Contributor {
                            side: id,
                            owner,
                            caster: casters
                                .binary_search_by_key(&owner, |caster| caster.owner)
                                .is_ok(),
                            footprint,
                        });
                    }
                }
            }
            radiance.push(Radiance {
                frame: Arc::clone(&frame),
                alpha,
                casters,
                contributors,
                average,
                reflectivity,
                samples: samples.to_vec(),
                dimensions: light.as_ref().map_or([0; 2], |light| light.dimensions()),
                projection: info.lightmap_vecs,
                mins: face.lightmap_mins,
                sky,
                extras,
                boundary,
            });
            if face.dispinfo >= 0 {
                let terrain = &map.terrains[face.dispinfo as usize];
                budget.consume(terrain.vertices.len())?;
                let size = terrain
                    .info
                    .grid_size()
                    .map_err(|error| error.to_string())?;
                for (index, indices) in terrain.indices.iter().enumerate() {
                    let objects = terrain_shadows.objects();
                    budget.consume(objects.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                    let shadow = objects
                        .binary_search_by_key(&(face.dispinfo as usize, index), |triangle| {
                            triangle.source()
                        })
                        .map_err(|_| "Radiance terrain source has no prepared shadow geometry")?;
                    let triangle = objects[shadow].clone();
                    let points = [
                        triangle.point([1.0, 0.0, 0.0]),
                        triangle.point([0.0, 1.0, 0.0]),
                        triangle.point([0.0, 0.0, 1.0]),
                    ];
                    add_surface(
                        &mut surfaces,
                        points,
                        source,
                        Some(indices.map(|index| {
                            DVec2::new((index % size) as f64, (index / size) as f64)
                                / (size - 1) as f64
                        })),
                        None,
                        Geometry::Terrain(triangle),
                        &frame,
                        budget,
                    )?;
                }
            } else {
                budget.consume(face.edge_count as usize)?;
                let _point_scratch = budget.reserve::<DVec3>(face.edge_count as usize)?;
                let points = surfedges
                    [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
                    .iter()
                    .map(|&edge| {
                        let pair = edges[edge.unsigned_abs() as usize].0;
                        DVec3::from_array(
                            vertices[pair[usize::from(edge < 0)] as usize].map(f64::from),
                        )
                    })
                    .collect::<Vec<_>>();
                let _fan_scratch = budget.reserve::<Triangle>(points.len().saturating_sub(2))?;
                let mut fan = Vec::with_capacity(points.len().saturating_sub(2));
                for index in 1..points.len().saturating_sub(1) {
                    let points = [points[0], points[index], points[index + 1]];
                    budget.consume(1)?;
                    if (points[1] - points[0])
                        .cross(points[2] - points[0])
                        .length_squared()
                        == 0.0
                    {
                        continue;
                    }
                    fan.push(Triangle::new(points)?);
                }
                let fan = Arc::new(BrushFan {
                    triangles: fan.into_boxed_slice(),
                    frame: Arc::clone(&frame),
                });
                for (index, triangle) in fan.triangles.iter().enumerate() {
                    let points = [
                        triangle.point([1.0, 0.0, 0.0]),
                        triangle.point([0.0, 1.0, 0.0]),
                        triangle.point([0.0, 0.0, 1.0]),
                    ];
                    add_surface(
                        &mut surfaces,
                        points,
                        source,
                        None,
                        radiance[source].boundary.as_ref(),
                        Geometry::Brush {
                            fan: Arc::clone(&fan),
                            index,
                        },
                        &frame,
                        budget,
                    )?;
                }
            }
        }
    }
    budget.consume(radiance.len())?;
    let sheet_sources = radiance.iter().any(|source| {
        !source.sky
            && source.boundary.is_some()
            && (source.casters.is_empty() || !source.contributors.is_empty())
    });
    let fractional_sources = radiance.iter().any(|source| {
        source
            .alpha
            .as_ref()
            .is_some_and(super::brush_alpha::Face::fractional)
    });
    Ok(SurfaceIndex {
        crossings: surfaces.len(),
        shadow_count: shadows.objects().len(),
        fractional_sources,
        sheet_sources,
        geometry: ShadowIndex::new(surfaces, &mut |work| budget.consume(work))?,
        radiance,
        _allocation: allocation,
    })
}

pub(super) struct Hit {
    pub color: DVec3,
    pub point: DVec3,
    pub sky: bool,
    pub front: bool,
    local_point: Option<DVec3>,
    source: usize,
    uv: Option<DVec2>,
    distance: f64,
    visibility: DVec3,
}

struct Crossing {
    fraction: f64,
    ordinal: usize,
    source: usize,
    uv: Option<DVec2>,
    front: bool,
    point: DVec3,
    sample_point: DVec3,
    boundary_error: f64,
    coverage: Coverage,
    sheet: bool,
}

fn pending_work(pending: &Cell<usize>, work: usize) -> Result<(), String> {
    pending.set(
        pending
            .get()
            .checked_add(work)
            .ok_or("Radiance source work overflow")?,
    );
    Ok(())
}

fn transport_bytes(amount: DVec3) -> [u8; 3] {
    amount
        .to_array()
        .map(|channel| (channel * f64::from(u8::MAX)).round() as u8)
}

fn limited_coverage(amount: Coverage, remaining: DVec3) -> Coverage {
    let amount = transport_bytes(amount.amount());
    let remaining = transport_bytes(remaining);
    Coverage::from_channels(std::array::from_fn(|channel| {
        amount[channel].min(remaining[channel])
    }))
}

impl SurfaceIndex {
    fn crossing(
        &self,
        surface: &Surface,
        fraction: f64,
        weights: [f64; 3],
        delta: DVec3,
        pending: &Cell<usize>,
    ) -> Result<Crossing, String> {
        let radiance = &self.radiance[surface.source];
        let coverage = if !radiance.sky && radiance.alpha.is_some() && radiance.boundary.is_some() {
            Coverage::CLEAR
        } else if let Some(alpha) = &radiance.alpha {
            pending_work(pending, SOURCE_SAMPLE_WORK)?;
            alpha.coverage(surface.geometry.texture_point(weights))?
        } else {
            Coverage::OPAQUE
        };
        let uv = surface.coordinates.map(|coordinates| {
            coordinates
                .into_iter()
                .zip(weights)
                .map(|(point, weight)| point * weight)
                .sum::<DVec2>()
        });
        let sample_point = surface.geometry.point(weights);
        let boundary_error =
            surface.boundary_error * weights.into_iter().map(f64::abs).sum::<f64>();
        let mut sheet = radiance.boundary.is_some()
            && radiance.casters.is_empty()
            && radiance.alpha.is_none()
            && !radiance.sky;
        if !radiance.contributors.is_empty() {
            let mut supported = false;
            sheet = false;
            for contributor in &radiance.contributors {
                pending_work(
                    pending,
                    contributor.footprint.points.len() * SOURCE_MEMBERSHIP_WORK,
                )?;
                if contributor
                    .footprint
                    .contains(sample_point, boundary_error)?
                {
                    supported = true;
                    sheet |= !contributor.caster;
                }
            }
            if !supported {
                return Err(format!(
                    "Unconditioned native source attribution at side:{}",
                    radiance.contributors[0].side
                ));
            }
        }
        Ok(Crossing {
            fraction,
            ordinal: surface.ordinal,
            source: surface.source,
            uv,
            front: surface.normal.dot(-delta) > 0.0,
            point: radiance.frame.point(sample_point),
            sample_point,
            boundary_error,
            coverage,
            sheet,
        })
    }

    fn membership(&self, crossing: &Crossing) -> Membership<'_> {
        Membership {
            contributors: &self.radiance[crossing.source].contributors,
            point: crossing.sample_point,
            error: crossing.boundary_error,
        }
    }

    fn binary_coverage(
        &self,
        ray: &Ray<'_>,
        crossing: &Crossing,
        budget: &mut Budget<'_>,
    ) -> Result<Coverage, String> {
        let radiance = &self.radiance[crossing.source];
        if radiance.alpha.is_none() || radiance.boundary.is_none() {
            return Ok(crossing.coverage);
        }
        for binding in &radiance.casters {
            if let Some(event) = self.hull_event(ray, crossing, binding, budget)? {
                let pending = Cell::new(0usize);
                let coverage = self.event_coverage(crossing.source, event.point, &pending)?;
                budget.consume(pending.replace(0))?;
                if !coverage.clear() && !coverage.opaque() {
                    return Err("Binary radiance source returned fractional coverage".into());
                }
                return Ok(coverage);
            }
        }
        Ok(Coverage::CLEAR)
    }

    fn sheet_terminal(
        &self,
        start: DVec3,
        end: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Crossing>, String> {
        let pending = Cell::new(0usize);
        let mut first: Option<Crossing> = None;
        let result = self.geometry.visit(
            start,
            end,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending.replace(0))
                        .ok_or("Legacy sheet query work overflow")?,
                )
            },
            &mut |surface| {
                let limit = first.as_ref().map_or(1.0, |first| first.fraction);
                let source = &self.radiance[surface.source];
                if source.sky
                    || source.boundary.is_none()
                    || (!source.casters.is_empty() && source.contributors.is_empty())
                {
                    return Ok(Visit::Continue(limit));
                }
                let Some((fraction, weights)) =
                    surface.geometry.intersection(start, end, &pending)?
                else {
                    return Ok(Visit::Continue(limit));
                };
                if first.as_ref().is_some_and(|first| {
                    (fraction, surface.ordinal) >= (first.fraction, first.ordinal)
                }) {
                    return Ok(Visit::Continue(limit));
                }
                let hit = self.crossing(surface, fraction, weights, end - start, &pending)?;
                if hit.sheet {
                    first = Some(hit);
                }
                Ok(Visit::Continue(
                    first.as_ref().map_or(1.0, |first| first.fraction),
                ))
            },
        );
        budget.consume(pending.replace(0))?;
        result?;
        Ok(first)
    }

    fn emit(
        &self,
        crossing: &Crossing,
        visibility: DVec3,
        endpoint: (DVec3, f64),
        cone: f64,
        budget: &mut Budget<'_>,
        visitor: &mut impl FnMut(&Hit, &mut Budget<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let radiance = &self.radiance[crossing.source];
        if visibility == DVec3::ZERO || (!radiance.sky && !crossing.front) {
            return Ok(());
        }
        budget.consume(24)?;
        let (point, fraction) = endpoint;
        let distance = fraction * super::sky::TRACE_LENGTH;
        let sample_point = if radiance.sky {
            point
        } else {
            crossing.sample_point
        };
        visitor(
            &Hit {
                color: radiance.at(sample_point, crossing.uv, distance, cone) * visibility,
                point,
                sky: radiance.sky,
                front: crossing.front,
                local_point: radiance.frame.pose.map(|_| sample_point),
                source: crossing.source,
                uv: crossing.uv,
                distance,
                visibility,
            },
            budget,
        )
    }

    fn event_coverage(
        &self,
        source: usize,
        point: DVec3,
        pending: &Cell<usize>,
    ) -> Result<Coverage, String> {
        let radiance = &self.radiance[source];
        if let Some(alpha) = &radiance.alpha {
            pending_work(
                pending,
                SOURCE_SAMPLE_WORK + radiance.frame.transform_work(),
            )?;
            alpha.coverage(radiance.frame.local_point(point))
        } else {
            Ok(Coverage::OPAQUE)
        }
    }

    fn hull_event<'a>(
        &self,
        ray: &Ray<'a>,
        crossing: &Crossing,
        binding: &CasterBinding,
        budget: &mut Budget<'_>,
    ) -> Result<Option<HullEvent<'a>>, String> {
        let start = ray.start;
        let end = ray.end;
        let hull = ray
            .scene
            .shadows
            .objects()
            .get(binding.slot)
            .ok_or("Radiance source lost its prepared caster index")?;
        if hull
            .source
            .as_ref()
            .is_none_or(|source| source.brush != binding.owner)
        {
            return Err("Radiance source lost its prepared caster ownership".into());
        }
        let radiance = &self.radiance[crossing.source];
        let boundary = radiance
            .boundary
            .as_ref()
            .ok_or("Radiance caster source lost its original boundary")?;
        budget.consume(hull.work() + hull.plane_count() + SOURCE_TRANSPORT_WORK)?;
        let Some(interval) = hull.interval(start, end)? else {
            return Ok(None);
        };
        let (event, work) = boundary.crossing(
            hull,
            interval,
            start,
            end,
            crossing.point,
            crossing.boundary_error,
            self.membership(crossing),
        )?;
        budget.consume(work)?;
        let Some((entering, point)) = event else {
            return Ok(None);
        };
        let boundary_slot = usize::from(!entering);
        let Some(event) = hull.optical_boundaries(start, end)?[boundary_slot] else {
            return Ok(None);
        };
        if ray
            .terminal
            .is_some_and(|terminal| event.fraction > terminal)
        {
            return Ok(None);
        }
        Ok(Some(HullEvent {
            hull,
            interval,
            entering,
            point,
            boundary: boundary_slot,
            value: event,
        }))
    }

    fn hull_weight(
        &self,
        ray: &Ray<'_>,
        crossing: &Crossing,
        binding: &CasterBinding,
        budget: &mut Budget<'_>,
    ) -> Result<Coverage, String> {
        let Some(event) = self.hull_event(ray, crossing, binding, budget)? else {
            return Ok(Coverage::CLEAR);
        };
        let hull = event.hull;
        let interval = event.interval;
        let entering = event.entering;
        let start = ray.start;
        let end = ray.end;
        let radiance = &self.radiance[crossing.source];
        let pending = Cell::new(0usize);
        let amount = self.event_coverage(crossing.source, event.point, &pending)?;
        budget.consume(pending.replace(0))?;
        if amount
            .amount()
            .cmpgt(event.value.non_sky_coverage.amount())
            .any()
        {
            return Err(
                "Radiance source coverage exceeds its certified non-sky caster event".into(),
            );
        }
        if amount.clear() {
            return Ok(Coverage::CLEAR);
        }
        let remaining = transport_bytes(ray.scene.source_visibility(
            start,
            end,
            event.value.fraction,
            SourceTarget::Hull {
                slot: binding.slot,
                boundary: event.boundary,
            },
            ray.skip_owner,
            budget,
        )?);
        if remaining.iter().all(|&amount| amount == 0) {
            return Ok(Coverage::CLEAR);
        }
        let amounts = transport_bytes(amount.amount());
        let mut owned: [u8; 3] = std::array::from_fn(|channel| {
            if remaining[channel] == 0 {
                0
            } else {
                amounts[channel]
            }
        });
        let limit = if amount.opaque() {
            crossing.fraction
        } else {
            1.0
        };
        let result = self.geometry.visit(
            start,
            end,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending.replace(0))
                        .ok_or("Radiance event partition work overflow")?,
                )
            },
            &mut |surface| {
                if surface.ordinal == crossing.ordinal {
                    return Ok(Visit::Continue(limit));
                }
                let other = &self.radiance[surface.source];
                let Some(other_boundary) = &other.boundary else {
                    return Ok(Visit::Continue(limit));
                };
                if other.sky || other.frame.entity != radiance.frame.entity {
                    return Ok(Visit::Continue(limit));
                }
                pending_work(
                    &pending,
                    other_boundary.owners.len().checked_ilog2().unwrap_or(0) as usize + 1,
                )?;
                if other_boundary.owners.binary_search(&binding.owner).is_err() {
                    return Ok(Visit::Continue(limit));
                }
                let Some((fraction, weights)) =
                    surface.geometry.intersection(start, end, &pending)?
                else {
                    return Ok(Visit::Continue(limit));
                };
                if fraction > limit {
                    return Ok(Visit::Continue(limit));
                }
                let other_crossing =
                    self.crossing(surface, fraction, weights, end - start, &pending)?;
                let (other_event, work) = other_boundary.crossing(
                    hull,
                    interval,
                    start,
                    end,
                    other_crossing.point,
                    other_crossing.boundary_error,
                    self.membership(&other_crossing),
                )?;
                pending_work(&pending, work)?;
                let Some((other_entering, point)) = other_event else {
                    return Ok(Visit::Continue(limit));
                };
                if other_entering != entering {
                    return Ok(Visit::Continue(limit));
                }
                let coverage = self.event_coverage(surface.source, point, &pending)?;
                if coverage
                    .amount()
                    .cmpgt(event.value.non_sky_coverage.amount())
                    .any()
                {
                    return Err(
                        "Radiance source coverage exceeds its certified non-sky caster event"
                            .into(),
                    );
                }
                pending_work(&pending, SOURCE_TRANSPORT_WORK)?;
                let earlier = (fraction, surface.ordinal) < (crossing.fraction, crossing.ordinal);
                for (channel, amount) in transport_bytes(coverage.amount()).into_iter().enumerate()
                {
                    if amount > amounts[channel] || (amount == amounts[channel] && earlier) {
                        owned[channel] = 0;
                    }
                }
                Ok(if owned.iter().all(|&amount| amount == 0) {
                    Visit::Stop
                } else {
                    Visit::Continue(limit)
                })
            },
        );
        budget.consume(pending.replace(0))?;
        result?;
        Ok(Coverage::from_channels(std::array::from_fn(|channel| {
            owned[channel].min(remaining[channel])
        })))
    }

    pub fn styled_colors(
        &self,
        hit: &Hit,
        cone: f64,
        budget: &mut Budget<'_>,
    ) -> Result<[(u8, DVec3); 4], String> {
        let mut colors = [(255, DVec3::ZERO); 4];
        colors[0] = (0, hit.color);
        let radiance = &self.radiance[hit.source];
        for (index, layer) in radiance.extras.iter().enumerate() {
            budget.consume(24)?;
            colors[index + 1] = (
                layer.style,
                radiance.at_layer(
                    &layer.samples,
                    layer.average,
                    hit.local_point.unwrap_or(hit.point),
                    hit.uv,
                    hit.distance,
                    cone,
                ) * hit.visibility,
            );
        }
        Ok(colors)
    }

    fn next_crossing(
        &self,
        point: DVec3,
        end: DVec3,
        previous: Option<(f64, bool, usize)>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Crossing>, String> {
        let mut first: Option<Crossing> = None;
        let pending_alpha = Cell::new(0usize);
        let result = self.geometry.visit(
            point,
            end,
            &mut |work| {
                budget.consume(
                    work.checked_add(pending_alpha.replace(0))
                        .ok_or("Brush alpha source work overflow")?,
                )
            },
            &mut |surface| {
                let Some((fraction, weights)) =
                    surface.geometry.intersection(point, end, &pending_alpha)?
                else {
                    return Ok(Visit::Continue(
                        first.as_ref().map_or(1.0, |hit| hit.fraction),
                    ));
                };
                let sky = self.radiance[surface.source].sky;
                let key = (fraction, sky, surface.ordinal);
                if previous.is_some_and(|previous| key <= previous)
                    || first.as_ref().is_some_and(|hit| {
                        key >= (hit.fraction, self.radiance[hit.source].sky, hit.ordinal)
                    })
                {
                    return Ok(Visit::Continue(
                        first.as_ref().map_or(1.0, |hit| hit.fraction),
                    ));
                }
                let crossing =
                    self.crossing(surface, fraction, weights, end - point, &pending_alpha)?;
                let radiance = &self.radiance[surface.source];
                if crossing.coverage.clear()
                    && (radiance.sky || radiance.alpha.is_none() || radiance.boundary.is_none())
                {
                    return Ok(Visit::Continue(
                        first.as_ref().map_or(1.0, |hit| hit.fraction),
                    ));
                }
                first = Some(crossing);
                Ok(Visit::Continue(fraction))
            },
        );
        budget.consume(pending_alpha.replace(0))?;
        result?;
        Ok(first)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn trace(
        &self,
        point: DVec3,
        direction: DVec3,
        cone: f64,
        scene: &Scene<'_>,
        skipped_owner: Option<u32>,
        budget: &mut Budget<'_>,
        visitor: &mut impl FnMut(&Hit, &mut Budget<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        budget.consume(11)?;
        let end = point + direction * super::sky::TRACE_LENGTH;
        if scene.shadows.objects().len() != self.shadow_count {
            return Err("Radiance sources require their prepared global caster index".into());
        }
        let sheet = if self.fractional_sources && self.sheet_sources {
            self.sheet_terminal(point, end, budget)?
        } else {
            None
        };
        let sky = if self.fractional_sources {
            scene
                .shadows
                .sky_terminal_fraction(point, end, &mut |work| budget.consume(work))?
        } else {
            None
        };
        let sky_limit = sky.map(|fraction| (fraction + super::shadows::TRIANGLE_EPSILON).min(1.0));
        let sheet_fraction = sheet.as_ref().map(|sheet| sheet.fraction);
        let terminal = match (sheet_fraction, sky_limit) {
            (Some(sheet), Some(sky)) => Some(sheet.min(sky)),
            (sheet, sky) => sheet.or(sky),
        };
        let ray = Ray {
            start: point,
            end,
            scene,
            skip_owner: skipped_owner,
            terminal,
        };
        let mut previous = None;
        let mut emitted_sky = false;
        for _ in 0..self.crossings {
            budget.consume(1)?;
            let Some(crossing) = self.next_crossing(point, end, previous, budget)? else {
                break;
            };
            let radiance = &self.radiance[crossing.source];
            previous = Some((crossing.fraction, radiance.sky, crossing.ordinal));
            if !self.fractional_sources {
                if radiance.sky {
                    let Some(fraction) =
                        scene
                            .shadows
                            .optical_sky_fraction(point, end, &mut |work| budget.consume(work))?
                    else {
                        break;
                    };
                    let visibility =
                        scene.sky_visibility(point, end, fraction, skipped_owner, budget)?;
                    self.emit(
                        &crossing,
                        visibility,
                        (point.lerp(end, fraction), fraction),
                        cone,
                        budget,
                        visitor,
                    )?;
                } else {
                    let amount = self.binary_coverage(&ray, &crossing, budget)?;
                    if amount.clear() {
                        continue;
                    }
                    let visibility = scene.visibility_to_source(
                        point,
                        crossing.point,
                        skipped_owner,
                        radiance.boundary.as_ref(),
                        crossing.boundary_error,
                        budget,
                    )?;
                    self.emit(
                        &crossing,
                        limited_coverage(amount, visibility).amount(),
                        (crossing.point, crossing.fraction),
                        cone,
                        budget,
                        visitor,
                    )?;
                }
                break;
            }
            if radiance.sky {
                if emitted_sky {
                    continue;
                }
                emitted_sky = true;
                let Some(fraction) = sky else {
                    continue;
                };
                if sheet_fraction.is_some_and(|sheet| sheet <= sky_limit.unwrap_or(fraction)) {
                    continue;
                }
                let visibility = scene.source_visibility(
                    point,
                    end,
                    fraction,
                    SourceTarget::Sky,
                    skipped_owner,
                    budget,
                )?;
                self.emit(
                    &crossing,
                    visibility,
                    (point.lerp(end, fraction), fraction),
                    cone,
                    budget,
                    visitor,
                )?;
                continue;
            }
            if !crossing.front || radiance.reflectivity == DVec3::ZERO {
                continue;
            }
            let mut weight = Coverage::CLEAR;
            for binding in &radiance.casters {
                budget.consume(SOURCE_TRANSPORT_WORK)?;
                weight.add(self.hull_weight(&ray, &crossing, binding, budget)?);
            }
            if let Geometry::Terrain(triangle) = &self.geometry.objects()[crossing.ordinal].geometry
                && terminal.is_none_or(|terminal| crossing.fraction <= terminal)
            {
                let remaining = scene.source_visibility(
                    point,
                    end,
                    crossing.fraction,
                    SourceTarget::Terrain {
                        slot: triangle.event_slot(),
                    },
                    skipped_owner,
                    budget,
                )?;
                budget.consume(SOURCE_TRANSPORT_WORK)?;
                weight.add(limited_coverage(crossing.coverage, remaining));
            }
            if sheet
                .as_ref()
                .is_some_and(|sheet| sheet.ordinal == crossing.ordinal)
                && sky_limit.is_none_or(|sky| crossing.fraction <= sky)
            {
                let remaining = scene.source_visibility(
                    point,
                    end,
                    crossing.fraction,
                    SourceTarget::Sheet,
                    skipped_owner,
                    budget,
                )?;
                budget.consume(SOURCE_TRANSPORT_WORK)?;
                weight.add(limited_coverage(Coverage::OPAQUE, remaining));
            }
            self.emit(
                &crossing,
                weight.amount(),
                (crossing.point, crossing.fraction),
                cone,
                budget,
                visitor,
            )?;
        }
        Ok(())
    }
}
