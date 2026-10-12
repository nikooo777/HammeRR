use super::*;

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
    pub vertices: [u32; 4],
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
                || part.vertices.iter().any(|id| *id >= 6)
                || part.vertices.into_iter().collect::<BTreeSet<_>>().len() != 4
            {
                return Err(fail());
            }
            let points = part.vertices.map(|id| self.vertices[id as usize].position);
            let size = area(&points, plane.normal);
            if !size.is_finite() || size <= MIN_EDGE * MIN_EDGE {
                return Err(fail());
            }
            for index in 0..4 {
                let cross = (points[(index + 1) % 4] - points[index])
                    .cross(points[(index + 2) % 4] - points[(index + 1) % 4]);
                if cross.dot(plane.normal) >= -MIN_EDGE * MIN_EDGE {
                    return Err(fail());
                }
                *directed
                    .entry([part.vertices[index], part.vertices[(index + 1) % 4]])
                    .or_insert(0) += 1;
            }
            total += size;
        }
        let boundary = [
            [0, 4],
            [4, 1],
            [1, 2],
            [2, 5],
            [5, 3],
            [3, 0],
            [4, 5],
            [5, 4],
        ];
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
    if requests.is_empty() || requests.len() > MAX_SOLIDS {
        return Err(invalid(
            "Select one quad face in each of 1–16 brushes to partition",
        ));
    }
    let mut sorted = BTreeMap::new();
    for request in requests {
        if sorted.insert(request.solid, request).is_some() {
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
        let mut request = request.clone();
        request.edges.sort_unstable();
        let pair = shape
            .crease_pairs(request.side)?
            .into_iter()
            .find(|pair| pair.edges == request.edges)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {id}: choose opposing source edges to partition"
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
        let mut vertices = ring
            .iter()
            .enumerate()
            .map(|(index, vertex)| PartitionVertex {
                id: index as u32,
                position: shape.vertices[*vertex].position,
                source: VertexSource::Corner(shape.vertices[*vertex].sides.clone()),
            })
            .collect::<Vec<_>>();
        for (index, endpoints) in [[ring[0], ring[1]], [ring[2], ring[3]]]
            .into_iter()
            .enumerate()
        {
            let mut corners = endpoints.map(|vertex| shape.vertices[vertex].sides.clone());
            corners.sort();
            vertices.push(PartitionVertex {
                id: (index + 4) as u32,
                position: pair.midpoints[index],
                source: VertexSource::Edge {
                    corners,
                    parameter: 0.5,
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
        let partition = Partition {
            source: request,
            owner,
            plane,
            vertices,
            loops: [
                PartitionLoop {
                    id: 0,
                    vertices: [0, 4, 5, 3],
                    source_side: face.side,
                },
                PartitionLoop {
                    id: 1,
                    vertices: [4, 1, 2, 5],
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

    fn fixture(oblique: bool) -> (Document, u32) {
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
                1 => changed.loops[1].vertices = changed.loops[0].vertices,
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
