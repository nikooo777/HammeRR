use super::*;
use crate::context::{Note, Scope};
use source_formats::vmf::Connection;

fn options(count: u32) -> Options {
    Options {
        count,
        translation: DVec3::X * 64.0,
        rotation: DVec3::ZERO,
        scale: DVec3::ONE,
        pivot: DVec3::ZERO,
        basis: DMat3::IDENTITY,
        texture_lock: true,
    }
}

fn entity(document: &mut Document, name: &str, origin: DVec3) -> ObjectId {
    document
        .edit("entity", |tx| {
            edit::create_entity(
                tx,
                "info_target",
                Some(origin),
                &[("targetname".into(), name.into())],
                Vec::new(),
            )
        })
        .unwrap()
}

fn number(id: ObjectId) -> u32 {
    let ObjectId::Entity(id) = id else { panic!() };
    id
}

#[test]
fn mixed_array_remaps_only_copied_internal_links_and_is_one_undo_step() {
    let mut document = Document::empty();
    let brush = document
        .edit("block", |tx| {
            edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "brick/wall")
        })
        .unwrap();
    let a = entity(&mut document, "parent", DVec3::X * 24.0);
    let b = entity(&mut document, "child", DVec3::X * 48.0);
    document
        .edit("links", |tx| {
            let entity = tx.entity_mut(number(b))?;
            entity.properties.0.extend([
                ("parentname".into(), "parent,attachment".into()),
                ("custom".into(), "untouched".into()),
            ]);
            entity.connections = Some(vec![
                Connection {
                    output: "OnUser1".into(),
                    value: "parent,SetParent,parent,0.00,-1".into(),
                },
                Connection {
                    output: "OnUser2".into(),
                    value: "outside,Trigger,,0,-1".into(),
                },
            ]);
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let result = document
        .edit("repeat array", |tx| {
            create(tx, &[brush, a, b], options(2), &Catalog::default())
        })
        .unwrap();
    assert_eq!(document.undo_labels().count(), history + 1);
    assert_eq!(result.objects().len(), 6);
    document.vmf().validate_ids().unwrap();
    assert_eq!(
        document.entity(number(b)),
        before.entities.iter().find(|entity| entity.id == number(b))
    );
    for copy in &result.copies {
        let child = document.entity(number(copy.objects[2])).unwrap();
        assert_eq!(
            child.properties.get("targetname"),
            Some(format!("child_copy{}", copy.index).as_str())
        );
        assert_eq!(
            child.properties.get("parentname"),
            Some(format!("parent_copy{},attachment", copy.index).as_str())
        );
        assert_eq!(
            child.connections.as_ref().unwrap()[0].value,
            format!(
                "parent_copy{0},SetParent,parent_copy{0},0.00,-1",
                copy.index
            )
        );
        assert_eq!(
            child.connections.as_ref().unwrap()[1].value,
            "outside,Trigger,,0,-1"
        );
        assert_eq!(copy.remapped_references, 3);
        let ObjectId::Solid(new) = copy.objects[0] else {
            panic!()
        };
        let ObjectId::Solid(old) = brush else {
            panic!()
        };
        let source = document.solid(old).unwrap();
        let target = document.solid(new).unwrap();
        for (old, new) in source.sides.iter().zip(&target.sides) {
            assert_ne!(old.id, new.id);
            let delta = DVec3::X * 64.0 * f64::from(copy.index);
            assert!(
                crate::texel_uv(old, old.plane.0[0])
                    .abs_diff_eq(crate::texel_uv(new, old.plane.0[0] + delta), 1e-8)
            );
        }
    }
    let after = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after);
}

#[test]
fn nth_copy_uses_original_and_fixed_pivot_with_world_translation() {
    let mut document = Document::empty();
    let source = entity(&mut document, "", DVec3::X * 10.0);
    let result = document
        .edit("repeat", |tx| {
            create(
                tx,
                &[source],
                Options {
                    translation: DVec3::Y * 2.0,
                    rotation: DVec3::Z * 90.0,
                    ..options(2)
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    let expected = [DVec3::new(0.0, 12.0, 0.0), DVec3::new(-10.0, 4.0, 0.0)];
    for (copy, expected) in result.copies.iter().zip(expected) {
        let entity = document.entity(number(copy.objects[0])).unwrap();
        assert!(entity.origin().unwrap().abs_diff_eq(expected, 1e-8));
        assert_eq!(entity.properties.get("targetname"), Some(""));
    }
}

#[test]
fn fixed_local_frame_rotates_each_original_and_keeps_attached_texels() {
    let mut document = Document::empty();
    let source = document
        .edit("source", |tx| {
            edit::create_block(
                tx,
                DVec3::new(3.125, -7.25, 2.5),
                DVec3::new(19.5, 3.75, 13.625),
                "brick/wall",
            )
        })
        .unwrap();
    let ObjectId::Solid(id) = source else {
        panic!()
    };
    let original = document.solid(id).unwrap().clone();
    let basis = DMat3::from_rotation_z(0.61) * DMat3::from_rotation_y(-0.37);
    let pivot = DVec3::new(-5.25, 8.125, 1.5);
    let step = DVec3::new(4.5, -1.25, 2.0);
    let result = document
        .edit("local array", |tx| {
            create(
                tx,
                &[source],
                Options {
                    basis,
                    pivot,
                    translation: step,
                    rotation: DVec3::X * 90.0,
                    ..options(3)
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    assert_eq!(document.solid(id), Some(&original));
    for copy in result.copies {
        let ObjectId::Solid(id) = copy.objects[0] else {
            panic!()
        };
        let target = document.solid(id).unwrap();
        let factor = f64::from(copy.index);
        for (before, after) in original.sides.iter().zip(&target.sides) {
            for (point, actual) in before.plane.0.into_iter().zip(after.plane.0) {
                let local = basis.transpose() * (point - pivot);
                let rotated = match copy.index {
                    1 => DVec3::new(local.x, -local.z, local.y),
                    2 => DVec3::new(local.x, -local.y, -local.z),
                    3 => DVec3::new(local.x, local.z, -local.y),
                    _ => unreachable!(),
                };
                let expected = pivot + basis * (rotated + step * factor);
                assert!(actual.abs_diff_eq(expected, 1e-8));
                assert!(
                    crate::texel_uv(before, point)
                        .abs_diff_eq(crate::texel_uv(after, actual), 1e-8)
                );
            }
        }
    }
}

#[test]
fn nth_scales_precede_rotation_in_one_fixed_frame_and_keep_texture_policy() {
    let scales = [
        DVec3::new(2.0, 0.5, 1.25),
        DVec3::new(4.0, 0.25, 1.5625),
        DVec3::new(8.0, 0.125, 1.953125),
    ];
    for basis in [
        DMat3::IDENTITY,
        DMat3::from_rotation_z(0.61) * DMat3::from_rotation_y(-0.37),
    ] {
        for texture_lock in [false, true] {
            let mut document = Document::empty();
            let source = document
                .edit("source", |tx| {
                    edit::create_block(
                        tx,
                        DVec3::new(3.125, -7.25, 2.5),
                        DVec3::new(19.5, 3.75, 13.625),
                        "brick/wall",
                    )
                })
                .unwrap();
            let ObjectId::Solid(id) = source else {
                panic!()
            };
            let original = document.solid(id).unwrap().clone();
            let pivot = DVec3::new(-5.25, 8.125, 1.5);
            let step = DVec3::new(4.5, -1.25, 2.0);
            let result = document
                .edit("scaled array", |tx| {
                    create(
                        tx,
                        &[source],
                        Options {
                            basis,
                            pivot,
                            texture_lock,
                            scale: scales[0],
                            translation: step,
                            rotation: DVec3::X * 90.0,
                            ..options(3)
                        },
                        &Catalog::default(),
                    )
                })
                .unwrap();
            assert_eq!(document.solid(id), Some(&original));
            for (copy, scale) in result.copies.iter().zip(scales) {
                let ObjectId::Solid(id) = copy.objects[0] else {
                    panic!()
                };
                for (before, after) in original
                    .sides
                    .iter()
                    .zip(&document.solid(id).unwrap().sides)
                {
                    for (point, actual) in before.plane.0.into_iter().zip(after.plane.0) {
                        let local = (basis.transpose() * (point - pivot)) * scale;
                        let rotated = match copy.index {
                            1 => DVec3::new(local.x, -local.z, local.y),
                            2 => DVec3::new(local.x, -local.y, -local.z),
                            3 => DVec3::new(local.x, local.z, -local.y),
                            _ => unreachable!(),
                        };
                        let expected = pivot + basis * (rotated + step * f64::from(copy.index));
                        assert!(actual.abs_diff_eq(expected, 1e-8));
                        if texture_lock {
                            assert!(
                                crate::texel_uv(before, point)
                                    .abs_diff_eq(crate::texel_uv(after, actual), 1e-8)
                            );
                        } else {
                            assert_eq!(
                                (before.u_axis, before.v_axis),
                                (after.u_axis, after.v_axis)
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn identity_scale_is_exactly_the_existing_duplicate_transform_sequence() {
    for basis in [DMat3::IDENTITY, DMat3::from_rotation_z(0.61)] {
        let mut document = Document::empty();
        let source = document
            .edit("source", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-3.125, 2.25, 1.5),
                    DVec3::new(13.5, 9.75, 24.125),
                    "brick/wall",
                )
            })
            .unwrap();
        let mut control = document.clone();
        let request = Options {
            basis,
            translation: DVec3::new(4.125, -8.5, 2.25),
            rotation: DVec3::new(17.0, 29.0, -11.0),
            ..options(3)
        };
        document
            .edit("array", |tx| {
                create(tx, &[source], request, &Catalog::default())
            })
            .unwrap();
        control
            .edit("previous transform sequence", |tx| {
                for index in 1..=request.count {
                    let objects = edit::duplicate(tx, &[source], DVec3::ZERO)?;
                    let factor = f64::from(index);
                    edit::transform(
                        tx,
                        &objects,
                        edit::Transform {
                            translation: if basis == DMat3::IDENTITY {
                                request.translation * factor
                            } else {
                                basis * (request.translation * factor)
                            },
                            rotation: request.rotation * factor,
                            pivot: request.pivot,
                            basis,
                            texture_lock: request.texture_lock,
                            ..edit::Transform::default()
                        },
                    )?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(document.vmf(), control.vmf());
    }
}

#[test]
fn scaled_terrain_copies_transform_saved_rows_and_keep_blend_metadata() {
    let mut document = Document::empty();
    let source = document
        .edit("terrain source", |tx| {
            let source =
                edit::create_block(tx, DVec3::ZERO, DVec3::new(16.0, 16.0, 4.0), "brick/wall")?;
            let ObjectId::Solid(id) = source else {
                unreachable!()
            };
            let geometry = BrushGeometry::from_solid(tx.document().solid(id).unwrap()).unwrap();
            let top = geometry
                .faces
                .iter()
                .find(|face| face.plane.normal.z > 0.9)
                .unwrap();
            let side = tx.document().solid(id).unwrap().sides[top.side].id;
            crate::displacement::edit::create(
                tx,
                &[crate::surface::FaceId { solid: id, side }],
                2,
                0,
            )?;
            let info = tx
                .solid_mut(id)?
                .sides
                .iter_mut()
                .find_map(|side| side.displacement.as_mut())
                .unwrap();
            info.distances.as_mut().unwrap()[2][2] = 3.25;
            info.alphas = Some(vec![vec![93.0; 5]; 5]);
            info.triangle_tags = Some(vec![vec![9; 8]; 4]);
            Ok(source)
        })
        .unwrap();
    let ObjectId::Solid(id) = source else {
        unreachable!()
    };
    let original = document.solid(id).unwrap().clone();
    let geometry = BrushGeometry::from_solid(&original).unwrap();
    let side_index = original
        .sides
        .iter()
        .position(|side| side.displacement.is_some())
        .unwrap();
    let face = geometry
        .faces
        .iter()
        .find(|face| face.side == side_index)
        .unwrap();
    let info = original.sides[side_index].displacement.as_ref().unwrap();
    let mesh = crate::DisplacementMesh::build(face, info).unwrap();
    let result = document
        .edit("scaled terrain", |tx| {
            create(
                tx,
                &[source],
                Options {
                    translation: DVec3::ZERO,
                    scale: DVec3::new(2.0, 0.5, 1.25),
                    ..options(2)
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    for (copy, scale) in result
        .copies
        .iter()
        .zip([DVec3::new(2.0, 0.5, 1.25), DVec3::new(4.0, 0.25, 1.5625)])
    {
        let ObjectId::Solid(id) = copy.objects[0] else {
            unreachable!()
        };
        let solid = document.solid(id).unwrap();
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == side_index)
            .unwrap();
        let copied = solid.sides[side_index].displacement.as_ref().unwrap();
        let copied_mesh = crate::DisplacementMesh::build(face, copied).unwrap();
        for (before, after) in mesh.positions.iter().zip(&copied_mesh.positions) {
            assert!(after.abs_diff_eq(*before * scale, 1e-8));
        }
        assert_eq!(copied.alphas, info.alphas);
        assert_eq!(copied.triangle_tags, info.triangle_tags);
    }
    assert_eq!(document.solid(id), Some(&original));
}

#[test]
fn scale_only_brush_entity_arrays_keep_names_links_and_owned_geometry() {
    let mut document = Document::empty();
    let sources = document
        .edit("parts", |tx| {
            let mut parts = Vec::new();
            for (index, name) in ["parent", "child"].into_iter().enumerate() {
                let brush = edit::create_block(
                    tx,
                    DVec3::X * index as f64 * 32.0,
                    DVec3::new(16.0, 12.0, 8.0) + DVec3::X * index as f64 * 32.0,
                    "brick/wall",
                )?;
                let ObjectId::Solid(id) = brush else {
                    unreachable!()
                };
                parts.push(edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[
                        ("targetname".into(), name.into()),
                        ("origin".into(), "1 2 3".into()),
                    ],
                )?);
            }
            tx.entity_mut(number(parts[1]))?
                .properties
                .set("parentname", "parent,attachment");
            Ok(parts)
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let result = document
        .edit("scaled array", |tx| {
            create(
                tx,
                &sources,
                Options {
                    translation: DVec3::ZERO,
                    scale: DVec3::splat(2.0),
                    ..options(2)
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    for copy in &result.copies {
        let parent = document.entity(number(copy.objects[0])).unwrap();
        let child = document.entity(number(copy.objects[1])).unwrap();
        assert_eq!(
            parent.properties.get("targetname"),
            Some(format!("parent_copy{}", copy.index).as_str())
        );
        assert_eq!(
            child.properties.get("parentname"),
            Some(format!("parent_copy{},attachment", copy.index).as_str())
        );
        assert_eq!(copy.remapped_references, 1);
        let factor = if copy.index == 1 { 2.0 } else { 4.0 };
        assert!(
            parent
                .origin()
                .unwrap()
                .abs_diff_eq(DVec3::new(1.0, 2.0, 3.0) * factor, 1e-8)
        );
        let bounds = BrushGeometry::from_solid(&parent.solids[0]).unwrap();
        assert!(
            (bounds.maxs - bounds.mins).abs_diff_eq(DVec3::new(16.0, 12.0, 8.0) * factor, 1e-8)
        );
    }
    assert_eq!(document.undo_labels().count(), history + 1);
    let after = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after);
}

#[test]
fn invalid_scales_and_unrepresentable_entities_leave_history_and_ids_intact() {
    let mut document = Document::empty();
    let brush = document
        .edit("source", |tx| {
            edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "brick/wall")
        })
        .unwrap();
    let point = entity(&mut document, "point", DVec3::ZERO);
    let model = document
        .edit("model", |tx| {
            edit::create_entity(
                tx,
                "prop_static",
                Some(DVec3::ZERO),
                &[("model".into(), "models/props/part.mdl".into())],
                Vec::new(),
            )
        })
        .unwrap();
    let mut control = document.clone();
    for (sources, scale, message) in [
        (vec![brush], DVec3::ZERO, "finite and positive"),
        (vec![brush], DVec3::splat(-1.0), "finite and positive"),
        (vec![brush], DVec3::NAN, "finite and positive"),
        (
            vec![brush],
            DVec3::splat(f64::INFINITY),
            "finite and positive",
        ),
        (vec![brush], DVec3::splat(1e300), "overflows or collapses"),
        (vec![brush], DVec3::splat(1e-300), "overflows or collapses"),
        (vec![brush], DVec3::splat(1000.0), "±65536"),
        (
            vec![brush, point],
            DVec3::splat(1.1),
            "no scalable brush geometry",
        ),
        (
            vec![brush, model],
            DVec3::splat(1.1),
            "no scalable brush geometry",
        ),
    ] {
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        let error = document
            .edit("invalid scale array", |tx| {
                create(
                    tx,
                    &sources,
                    Options {
                        scale,
                        ..options(2)
                    },
                    &Catalog::default(),
                )
            })
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
    let request = Options {
        scale: DVec3::splat(1.1),
        ..options(2)
    };
    let expected = control
        .edit("valid", |tx| {
            create(tx, &[brush], request, &Catalog::default())
        })
        .unwrap();
    let actual = document
        .edit("valid", |tx| {
            create(tx, &[brush], request, &Catalog::default())
        })
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(document.vmf(), control.vmf());
}

#[test]
fn invalid_repeat_frames_reject_the_complete_transaction() {
    for basis in [
        DMat3::ZERO,
        DMat3::from_diagonal(DVec3::new(-1.0, 1.0, 1.0)),
        DMat3::from_diagonal(DVec3::new(1.0, 1.01, 1.0)),
        DMat3::from_cols(DVec3::NAN, DVec3::Y, DVec3::Z),
    ] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        assert!(
            document
                .edit("invalid local array", |tx| create(
                    tx,
                    &[source],
                    Options {
                        basis,
                        ..options(2)
                    },
                    &Catalog::default(),
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
}

#[test]
fn local_arrays_keep_generated_names_and_internal_parent_links() {
    let mut document = Document::empty();
    let parent = entity(&mut document, "parent", DVec3::X * 10.0);
    let child = entity(&mut document, "child", DVec3::Y * 20.0);
    document
        .edit("parent link", |tx| {
            tx.entity_mut(number(child))?
                .properties
                .set("parentname", "parent,attachment");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let basis = DMat3::from_rotation_z(0.61);
    let result = document
        .edit("local array", |tx| {
            create(
                tx,
                &[child, parent],
                Options {
                    basis,
                    ..options(2)
                },
                &Catalog::default(),
            )
        })
        .unwrap();
    for copy in result.copies {
        let child = document.entity(number(copy.objects[0])).unwrap();
        let parent = document.entity(number(copy.objects[1])).unwrap();
        assert_eq!(
            child.properties.get("parentname"),
            Some(format!("parent_copy{},attachment", copy.index).as_str())
        );
        assert_eq!(
            parent.properties.get("targetname"),
            Some(format!("parent_copy{}", copy.index).as_str())
        );
        assert_eq!(copy.remapped_references, 1);
        assert!(parent.origin().unwrap().abs_diff_eq(
            DVec3::X * 10.0 + basis.x_axis * (64.0 * f64::from(copy.index)),
            1e-8
        ));
    }
    assert_eq!(
        document.entity(number(parent)),
        before
            .entities
            .iter()
            .find(|entity| entity.id == number(parent))
    );
    assert_eq!(
        document.entity(number(child)),
        before
            .entities
            .iter()
            .find(|entity| entity.id == number(child))
    );
    document.vmf().validate_ids().unwrap();
}

#[test]
fn names_avoid_collisions_and_reject_new_reference_acquisition() {
    let mut document = Document::empty();
    let source = entity(&mut document, "part", DVec3::ZERO);
    entity(&mut document, "part_copy1", DVec3::Y * 64.0);
    let result = document
        .edit("repeat", |tx| {
            create(tx, &[source], options(1), &Catalog::default())
        })
        .unwrap();
    assert_eq!(result.copies[0].names[0].after, "part_copy1_2");
    document.undo().unwrap();
    document
        .edit("external reference", |tx| {
            tx.entity_mut(number(source))?.connections = Some(vec![Connection {
                output: "OnUser1".into(),
                value: "part_copy1_2,Trigger,,0,-1".into(),
            }]);
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    assert!(
        document
            .edit("repeat", |tx| create(
                tx,
                &[source],
                options(1),
                &Catalog::default()
            ))
            .unwrap_err()
            .to_string()
            .contains("acquire")
    );
    assert_eq!(document.vmf(), &before);
}

#[test]
fn shared_names_wildcards_and_untyped_internal_text_reject_atomically() {
    for value in ["part", "par*"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        document
            .edit("unsafe", |tx| {
                tx.entity_mut(number(source))?
                    .properties
                    .set("custom", value);
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("repeat", |tx| create(
                    tx,
                    &[source],
                    options(2),
                    &Catalog::default()
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
    }
    let mut document = Document::empty();
    let source = entity(&mut document, "shared", DVec3::ZERO);
    entity(&mut document, "SHARED", DVec3::X * 16.0);
    assert!(
        document
            .edit("repeat", |tx| create(
                tx,
                &[source],
                options(1),
                &Catalog::default()
            ))
            .unwrap_err()
            .to_string()
            .contains("shared")
    );
}

#[test]
fn limits_and_late_geometry_failures_restore_history_and_allocators() {
    let mut document = Document::empty();
    let source = entity(&mut document, "", DVec3::ZERO);
    let baseline = document.clone();
    for options in [
        options(0),
        options(MAX_COPIES + 1),
        Options {
            translation: DVec3::ZERO,
            ..options(1)
        },
        Options {
            translation: DVec3::X * 40_000.0,
            ..options(2)
        },
        Options {
            rotation: DVec3::splat(f64::NAN),
            ..options(1)
        },
    ] {
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        assert!(
            document
                .edit("rejected", |tx| create(
                    tx,
                    &[source],
                    options,
                    &Catalog::default()
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
    let mut control = baseline;
    let expected = control
        .edit("repeat", |tx| {
            create(tx, &[source], options(2), &Catalog::default())
        })
        .unwrap();
    let actual = document
        .edit("repeat", |tx| {
            create(tx, &[source], options(2), &Catalog::default())
        })
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(document.vmf(), control.vmf());
}

#[test]
fn context_copy_budget_is_enforced_and_rolls_back_the_entire_array() {
    let mut document = Document::empty();
    let source = entity(&mut document, "", DVec3::ZERO);
    document
        .edit("notes", |tx| {
            let mut metadata = tx.document().context()?.clone();
            let note = Note {
                summary: "n".repeat(200),
                body: "x".repeat(3800),
                author: "agent".into(),
                observed: String::new(),
                region: None,
                extra: Vec::new(),
            };
            metadata.notes.insert(Scope::Object(source), note.clone());
            for id in 1000..1259 {
                metadata
                    .notes
                    .insert(Scope::Object(ObjectId::Entity(id)), note.clone());
            }
            tx.set_context(metadata)
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let error = document
        .edit("repeat", |tx| {
            create(tx, &[source], options(4), &Catalog::default())
        })
        .unwrap_err();
    assert!(error.to_string().contains("budget"), "{error}");
    assert_eq!(document.vmf(), &before);
    assert_eq!(document.undo_labels().count(), history);
    let result = document
        .edit("one copy", |tx| {
            create(tx, &[source], options(1), &Catalog::default())
        })
        .unwrap();
    assert!(
        document
            .context()
            .unwrap()
            .notes
            .contains_key(&Scope::Object(result.objects()[0]))
    );
    assert_eq!(
        document.context().unwrap().notes[&Scope::Object(source)]
            .body
            .len(),
        3800
    );
}

#[test]
fn inherited_fgd_references_and_duplicate_esc_connections_follow_each_copy() {
    let catalog = Catalog::parse(
        r#"
@BaseClass = Linked [
 target(target_destination) : "Target"
 input CustomLink(target_destination) : "Link"
]
@PointClass base(Linked) = info_target []
"#,
    )
    .unwrap();
    let mut document = Document::empty();
    let parent = entity(&mut document, "parent", DVec3::ZERO);
    let child = entity(&mut document, "child", DVec3::Y * 16.0);
    document
        .edit("links", |tx| {
            let child = tx.entity_mut(number(child))?;
            child.properties.0.extend([
                ("target".into(), "parent".into()),
                ("target".into(), "PARENT".into()),
            ]);
            let connection = Connection {
                output: "OnUser1".into(),
                value: "parent\u{1b}CustomLink\u{1b}PARENT\u{1b}0.00\u{1b}01".into(),
            };
            child.connections = Some(vec![connection.clone(), connection]);
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let result = document
        .edit("repeat", |tx| {
            create(tx, &[parent, child], options(2), &catalog)
        })
        .unwrap();
    for copy in result.copies {
        let child = document.entity(number(copy.objects[1])).unwrap();
        let name = format!("parent_copy{}", copy.index);
        assert_eq!(
            child.properties.get_all("target").collect::<Vec<_>>(),
            [name.as_str(), name.as_str()]
        );
        let connections = child.connections.as_ref().unwrap();
        assert_eq!(
            connections[0].value,
            format!("{name}\u{1b}CustomLink\u{1b}{name}\u{1b}0.00\u{1b}01")
        );
        assert_eq!(connections[0], connections[1]);
        assert_eq!(copy.remapped_references, 6);
    }
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
}

#[test]
fn owned_brush_notes_copy_with_observations_and_reserved_orphan_ids() {
    let mut document = Document::empty();
    let brush = document
        .edit("block", |tx| {
            edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "brick/wall")
        })
        .unwrap();
    let ObjectId::Solid(brush_id) = brush else {
        panic!()
    };
    let source = document
        .edit("tie", |tx| {
            edit::tie_to_entity(tx, &[brush_id], "func_detail", &[])
        })
        .unwrap();
    let note = Note {
        summary: "Assembly part".into(),
        body: "Keep this brush".into(),
        author: "agent".into(),
        observed: "0123456789abcdef".into(),
        region: None,
        extra: Vec::new(),
    };
    let orphan = ObjectId::Entity(number(source) + 1);
    document
        .edit("notes", |tx| {
            let mut metadata = tx.document().context()?.clone();
            for id in [source, brush, orphan] {
                metadata.notes.insert(Scope::Object(id), note.clone());
            }
            tx.set_context(metadata)
        })
        .unwrap();
    let before = document.vmf().clone();
    let result = document
        .edit("repeat", |tx| {
            create(tx, &[source], options(2), &Catalog::default())
        })
        .unwrap();
    for copy in result.copies {
        let copied = copy.objects[0];
        assert_ne!(copied, orphan);
        let owned = ObjectId::Solid(document.entity(number(copied)).unwrap().solids[0].id);
        assert_ne!(owned, brush);
        for id in [copied, owned, orphan] {
            assert_eq!(document.context().unwrap().notes[&Scope::Object(id)], note);
        }
    }
    document.vmf().validate_ids().unwrap();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
}

#[test]
fn named_script_and_dynamic_io_sources_reject_without_rejecting_generic_text() {
    for key in ["vscripts", "thinkfunction"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        document
            .edit("script", |tx| {
                tx.entity_mut(number(source))?
                    .properties
                    .set(key, "script_entry");
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("repeat", |tx| create(
                    tx,
                    &[source],
                    options(1),
                    &Catalog::default()
                ))
                .unwrap_err()
                .to_string()
                .contains("script")
        );
        assert_eq!(document.vmf(), &before);
    }
    for input in ["AddOutput", "RunScriptCode", "RunScriptFile"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "part", DVec3::ZERO);
        document
            .edit("dynamic I/O", |tx| {
                tx.entity_mut(number(source))?.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: format!("!self\u{1b}{input}\u{1b}opaque\u{1b}0\u{1b}-1"),
                }]);
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("repeat", |tx| create(
                    tx,
                    &[source],
                    options(1),
                    &Catalog::default()
                ))
                .unwrap_err()
                .to_string()
                .contains("dynamic I/O")
        );
        assert_eq!(document.vmf(), &before);
    }
    let mut document = Document::empty();
    let source = entity(&mut document, "part", DVec3::ZERO);
    document
        .edit("description", |tx| {
            tx.entity_mut(number(source))?
                .properties
                .set("description", "The part remains visible near part_copy1");
            Ok(())
        })
        .unwrap();
    let result = document
        .edit("repeat", |tx| {
            create(tx, &[source], options(1), &Catalog::default())
        })
        .unwrap();
    assert_eq!(
        document
            .entity(number(result.objects()[0]))
            .unwrap()
            .properties
            .get("description"),
        Some("The part remains visible near part_copy1")
    );
}

#[test]
fn duplicate_classnames_and_numeric_node_or_side_references_reject_atomically() {
    let catalog = Catalog::parse(
        r#"
@BaseClass = NumericLinks [ link(node_dest) : "Node" faces(sidelist) : "Faces" ]
@PointClass base(NumericLinks) = info_target []
"#,
    )
    .unwrap();
    for key in ["classname", "nodeid", "link", "faces"] {
        let mut document = Document::empty();
        let source = entity(&mut document, "", DVec3::ZERO);
        document
            .edit("unsafe IDs", |tx| {
                tx.entity_mut(number(source))?
                    .properties
                    .0
                    .push((key.into(), "17".into()));
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        let history = document.undo_labels().count();
        assert!(
            document
                .edit("repeat", |tx| create(tx, &[source], options(1), &catalog))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.undo_labels().count(), history);
    }
}
