use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const FGD: &str = r#"
@BaseClass = Light [
    _light(color255) : "Brightness" : "255 255 255 200"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _ambient(color255) : "Ambient" : "255 255 255 20"
    _lightscaleHDR(float) : "HDR scale" : "1"
    rendercolor(color255) : "Color" : "255 255 255"
]
@PointClass base(Light) = light []
@PointClass base(Light) = light_spot [ pitch(integer) : "Pitch" : -90 ]
@PointClass base(Light) = light_environment []
@PointClass base(Light) = light_dynamic []
@SolidClass base(Light) = worldspawn []
"#;

fn session(actor: Actor) -> Session {
    let mut session = Session::new(Err("template authoring needs no assets".into()));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    session
}

fn create(session: &mut Session, actor: Actor, classname: &str) -> u32 {
    let ObjectId::Entity(id) = session
        .create_entity(
            actor,
            classname,
            Some(glam::DVec3::Z * 64.0),
            BTreeMap::from([
                ("_light".into(), "$Lamp".into()),
                ("_lightHDR".into(), "$_HDR_1".into()),
            ]),
        )
        .unwrap()
    else {
        unreachable!()
    };
    id
}

#[test]
fn light_value_templates_share_create_set_draft_undo_and_save_reopen_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for classname in ["light", "light_spot"] {
            let mut session = session(actor);
            let id = create(&mut session, actor, classname);
            session
                .set_entity_properties(actor, id, &[("_light".into(), Some("$Other".into()))])
                .unwrap();
            let before = session.document().vmf().clone();
            let history = session.history_snapshot();
            session
                .begin_property_edit(PropertyTarget::Entities(vec![id]))
                .unwrap();
            let token = session.property_draft().unwrap().token;
            session
                .update_property_edit(
                    token,
                    vec![PropertyChange::Set {
                        key: "_light".into(),
                        value: "$Draft".into(),
                    }],
                )
                .unwrap();
            let preview = session.preview_property_edit().unwrap();
            let field = &preview
                .properties
                .iter()
                .find(|property| property.field.key == "_light")
                .unwrap()
                .field;
            assert_eq!(field.value, "$Draft");
            assert_eq!(field.control, inspector::Control::Text);
            assert!(field.color.is_none() && field.intensity.is_none());
            assert!(field.components.is_empty());
            assert!(
                field
                    .description
                    .contains("Unresolved template light value")
            );
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
            assert!(
                session
                    .append_property_change(
                        token,
                        PropertyChange::Component {
                            key: "_light".into(),
                            index: 0,
                            value: 255.0,
                        },
                    )
                    .is_err()
            );
            session.commit_property_edit(actor, token).unwrap();
            let authored = session.document().vmf().clone();
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document().vmf(), &authored);
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("light-template.vmf");
            session.save(actor, Some(&path)).unwrap();
            session.open(actor, &path, false).unwrap();
            let reopened = session.document().entity(id).unwrap();
            assert_eq!(reopened.properties.get("_light"), Some("$Draft"));
            assert_eq!(reopened.properties.get("_lightHDR"), Some("$_HDR_1"));
            let field = session.entity_inspector(id).unwrap();
            for key in ["_light", "_lightHDR"] {
                let field = field
                    .properties
                    .iter()
                    .find(|field| field.key == key)
                    .unwrap();
                assert_eq!(field.control, inspector::Control::Text);
                assert!(field.color.is_none() && field.intensity.is_none());
            }
        }
    }
}

#[test]
fn light_value_template_exception_rejects_invalid_tokens_and_other_color_contexts_atomically() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = session(actor);
        let id = create(&mut session, actor, "light");
        session
            .begin_property_edit(PropertyTarget::Entities(vec![id]))
            .unwrap();
        let token = session.property_draft().unwrap().token;
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let draft = serde_json::to_value(session.property_draft()).unwrap();
        for value in [
            "$".into(),
            "$1lamp".into(),
            "$lamp suffix".into(),
            "prefix$lamp".into(),
            " $lamp".into(),
            "$lamp ".into(),
            "$lamp\n".into(),
            "$làmp".into(),
            "$lamp\0".into(),
            format!("${}", "a".repeat(65)),
        ] {
            assert!(
                session
                    .create_entity(
                        actor,
                        "light",
                        None,
                        BTreeMap::from([("_light".into(), value.clone())]),
                    )
                    .is_err(),
                "{value:?}"
            );
            assert!(
                session
                    .set_entity_properties(actor, id, &[("_lightHDR".into(), Some(value.clone()))])
                    .is_err(),
                "{value:?}"
            );
            assert!(
                session
                    .update_property_edit(
                        token,
                        vec![PropertyChange::Set {
                            key: "_light".into(),
                            value,
                        }],
                    )
                    .is_err()
            );
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(
                serde_json::to_value(session.property_draft()).unwrap(),
                draft
            );
        }
        for key in ["_ambient", "rendercolor", "_lightscaleHDR"] {
            assert!(
                session
                    .set_entity_properties(actor, id, &[(key.into(), Some("$Lamp".into()))])
                    .is_err()
            );
        }
        for classname in ["light_environment", "light_dynamic"] {
            assert!(
                session
                    .create_entity(
                        actor,
                        classname,
                        None,
                        BTreeMap::from([("_light".into(), "$Lamp".into())]),
                    )
                    .is_err()
            );
        }
        session.cancel_property_edit(token).unwrap();
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        assert!(
            session
                .update_property_edit(
                    token,
                    vec![PropertyChange::Set {
                        key: "_light".into(),
                        value: "$Lamp".into(),
                    }],
                )
                .is_err()
        );
        session.cancel_property_edit(token).unwrap();
        let longest = format!("${}", "a".repeat(64));
        session
            .set_entity_properties(actor, id, &[("_LIGHT".into(), Some(longest))])
            .unwrap();
    }
}

#[test]
fn light_value_template_exception_preserves_readonly_schema_and_generic_color_validation() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = session(actor);
        let id = create(&mut session, actor, "light");
        let catalog = Catalog::parse(FGD).unwrap();
        let color = catalog.class("light").unwrap().property("_light").unwrap();
        assert!(color.validate("$Lamp").is_err());
        assert!(color.validate("255 220 180 400").is_ok());
        for declaration in ["color255) readonly", "float)", "color1)"] {
            session.set_definitions(
                actor,
                Catalog::parse(&format!(
                    "@PointClass = light [ _light({declaration} : \"Brightness\" ]"
                ))
                .unwrap(),
            );
            let source = session.document().vmf().clone();
            assert!(
                session
                    .set_entity_properties(actor, id, &[("_light".into(), Some("$Lamp".into()))])
                    .is_err()
            );
            assert!(
                session
                    .create_entity(
                        actor,
                        "light",
                        None,
                        BTreeMap::from([("_light".into(), "$Lamp".into())]),
                    )
                    .is_err()
            );
            session
                .begin_property_edit(PropertyTarget::Entities(vec![id]))
                .unwrap();
            let token = session.property_draft().unwrap().token;
            assert!(
                session
                    .update_property_edit(
                        token,
                        vec![PropertyChange::Set {
                            key: "_light".into(),
                            value: "$Lamp".into(),
                        }],
                    )
                    .is_err()
            );
            assert_eq!(session.document().vmf(), &source);
            session.cancel_property_edit(token).unwrap();
        }
    }
}
