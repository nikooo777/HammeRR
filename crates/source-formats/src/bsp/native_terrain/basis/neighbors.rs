use std::io;

use crate::bsp::{
    DisplacementInfo, DisplacementNeighbor, INVALID_DISPLACEMENT_NEIGHBOR, MAX_DISPLACEMENTS,
    invalid,
};

use super::{Arithmetic, NormalizeOrder, Patch, Work, stage_digest};

const EDGE_DIMS: [usize; 4] = [0, 1, 0, 1];
const EDGE_FIXED_MAXIMUM: [i32; 4] = [0, 1, 1, 0];
const MAX_GATHERED_NEIGHBORS: usize = 24;
const FIXED_PERCENT_SCALE: i32 = 1 << 16;
const CORNER_MATCH_DISTANCE: f32 = f32::from_bits(0x3dcc_cccd);
const INITIAL_CORNER_DISTANCE: f32 = f32::from_bits(0x6753_c21c);
const NORMALIZE_WORK: usize = 20;
const BASIS_WRITE_WORK: usize = 9;
const CORNER_SEARCH_WORK: usize = 4 * 9;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct NeighborReceipt {
    pub t_junction_events: usize,
    pub corner_events: usize,
    pub edge_pairs: usize,
    pub fine_samples: usize,
    pub t_junction_digest: u32,
    pub corner_digest: u32,
}

#[derive(Clone, Copy)]
struct CommonSample {
    source: [i32; 2],
    peer: [i32; 2],
}

struct EdgeWalk {
    sample: CommonSample,
    source_step: i32,
    peer_step: [i32; 2],
    free_dim: usize,
    end: i32,
    source_cells: i32,
    peer_cells: i32,
}

pub(super) fn apply_neighbors(
    patches: &mut [Patch],
    fp: &Arithmetic,
    work: &mut Work<'_>,
) -> io::Result<NeighborReceipt> {
    validate_catalog(patches, work)?;
    let mut receipt = NeighborReceipt::default();
    blend_t_junctions(patches, fp, work, &mut receipt)?;
    charge_digest(patches, work)?;
    work.checkpoint()?;
    receipt.t_junction_digest = stage_digest(patches);
    blend_corners(patches, fp, work, &mut receipt)?;
    charge_digest(patches, work)?;
    work.checkpoint()?;
    receipt.corner_digest = stage_digest(patches);
    blend_edges(patches, fp, work, &mut receipt)?;
    work.checkpoint()?;
    Ok(receipt)
}

fn charge_digest(patches: &[Patch], work: &mut Work<'_>) -> io::Result<()> {
    let count = patches
        .iter()
        .map(|patch| patch.positions.len())
        .sum::<usize>();
    work.charge(count * 12)
}

fn validate_catalog(patches: &[Patch], work: &mut Work<'_>) -> io::Result<()> {
    if patches.len() > MAX_DISPLACEMENTS {
        return Err(invalid(
            "Native basis neighbor catalog exceeds displacement bounds",
        ));
    }
    for (ordinal, patch) in patches.iter().enumerate() {
        work.checkpoint()?;
        let info = patch.source.info();
        if usize::from(patch.source.ordinal()) != ordinal || !(2..=4).contains(&info.power) {
            return Err(invalid(
                "Native basis neighbor catalog order or power differs",
            ));
        }
        let count = patch.source.vertex_count();
        if patch.positions.len() != count
            || patch.normals.len() != count
            || patch.tangent_s.len() != count
            || patch.tangent_t.len() != count
            || patch
                .positions
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
            || patch
                .source
                .base_quad()
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(invalid(
                "Native basis neighbor arrays or finite BASE identity differ",
            ));
        }
        work.charge(count)?;
        for neighbor in info.edges.into_iter().flatten() {
            if neighbor.index == INVALID_DISPLACEMENT_NEIGHBOR {
                continue;
            }
            if usize::from(neighbor.index) >= patches.len()
                || neighbor.orientation > 3
                || power_shift(neighbor).is_err()
            {
                return Err(invalid(
                    "Native basis edge neighbor reference/orientation/span is invalid",
                ));
            }
        }
        for corner in info.corners {
            let count = usize::from(corner.count);
            if count > corner.neighbors.len()
                || corner.neighbors[..count]
                    .iter()
                    .any(|&peer| usize::from(peer) >= patches.len())
            {
                return Err(invalid(
                    "Native basis corner neighbor count or reference is invalid",
                ));
            }
        }
    }
    Ok(())
}

fn blend_t_junctions(
    patches: &mut [Patch],
    fp: &Arithmetic,
    work: &mut Work<'_>,
    receipt: &mut NeighborReceipt,
) -> io::Result<()> {
    for source in 0..patches.len() {
        work.checkpoint()?;
        let info = patches[source].source.info();
        let cells = cells(info.power);
        for (edge, neighbors) in info.edges.into_iter().enumerate() {
            work.charge(1)?;
            if neighbors
                .iter()
                .any(|neighbor| neighbor.index == INVALID_DISPLACEMENT_NEIGHBOR)
            {
                continue;
            }
            let midpoint = edge_midpoint(edge, cells);
            let source_index = grid_index(midpoint, cells)?;
            let position = patches[source].positions[source_index];
            let peers = neighbors.map(|neighbor| usize::from(neighbor.index));
            let first = find_corner(&patches[peers[0]], position, fp, work)?;
            let second = find_corner(&patches[peers[1]], position, fp, work)?;
            let (Some(first), Some(second)) = (first, second) else {
                continue;
            };
            work.charge(12 + 2 * NORMALIZE_WORK)?;
            let normal = t_junction_sum(
                patches[source].normals[source_index],
                patches[peers[0]].normals[first],
                patches[peers[1]].normals[second],
                fp,
            );
            let tangent_s = t_junction_sum(
                patches[source].tangent_s[source_index],
                patches[peers[0]].tangent_s[first],
                patches[peers[1]].tangent_s[second],
                fp,
            );
            let normal = fp.normalize(normal, NormalizeOrder::ZFirst);
            let tangent_s = fp.normalize(tangent_s, NormalizeOrder::ZFirst);
            write_basis(patches, source, source_index, normal, tangent_s, fp, work)?;
            write_basis(patches, peers[0], first, normal, tangent_s, fp, work)?;
            write_basis(patches, peers[1], second, normal, tangent_s, fp, work)?;
            receipt.t_junction_events += 1;
        }
    }
    Ok(())
}

fn blend_corners(
    patches: &mut [Patch],
    fp: &Arithmetic,
    work: &mut Work<'_>,
    receipt: &mut NeighborReceipt,
) -> io::Result<()> {
    for source in 0..patches.len() {
        work.checkpoint()?;
        let info = patches[source].source.info();
        let (neighbors, count) = gather_neighbors(info);
        work.charge(count)?;
        for corner in 0..4 {
            let source_index =
                grid_index(corner_point(corner, cells(info.power)), cells(info.power))?;
            let position = patches[source].positions[source_index];
            let mut normal = patches[source].normals[source_index];
            let mut tangent_s = patches[source].tangent_s[source_index];
            let mut matches = [None; MAX_GATHERED_NEIGHBORS];
            for entry in 0..count {
                let peer = neighbors[entry];
                let Some(index) = find_corner(&patches[peer], position, fp, work)? else {
                    continue;
                };
                work.charge(6)?;
                matches[entry] = Some(index);
                normal = fp.vec_add(normal, patches[peer].normals[index]);
                tangent_s = fp.vec_add(tangent_s, patches[peer].tangent_s[index]);
            }
            work.charge(2 * NORMALIZE_WORK)?;
            let normal = fp.normalize(normal, NormalizeOrder::ZFirst);
            let tangent_s = fp.normalize(tangent_s, NormalizeOrder::ZFirst);
            write_basis(patches, source, source_index, normal, tangent_s, fp, work)?;
            for entry in 0..count {
                if let Some(index) = matches[entry] {
                    write_basis(
                        patches,
                        neighbors[entry],
                        index,
                        normal,
                        tangent_s,
                        fp,
                        work,
                    )?;
                }
            }
            receipt.corner_events += 1;
        }
    }
    Ok(())
}

fn blend_edges(
    patches: &mut [Patch],
    fp: &Arithmetic,
    work: &mut Work<'_>,
    receipt: &mut NeighborReceipt,
) -> io::Result<()> {
    for source in 0..patches.len() {
        work.checkpoint()?;
        let info = patches[source].source.info();
        for (edge, neighbors) in info.edges.into_iter().enumerate() {
            for (subslot, neighbor) in neighbors.into_iter().enumerate() {
                work.checkpoint()?;
                work.charge(1)?;
                if neighbor.index == INVALID_DISPLACEMENT_NEIGHBOR {
                    continue;
                }
                let peer = usize::from(neighbor.index);
                let peer_info = patches[peer].source.info();
                let mut iterator = EdgeWalk::new(info, peer_info, edge, subslot, neighbor)?;
                let Some(mut previous) = iterator.next()? else {
                    continue;
                };
                while let Some(current) = iterator.next()? {
                    work.charge(1)?;
                    let source_index = grid_index(current.source, iterator.source_cells)?;
                    if current.source[iterator.free_dim] != iterator.end {
                        let peer_index = grid_index(current.peer, iterator.peer_cells)?;
                        work.charge(6 + 2 * NORMALIZE_WORK)?;
                        let source_normal = patches[source].normals[source_index];
                        let peer_normal = patches[peer].normals[peer_index];
                        let normal = [
                            fp.add(peer_normal[0], source_normal[0]),
                            fp.add(source_normal[1], peer_normal[1]),
                            fp.add(peer_normal[2], source_normal[2]),
                        ];
                        let tangent_s = fp.vec_add(
                            patches[peer].tangent_s[peer_index],
                            patches[source].tangent_s[source_index],
                        );
                        let normal = fp.normalize(normal, NormalizeOrder::ZFirstYx);
                        let tangent_s = fp.normalize(tangent_s, NormalizeOrder::XyFirst);
                        write_basis(patches, source, source_index, normal, tangent_s, fp, work)?;
                        write_basis(patches, peer, peer_index, normal, tangent_s, fp, work)?;
                        receipt.edge_pairs += 1;
                    }
                    let previous_index = grid_index(previous.source, iterator.source_cells)?;
                    let previous_free = previous.source[iterator.free_dim];
                    let current_free = current.source[iterator.free_dim];
                    work.charge(2)?;
                    let inverse_span =
                        fp.div(1.0, fp.sub(current_free as f32, previous_free as f32));
                    for index in previous_free + 1..current_free {
                        work.charge(2 + 18 + 2 * NORMALIZE_WORK)?;
                        let percent =
                            fp.mul(inverse_span, fp.sub(index as f32, previous_free as f32));
                        let normal = lerp_normal(
                            patches[source].normals[previous_index],
                            patches[source].normals[source_index],
                            percent,
                            fp,
                        );
                        let tangent_s = lerp_tangent_s(
                            patches[source].tangent_s[previous_index],
                            patches[source].tangent_s[source_index],
                            percent,
                            fp,
                        );
                        let normal = fp.normalize(normal, NormalizeOrder::ZFirst);
                        let tangent_s = fp.normalize(tangent_s, NormalizeOrder::ZFirst);
                        let mut point = current.source;
                        point[iterator.free_dim] = index;
                        let index = grid_index(point, iterator.source_cells)?;
                        write_basis(patches, source, index, normal, tangent_s, fp, work)?;
                        receipt.fine_samples += 1;
                    }
                    previous = current;
                }
            }
        }
    }
    Ok(())
}

fn gather_neighbors(info: DisplacementInfo) -> ([usize; MAX_GATHERED_NEIGHBORS], usize) {
    let mut neighbors = [0; MAX_GATHERED_NEIGHBORS];
    let mut count = 0;
    for corner in info.corners {
        for peer in &corner.neighbors[..usize::from(corner.count)] {
            neighbors[count] = usize::from(*peer);
            count += 1;
        }
    }
    for peer in info.edges.into_iter().flatten() {
        if peer.index != INVALID_DISPLACEMENT_NEIGHBOR {
            neighbors[count] = usize::from(peer.index);
            count += 1;
        }
    }
    (neighbors, count)
}

fn find_corner(
    patch: &Patch,
    point: [f32; 3],
    fp: &Arithmetic,
    work: &mut Work<'_>,
) -> io::Result<Option<usize>> {
    work.charge(CORNER_SEARCH_WORK)?;
    let cells = cells(patch.source.power());
    let mut closest = INITIAL_CORNER_DISTANCE;
    let mut selected = 0;
    for corner in 0..4 {
        let index = grid_index(corner_point(corner, cells), cells)?;
        let difference = fp.vec_sub(patch.positions[index], point);
        let x = fp.mul(difference[0], difference[0]);
        let y = fp.mul(difference[1], difference[1]);
        let z = fp.mul(difference[2], difference[2]);
        let distance = fp.sqrt(fp.add(z, fp.add(x, y)));
        if distance < closest {
            selected = index;
        }
        if distance <= closest {
            closest = distance;
        }
    }
    Ok((closest <= CORNER_MATCH_DISTANCE).then_some(selected))
}

fn write_basis(
    patches: &mut [Patch],
    patch: usize,
    index: usize,
    normal: [f32; 3],
    tangent_s: [f32; 3],
    fp: &Arithmetic,
    work: &mut Work<'_>,
) -> io::Result<()> {
    work.charge(BASIS_WRITE_WORK)?;
    patches[patch].normals[index] = normal;
    patches[patch].tangent_s[index] = tangent_s;
    patches[patch].tangent_t[index] = [
        fp.sub(
            fp.mul(normal[2], tangent_s[1]),
            fp.mul(tangent_s[2], normal[1]),
        ),
        fp.sub(
            fp.mul(tangent_s[2], normal[0]),
            fp.mul(normal[2], tangent_s[0]),
        ),
        fp.sub(
            fp.mul(normal[1], tangent_s[0]),
            fp.mul(tangent_s[1], normal[0]),
        ),
    ];
    Ok(())
}

fn t_junction_sum(
    source: [f32; 3],
    first: [f32; 3],
    second: [f32; 3],
    fp: &Arithmetic,
) -> [f32; 3] {
    let sum = fp.vec_add(source, first);
    [
        fp.add(sum[0], second[0]),
        fp.add(sum[1], second[1]),
        fp.add(second[2], sum[2]),
    ]
}

fn lerp_normal(first: [f32; 3], last: [f32; 3], percent: f32, fp: &Arithmetic) -> [f32; 3] {
    let scaled = scaled_difference(first, last, percent, fp);
    [
        fp.add(scaled[0], first[0]),
        fp.add(scaled[1], first[1]),
        fp.add(first[2], scaled[2]),
    ]
}

fn lerp_tangent_s(first: [f32; 3], last: [f32; 3], percent: f32, fp: &Arithmetic) -> [f32; 3] {
    let scaled = scaled_difference(first, last, percent, fp);
    [
        fp.add(first[0], scaled[0]),
        fp.add(scaled[1], first[1]),
        fp.add(first[2], scaled[2]),
    ]
}

fn scaled_difference(first: [f32; 3], last: [f32; 3], percent: f32, fp: &Arithmetic) -> [f32; 3] {
    std::array::from_fn(|axis| fp.mul(fp.sub(last[axis], first[axis]), percent))
}

const fn cells(power: i32) -> i32 {
    1 << power
}

const fn corner_point(corner: usize, cells: i32) -> [i32; 2] {
    match corner {
        0 => [0, 0],
        1 => [0, cells],
        2 => [cells, cells],
        _ => [cells, 0],
    }
}

const fn edge_midpoint(edge: usize, cells: i32) -> [i32; 2] {
    let midpoint = cells / 2;
    match edge {
        0 => [0, midpoint],
        1 => [midpoint, cells],
        2 => [cells, midpoint],
        _ => [midpoint, 0],
    }
}

fn grid_index(point: [i32; 2], cells: i32) -> io::Result<usize> {
    if point
        .into_iter()
        .any(|coordinate| !(0..=cells).contains(&coordinate))
    {
        return Err(invalid("Native basis neighbor iterator left its grid"));
    }
    Ok((point[1] * (cells + 1) + point[0]) as usize)
}

fn power_shift(neighbor: DisplacementNeighbor) -> io::Result<i32> {
    match (neighbor.span, neighbor.neighbor_span) {
        (0, 0) => Ok(0),
        (0, 1 | 2) => Ok(-1),
        (1 | 2, 0) => Ok(1),
        _ => Err(invalid(
            "Native basis neighbor span combination is unsupported",
        )),
    }
}

fn span_ends(power: i32, edge: usize, span: u8) -> ([i32; 2], [i32; 2]) {
    let cells = cells(power);
    let free = 1 - EDGE_DIMS[edge];
    let mut start = corner_point(edge, cells);
    let mut end = corner_point((edge + 1) & 3, cells);
    match (edge >= 2, span) {
        (true, 1) | (false, 2) => start[free] = cells / 2,
        (true, 2) | (false, 1) => end[free] = cells / 2,
        _ => {}
    }
    (start, end)
}

fn rotate_increment(increment: [i32; 2], orientation: u8) -> [i32; 2] {
    let [x, y] = increment;
    match orientation {
        0 => [x, y],
        1 => [y, -x],
        2 => [-x, -y],
        _ => [-y, x],
    }
}

impl EdgeWalk {
    fn new(
        source: DisplacementInfo,
        peer: DisplacementInfo,
        edge: usize,
        subslot: usize,
        neighbor: DisplacementNeighbor,
    ) -> io::Result<Self> {
        let source_cells = cells(source.power);
        let peer_cells = cells(peer.power);
        let fixed = EDGE_DIMS[edge];
        let free_dim = 1 - fixed;
        let mut source_point = [0; 2];
        source_point[fixed] = EDGE_FIXED_MAXIMUM[edge] * source_cells;
        source_point[free_dim] = source_cells / 2 * subslot as i32;
        let (source_start, source_end) = span_ends(source.power, edge, neighbor.span);
        let denominator = source_end[free_dim] - source_start[free_dim];
        let percent =
            (source_point[free_dim] - source_start[free_dim]) * FIXED_PERCENT_SCALE / denominator;
        if !(0..=FIXED_PERCENT_SCALE).contains(&percent) {
            return Err(invalid(
                "Native basis neighbor subslot lies outside its span",
            ));
        }
        let peer_edge = (edge + 2 + usize::from(neighbor.orientation)) & 3;
        let (peer_end, peer_start) = span_ends(peer.power, peer_edge, neighbor.neighbor_span);
        let peer_fixed = EDGE_DIMS[peer_edge];
        let peer_free = 1 - peer_fixed;
        let mut peer_point = peer_start;
        peer_point[peer_free] +=
            (peer_end[peer_free] - peer_start[peer_free]) * percent / FIXED_PERCENT_SCALE;
        let effective_power = peer.power + power_shift(neighbor)?;
        let source_step = 1 << (source.power - effective_power).max(0);
        let mut peer_step = [0; 2];
        peer_step[free_dim] = 1 << (effective_power - source.power).max(0);
        let peer_step = rotate_increment(peer_step, neighbor.orientation);
        let end = if neighbor.span == 1 {
            source_cells / 2
        } else {
            source_cells
        };
        if source_point[free_dim] > end || (end - source_point[free_dim]) % source_step != 0 {
            return Err(invalid(
                "Native basis neighbor common-sample span is unsupported",
            ));
        }
        grid_index(source_point, source_cells)?;
        grid_index(peer_point, peer_cells)?;
        Ok(Self {
            sample: CommonSample {
                source: source_point,
                peer: peer_point,
            },
            source_step,
            peer_step,
            free_dim,
            end,
            source_cells,
            peer_cells,
        })
    }

    fn next(&mut self) -> io::Result<Option<CommonSample>> {
        if self.sample.source[self.free_dim] > self.end {
            return Ok(None);
        }
        grid_index(self.sample.source, self.source_cells)?;
        grid_index(self.sample.peer, self.peer_cells)?;
        let current = self.sample;
        self.sample.source[self.free_dim] += self.source_step;
        for axis in 0..2 {
            self.sample.peer[axis] += self.peer_step[axis];
        }
        Ok(Some(current))
    }
}
