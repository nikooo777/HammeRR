use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::vmf::Solid;

use crate::surface::FaceId;
use crate::{Document, EditError, ObjectId, Plane, Transaction, clip, subobject};

pub const MAX_BRUSHES: usize = 16;
pub const MAX_PARTS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub thickness: f64,
    pub outward: bool,
    pub open_faces: BTreeSet<FaceId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Result {
    pub objects: Vec<ObjectId>,
    pub brushes: Vec<u32>,
    pub sources: usize,
    pub cavities: Vec<Cavity>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cavity {
    pub source: u32,
    pub center: DVec3,
    pub mins: DVec3,
    pub maxs: DVec3,
}

struct Shell {
    owner: ObjectId,
    source: u32,
    parts: Vec<Solid>,
    cavity: Cavity,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn plan(
    document: &Document,
    ids: &[ObjectId],
    spec: &Spec,
) -> std::result::Result<Vec<Shell>, EditError> {
    if !spec.thickness.is_finite() || !(0.125..=4096.0).contains(&spec.thickness) {
        return Err(invalid(
            "Wall thickness must be between 0.125 and 4096 units",
        ));
    }
    let mut sources = BTreeSet::new();
    for id in ids {
        match id {
            ObjectId::Solid(id) => {
                sources.insert(*id);
            }
            ObjectId::Entity(id) => {
                let entity = document
                    .entity(*id)
                    .ok_or(EditError::NotFound(ObjectId::Entity(*id)))?;
                if entity.solids.is_empty() {
                    return Err(invalid("Select brushes or brush entities to hollow"));
                }
                sources.extend(entity.solids.iter().map(|solid| solid.id));
            }
        }
    }
    if sources.is_empty() || sources.len() > MAX_BRUSHES {
        return Err(invalid("Hollow between 1 and 16 brushes at a time"));
    }
    if spec.open_faces.len() > MAX_BRUSHES * subobject::MAX_SIDES {
        return Err(invalid("Too many opening faces"));
    }
    for face in &spec.open_faces {
        if !sources.contains(&face.solid)
            || !subobject::solid(document, face.solid)?
                .1
                .sides
                .iter()
                .any(|side| side.id == face.side)
        {
            return Err(invalid(format!(
                "Opening face {} must belong to selected brush {}",
                face.side, face.solid
            )));
        }
    }
    let mut shells = Vec::new();
    let mut total = 0;
    for id in sources {
        let (owner, source) = subobject::solid(document, id)?;
        subobject::topology(source)?;
        let openings = spec
            .open_faces
            .iter()
            .filter(|face| face.solid == id)
            .map(|face| face.side)
            .collect::<BTreeSet<_>>();
        let walls = source.sides.len() - openings.len();
        if walls == 0 {
            return Err(invalid(format!("Brush {id}: keep at least one wall")));
        }
        if total + walls > MAX_PARTS {
            return Err(invalid(
                "Hollowing would create more than 256 wall brushes; use a smaller selection",
            ));
        }
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id} has displacements; hollowing is unsupported"
            )));
        }
        let mut outer = source.clone();
        let mut inner = source.clone();
        for (index, side) in source.sides.iter().enumerate() {
            if openings.contains(&side.id) {
                continue;
            }
            let plane = Plane::from_points(side.plane.0).unwrap();
            if spec.outward {
                outer.sides[index].plane.0 = clip::plane_points(Plane {
                    dist: plane.dist + spec.thickness,
                    ..plane
                });
            } else {
                inner.sides[index].plane.0 = clip::plane_points(Plane {
                    dist: plane.dist - spec.thickness,
                    ..plane
                });
            }
        }
        let inner_shape = subobject::topology(&inner).map_err(|error| {
            invalid(format!(
                "Brush {id}: thickness leaves no valid inner space ({error})"
            ))
        })?;
        let center = inner_shape
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .sum::<DVec3>()
            / inner_shape.vertices.len() as f64;
        let (mins, maxs) = inner_shape
            .vertices
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), vertex| {
                (mins.min(vertex.position), maxs.max(vertex.position))
            });
        let cavity = Cavity {
            source: id,
            center,
            mins,
            maxs,
        };
        subobject::topology(&outer)?;
        let mut remainder = outer;
        let mut parts = Vec::new();
        for side in &inner.sides {
            if openings.contains(&side.id) {
                continue;
            }
            let plane = Plane::from_points(side.plane.0).unwrap();
            let mut part = clip::half(
                &remainder,
                Plane {
                    normal: -plane.normal,
                    dist: -plane.dist,
                },
                &side.material,
            )?;
            inherit_cap(&mut part, side);
            parts.push(part);
            remainder = clip::half(&remainder, plane, &side.material)?;
            inherit_cap(&mut remainder, side);
        }
        total += parts.len();
        if total > MAX_PARTS {
            return Err(invalid(
                "Hollowing would create more than 256 wall brushes; use a smaller selection",
            ));
        }
        shells.push(Shell {
            owner,
            source: id,
            parts,
            cavity,
        });
    }
    Ok(shells)
}

fn inherit_cap(solid: &mut Solid, source: &source_formats::vmf::Side) {
    let cap = solid.sides.last_mut().unwrap();
    let plane = cap.plane;
    *cap = source.clone();
    cap.id = 0;
    cap.plane = plane;
}

pub fn hollow(
    tx: &mut Transaction,
    ids: &[ObjectId],
    spec: &Spec,
) -> std::result::Result<Result, EditError> {
    let shells = plan(tx.document(), ids, spec)?;
    let mut result = Result {
        objects: Vec::new(),
        brushes: Vec::new(),
        sources: shells.len(),
        cavities: Vec::new(),
    };
    for mut shell in shells {
        result.cavities.push(shell.cavity);
        for (index, part) in shell.parts.iter_mut().enumerate() {
            if index > 0 {
                part.id = tx.new_object_id()?;
            }
            for side in &mut part.sides {
                if index > 0 || side.id == 0 {
                    side.id = tx.new_side_id()?;
                }
            }
            subobject::topology(part)?;
            result.brushes.push(part.id);
        }
        match shell.owner {
            ObjectId::Solid(id) => {
                *tx.solid_mut(id)? = shell.parts.remove(0);
                result.objects.push(shell.owner);
                for part in shell.parts {
                    result.objects.push(tx.add_solid(part)?);
                }
            }
            ObjectId::Entity(id) => {
                let entity = tx.entity_mut(id)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == shell.source)
                    .unwrap();
                entity.solids.splice(index..=index, shell.parts);
                result.objects.push(shell.owner);
            }
        }
    }
    result.objects.sort();
    result.objects.dedup();
    Ok(result)
}
