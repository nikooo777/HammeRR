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
