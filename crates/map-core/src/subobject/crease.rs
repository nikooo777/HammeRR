use super::*;

pub const MAX_BOUNDARY_SOURCE_CORNERS: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct CreasePair {
    pub edges: [u32; 2],
    pub midpoints: [DVec3; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Crease {
    pub solid: u32,
    pub side: u32,
    pub edges: [u32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreaseChange {
    pub source: Crease,
    pub positions: [DVec3; 2],
    pub new_side: u32,
    pub selection: Vec<Component>,
}

#[derive(Clone, PartialEq)]
pub struct CreasePlan {
    pub replacements: Vec<Replacement>,
    pub changes: Vec<CreaseChange>,
}

pub fn crease_pairs(source: &Solid, side: u32) -> Result<Vec<CreasePair>, EditError> {
    let shape = topology(source)?;
    shape.crease_pairs(side)
}

impl Topology {
    fn quad_boundary_edges(&self, side: u32) -> Result<Vec<(u32, DVec3)>, EditError> {
        self.bounded_boundary_edges(side, 4..=4, "crease")
    }

    fn bounded_boundary_edges(
        &self,
        side: u32,
        corners: std::ops::RangeInclusive<usize>,
        operation: &str,
    ) -> Result<Vec<(u32, DVec3)>, EditError> {
        let shape = self;
        let face = shape
            .faces
            .iter()
            .find(|face| face.side == side)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {}: {operation} face {side} does not exist",
                    shape.solid
                ))
            })?;
        if !corners.contains(&face.vertices.len()) {
            if corners != (4..=4) {
                return Err(invalid(format!(
                    "Brush {}: boundary choices require 3–8 source corners",
                    shape.solid
                )));
            }
            return Err(invalid(format!(
                "Brush {}: a crease requires a face with exactly four corners",
                shape.solid
            )));
        }
        let mut edges = Vec::new();
        for index in 0..face.vertices.len() {
            let mut vertices = [
                face.vertices[index],
                face.vertices[(index + 1) % face.vertices.len()],
            ];
            vertices.sort_unstable();
            let edge = shape
                .edges
                .iter()
                .find(|edge| {
                    let mut pair = edge.vertices;
                    pair.sort_unstable();
                    pair == vertices
                })
                .unwrap();
            let neighbor = *edge.sides.iter().find(|id| **id != side).unwrap();
            let midpoint = vertices
                .map(|index| shape.vertices[index].position)
                .iter()
                .sum::<DVec3>()
                * 0.5;
            edges.push((neighbor, midpoint));
        }
        Ok(edges)
    }

    pub fn crease_pairs(&self, side: u32) -> Result<Vec<CreasePair>, EditError> {
        let edges = self.quad_boundary_edges(side)?;
        let mut choices = Vec::new();
        for index in 0..2 {
            let mut pair = [edges[index], edges[index + 2]];
            pair.sort_by_key(|edge| edge.0);
            choices.push(CreasePair {
                edges: pair.map(|edge| edge.0),
                midpoints: pair.map(|edge| edge.1),
            });
        }
        choices.sort_by_key(|pair| pair.edges);
        Ok(choices)
    }

    pub fn partition_pairs(&self, side: u32) -> Result<Vec<CreasePair>, EditError> {
        self.polygon_partition_pairs(side)
    }

    pub fn polygon_partition_pairs(&self, side: u32) -> Result<Vec<CreasePair>, EditError> {
        Ok(boundary_pair_choices(&self.bounded_boundary_edges(
            side,
            3..=MAX_BOUNDARY_SOURCE_CORNERS,
            "partition",
        )?))
    }
}

fn boundary_pair_choices(edges: &[(u32, DVec3)]) -> Vec<CreasePair> {
    let mut choices = Vec::new();
    if edges.len() == 4 {
        for index in 0..2 {
            let mut pair = [edges[index], edges[index + 2]];
            pair.sort_by_key(|edge| edge.0);
            choices.push(CreasePair {
                edges: pair.map(|edge| edge.0),
                midpoints: pair.map(|edge| edge.1),
            });
        }
        choices.sort_by_key(|pair| pair.edges);
    }
    let mut remaining = Vec::new();
    for first in 0..edges.len() {
        for second in first + 1..edges.len() {
            let mut pair = [edges[first], edges[second]];
            pair.sort_by_key(|edge| edge.0);
            if choices
                .iter()
                .any(|choice| choice.edges == pair.map(|edge| edge.0))
            {
                continue;
            }
            remaining.push(CreasePair {
                edges: pair.map(|edge| edge.0),
                midpoints: pair.map(|edge| edge.1),
            });
        }
    }
    remaining.sort_by_key(|pair| pair.edges);
    choices.extend(remaining);
    choices
}

pub fn crease(
    tx: &mut Transaction,
    requests: &[Crease],
    rise: f64,
) -> Result<CreasePlan, EditError> {
    crease_with_roof_textures(tx, requests, rise, false)
}

pub fn crease_with_roof_textures(
    tx: &mut Transaction,
    requests: &[Crease],
    rise: f64,
    roof_texture_lock: bool,
) -> Result<CreasePlan, EditError> {
    crease_roofs(tx, requests, rise, roof_texture_lock, &BTreeMap::new())
}

pub(super) fn crease_partition_roofs(
    tx: &mut Transaction,
    partitions: &[Partition],
    rise: f64,
    roof_texture_lock: bool,
) -> Result<CreasePlan, EditError> {
    if partitions
        .iter()
        .any(|part| part.loops.iter().any(|piece| piece.vertices.len() != 4))
    {
        return Err(invalid(
            "Shared ridges require two quad roof pieces; keep or extrude these cuts",
        ));
    }
    let requests = partitions
        .iter()
        .map(|part| part.source.clone())
        .collect::<Vec<_>>();
    let endpoints = partitions
        .iter()
        .filter(|part| part.cut().unwrap().edge_parameters != [0.5; 2])
        .map(|part| {
            (
                part.source.solid,
                [part.vertices[4].position, part.vertices[5].position],
            )
        })
        .collect();
    crease_roofs(tx, &requests, rise, roof_texture_lock, &endpoints)
}

fn crease_roofs(
    tx: &mut Transaction,
    requests: &[Crease],
    rise: f64,
    roof_texture_lock: bool,
    endpoints: &BTreeMap<u32, [DVec3; 2]>,
) -> Result<CreasePlan, EditError> {
    if requests.is_empty() || requests.len() > MAX_SOLIDS {
        return Err(invalid(
            "Select one quad face in each of 1–16 brushes to crease",
        ));
    }
    if !rise.is_finite() || !(MIN_SPLIT_RISE..=MAX_SPLIT_RISE).contains(&rise) {
        return Err(invalid(
            "Ridge rise must be finite and between 0.125 and 4096 units",
        ));
    }
    let mut sources = BTreeMap::new();
    for request in requests {
        let (owner, source) = solid(tx.document(), request.solid)?;
        if sources
            .insert(request.solid, (owner, source.clone(), request.clone()))
            .is_some()
        {
            return Err(invalid(format!(
                "Brush {}: select exactly one quad face to crease",
                request.solid
            )));
        }
    }
    let mut pending = Vec::new();
    for (id, (owner, source, mut request)) in sources {
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: creasing displacements is unsupported"
            )));
        }
        if source.sides.len() >= MAX_SIDES {
            return Err(invalid(format!(
                "Brush {id}: crease exceeds the 64-face limit"
            )));
        }
        let shape = topology(&source)?;
        request.edges.sort_unstable();
        let pair = shape
            .crease_pairs(request.side)?
            .into_iter()
            .find(|pair| pair.edges == request.edges)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {id}: choose an opposing edge pair from the source face"
                ))
            })?;
        let original = source
            .sides
            .iter()
            .find(|side| side.id == request.side)
            .unwrap();
        let normal = Plane::from_points(original.plane.0).unwrap().normal;
        let flat_positions = endpoints.get(&id).copied().unwrap_or(pair.midpoints);
        let positions = flat_positions.map(|point| point + normal * rise);
        if positions
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
        {
            return Err(invalid(format!(
                "Brush {id}: ridge corners exceed ±65536 units"
            )));
        }
        for side in source.sides.iter().filter(|side| side.id != request.side) {
            let plane = Plane::from_points(side.plane.0).unwrap();
            if positions
                .iter()
                .any(|point| plane.distance_to(*point) > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: ridge crosses a neighboring face",
                    side.id
                )));
            }
        }
        let face = shape
            .faces
            .iter()
            .find(|face| face.side == request.side)
            .unwrap();
        let mut ring = face
            .vertices
            .iter()
            .map(|index| shape.vertices[*index].position)
            .collect::<Vec<_>>();
        let start = (0..4)
            .find(|index| {
                ((ring[*index] + ring[(*index + 1) % 4]) * 0.5).distance(pair.midpoints[0])
                    < EPSILON
            })
            .unwrap();
        ring.rotate_left(start);
        let roofs = [
            [ring[0], positions[0], positions[1], ring[3]],
            [positions[0], ring[1], ring[2], positions[1]],
        ];
        let flat_roofs = [
            [ring[0], flat_positions[0], flat_positions[1], ring[3]],
            [flat_positions[0], ring[1], ring[2], flat_positions[1]],
        ];
        let points = shape
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .chain(positions)
            .collect::<Vec<_>>();
        let temporary = (0..=MAX_SIDES as u32)
            .find(|id| source.sides.iter().all(|side| side.id != *id))
            .unwrap();
        let mut replacement = source.clone();
        let index = replacement
            .sides
            .iter()
            .position(|side| side.id == request.side)
            .unwrap();
        let mut extra = original.clone();
        extra.id = temporary;
        replacement.sides.insert(index + 1, extra);
        for (roof_index, roof) in roofs.iter().enumerate() {
            let triple = [roof[0], roof[1], roof[2]];
            let plane = Plane::from_points(triple).ok_or_else(|| {
                invalid(format!("Brush {id}: crease creates a collapsed roof face"))
            })?;
            if roof
                .iter()
                .any(|point| plane.distance_to(*point).abs() > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}: ridge creates a nonplanar roof; align the cut endpoints, reset to midpoint or extrude"
                )));
            }
            if points
                .iter()
                .any(|point| plane.distance_to(*point) > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}: crease makes the brush concave or inverted"
                )));
            }
            replacement.sides[index + roof_index].plane.0 = triple;
            if roof_texture_lock {
                let projection =
                    crate::texture_deform::attach(original, &flat_roofs[roof_index], roof)
                        .map_err(|error| {
                            invalid(format!(
                                "Brush {}, face {}: {error}",
                                request.solid, original.id
                            ))
                        })?;
                replacement.sides[index + roof_index].u_axis = projection.u;
                replacement.sides[index + roof_index].v_axis = projection.v;
            }
        }
        let after = topology(&replacement)?;
        if after.vertices.len() != points.len()
            || points.iter().any(|point| {
                !after
                    .vertices
                    .iter()
                    .any(|vertex| vertex.position.distance(*point) < EPSILON * 10.0)
            })
        {
            return Err(invalid(format!(
                "Brush {id}: crease changes existing corners or enclosed geometry"
            )));
        }
        let selection = positions
            .iter()
            .map(|point| Component {
                solid: id,
                sides: after
                    .vertices
                    .iter()
                    .find(|vertex| vertex.position.distance(*point) < EPSILON * 10.0)
                    .unwrap()
                    .sides
                    .clone(),
            })
            .collect::<Vec<_>>();
        pending.push((
            Replacement {
                owner,
                solid: replacement,
            },
            request,
            positions,
            index + 1,
            temporary,
            selection,
        ));
    }
    let mut plan = CreasePlan {
        replacements: Vec::new(),
        changes: Vec::new(),
    };
    for (mut replacement, source, positions, index, temporary, mut selection) in pending {
        let new_side = tx.new_side_id()?;
        replacement.solid.sides[index].id = new_side;
        for component in &mut selection {
            for side in &mut component.sides {
                if *side == temporary {
                    *side = new_side;
                }
            }
            component.sides.sort_unstable();
        }
        plan.changes.push(CreaseChange {
            source,
            positions,
            new_side,
            selection,
        });
        plan.replacements.push(replacement);
    }
    apply(tx, &plan.replacements)?;
    Ok(plan)
}
