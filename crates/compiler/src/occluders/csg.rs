use glam::DVec3;
use map_core::{BrushGeometry, subobject};
use source_formats::{bsp::MAX_OCCLUDERS, vmf::Solid};

use super::{
    MAX_BRUSHES_PER_OCCLUDER, MAX_POLYGONS_PER_OCCLUDER, MAX_VERTICES_PER_OCCLUDER, Polygon,
};
use crate::Control;
use crate::emit::stored_vertex;
use crate::planes::PlaneSet;
use crate::winding::{ON_EPSILON, Winding};

#[path = "csg/certified.rs"]
mod certified;

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
}

fn bounds_separated(
    sa: &Solid,
    a: &BrushGeometry,
    sb: &Solid,
    b: &BrushGeometry,
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    let (a_mins, a_maxs) = (stored_vertex(a.mins), stored_vertex(a.maxs));
    let (b_mins, b_maxs) = (stored_vertex(b.mins), stored_vertex(b.maxs));
    for axis in 0..3 {
        let mut direction = DVec3::ZERO;
        direction[axis] = 1.0;
        if a_maxs[axis] < b_mins[axis]
            && certified::source_separated(sa, a, sb, b, direction, budget, control)?
        {
            return Ok(true);
        }
        if b_maxs[axis] < a_mins[axis]
            && certified::source_separated(sb, b, sa, a, direction, budget, control)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn literal_box(solid: &Solid, geometry: &BrushGeometry) -> bool {
    if solid.sides.len() != 6 || geometry.faces.len() != 6 {
        return false;
    }
    let mut sides = [false; 6];
    for face in &geometry.faces {
        let Some(axis) = (0..3).find(|&axis| {
            face.plane.normal[axis].abs() == 1.0
                && (0..3).all(|other| other == axis || face.plane.normal[other] == 0.0)
        }) else {
            return false;
        };
        let positive = face.plane.normal[axis] > 0.0;
        let slot = axis * 2 + usize::from(positive);
        if sides[slot] || face.vertices.len() != 4 {
            return false;
        }
        sides[slot] = true;
        let coordinate = if positive {
            geometry.maxs[axis]
        } else {
            geometry.mins[axis]
        };
        let mut corners = [false; 4];
        for point in &face.vertices {
            if point[axis] != coordinate {
                return false;
            }
            let mut corner = 0;
            for other in 0..3 {
                if other != axis {
                    corner *= 2;
                    if point[other] == geometry.maxs[other] {
                        corner += 1;
                    } else if point[other] != geometry.mins[other] {
                        return false;
                    }
                }
            }
            if corners[corner] {
                return false;
            }
            corners[corner] = true;
        }
        let points = solid.sides[face.side].plane.0;
        if points[0] == points[1]
            || points[1] == points[2]
            || points[0] == points[2]
            || points.iter().any(|point| !face.vertices.contains(point))
        {
            return false;
        }
    }
    sides.into_iter().all(|present| present)
}

fn no_area_contact(
    sa: &Solid,
    a: &BrushGeometry,
    sb: &Solid,
    b: &BrushGeometry,
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    let low = a.mins.max(b.mins);
    let high = a.maxs.min(b.maxs);
    if !low.cmple(high).all() || (0..3).filter(|&axis| low[axis] == high[axis]).count() < 2 {
        return Ok(false);
    }
    budget.consume(512, control)?;
    Ok(literal_box(sa, a) && literal_box(sb, b))
}

struct Features {
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
    let mut directions = Vec::<DVec3>::with_capacity(topology.edges.len());
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
    Ok(Features { directions })
}

fn projection(geometry: &BrushGeometry, axis: DVec3) -> (f64, f64) {
    let mut low = f64::MAX;
    let mut high = f64::MIN;
    for point in geometry.faces.iter().flat_map(|face| &face.vertices) {
        let point = stored_vertex(*point);
        let (mut lower, mut upper) = (0.0f64, 0.0f64);
        for coordinate in 0..3 {
            let product = point[coordinate] * axis[coordinate];
            lower = (lower + product.next_down()).next_down();
            upper = (upper + product.next_up()).next_up();
        }
        low = low.min(lower);
        high = high.max(upper);
    }
    (low, high)
}

fn separated_on_axis(
    sa: &Solid,
    a: &BrushGeometry,
    sb: &Solid,
    b: &BrushGeometry,
    axis: DVec3,
    budget: &mut Budget,
    control: &Control,
) -> Result<bool, String> {
    let Some(axis) = axis.try_normalize() else {
        return Ok(false);
    };
    let points = a
        .faces
        .iter()
        .chain(&b.faces)
        .map(|face| face.vertices.len())
        .sum::<usize>();
    budget.consume(points.saturating_mul(20).saturating_add(1), control)?;
    let (anl, anh) = projection(a, axis);
    let (bnl, bnh) = projection(b, axis);
    if anh < bnl && certified::source_separated(sa, a, sb, b, axis, budget, control)? {
        return Ok(true);
    }
    if bnh < anl && certified::source_separated(sb, b, sa, a, -axis, budget, control)? {
        return Ok(true);
    }
    Ok(false)
}

struct Interactions {
    neighbors: Vec<Vec<usize>>,
    brushes: Vec<Option<certified::Brush>>,
}

fn interacting(
    geometries: &[(&Solid, BrushGeometry)],
    budget: &mut Budget,
    control: &Control,
) -> Result<Interactions, Failure> {
    let mut certified = (0..geometries.len()).map(|_| None).collect::<Vec<_>>();
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
            if bounds_separated(solid_a, geometry_a, solid_b, geometry_b, budget, control)
                .map_err(context)?
                || no_area_contact(solid_a, geometry_a, solid_b, geometry_b, budget, control)
                    .map_err(context)?
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
            let mut disjoint = false;
            for axis in face_axes.chain(edge_axes) {
                if separated_on_axis(
                    solid_a, geometry_a, solid_b, geometry_b, axis, budget, control,
                )
                .map_err(context)?
                {
                    disjoint = true;
                    break;
                }
            }
            if disjoint {
                continue;
            }
            if certified[a].is_none() {
                certified[a] = Some(
                    certified::Brush::prepare(solid_a, geometry_a, budget, control)
                        .map_err(context)?,
                );
            }
            if certified[b].is_none() {
                certified[b] = Some(
                    certified::Brush::prepare(solid_b, geometry_b, budget, control)
                        .map_err(context)?,
                );
            }
            match certified::pair(
                certified[a].as_ref().unwrap(),
                certified[b].as_ref().unwrap(),
                budget,
                control,
            )
            .map_err(context)?
            {
                certified::Relation::Separate => continue,
                certified::Relation::Contact(rank) if rank >= 2 => continue,
                _ => {}
            }
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
    }
    Ok(Interactions {
        neighbors,
        brushes: certified,
    })
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
    if winding.is_tiny() {
        return Err("Occluder union has an uncertified tiny native polygon".into());
    }
    native_support(&winding, plane, planes)?;
    Ok(winding)
}

fn native_support(winding: &Winding, plane: usize, planes: &PlaneSet) -> Result<(), String> {
    let support = planes.get(plane);
    let normal = support.normal.as_vec3().as_dvec3();
    let distance = f64::from(support.dist as f32);
    if winding.normal().dot(normal) < 0.99
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
    Ok(())
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
    let interactions = interacting(geometries, budget, control)?;
    let neighbors = &interactions.neighbors;
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
            .enumerate()
            .zip(&active_faces[brush])
            .filter_map(|((index, face), &active)| active.then_some((index, face)));
        for polygon in polygons {
            let (face_index, _) = faces.next().ok_or_else(|| {
                context("Occluder polygon has no matching active source face".to_owned())
            })?;
            if neighbors[brush].is_empty() {
                let winding =
                    native_winding(polygon.winding, polygon.plane, planes, budget, control)
                        .map_err(context)?;
                append_boundary(
                    &mut output,
                    &mut vertices,
                    Polygon {
                        plane: polygon.plane,
                        winding,
                    },
                )
                .map_err(context)?;
                continue;
            }
            {
                let certified = interactions.brushes[brush].as_ref().unwrap();
                budget
                    .reserve(
                        certified.faces[face_index].len().saturating_mul(
                            size_of::<certified::Point>().div_ceil(size_of::<DVec3>()),
                        ),
                        1,
                        control,
                    )
                    .map_err(context)?;
                let mut fragments = vec![certified.faces[face_index].clone()];
                let mut bodies = vec![certified];
                for &other in &neighbors[brush] {
                    let cutter = interactions.brushes[other].as_ref().unwrap();
                    bodies.push(cutter);
                    budget
                        .consume(active_faces[other].len() + 1, control)
                        .map_err(context)?;
                    let keep_coplanar = active_faces[other]
                        .iter()
                        .map(|&active| brush < other || !active)
                        .collect::<Vec<_>>();
                    let mut next = Vec::new();
                    for winding in fragments {
                        next.extend(
                            certified::subtract(
                                winding,
                                &certified.planes[face_index],
                                cutter,
                                &keep_coplanar,
                                budget,
                                control,
                            )
                            .map_err(|message| Failure {
                                message,
                                brushes: [Some(brush), Some(other)],
                            })?,
                        );
                        if next.len() > MAX_POLYGONS_PER_OCCLUDER {
                            return Err(context(
                                "Exact occluder boundary exceeds the existing 256-fragment limit"
                                    .into(),
                            ));
                        }
                    }
                    fragments = next;
                    if fragments.is_empty() {
                        break;
                    }
                }
                for points in fragments {
                    certified::preserves(&points, &bodies, budget, control).map_err(
                        |(index, message)| Failure {
                            message,
                            brushes: [
                                Some(brush),
                                index.checked_sub(1).map(|index| neighbors[brush][index]),
                            ],
                        },
                    )?;
                    let winding =
                        certified::native_winding(points, certified, face_index, budget, control)
                            .map_err(context)?;
                    budget
                        .consume(winding.0.len().saturating_mul(winding.0.len()), control)
                        .map_err(context)?;
                    native_support(&winding, polygon.plane, planes).map_err(context)?;
                    if winding
                        .0
                        .iter()
                        .any(|point| point.cmplt(mins).any() || point.cmpgt(maxs).any())
                    {
                        return Err(context(
                            "Exact occluder boundary exceeds its original native source bounds"
                                .into(),
                        ));
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
        }
        grouped.push(output);
    }
    Ok(grouped)
}
