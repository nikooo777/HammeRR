use std::collections::HashMap;

use glam::DVec3;
use map_core::{BrushGeometry, subobject};
use source_formats::bsp::contents;
use source_formats::vmf::{Entity, Solid};

use crate::map::{Map, origins};
use crate::{Control, Diagnostic, Report};

const MAX_HELPERS: usize = 16;
const MAX_SIDES: usize = 64;
const MAX_INTERSECTION_PLANES: usize = 128;
const MAX_POINTS: usize = 2048;
const POINT_EPSILON: f64 = 1e-6;
const MIN_VOLUME: f64 = 1e-6;
const ORIENTATION_EPSILON: f64 = 1e-8;
const RELATIVE_VOLUME_MARGIN: f64 = 1e-10;
const OVERLAP_FRACTION: f64 = 0.1;
const GEOMETRY_MARGIN: f64 = crate::winding::ON_EPSILON * 2.0;

pub(crate) struct Definition {
    entity: u32,
    solid: u32,
    geometry: BrushGeometry,
}

#[derive(Default)]
pub(crate) struct Summary {
    pub solved_clusters: usize,
    pub helpers: usize,
    pub groups: usize,
    pub grouped_leaves: usize,
}

pub(crate) struct Partition {
    pub mapping: Vec<usize>,
    pub count: usize,
    pub summary: Summary,
}

fn clipping_work(sides: usize) -> usize {
    sides * sides * (sides + 8)
}

pub(crate) fn source_solid(entity: &Entity) -> Result<Option<&Solid>, String> {
    let mut visible = entity.solids.iter().filter(|solid| !solid.hidden);
    let Some(solid) = visible.next() else {
        return if entity.solids.is_empty() {
            Err(
                "Visclusters require one visible convex brush per helper and at most 16 helpers"
                    .into(),
            )
        } else {
            Ok(None)
        };
    };
    if visible.next().is_some() {
        return Err(
            "Visclusters require one visible convex brush per helper and at most 16 helpers".into(),
        );
    }
    if solid.sides.len() > MAX_SIDES {
        return Err("Viscluster brushes support at most 64 sides".into());
    }
    Ok(Some(solid))
}

pub(crate) fn source_rules(entity: &Entity, solid: &Solid) -> Result<(), String> {
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        return Err(
            "Visclusters require ordinary convex geometry without terrain or origin materials"
                .into(),
        );
    }
    for (key, value) in entity.properties.iter() {
        if key.eq_ignore_ascii_case("parentname") && !value.trim().is_empty() {
            return Err(
                "Viscluster volumes are fixed in authored world space; remove the parent".into(),
            );
        }
        if key.eq_ignore_ascii_case("angles")
            && source_formats::fgd::vector(value, 3)
                .is_none_or(|values| values.iter().any(|&value| value != 0.0))
        {
            return Err(
                "Viscluster entity angles are unsupported; transform the brush geometry instead"
                    .into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn hull_work(sides: usize) -> usize {
    clipping_work(sides) * 2 + MAX_POINTS * MAX_POINTS
}

pub(crate) fn checked_hull(solid: &Solid, geometry: &BrushGeometry) -> Result<usize, String> {
    let point_count = geometry
        .faces
        .iter()
        .map(|face| face.vertices.len())
        .sum::<usize>();
    if point_count > MAX_POINTS {
        return Err("Viscluster input exceeds its 2048-point hull budget".into());
    }
    subobject::topology(solid)
        .map_err(|error| format!("Viscluster needs closed convex geometry: {error}"))?;
    measure(geometry)?;
    Ok(MAX_POINTS * MAX_POINTS - point_count * point_count)
}

pub(crate) fn load(
    map: &mut Map<'_>,
    entity: &Entity,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    if !entity.solids.is_empty() && entity.solids.iter().all(|solid| solid.hidden) {
        return Ok(());
    }
    let result: Result<Option<Definition>, String> = (|| {
        control.checkpoint()?;
        if map.viscluster_attempts >= MAX_HELPERS {
            return Err(
                "Viscluster input exceeds the 16-helper admission limit, including invalid helpers"
                    .into(),
            );
        }
        map.viscluster_attempts += 1;
        let Some(solid) = source_solid(entity)? else {
            return Ok(None);
        };
        let reserved = hull_work(solid.sides.len());
        map.viscluster_work = map
            .viscluster_work
            .checked_add(reserved)
            .filter(|&work| work <= crate::visibility::MAX_VISIBILITY_WORK)
            .ok_or("Viscluster input exhausted the shared visibility work budget")?;
        source_rules(entity, solid)?;
        if origins::origin_material(&mut map.materials, solid, control)? {
            return Err(
                "Visclusters require ordinary convex geometry without terrain or origin materials"
                    .into(),
            );
        }
        control.checkpoint()?;
        let Some(geometry) = origins::checked_geometry(solid, report) else {
            return Ok(None);
        };
        let refund = checked_hull(solid, &geometry)?;
        control.checkpoint()?;
        map.viscluster_work -= refund;
        Ok(Some(Definition {
            entity: entity.id,
            solid: solid.id,
            geometry,
        }))
    })();
    match result {
        Ok(Some(definition)) => map.visclusters.push(definition),
        Ok(None) => {}
        Err(message) => {
            control.checkpoint()?;
            report.diagnostics.push(Diagnostic::error(
                message,
                Some(format!("entity:{}", entity.id)),
                entity.origin(),
            ));
        }
    }
    Ok(())
}

struct Measurement {
    volume: f64,
    area: f64,
}

fn separated_edges(
    a: &BrushGeometry,
    b: &BrushGeometry,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<bool, String> {
    let mut points = Vec::new();
    let mut directions = Vec::new();
    for geometry in [a, b] {
        let count = geometry
            .faces
            .iter()
            .map(|face| face.vertices.len())
            .sum::<usize>();
        consume(count * count + count)?;
        let mut edges = Vec::<DVec3>::new();
        for face in &geometry.faces {
            for index in 0..face.vertices.len() {
                let delta = face.vertices[(index + 1) % face.vertices.len()] - face.vertices[index];
                let Some(direction) = delta.try_normalize() else {
                    return Err("Viscluster separating-axis check has a collapsed edge".into());
                };
                if !edges
                    .iter()
                    .any(|other| other.cross(direction).length_squared() < 1e-12)
                {
                    edges.push(direction);
                }
            }
        }
        points.push(
            geometry
                .faces
                .iter()
                .flat_map(|face| &face.vertices)
                .copied()
                .collect::<Vec<_>>(),
        );
        directions.push(edges);
    }
    for a in &directions[0] {
        for b in &directions[1] {
            consume(1)?;
            let cross = a.cross(*b);
            if cross.length_squared() < 1e-12 {
                continue;
            }
            let Some(axis) = cross.try_normalize() else {
                continue;
            };
            consume(points[0].len() + points[1].len())?;
            let range = |points: &[DVec3]| {
                points
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(low, high), point| {
                        let value = point.dot(axis);
                        (low.min(value), high.max(value))
                    })
            };
            let (a_low, a_high) = range(&points[0]);
            let (b_low, b_high) = range(&points[1]);
            if a_low.max(b_low) - a_high.min(b_high) > GEOMETRY_MARGIN {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn measure(geometry: &BrushGeometry) -> Result<Measurement, String> {
    let count = geometry
        .faces
        .iter()
        .map(|face| face.vertices.len())
        .sum::<usize>();
    if geometry.faces.len() < 4 || !(12..=MAX_POINTS).contains(&count) {
        return Err("Viscluster hull exceeds its point budget or has no closed volume".into());
    }
    let center = geometry
        .faces
        .iter()
        .flat_map(|face| &face.vertices)
        .copied()
        .sum::<DVec3>()
        / count as f64;
    let mut vertices = Vec::<DVec3>::new();
    let mut edges = HashMap::<(usize, usize), usize>::new();
    let mut volume = 0.0;
    let mut area = 0.0;
    for face in &geometry.faces {
        if face.vertices.len() < 3 || !face.plane.normal.is_finite() || !face.plane.dist.is_finite()
        {
            return Err("Viscluster hull has invalid faces".into());
        }
        let mut ring = Vec::new();
        for &point in &face.vertices {
            if !point.is_finite()
                || geometry
                    .faces
                    .iter()
                    .any(|other| other.plane.distance_to(point) > GEOMETRY_MARGIN)
            {
                return Err("Viscluster hull has uncertain half-space support".into());
            }
            let index = vertices
                .iter()
                .position(|other| other.distance_squared(point) <= POINT_EPSILON * POINT_EPSILON)
                .unwrap_or_else(|| {
                    vertices.push(point);
                    vertices.len() - 1
                });
            ring.push(index);
        }
        for index in 0..ring.len() {
            let edge = (ring[index], ring[(index + 1) % ring.len()]);
            if edge.0 == edge.1 {
                return Err("Viscluster hull has a collapsed edge".into());
            }
            *edges.entry(edge).or_default() += 1;
        }
        let a = face.vertices[0] - center;
        for pair in face.vertices[1..].windows(2) {
            let cross = (pair[1] - center).cross(pair[0] - center);
            let tetrahedron = a.dot(cross) / 6.0;
            if tetrahedron < -ORIENTATION_EPSILON {
                return Err("Viscluster hull has inconsistent face orientation".into());
            }
            volume += tetrahedron;
            area += (pair[1] - face.vertices[0])
                .cross(pair[0] - face.vertices[0])
                .length()
                * 0.5;
        }
    }
    if edges
        .iter()
        .any(|(&(a, b), &count)| count != 1 || edges.get(&(b, a)) != Some(&1))
        || vertices.len() + geometry.faces.len() != edges.len() / 2 + 2
        || !volume.is_finite()
        || volume <= MIN_VOLUME
        || !area.is_finite()
    {
        return Err("Viscluster hull has open, degenerate or numerically uncertain volume".into());
    }
    Ok(Measurement { volume, area })
}

fn claims(
    helper: &BrushGeometry,
    leaf: &BrushGeometry,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<bool, String> {
    if helper.maxs.cmple(leaf.mins).any() || leaf.maxs.cmple(helper.mins).any() {
        return Ok(false);
    }
    let planes = helper
        .faces
        .iter()
        .chain(&leaf.faces)
        .map(|face| face.plane)
        .collect::<Vec<_>>();
    if planes.len() > MAX_INTERSECTION_PLANES {
        return Err("Viscluster intersection exceeds its 128-plane budget".into());
    }
    let points = leaf
        .faces
        .iter()
        .map(|face| face.vertices.len())
        .sum::<usize>();
    consume(clipping_work(planes.len()) + points * points)?;
    let base = measure(leaf)?;
    if helper.faces.iter().any(|face| {
        leaf.faces
            .iter()
            .flat_map(|side| &side.vertices)
            .all(|&point| face.plane.distance_to(point) > GEOMETRY_MARGIN)
    }) || leaf.faces.iter().any(|face| {
        helper
            .faces
            .iter()
            .flat_map(|side| &side.vertices)
            .all(|&point| face.plane.distance_to(point) > GEOMETRY_MARGIN)
    }) {
        return Ok(false);
    }
    let intersection = match BrushGeometry::from_planes(&planes) {
        Ok(intersection) => intersection,
        Err(_) if separated_edges(helper, leaf, consume)? => return Ok(false),
        Err(_) => return Err(
            "Viscluster intersection is empty or numerically uncertain; separate the boundaries"
                .into(),
        ),
    };
    let intersection_points = intersection
        .faces
        .iter()
        .map(|face| face.vertices.len())
        .sum::<usize>();
    if intersection_points > MAX_POINTS {
        return Err("Viscluster intersection exceeds its 2048-point budget".into());
    }
    consume(intersection_points * intersection_points)?;
    let overlap = measure(&intersection)?;
    if overlap.volume > base.volume + base.area * GEOMETRY_MARGIN {
        return Err("Viscluster intersection exceeds its leaf volume".into());
    }
    let difference = overlap.volume - base.volume * OVERLAP_FRACTION;
    let uncertainty = (overlap.area + base.area * OVERLAP_FRACTION) * GEOMETRY_MARGIN
        + base.volume * RELATIVE_VOLUME_MARGIN;
    if difference.abs() <= uncertainty {
        return Err("Viscluster overlap is numerically uncertain at the strict 10% leaf-volume threshold; move its boundary".into());
    }
    Ok(difference > 0.0)
}

#[derive(Clone, Copy)]
pub(crate) struct Context<'a> {
    pub map: &'a Map<'a>,
    pub tree: &'a crate::tree::Tree,
    pub areas: &'a crate::areas::Areas,
    pub water: &'a crate::water::Water,
    pub control: &'a Control,
}

fn helper_error(report: &mut Report, helper: &Definition, message: &str) {
    report.diagnostics.push(Diagnostic::error(
        message,
        Some(format!("entity:{}", helper.entity)),
        Some(helper.geometry.center()),
    ));
}

pub(crate) fn partition(
    context: Context<'_>,
    clusters: &[Option<usize>],
    count: usize,
    report: &mut Report,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Partition, String> {
    let Context {
        map,
        tree,
        areas,
        water,
        control,
    } = context;
    let mut claims_by_leaf = vec![None; count];
    let mut members = vec![0; map.visclusters.len()];
    let mut helper_areas = vec![None; map.visclusters.len()];
    for node in tree.preorder() {
        control.checkpoint()?;
        let Some(cluster) = clusters[node] else {
            continue;
        };
        let volume = &tree.nodes[node].volume;
        let (mins, maxs) = volume.bounds();
        for (index, helper) in map.visclusters.iter().enumerate() {
            consume(1)?;
            if helper.geometry.maxs.cmple(mins).any() || maxs.cmple(helper.geometry.mins).any() {
                continue;
            }
            let result = (|| -> Result<bool, String> {
                if volume.sides.len() + helper.geometry.faces.len() > MAX_INTERSECTION_PLANES {
                    return Err("Viscluster intersection exceeds its 128-plane budget".into());
                }
                let point_count = volume
                    .sides
                    .iter()
                    .map(|side| side.winding.0.len())
                    .sum::<usize>();
                if point_count > MAX_POINTS {
                    return Err("Viscluster leaf exceeds its 2048-point budget".into());
                }
                consume(point_count)?;
                let leaf = BrushGeometry {
                    faces: volume
                        .sides
                        .iter()
                        .enumerate()
                        .map(|(side, face)| map_core::brush::Face {
                            side,
                            plane: map_core::Plane {
                                normal: map.planes.get(face.plane).normal,
                                dist: map.planes.get(face.plane).dist,
                            },
                            vertices: face.winding.0.clone(),
                        })
                        .collect(),
                    mins,
                    maxs,
                };
                claims(&helper.geometry, &leaf, consume)
            })()
            .map_err(|message| {
                let message = format!(
                    "func_viscluster entity:{} solid:{}: {message}",
                    helper.entity, helper.solid
                );
                helper_error(report, helper, &message);
                message
            })?;
            if !result {
                continue;
            }
            if let Some(previous) = claims_by_leaf[cluster] {
                let previous: &Definition = &map.visclusters[previous];
                let message = format!(
                    "Viscluster entity:{} and entity:{} both qualify for leaf cluster {cluster}; separate their overlap claims",
                    previous.entity, helper.entity
                );
                helper_error(report, previous, &message);
                helper_error(report, helper, &message);
                return Err(message);
            }
            if tree.nodes[node].contents
                & (contents::WATER | contents::SLIME | contents::AREAPORTAL)
                != 0
                || water.leaves[node] >= 0
            {
                let message = format!(
                    "Viscluster entity:{} selects liquid or areaportal leaves; only dry volumes are supported",
                    helper.entity
                );
                helper_error(report, helper, &message);
                return Err(message);
            }
            let area = areas.indices[node];
            if helper_areas[index].is_some_and(|previous| previous != area) {
                let message = format!(
                    "Viscluster entity:{} crosses visibility areas; use separate helpers",
                    helper.entity
                );
                helper_error(report, helper, &message);
                return Err(message);
            }
            helper_areas[index] = Some(area);
            claims_by_leaf[cluster] = Some(index);
            members[index] += 1;
        }
    }
    let mut ids = vec![None; map.visclusters.len()];
    let mut mapping = Vec::with_capacity(count);
    let mut count = 0;
    for claim in claims_by_leaf {
        consume(1)?;
        let id = if let Some(helper) = claim {
            *ids[helper].get_or_insert_with(|| {
                let id = count;
                count += 1;
                id
            })
        } else {
            let id = count;
            count += 1;
            id
        };
        mapping.push(id);
    }
    for (helper, &members) in map.visclusters.iter().zip(&members) {
        if members <= 1 {
            report.diagnostics.push(Diagnostic::warning(
                format!(
                    "Viscluster selects {members} open leaves and does not reduce cluster count"
                ),
                Some(format!("entity:{}", helper.entity)),
                Some(helper.geometry.center()),
            ));
        }
    }
    Ok(Partition {
        summary: Summary {
            solved_clusters: mapping.len(),
            helpers: map.visclusters.len(),
            groups: members.iter().filter(|&&count| count > 1).count(),
            grouped_leaves: members.iter().filter(|&&count| count > 1).sum(),
        },
        mapping,
        count,
    })
}
