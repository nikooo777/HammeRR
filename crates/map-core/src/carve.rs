use std::collections::{BTreeMap, BTreeSet};

use source_formats::vmf::{Side, Solid};

use crate::plane::ON_EPSILON;
use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction, clip, subobject};

pub const MAX_TARGETS: usize = 16;
pub const MAX_CUTTERS: usize = 8;
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
    pub cutters: Vec<u32>,
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
                if index > 0 || from_cutter {
                    side.id = tx.new_side_id()?;
                }
                result.faces.push(Inheritance {
                    solid: solid.id,
                    side: side.id,
                    source_solid: cutter_sides
                        .get(&source_side)
                        .copied()
                        .unwrap_or(part.source),
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

    fn literal_pumpkin_wall(document: &mut Document) -> u32 {
        let points = [
            [
                [277.2870973730347, 29.566191478555105, 262.6972118647972],
                [299.5661914785551, 7.287097373034776, 262.6972118647972],
                [299.5661914785551, 7.287097373034769, 230.26278813520275],
            ],
            [
                [277.2870973730347, 29.566191478555098, 230.26278813520275],
                [299.5661914785551, 7.287097373034769, 230.26278813520275],
                [311.19087192664085, 27.421634530871863, 204.02278813520277],
            ],
            [
                [299.5661914785551, 7.287097373034776, 262.6972118647972],
                [330.0, -0.867617042889826, 262.6972118647972],
                [330.0, -0.8676170428898331, 230.26278813520275],
            ],
            [
                [299.5661914785551, 7.287097373034769, 230.26278813520275],
                [330.0, -0.8676170428898331, 230.26278813520275],
                [330.0, 22.381743853281712, 204.02278813520277],
            ],
            [
                [259.76319005239566, 133.6033369978714, 16.56441888656123],
                [275.7631900523956, 73.89052407676932, -3.961984380783987e-14],
                [271.47600297349766, 89.89052407676927, -61.819252882500436],
            ],
            [
                [276.4085686412268, 10.06345274845895, 252.5317012372206],
                [270.8120755403682, 72.56387694163755, 239.94693899692467],
                [227.6984721369813, 78.16037004249615, 286.9139110788836],
            ],
            [
                [-9.023909082847101, 54.9760909171529, 31.756897638072008],
                [2.1690771188701894, 2.1690771188701876, -2.625312203590949],
                [-50.637936679412505, 13.36206332058748, -37.007522045253936],
            ],
            [
                [192.49771223766027, 128.49771223766055, -45.25483399593913],
                [
                    160.49771223766027,
                    160.4977122376605,
                    -5.0399249524069817e-14,
                ],
                [128.4977122376602, 192.4977122376605, -45.25483399593906],
            ],
        ];
        let ObjectId::Solid(id) = document
            .edit("literal wall", |tx| {
                let id = tx.new_object_id()?;
                let sides = points
                    .into_iter()
                    .map(|points| edit::new_side(tx, points.map(DVec3::from_array), "brick/wall"))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                tx.add_solid(Solid {
                    id,
                    sides,
                    editor: None,
                    hidden: false,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                })
            })
            .unwrap()
        else {
            unreachable!()
        };
        id
    }

    #[test]
    fn alternate_partition_preserves_literal_wall_volume_for_cutter_plane_orders() {
        let mut document = Document::empty();
        let target = literal_pumpkin_wall(&mut document);
        let ids = document
            .edit("eye cutter", |tx| {
                primitive::create(
                    tx,
                    &primitive::Spec {
                        shape: primitive::Shape::Cone,
                        axis: primitive::Axis::Y,
                        mins: DVec3::new(286.0, -14.0, 248.0),
                        maxs: DVec3::new(320.0, 34.4, 274.0),
                        segments: 3,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let ObjectId::Solid(cutter_id) = ids[0] else {
            unreachable!()
        };
        let source = document.solid(target).unwrap();
        let cutter = document.solid(cutter_id).unwrap();
        subobject::topology(source).unwrap();
        let plane = Plane::from_points(cutter.sides[1].plane.0).unwrap();
        assert!(matches!(
            checked_half(
                source,
                Plane {
                    normal: -plane.normal,
                    dist: -plane.dist,
                },
                &cutter.sides[1],
            ),
            Err(clip::HalfError::SmallVolume(_))
        ));
        let mut intersection = source.clone();
        for side in &cutter.sides {
            let plane = Plane::from_points(side.plane.0).unwrap();
            if matches!(position(&intersection, plane).unwrap(), Position::Crossing) {
                intersection = half(&intersection, plane, side).unwrap();
            }
        }
        let expected = volume(source) - volume(&intersection);
        assert!(expected > 0.0);
        for order in [[0, 1, 2, 3], [0, 2, 3, 1], [3, 2, 1, 0]] {
            let mut reordered = cutter.clone();
            reordered.sides = order
                .iter()
                .map(|index| cutter.sides[*index].clone())
                .collect();
            let parts = subtract(source, &reordered).unwrap();
            let actual: f64 = parts.iter().map(volume).sum();
            assert!(
                (actual - expected).abs() < 1e-6,
                "{order:?}: {actual} != {expected}"
            );
            for (index, part) in parts.iter().enumerate() {
                subobject::topology(part).unwrap();
                assert!(volume(part) > 1e-6);
                assert!(!intersects(part, cutter).unwrap());
                for face in BrushGeometry::from_solid(part).unwrap().faces {
                    for point in face.vertices {
                        assert!(source.sides.iter().all(|side| {
                            Plane::from_points(side.plane.0).unwrap().distance_to(point)
                                <= ON_EPSILON
                        }));
                    }
                }
                for other in &parts[index + 1..] {
                    assert!(!intersects(part, other).unwrap());
                }
            }
        }
    }

    #[test]
    fn unavoidable_small_final_fragment_is_refused_atomically() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::ZERO, DVec3::ONE);
        let cutter = block(&mut document, DVec3::splat(-1.0), DVec3::splat(2.0));
        document
            .edit("corner plane", |tx| {
                let plane = Plane {
                    normal: DVec3::ONE.normalize(),
                    dist: 2.99 / 3.0_f64.sqrt(),
                };
                let side = edit::new_side(tx, clip::plane_points(plane), "brick/cut")?;
                tx.solid_mut(cutter)?.sides.push(side);
                Ok(())
            })
            .unwrap();
        subobject::topology(document.solid(cutter).unwrap()).unwrap();
        let before = document.vmf().clone();
        let revision = document.revision();
        let history = document.undo_labels().count();
        assert!(
            document
                .edit("carve corner", |tx| carve(tx, cutter, &[target]))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
        assert_eq!(document.undo_labels().count(), history);
    }

    #[test]
    fn overlapping_cutters_subtract_union_volume_once_with_stable_order_and_inheritance() {
        let mut document = Document::empty();
        let target = block(&mut document, DVec3::ZERO, DVec3::splat(128.0));
        let first = block(&mut document, DVec3::splat(16.0), DVec3::splat(80.0));
        let second = block(&mut document, DVec3::splat(48.0), DVec3::splat(112.0));
        document
            .edit("cutter settings", |tx| {
                for (id, material, scale) in
                    [(first, "brick/first", 8), (second, "brick/second", 32)]
                {
                    for side in &mut tx.solid_mut(id)?.sides {
                        side.material = material.into();
                        side.lightmap_scale = scale;
                        side.extra_properties.set("cut_source", material);
                    }
                }
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        let mut reverse = document.preview_snapshot();
        let expected = reverse
            .edit("reversed", |tx| carve_many(tx, &[second, first], &[target]))
            .unwrap();
        let result = document
            .edit("batch carve", |tx| {
                carve_many(tx, &[first, second], &[target])
            })
            .unwrap();
        assert_eq!(result, expected);
        assert_eq!(document.vmf(), reverse.vmf());
        assert_eq!(result.cutters, vec![first, second]);
        let remaining: f64 = result.changes[0]
            .fragments
            .iter()
            .map(|id| volume(document.solid(*id).unwrap()))
            .sum();
        assert!((remaining - (128f64.powi(3) - 2.0 * 64f64.powi(3) + 32f64.powi(3))).abs() < 1e-5);
        for face in &result.faces {
            let old = source
                .solids()
                .find(|solid| solid.id == face.source_solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id == face.source_side)
                .unwrap();
            let new = document
                .solid(face.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id == face.side)
                .unwrap();
            assert_eq!(new.material, old.material);
            assert_eq!(new.u_axis, old.u_axis);
            assert_eq!(new.v_axis, old.v_axis);
            assert_eq!(new.lightmap_scale, old.lightmap_scale);
            assert_eq!(new.extra_properties, old.extra_properties);
        }
        assert_eq!(
            document.solid(first).unwrap(),
            source.solids().find(|solid| solid.id == first).unwrap()
        );
        assert_eq!(
            document.solid(second).unwrap(),
            source.solids().find(|solid| solid.id == second).unwrap()
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &source);
    }

    #[test]
    fn combined_full_removal_and_invalid_cutter_sets_are_atomic() {
        for entity in [false, true] {
            let mut document = Document::empty();
            let target = block(&mut document, DVec3::ZERO, DVec3::splat(64.0));
            let first = block(&mut document, DVec3::ZERO, DVec3::new(32.0, 64.0, 64.0));
            let second = block(
                &mut document,
                DVec3::new(32.0, 0.0, 0.0),
                DVec3::splat(64.0),
            );
            if entity {
                document
                    .edit("owner", |tx| {
                        edit::tie_to_entity(tx, &[target], "func_detail", &[])
                    })
                    .unwrap();
            }
            let source = document.vmf().clone();
            for cutters in [
                vec![],
                vec![first, first],
                vec![first, second, target],
                vec![first, second, 999_999],
                vec![first; MAX_CUTTERS + 1],
            ] {
                assert!(
                    document
                        .edit("invalid batch", |tx| carve_many(tx, &cutters, &[target]))
                        .is_err()
                );
                assert_eq!(document.vmf(), &source);
            }
            let result = document.edit("combined removal", |tx| {
                carve_many(tx, &[first, second], &[target])
            });
            if entity {
                assert!(result.is_err());
                assert_eq!(document.vmf(), &source);
            } else {
                assert_eq!(result.unwrap().removed, vec![target]);
                assert!(document.solid(target).is_none());
                document.undo().unwrap();
                assert_eq!(document.vmf(), &source);
            }
        }
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
        for (id, (mins, maxs)) in result.changes[0].fragments.iter().zip([
            ([0.0, 0.0, 48.0], [64.0, 64.0, 64.0]),
            ([0.0, 0.0, 0.0], [64.0, 64.0, 16.0]),
            ([0.0, 0.0, 16.0], [16.0, 64.0, 48.0]),
            ([48.0, 0.0, 16.0], [64.0, 64.0, 48.0]),
            ([16.0, 48.0, 16.0], [48.0, 64.0, 48.0]),
            ([16.0, 0.0, 16.0], [48.0, 16.0, 48.0]),
        ]) {
            let geometry = BrushGeometry::from_solid(document.solid(*id).unwrap()).unwrap();
            assert_eq!(geometry.mins, DVec3::from_array(mins));
            assert_eq!(geometry.maxs, DVec3::from_array(maxs));
        }
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
