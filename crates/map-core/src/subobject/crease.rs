use super::*;

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
    pub fn crease_pairs(&self, side: u32) -> Result<Vec<CreasePair>, EditError> {
        let shape = self;
        let face = shape
            .faces
            .iter()
            .find(|face| face.side == side)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {}: crease face {side} does not exist",
                    shape.solid
                ))
            })?;
        if face.vertices.len() != 4 {
            return Err(invalid(format!(
                "Brush {}: a crease requires a face with exactly four corners",
                shape.solid
            )));
        }
        let mut edges = Vec::new();
        for index in 0..4 {
            let mut vertices = [face.vertices[index], face.vertices[(index + 1) % 4]];
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
}

pub fn crease(
    tx: &mut Transaction,
    requests: &[Crease],
    rise: f64,
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
        let positions = pair.midpoints.map(|point| point + normal * rise);
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
                    "Brush {id}: crease creates a nonplanar roof; choose a representable quad"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;

    fn fixture() -> (Document, u32) {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        (document, id)
    }

    fn request(document: &Document, id: u32, side: u32, index: usize) -> Crease {
        Crease {
            solid: id,
            side,
            edges: crease_pairs(solid(document, id).unwrap().1, side).unwrap()[index].edges,
        }
    }

    #[test]
    fn both_opposing_pairs_on_every_box_face_preserve_old_corners_and_select_the_ridge() {
        let (document, id) = fixture();
        let before = topology(document.solid(id).unwrap()).unwrap();
        for face in &before.faces {
            for pair in 0..2 {
                for rise in [MIN_SPLIT_RISE, 16.25, MAX_SPLIT_RISE] {
                    let mut changed = document.clone();
                    let request = request(&document, id, face.side, pair);
                    let plan = changed
                        .edit("crease", |tx| crease(tx, &[request], rise))
                        .unwrap();
                    let after = topology(changed.solid(id).unwrap()).unwrap();
                    assert_eq!(after.faces.len(), 7);
                    assert_eq!(after.vertices.len(), 10);
                    for vertex in &before.vertices {
                        assert!(after.vertices.iter().any(|candidate| {
                            candidate.position.abs_diff_eq(vertex.position, EPSILON)
                        }));
                    }
                    for (component, point) in plan.changes[0]
                        .selection
                        .iter()
                        .zip(plan.changes[0].positions)
                    {
                        let indices = after.resolve(&component.sides).unwrap();
                        assert_eq!(indices.len(), 1);
                        assert!(
                            after.vertices[indices[0]]
                                .position
                                .abs_diff_eq(point, EPSILON)
                        );
                    }
                    assert_eq!(plan.changes[0].new_side, 7);
                    changed.vmf().validate_ids().unwrap();
                }
            }
        }
    }

    #[test]
    fn crease_preserves_ownership_metadata_world_projections_round_trip_and_exact_history() {
        let (mut document, id) = fixture();
        document
            .edit("metadata", |tx| {
                for side in &mut tx.solid_mut(id)?.sides {
                    side.extra_properties.set("vendor", "keep");
                    side.u_axis.offset = 17.123456789;
                    side.v_axis.scale = -0.523456789;
                    side.lightmap_scale = 8;
                    side.smoothing_groups = Some(7);
                }
                Ok(())
            })
            .unwrap();
        let owner = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])
            })
            .unwrap();
        let side = solid(&document, id).unwrap().1.sides[0].id;
        let selected = request(&document, id, side, 0);
        let before = document.vmf().clone();
        let plan = document
            .edit("crease", |tx| crease(tx, &[selected], 16.125))
            .unwrap();
        let result = document.vmf().clone();
        assert_eq!(document.owner_of_solid(id), Some(owner));
        assert_eq!(result.entities[0].properties, before.entities[0].properties);
        for side in &result.entities[0].solids[0].sides {
            let source_id = if side.id == plan.changes[0].new_side {
                plan.changes[0].source.side
            } else {
                side.id
            };
            let mut expected = before.entities[0].solids[0]
                .sides
                .iter()
                .find(|old| old.id == source_id)
                .unwrap()
                .clone();
            expected.id = side.id;
            if source_id == plan.changes[0].source.side {
                expected.plane = side.plane;
            }
            assert_eq!(*side, expected);
        }
        let vmf = source_formats::vmf::Vmf::parse(&result.write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        assert_eq!(
            topology(&vmf.entities[0].solids[0]).unwrap().vertices.len(),
            10
        );
        assert_eq!(
            crease_pairs(&before.entities[0].solids[0], plan.changes[0].source.side).unwrap(),
            crease_pairs(
                &source_formats::vmf::Vmf::parse(&before.write(&Default::default()))
                    .unwrap()
                    .entities[0]
                    .solids[0],
                plan.changes[0].source.side
            )
            .unwrap()
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &result);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 8);
    }

    #[test]
    fn invalid_rises_pairs_and_batches_preserve_geometry_history_and_allocator() {
        let (mut document, id) = fixture();
        let side = document.solid(id).unwrap().sides[0].id;
        let selected = request(&document, id, side, 0);
        let source = document.vmf().clone();
        let history = document.undo_labels().count();
        for rise in [
            f64::NAN,
            f64::INFINITY,
            -1.0,
            0.0,
            0.01,
            MAX_SPLIT_RISE + 1.0,
        ] {
            assert!(
                document
                    .edit("invalid", |tx| crease(
                        tx,
                        std::slice::from_ref(&selected),
                        rise
                    ))
                    .is_err()
            );
        }
        let mut wrong = selected.clone();
        wrong.edges = [selected.edges[0]; 2];
        let mut absent = selected.clone();
        absent.side = 0;
        for requests in [
            vec![],
            vec![wrong],
            vec![absent],
            vec![selected.clone(); 2],
            vec![selected; MAX_SOLIDS + 1],
        ] {
            assert!(
                document
                    .edit("invalid", |tx| crease(tx, &requests, 16.0))
                    .is_err()
            );
        }
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.undo_labels().count(), history);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);
    }

    #[test]
    fn oblique_fractional_creases_survive_round_trip_and_reject_nonplanar_or_crossing_quads() {
        let (mut document, id) = fixture();
        let side = document.solid(id).unwrap().sides[0].id;
        document
            .edit("rotate", |tx| {
                crate::transform::transform(
                    tx,
                    &[ObjectId::Solid(id)],
                    crate::transform::Transform {
                        rotation: DVec3::new(13.0, 27.0, 39.0),
                        translation: DVec3::new(12.125, 24.25, 36.5),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let selected = request(&document, id, side, 1);
        document
            .edit("crease", |tx| crease(tx, &[selected], 16.125))
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        assert_eq!(topology(&vmf.world.solids[0]).unwrap().vertices.len(), 10);

        let (mut document, id) = fixture();
        let top = topology(document.solid(id).unwrap())
            .unwrap()
            .vertices
            .into_iter()
            .filter(|vertex| vertex.position.z == 64.0)
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides,
            })
            .collect::<Vec<_>>();
        let plan = plan_transform(
            &document,
            &top,
            &ComponentTransform::Scale(DVec3::new(0.5, 1.0, 1.0)),
            None,
            false,
        )
        .unwrap();
        document
            .edit("taper", |tx| apply(tx, &plan.replacements))
            .unwrap();
        let source = document.vmf().clone();
        let top_side = source.world.solids[0]
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let mut rejected = 0;
        for index in 0..2 {
            let selected = request(&document, id, top_side, index);
            let mut candidate = document.clone();
            if candidate
                .edit("crease", |tx| crease(tx, &[selected], 16.0))
                .is_err()
            {
                rejected += 1;
                assert_eq!(candidate.vmf(), &source);
            } else {
                assert_eq!(
                    topology(candidate.solid(id).unwrap())
                        .unwrap()
                        .vertices
                        .len(),
                    10
                );
            }
        }
        assert_eq!(rejected, 1);
        let sloped = source.world.solids[0]
            .sides
            .iter()
            .find(|side| {
                let normal = Plane::from_points(side.plane.0).unwrap().normal;
                normal.x.abs() > 0.1 && normal.z.abs() > 0.1
            })
            .unwrap()
            .id;
        let selected = request(&document, id, sloped, 0);
        assert!(
            document
                .edit("nonplanar", |tx| crease(tx, &[selected], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("neighboring")
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);
    }

    #[test]
    fn displaced_and_nonquad_faces_reject_without_geometry_or_id_changes() {
        let (document, id) = fixture();
        let side = document.solid(id).unwrap().sides[0].id;
        let selected = request(&document, id, side, 0);
        let mut source = document.vmf().clone();
        source.world.solids[0].sides[0].displacement = Some(source_formats::vmf::DispInfo {
            power: 2,
            start_position: DVec3::ZERO,
            flags: None,
            elevation: 0.0,
            subdivided: false,
            normals: None,
            distances: None,
            offsets: None,
            offset_normals: None,
            alphas: None,
            triangle_tags: None,
            allowed_verts: None,
            extra_properties: Default::default(),
            extra: vec![],
        });
        let mut document = Document::try_new(source.clone()).unwrap();
        assert!(
            document
                .edit("displaced", |tx| crease(tx, &[selected], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("displacements")
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);

        let (mut document, id) = fixture();
        let selected = topology(document.solid(id).unwrap())
            .unwrap()
            .vertices
            .into_iter()
            .filter(|vertex| vertex.position.z == 64.0)
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides,
            })
            .collect::<Vec<_>>();
        let welded = plan_weld(&document, &selected).unwrap();
        document
            .edit("pyramid", |tx| apply(tx, &welded.replacements))
            .unwrap();
        let source = document.vmf().clone();
        let side = topology(document.solid(id).unwrap())
            .unwrap()
            .faces
            .iter()
            .find(|face| face.vertices.len() == 3)
            .unwrap()
            .side;
        assert!(
            crease_pairs(document.solid(id).unwrap(), side)
                .err()
                .unwrap()
                .to_string()
                .contains("four corners")
        );
        let selected = Crease {
            solid: id,
            side,
            edges: [1, 2],
        };
        assert!(
            document
                .edit("triangle", |tx| crease(tx, &[selected], 16.0))
                .is_err()
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);
    }

    #[test]
    fn exhausted_second_allocation_rolls_back_every_candidate_and_reserved_id() {
        let (mut document, first) = fixture();
        let ObjectId::Solid(second) = document
            .edit("second", |tx| {
                edit::create_block(tx, DVec3::splat(128.0), DVec3::splat(192.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let mut vmf = document.vmf().clone();
        vmf.world.solids[0].sides[0].id = u32::MAX - 1;
        let mut document = Document::try_new(vmf.clone()).unwrap();
        let requests = [first, second]
            .map(|id| request(&document, id, document.solid(id).unwrap().sides[0].id, 0));
        assert!(
            document
                .edit("overflow", |tx| crease(tx, &requests, 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("exhausted")
        );
        assert_eq!(document.vmf(), &vmf);
        assert_eq!(document.undo_labels().count(), 0);
        assert_eq!(
            document
                .edit("last available", |tx| tx.new_side_id())
                .unwrap(),
            u32::MAX
        );
        assert!(document.edit("exhausted", |tx| tx.new_side_id()).is_err());
    }

    #[test]
    fn unsupported_geometry_face_budget_and_world_bounds_reject_before_allocation() {
        let (mut document, first) = fixture();
        let side = document.solid(first).unwrap().sides[0].id;
        let selected = request(&document, first, side, 0);
        let objects = document
            .edit("cylinder", |tx| {
                crate::primitive::create(
                    tx,
                    &crate::primitive::Spec {
                        shape: crate::primitive::Shape::Cylinder,
                        segments: 62,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let ObjectId::Solid(second) = objects[0] else {
            unreachable!()
        };
        let second_side = topology(document.solid(second).unwrap())
            .unwrap()
            .faces
            .iter()
            .find(|face| face.vertices.len() == 4)
            .unwrap()
            .side;
        let limited = request(&document, second, second_side, 0);
        let source = document.vmf().clone();
        let next = source.side_ids().max().unwrap() + 1;
        assert!(
            document
                .edit("batch", |tx| crease(tx, &[selected, limited], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("64-face")
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(
            document.edit("allocate", |tx| tx.new_side_id()).unwrap(),
            next
        );

        let (mut document, id) = fixture();
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let corners = shape
            .vertices
            .iter()
            .filter(|vertex| vertex.position.x == 64.0 && vertex.position.y == 64.0)
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        let moved = plan_move(&document, &corners, DVec3::X * -16.0).unwrap();
        document
            .edit("trapezoid prism", |tx| apply(tx, &moved))
            .unwrap();
        let solid = document.solid(id).unwrap();
        let side = solid
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let pairs = crease_pairs(solid, side).unwrap();
        let index = pairs
            .iter()
            .position(|pair| (pair.midpoints[1] - pair.midpoints[0]).y.abs() > 63.0)
            .unwrap();
        let selected = request(&document, id, side, index);
        let source = document.vmf().clone();
        assert!(
            document
                .edit("nonplanar roof", |tx| crease(tx, &[selected], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("nonplanar")
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);

        let (mut document, id) = fixture();
        document
            .edit("boundary", |tx| {
                crate::transform::transform(
                    tx,
                    &[ObjectId::Solid(id)],
                    crate::transform::Transform {
                        translation: DVec3::X * 65_472.0,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let side = document
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.x > 0.99)
            .unwrap()
            .id;
        let selected = request(&document, id, side, 0);
        let source = document.vmf().clone();
        assert!(
            document
                .edit("outside", |tx| crease(tx, &[selected], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("65536")
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);
    }
}
