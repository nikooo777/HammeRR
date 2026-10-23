use super::*;
use crate::{Document, Plane, displacement};
use glam::DQuat;

fn options() -> Options {
    Options {
        material: "Materials/Decals/marking.VMT".into(),
        targetname: Some("  fitted marking  ".into()),
        render_order: 3,
        u: [1.0, 0.0],
        v: [-0.5, 1.5],
        fade_min: 16.0,
        fade_max: 64.0,
    }
}

fn fixture(detail: bool) -> (Document, FaceId) {
    let mut document = Document::empty();
    let face = document
        .edit("tilted trapezoid", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 0.0),
                DVec3::new(64.0, 64.0, 16.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let turn = DQuat::from_rotation_x(0.3) * DQuat::from_rotation_y(0.2);
            let solid = tx.solid_mut(id)?;
            for side in &mut solid.sides {
                for point in &mut side.plane.0 {
                    point.x *= 1.0 + point.y / 256.0;
                    *point = turn * *point + DVec3::new(100.125, 200.25, 40.5);
                }
            }
            let side = solid
                .sides
                .iter()
                .find(|side| {
                    Plane::from_points(side.plane.0)
                        .unwrap()
                        .normal
                        .dot(turn * DVec3::Z)
                        > 0.99
                })
                .unwrap()
                .id;
            if detail {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(FaceId { solid: id, side })
        })
        .unwrap();
    (document, face)
}

fn coplanar_fixture(gap: f64, detail: bool) -> (Document, [FaceId; 2]) {
    let mut document = Document::empty();
    let faces = document
        .edit("coplanar skew plates", |tx| {
            let mut faces = Vec::new();
            for x in [-96.0, -16.0 + gap] {
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(x, -32.0, 0.0),
                    DVec3::new(x + 80.0, 32.0, 16.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let turn = DQuat::from_rotation_x(0.3) * DQuat::from_rotation_y(0.2);
                let brush = tx.solid_mut(solid)?;
                let side = brush
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id;
                for side in &mut brush.sides {
                    for point in &mut side.plane.0 {
                        point.x += point.y * 0.25;
                        *point = turn * *point + DVec3::new(100.125, 200.25, 40.5);
                    }
                }
                if detail {
                    edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                }
                faces.push(FaceId { solid, side });
            }
            Ok(faces.try_into().unwrap())
        })
        .unwrap();
    (document, faces)
}

#[test]
fn coplanar_multiface_fits_keep_anchor_frame_and_exact_adjacent_or_disjoint_bindings() {
    for gap in [0.0, 48.0] {
        for detail in [false, true] {
            let (mut document, faces) = coplanar_fixture(gap, detail);
            let before = document.vmf().clone();
            let anchor = fit(&before, faces[0], options()).unwrap();
            let definition = fit_faces(&before, &faces, options()).unwrap();
            assert_eq!(definition.basis_origin, anchor.basis_origin);
            assert_eq!(definition.basis_normal, anchor.basis_normal);
            assert_eq!(definition.basis_u, anchor.basis_u);
            assert_eq!(definition.basis_v, anchor.basis_v);
            assert_eq!(definition.sides, faces.map(|face| face.side));
            assert_eq!(definition.options, anchor.options);
            let origin = DVec3::from_array(definition.basis_origin);
            let normal = DVec3::from_array(definition.basis_normal);
            let u = DVec3::from_array(definition.basis_u);
            let v = DVec3::from_array(definition.basis_v);
            let expected_normal =
                DQuat::from_rotation_x(0.3) * DQuat::from_rotation_y(0.2) * DVec3::Z;
            assert!(normal.abs_diff_eq(expected_normal, 1e-10));
            assert!(normal.cross(u).abs_diff_eq(v, 1e-10));
            for face in faces {
                for point in crate::surface::face_vertices(&document, face).unwrap() {
                    let local = point - origin;
                    assert!(normal.dot(local).abs() < 1e-8);
                    assert!(local.dot(u) >= definition.uv_points[0][0] - 1e-8);
                    assert!(local.dot(u) <= definition.uv_points[2][0] + 1e-8);
                    assert!(local.dot(v) >= definition.uv_points[0][1] - 1e-8);
                    assert!(local.dot(v) <= definition.uv_points[2][1] + 1e-8);
                }
            }
            let ObjectId::Entity(id) = document
                .edit("one multiface overlay", |tx| {
                    create_faces(tx, &faces, options())
                })
                .unwrap()
            else {
                unreachable!()
            };
            assert_eq!(parse(document.entity(id).unwrap()).unwrap(), definition);
            assert_eq!(
                document
                    .vmf()
                    .entities
                    .iter()
                    .filter(|entity| entity.classname() == Some("info_overlay"))
                    .count(),
                1
            );
            let after = document.vmf().clone();
            let reopened = Vmf::parse(&after.write(&Default::default())).unwrap();
            assert_eq!(
                parse(
                    reopened
                        .entities
                        .iter()
                        .find(|entity| entity.id == id)
                        .unwrap()
                )
                .unwrap(),
                definition
            );
            document.undo().unwrap();
            assert_eq!(document.vmf(), &before);
            document.redo().unwrap();
            assert_eq!(document.vmf(), &after);
        }
    }
}

#[test]
fn coplanar_multiface_single_source_remains_the_exact_original_fit_and_entity() {
    let (document, face) = fixture(false);
    assert_eq!(
        fit_faces(document.vmf(), &[face], options()).unwrap(),
        fit(document.vmf(), face, options()).unwrap()
    );
    let mut original = Document::new(document.vmf().clone());
    let mut multiple = Document::new(document.vmf().clone());
    original
        .edit("original", |tx| create(tx, face, options()))
        .unwrap();
    multiple
        .edit("one source", |tx| create_faces(tx, &[face], options()))
        .unwrap();
    assert_eq!(original.vmf(), multiple.vmf());
}

#[test]
fn coplanar_multiface_invalid_sources_reject_before_allocating_or_mutating() {
    let (document, faces) = coplanar_fixture(48.0, false);
    let mut cases = vec![
        (document.vmf().clone(), vec![], "at least one"),
        (
            document.vmf().clone(),
            vec![faces[0]; bsp::MAX_OVERLAY_FACES + 1],
            "budget",
        ),
        (document.vmf().clone(), vec![faces[0], faces[0]], "distinct"),
        (
            document.vmf().clone(),
            vec![
                faces[0],
                FaceId {
                    solid: faces[1].solid,
                    side: faces[0].side,
                },
            ],
            "does not belong",
        ),
    ];
    let mut hidden = document.vmf().clone();
    hidden.world.solids[1].hidden = true;
    cases.push((hidden, faces.to_vec(), "visible"));
    for translation in [DVec3::Z, DVec3::ZERO] {
        let mut moved = Document::new(document.vmf().clone());
        moved
            .edit("incompatible plane", |tx| {
                edit::transform(
                    tx,
                    &[ObjectId::Solid(faces[1].solid)],
                    edit::Transform {
                        translation,
                        rotation: if translation == DVec3::ZERO {
                            DVec3::new(0.0, 2.0, 0.0)
                        } else {
                            DVec3::ZERO
                        },
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        cases.push((moved.into_vmf(), faces.to_vec(), "coplanar"));
    }
    let mut model = Document::new(document.vmf().clone());
    model
        .edit("model", |tx| {
            edit::tie_to_entity(tx, &[faces[1].solid], "func_brush", &[])
        })
        .unwrap();
    cases.push((model.into_vmf(), faces.to_vec(), "world and func_detail"));
    let mut terrain = Document::new(document.vmf().clone());
    terrain
        .edit("terrain", |tx| {
            displacement::edit::create(tx, &[faces[1]], 2, 0)
        })
        .unwrap();
    cases.push((terrain.into_vmf(), faces.to_vec(), "terrain"));
    for (vmf, sources, message) in cases {
        let mut document = Document::new(vmf);
        let before = document.vmf().clone();
        let revision = document.revision();
        let error = document
            .edit("invalid multiface overlay", |tx| {
                create_faces(tx, &sources, options())
            })
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
        assert_eq!(document.undo_labels().count(), 0);
    }
}

#[test]
fn fitted_world_and_detail_trapezoids_roundtrip_with_one_undoable_creation() {
    for detail in [false, true] {
        let (mut document, face) = fixture(detail);
        let before = document.vmf().clone();
        let definition = fit(document.vmf(), face, options()).unwrap();
        assert_eq!(document.vmf(), &before);
        assert_eq!(definition.sides, vec![face.side]);
        assert_eq!(definition.options.material, "decals/marking");
        assert_eq!(definition.options.targetname, options().targetname);
        assert_eq!(definition.fade_squared().unwrap(), [256.0, 4096.0]);
        let points = crate::surface::face_vertices(&document, face).unwrap();
        for (corner, point) in definition.corners().iter().zip(&points) {
            assert!(DVec3::from_array(*corner).abs_diff_eq(*point, 1e-8));
        }
        let ObjectId::Entity(id) = document
            .edit("Fit overlay to face", |tx| create(tx, face, options()))
            .unwrap()
        else {
            unreachable!()
        };
        let after = document.vmf().clone();
        let entity = document.entity(id).unwrap();
        assert_eq!(entity.origin().unwrap().to_array(), definition.basis_origin);
        assert_eq!(parse(entity).unwrap(), definition);
        let reopened = Vmf::parse(&after.write(&Default::default())).unwrap();
        let entity = reopened
            .entities
            .iter()
            .find(|entity| entity.id == id)
            .unwrap();
        assert_eq!(parse(entity).unwrap(), definition);
        assert_eq!(document.undo().unwrap().label, "Fit overlay to face");
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &after);
    }
}

#[test]
fn fitting_rejects_hidden_model_terrain_and_nonquad_targets_without_editing() {
    let (document, face) = fixture(false);
    let mut hidden = document.vmf().clone();
    hidden.world.solids[0].hidden = true;
    let mut model = Document::new(document.vmf().clone());
    model
        .edit("runtime model", |tx| {
            edit::tie_to_entity(tx, &[face.solid], "func_brush", &[])
        })
        .unwrap();
    let mut hidden_owner = model.vmf().clone();
    hidden_owner.entities[0].hidden = true;
    let mut terrain = Document::new(document.vmf().clone());
    terrain
        .edit("terrain", |tx| {
            displacement::edit::create(tx, &[face], 2, 0)
        })
        .unwrap();
    for (source, message) in [
        (hidden, "visible source face"),
        (model.into_vmf(), "world and func_detail"),
        (hidden_owner, "visible source face"),
        (terrain.into_vmf(), "terrain brushes"),
    ] {
        let mut document = Document::new(source);
        let before = document.vmf().clone();
        let revision = document.revision();
        assert!(fit(&before, face, options()).unwrap_err().contains(message));
        let result = document.edit("invalid overlay", |tx| create(tx, face, options()));
        assert!(result.unwrap_err().to_string().contains(message));
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
    }
    let mut triangle = Document::empty();
    let face = triangle
        .edit("triangular prism", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 0.0),
                DVec3::new(64.0, 64.0, 16.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let solid = tx.solid_mut(id)?;
            let face = FaceId {
                solid: id,
                side: solid
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id,
            };
            solid
                .sides
                .iter_mut()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal.x > 0.99)
                .unwrap()
                .plane
                .0 = [
                DVec3::new(-64.0, 64.0, 0.0),
                DVec3::new(64.0, -64.0, 0.0),
                DVec3::new(64.0, -64.0, 16.0),
            ];
            Ok(face)
        })
        .unwrap();
    let before = triangle.vmf().clone();
    assert!(
        fit(&before, face, options())
            .unwrap_err()
            .contains("quad face")
    );
    assert!(
        triangle
            .edit("invalid quad", |tx| create(tx, face, options()))
            .is_err()
    );
    assert_eq!(triangle.vmf(), &before);
}

#[test]
fn checked_imports_and_fitting_share_projection_and_option_validation() {
    let (mut document, face) = fixture(false);
    let definition = fit(document.vmf(), face, options()).unwrap();
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create(tx, face, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let entity = document.entity(id).unwrap();
    for (key, value, message) in [
        ("BasisNormal", "0 0 2", "perpendicular unit axes"),
        ("uv0", "0 0 1e40", "three finite native numbers"),
        ("sides", "oops", "integer VMF side IDs"),
        ("RenderOrder", "4", "integer from 0 to 3"),
        ("fademindist", "1e30", "squared distance"),
    ] {
        let mut imported = entity.clone();
        imported.properties.set(key, value);
        assert!(parse(&imported).unwrap_err().contains(message));
    }
    for options in [
        Options {
            material: "../invalid".into(),
            ..options()
        },
        Options {
            u: [0.0, 0.0],
            ..options()
        },
        Options {
            fade_max: f32::INFINITY,
            ..options()
        },
        Options {
            fade_min: 1e30,
            ..options()
        },
        Options {
            targetname: Some("invalid\0name".into()),
            ..options()
        },
    ] {
        let mut invalid = definition.clone();
        invalid.options = options.clone();
        let expected = invalid.validate().unwrap_err();
        assert_eq!(fit(document.vmf(), face, options).unwrap_err(), expected);
    }
    let mut ignored_z = entity.clone();
    let point = definition.uv_points[0];
    ignored_z
        .properties
        .set("uv0", format!("{} {} 123", point[0], point[1]));
    assert_eq!(parse(&ignored_z).unwrap().corners(), definition.corners());
    let mut collapsed = definition.clone();
    collapsed.uv_points = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1e-50, 1.0, 0.0],
        [1e-50, 0.0, 0.0],
    ];
    assert!(
        collapsed
            .validate()
            .unwrap_err()
            .contains("convex quadrilateral")
    );
    let mut references = entity.clone();
    references.properties.set("sides", "7 6 7");
    assert_eq!(parse(&references).unwrap().sides, vec![6, 7]);
    references.properties.set(
        "sides",
        (0..=bsp::MAX_OVERLAY_FACES)
            .map(|side| side.to_string())
            .collect::<Vec<_>>()
            .join(" "),
    );
    assert!(
        parse(&references)
            .unwrap_err()
            .contains("side allocation budget")
    );
}

#[test]
fn multiface_refit_uses_explicit_current_quad_frame_and_preserves_exact_bindings_and_metadata() {
    let (mut document, faces) = coplanar_fixture(48.0, true);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create_faces(tx, &faces, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let old = parse(document.entity(id).unwrap()).unwrap();
    let owners = faces.map(|face| document.owner_of_solid(face.solid).unwrap());
    document
        .edit("move bound geometry and retain imported fields", |tx| {
            let entity = tx.entity_mut(id)?;
            entity
                .properties
                .set("sides", format!(" {}  {} ", faces[1].side, faces[0].side));
            entity.properties.set("StartU", "01.00000");
            entity.properties.set("RenderOrder", "03");
            entity.properties.set("custom_option", "opaque raw value");
            entity.connections = Some(vec![source_formats::vmf::Connection {
                output: "OnUser1".into(),
                value: "relay,Trigger,,0,-1".into(),
            }]);
            entity.editor = Some(source_formats::vmf::Properties(vec![(
                "color".into(),
                "1 2 3".into(),
            )]));
            entity.extra.push(source_formats::keyvalues::Node::block(
                "custom",
                vec![source_formats::keyvalues::Node::string("keep", "verbatim")],
            ));
            edit::transform(
                tx,
                &owners,
                edit::Transform {
                    translation: DVec3::new(4.125, -2.375, 8.625),
                    rotation: DVec3::new(7.25, -3.125, 2.625),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let before = document.vmf().clone();
    assert!(
        fit_existing_refit(&before, id, None)
            .unwrap_err()
            .contains("Choose a reference face")
    );
    let (sources, expected) = fit_existing_refit(&before, id, Some(faces[1])).unwrap();
    assert_eq!(sources, [faces[1], faces[0]]);
    let anchor = fit(&before, faces[1], options()).unwrap();
    assert_eq!(expected.basis_origin, anchor.basis_origin);
    assert_eq!(expected.basis_u, anchor.basis_u);
    assert_eq!(expected.basis_v, anchor.basis_v);
    assert_eq!(expected.basis_normal, anchor.basis_normal);
    assert_ne!(expected.basis_u, old.basis_u);
    assert_eq!(expected, fit_faces(&before, &sources, options()).unwrap());
    assert!(fit_existing(&before, id).is_err());
    assert!(fit_rebind(&before, id, faces[0]).is_err());
    assert!(editable_corners(&before, id).is_err());
    assert_eq!(
        document
            .edit("Refit overlay", |tx| refit_with_reference(
                tx,
                id,
                Some(faces[1])
            ))
            .unwrap(),
        ObjectId::Entity(id)
    );
    assert_eq!(parse(document.entity(id).unwrap()).unwrap(), expected);
    let original = before
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap();
    let mut retained = document.entity(id).unwrap().clone();
    for key in [
        "origin",
        "BasisOrigin",
        "BasisNormal",
        "BasisU",
        "BasisV",
        "uv0",
        "uv1",
        "uv2",
        "uv3",
    ] {
        retained
            .properties
            .set(key, original.properties.get(key).unwrap());
    }
    assert_eq!(&retained, original);
    for solid in before.solids() {
        assert_eq!(
            document
                .vmf()
                .solids()
                .find(|candidate| candidate.id == solid.id)
                .unwrap(),
            solid
        );
    }
    let after = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after);
}

#[test]
fn multiface_refit_rejects_unresolved_or_ambiguous_native_ids_and_invalid_references_atomically() {
    let (mut document, faces) = coplanar_fixture(48.0, false);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create_faces(tx, &faces, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let base = document.vmf().clone();
    let mut missing = base.clone();
    missing
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("sides", format!("{} {}", faces[0].side, u32::MAX));
    let mut ambiguous = base.clone();
    ambiguous.world.solids[1].sides[0].id = faces[0].side;
    for (vmf, reference) in [
        (base.clone(), None),
        (
            base.clone(),
            Some(FaceId {
                solid: faces[1].solid,
                side: faces[0].side,
            }),
        ),
        (missing, Some(faces[0])),
        (ambiguous, Some(faces[0])),
    ] {
        let mut rejected = Document::new(vmf);
        let before = rejected.vmf().clone();
        let history = rejected.undo_labels().count();
        assert!(
            rejected
                .edit("Rejected refit", |tx| refit_with_reference(
                    tx, id, reference
                ))
                .is_err()
        );
        assert_eq!(rejected.vmf(), &before);
        assert_eq!(rejected.undo_labels().count(), history);
    }
    let (mut original, face) = fixture(false);
    let ObjectId::Entity(id) = original
        .edit("single", |tx| create(tx, face, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let mut delegated = Document::new(original.vmf().clone());
    assert_eq!(
        fit_existing_refit(original.vmf(), id, None).unwrap(),
        (vec![face], fit_existing(original.vmf(), id).unwrap().1)
    );
    original.edit("refit", |tx| refit(tx, id)).unwrap();
    delegated
        .edit("refit", |tx| refit_with_reference(tx, id, None))
        .unwrap();
    assert_eq!(original.vmf(), delegated.vmf());
}

#[test]
fn refit_updates_only_existing_projection_and_preserves_raw_imported_metadata() {
    use source_formats::{
        keyvalues::Node,
        vmf::{Connection, Properties},
    };

    for detail in [false, true] {
        let (mut document, face) = fixture(detail);
        let ObjectId::Entity(id) = document
            .edit("overlay", |tx| create(tx, face, options()))
            .unwrap()
        else {
            unreachable!()
        };
        let source_owner = document.owner_of_solid(face.solid).unwrap();
        document
            .edit("imported metadata and moved source", |tx| {
                let entity = tx.entity_mut(id)?;
                for (key, raw) in [
                    ("material", "Materials/Decals/marking.VMT"),
                    ("targetname", "  imported marking  "),
                    ("RenderOrder", "03"),
                    ("StartU", "01.000"),
                    ("EndU", "00.000"),
                    ("StartV", "-0.5000"),
                    ("EndV", "1.5000"),
                    ("fademindist", "016.000"),
                    ("fademaxdist", "064.000"),
                    ("custom_option", "opaque raw value"),
                ] {
                    entity.properties.set(key, raw);
                }
                entity.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: "relay,Trigger,,0,-1".into(),
                }]);
                entity.editor = Some(Properties(vec![("color".into(), "1 2 3".into())]));
                entity.extra.push(Node::block(
                    "custom",
                    vec![Node::string("keep", "verbatim")],
                ));
                edit::transform(
                    tx,
                    &[source_owner],
                    edit::Transform {
                        translation: DVec3::new(3.125, -2.375, 4.625),
                        rotation: DVec3::new(7.25, -3.125, 2.625),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        let (bound, expected) = fit_existing(&before, id).unwrap();
        assert_eq!(bound, face);
        assert_ne!(
            parse(document.entity(id).unwrap()).unwrap().corners(),
            expected.corners()
        );
        assert_eq!(
            document.edit("Refit overlay", |tx| refit(tx, id)).unwrap(),
            ObjectId::Entity(id)
        );
        let after = document.vmf().clone();
        assert_eq!(parse(document.entity(id).unwrap()).unwrap(), expected);
        let mut retained = document.entity(id).unwrap().clone();
        let original = before
            .entities
            .iter()
            .find(|entity| entity.id == id)
            .unwrap();
        for key in [
            "origin",
            "sides",
            "BasisOrigin",
            "BasisNormal",
            "BasisU",
            "BasisV",
            "uv0",
            "uv1",
            "uv2",
            "uv3",
        ] {
            retained
                .properties
                .set(key, original.properties.get(key).unwrap());
        }
        assert_eq!(&retained, original);
        assert_eq!(after.entities.len(), before.entities.len());
        for original in before.solids() {
            assert_eq!(
                after
                    .solids()
                    .find(|solid| solid.id == original.id)
                    .unwrap(),
                original
            );
        }
        assert_eq!(document.undo().unwrap().label, "Refit overlay");
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &after);
    }
}

#[test]
fn refit_rejects_ambiguous_hidden_missing_and_malformed_bindings_atomically() {
    let (mut document, face) = fixture(false);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create(tx, face, options()))
        .unwrap()
    else {
        unreachable!()
    };
    for case in 0..6 {
        let mut source = document.vmf().clone();
        let other_side = source.world.solids[0]
            .sides
            .iter()
            .find(|side| side.id != face.side)
            .unwrap()
            .id;
        let overlay = source
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        match case {
            0 => overlay
                .properties
                .set("sides", format!("{} {other_side}", face.side)),
            1 => overlay.hidden = true,
            2 => overlay.properties.set("sides", u32::MAX.to_string()),
            3 => overlay.properties.0.push(("basisu".into(), "1 0 0".into())),
            4 => overlay.properties.set("BasisNormal", "malformed"),
            5 => source.world.solids[0].hidden = true,
            _ => unreachable!(),
        }
        let mut invalid = Document::new(source);
        let before = invalid.vmf().clone();
        let revision = invalid.revision();
        assert!(fit_existing(&before, id).is_err(), "case {case}");
        assert!(
            invalid.edit("invalid refit", |tx| refit(tx, id)).is_err(),
            "case {case}"
        );
        assert_eq!(invalid.vmf(), &before);
        assert_eq!(invalid.revision(), revision);
    }
}

#[test]
fn explicit_rebinding_fits_a_different_owner_face_and_preserves_raw_entity_data() {
    use source_formats::keyvalues::Node;
    use source_formats::vmf::{Connection, Properties};

    for destination_detail in [false, true] {
        let (mut document, original_face) = fixture(!destination_detail);
        let ObjectId::Entity(id) = document
            .edit("create overlay", |tx| create(tx, original_face, options()))
            .unwrap()
        else {
            unreachable!()
        };
        let destination = document
            .edit("destination and imported fields", |tx| {
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(-180.125, 32.375, 128.625),
                    DVec3::new(-84.125, 96.375, 144.625),
                    "test/checker",
                )?
                else {
                    unreachable!()
                };
                let side = tx
                    .solid_mut(solid)?
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id;
                edit::transform(
                    tx,
                    &[ObjectId::Solid(solid)],
                    edit::Transform {
                        rotation: DVec3::new(13.125, -24.625, 7.375),
                        ..Default::default()
                    },
                )?;
                if destination_detail {
                    edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                }
                let entity = tx.entity_mut(id)?;
                for (key, value) in [
                    ("material", "Materials/Decals/marking.VMT"),
                    ("targetname", "  original marking  "),
                    ("RenderOrder", "03"),
                    ("StartU", "01.000"),
                    ("custom", "opaque value"),
                ] {
                    entity.properties.set(key, value);
                }
                entity.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: "relay,Trigger,,0,-1".into(),
                }]);
                entity.editor = Some(Properties(vec![("color".into(), "1 2 3".into())]));
                entity.extra.push(Node::block(
                    "opaque",
                    vec![Node::string("keep", "unchanged")],
                ));
                Ok(FaceId { solid, side })
            })
            .unwrap();
        let before = document.vmf().clone();
        let expected = fit_rebind(&before, id, destination).unwrap();
        assert_eq!(expected.sides, [destination.side]);
        assert!(fit_rebind(&before, id, original_face).is_err());
        assert_eq!(
            document
                .edit("Rebind overlay", |tx| rebind(tx, id, destination))
                .unwrap(),
            ObjectId::Entity(id)
        );
        assert_eq!(parse(document.entity(id).unwrap()).unwrap(), expected);
        let after = document.vmf().clone();
        let original = before
            .entities
            .iter()
            .find(|entity| entity.id == id)
            .unwrap();
        let mut retained = document.entity(id).unwrap().clone();
        for key in [
            "origin",
            "sides",
            "BasisOrigin",
            "BasisNormal",
            "BasisU",
            "BasisV",
            "uv0",
            "uv1",
            "uv2",
            "uv3",
        ] {
            retained
                .properties
                .set(key, original.properties.get(key).unwrap());
        }
        assert_eq!(&retained, original);
        assert_eq!(before.entities.len(), after.entities.len());
        for solid in before.solids() {
            assert_eq!(
                after
                    .solids()
                    .find(|candidate| candidate.id == solid.id)
                    .unwrap(),
                solid
            );
        }
        assert_eq!(document.undo().unwrap().label, "Rebind overlay");
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &after);
    }
}

#[test]
fn rebind_rejects_an_unsupported_existing_binding_or_hidden_destination_atomically() {
    let (mut document, source) = fixture(false);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create(tx, source, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let destination = document
        .edit("destination", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::splat(-256.0),
                DVec3::splat(-192.0),
                "test/checker",
            )?
            else {
                unreachable!()
            };
            Ok(FaceId {
                solid,
                side: tx.solid_mut(solid)?.sides[0].id,
            })
        })
        .unwrap();
    for unsupported_binding in [false, true] {
        let mut vmf = document.vmf().clone();
        if unsupported_binding {
            vmf.entities
                .iter_mut()
                .find(|entity| entity.id == id)
                .unwrap()
                .properties
                .set("sides", format!("{} {}", source.side, destination.side));
        } else {
            vmf.world
                .solids
                .iter_mut()
                .find(|solid| solid.id == destination.solid)
                .unwrap()
                .hidden = true;
        }
        let mut invalid = Document::new(vmf);
        let before = invalid.vmf().clone();
        assert!(
            invalid
                .edit("invalid rebind", |tx| rebind(tx, id, destination))
                .is_err()
        );
        assert_eq!(invalid.vmf(), &before);
        assert_eq!(invalid.revision(), 0);
    }
}

#[test]
fn corner_edits_keep_the_imported_frame_binding_settings_and_metadata() {
    use source_formats::{keyvalues::Node, vmf::Connection};
    for detail in [false, true] {
        let (mut document, face) = fixture(detail);
        let ObjectId::Entity(id) = document
            .edit("overlay", |tx| create(tx, face, options()))
            .unwrap()
        else {
            unreachable!()
        };
        document
            .edit("raw import", |tx| {
                let entity = tx.entity_mut(id)?;
                entity
                    .properties
                    .set("material", "Materials/Decals/marking.VMT");
                entity.properties.set("StartU", "01.0000");
                entity.properties.set("sides", format!("  {}  ", face.side));
                entity.properties.set("origin", "999.000 888.000 777.000");
                entity.properties.set("custom", "keep me");
                for index in 0..4 {
                    let mut point =
                        parse_vec3(entity.properties.get(&format!("uv{index}")).unwrap()).unwrap();
                    point.z = 17.0 + index as f64;
                    entity
                        .properties
                        .set(format!("uv{index}"), format_vec3(point));
                }
                entity.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: "relay,Trigger,,0,-1".into(),
                }]);
                entity
                    .extra
                    .push(Node::block("custom", vec![Node::string("key", "value")]));
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        let (bound, existing) = editable_corners(&before, id).unwrap();
        assert_eq!(bound, face);
        let corners = [
            [-24.5, -16.25],
            [-20.0, 24.75],
            [28.5, 16.5],
            [32.25, -20.125],
        ];
        let (bound, expected) = with_corners(&before, id, corners).unwrap();
        assert_eq!(bound, face);
        assert_ne!(expected.corners(), existing.corners());
        assert_eq!(expected.basis_origin, existing.basis_origin);
        assert_eq!(expected.basis_u, existing.basis_u);
        assert_eq!(expected.basis_v, existing.basis_v);
        assert_eq!(
            document
                .edit("Edit overlay corners", |tx| edit_corners(tx, id, corners))
                .unwrap(),
            ObjectId::Entity(id)
        );
        let after = document.vmf().clone();
        assert_eq!(parse(document.entity(id).unwrap()).unwrap(), expected);
        let original = before
            .entities
            .iter()
            .find(|entity| entity.id == id)
            .unwrap();
        let mut retained = document.entity(id).unwrap().clone();
        for index in 0..4 {
            let key = format!("uv{index}");
            let point = parse_vec3(retained.properties.get(&key).unwrap()).unwrap();
            assert_eq!(point.z, 17.0 + index as f64);
            retained
                .properties
                .set(&key, original.properties.get(&key).unwrap());
        }
        assert_eq!(&retained, original);
        for original in before.solids() {
            assert_eq!(
                after
                    .solids()
                    .find(|solid| solid.id == original.id)
                    .unwrap(),
                original
            );
        }
        let reopened = Vmf::parse(&after.write(&Default::default())).unwrap();
        assert_eq!(editable_corners(&reopened, id).unwrap().1, expected);
        assert_eq!(document.undo().unwrap().label, "Edit overlay corners");
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &after);
    }
}

#[test]
fn corner_edits_reject_nonconvex_nonfinite_bounds_and_native_world_collapse_atomically() {
    let (mut document, face) = fixture(false);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create(tx, face, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let before = document.vmf().clone();
    for corners in [
        [[f64::NAN, 0.0], [0.0, 64.0], [64.0, 64.0], [64.0, 0.0]],
        [[f64::INFINITY, 0.0], [0.0, 64.0], [64.0, 64.0], [64.0, 0.0]],
        [[0.0, 0.0], [64.0, 64.0], [0.0, 64.0], [64.0, 0.0]],
        [[0.0, 0.0], [0.0, 64.0], [0.0, 64.0], [64.0, 0.0]],
        [[0.0, 0.0], [0.0, 64.0], [16.0, 16.0], [64.0, 0.0]],
        [[0.0, 0.0], [0.0, 64.0], [1e9, 64.0], [1e9, 0.0]],
        [[0.0, 0.0], [0.0, 64.0], [1e-8, 64.0], [1e-8, 0.0]],
    ] {
        let revision = document.revision();
        assert!(with_corners(&before, id, corners).is_err(), "{corners:?}");
        assert!(
            document
                .edit("invalid corners", |tx| edit_corners(tx, id, corners))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
    }
    assert!(
        with_corners(
            &before,
            id,
            [[0.0, 0.0], [0.0, 64.0], [1e-8, 64.0], [1e-8, 0.0]]
        )
        .unwrap_err()
        .contains("native world-coordinate")
    );
}

#[test]
fn moved_sources_require_explicit_refit_before_corner_editing() {
    let (mut document, face) = fixture(false);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create(tx, face, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let owner = document.owner_of_solid(face.solid).unwrap();
    document
        .edit("move source", |tx| {
            edit::translate(tx, &[owner], DVec3::new(0.0, 0.0, 16.0), true)
        })
        .unwrap();
    let before = document.vmf().clone();
    let corners = [[-24.0, -16.0], [-20.0, 24.0], [28.0, 16.0], [32.0, -20.0]];
    assert!(
        with_corners(&before, id, corners)
            .unwrap_err()
            .contains("Refit selected overlay")
    );
    assert_eq!(document.vmf(), &before);
    document.edit("refit", |tx| refit(tx, id)).unwrap();
    assert!(with_corners(document.vmf(), id, corners).is_ok());
}

#[test]
fn corner_editing_retains_left_handed_axes_and_rejects_ambiguous_or_unsupported_bindings() {
    let (mut document, face) = fixture(false);
    let ObjectId::Entity(id) = document
        .edit("overlay", |tx| create(tx, face, options()))
        .unwrap()
    else {
        unreachable!()
    };
    let definition = parse(document.entity(id).unwrap()).unwrap();
    document
        .edit("left handed frame", |tx| {
            tx.entity_mut(id)?.properties.set(
                "BasisV",
                format_vec3(-DVec3::from_array(definition.basis_v)),
            );
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let corners = [[-24.0, -16.0], [-20.0, 24.0], [28.0, 16.0], [32.0, -20.0]];
    assert_eq!(
        with_corners(&before, id, corners).unwrap().1.basis_v,
        definition.basis_v.map(|value| -value)
    );
    for case in 0..8 {
        let mut invalid = before.clone();
        let index = invalid
            .entities
            .iter()
            .position(|entity| entity.id == id)
            .unwrap();
        match case {
            0 => invalid.entities[index].hidden = true,
            1 => invalid.world.solids[0].hidden = true,
            2 => invalid.entities[index]
                .properties
                .set("sides", u32::MAX.to_string()),
            3 => {
                let other = invalid.world.solids[0]
                    .sides
                    .iter()
                    .find(|side| side.id != face.side)
                    .unwrap()
                    .id;
                invalid.entities[index]
                    .properties
                    .set("sides", format!("{} {other}", face.side));
            }
            4 => {
                let duplicate = invalid.entities[index]
                    .properties
                    .get("uv0")
                    .unwrap()
                    .to_owned();
                invalid.entities[index]
                    .properties
                    .0
                    .push(("uv0".into(), duplicate));
            }
            5 => {
                let mut owner = Document::new(invalid);
                owner
                    .edit("model", |tx| {
                        edit::tie_to_entity(tx, &[face.solid], "func_brush", &[])
                    })
                    .unwrap();
                invalid = owner.into_vmf();
            }
            6 => {
                let mut terrain = Document::new(invalid);
                terrain
                    .edit("terrain", |tx| {
                        displacement::edit::create(tx, &[face], 2, 0)
                    })
                    .unwrap();
                invalid = terrain.into_vmf();
            }
            7 => {
                let u = DVec3::from_array(definition.basis_u);
                let v = -DVec3::from_array(definition.basis_v);
                invalid.entities[index]
                    .properties
                    .set("BasisV", format_vec3((v + u * 0.005).normalize()));
            }
            _ => unreachable!(),
        }
        assert!(with_corners(&invalid, id, corners).is_err(), "case {case}");
    }
    assert_eq!(document.vmf(), &before);
}
