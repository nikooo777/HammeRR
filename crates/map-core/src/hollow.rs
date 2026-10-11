use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::vmf::Solid;

use crate::{Document, EditError, ObjectId, Plane, Transaction, clip, subobject};

pub const MAX_BRUSHES: usize = 16;
pub const MAX_PARTS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub thickness: f64,
    pub outward: bool,
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
    let mut shells = Vec::new();
    let mut total = 0;
    for id in sources {
        let (owner, source) = subobject::solid(document, id)?;
        subobject::topology(source)?;
        if total + source.sides.len() > MAX_PARTS {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushGeometry, edit, primitive, query};
    use glam::DVec3;

    fn fixture() -> (Document, ObjectId) {
        let mut document = Document::empty();
        let id = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(128.0), "brick/test")
            })
            .unwrap();
        (document, id)
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
                        (face.vertices[0] - center).dot((pair[1] - center).cross(pair[0] - center))
                            / 6.0
                    })
                    .sum::<f64>()
            })
            .sum()
    }

    #[test]
    fn inward_and_outward_shells_have_expected_bounds_volume_and_unique_ids() {
        for outward in [false, true] {
            let (mut document, id) = fixture();
            let before = document.vmf().clone();
            let result = document
                .edit("hollow", |tx| {
                    hollow(
                        tx,
                        &[id],
                        &Spec {
                            thickness: 16.0,
                            outward,
                        },
                    )
                })
                .unwrap();
            assert_eq!(result.brushes.len(), 6);
            let size: f64 = if outward { 160.0 } else { 128.0 };
            let inside: f64 = if outward { 128.0 } else { 96.0 };
            let sum: f64 = document.vmf().solids().map(volume).sum();
            assert!((sum - (size.powi(3) - inside.powi(3))).abs() < 1e-5);
            let (lo, hi) = query::bounds_of(&document, &result.objects).unwrap();
            assert_eq!(lo, DVec3::splat(if outward { -16.0 } else { 0.0 }));
            assert_eq!(hi, DVec3::splat(if outward { 144.0 } else { 128.0 }));
            let side_ids = document
                .vmf()
                .solids()
                .flat_map(|solid| solid.sides.iter().map(|side| side.id))
                .collect::<Vec<_>>();
            assert_eq!(
                side_ids.len(),
                side_ids.iter().collect::<BTreeSet<_>>().len()
            );
            let after = document.vmf().clone();
            document.undo().unwrap();
            assert_eq!(*document.vmf(), before);
            document.redo().unwrap();
            assert_eq!(*document.vmf(), after);
        }
    }

    #[test]
    fn invalid_thickness_and_mixed_invalid_batches_roll_back() {
        for thickness in [0.0, -1.0, 64.0, 80.0, 4097.0, f64::NAN] {
            let (mut document, id) = fixture();
            let before = document.vmf().clone();
            let revision = document.revision();
            assert!(
                document
                    .edit("hollow", |tx| hollow(
                        tx,
                        &[id],
                        &Spec {
                            thickness,
                            outward: false
                        }
                    ))
                    .is_err()
            );
            assert_eq!(*document.vmf(), before);
            assert_eq!(document.revision(), revision);
        }
        let (mut document, id) = fixture();
        let other = document
            .edit("thin", |tx| {
                edit::create_block(
                    tx,
                    DVec3::splat(256.0),
                    DVec3::new(264.0, 384.0, 384.0),
                    "brick/test",
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("hollow", |tx| hollow(
                    tx,
                    &[id, other],
                    &Spec {
                        thickness: 16.0,
                        outward: false
                    }
                ))
                .is_err()
        );
        assert_eq!(*document.vmf(), before);
    }

    #[test]
    fn sloped_brush_entity_keeps_ownership_and_round_trips() {
        let mut document = Document::empty();
        let ids = document
            .edit("wedge", |tx| {
                primitive::create(tx, &primitive::Spec::default())
            })
            .unwrap();
        let ObjectId::Solid(id) = ids[0] else {
            unreachable!()
        };
        let owner = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])
            })
            .unwrap();
        let result = document
            .edit("hollow", |tx| {
                hollow(
                    tx,
                    &[owner],
                    &Spec {
                        thickness: 8.0,
                        outward: false,
                    },
                )
            })
            .unwrap();
        assert_eq!(result.objects, vec![owner]);
        assert_eq!(result.brushes.len(), 5);
        for id in result.brushes {
            assert_eq!(document.owner_of_solid(id), Some(owner));
        }
        let parsed =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        assert_eq!(parsed.entities[0].solids.len(), 5);
        assert!(
            parsed.entities[0]
                .solids
                .iter()
                .all(|solid| subobject::topology(solid).is_ok())
        );
    }

    #[test]
    fn generated_caps_inherit_mapping_lighting_and_unknown_fields() {
        let (mut document, id) = fixture();
        let ObjectId::Solid(solid) = id else {
            unreachable!()
        };
        document
            .edit("metadata", |tx| {
                for side in &mut tx.solid_mut(solid)?.sides {
                    side.extra_properties.set("custom_surface", "preserved");
                    side.extra.push(source_formats::keyvalues::Node::string(
                        "custom_block",
                        "preserved",
                    ));
                    side.lightmap_scale = 8;
                    side.smoothing_groups = Some(7);
                    side.u_axis.offset = 13.5;
                }
                Ok(())
            })
            .unwrap();
        let source = document.solid(solid).unwrap().clone();
        document
            .edit("hollow", |tx| {
                hollow(
                    tx,
                    &[id],
                    &Spec {
                        thickness: 16.0,
                        outward: false,
                    },
                )
            })
            .unwrap();
        for side in document.vmf().solids().flat_map(|solid| &solid.sides) {
            assert_eq!(
                side.extra_properties.get("custom_surface"),
                Some("preserved")
            );
            assert_eq!(side.extra, source.sides[0].extra);
            assert_eq!(side.lightmap_scale, 8);
            assert_eq!(side.smoothing_groups, Some(7));
            assert!(
                source.sides.iter().any(
                    |original| original.u_axis == side.u_axis && original.v_axis == side.v_axis
                )
            );
        }
    }

    #[test]
    fn brush_and_wall_budgets_reject_without_mutation() {
        let mut document = Document::empty();
        let ids = (0..17)
            .map(|index| {
                document
                    .edit("block", |tx| {
                        edit::create_block(
                            tx,
                            DVec3::splat(index as f64 * 128.0),
                            DVec3::splat(index as f64 * 128.0 + 64.0),
                            "brick/test",
                        )
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("hollow", |tx| hollow(
                    tx,
                    &ids,
                    &Spec {
                        thickness: 8.0,
                        outward: false
                    }
                ))
                .is_err()
        );
        assert_eq!(*document.vmf(), before);
        let mut document = Document::empty();
        let mut ids = Vec::new();
        for index in 0..9 {
            let origin = DVec3::X * (index as f64 * 256.0);
            ids.extend(
                document
                    .edit("cylinder", |tx| {
                        primitive::create(
                            tx,
                            &primitive::Spec {
                                shape: primitive::Shape::Cylinder,
                                segments: 30,
                                mins: origin,
                                maxs: origin + DVec3::splat(128.0),
                                ..Default::default()
                            },
                        )
                    })
                    .unwrap(),
            );
        }
        let before = document.vmf().clone();
        let error = document
            .edit("hollow", |tx| {
                hollow(
                    tx,
                    &ids,
                    &Spec {
                        thickness: 4.0,
                        outward: false,
                    },
                )
            })
            .unwrap_err();
        assert!(error.to_string().contains("256"), "{error}");
        assert_eq!(*document.vmf(), before);
    }
}
