use std::collections::BTreeSet;

use source_formats::vmf::Solid;

use crate::{Document, EditError, ObjectId, Plane, Transaction, carve, subobject};

pub const MAX_BRUSHES: usize = 16;
pub const MAX_PROOF_FRAGMENTS: usize = 256;
const SUPPORT_EPSILON: f64 = 0.000_01;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inheritance {
    pub side: u32,
    pub source_solid: u32,
    pub source_side: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Result {
    pub sources: Vec<u32>,
    pub solid: u32,
    pub object: ObjectId,
    pub removed: Vec<u32>,
    pub faces: Vec<Inheritance>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn owner_key(owner: ObjectId) -> Option<u32> {
    match owner {
        ObjectId::Solid(_) => None,
        ObjectId::Entity(id) => Some(id),
    }
}

fn plan(document: &Document, sources: &[u32]) -> std::result::Result<(Solid, Result), EditError> {
    let ids = sources.iter().copied().collect::<BTreeSet<_>>();
    if !(2..=MAX_BRUSHES).contains(&sources.len()) || ids.len() != sources.len() {
        return Err(invalid("Choose between 2 and 16 distinct brushes to merge"));
    }
    let mut inputs = Vec::new();
    let mut vertices = Vec::new();
    let mut expected_owner = None;
    let mut source_side_ids = BTreeSet::new();
    for id in &ids {
        let (owner, solid) = subobject::solid(document, *id)?;
        if inputs.is_empty() {
            expected_owner = owner_key(owner);
        } else if expected_owner != owner_key(owner) {
            return Err(invalid(
                "Merge world brushes together or brushes belonging to one entity; mixed owners are unsupported",
            ));
        }
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: merging displacements is unsupported"
            )));
        }
        let shape = subobject::topology(solid)?;
        vertices.extend(shape.vertices.into_iter().map(|vertex| vertex.position));
        for side in &solid.sides {
            if !source_side_ids.insert(side.id) {
                return Err(invalid(
                    "Source brushes have duplicate face IDs; repair them before merging",
                ));
            }
        }
        inputs.push((owner, solid));
    }
    let (owner, first) = inputs[0];
    let mut merged = first.clone();
    merged.sides.clear();
    let mut planes: Vec<Plane> = Vec::new();
    let mut faces = Vec::new();
    for (_, solid) in &inputs {
        let mut sides = solid.sides.iter().collect::<Vec<_>>();
        sides.sort_by_key(|side| side.id);
        for side in sides {
            let plane = Plane::from_points(side.plane.0).unwrap();
            if vertices
                .iter()
                .any(|vertex| plane.distance_to(*vertex) > SUPPORT_EPSILON)
                || planes.iter().any(|existing| existing.approx_eq(&plane))
            {
                continue;
            }
            planes.push(plane);
            merged.sides.push(side.clone());
            faces.push(Inheritance {
                side: side.id,
                source_solid: solid.id,
                source_side: side.id,
            });
        }
    }
    subobject::topology(&merged).map_err(|error| {
        invalid(format!(
            "The brush union is not a supported convex solid: {error}"
        ))
    })?;
    let mut remainder = vec![merged.clone()];
    for (_, source) in &inputs {
        let mut next = Vec::new();
        for part in remainder {
            if carve::intersects(&part, source)? {
                next.extend(carve::subtract(&part, source)?);
            } else {
                next.push(part);
            }
            if next.len() > MAX_PROOF_FRAGMENTS {
                return Err(invalid(
                    "Merge coverage proof exceeds 256 fragments; reduce the selection",
                ));
            }
        }
        remainder = next;
        if remainder.is_empty() {
            break;
        }
    }
    if !remainder.is_empty() {
        return Err(invalid(
            "The brush union is nonconvex or contains gaps; merging would add uncovered volume",
        ));
    }
    let sources = ids.into_iter().collect::<Vec<_>>();
    let removed = sources[1..].to_vec();
    let result = Result {
        sources,
        solid: merged.id,
        object: owner,
        removed,
        faces,
    };
    Ok((merged, result))
}

pub fn merge(tx: &mut Transaction, sources: &[u32]) -> std::result::Result<Result, EditError> {
    let (merged, result) = plan(tx.document(), sources)?;
    match result.object {
        ObjectId::Solid(id) => {
            *tx.solid_mut(id)? = merged;
            for source in &result.removed {
                tx.remove(ObjectId::Solid(*source))?;
            }
        }
        ObjectId::Entity(id) => {
            let entity = tx.entity_mut(id)?;
            let index = entity
                .solids
                .iter()
                .position(|solid| solid.id == result.solid)
                .unwrap();
            entity.solids[index] = merged;
            entity
                .solids
                .retain(|solid| !result.removed.contains(&solid.id));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
