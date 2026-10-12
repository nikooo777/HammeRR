use super::*;

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
    pub fn cut(&self) -> Result<PartitionCut, EditError> {
        let mut edge_parameters = [0.0; 2];
        for (index, parameter) in edge_parameters.iter_mut().enumerate() {
            let Some(PartitionVertex {
                source:
                    VertexSource::Edge {
                        parameter: value, ..
                    },
                ..
            }) = self.vertices.get(index + 4)
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
        if self.vertices.len() != 6
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
                || !(3..=5).contains(&part.vertices.len())
                || part.vertices.iter().any(|id| *id >= 6)
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
                if cross.dot(plane.normal) >= -MIN_EDGE * MIN_EDGE {
                    return Err(fail());
                }
                *directed
                    .entry([part.vertices[index], part.vertices[(index + 1) % count]])
                    .or_insert(0) += 1;
            }
            total += size;
        }
        let mut boundary = vec![[4, 5], [5, 4]];
        for index in 0..4 {
            let corners = [index as u32, ((index + 1) % 4) as u32];
            let endpoint = self.vertices[4..].iter().find(|vertex| {
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
            "Select one quad face in each of 1–16 brushes to partition",
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

pub(crate) fn checked_partition(
    document: &Document,
    part: &Partition,
) -> Result<Partition, EditError> {
    if part.vertices.len() != 6
        || part
            .loops
            .iter()
            .any(|piece| !(3..=5).contains(&piece.vertices.len()))
        || part.vertices.iter().any(|vertex| match &vertex.source {
            VertexSource::Corner(sides) => sides.len() > MAX_SIDES,
            VertexSource::Edge { corners, .. } => {
                corners.iter().any(|sides| sides.len() > MAX_SIDES)
            }
        })
    {
        return Err(invalid(
            "Partition topology exceeds its bounded source and loop limits",
        ));
    }
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
            "Select one quad face in each of 1–16 brushes to partition",
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
                "Select exactly one quad face per brush to partition",
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
            .partition_pairs(request.side)?
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
        let mut ring = face.vertices.clone();
        let start = (0..4)
            .find(|index| {
                let midpoint = (shape.vertices[ring[*index]].position
                    + shape.vertices[ring[(*index + 1) % 4]].position)
                    * 0.5;
                midpoint.distance(pair.midpoints[0]) < EPSILON
            })
            .ok_or_else(|| invalid(format!("Brush {id}: partition edge incidence changed")))?;
        ring.rotate_left(start);
        let second = (1..4)
            .find(|index| {
                let midpoint = (shape.vertices[ring[*index]].position
                    + shape.vertices[ring[(*index + 1) % 4]].position)
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
        for (index, endpoints) in [[ring[0], ring[1]], [ring[second], ring[(second + 1) % 4]]]
            .into_iter()
            .enumerate()
        {
            let mut corners = endpoints.map(|vertex| shape.vertices[vertex].sides.clone());
            corners.sort();
            vertices.push(PartitionVertex {
                id: (index + 4) as u32,
                position: {
                    let endpoints = corners.each_ref().map(|corner| {
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
                    corners,
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
        let loops = match second {
            1 => [vec![4, 1, 5], vec![5, 2, 3, 0, 4]],
            2 => [vec![0, 4, 5, 3], vec![4, 1, 2, 5]],
            3 => [vec![5, 0, 4], vec![4, 1, 2, 3, 5]],
            _ => unreachable!(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;

    pub(super) fn fixture(oblique: bool) -> (Document, u32) {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.125, -7.25, 16.5),
                    DVec3::new(64.875, 48.125, 80.25),
                    "brick/test",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        if oblique {
            document
                .edit("oblique", |tx| {
                    edit::transform(
                        tx,
                        &[ObjectId::Solid(id)],
                        edit::Transform {
                            rotation: DVec3::new(17.0, -23.0, 31.0),
                            scale: DVec3::new(1.125, 0.75, 1.5),
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
        }
        (document, id)
    }

    #[test]
    fn adjustable_cuts_cover_quads_and_keep_canonical_edge_provenance() {
        for oblique in [false, true] {
            let (document, id) = fixture(oblique);
            let before = document.vmf().clone();
            let shape = topology(document.solid(id).unwrap()).unwrap();
            for face in &shape.faces {
                for pair in crease_pairs(document.solid(id).unwrap(), face.side).unwrap() {
                    let source = Crease {
                        solid: id,
                        side: face.side,
                        edges: pair.edges,
                    };
                    let midpoint =
                        plan_partitions(&document, std::slice::from_ref(&source)).unwrap();
                    for parameters in [[0.5, 0.5], [0.2, 0.8], [0.7, 0.3], [0.1, 0.1]] {
                        let cut = PartitionCut {
                            source: source.clone(),
                            edge_parameters: parameters,
                        };
                        let partition = plan_partitions_at(&document, std::slice::from_ref(&cut))
                            .unwrap()
                            .remove(0);
                        assert_eq!(partition.cut().unwrap(), cut);
                        assert_eq!(partition.vertices[..4], midpoint[0].vertices[..4]);
                        assert_eq!(partition.loops, midpoint[0].loops);
                        checked_partition(&document, &partition).unwrap();
                        for (index, parameter) in parameters.iter().enumerate() {
                            let VertexSource::Edge { corners, .. } =
                                &partition.vertices[index + 4].source
                            else {
                                unreachable!()
                            };
                            let endpoints = corners.each_ref().map(|corner| {
                                shape
                                    .vertices
                                    .iter()
                                    .find(|vertex| &vertex.sides == corner)
                                    .unwrap()
                                    .position
                            });
                            assert!(
                                partition.vertices[index + 4]
                                    .position
                                    .distance(endpoints[0].lerp(endpoints[1], *parameter))
                                    < EPSILON
                            );
                        }
                        if parameters == [0.5; 2] {
                            assert_eq!(partition, midpoint[0]);
                        }
                    }
                }
            }
            assert_eq!(document.vmf(), &before);
        }
    }

    #[test]
    fn adjustable_cut_bounds_reject_before_any_document_mutation() {
        let (document, id) = fixture(false);
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let side = shape.faces[0].side;
        let cut = PartitionCut {
            source: Crease {
                solid: id,
                side,
                edges: crease_pairs(document.solid(id).unwrap(), side).unwrap()[0].edges,
            },
            edge_parameters: [0.2, 0.8],
        };
        let before = document.vmf().clone();
        for parameter in [f64::NAN, f64::INFINITY, -0.1, 0.0, 1.0, 1.1, 1e-12] {
            let mut invalid = cut.clone();
            invalid.edge_parameters[0] = parameter;
            assert!(plan_partitions_at(&document, &[invalid]).is_err());
        }
        assert!(plan_partitions_at(&document, &[]).is_err());
        assert!(plan_partitions_at(&document, &vec![cut.clone(); MAX_SOLIDS + 1]).is_err());
        assert!(plan_partitions_at(&document, &[cut.clone(), cut]).is_err());
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn both_directions_cover_every_fractional_and_oblique_quad_without_map_edits() {
        for oblique in [false, true] {
            let (document, id) = fixture(oblique);
            let before = document.vmf().clone();
            let revision = document.revision();
            for face in topology(document.solid(id).unwrap()).unwrap().faces {
                for pair in crease_pairs(document.solid(id).unwrap(), face.side).unwrap() {
                    let request = Crease {
                        solid: id,
                        side: face.side,
                        edges: pair.edges,
                    };
                    let plan = plan_partitions(&document, std::slice::from_ref(&request)).unwrap();
                    let partition = &plan[0];
                    assert_eq!(plan_partitions(&document, &[request]).unwrap(), plan);
                    assert_eq!(partition.vertices.len(), 6);
                    assert_eq!(partition.loops[0].vertices, [0, 4, 5, 3]);
                    assert_eq!(partition.loops[1].vertices, [4, 1, 2, 5]);
                    for index in 0..2 {
                        assert_eq!(
                            partition.vertices[index + 4].position,
                            pair.midpoints[index]
                        );
                        let VertexSource::Edge { corners, parameter } =
                            &partition.vertices[index + 4].source
                        else {
                            panic!()
                        };
                        let shape = topology(document.solid(id).unwrap()).unwrap();
                        let points = corners
                            .each_ref()
                            .map(|sides| shape.vertices[shape.resolve(sides).unwrap()[0]].position);
                        assert!(
                            (points[0].lerp(points[1], *parameter) - pair.midpoints[index])
                                .length()
                                < EPSILON
                        );
                    }
                    assert_eq!(
                        partition.positions(0.0),
                        partition
                            .vertices
                            .iter()
                            .map(|vertex| vertex.position)
                            .collect::<Vec<_>>()
                    );
                    let raised = partition.positions(16.25);
                    assert_eq!(&raised[..4], &partition.positions(0.0)[..4]);
                    assert!(
                        (raised[4] - pair.midpoints[0])
                            .abs_diff_eq(partition.normal() * 16.25, EPSILON)
                    );
                }
            }
            assert_eq!(document.vmf(), &before);
            assert_eq!(document.revision(), revision);
            assert_eq!(document.undo_labels().count(), if oblique { 2 } else { 1 });
            let mut allocated = document.clone();
            assert_eq!(
                allocated
                    .edit("allocate", |tx| Ok((
                        tx.new_object_id()?,
                        tx.new_side_id()?
                    )))
                    .unwrap(),
                (3, 7)
            );
        }
    }

    #[test]
    fn flat_partitions_preserve_owned_source_planes_metadata_and_canonical_input_order() {
        let (mut document, id) = fixture(true);
        let owner = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])
            })
            .unwrap();
        let (_, source) = solid(&document, id).unwrap();
        let before = source.clone();
        let side = source.sides[0].id;
        let edges = crease_pairs(source, side).unwrap()[0].edges;
        let first = plan_partitions(
            &document,
            &[Crease {
                solid: id,
                side,
                edges,
            }],
        )
        .unwrap();
        let second = plan_partitions(
            &document,
            &[Crease {
                solid: id,
                side,
                edges: [edges[1], edges[0]],
            }],
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].owner, owner);
        assert_eq!(first[0].plane, before.sides[0].plane.0);
        assert_eq!(solid(&document, id).unwrap().1, &before);
        let ring = first[0].vertices[..4]
            .iter()
            .map(|vertex| vertex.position)
            .collect::<Vec<_>>();
        for invalid in 0..4 {
            let mut changed = first[0].clone();
            match invalid {
                0 => changed.loops[0].vertices.swap(0, 1),
                1 => changed.loops[1].vertices = changed.loops[0].vertices.clone(),
                2 => changed.loops[1].vertices[0] = 9,
                _ => {
                    let normal = changed.normal();
                    changed.vertices[4].position += normal;
                }
            }
            assert!(changed.validate(&ring).is_err());
        }
    }

    #[test]
    fn unsupported_partition_requests_reject_without_consuming_ids() {
        let (document, id) = fixture(false);
        let side = document.solid(id).unwrap().sides[0].id;
        let request = Crease {
            solid: id,
            side,
            edges: crease_pairs(document.solid(id).unwrap(), side).unwrap()[0].edges,
        };
        for requests in [
            vec![],
            vec![request.clone(); 2],
            vec![request.clone(); 17],
            vec![Crease {
                side: u32::MAX,
                ..request.clone()
            }],
            vec![Crease {
                edges: [request.edges[0]; 2],
                ..request
            }],
        ] {
            assert!(plan_partitions(&document, &requests).is_err());
        }
        let mut allocated = document.clone();
        assert_eq!(
            allocated
                .edit("allocate", |tx| Ok((
                    tx.new_object_id()?,
                    tx.new_side_id()?
                )))
                .unwrap(),
            (3, 7)
        );
    }
}

#[cfg(test)]
mod boundary_tests;
