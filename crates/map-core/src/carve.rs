use std::collections::{BTreeMap, BTreeSet};

use source_formats::vmf::{Side, Solid};

use crate::plane::ON_EPSILON;
use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction, clip, subobject};

pub const MAX_TARGETS: usize = 16;
pub const MAX_CUTTERS: usize = 8;
pub const MAX_FRAGMENTS: usize = 256;
const TARGET_NORMAL_EPSILON: f64 = 1e-8;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CutMaterialPolicy {
    #[default]
    Cutter,
    Target,
    Material(String),
}

impl CutMaterialPolicy {
    fn normalized(&self) -> std::result::Result<Self, EditError> {
        match self {
            Self::Material(material) => {
                if material.trim().is_empty() {
                    return Err(invalid("Cut material must not be empty"));
                }
                source_formats::keyvalues::validate_string(material)
                    .map_err(|error| invalid(error.to_string()))?;
                Ok(Self::Material(material.trim().to_ascii_uppercase()))
            }
            _ => Ok(self.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub source: u32,
    pub fragments: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inheritance {
    pub solid: u32,
    pub side: u32,
    pub source_solid: u32,
    pub source_side: u32,
    pub surface_source_solid: u32,
    pub surface_source_side: u32,
    pub cutter: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Result {
    pub cutter: u32,
    pub cutters: Vec<u32>,
    pub cut_material: CutMaterialPolicy,
    pub changes: Vec<Change>,
    pub unchanged: Vec<u32>,
    pub removed: Vec<u32>,
    pub objects: Vec<ObjectId>,
    pub faces: Vec<Inheritance>,
}

struct Planned {
    owner: ObjectId,
    source: u32,
    parts: Vec<Solid>,
}

enum Position {
    Inside,
    Outside,
    Crossing,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn position(solid: &Solid, plane: Plane) -> std::result::Result<Position, EditError> {
    let shape = BrushGeometry::from_solid(solid)
        .map_err(|error| invalid(format!("Brush {}: {error}", solid.id)))?;
    let mut front = false;
    let mut back = false;
    for vertex in shape.faces.iter().flat_map(|face| &face.vertices) {
        let distance = plane.distance_to(*vertex);
        front |= distance > ON_EPSILON;
        back |= distance < -ON_EPSILON;
    }
    Ok(if !front {
        Position::Inside
    } else if !back {
        Position::Outside
    } else {
        Position::Crossing
    })
}

fn half(solid: &Solid, plane: Plane, source: &Side) -> std::result::Result<Solid, EditError> {
    checked_half(solid, plane, source).map_err(clip::HalfError::into_edit_error)
}

fn checked_half(
    solid: &Solid,
    plane: Plane,
    source: &Side,
) -> std::result::Result<Solid, clip::HalfError> {
    let mut part = clip::checked_half(solid, plane, &source.material)?;
    let cap = part.sides.last_mut().unwrap();
    let plane = cap.plane;
    *cap = source.clone();
    cap.plane = plane;
    subobject::topology(&part).map_err(clip::HalfError::Invalid)?;
    Ok(part)
}

pub(crate) fn intersects(source: &Solid, cutter: &Solid) -> std::result::Result<bool, EditError> {
    let mut inside = source.clone();
    for side in &cutter.sides {
        let plane = Plane::from_points(side.plane.0).unwrap();
        match position(&inside, plane)? {
            Position::Inside => {}
            Position::Outside => return Ok(false),
            Position::Crossing => inside = half(&inside, plane, side)?,
        }
    }
    Ok(true)
}

pub(crate) fn subtract(
    source: &Solid,
    cutter: &Solid,
) -> std::result::Result<Vec<Solid>, EditError> {
    let mut inside = source.clone();
    let mut parts = Vec::new();
    let mut pending = cutter.sides.iter().collect::<Vec<_>>();
    while !pending.is_empty() {
        let mut deferred = Vec::new();
        let mut failure = None;
        let mut progressed = false;
        for side in pending {
            let plane = Plane::from_points(side.plane.0).unwrap();
            match position(&inside, plane)? {
                Position::Inside => progressed = true,
                Position::Outside => {
                    parts.push(inside);
                    return Ok(parts);
                }
                Position::Crossing => match split(&inside, plane, side) {
                    Ok((outside, remainder)) => {
                        parts.push(outside);
                        inside = remainder;
                        progressed = true;
                    }
                    Err(error @ clip::HalfError::SmallVolume(_)) => {
                        deferred.push(side);
                        failure.get_or_insert(error);
                    }
                    Err(error) => return Err(error.into_edit_error()),
                },
            }
        }
        if !progressed {
            return Err(failure.unwrap().into_edit_error());
        }
        pending = deferred;
    }
    Ok(parts)
}

fn split(
    solid: &Solid,
    plane: Plane,
    side: &Side,
) -> std::result::Result<(Solid, Solid), clip::HalfError> {
    let outside = checked_half(
        solid,
        Plane {
            normal: -plane.normal,
            dist: -plane.dist,
        },
        side,
    )?;
    let inside = checked_half(solid, plane, side)?;
    Ok((outside, inside))
}

fn plan(
    document: &Document,
    cutters: &[u32],
    targets: &[u32],
) -> std::result::Result<(Vec<Planned>, Vec<u32>), EditError> {
    let targets = targets.iter().copied().collect::<BTreeSet<_>>();
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(invalid("Carve between 1 and 16 target brushes at a time"));
    }
    let ids = cutters.iter().copied().collect::<BTreeSet<_>>();
    if cutters.is_empty() || cutters.len() > MAX_CUTTERS || ids.len() != cutters.len() {
        return Err(invalid("Choose between 1 and 8 distinct cutter brushes"));
    }
    let mut cutters = Vec::new();
    for id in ids {
        if targets.contains(&id) {
            return Err(invalid("A cutter cannot also be a target brush"));
        }
        let (_, cutter) = subobject::solid(document, id)?;
        subobject::topology(cutter)?;
        if cutter.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Cutter {id}: carving displacements is unsupported"
            )));
        }
        cutters.push(cutter);
    }
    let mut planned = Vec::new();
    let mut unchanged = Vec::new();
    let mut count = 0;
    for id in targets {
        let (owner, source) = subobject::solid(document, id)?;
        subobject::topology(source)?;
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: carving displacements is unsupported"
            )));
        }
        let mut parts = vec![source.clone()];
        let mut affected = false;
        for cutter in &cutters {
            let mut remaining = Vec::new();
            for part in parts {
                if intersects(&part, cutter)? {
                    affected = true;
                    remaining.extend(subtract(&part, cutter)?);
                } else {
                    remaining.push(part);
                }
                if count + remaining.len() > MAX_FRAGMENTS {
                    return Err(invalid(
                        "Carving would exceed 256 fragments at an intermediate stage; reduce the selection",
                    ));
                }
            }
            parts = remaining;
            if parts.is_empty() {
                break;
            }
        }
        if !affected {
            unchanged.push(id);
            continue;
        }
        count += parts.len();
        if count > MAX_FRAGMENTS {
            return Err(invalid(
                "Carving would exceed 256 fragments; reduce the selection",
            ));
        }
        planned.push(Planned {
            owner,
            source: id,
            parts,
        });
    }
    if planned.is_empty() {
        return Err(invalid(
            "The cutter does not overlap any target brush; nothing changed",
        ));
    }
    for entity in document
        .vmf()
        .entities
        .iter()
        .filter(|entity| !entity.solids.is_empty())
    {
        if entity.solids.iter().all(|solid| {
            planned
                .iter()
                .any(|part| part.source == solid.id && part.parts.is_empty())
        }) {
            return Err(invalid(format!(
                "Carving would remove every brush from entity {}; keep a brush or delete the entity explicitly",
                entity.id
            )));
        }
    }
    Ok((planned, unchanged))
}

pub fn carve(
    tx: &mut Transaction,
    cutter: u32,
    targets: &[u32],
) -> std::result::Result<Result, EditError> {
    carve_many(tx, &[cutter], targets)
}

pub fn carve_many(
    tx: &mut Transaction,
    cutters: &[u32],
    targets: &[u32],
) -> std::result::Result<Result, EditError> {
    carve_many_with_material(tx, cutters, targets, &CutMaterialPolicy::Cutter)
}

pub fn carve_with_material(
    tx: &mut Transaction,
    cutter: u32,
    targets: &[u32],
    cut_material: &CutMaterialPolicy,
) -> std::result::Result<Result, EditError> {
    carve_many_with_material(tx, &[cutter], targets, cut_material)
}

fn target_surface<'a>(side: &Side, target: &'a Solid) -> std::result::Result<&'a Side, EditError> {
    let normal = Plane::from_points(side.plane.0).unwrap().normal;
    let mut matches = target.sides.iter().filter(|source| {
        let source_normal = Plane::from_points(source.plane.0).unwrap().normal;
        source_normal.dot(normal) >= 1.0 - TARGET_NORMAL_EPSILON
    });
    let source = matches.next().ok_or_else(|| {
        invalid(format!(
            "Brush {} has no original face parallel to a generated cut face with the same outward normal; choose cutter or named cut material",
            target.id
        ))
    })?;
    if matches.next().is_some() {
        return Err(invalid(format!(
            "Brush {} has ambiguous original faces for a generated cut face; choose cutter or named cut material",
            target.id
        )));
    }
    Ok(source)
}

pub fn carve_many_with_material(
    tx: &mut Transaction,
    cutters: &[u32],
    targets: &[u32],
    cut_material: &CutMaterialPolicy,
) -> std::result::Result<Result, EditError> {
    let cut_material = cut_material.normalized()?;
    let (planned, unchanged) = plan(tx.document(), cutters, targets)?;
    let cutters = cutters
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cutter_sides = cutters
        .iter()
        .map(|id| {
            subobject::solid(tx.document(), *id).map(|(_, solid)| {
                solid
                    .sides
                    .iter()
                    .map(|side| (side.id, *id))
                    .collect::<Vec<_>>()
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<BTreeMap<_, _>>();
    let mut result = Result {
        cutter: cutters[0],
        cutters,
        cut_material: cut_material.clone(),
        changes: Vec::new(),
        unchanged,
        removed: Vec::new(),
        objects: Vec::new(),
        faces: Vec::new(),
    };
    let mut allocated = Vec::new();
    for mut part in planned {
        for (index, solid) in part.parts.iter_mut().enumerate() {
            if index > 0 {
                solid.id = tx.new_object_id()?;
            }
            for side in &mut solid.sides {
                let source_side = side.id;
                let from_cutter = cutter_sides.contains_key(&source_side);
                let source_solid = cutter_sides
                    .get(&source_side)
                    .copied()
                    .unwrap_or(part.source);
                let (surface_source_solid, surface_source_side) = if from_cutter {
                    let source = match &cut_material {
                        CutMaterialPolicy::Target => {
                            let (_, target) = subobject::solid(tx.document(), part.source)?;
                            target_surface(side, target)?
                        }
                        _ => &*side,
                    };
                    let surface_source = (
                        if cut_material == CutMaterialPolicy::Target {
                            part.source
                        } else {
                            source_solid
                        },
                        source.id,
                    );
                    let plane = side.plane;
                    let mut surface = source.clone();
                    surface.plane = plane;
                    if let CutMaterialPolicy::Material(material) = &cut_material {
                        surface.material = material.clone();
                    }
                    if cut_material != CutMaterialPolicy::Cutter {
                        crate::surface::update(
                            &mut surface,
                            &crate::surface::SurfaceEdit::default(),
                            &[],
                        )?;
                    }
                    *side = surface;
                    surface_source
                } else {
                    (source_solid, source_side)
                };
                if index > 0 || from_cutter {
                    side.id = tx.new_side_id()?;
                }
                result.faces.push(Inheritance {
                    solid: solid.id,
                    side: side.id,
                    source_solid,
                    source_side,
                    surface_source_solid,
                    surface_source_side,
                    cutter: from_cutter,
                });
            }
            subobject::topology(solid)?;
        }
        if part.parts.is_empty() {
            result.removed.push(part.source);
        }
        result.changes.push(Change {
            source: part.source,
            fragments: part.parts.iter().map(|solid| solid.id).collect(),
        });
        allocated.push(part);
    }
    for mut part in allocated {
        match part.owner {
            ObjectId::Solid(id) => {
                if part.parts.is_empty() {
                    tx.remove(part.owner)?;
                } else {
                    *tx.solid_mut(id)? = part.parts.remove(0);
                    result.objects.push(part.owner);
                    for solid in part.parts {
                        result.objects.push(tx.add_solid(solid)?);
                    }
                }
            }
            ObjectId::Entity(id) => {
                let entity = tx.entity_mut(id)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == part.source)
                    .unwrap();
                entity.solids.splice(index..=index, part.parts);
                result.objects.push(part.owner);
            }
        }
    }
    result.objects.sort();
    result.objects.dedup();
    Ok(result)
}
