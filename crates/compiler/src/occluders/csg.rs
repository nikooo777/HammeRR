use glam::DVec3;
use map_core::{BrushGeometry, subobject};
use source_formats::{bsp::MAX_OCCLUDERS, vmf::Solid};

use super::{
    MAX_BRUSHES_PER_OCCLUDER, MAX_POLYGONS_PER_OCCLUDER, MAX_VERTICES_PER_OCCLUDER, Polygon,
};
use crate::Control;
use crate::emit::stored_vertex;
use crate::planes::PlaneSet;
use crate::winding::{ON_EPSILON, Side, Winding};

const MAX_WORK: usize = 4_000_000;
const MAX_ALLOCATED_POINTS: usize = 1_000_000;
const MAX_FRAGMENTS: usize = 65_536;
const MAX_SOURCE_BRUSHES: usize = MAX_OCCLUDERS * MAX_BRUSHES_PER_OCCLUDER;
const GEOMETRY_EPSILON: f64 = 1e-7;
const NORMAL_EPSILON: f64 = 1e-10;

pub(super) struct Failure {
    pub message: String,
    pub brushes: [Option<usize>; 2],
}

pub(crate) struct Budget {
    work: usize,
    points: usize,
    fragments: usize,
    max_work: usize,
    max_points: usize,
    max_fragments: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            work: 0,
            points: 0,
            fragments: 0,
            max_work: MAX_WORK,
            max_points: MAX_ALLOCATED_POINTS,
            max_fragments: MAX_FRAGMENTS,
        }
    }
}

impl Budget {
    fn consume(&mut self, work: usize, control: &Control) -> Result<(), String> {
        control.checkpoint()?;
        self.work = self
            .work
            .checked_add(work)
            .filter(|&value| value <= self.max_work)
            .ok_or("Occluder union exceeds its four-million work-unit budget")?;

        Ok(())
    }

    fn reserve(
        &mut self,
        points: usize,
        fragments: usize,
        control: &Control,
    ) -> Result<(), String> {
        self.consume(points.saturating_add(fragments), control)?;
        let points = self
            .points
            .checked_add(points)
            .filter(|&value| value <= self.max_points)
            .ok_or("Occluder union exceeds its one-million allocated-point budget")?;
        let fragments = self
            .fragments
            .checked_add(fragments)
            .filter(|&value| value <= self.max_fragments)
            .ok_or("Occluder union exceeds its 65536 generated-fragment budget")?;
        self.points = points;
        self.fragments = fragments;
        Ok(())
    }

    fn split(&mut self, points: usize, control: &Control) -> Result<(), String> {
        self.reserve(points.saturating_mul(5).saturating_add(8), 2, control)
    }
}

enum BoundsRelation {
    Separated,
    CollapsedGap,
    Unproven,
}

fn bounds_relation(a: &BrushGeometry, b: &BrushGeometry) -> BoundsRelation {
    let (a_mins, a_maxs) = (stored_vertex(a.mins), stored_vertex(a.maxs));
    let (b_mins, b_maxs) = (stored_vertex(b.mins), stored_vertex(b.maxs));
    let mut source_gap = false;
    for axis in 0..3 {
        if a.maxs[axis] < b.mins[axis] {
            source_gap = true;
            if a_maxs[axis] < b_mins[axis] {
                return BoundsRelation::Separated;
            }
        }
        if b.maxs[axis] < a.mins[axis] {
            source_gap = true;
            if b_maxs[axis] < a_mins[axis] {
                return BoundsRelation::Separated;
            }
        }
    }
    if source_gap {
        return BoundsRelation::CollapsedGap;
    }
    BoundsRelation::Unproven
}

fn no_area_contact(a: &BrushGeometry, b: &BrushGeometry) -> bool {
    let low = a.mins.max(b.mins);
    let high = a.maxs.min(b.maxs);
    // Two exact zero-width intervals constrain intersection to a line or point;
    // monotone native coordinate rounding preserves their shared endpoints.
    low.cmple(high).all() && (0..3).filter(|&axis| low[axis] == high[axis]).count() >= 2
}

struct Features {
    points: Vec<DVec3>,
    directions: Vec<DVec3>,
}

fn features(solid: &Solid, budget: &mut Budget, control: &Control) -> Result<Features, String> {
    if solid.sides.len() > subobject::MAX_SIDES {
        return Err("Occluder union requires closed convex brushes with at most 64 sides each; separate or simplify this helper".into());
    }
    let sides = solid.sides.len();
    let clipping = sides.saturating_mul(sides).saturating_mul(sides + 8);
    budget.reserve(clipping, sides, control)?;
    let topology = subobject::topology(solid).map_err(|error| {
        format!("Occluder union requires closed convex brush topology: {error}")
    })?;
    budget.reserve(topology.vertices.len() + topology.edges.len(), 0, control)?;
    let mut directions = Vec::<DVec3>::new();
    for edge in &topology.edges {
        budget.consume(directions.len() + 1, control)?;
        let direction = (topology.vertices[edge.vertices[1]].position
            - topology.vertices[edge.vertices[0]].position)
            .normalize();
        if !directions
            .iter()
            .any(|other| direction.cross(*other).length_squared() <= NORMAL_EPSILON)
        {
            directions.push(direction);
        }
    }
    Ok(Features {
        points: topology
            .vertices
            .into_iter()
            .map(|vertex| vertex.position)
            .collect(),
        directions,
    })
}

fn overlap_on_axis(
    a: &Features,
    b: &Features,
    axis: DVec3,
    budget: &mut Budget,
    control: &Control,
) -> Result<Option<f64>, String> {
    budget.consume(a.points.len() + b.points.len() + 1, control)?;
    let Some(axis) = axis.try_normalize() else {
        return Ok(None);
    };
    let range = |points: &[DVec3]| {
        points
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), point| {
                let value = point.dot(axis);
                (low.min(value), high.max(value))
            })
    };
    let (a_low, a_high) = range(&a.points);
    let (b_low, b_high) = range(&b.points);
    Ok(Some(a_high.min(b_high) - a_low.max(b_low)))
}

fn split(
    winding: &Winding,
    normal: DVec3,
    distance: f64,
    budget: &mut Budget,
    control: &Control,
) -> Result<(Winding, Winding), String> {
    budget.split(winding.0.len(), control)?;
    let (Some(front), Some(back)) = winding.split(normal, distance) else {
        return Err("Occluder union produced a collapsed intersection; separate or simplify the touching brushes".into());
    };
    if front.is_tiny()
        || back.is_tiny()
        || (front.area() + back.area() - winding.area()).abs()
            > (winding.area() * 1e-9).max(GEOMETRY_EPSILON)
    {
        return Err("Occluder union produced a sliver or imprecise intersection; separate or simplify the touching brushes".into());
    }
    Ok((front, back))
}

fn face_contact(
    a: &BrushGeometry,
    b: &BrushGeometry,
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    for face in &a.faces {
        for other in &b.faces {
            budget.consume(1, control)?;
            if (face.plane.normal + other.plane.normal).length_squared() > NORMAL_EPSILON
                || (face.plane.dist + other.plane.dist).abs() > GEOMETRY_EPSILON
            {
                continue;
            }
            budget.reserve(face.vertices.len(), 1, control)?;
            let mut winding = Winding(face.vertices.clone());
            let mut outside = false;
            for clip in &b.faces {
                budget.consume(winding.0.len(), control)?;
                match winding.classify(clip.plane.normal, clip.plane.dist) {
                    Side::Front => {
                        outside = true;
                        break;
                    }
                    Side::Cross => {
                        winding = split(
                            &winding,
                            clip.plane.normal,
                            clip.plane.dist,
                            budget,
                            control,
                        )?
                        .1;
                    }
                    Side::On | Side::Back => {}
                }
            }
            if !outside && winding.area() > ON_EPSILON * ON_EPSILON {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn interacting(
    geometries: &[(&Solid, BrushGeometry)],
    budget: &mut Budget,
    control: &Control,
) -> Result<Vec<Vec<usize>>, Failure> {
    let mut features_by_brush = (0..geometries.len()).map(|_| None).collect::<Vec<_>>();
    let mut neighbors = vec![Vec::new(); geometries.len()];
    for (a, (solid_a, geometry_a)) in geometries.iter().enumerate() {
        for (b, (solid_b, geometry_b)) in geometries.iter().enumerate().skip(a + 1) {
            let context = |error| Failure {
                message: format!(
                    "Occluder brush pair {} and {}: {error}",
                    solid_a.id, solid_b.id
                ),
                brushes: [Some(a), Some(b)],
            };
            budget.consume(1, control).map_err(context)?;
            let bounds = bounds_relation(geometry_a, geometry_b);
            if matches!(bounds, BoundsRelation::Separated)
                || no_area_contact(geometry_a, geometry_b)
            {
                continue;
            }
            if features_by_brush[a].is_none() {
                features_by_brush[a] = Some(features(solid_a, budget, control).map_err(context)?);
            }
            if features_by_brush[b].is_none() {
                features_by_brush[b] = Some(features(solid_b, budget, control).map_err(context)?);
            }
            let fa = features_by_brush[a].as_ref().unwrap();
            let fb = features_by_brush[b].as_ref().unwrap();
            let face_axes = geometry_a
                .faces
                .iter()
                .chain(&geometry_b.faces)
                .map(|face| face.plane.normal);
            let edge_axes = fa.directions.iter().flat_map(|a| {
                fb.directions.iter().filter_map(move |b| {
                    let axis = a.cross(*b);
                    (axis.length_squared() > NORMAL_EPSILON).then_some(axis)
                })
            });
            let mut minimum = f64::MAX;
            let mut disjoint = false;
            for axis in face_axes.chain(edge_axes) {
                let Some(overlap) =
                    overlap_on_axis(fa, fb, axis, budget, control).map_err(context)?
                else {
                    continue;
                };
                if overlap < -ON_EPSILON {
                    disjoint = true;
                    break;
                }
                minimum = minimum.min(overlap);
            }
            if disjoint {
                continue;
            }
            if matches!(bounds, BoundsRelation::CollapsedGap) {
                return Err(context("Occluder brush gap collapses after native vertex rounding; separate or align the brush faces exactly".into()));
            }
            if minimum <= ON_EPSILON {
                if minimum.abs() > GEOMETRY_EPSILON {
                    return Err(context("Occluder union has a gap or overlap within 0.01 units of a contact; separate or align the brush faces exactly".into()));
                }
                if !face_contact(geometry_a, geometry_b, budget, control).map_err(context)? {
                    return Err(context("Occluder contact has no certified shared face or zero-area AABB intersection; overlap their volumes or separate the brushes".into()));
                }
            }
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
    }
    Ok(neighbors)
}

fn subtract(
    winding: Winding,
    normal: DVec3,
    cutter: &BrushGeometry,
    keep_coplanar: &[bool],
    budget: &mut Budget,
    control: &Control,
) -> Result<Vec<Winding>, String> {
    for (index, face) in cutter.faces.iter().enumerate() {
        budget.consume(winding.0.len(), control)?;
        match winding.classify(face.plane.normal, face.plane.dist) {
            Side::Front => return Ok(vec![winding]),
            Side::On => {
                if winding
                    .0
                    .iter()
                    .any(|point| face.plane.distance_to(*point).abs() > GEOMETRY_EPSILON)
                {
                    return Err("Occluder union has nearly coplanar faces within 0.01 units; align them exactly or separate the brushes".into());
                }
                if normal.dot(face.plane.normal) > 0.0 && keep_coplanar[index] {
                    return Ok(vec![winding]);
                }
            }
            _ => {}
        }
    }
    let mut outside = Vec::new();
    let mut inside = winding;
    for face in &cutter.faces {
        budget.consume(inside.0.len(), control)?;
        match inside.classify(face.plane.normal, face.plane.dist) {
            Side::Front => {
                outside.push(inside);
                break;
            }
            Side::Cross => {
                let (front, back) =
                    split(&inside, face.plane.normal, face.plane.dist, budget, control)?;
                outside.push(front);
                inside = back;
            }
            Side::On | Side::Back => {}
        }
    }
    Ok(outside)
}

fn native_winding(
    mut winding: Winding,
    plane: usize,
    planes: &PlaneSet,
    budget: &mut Budget,
    control: &Control,
) -> Result<Winding, String> {
    budget.consume(winding.0.len().saturating_mul(winding.0.len()), control)?;
    for point in &mut winding.0 {
        *point = stored_vertex(*point);
    }
    let support = planes.get(plane);
    let normal = support.normal.as_vec3().as_dvec3();
    let distance = f64::from(support.dist as f32);
    if winding.is_tiny()
        || winding.normal().dot(normal) < 0.99
        || winding
            .0
            .iter()
            .any(|point| !point.is_finite() || (point.dot(normal) - distance).abs() > ON_EPSILON)
    {
        return Err("Occluder union loses plane support after native vertex rounding; enlarge or simplify its brushes".into());
    }
    for (index, &point) in winding.0.iter().enumerate() {
        let next = winding.0[(index + 1) % winding.0.len()];
        let edge = next - point;
        if edge.length_squared() <= GEOMETRY_EPSILON.powi(2)
            || winding
                .0
                .iter()
                .any(|other| edge.cross(*other - point).dot(normal) > ON_EPSILON * edge.length())
        {
            return Err("Occluder union produces a collapsed or nonconvex native polygon; simplify its brushes".into());
        }
    }
    Ok(winding)
}

fn append_boundary(
    output: &mut Vec<Polygon>,
    vertices: &mut usize,
    polygon: Polygon,
) -> Result<(), String> {
    let count = vertices
        .checked_add(polygon.winding.0.len())
        .filter(|&count| {
            count <= MAX_VERTICES_PER_OCCLUDER && output.len() < MAX_POLYGONS_PER_OCCLUDER
        })
        .ok_or_else(|| format!(
            "Occluder source brush exceeds its {MAX_POLYGONS_PER_OCCLUDER} boundary-polygon or {MAX_VERTICES_PER_OCCLUDER} boundary-vertex budget; simplify or split the helper"
        ))?;
    *vertices = count;
    output.push(polygon);
    Ok(())
}

pub(super) fn boundary_grouped(
    geometries: &[(&Solid, BrushGeometry)],
    polygons: Vec<Vec<Polygon>>,
    active_faces: &[Vec<bool>],
    planes: &PlaneSet,
    budget: &mut Budget,
    control: &Control,
) -> Result<Vec<Vec<Polygon>>, Failure> {
    if geometries.len() > MAX_SOURCE_BRUSHES
        || geometries.len() != polygons.len()
        || geometries.len() != active_faces.len()
    {
        return Err(Failure {
            message: "Occluder union requires bounded matching source-brush tables".into(),
            brushes: [None, None],
        });
    }
    budget
        .consume(geometries.len(), control)
        .map_err(|message| Failure {
            message,
            brushes: [None, None],
        })?;
    for (brush, (solid, geometry)) in geometries.iter().enumerate() {
        let context = |error| Failure {
            message: format!("Occluder brush {}: {error}", solid.id),
            brushes: [Some(brush), None],
        };
        if geometry.faces.len() > MAX_POLYGONS_PER_OCCLUDER
            || geometry.faces.len() != active_faces[brush].len()
            || polygons[brush].len() != active_faces[brush].iter().filter(|&&active| active).count()
            || !geometry.mins.is_finite()
            || !geometry.maxs.is_finite()
            || !geometry.mins.cmplt(geometry.maxs).all()
        {
            return Err(context(
                "Occluder source geometry and active-face tables do not match".to_owned(),
            ));
        }
        let mut points = 0usize;
        for face in &geometry.faces {
            budget
                .consume(face.vertices.len(), control)
                .map_err(context)?;
            points = points
                .checked_add(face.vertices.len())
                .filter(|&count| count <= MAX_VERTICES_PER_OCCLUDER)
                .ok_or_else(|| context(format!("Occluder source brush exceeds its {MAX_VERTICES_PER_OCCLUDER}-vertex budget")))?;
            if face.vertices.len() < 3
                || face.vertices.iter().any(|point| {
                    !point.is_finite()
                        || point.cmplt(geometry.mins).any()
                        || point.cmpgt(geometry.maxs).any()
                })
            {
                return Err(context(
                    "Occluder source faces exceed their checked finite bounds".into(),
                ));
            }
        }
        let (mins, maxs) = (stored_vertex(geometry.mins), stored_vertex(geometry.maxs));
        for polygon in &polygons[brush] {
            budget
                .consume(polygon.winding.0.len(), control)
                .map_err(context)?;
            if polygon.plane >= planes.len()
                || polygon.winding.0.len() < 3
                || polygon.winding.0.iter().any(|point| {
                    !point.is_finite() || point.cmplt(mins).any() || point.cmpgt(maxs).any()
                })
            {
                return Err(context(
                    "Occluder native polygon exceeds its checked source bounds or plane table"
                        .into(),
                ));
            }
        }
    }
    let neighbors = interacting(geometries, budget, control)?;
    let mut grouped = Vec::new();
    for (brush, polygons) in polygons.into_iter().enumerate() {
        let source = geometries[brush].0.id;
        let context = |error| Failure {
            message: format!("Occluder brush {source}: {error}"),
            brushes: [Some(brush), None],
        };
        let (mins, maxs) = (
            stored_vertex(geometries[brush].1.mins),
            stored_vertex(geometries[brush].1.maxs),
        );
        let mut output = Vec::new();
        let mut vertices = 0usize;
        let mut faces = geometries[brush]
            .1
            .faces
            .iter()
            .zip(&active_faces[brush])
            .filter_map(|(face, &active)| active.then_some(face));
        for polygon in polygons {
            let face = faces.next().ok_or_else(|| {
                context("Occluder polygon has no matching active source face".to_owned())
            })?;
            if neighbors[brush].is_empty() {
                append_boundary(&mut output, &mut vertices, polygon).map_err(context)?;
                continue;
            }
            let normal = planes.get(polygon.plane).normal;
            budget
                .reserve(face.vertices.len(), 1, control)
                .map_err(context)?;
            let mut fragments = vec![Winding(face.vertices.clone())];
            for &other in &neighbors[brush] {
                let cutter = &geometries[other].1;
                budget
                    .consume(active_faces[other].len(), control)
                    .map_err(context)?;
                let keep_coplanar = active_faces[other]
                    .iter()
                    .map(|&active| brush < other || !active)
                    .collect::<Vec<_>>();
                let mut next = Vec::new();
                for winding in fragments {
                    next.extend(
                        subtract(winding, normal, cutter, &keep_coplanar, budget, control)
                            .map_err(|error| Failure {
                                message: format!(
                                    "Occluder brush {source} against brush {}: {error}",
                                    geometries[other].0.id
                                ),
                                brushes: [Some(brush), Some(other)],
                            })?,
                    );
                    if next.len() > MAX_POLYGONS_PER_OCCLUDER {
                        return Err(context("Occluder source brush exceeds its 256 boundary-fragment budget; simplify or split the helper".into()));
                    }
                }
                fragments = next;
                if fragments.is_empty() {
                    break;
                }
            }
            for winding in fragments {
                let winding = native_winding(winding, polygon.plane, planes, budget, control)
                    .map_err(context)?;
                budget.consume(winding.0.len(), control).map_err(context)?;
                if winding
                    .0
                    .iter()
                    .any(|point| point.cmplt(mins).any() || point.cmpgt(maxs).any())
                {
                    return Err(context("Occluder boundary exceeds its native source bounds; separate or simplify the brushes".into()));
                }
                append_boundary(
                    &mut output,
                    &mut vertices,
                    Polygon {
                        plane: polygon.plane,
                        winding,
                    },
                )
                .map_err(context)?;
            }
        }
        grouped.push(output);
    }
    Ok(grouped)
}
