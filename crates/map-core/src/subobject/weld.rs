use super::*;

#[derive(Debug, Clone)]
pub struct WeldChange {
    pub solid: u32,
    pub position: DVec3,
    pub merged_vertices: usize,
    pub removed_sides: Vec<u32>,
    pub selection: Component,
}

pub struct WeldPlan {
    pub replacements: Vec<Replacement>,
    pub changes: Vec<WeldChange>,
}

pub fn plan_weld(document: &Document, components: &[Component]) -> Result<WeldPlan, EditError> {
    if !(2..=MAX_COMPONENTS).contains(&components.len()) {
        return Err(invalid("Weld needs 2–512 selected corners"));
    }
    let mut grouped: BTreeMap<u32, Vec<&Component>> = BTreeMap::new();
    for component in components {
        grouped.entry(component.solid).or_default().push(component);
    }
    if grouped.len() > MAX_SOLIDS {
        return Err(invalid("Weld supports at most 16 brushes at a time"));
    }
    let mut plan = WeldPlan {
        replacements: Vec::new(),
        changes: Vec::new(),
    };
    for (id, components) in grouped {
        let (owner, source) = solid(document, id)?;
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: welding displacements is unsupported"
            )));
        }
        let shape = topology(source)?;
        let mut selected = BTreeSet::new();
        for component in components {
            let resolved = shape.resolve(&component.sides)?;
            if resolved.len() != 1 {
                return Err(invalid(format!(
                    "Brush {id}: weld selects corners, not edges or faces"
                )));
            }
            selected.insert(resolved[0]);
        }
        if selected.len() < 2 {
            return Err(invalid(format!(
                "Brush {id}: select at least two distinct corners to weld"
            )));
        }
        let position = selected
            .iter()
            .map(|index| shape.vertices[*index].position)
            .sum::<DVec3>()
            / selected.len() as f64;
        let positions = shape
            .vertices
            .iter()
            .enumerate()
            .map(|(index, vertex)| {
                if selected.contains(&index) {
                    position
                } else {
                    vertex.position
                }
            })
            .collect::<Vec<_>>();
        let mut replacement = source.clone();
        replacement.sides.clear();
        let mut removed_sides = Vec::new();
        for original in &source.sides {
            let face = shape
                .faces
                .iter()
                .find(|face| face.side == original.id)
                .unwrap();
            let mut ring = face
                .vertices
                .iter()
                .map(|index| positions[*index])
                .collect::<Vec<_>>();
            ring.dedup_by(|a, b| a.distance(*b) < EPSILON);
            if ring.len() > 1 && ring[0].distance(*ring.last().unwrap()) < EPSILON {
                ring.pop();
            }
            if ring.iter().enumerate().any(|(index, point)| {
                ring[index + 1..]
                    .iter()
                    .any(|other| point.distance(*other) < EPSILON)
            }) {
                return Err(invalid(format!(
                    "Brush {id}, face {}: weld creates a self-touching face",
                    face.side
                )));
            }
            let triple = ring.first().and_then(|first| {
                ring[1..]
                    .windows(2)
                    .map(|pair| [*first, pair[0], pair[1]])
                    .max_by(|a, b| area(a).total_cmp(&area(b)))
                    .filter(|points| area(points) > EPSILON * EPSILON)
            });
            let Some(triple) = triple else {
                removed_sides.push(face.side);
                continue;
            };
            let plane = Plane::from_points(triple).unwrap();
            if ring
                .iter()
                .any(|point| plane.distance_to(*point).abs() > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: weld makes the face nonplanar; select more corners",
                    face.side
                )));
            }
            if positions
                .iter()
                .any(|point| plane.distance_to(*point) > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: weld makes the brush concave or inverted",
                    face.side
                )));
            }
            let mut side = original.clone();
            if face.vertices.iter().any(|index| selected.contains(index)) {
                side.plane.0 = triple;
            }
            replacement.sides.push(side);
        }
        let after = topology(&replacement)?;
        let mut unique = Vec::<DVec3>::new();
        for point in &positions {
            if !unique
                .iter()
                .any(|existing| existing.distance(*point) < EPSILON)
            {
                unique.push(*point);
            }
        }
        if after.vertices.len() != unique.len()
            || unique.iter().any(|point| {
                !after
                    .vertices
                    .iter()
                    .any(|vertex| vertex.position.distance(*point) < EPSILON * 10.0)
            })
        {
            return Err(invalid(format!(
                "Brush {id}: weld changes unselected corners or enclosed geometry"
            )));
        }
        let vertex = after
            .vertices
            .iter()
            .find(|vertex| vertex.position.distance(position) < EPSILON * 10.0)
            .ok_or_else(|| {
                invalid(format!(
                    "Brush {id}: welded corner is no longer on the brush boundary"
                ))
            })?;
        plan.changes.push(WeldChange {
            solid: id,
            position,
            merged_vertices: selected.len(),
            removed_sides,
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
    Ok(plan)
}

fn area(points: &[DVec3; 3]) -> f64 {
    (points[0] - points[1])
        .cross(points[2] - points[1])
        .length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;

    fn fixture(origin: DVec3, size: f64) -> (Document, u32, Vec<Component>) {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, origin, origin + DVec3::splat(size), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let selection = shape
            .vertices
            .iter()
            .filter(|vertex| (vertex.position.z - origin.z - size).abs() < EPSILON)
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect();
        (document, id, selection)
    }

    #[test]
    fn welding_a_top_ring_creates_a_pyramid_and_preserves_surviving_faces() {
        let (mut document, id, selection) = fixture(DVec3::ZERO, 64.0);
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
        let plan = plan_weld(&document, &selection).unwrap();
        assert_eq!(document.vmf(), &original);
        assert_eq!(plan.changes[0].position, DVec3::new(32.0, 32.0, 64.0));
        assert_eq!(plan.changes[0].merged_vertices, 4);
        assert_eq!(plan.changes[0].removed_sides.len(), 1);
        let history = document.undo_labels().count();
        document
            .edit("weld", |tx| apply(tx, &plan.replacements))
            .unwrap();
        let result = document.vmf().clone();
        let solid = document.solid(id).unwrap();
        let shape = topology(solid).unwrap();
        assert_eq!(
            (shape.vertices.len(), shape.edges.len(), shape.faces.len()),
            (5, 8, 5)
        );
        assert_eq!(
            shape
                .resolve(&plan.changes[0].selection.sides)
                .unwrap()
                .len(),
            1
        );
        for side in &solid.sides {
            let mut expected = original.world.solids[0]
                .sides
                .iter()
                .find(|old| old.id == side.id)
                .unwrap()
                .clone();
            expected.plane = side.plane;
            assert_eq!(*side, expected);
        }
        assert_eq!(document.undo_labels().count(), history + 1);
        let parsed = source_formats::vmf::Vmf::parse(&result.write(&Default::default())).unwrap();
        assert_eq!(topology(&parsed.world.solids[0]).unwrap().vertices.len(), 5);
        document.undo().unwrap();
        assert_eq!(document.vmf(), &original);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &result);
    }

    #[test]
    fn invalid_corner_sets_reject_without_mutation() {
        let (document, id, selection) = fixture(DVec3::ZERO, 64.0);
        let source = document.vmf().clone();
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let cases = [
            vec![],
            vec![selection[0].clone()],
            vec![selection[0].clone(), selection[0].clone()],
            selection[..2].to_vec(),
            shape
                .vertices
                .iter()
                .map(|vertex| Component {
                    solid: id,
                    sides: vertex.sides.clone(),
                })
                .collect(),
            vec![
                Component {
                    solid: id,
                    sides: shape.edges[0].sides.clone(),
                },
                selection[0].clone(),
            ],
            vec![
                Component {
                    solid: id,
                    sides: vec![shape.faces[0].side],
                },
                selection[0].clone(),
            ],
        ];
        for selected in cases {
            assert!(plan_weld(&document, &selected).is_err(), "{selected:?}");
            assert_eq!(document.vmf(), &source);
        }
    }

    #[test]
    fn multibrush_weld_is_atomic_and_entity_ownership_is_retained() {
        let (mut document, id, mut selection) = fixture(DVec3::ZERO, 64.0);
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
        let shape = topology(document.solid(other).unwrap()).unwrap();
        let original = document.vmf().clone();
        selection.push(Component {
            solid: other,
            sides: shape.vertices[0].sides.clone(),
        });
        assert!(plan_weld(&document, &selection).is_err());
        assert_eq!(document.vmf(), &original);
        selection.pop();
        selection.extend(
            shape
                .vertices
                .iter()
                .filter(|vertex| vertex.position.z == 192.0)
                .map(|vertex| Component {
                    solid: other,
                    sides: vertex.sides.clone(),
                }),
        );
        let plan = plan_weld(&document, &selection).unwrap();
        document
            .edit("weld", |tx| apply(tx, &plan.replacements))
            .unwrap();
        assert_eq!(document.owner_of_solid(id), Some(owner));
        assert_eq!(
            document.vmf().entities[0].properties,
            original.entities[0].properties
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &original);
    }

    #[test]
    fn fractional_and_large_coordinate_welds_keep_the_exact_target() {
        for (origin, size) in [(DVec3::ZERO, 0.125), (DVec3::splat(32000.0), 64.0)] {
            let (document, _, selection) = fixture(origin, size);
            let plan = plan_weld(&document, &selection).unwrap();
            assert_eq!(
                plan.changes[0].position,
                origin + DVec3::new(size * 0.5, size * 0.5, size)
            );
        }
    }

    #[test]
    fn every_face_ring_can_weld_on_an_oblique_box() {
        let (mut document, id, _) = fixture(DVec3::ZERO, 64.0);
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
        let source = document.vmf().clone();
        let shape = topology(document.solid(id).unwrap()).unwrap();
        for face in &shape.faces {
            let selected = face
                .vertices
                .iter()
                .map(|index| Component {
                    solid: id,
                    sides: shape.vertices[*index].sides.clone(),
                })
                .collect::<Vec<_>>();
            let expected = face
                .vertices
                .iter()
                .map(|index| shape.vertices[*index].position)
                .sum::<DVec3>()
                / 4.0;
            let plan = plan_weld(&document, &selected).unwrap();
            assert!(plan.changes[0].position.abs_diff_eq(expected, EPSILON));
            assert_eq!(
                topology(&plan.replacements[0].solid)
                    .unwrap()
                    .vertices
                    .len(),
                5
            );
            assert_eq!(document.vmf(), &source);
        }
    }

    #[test]
    fn displacement_and_ambiguous_sources_reject_before_planning() {
        let (document, _, selection) = fixture(DVec3::ZERO, 64.0);
        let mut vmf = document.vmf().clone();
        vmf.world.solids.push(vmf.world.solids[0].clone());
        assert!(
            plan_weld(&Document::new(vmf), &selection)
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate")
        );
        let mut vmf = document.vmf().clone();
        vmf.world.solids[0].sides[0].displacement = Some(source_formats::vmf::DispInfo {
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
            extra: Vec::new(),
        });
        assert!(
            plan_weld(&Document::new(vmf), &selection)
                .err()
                .unwrap()
                .to_string()
                .contains("displacements")
        );
    }
}
