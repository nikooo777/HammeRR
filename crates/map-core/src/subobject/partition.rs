use super::*;

pub const MAX_PARTITION_VERTICES: usize = MAX_BOUNDARY_SOURCE_CORNERS + 2;
pub const MAX_PARTITION_LOOP_VERTICES: usize = MAX_BOUNDARY_SOURCE_CORNERS + 1;

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionCut {
    pub source: Crease,
    pub edge_parameters: [f64; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub enum VertexSource {
    Corner(Vec<u32>),
    Edge {
        corners: [Vec<u32>; 2],
        parameter: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionVertex {
    pub id: u32,
    pub position: DVec3,
    pub source: VertexSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionLoop {
    pub id: u32,
    pub vertices: Vec<u32>,
    pub source_side: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Partition {
    pub source: Crease,
    pub owner: ObjectId,
    pub plane: [DVec3; 3],
    pub vertices: Vec<PartitionVertex>,
    pub loops: [PartitionLoop; 2],
}

pub fn partition_pairs(source: &Solid, side: u32) -> Result<Vec<CreasePair>, EditError> {
    topology(source)?.partition_pairs(side)
}

pub fn polygon_partition_pairs(source: &Solid, side: u32) -> Result<Vec<CreasePair>, EditError> {
    topology(source)?.polygon_partition_pairs(side)
}

fn area(points: &[DVec3], normal: DVec3) -> f64 {
    let origin = points[0];
    -(1..points.len() - 1)
        .map(|index| {
            (points[index] - origin)
                .cross(points[index + 1] - origin)
                .dot(normal)
        })
        .sum::<f64>()
        * 0.5
}

impl Partition {
    pub fn source_corner_count(&self) -> usize {
        self.vertices.len().saturating_sub(2)
    }

    pub fn cut(&self) -> Result<PartitionCut, EditError> {
        check_partition_bounds(self)?;
        let mut edge_parameters = [0.0; 2];
        for (index, parameter) in edge_parameters.iter_mut().enumerate() {
            let Some(PartitionVertex {
                source:
                    VertexSource::Edge {
                        parameter: value, ..
                    },
                ..
            }) = self.vertices.get(index + self.source_corner_count())
            else {
                return Err(invalid("Partition edge provenance is invalid"));
            };
            *parameter = *value;
        }
        Ok(PartitionCut {
            source: self.source.clone(),
            edge_parameters,
        })
    }

    pub fn normal(&self) -> DVec3 {
        Plane::from_points(self.plane).unwrap().normal
    }

    pub fn positions(&self, rise: f64) -> Vec<DVec3> {
        let offset = self.normal() * rise;
        self.vertices
            .iter()
            .map(|vertex| match vertex.source {
                VertexSource::Corner(_) => vertex.position,
                VertexSource::Edge { .. } => vertex.position + offset,
            })
            .collect()
    }

    fn validate(&self, source_ring: &[DVec3]) -> Result<(), EditError> {
        let fail = || {
            invalid(format!(
                "Brush {}: invalid flat partition coverage",
                self.source.solid
            ))
        };
        let plane = Plane::from_points(self.plane).ok_or_else(fail)?;
        let corners = source_ring.len();
        if !(3..=MAX_BOUNDARY_SOURCE_CORNERS).contains(&corners)
            || self.vertices.len() != corners + 2
            || self.vertices.iter().enumerate().any(|(index, vertex)| {
                vertex.id != index as u32
                    || !vertex.position.is_finite()
                    || vertex.position.abs().max_element() > MAX_COORDINATE
                    || plane.distance_to(vertex.position).abs() > EPSILON
            })
        {
            return Err(fail());
        }
        let mut directed = BTreeMap::new();
        let mut total = 0.0;
        for (index, part) in self.loops.iter().enumerate() {
            if part.id != index as u32
                || part.source_side != self.source.side
                || !(3..=corners + 1).contains(&part.vertices.len())
                || part
                    .vertices
                    .iter()
                    .any(|id| *id as usize >= self.vertices.len())
                || part.vertices.iter().collect::<BTreeSet<_>>().len() != part.vertices.len()
            {
                return Err(fail());
            }
            let points = part
                .vertices
                .iter()
                .map(|id| self.vertices[*id as usize].position)
                .collect::<Vec<_>>();
            let count = points.len();
            let size = area(&points, plane.normal);
            if !size.is_finite() || size <= MIN_EDGE * MIN_EDGE {
                return Err(fail());
            }
            for index in 0..count {
                let cross = (points[(index + 1) % count] - points[index])
                    .cross(points[(index + 2) % count] - points[(index + 1) % count]);
                if cross.dot(plane.normal) >= -MIN_EDGE * MIN_EDGE
                    || (0..count)
                        .filter(|other| *other != index && *other != (index + 1) % count)
                        .any(|other| {
                            (points[(index + 1) % count] - points[index])
                                .cross(points[other] - points[index])
                                .dot(plane.normal)
                                >= -MIN_EDGE * MIN_EDGE
                        })
                {
                    return Err(fail());
                }
                *directed
                    .entry([part.vertices[index], part.vertices[(index + 1) % count]])
                    .or_insert(0) += 1;
            }
            total += size;
        }
        let edge_points = [corners as u32, corners as u32 + 1];
        let mut boundary = vec![edge_points, [edge_points[1], edge_points[0]]];
        for index in 0..corners {
            let corners = [index as u32, ((index + 1) % corners) as u32];
            let endpoint = self.vertices[self.source_corner_count()..]
                .iter()
                .find(|vertex| {
                    let VertexSource::Edge { corners: edge, .. } = &vertex.source else {
                        return false;
                    };
                    let endpoints =
                        corners.map(|corner| match &self.vertices[corner as usize].source {
                            VertexSource::Corner(sides) => sides,
                            _ => unreachable!(),
                        });
                    edge.iter().all(|corner| endpoints.contains(&corner))
                });
            if let Some(endpoint) = endpoint {
                boundary.extend([[corners[0], endpoint.id], [endpoint.id, corners[1]]]);
            } else {
                boundary.push(corners);
            }
        }
        if directed != boundary.into_iter().map(|edge| (edge, 1)).collect()
            || (total - area(source_ring, plane.normal)).abs() > total * 1e-10 + EPSILON
        {
            return Err(fail());
        }
        Ok(())
    }
}

pub fn plan_partitions(
    document: &Document,
    requests: &[Crease],
) -> Result<Vec<Partition>, EditError> {
    if requests.len() > MAX_SOLIDS {
        return Err(invalid(
            "Select one 3–8-corner face in each of 1–16 brushes to partition",
        ));
    }
    plan_partitions_at(
        document,
        &requests
            .iter()
            .map(|source| PartitionCut {
                source: source.clone(),
                edge_parameters: [0.5; 2],
            })
            .collect::<Vec<_>>(),
    )
}

pub(super) fn check_partition_bounds(part: &Partition) -> Result<(), EditError> {
    let corners = part.source_corner_count();
    if !(3..=MAX_BOUNDARY_SOURCE_CORNERS).contains(&corners)
        || part.vertices.len() > MAX_PARTITION_VERTICES
        || part
            .loops
            .iter()
            .any(|piece| !(3..=MAX_PARTITION_LOOP_VERTICES).contains(&piece.vertices.len()))
        || part.vertices.iter().any(|vertex| match &vertex.source {
            VertexSource::Corner(sides) => !(3..=MAX_SIDES).contains(&sides.len()),
            VertexSource::Edge { corners, .. } => corners
                .iter()
                .any(|sides| !(3..=MAX_SIDES).contains(&sides.len())),
        })
        || part.vertices[..corners]
            .iter()
            .any(|vertex| !matches!(vertex.source, VertexSource::Corner(_)))
        || part.vertices[corners..]
            .iter()
            .any(|vertex| !matches!(vertex.source, VertexSource::Edge { .. }))
    {
        return Err(invalid(
            "Partition topology exceeds its bounded source and loop limits",
        ));
    }
    Ok(())
}

pub(crate) fn checked_partition(
    document: &Document,
    part: &Partition,
) -> Result<Partition, EditError> {
    check_partition_bounds(part)?;
    let canonical = plan_partitions_at(document, &[part.cut()?])?.remove(0);
    let mut checked = part.clone();
    checked.loops = canonical.loops.clone();
    let mut reversed = canonical.loops.clone();
    reversed.swap(0, 1);
    reversed[0].id = 0;
    reversed[1].id = 1;
    if checked != canonical || (part.loops != canonical.loops && part.loops != reversed) {
        return Err(invalid(
            "Partition source geometry, ownership or loop incidence changed",
        ));
    }
    Ok(canonical)
}

pub fn plan_partitions_at(
    document: &Document,
    requests: &[PartitionCut],
) -> Result<Vec<Partition>, EditError> {
    if requests.is_empty() || requests.len() > MAX_SOLIDS {
        return Err(invalid(
            "Select one 3–8-corner face in each of 1–16 brushes to partition",
        ));
    }
    let mut sorted = BTreeMap::new();
    for request in requests {
        if request
            .edge_parameters
            .iter()
            .any(|parameter| !parameter.is_finite() || *parameter <= 0.0 || *parameter >= 1.0)
        {
            return Err(invalid(
                "Partition edge positions must be finite and strictly between 0 and 1",
            ));
        }
        if sorted.insert(request.source.solid, request).is_some() {
            return Err(invalid(
                "Select exactly one 3–8-corner face per brush to partition",
            ));
        }
    }
    let mut partitions = Vec::new();
    for (id, request) in sorted {
        let (owner, source) = solid(document, id)?;
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: partitioning displacements is unsupported"
            )));
        }
        let shape = topology(source)?;
        let parameters = request.edge_parameters;
        let mut request = request.source.clone();
        request.edges.sort_unstable();
        let pair = shape
            .polygon_partition_pairs(request.side)?
            .into_iter()
            .find(|pair| pair.edges == request.edges)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {id}: choose two distinct source boundary edges to partition"
                ))
            })?;
        let face = shape
            .faces
            .iter()
            .find(|face| face.side == request.side)
            .unwrap();
        let corners = face.vertices.len();
        let mut ring = face.vertices.clone();
        let start = (0..corners)
            .find(|index| {
                let midpoint = (shape.vertices[ring[*index]].position
                    + shape.vertices[ring[(*index + 1) % corners]].position)
                    * 0.5;
                midpoint.distance(pair.midpoints[0]) < EPSILON
            })
            .ok_or_else(|| invalid(format!("Brush {id}: partition edge incidence changed")))?;
        ring.rotate_left(start);
        let second = (1..corners)
            .find(|index| {
                let midpoint = (shape.vertices[ring[*index]].position
                    + shape.vertices[ring[(*index + 1) % corners]].position)
                    * 0.5;
                midpoint.distance(pair.midpoints[1]) < EPSILON
            })
            .ok_or_else(|| invalid("Partition second edge incidence changed"))?;
        let mut vertices = ring
            .iter()
            .enumerate()
            .map(|(index, vertex)| PartitionVertex {
                id: index as u32,
                position: shape.vertices[*vertex].position,
                source: VertexSource::Corner(shape.vertices[*vertex].sides.clone()),
            })
            .collect::<Vec<_>>();
        for (index, endpoints) in [
            [ring[0], ring[1]],
            [ring[second], ring[(second + 1) % corners]],
        ]
        .into_iter()
        .enumerate()
        {
            let mut incidence = endpoints.map(|vertex| shape.vertices[vertex].sides.clone());
            incidence.sort();
            vertices.push(PartitionVertex {
                id: (index + corners) as u32,
                position: {
                    let endpoints = incidence.each_ref().map(|corner| {
                        shape
                            .vertices
                            .iter()
                            .find(|vertex| &vertex.sides == corner)
                            .unwrap()
                            .position
                    });
                    if parameters[index] == 0.5 {
                        pair.midpoints[index]
                    } else {
                        endpoints[0].lerp(endpoints[1], parameters[index])
                    }
                },
                source: VertexSource::Edge {
                    corners: incidence,
                    parameter: parameters[index],
                },
            });
        }
        let plane = source
            .sides
            .iter()
            .find(|side| side.id == request.side)
            .unwrap()
            .plane
            .0;
        let loops = if corners == 4 {
            match second {
                1 => [vec![4, 1, 5], vec![5, 2, 3, 0, 4]],
                2 => [vec![0, 4, 5, 3], vec![4, 1, 2, 5]],
                3 => [vec![5, 0, 4], vec![4, 1, 2, 3, 5]],
                _ => unreachable!(),
            }
        } else {
            let first = std::iter::once(corners as u32)
                .chain((1..=second).map(|index| index as u32))
                .chain(std::iter::once(corners as u32 + 1))
                .collect();
            let second = std::iter::once(corners as u32 + 1)
                .chain((second + 1..corners).map(|index| index as u32))
                .chain([0, corners as u32])
                .collect();
            [first, second]
        };
        let partition = Partition {
            source: request,
            owner,
            plane,
            vertices,
            loops: [
                PartitionLoop {
                    id: 0,
                    vertices: loops[0].clone(),
                    source_side: face.side,
                },
                PartitionLoop {
                    id: 1,
                    vertices: loops[1].clone(),
                    source_side: face.side,
                },
            ],
        };
        let points = ring
            .iter()
            .map(|vertex| shape.vertices[*vertex].position)
            .collect::<Vec<_>>();
        partition.validate(&points)?;
        partitions.push(partition);
    }
    Ok(partitions)
}
