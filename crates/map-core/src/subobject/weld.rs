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

#[derive(Debug, Clone, PartialEq, Default)]
pub enum WeldTarget {
    #[default]
    Mean,
    Position(DVec3),
    Corner(Component),
}

pub fn plan_weld(document: &Document, components: &[Component]) -> Result<WeldPlan, EditError> {
    plan_weld_target(document, components, &WeldTarget::Mean)
}

pub fn plan_weld_target(
    document: &Document,
    components: &[Component],
    target: &WeldTarget,
) -> Result<WeldPlan, EditError> {
    plan_weld_with_textures(document, components, target, false)
}

pub fn plan_weld_with_textures(
    document: &Document,
    components: &[Component],
    target: &WeldTarget,
    texture_lock: bool,
) -> Result<WeldPlan, EditError> {
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
    let target = match target {
        WeldTarget::Mean => None,
        WeldTarget::Position(position) => Some(*position),
        WeldTarget::Corner(component) => {
            if !grouped.contains_key(&component.solid) {
                return Err(invalid(
                    "Choose a corner from one of the brushes being welded",
                ));
            }
            let shape = topology(solid(document, component.solid)?.1)?;
            let resolved = shape.resolve(&component.sides)?;
            if resolved.len() != 1 {
                return Err(invalid("A weld target must identify one existing corner"));
            }
            Some(shape.vertices[resolved[0]].position)
        }
    };
    if target.is_some_and(|position| {
        !position.is_finite() || position.abs().max_element() > MAX_COORDINATE
    }) {
        return Err(invalid(
            "Weld target needs finite coordinates within ±65536 units",
        ));
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
        let position = target.unwrap_or_else(|| {
            selected
                .iter()
                .map(|index| shape.vertices[*index].position)
                .sum::<DVec3>()
                / selected.len() as f64
        });
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
                if texture_lock {
                    let before = ring
                        .iter()
                        .map(|point| {
                            let contributors = face
                                .vertices
                                .iter()
                                .filter(|index| positions[**index].distance(*point) < EPSILON)
                                .copied()
                                .collect::<Vec<_>>();
                            contributors
                                .iter()
                                .map(|index| shape.vertices[*index].position)
                                .sum::<DVec3>()
                                / contributors.len() as f64
                        })
                        .collect::<Vec<_>>();
                    let projection = crate::texture_deform::attach(original, &before, &ring)
                        .map_err(|error| {
                            invalid(format!("Brush {id}, face {}: {error}", original.id))
                        })?;
                    side.u_axis = projection.u;
                    side.v_axis = projection.v;
                }
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
