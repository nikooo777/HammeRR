use super::*;

pub const MIN_RISE: f64 = 0.125;
pub const MAX_RISE: f64 = 4096.0;

#[derive(Debug, Clone, PartialEq)]
pub struct SplitFace {
    pub side: u32,
    pub source: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitChange {
    pub solid: u32,
    pub position: DVec3,
    pub source_sides: Vec<u32>,
    pub new_sides: Vec<u32>,
    pub faces: Vec<SplitFace>,
    pub selection: Component,
}

#[derive(Clone, PartialEq)]
pub struct SplitPlan {
    pub replacements: Vec<Replacement>,
    pub changes: Vec<SplitChange>,
}

pub fn split(
    tx: &mut Transaction,
    components: &[Component],
    rise: f64,
) -> Result<SplitPlan, EditError> {
    if components.is_empty() || components.len() > MAX_SOLIDS {
        return Err(invalid(
            "Select one face or edge in each of 1–16 brushes to split",
        ));
    }
    if !rise.is_finite() || !(MIN_RISE..=MAX_RISE).contains(&rise) {
        return Err(invalid(
            "Split rise must be finite and between 0.125 and 4096 units",
        ));
    }
    let mut sources = BTreeMap::new();
    for component in components {
        let (owner, source) = solid(tx.document(), component.solid)?;
        if sources
            .insert(component.solid, (owner, source.clone(), component))
            .is_some()
        {
            return Err(invalid(format!(
                "Brush {}: select exactly one face or edge to split",
                component.solid
            )));
        }
    }
    let mut plan = SplitPlan {
        replacements: Vec::new(),
        changes: Vec::new(),
    };
    for (id, (owner, source, component)) in sources {
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: splitting displacements is unsupported"
            )));
        }
        let shape = topology(&source)?;
        let resolved = shape.resolve(&component.sides)?;
        if !(1..=2).contains(&component.sides.len()) {
            return Err(invalid(format!(
                "Brush {id}: split selects a face or edge, not a corner"
            )));
        }
        let mut source_sides = component.sides.clone();
        source_sides.sort_unstable();
        let normal = source_sides
            .iter()
            .map(|id| {
                Plane::from_points(
                    source
                        .sides
                        .iter()
                        .find(|side| side.id == *id)
                        .unwrap()
                        .plane
                        .0,
                )
                .unwrap()
                .normal
            })
            .sum::<DVec3>()
            .try_normalize()
            .ok_or_else(|| invalid(format!("Brush {id}: edge has no outward split direction")))?;
        let center = resolved
            .iter()
            .map(|index| shape.vertices[*index].position)
            .sum::<DVec3>()
            / resolved.len() as f64;
        let position = center + normal * rise;
        if !position.is_finite() || position.abs().max_element() > MAX_COORDINATE {
            return Err(invalid(format!(
                "Brush {id}: split corner exceeds ±65536 units"
            )));
        }
        for side in &source.sides {
            if !source_sides.contains(&side.id)
                && Plane::from_points(side.plane.0)
                    .unwrap()
                    .distance_to(position)
                    > EPSILON
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: split crosses a neighboring face; reduce the rise",
                    side.id
                )));
            }
        }
        let positions = shape
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .chain([position])
            .collect::<Vec<_>>();
        let mut replacement = source.clone();
        replacement.sides.clear();
        let mut new_sides = Vec::new();
        let mut faces = Vec::new();
        for original in &source.sides {
            if !source_sides.contains(&original.id) {
                replacement.sides.push(original.clone());
                continue;
            }
            let face = shape
                .faces
                .iter()
                .find(|face| face.side == original.id)
                .unwrap();
            let mut first = true;
            for index in 0..face.vertices.len() {
                let a = face.vertices[index];
                let b = face.vertices[(index + 1) % face.vertices.len()];
                if source_sides.len() == 2 && resolved.contains(&a) && resolved.contains(&b) {
                    continue;
                }
                let points = [
                    position,
                    shape.vertices[a].position,
                    shape.vertices[b].position,
                ];
                let plane = Plane::from_points(points).ok_or_else(|| {
                    invalid(format!(
                        "Brush {id}, face {}: split creates a collapsed face",
                        original.id
                    ))
                })?;
                if positions
                    .iter()
                    .any(|point| plane.distance_to(*point) > EPSILON)
                {
                    return Err(invalid(format!(
                        "Brush {id}, face {}: split makes the brush concave or inverted",
                        original.id
                    )));
                }
                let mut side = original.clone();
                if !first {
                    side.id = tx.new_side_id()?;
                    new_sides.push(side.id);
                }
                first = false;
                side.plane.0 = points;
                faces.push(SplitFace {
                    side: side.id,
                    source: original.id,
                });
                replacement.sides.push(side);
                if replacement.sides.len() > MAX_SIDES {
                    return Err(invalid(format!(
                        "Brush {id}: split exceeds the 64-face limit"
                    )));
                }
            }
        }
        if replacement.sides.len() > MAX_SIDES {
            return Err(invalid(format!(
                "Brush {id}: split exceeds the 64-face limit"
            )));
        }
        let after = topology(&replacement)?;
        if after.vertices.len() != positions.len()
            || positions.iter().any(|point| {
                !after
                    .vertices
                    .iter()
                    .any(|vertex| vertex.position.distance(*point) < EPSILON * 10.0)
            })
        {
            return Err(invalid(format!(
                "Brush {id}: split changes existing corners or enclosed geometry"
            )));
        }
        let vertex = after
            .vertices
            .iter()
            .find(|vertex| vertex.position.distance(position) < EPSILON * 10.0)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {id}: split corner is not on the brush boundary"
                ))
            })?;
        plan.changes.push(SplitChange {
            solid: id,
            position,
            source_sides,
            new_sides,
            faces,
            selection: Component {
                solid: id,
                sides: vertex.sides.clone(),
            },
        });
        plan.replacements.push(Replacement {
            owner,
            solid: replacement,
        });
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

    #[test]
    fn every_box_face_and_edge_can_split_without_moving_existing_corners() {
        let (document, id) = fixture();
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let components = shape
            .faces
            .iter()
            .map(|face| Component {
                solid: id,
                sides: vec![face.side],
            })
            .chain(shape.edges.iter().map(|edge| Component {
                solid: id,
                sides: edge.sides.clone(),
            }));
        for component in components {
            let mut changed = document.clone();
            let plan = changed
                .edit("split", |tx| {
                    split(tx, std::slice::from_ref(&component), 16.0)
                })
                .unwrap();
            let after = topology(changed.solid(id).unwrap()).unwrap();
            assert_eq!(after.vertices.len(), 9);
            assert_eq!(
                after.faces.len(),
                if component.sides.len() == 1 { 9 } else { 10 }
            );
            assert_eq!(
                plan.changes[0].new_sides.len(),
                if component.sides.len() == 1 { 3 } else { 4 }
            );
            for original in &shape.vertices {
                assert!(
                    after
                        .vertices
                        .iter()
                        .any(|vertex| vertex.position.abs_diff_eq(original.position, EPSILON))
                );
            }
            assert_eq!(
                after
                    .resolve(&plan.changes[0].selection.sides)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn split_faces_inherit_metadata_and_ids_survive_vmf_and_history() {
        let (mut document, id) = fixture();
        document
            .edit("metadata", |tx| {
                for side in &mut tx.solid_mut(id)?.sides {
                    side.extra_properties.set("vendor", "keep");
                    side.u_axis.offset = 17.25;
                    side.v_axis.scale = -0.5;
                    side.lightmap_scale = 8;
                    side.smoothing_groups = Some(7);
                }
                Ok(())
            })
            .unwrap();
        let original = document.vmf().clone();
        let selected = Component {
            solid: id,
            sides: vec![original.world.solids[0].sides[0].id],
        };
        let plan = document
            .edit("split", |tx| {
                split(tx, std::slice::from_ref(&selected), 16.0)
            })
            .unwrap();
        let result = document.vmf().clone();
        for side in &result.world.solids[0].sides {
            let parent = if plan.changes[0].new_sides.contains(&side.id) {
                selected.sides[0]
            } else {
                side.id
            };
            let mut expected = original.world.solids[0]
                .sides
                .iter()
                .find(|old| old.id == parent)
                .unwrap()
                .clone();
            expected.id = side.id;
            expected.plane = side.plane;
            assert_eq!(*side, expected);
        }
        result.validate_ids().unwrap();
        let parsed = source_formats::vmf::Vmf::parse(&result.write(&Default::default())).unwrap();
        parsed.validate_ids().unwrap();
        assert_eq!(topology(&parsed.world.solids[0]).unwrap().vertices.len(), 9);
        document.undo().unwrap();
        assert_eq!(document.vmf(), &original);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &result);
        let next = document.edit("allocate", |tx| tx.new_side_id()).unwrap();
        assert!(next > *plan.changes[0].new_sides.iter().max().unwrap());
    }

    #[test]
    fn rejected_splits_restore_ids_geometry_and_history() {
        let (mut document, id) = fixture();
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let face = Component {
            solid: id,
            sides: vec![shape.faces[0].side],
        };
        let corner = Component {
            solid: id,
            sides: shape.vertices[0].sides.clone(),
        };
        let source = document.vmf().clone();
        let history = document.undo_labels().count();
        for rise in [f64::NAN, f64::INFINITY, 0.0, -16.0, 0.01, 4097.0] {
            assert!(
                document
                    .edit("invalid", |tx| split(tx, std::slice::from_ref(&face), rise))
                    .is_err()
            );
        }
        for components in [vec![], vec![corner], vec![face.clone(), face.clone()]] {
            assert!(
                document
                    .edit("invalid", |tx| split(tx, &components, 16.0))
                    .is_err()
            );
        }
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.undo_labels().count(), history);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);
    }

    #[test]
    fn later_invalid_brush_rolls_back_allocated_faces_and_entity_ownership_is_retained() {
        let (mut document, id) = fixture();
        let owner = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])
            })
            .unwrap();
        let ObjectId::Solid(other) = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::splat(128.0), DVec3::splat(192.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let first = topology(solid(&document, id).unwrap().1).unwrap();
        let second = topology(document.solid(other).unwrap()).unwrap();
        let selected = Component {
            solid: id,
            sides: vec![first.faces[0].side],
        };
        let invalid = Component {
            solid: other,
            sides: second.vertices[0].sides.clone(),
        };
        let original = document.vmf().clone();
        assert!(
            document
                .edit("batch", |tx| split(tx, &[selected.clone(), invalid], 16.0))
                .is_err()
        );
        assert_eq!(document.vmf(), &original);
        let plan = document
            .edit("split", |tx| split(tx, &[selected], 16.0))
            .unwrap();
        assert_eq!(plan.changes[0].new_sides[0], 13);
        assert_eq!(document.owner_of_solid(id), Some(owner));
        assert_eq!(
            document.vmf().entities[0].properties,
            original.entities[0].properties
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &original);
    }

    #[test]
    fn face_limit_and_coordinate_limit_reject_without_consuming_ids() {
        let mut document = Document::empty();
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
        let ObjectId::Solid(id) = objects[0] else {
            unreachable!()
        };
        let source = document.vmf().clone();
        let selected = Component {
            solid: id,
            sides: vec![
                document
                    .solid(id)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id,
            ],
        };
        let expected = document.vmf().side_ids().max().unwrap() + 1;
        let error = document
            .edit("split", |tx| split(tx, &[selected], 16.0))
            .err()
            .unwrap();
        assert!(error.to_string().contains("64-face"));
        assert_eq!(document.vmf(), &source);
        assert_eq!(
            document.edit("allocate", |tx| tx.new_side_id()).unwrap(),
            expected
        );

        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("boundary", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(65500.0, 0.0, 0.0),
                    DVec3::new(65536.0, 64.0, 64.0),
                    "brick/test",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let selected = Component {
            solid: id,
            sides: vec![
                document
                    .solid(id)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.x > 0.99)
                    .unwrap()
                    .id,
            ],
        };
        let source = document.vmf().clone();
        assert!(
            document
                .edit("split", |tx| split(tx, &[selected], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("65536")
        );
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.edit("allocate", |tx| tx.new_side_id()).unwrap(), 7);
    }

    #[test]
    fn oblique_splits_are_valid_and_neighboring_faces_bound_the_rise() {
        let (mut document, id) = fixture();
        let selected = Component {
            solid: id,
            sides: vec![document.solid(id).unwrap().sides[0].id],
        };
        document
            .edit("rotate", |tx| {
                crate::transform::transform(
                    tx,
                    &[ObjectId::Solid(id)],
                    crate::transform::Transform {
                        rotation: DVec3::new(13.0, 27.0, 39.0),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        document
            .edit("split", |tx| split(tx, &[selected], 0.125))
            .unwrap();
        assert_eq!(
            topology(document.solid(id).unwrap())
                .unwrap()
                .vertices
                .len(),
            9
        );

        let (mut document, id) = fixture();
        document
            .edit("clip", |tx| {
                crate::clip::clip(
                    tx,
                    &[ObjectId::Solid(id)],
                    Plane {
                        normal: DVec3::new(1.0, 0.0, 1.0),
                        dist: 80.0,
                    },
                    crate::clip::Keep::Back,
                    None,
                )
            })
            .unwrap();
        let selected = Component {
            solid: id,
            sides: vec![
                document
                    .solid(id)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.x > 0.99)
                    .unwrap()
                    .id,
            ],
        };
        let source = document.vmf().clone();
        assert!(
            document
                .edit("split", |tx| split(tx, &[selected], 16.0))
                .err()
                .unwrap()
                .to_string()
                .contains("neighboring")
        );
        assert_eq!(document.vmf(), &source);
    }
}
