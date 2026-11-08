use std::collections::{BTreeMap, BTreeSet};

use glam::{DVec3, Vec3};
use source_formats::{
    bsp,
    vmf::{DispInfo, Solid, Vmf},
};

use super::{grid_clip, native, neighbors};
use crate::surface::FaceId;
use crate::{BrushGeometry, EditError, Face};

pub(crate) struct Replacement<'a> {
    pub(crate) source: FaceId,
    pub(crate) parts: &'a [Solid],
}

struct Source<'a> {
    face: Face,
    info: &'a DispInfo,
    bounds: [Vec3; 2],
    selected: bool,
}

struct Surface {
    power: u32,
    corners: [DVec3; 4],
    positions: Vec<DVec3>,
}

impl Surface {
    fn grid(&self) -> neighbors::SurfaceGrid<'_> {
        neighbors::SurfaceGrid {
            power: self.power,
            corners: self.corners,
            positions: &self.positions,
        }
    }
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn bounds(face: &Face) -> Result<[Vec3; 2], EditError> {
    if face.vertices.is_empty() {
        return Err(invalid("Native terrain topology requires base face points"));
    }
    let mut points = Vec::with_capacity(face.vertices.len());
    for &point in &face.vertices {
        let point = native::stored_vertex(point).as_vec3();
        if !point.is_finite() {
            return Err(invalid(
                "Native terrain topology needs finite stored corners",
            ));
        }
        points.push(point);
    }
    Ok(neighbors::native_bounds(&points))
}

fn collect<'a>(
    solid: &'a Solid,
    origin: Option<FaceId>,
    selected: &BTreeSet<FaceId>,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<Source<'a>>, EditError> {
    let work = solid
        .sides
        .len()
        .checked_pow(3)
        .ok_or_else(|| invalid("Native terrain topology work overflow"))?;
    consume(work).map_err(invalid)?;
    let geometry = BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
    let mut result = Vec::new();
    for face in geometry.faces {
        let side = &solid.sides[face.side];
        let Some(info) = side.displacement.as_ref() else {
            continue;
        };
        let source = FaceId {
            solid: origin.map_or(solid.id, |source| source.solid),
            side: side.id,
        };
        result.push(Source {
            bounds: bounds(&face)?,
            face,
            info,
            selected: selected.contains(&source),
        });
    }
    Ok(result)
}

fn prepare(
    source: &Source<'_>,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Surface, EditError> {
    if !(2..=4).contains(&source.info.power) {
        return Err(invalid("Native terrain topology requires power 2, 3 or 4"));
    }
    let size = (1usize << source.info.power) + 1;
    consume(256 * size * size).map_err(invalid)?;
    let mesh = native::stored_mesh(&source.face, source.info).map_err(invalid)?;
    let corners = [0, (size - 1) * size, size * size - 1, size - 1]
        .map(|index| native::stored_vertex(mesh.base_positions[index]));
    Ok(Surface {
        power: source.info.power,
        corners,
        positions: mesh.positions,
    })
}

fn edge_key(pair: &neighbors::NeighborPair) -> [usize; 6] {
    let span = |span| match span {
        neighbors::Span::Full => 0,
        neighbors::Span::FirstHalf => 1,
        neighbors::Span::SecondHalf => 2,
    };
    let a = [pair.first, pair.first_edge, span(pair.first_span)];
    let b = [pair.second, pair.second_edge, span(pair.second_span)];
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    [a[0], a[1], a[2], b[0], b[1], b[2]]
}

fn topology_error(source: FaceId, reason: impl std::fmt::Display) -> EditError {
    invalid(format!(
        "Terrain brush {} face {} cannot preserve exact native neighbors: {reason}. Align base corners and shared samples before editing",
        source.solid, source.side,
    ))
}

pub(crate) fn check(
    vmf: &Vmf,
    selected: &[FaceId],
    replacements: &[Replacement<'_>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(), EditError> {
    if selected.is_empty() {
        return Ok(());
    }
    let selected = selected.iter().copied().collect::<BTreeSet<_>>();
    if !native::visible_terrain_sources(vmf).any(|(solid, supported)| {
        supported
            && solid.sides.iter().any(|side| {
                side.displacement.is_some()
                    && selected.contains(&FaceId {
                        solid: solid.id,
                        side: side.id,
                    })
            })
    }) {
        return Ok(());
    }
    if vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
    }) {
        return Err(invalid(
            "Exact terrain proof requires complete resolved visible-instance context",
        ));
    }
    let mut changed = BTreeMap::new();
    for replacement in replacements {
        if !selected.contains(&replacement.source)
            || changed
                .insert(replacement.source.solid, replacement)
                .is_some()
        {
            return Err(invalid(
                "Native terrain topology lost unique replacement sources",
            ));
        }
    }
    let mut sources = Vec::new();
    let mut seeds = Vec::new();
    for (solid, _) in native::visible_terrain_sources(vmf).filter(|(_, supported)| *supported) {
        let original = collect(solid, None, &selected, consume)?;
        seeds.extend(
            original
                .iter()
                .filter(|source| source.selected)
                .map(|source| source.bounds),
        );
        if let Some(replacement) = changed.get(&solid.id) {
            for part in replacement.parts.iter().filter(|part| !part.hidden) {
                sources.extend(collect(part, Some(replacement.source), &selected, consume)?);
            }
        } else {
            sources.extend(original);
        }
        if sources.len() > bsp::MAX_DISPLACEMENTS {
            return Err(invalid(
                "Native terrain topology exceeds the displacement limit",
            ));
        }
    }
    let mut included = sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| source.selected.then_some(index))
        .collect::<BTreeSet<_>>();
    let selected_count = included.len();
    for (index, source) in sources.iter().enumerate() {
        consume(seeds.len()).map_err(invalid)?;
        if seeds
            .iter()
            .any(|&seed| neighbors::native_bounds_touch(seed, source.bounds))
        {
            included.insert(index);
        }
    }
    let mut queue = included.iter().copied().collect::<Vec<_>>();
    let mut cursor = 0;
    while cursor < queue.len() {
        let first = queue[cursor];
        cursor += 1;
        for (index, source) in sources.iter().enumerate() {
            consume(1).map_err(invalid)?;
            if !included.contains(&index)
                && neighbors::native_bounds_touch(sources[first].bounds, source.bounds)
            {
                included.insert(index);
                queue.push(index);
            }
        }
        if included.len() > selected_count + grid_clip::MAX_NEIGHBORS {
            return Err(invalid(
                "Exact terrain proof exceeds its 128-neighbor native BASE-box context limit",
            ));
        }
    }
    if included.is_empty() {
        return Ok(());
    }
    let surfaces = included
        .iter()
        .map(|&index| prepare(&sources[index], consume))
        .collect::<Result<Vec<_>, _>>()?;
    let grids = surfaces.iter().map(Surface::grid).collect::<Vec<_>>();
    let source = *selected
        .first()
        .ok_or_else(|| invalid("Native terrain topology lost its selected source"))?;
    let exact =
        neighbors::find_spans(&grids, consume).map_err(|error| topology_error(source, error))?;
    let exact_connections = neighbors::native_connections(&grids, &exact.pairs, consume)
        .map_err(|error| topology_error(source, error))?;
    let (actual, actual_connections) = neighbors::native_neighborhood(&grids, consume)
        .map_err(|error| topology_error(source, error))?;
    let exact_edges = exact.pairs.iter().map(edge_key).collect::<BTreeSet<_>>();
    let actual_edges = actual.pairs.iter().map(edge_key).collect::<BTreeSet<_>>();
    consume(exact_edges.len() + actual_edges.len() + 12 * grids.len()).map_err(invalid)?;
    if exact.ambiguous != 0
        || exact.mismatched != 0
        || actual.ambiguous != 0
        || actual.mismatched != 0
        || exact_edges != actual_edges
        || exact_connections.edges != actual_connections.edges
        || exact_connections.corners != actual_connections.corners
    {
        let reason = if exact.ambiguous != 0
            || exact.mismatched != 0
            || actual.ambiguous != 0
            || actual.mismatched != 0
        {
            "neighbor discovery is ambiguous or shared samples disagree"
        } else if exact_edges != actual_edges || exact_connections.edges != actual_connections.edges
        {
            "native edge records differ from the exact edit graph"
        } else {
            "native corner records differ from the exact edit graph"
        };
        return Err(topology_error(source, reason));
    }
    Ok(())
}
