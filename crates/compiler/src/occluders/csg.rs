use glam::DVec3;
use map_core::{BrushGeometry, subobject};
use source_formats::vmf::Solid;

use super::{MAX_POLYGONS_PER_OCCLUDER, MAX_VERTICES_PER_OCCLUDER, Polygon};
use crate::Control;
use crate::emit::stored_vertex;
use crate::planes::PlaneSet;
use crate::winding::{ON_EPSILON, Side, Winding};

const MAX_WORK: usize = 4_000_000;
const MAX_ALLOCATED_POINTS: usize = 1_000_000;
const MAX_FRAGMENTS: usize = 65_536;
const GEOMETRY_EPSILON: f64 = 1e-7;
const NORMAL_EPSILON: f64 = 1e-10;

#[cfg(test)]
#[path = "csg/tests.rs"]
mod tests;

pub(crate) struct Budget {
    work: usize,
    points: usize,
    fragments: usize,
    max_work: usize,
    max_points: usize,
    max_fragments: usize,
    #[cfg(test)]
    cancel_after_work: Option<usize>,
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
            #[cfg(test)]
            cancel_after_work: None,
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
        #[cfg(test)]
        if self.cancel_after_work.is_some_and(|work| self.work >= work) {
            control.cancel()?;
            control.checkpoint()?;
        }
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

fn separated(a: &BrushGeometry, b: &BrushGeometry) -> bool {
    (a.maxs + DVec3::splat(ON_EPSILON)).cmplt(b.mins).any()
        || (b.maxs + DVec3::splat(ON_EPSILON)).cmplt(a.mins).any()
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
) -> Result<Vec<Vec<usize>>, String> {
    let mut features_by_brush = (0..geometries.len()).map(|_| None).collect::<Vec<_>>();
    let mut neighbors = vec![Vec::new(); geometries.len()];
    for (a, (solid_a, geometry_a)) in geometries.iter().enumerate() {
        for (b, (solid_b, geometry_b)) in geometries.iter().enumerate().skip(a + 1) {
            budget.consume(1, control)?;
            if separated(geometry_a, geometry_b) {
                continue;
            }
            if features_by_brush[a].is_none() {
                features_by_brush[a] = Some(features(solid_a, budget, control)?);
            }
            if features_by_brush[b].is_none() {
                features_by_brush[b] = Some(features(solid_b, budget, control)?);
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
                let Some(overlap) = overlap_on_axis(fa, fb, axis, budget, control)? else {
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
            if minimum <= ON_EPSILON {
                if minimum.abs() > GEOMETRY_EPSILON {
                    return Err("Occluder union has a gap or overlap within 0.01 units of a contact; separate or align the brush faces exactly".into());
                }
                if !face_contact(geometry_a, geometry_b, budget, control)? {
                    return Err("Occluder brushes meet only along an edge or point; use a shared face, overlap their volumes or separate func_occluder entities".into());
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

pub(super) fn boundary(
    geometries: &[(&Solid, BrushGeometry)],
    polygons: Vec<Vec<Polygon>>,
    active_faces: &[Vec<bool>],
    planes: &PlaneSet,
    budget: &mut Budget,
    control: &Control,
) -> Result<Vec<Polygon>, String> {
    let neighbors = interacting(geometries, budget, control)?;
    let mut output = Vec::new();
    let mut vertices = 0;
    for (brush, polygons) in polygons.into_iter().enumerate() {
        let mut faces = geometries[brush]
            .1
            .faces
            .iter()
            .zip(&active_faces[brush])
            .filter_map(|(face, &active)| active.then_some(face));
        for polygon in polygons {
            let face = faces.next().unwrap();
            if neighbors[brush].is_empty() {
                vertices += polygon.winding.0.len();
                output.push(polygon);
                continue;
            }
            let normal = planes.get(polygon.plane).normal;
            budget.reserve(face.vertices.len(), 1, control)?;
            let mut fragments = vec![Winding(face.vertices.clone())];
            for &other in &neighbors[brush] {
                let cutter = &geometries[other].1;
                let keep_coplanar = active_faces[other]
                    .iter()
                    .map(|&active| brush < other || !active)
                    .collect::<Vec<_>>();
                let mut next = Vec::new();
                for winding in fragments {
                    next.extend(subtract(
                        winding,
                        normal,
                        cutter,
                        &keep_coplanar,
                        budget,
                        control,
                    )?);
                    if next.len() > MAX_POLYGONS_PER_OCCLUDER {
                        return Err("Occluder union exceeds its 256 boundary-polygon budget; simplify or split the helper".into());
                    }
                }
                fragments = next;
                if fragments.is_empty() {
                    break;
                }
            }
            for winding in fragments {
                let winding = native_winding(winding, polygon.plane, planes, budget, control)?;
                vertices += winding.0.len();
                if output.len() >= MAX_POLYGONS_PER_OCCLUDER || vertices > MAX_VERTICES_PER_OCCLUDER
                {
                    return Err("Occluder union exceeds its 256 boundary-polygon or 4096 boundary-vertex budget; simplify or split the helper".into());
                }
                output.push(Polygon {
                    plane: polygon.plane,
                    winding,
                });
            }
        }
    }
    if output.len() > MAX_POLYGONS_PER_OCCLUDER || vertices > MAX_VERTICES_PER_OCCLUDER {
        return Err("Occluder union exceeds its 256 boundary-polygon or 4096 boundary-vertex budget; simplify or split the helper".into());
    }
    Ok(output)
}
