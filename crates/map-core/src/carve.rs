use std::collections::BTreeSet;

use source_formats::vmf::{Side, Solid};

use crate::plane::ON_EPSILON;
use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction, clip, subobject};

pub const MAX_TARGETS: usize = 16;
pub const MAX_FRAGMENTS: usize = 256;

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
    pub cutter: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Result {
    pub cutter: u32,
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
    let mut part = clip::half(solid, plane, &source.material)?;
    let cap = part.sides.last_mut().unwrap();
    let plane = cap.plane;
    *cap = source.clone();
    cap.plane = plane;
    subobject::topology(&part)?;
    Ok(part)
}

fn intersects(source: &Solid, cutter: &Solid) -> std::result::Result<bool, EditError> {
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

fn subtract(source: &Solid, cutter: &Solid) -> std::result::Result<Vec<Solid>, EditError> {
    let mut inside = source.clone();
    let mut parts = Vec::new();
    for side in &cutter.sides {
        let plane = Plane::from_points(side.plane.0).unwrap();
        match position(&inside, plane)? {
            Position::Inside => {}
            Position::Outside => {
                parts.push(inside);
                break;
            }
            Position::Crossing => {
                parts.push(half(
                    &inside,
                    Plane {
                        normal: -plane.normal,
                        dist: -plane.dist,
                    },
                    side,
                )?);
                inside = half(&inside, plane, side)?;
            }
        }
    }
    Ok(parts)
}

fn plan(
    document: &Document,
    cutter: u32,
    targets: &[u32],
) -> std::result::Result<(Vec<Planned>, Vec<u32>), EditError> {
    let targets = targets.iter().copied().collect::<BTreeSet<_>>();
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(invalid("Carve between 1 and 16 target brushes at a time"));
    }
    if targets.contains(&cutter) {
        return Err(invalid("The cutter cannot also be a target brush"));
    }
    let (_, cutter) = subobject::solid(document, cutter)?;
    subobject::topology(cutter)?;
    if cutter.sides.iter().any(|side| side.displacement.is_some()) {
        return Err(invalid("Carving with displacement brushes is unsupported"));
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
        if !intersects(source, cutter)? {
            unchanged.push(id);
            continue;
        }
        let parts = subtract(source, cutter)?;
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
    let (planned, unchanged) = plan(tx.document(), cutter, targets)?;
    let cutter_sides = subobject::solid(tx.document(), cutter)?
        .1
        .sides
        .iter()
        .map(|side| side.id)
        .collect::<BTreeSet<_>>();
    let mut result = Result {
        cutter,
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
                let from_cutter = cutter_sides.contains(&source_side);
                if index > 0 || from_cutter {
                    side.id = tx.new_side_id()?;
                }
                result.faces.push(Inheritance {
                    solid: solid.id,
                    side: side.id,
                    source_solid: if from_cutter { cutter } else { part.source },
                    source_side,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{edit, primitive};
    use glam::DVec3;
    use source_formats::keyvalues::Node;

    fn block(document: &mut Document, mins: DVec3, maxs: DVec3) -> u32 {
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, mins, maxs, "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        id
    }

    fn volume(solid: &Solid) -> f64 {
        let shape = BrushGeometry::from_solid(solid).unwrap();
        let center = shape.center();
        shape
            .faces
            .iter()
            .map(|face| {
                face.vertices[1..]
                    .windows(2)
                    .map(|pair| {
                        (face.vertices[0] - center).dot((pair[0] - center).cross(pair[1] - center))
                            / 6.0
                    })
                    .sum::<f64>()
            })
            .sum::<f64>()
            .abs()
    }

    #[test]
    fn enclosed_cutter_creates_exact_nonoverlapping_shell_and_round_trips() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::ZERO, DVec3::splat(64.0));
        let cutter = block(&mut document, DVec3::splat(16.0), DVec3::splat(48.0));
        let source = document.vmf().clone();
        let original_cutter = document.solid(cutter).unwrap().clone();
        let history = document.undo_labels().count();
        let result = document
            .edit("carve", |tx| carve(tx, cutter, &[target]))
            .unwrap();
        assert_eq!(result.changes[0].fragments.len(), 6);
        assert_eq!(result.changes[0].fragments[0], target);
        assert!(result.removed.is_empty());
        assert_eq!(document.solid(cutter), Some(&original_cutter));
        let sum = result.changes[0]
            .fragments
            .iter()
            .map(|id| volume(document.solid(*id).unwrap()))
            .sum::<f64>();
        assert!((sum - (64.0_f64.powi(3) - 32.0_f64.powi(3))).abs() < 1e-6);
        for (index, id) in result.changes[0].fragments.iter().enumerate() {
            let solid = document.solid(*id).unwrap();
            subobject::topology(solid).unwrap();
            assert!(!intersects(solid, &original_cutter).unwrap());
            for other in &result.changes[0].fragments[index + 1..] {
                assert!(!intersects(solid, document.solid(*other).unwrap()).unwrap());
            }
        }
        document.vmf().validate_ids().unwrap();
        let changed = document.vmf().clone();
        let text = changed.write(&Default::default());
        let reopened = source_formats::vmf::Vmf::parse(&text).unwrap();
        reopened.validate_ids().unwrap();
        assert_eq!(reopened.solids().count(), 7);
        assert_eq!(document.undo_labels().count(), history + 1);
        document.undo().unwrap();
        assert_eq!(document.vmf(), &source);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &changed);
    }

    #[test]
    fn partial_full_and_disjoint_targets_are_reported_and_atomic() {
        let mut document = Document::empty();
        let partial = block(&mut document, DVec3::ZERO, DVec3::splat(64.0));
        let full = block(
            &mut document,
            DVec3::new(48.0, 16.0, 16.0),
            DVec3::new(56.0, 32.0, 32.0),
        );
        let untouched = block(&mut document, DVec3::splat(128.0), DVec3::splat(160.0));
        let tangent = block(
            &mut document,
            DVec3::new(80.0, 0.0, 0.0),
            DVec3::new(96.0, 64.0, 64.0),
        );
        let cutter = block(
            &mut document,
            DVec3::new(32.0, -16.0, -16.0),
            DVec3::new(80.0, 80.0, 80.0),
        );
        let original = document.vmf().clone();
        let result = document
            .edit("carve", |tx| {
                carve(tx, cutter, &[partial, full, untouched, tangent])
            })
            .unwrap();
        assert_eq!(result.changes.len(), 2);
        assert_eq!(result.changes[0].fragments, vec![partial]);
        assert_eq!(result.removed, vec![full]);
        assert_eq!(result.unchanged, vec![untouched, tangent]);
        assert!((volume(document.solid(partial).unwrap()) - 32.0 * 64.0 * 64.0).abs() < 1e-6);
        assert!(document.solid(full).is_none());
        assert_eq!(
            document.solid(untouched),
            original.solids().find(|solid| solid.id == untouched)
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &original);
        let history = document.undo_labels().count();
        assert!(
            document
                .edit("no overlap", |tx| carve(tx, cutter, &[untouched, tangent]))
                .is_err()
        );
        assert_eq!(document.vmf(), &original);
        assert_eq!(document.undo_labels().count(), history);
    }

    #[test]
    fn inheritance_keeps_world_uvs_lighting_and_unknown_metadata() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::ZERO, DVec3::splat(64.0));
        let cutter = block(&mut document, DVec3::splat(16.0), DVec3::splat(48.0));
        document
            .edit("metadata", |tx| {
                for id in [target, cutter] {
                    let solid = tx.solid_mut(id)?;
                    solid
                        .extra
                        .push(Node::string("vendor", format!("brush-{id}")));
                    for side in &mut solid.sides {
                        side.material = format!("CUSTOM/FACE_{}", side.id);
                        side.u_axis.offset = 17.25;
                        side.v_axis.scale = -0.75;
                        side.lightmap_scale = 32;
                        side.smoothing_groups = Some(7);
                        side.extra.push(Node::string("vendor", "face"));
                    }
                }
                Ok(())
            })
            .unwrap();
        let original = document.vmf().clone();
        let result = document
            .edit("carve", |tx| carve(tx, cutter, &[target]))
            .unwrap();
        assert!(result.faces.iter().any(|face| face.cutter));
        assert!(result.faces.iter().any(|face| !face.cutter));
        for inheritance in &result.faces {
            let source = original
                .solids()
                .find(|solid| solid.id == inheritance.source_solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id == inheritance.source_side)
                .unwrap();
            let side = document
                .solid(inheritance.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id == inheritance.side)
                .unwrap();
            let mut expected = source.clone();
            expected.id = side.id;
            expected.plane = side.plane;
            assert_eq!(side, &expected);
        }
        for id in &result.changes[0].fragments {
            assert_eq!(
                document.solid(*id).unwrap().extra,
                original
                    .solids()
                    .find(|solid| solid.id == target)
                    .unwrap()
                    .extra
            );
        }
        document.vmf().validate_ids().unwrap();
    }

    #[test]
    fn brush_entity_ownership_is_retained_and_empty_owners_reject() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::ZERO, DVec3::splat(64.0));
        let cutter = block(&mut document, DVec3::splat(16.0), DVec3::splat(48.0));
        let owner = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &[target], "func_detail", &[])
            })
            .unwrap();
        let source = document.vmf().clone();
        let result = document
            .edit("carve", |tx| carve(tx, cutter, &[target]))
            .unwrap();
        assert_eq!(result.objects, vec![owner]);
        let ObjectId::Entity(entity) = owner else {
            unreachable!()
        };
        assert_eq!(document.entity(entity).unwrap().solids.len(), 6);
        assert_eq!(
            document.entity(entity).unwrap().properties,
            source.entities[0].properties
        );
        document.vmf().validate_ids().unwrap();
        document.undo().unwrap();
        assert_eq!(document.vmf(), &source);
        document
            .edit("grow cutter", |tx| {
                let mut replacement =
                    edit::block_solid(tx, DVec3::splat(-16.0), DVec3::splat(80.0), "brick/test")?;
                replacement.id = cutter;
                *tx.solid_mut(cutter)? = replacement;
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        assert!(
            document
                .edit("empty owner", |tx| carve(tx, cutter, &[target]))
                .unwrap_err()
                .to_string()
                .contains("every brush")
        );
        assert_eq!(document.vmf(), &source);
    }

    #[test]
    fn invalid_later_targets_restore_geometry_history_and_allocators() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::ZERO, DVec3::splat(64.0));
        let invalid_target = block(&mut document, DVec3::splat(128.0), DVec3::splat(192.0));
        let cutter = block(&mut document, DVec3::splat(16.0), DVec3::splat(48.0));
        document
            .edit("bad plane", |tx| {
                tx.solid_mut(invalid_target)?.sides[0].plane.0 = [DVec3::ZERO; 3];
                Ok(())
            })
            .unwrap();
        let original = document.vmf().clone();
        let history = document.undo_labels().count();
        for targets in [
            vec![],
            vec![cutter],
            vec![target, invalid_target],
            vec![999_999],
        ] {
            assert!(
                document
                    .edit("reject", |tx| carve(tx, cutter, &targets))
                    .is_err()
            );
            assert_eq!(document.vmf(), &original);
            assert_eq!(document.undo_labels().count(), history);
        }
        let allocated = document
            .edit("next ids", |tx| {
                Ok((tx.new_object_id()?, tx.new_side_id()?))
            })
            .unwrap();
        assert_eq!(allocated, (5, 19));
    }

    #[test]
    fn rotated_cutter_preserves_subtracted_volume() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::splat(-128.0), DVec3::splat(128.0));
        let cutter = document
            .edit("cylinder", |tx| {
                primitive::create(
                    tx,
                    &primitive::Spec {
                        shape: primitive::Shape::Cylinder,
                        mins: DVec3::splat(-32.0),
                        maxs: DVec3::splat(32.0),
                        segments: 12,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let ObjectId::Solid(cutter) = cutter[0] else {
            unreachable!()
        };
        let cutter_volume = volume(document.solid(cutter).unwrap());
        let result = document
            .edit("carve", |tx| carve(tx, cutter, &[target]))
            .unwrap();
        let sum = result.changes[0]
            .fragments
            .iter()
            .map(|id| volume(document.solid(*id).unwrap()))
            .sum::<f64>();
        assert!((sum - (256.0_f64.powi(3) - cutter_volume)).abs() < 1e-4);
        document.vmf().validate_ids().unwrap();
    }

    #[test]
    fn fragment_budgets_and_displacements_reject_without_allocating() {
        let mut document = Document::empty();
        let mut targets = Vec::new();
        for _ in 0..5 {
            targets.push(block(
                &mut document,
                DVec3::splat(-128.0),
                DVec3::splat(128.0),
            ));
        }
        let cutter = document
            .edit("many cutter planes", |tx| {
                primitive::create(
                    tx,
                    &primitive::Spec {
                        shape: primitive::Shape::Cylinder,
                        mins: DVec3::splat(-32.0),
                        maxs: DVec3::splat(32.0),
                        segments: 62,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let ObjectId::Solid(cutter) = cutter[0] else {
            unreachable!()
        };
        let source = document.vmf().clone();
        let history = document.undo_labels().count();
        let error = document
            .edit("overflow", |tx| carve(tx, cutter, &targets))
            .unwrap_err();
        assert!(error.to_string().contains("256 fragments"), "{error}");
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.undo_labels().count(), history);
        let ids = document
            .edit("next ids", |tx| {
                Ok((tx.new_object_id()?, tx.new_side_id()?))
            })
            .unwrap();
        assert_eq!(ids, (8, 95));
        for id in [targets[0], cutter] {
            let mut displaced = Document::new(source.clone());
            displaced
                .edit("displacement", |tx| {
                    tx.solid_mut(id)?.sides[0].displacement = Some(source_formats::vmf::DispInfo {
                        power: 2,
                        start_position: DVec3::splat(-32.0),
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
                    Ok(())
                })
                .unwrap();
            let source = displaced.vmf().clone();
            assert!(
                displaced
                    .edit("reject displacement", |tx| carve(tx, cutter, &[targets[0]]))
                    .unwrap_err()
                    .to_string()
                    .contains("displacement")
            );
            assert_eq!(displaced.vmf(), &source);
        }
    }
}
