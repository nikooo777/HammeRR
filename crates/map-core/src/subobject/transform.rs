use glam::{DMat3, DVec3, EulerRot};

use super::{
    Component, EPSILON, MAX_COORDINATE, Replacement, invalid, plan_affine, solid, topology,
};
use crate::{Document, EditError};

#[derive(Debug, Clone, PartialEq)]
pub enum ComponentTransform {
    Scale(DVec3),
    Rotate(DVec3),
}

pub struct TransformPlan {
    pub replacements: Vec<Replacement>,
    pub pivot: DVec3,
}

pub fn component_center(document: &Document, components: &[Component]) -> Result<DVec3, EditError> {
    if components.is_empty() || components.len() > super::MAX_COMPONENTS {
        return Err(invalid("Select between 1 and 512 faces, vertices or edges"));
    }
    let ids = components
        .iter()
        .map(|component| component.solid)
        .collect::<std::collections::BTreeSet<_>>();
    if ids.len() > super::MAX_SOLIDS {
        return Err(invalid(
            "Component editing supports at most 16 brushes at a time",
        ));
    }
    let mut mins = DVec3::splat(f64::INFINITY);
    let mut maxs = DVec3::splat(f64::NEG_INFINITY);
    for id in ids {
        let shape = topology(solid(document, id)?.1)?;
        for component in components.iter().filter(|component| component.solid == id) {
            for index in shape.resolve(&component.sides)? {
                let point = shape.vertices[index].position;
                mins = mins.min(point);
                maxs = maxs.max(point);
            }
        }
    }
    Ok((mins + maxs) * 0.5)
}

pub fn plan_transform(
    document: &Document,
    components: &[Component],
    transform: &ComponentTransform,
    pivot: Option<DVec3>,
    texture_lock: bool,
) -> Result<TransformPlan, EditError> {
    let center = component_center(document, components)?;
    let pivot = pivot.unwrap_or(center);
    if !pivot.is_finite() || pivot.abs().max_element() > MAX_COORDINATE {
        return Err(invalid(
            "Pivot needs finite world coordinates within ±65536 units",
        ));
    }
    let linear = match transform {
        ComponentTransform::Scale(factors) => {
            if !factors.is_finite() || factors.min_element() <= 0.0 || *factors == DVec3::ONE {
                return Err(invalid(
                    "Scale needs positive finite X/Y/Z factors and a nonidentity change",
                ));
            }
            DMat3::from_diagonal(*factors)
        }
        ComponentTransform::Rotate(degrees) => {
            if !degrees.is_finite() {
                return Err(invalid("Rotation needs finite X/Y/Z angles in degrees"));
            }
            let normalized = degrees.map(|angle| angle.rem_euclid(360.0));
            if normalized == DVec3::ZERO {
                return Err(invalid(
                    "Rotation must change at least one angle modulo 360 degrees",
                ));
            }
            let radians = normalized.map(f64::to_radians);
            DMat3::from_euler(EulerRot::ZYX, radians.z, radians.y, radians.x)
        }
    };
    let replacements = plan_affine(
        document,
        components,
        linear,
        pivot - linear * pivot,
        texture_lock,
    )?;
    let mut changed = false;
    for replacement in &replacements {
        let before = topology(solid(document, replacement.solid.id)?.1)?;
        let after = topology(&replacement.solid)?;
        changed |= before.vertices.iter().any(|vertex| {
            let other = after
                .vertices
                .iter()
                .find(|other| other.sides == vertex.sides)
                .unwrap();
            vertex.position.distance(other.position) > EPSILON
        });
    }
    if !changed {
        return Err(invalid(
            "The transform leaves every selected corner in place",
        ));
    }
    Ok(TransformPlan {
        replacements,
        pivot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, edit, subobject, texel_uv};

    fn fixture() -> (Document, u32, Vec<Component>) {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let top = shape
            .faces
            .iter()
            .find(|face| {
                face.vertices
                    .iter()
                    .all(|index| shape.vertices[*index].position.z == 64.0)
            })
            .unwrap();
        (
            document,
            id,
            vec![Component {
                solid: id,
                sides: vec![top.side],
            }],
        )
    }

    #[test]
    fn partial_face_transforms_keep_bottom_planes_ids_and_world_projections() {
        for spec in [
            ComponentTransform::Scale(DVec3::new(0.5, 0.5, 1.0)),
            ComponentTransform::Rotate(DVec3::new(30.0, 0.0, 0.0)),
        ] {
            let (mut document, id, selected) = fixture();
            let source = document.vmf().clone();
            let before = topology(document.solid(id).unwrap()).unwrap();
            let plan = plan_transform(&document, &selected, &spec, None, false).unwrap();
            assert_eq!(plan.pivot, DVec3::new(32.0, 32.0, 64.0));
            document
                .edit("reshape", |tx| subobject::apply(tx, &plan.replacements))
                .unwrap();
            let brush = document.solid(id).unwrap();
            let after = topology(brush).unwrap();
            for vertex in before
                .vertices
                .iter()
                .filter(|vertex| vertex.position.z == 0.0)
            {
                assert!(
                    after
                        .vertices
                        .iter()
                        .any(|other| other.sides == vertex.sides
                            && other.position.abs_diff_eq(vertex.position, EPSILON))
                );
            }
            for side in &source.world.solids[0].sides {
                let other = brush
                    .sides
                    .iter()
                    .find(|other| other.id == side.id)
                    .unwrap();
                assert_eq!(other.u_axis, side.u_axis);
                assert_eq!(other.v_axis, side.v_axis);
                assert_eq!(other.material, side.material);
                let indices = before.resolve(&[side.id]).unwrap();
                if indices
                    .iter()
                    .all(|index| before.vertices[*index].position.z == 0.0)
                {
                    assert_eq!(other, side);
                }
            }
            let saved = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default()))
                .unwrap();
            assert_eq!(topology(&saved.world.solids[0]).unwrap().vertices.len(), 8);
            saved.validate_ids().unwrap();
            document.undo().unwrap();
            assert_eq!(document.vmf(), &source);
        }
    }

    #[test]
    fn whole_brush_transforms_attach_every_corner_and_keep_entity_siblings() {
        for spec in [
            ComponentTransform::Scale(DVec3::new(2.0, 0.5, 1.25)),
            ComponentTransform::Rotate(DVec3::new(15.0, -20.0, 30.0)),
        ] {
            let (mut document, id, _) = fixture();
            let ObjectId::Solid(sibling) = document
                .edit("sibling", |tx| {
                    edit::create_block(tx, DVec3::splat(128.0), DVec3::splat(192.0), "brick/test")
                })
                .unwrap()
            else {
                unreachable!()
            };
            let owner = document
                .edit("owner", |tx| {
                    edit::tie_to_entity(tx, &[id, sibling], "func_detail", &[])
                })
                .unwrap();
            let before_vmf = document.vmf().clone();
            let brush = solid(&document, id).unwrap().1.clone();
            let before = topology(&brush).unwrap();
            let selected = before
                .vertices
                .iter()
                .map(|vertex| Component {
                    solid: id,
                    sides: vertex.sides.clone(),
                })
                .collect::<Vec<_>>();
            let pivot = DVec3::new(8.125, 16.25, 24.5);
            let plan = plan_transform(&document, &selected, &spec, Some(pivot), true).unwrap();
            document
                .edit("attached transform", |tx| {
                    subobject::apply(tx, &plan.replacements)
                })
                .unwrap();
            assert_eq!(document.owner_of_solid(id), Some(owner));
            let changed = solid(&document, id).unwrap().1;
            let after = topology(changed).unwrap();
            assert_eq!(
                document.vmf().entities[0].solids[1],
                before_vmf.entities[0].solids[1]
            );
            assert_eq!(
                document.vmf().entities[0].properties,
                before_vmf.entities[0].properties
            );
            for side in &brush.sides {
                let mapped = changed
                    .sides
                    .iter()
                    .find(|other| other.id == side.id)
                    .unwrap();
                for index in before.resolve(&[side.id]).unwrap() {
                    let vertex = &before.vertices[index];
                    let other = after
                        .vertices
                        .iter()
                        .find(|other| other.sides == vertex.sides)
                        .unwrap();
                    assert!(
                        texel_uv(side, vertex.position)
                            .abs_diff_eq(texel_uv(mapped, other.position), 0.0001)
                    );
                }
            }
            document.undo().unwrap();
            assert_eq!(document.vmf(), &before_vmf);
        }
    }

    #[test]
    fn invalid_partial_transforms_bounds_and_identity_leave_the_source_unchanged() {
        let (document, id, selected) = fixture();
        let source = document.vmf().clone();
        for spec in [
            ComponentTransform::Scale(DVec3::ONE),
            ComponentTransform::Scale(DVec3::ZERO),
            ComponentTransform::Scale(DVec3::new(-1.0, 1.0, 1.0)),
            ComponentTransform::Scale(DVec3::splat(f64::NAN)),
            ComponentTransform::Scale(DVec3::splat(1e8)),
            ComponentTransform::Scale(DVec3::splat(1e-8)),
            ComponentTransform::Rotate(DVec3::new(0.0, 0.0, 45.0)),
            ComponentTransform::Rotate(DVec3::new(0.0, 0.0, 360.0)),
            ComponentTransform::Rotate(DVec3::splat(f64::INFINITY)),
        ] {
            assert!(
                plan_transform(&document, &selected, &spec, None, false).is_err(),
                "{spec:?}"
            );
        }
        let spec = ComponentTransform::Scale(DVec3::new(0.5, 0.5, 1.0));
        assert!(plan_transform(&document, &selected, &spec, None, true).is_err());
        for pivot in [DVec3::splat(65_537.0), DVec3::splat(f64::NAN)] {
            assert!(plan_transform(&document, &selected, &spec, Some(pivot), false).is_err());
        }
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let vertex = &shape.vertices[0];
        let selected = [Component {
            solid: id,
            sides: vertex.sides.clone(),
        }];
        assert!(plan_transform(&document, &selected, &spec, Some(vertex.position), false).is_err());
        assert_eq!(document.vmf(), &source);
    }
}
