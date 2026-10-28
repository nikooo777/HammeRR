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
    split_with_textures(tx, components, rise, false)
}

pub fn split_with_textures(
    tx: &mut Transaction,
    components: &[Component],
    rise: f64,
    texture_lock: bool,
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
                if texture_lock {
                    let before = [center, points[1], points[2]];
                    let projection = crate::texture_deform::attach(original, &before, &points)
                        .map_err(|error| {
                            invalid(format!("Brush {id}, face {}: {error}", original.id))
                        })?;
                    side.u_axis = projection.u;
                    side.v_axis = projection.v;
                }
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
