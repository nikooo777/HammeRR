use super::super::window;
use super::*;
use crate::displacement::resample;

const SOURCE_POWERS: std::ops::RangeInclusive<u32> = 2..=4;

struct Layout {
    cut: Option<Cut>,
    windows: Vec<window::Window>,
}

struct Cut {
    rows: bool,
    index: usize,
    first_back: bool,
}

fn layout(
    surface: &Surface<'_>,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<Option<Layout>, EditError> {
    if !SOURCE_POWERS.contains(&surface.power) || surface.mesh.size != (1 << surface.power) + 1 {
        return Ok(None);
    }
    let Some((plane, keep)) = cut else {
        let Some(windows) = window::whole(surface.power) else {
            return Ok(None);
        };
        work.consume(windows.len()).map_err(invalid)?;
        return Ok(Some(Layout { cut: None, windows }));
    };
    for rows in [true, false] {
        for index in 1..surface.mesh.size - 1 {
            work.consume((surface.positions.len() + 1) * surface.mesh.positions.len())
                .map_err(invalid)?;
            let Some(first_back) = grid_clip::grid_line(
                &surface.mesh.positions,
                surface.mesh.size,
                plane,
                rows,
                index,
            ) else {
                continue;
            };
            if !surface.positions.iter().all(|positions| {
                grid_clip::grid_line(positions, surface.mesh.size, plane, rows, index)
                    == Some(first_back)
            }) {
                continue;
            }
            let Some(windows) = window::cut_children(surface.power, rows, index, first_back, keep)
            else {
                continue;
            };
            work.consume(windows.len()).map_err(invalid)?;
            return Ok(Some(Layout {
                cut: Some(Cut {
                    rows,
                    index,
                    first_back,
                }),
                windows,
            }));
        }
    }
    Ok(None)
}

pub(super) fn candidate(
    surface: &Surface<'_>,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<bool, EditError> {
    Ok(layout(surface, cut, work)?.is_some())
}

fn span_bounds(
    surface: &Surface<'_>,
    edge: usize,
    span: neighbors::Span,
    work: &mut Work,
) -> Result<[usize; 2], EditError> {
    let indices = span.indices(surface.power, edge).map_err(invalid)?;
    work.consume(indices.len()).map_err(invalid)?;
    let steps = indices.len() - 1;
    let start = if span == neighbors::Span::SecondHalf {
        steps
    } else {
        0
    };
    Ok([start, start + steps])
}

fn native_span([start, end]: [usize; 2], extent: usize) -> Option<neighbors::Span> {
    if start == 0 && end == extent {
        Some(neighbors::Span::Full)
    } else if start == 0 && end == extent / 2 {
        Some(neighbors::Span::FirstHalf)
    } else if start == extent / 2 && end == extent {
        Some(neighbors::Span::SecondHalf)
    } else {
        None
    }
}

pub(super) fn requires_partition(
    surface: &Surface<'_>,
    cut: Option<(Plane, Keep)>,
    (edge, span): (usize, neighbors::Span),
    peer: &Surface<'_>,
    (peer_edge, peer_span): (usize, neighbors::Span),
    work: &mut Work,
) -> Result<bool, EditError> {
    let layout = layout(surface, cut, work)?
        .ok_or_else(|| invalid("Native window edge closure lost its source layout"))?;
    let source_range = span_bounds(surface, edge, span, work)?;
    let peer_range = span_bounds(peer, peer_edge, peer_span, work)?;
    let extent = surface.mesh.size - 1;
    let peer_extent = peer.mesh.size - 1;
    let source_steps = source_range[1] - source_range[0];
    let peer_steps = peer_range[1] - peer_range[0];
    work.consume(32 * layout.windows.len()).map_err(invalid)?;
    for window in layout.windows {
        let (interval, edge_cells) = match edge {
            0 if window.column == 0 => (
                [window.row, window.row + window.row_cells],
                window.row_cells,
            ),
            1 if window.row + window.row_cells == extent => (
                [window.column, window.column + window.column_cells],
                window.column_cells,
            ),
            2 if window.column + window.column_cells == extent => (
                [extent - window.row - window.row_cells, extent - window.row],
                window.row_cells,
            ),
            3 if window.row == 0 => (
                [
                    extent - window.column - window.column_cells,
                    extent - window.column,
                ],
                window.column_cells,
            ),
            _ => continue,
        };
        let overlap = [
            interval[0].max(source_range[0]),
            interval[1].min(source_range[1]),
        ];
        if overlap[0] > overlap[1] {
            continue;
        }
        let mut mapped = [0; 2];
        for (output, point) in mapped.iter_mut().zip(overlap) {
            let numerator = (point - source_range[0]) * peer_steps;
            if numerator % source_steps != 0 {
                return Ok(true);
            }
            *output = peer_range[1] - numerator / source_steps;
        }
        if overlap[0] == overlap[1] {
            if mapped[0] != 0 && mapped[0] != peer_extent {
                return Ok(true);
            }
            continue;
        }
        let child_span = native_span(overlap.map(|point| point - interval[0]), edge_cells);
        let peer_span = native_span([mapped[1], mapped[0]], peer_extent);
        if !matches!(
            (child_span, peer_span),
            (Some(neighbors::Span::Full), Some(_)) | (Some(_), Some(neighbors::Span::Full))
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn movement(
    surface: &Surface<'_>,
    state: &LodState,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<bool, EditError> {
    if let Some(cut) = cut {
        return lod_proof::retained_movement(
            surface.raw.side.displacement.as_ref().unwrap(),
            &surface.mesh,
            state.proof(surface),
            cut,
            work,
        );
    }
    work.consume(
        surface.mesh.positions.len() + surface.positions.iter().map(Vec::len).sum::<usize>(),
    )
    .map_err(invalid)?;
    Ok(state.ideal_positions != surface.mesh.positions || state.positions != surface.positions)
}

pub(super) fn native_movement(
    document: &Document,
    source: FaceId,
    cut: Option<(Plane, Keep)>,
    context: &mut Context<'_>,
) -> Result<bool, EditError> {
    let (owner, solid) = crate::subobject::solid(document, source.solid)?;
    let side = solid
        .sides
        .iter()
        .find(|side| side.id == source.side && side.displacement.is_some())
        .ok_or_else(|| invalid("Terrain window classifier lost its source face"))?;
    if solid
        .sides
        .iter()
        .filter(|side| side.displacement.is_some())
        .count()
        != 1
    {
        return Err(invalid(
            "Terrain window classifier requires one source terrain face",
        ));
    }
    let hidden = solid.hidden
        || match owner {
            ObjectId::Entity(id) => document.entity(id).is_some_and(|entity| entity.hidden),
            _ => false,
        };
    if hidden || !SOURCE_POWERS.contains(&side.displacement.as_ref().unwrap().power) {
        return Ok(false);
    }
    if !native::terrain_source_is_visible(document.vmf(), source.solid) {
        return Err(invalid(
            "Terrain window classifier lost its visible source identity",
        ));
    }
    let vmf = context.vmf.unwrap_or(document.vmf());
    vmf.validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    let work = &mut context.work;
    let before = affected_sources(collect(vmf, work)?, &BTreeSet::from([source]), work)?
        .into_iter()
        .map(|raw| prepare(raw, Mode::Cells, work))
        .collect::<Result<Vec<_>, _>>()?;
    let index = before
        .iter()
        .position(|surface| surface.raw.source == source)
        .ok_or_else(|| invalid("Terrain window classifier lost its resolved source identity"))?;
    let surface = &before[index];
    if layout(surface, cut, work)?.is_none() {
        return Ok(false);
    }
    if context.vmf.is_some() {
        verify_source(document, vmf, surface, Mode::Cells, work)?;
    }
    let references = before.iter().collect::<Vec<_>>();
    let (_, _, patches) = original_lod(&references, work)?;
    let patch = patches
        .into_iter()
        .nth(index)
        .ok_or_else(|| invalid("Terrain window classifier lost its LOD state"))?;
    let state = raw_lod_state(surface, patch, work)?;
    movement(surface, &state, cut, work)
}

pub(super) fn validate(
    before: &[Surface<'_>],
    children: &[Surface<'_>],
    replacements: &[Replacement<'_>],
    mode: Mode,
    work: &mut Work,
) -> Result<BTreeSet<FaceId>, EditError> {
    let mut scoped = BTreeSet::new();
    for replacement in replacements {
        let Some(fields) = replacement.window_fields else {
            continue;
        };
        let windows = fields.layout();
        if mode != Mode::Cells {
            return Err(invalid("Native terrain windows require Cells mode"));
        }
        let source = before
            .iter()
            .find(|surface| surface.raw.source == replacement.source)
            .ok_or_else(|| invalid("Native terrain windows lost their source"))?;
        let expected = layout(source, replacement.cut, work)?.ok_or_else(|| {
            invalid("Native terrain windows require a supported whole or exact original grid-line layout")
        })?;
        let parts = children
            .iter()
            .filter(|surface| surface.raw.source == replacement.source)
            .collect::<Vec<_>>();
        if windows != expected.windows
            || windows.len() != parts.len()
            || parts.len() != replacement.parts.len()
        {
            return Err(invalid(
                "Native terrain windows differ from the complete retained source layout",
            ));
        }
        work.consume(parts.len()).map_err(invalid)?;
        let proof_children = parts
            .iter()
            .map(|child| (child.raw.side.displacement.as_ref().unwrap(), &child.mesh))
            .collect::<Vec<_>>();
        fields.verify(replacement.source, &proof_children, work)?;
        for (window, child) in windows.iter().zip(parts) {
            if child.power != window.output_power
                || child.mesh.size != (1 << window.output_power) + 1
            {
                return Err(invalid(
                    "Native terrain window power differs from its canonical source layout",
                ));
            }
            work.consume(4 * FIELD_WORK * child.mesh.positions.len())
                .map_err(invalid)?;
            let denominator = (child.mesh.size - 1) as i128;
            for row in 0..child.mesh.size {
                for column in 0..child.mesh.size {
                    let (indices, weights) = resample::weights(
                        source.mesh.size,
                        window.row as f64
                            + window.row_cells as f64 * row as f64 / denominator as f64,
                        window.column as f64
                            + window.column_cells as f64 * column as f64 / denominator as f64,
                    );
                    let mut exact_weights = [0; 3];
                    for (output, weight) in exact_weights.iter_mut().zip(weights) {
                        let scaled = weight * denominator as f64;
                        if !scaled.is_finite()
                            || scaled.fract() != 0.0
                            || !(0.0..=denominator as f64).contains(&scaled)
                        {
                            return Err(invalid(
                                "Native window interpolation requires bounded integral dyadic weights",
                            ));
                        }
                        *output = scaled as i128;
                    }
                    if exact_weights.iter().sum::<i128>() != denominator {
                        return Err(invalid(
                            "Native window interpolation weights must sum to their output-grid denominator",
                        ));
                    }
                    let base = weighted(
                        &indices.map(|index| source.mesh.base_positions[index]),
                        &exact_weights,
                        denominator,
                    )?;
                    let point = weighted(
                        &indices.map(|index| source.mesh.positions[index]),
                        &exact_weights,
                        denominator,
                    )?;
                    let index = row * child.mesh.size + column;
                    if !same(base, child.mesh.base_positions[index])
                        || !same(point, child.mesh.positions[index])
                    {
                        return Err(invalid(
                            "Native terrain window changed its exact authored source samples or cage",
                        ));
                    }
                }
            }
        }
        scoped.insert(replacement.source);
    }
    Ok(scoped)
}

pub(super) fn movement_candidate(
    surface: &Surface<'_>,
    state: &LodState,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<bool, EditError> {
    let expected = layout(surface, cut, work)?
        .ok_or_else(|| invalid("Native window source lost its exact layout"))?;
    if !movement(surface, state, cut, work)? {
        return Ok(false);
    }
    let (Some((plane, _)), Some(expected)) = (cut, expected.cut) else {
        return Ok(true);
    };
    work.consume((state.positions.len() + 1) * surface.mesh.positions.len())
        .map_err(invalid)?;
    for positions in std::iter::once(state.ideal_positions.as_slice())
        .chain(state.positions.iter().map(Vec::as_slice))
    {
        if grid_clip::grid_line(
            positions,
            surface.mesh.size,
            plane,
            expected.rows,
            expected.index,
        ) != Some(expected.first_back)
        {
            return Err(invalid(
                "Native window geometry changed its retained cut separation",
            ));
        }
    }
    Ok(true)
}

pub(super) fn certify_source(
    surface: &Surface<'_>,
    state: &LodState,
    cut: Option<(Plane, Keep)>,
    work: &mut Work,
) -> Result<(), EditError> {
    if !movement_candidate(surface, state, cut, work)? {
        return Err(invalid(
            "Native terrain windows require retained original native movement",
        ));
    }
    Ok(())
}
