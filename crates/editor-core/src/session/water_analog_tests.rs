use super::*;
use map_core::edit;

const FGD: &str = r#"
    @BaseClass = MovingWater [
        origin(vector) : "Origin" : "700 800 900"
        angles(angle) : "Orientation" : "0 90 0"
        StartPosition(float) : "Start position" : 1
        spawnflags(flags) = [ 1 : "Start open" : 1 ]
        speed(integer) : "Speed" : 100
        movedir(angle) : "Move direction" : "0 0 0"
        movedistance(float) : "Move distance" : 128
        blockdamage(float) : "Block damage" : 0
    ]
    @SolidClass base(MovingWater) = func_water_analog []
    @SolidClass base(MovingWater) = func_movelinear []
"#;

fn fixture(actor: Actor, definitions: bool) -> (Session, ObjectId) {
    let mut session = Session::new(Err("no game required".into()));
    if definitions {
        session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    }
    let brush = session
        .edit(actor, "block", |tx| {
            let brush = edit::create_block(
                tx,
                DVec3::new(-96.0, -320.0, 32.0),
                DVec3::new(224.0, 128.0, 288.0),
                "tools/toolsnodraw",
            )?;
            let ObjectId::Solid(id) = brush else {
                unreachable!()
            };
            tx.solid_mut(id)?
                .editor
                .as_mut()
                .unwrap()
                .set("color", "128 160 192");
            Ok(brush)
        })
        .unwrap();
    session.select(&[brush], SelectMode::Replace).unwrap();
    (session, brush)
}

#[test]
fn water_analog_creation_defaults_match_for_users_agents_and_loaded_definitions() {
    let mut sources = Vec::new();
    for actor in [Actor::User, Actor::Agent] {
        for definitions in [false, true] {
            let (mut session, brush) = fixture(actor, definitions);
            let before = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let ObjectId::Entity(id) = session
                .tie_to_entity(actor, &[brush], "FUNC_WATER_ANALOG", BTreeMap::new())
                .unwrap()
            else {
                panic!()
            };
            let entity = session.document().entity(id).unwrap();
            assert_eq!(
                entity.properties.iter().collect::<BTreeMap<_, _>>(),
                BTreeMap::from([
                    ("classname", "func_water_analog"),
                    ("origin", "64 -96 160"),
                    ("angles", "0 0 0"),
                    ("StartPosition", "0"),
                    ("spawnflags", "0"),
                ])
            );
            assert_eq!(entity.solids, before.world.solids);
            assert!(session.document().vmf().world.solids.is_empty());
            assert_eq!(session.selected_ids(), [ObjectId::Entity(id)]);
            let report = session.inspect_history(0, 1).unwrap();
            assert_eq!(report.total, history + 1);
            assert_eq!(report.entries[0].actor, actor.label());
            assert_eq!(report.entries[0].label, "tie to func_water_analog");
            let authored = session.document().vmf().clone();
            sources.push(authored.clone());
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.document().undo_labels().count(), history);
            session.redo(actor).unwrap();
            assert_eq!(session.document().vmf(), &authored);
        }
    }
    assert!(sources.iter().all(|source| source == &sources[0]));

    let (mut session, brush) = fixture(Actor::Agent, true);
    let ObjectId::Entity(id) = session
        .tie_to_entity(Actor::Agent, &[brush], "func_movelinear", BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    let entity = session.document().entity(id).unwrap();
    assert_eq!(entity.properties.get("speed"), Some("100"));
    assert_eq!(entity.properties.get("origin"), Some("700 800 900"));
    assert_eq!(entity.properties.get("StartPosition"), Some("1"));
}

#[test]
fn water_analog_creation_preserves_explicit_properties_for_compiler_validation() {
    let properties = BTreeMap::from([
        ("ORIGIN".into(), "1e1 -2 3.5".into()),
        ("angles".into(), "0 90 0".into()),
        ("StartPosition".into(), "0.5".into()),
        ("spawnflags".into(), "1".into()),
        ("speed".into(), "120".into()),
        ("targetname".into(), "authored_water".into()),
        ("custom_key".into(), "keep me".into()),
    ]);
    for actor in [Actor::User, Actor::Agent] {
        for definitions in [false, true] {
            let (mut session, brush) = fixture(actor, definitions);
            let ObjectId::Entity(id) = session
                .tie_to_entity(
                    actor,
                    &[brush],
                    compiler::WATER_ANALOG_CLASSNAME,
                    properties.clone(),
                )
                .unwrap()
            else {
                panic!()
            };
            let mut expected = properties.clone();
            expected.insert("classname".into(), compiler::WATER_ANALOG_CLASSNAME.into());
            assert_eq!(
                session
                    .document()
                    .entity(id)
                    .unwrap()
                    .properties
                    .0
                    .iter()
                    .cloned()
                    .collect::<BTreeMap<_, _>>(),
                expected
            );
        }
    }

    let (mut session, brush) = fixture(Actor::User, false);
    let ObjectId::Entity(id) = session
        .tie_to_entity(
            Actor::User,
            &[brush],
            compiler::WATER_ANALOG_CLASSNAME,
            BTreeMap::from([("angle".into(), "90".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    let entity = session.document().entity(id).unwrap();
    assert_eq!(entity.properties.get("angle"), Some("90"));
    assert_eq!(entity.properties.get("angles"), None);
}

#[test]
fn water_analog_creation_failures_preserve_brush_selection_and_history() {
    for actor in [Actor::User, Actor::Agent] {
        for definitions in [false, true] {
            let (mut session, brush) = fixture(actor, definitions);
            let before = session.document().vmf().clone();
            let history = session.history_snapshot();
            let selection = session.selected_ids();
            for properties in [
                BTreeMap::from([("origin".into(), "0 NaN 0".into())]),
                BTreeMap::from([("angles".into(), "90".into())]),
                BTreeMap::from([("custom_key".into(), "bad\"value".into())]),
                BTreeMap::from([("classname".into(), "func_detail".into())]),
                BTreeMap::from([
                    ("ORIGIN".into(), "1 2 3".into()),
                    ("origin".into(), "4 5 6".into()),
                ]),
            ] {
                assert!(
                    session
                        .tie_to_entity(
                            actor,
                            &[brush],
                            compiler::WATER_ANALOG_CLASSNAME,
                            properties
                        )
                        .is_err()
                );
                assert_eq!(session.document().vmf(), &before);
                assert_eq!(session.history_snapshot(), history);
                assert_eq!(session.selected_ids(), selection);
            }
            assert!(
                session
                    .tie_to_entity(
                        actor,
                        &[brush, ObjectId::Solid(u32::MAX)],
                        compiler::WATER_ANALOG_CLASSNAME,
                        BTreeMap::new(),
                    )
                    .is_err()
            );
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.selected_ids(), selection);
            session.set_definitions(
                actor,
                Catalog::parse("@PointClass = func_water_analog []").unwrap(),
            );
            assert!(
                session
                    .tie_to_entity(
                        actor,
                        &[brush],
                        compiler::WATER_ANALOG_CLASSNAME,
                        BTreeMap::new()
                    )
                    .unwrap_err()
                    .contains("point entity class")
            );
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.selected_ids(), selection);
        }
    }
}

#[test]
fn water_analog_imports_and_property_edits_do_not_apply_creation_defaults() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, brush) = fixture(actor, false);
        let ObjectId::Entity(id) = session
            .edit(actor, "imported water", |tx| {
                edit::tie_to_entity(
                    tx,
                    &[match brush {
                        ObjectId::Solid(id) => id,
                        _ => panic!(),
                    }],
                    compiler::WATER_ANALOG_CLASSNAME,
                    &[("speed".into(), "150".into())],
                )
            })
            .unwrap()
        else {
            panic!()
        };
        let saved = session.document().vmf().write(&WriteOptions::default());
        let imported = Vmf::parse(&saved).unwrap();
        session.replace_document(Document::new(imported.clone()), None);
        session.set_definitions(actor, Catalog::parse(FGD).unwrap());
        assert_eq!(session.document().vmf(), &imported);
        session
            .set_entity_properties(actor, id, &[("targetname".into(), Some("renamed".into()))])
            .unwrap();
        let entity = session.document().entity(id).unwrap();
        assert_eq!(entity.properties.get("targetname"), Some("renamed"));
        assert_eq!(entity.properties.get("speed"), Some("150"));
        for key in ["origin", "angles", "StartPosition", "spawnflags"] {
            assert_eq!(entity.properties.get(key), None);
        }
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &imported);
    }
}
