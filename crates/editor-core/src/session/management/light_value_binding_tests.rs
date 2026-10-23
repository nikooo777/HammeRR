use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const FGD: &str = r#"
@BaseClass = Light [
    _light(color255) : "Brightness" : "255 255 255 200"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _ambient(color255) : "Ambient" : "255 255 255 20"
    _ambientHDR(color255) : "AmbientHDR" : "-1 -1 -1 1"
    _AmbientScaleHDR(float) : "Ambient HDR scale" : "1"
    _lightscaleHDR(float) : "HDR scale" : "1"
    _fifty_percent_distance(string) : "50 percent distance" : "0"
    _zero_percent_distance(string) : "Near-zero distance" : "0"
    SunSpreadAngle(float) : "SunSpreadAngle" : "0"
    _inner_cone(integer) : "Inner (bright) angle" : 30
    _cone(integer) : "Outer (fading) angle" : 45
    _exponent(integer) : "Focus" : 1
    rendercolor(color255) : "Color" : "255 255 255"
]
@PointClass base(Light) = light []
@PointClass base(Light) = light_spot [ pitch(integer) : "Pitch" : -90 ]
@PointClass base(Light) = light_environment [ pitch(integer) : "Pitch" : -90 ]
@PointClass base(Light) = light_dynamic []
@SolidClass base(Light) = worldspawn []
"#;

fn session(actor: Actor) -> Session {
    let mut session = Session::new(Err("template authoring needs no assets".into()));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    session
}

fn create(session: &mut Session, actor: Actor, classname: &str) -> u32 {
    let mut properties = BTreeMap::from([
        ("_light".into(), "$Lamp".into()),
        ("_lightHDR".into(), "$_HDR_1".into()),
        ("_lightscaleHDR".into(), "$Gain".into()),
    ]);
    if classname == "light_environment" {
        properties.insert("_ambient".into(), "$Sky".into());
        properties.insert("_ambientHDR".into(), "$SkyHDR".into());
        properties.insert("_AmbientScaleHDR".into(), "$SkyGain".into());
        properties.insert("SunSpreadAngle".into(), "$Spread".into());
    }
    if matches!(classname, "light" | "light_spot") {
        properties.insert("_fifty_percent_distance".into(), "$Half".into());
        properties.insert("_zero_percent_distance".into(), "$End".into());
    }
    if classname == "light_spot" {
        for (key, value) in [
            ("_inner_cone", "$Inner"),
            ("_cone", "$Outer"),
            ("_exponent", "$Focus"),
            ("pitch", "$Tilt"),
        ] {
            properties.insert(key.into(), value.into());
        }
    }
    let ObjectId::Entity(id) = session
        .create_entity(actor, classname, Some(glam::DVec3::Z * 64.0), properties)
        .unwrap()
    else {
        unreachable!()
    };
    id
}

#[test]
fn light_value_templates_share_create_set_draft_undo_and_save_reopen_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for classname in ["light", "light_spot", "light_environment"] {
            let mut session = session(actor);
            let id = create(&mut session, actor, classname);
            session
                .set_entity_properties(
                    actor,
                    id,
                    &[
                        ("_light".into(), Some("$Other".into())),
                        ("_lightscaleHDR".into(), Some("$OtherGain".into())),
                    ],
                )
                .unwrap();
            let before = session.document().vmf().clone();
            let history = session.history_snapshot();
            session
                .begin_property_edit(PropertyTarget::Entities(vec![id]))
                .unwrap();
            let token = session.property_draft().unwrap().token;
            let mut changes = vec![
                PropertyChange::Set {
                    key: "_light".into(),
                    value: "$Draft".into(),
                },
                PropertyChange::Set {
                    key: "_lightscaleHDR".into(),
                    value: "$GainDraft".into(),
                },
            ];
            if classname == "light_environment" {
                changes.extend(["_ambient", "_ambientHDR"].map(|key| PropertyChange::Set {
                    key: key.into(),
                    value: "$SkyDraft".into(),
                }));
                changes.push(PropertyChange::Set {
                    key: "_AmbientScaleHDR".into(),
                    value: "$SkyGainDraft".into(),
                });
                changes.push(PropertyChange::Set {
                    key: "SunSpreadAngle".into(),
                    value: "$SpreadDraft".into(),
                });
            }
            if classname != "light_environment" {
                changes.extend(
                    ["_fifty_percent_distance", "_zero_percent_distance"].map(|key| {
                        PropertyChange::Set {
                            key: key.into(),
                            value: "$DistanceDraft".into(),
                        }
                    }),
                );
            }
            if classname == "light_spot" {
                changes.extend(["_inner_cone", "_cone", "_exponent", "pitch"].map(|key| {
                    PropertyChange::Set {
                        key: key.into(),
                        value: "$ShapeDraft".into(),
                    }
                }));
            }
            session.update_property_edit(token, changes).unwrap();
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
            assert_eq!(
                reopened.properties.get("_lightscaleHDR"),
                Some("$GainDraft")
            );
            let keys: &[_] = if classname == "light_environment" {
                assert_eq!(reopened.properties.get("_ambient"), Some("$SkyDraft"));
                assert_eq!(reopened.properties.get("_ambientHDR"), Some("$SkyDraft"));
                assert_eq!(
                    reopened.properties.get("_AmbientScaleHDR"),
                    Some("$SkyGainDraft")
                );
                assert_eq!(
                    reopened.properties.get("SunSpreadAngle"),
                    Some("$SpreadDraft")
                );
                &[
                    "_light",
                    "_lightHDR",
                    "_ambient",
                    "_ambientHDR",
                    "_lightscaleHDR",
                    "_AmbientScaleHDR",
                    "SunSpreadAngle",
                ]
            } else if classname == "light_spot" {
                for key in ["_inner_cone", "_cone", "_exponent", "pitch"] {
                    assert_eq!(reopened.properties.get(key), Some("$ShapeDraft"));
                }
                &[
                    "_light",
                    "_lightHDR",
                    "_lightscaleHDR",
                    "_fifty_percent_distance",
                    "_zero_percent_distance",
                    "_inner_cone",
                    "_cone",
                    "_exponent",
                    "pitch",
                ]
            } else {
                assert_eq!(
                    reopened.properties.get("_fifty_percent_distance"),
                    Some("$DistanceDraft")
                );
                assert_eq!(
                    reopened.properties.get("_zero_percent_distance"),
                    Some("$DistanceDraft")
                );
                &[
                    "_light",
                    "_lightHDR",
                    "_lightscaleHDR",
                    "_fifty_percent_distance",
                    "_zero_percent_distance",
                ]
            };
            let field = session.entity_inspector(id).unwrap();
            for &key in keys {
                let field = field
                    .properties
                    .iter()
                    .find(|field| field.key == key)
                    .unwrap();
                assert_eq!(field.control, inspector::Control::Text);
                assert!(field.color.is_none() && field.intensity.is_none());
                if key.to_ascii_lowercase().contains("scalehdr")
                    || key.ends_with("_distance")
                    || key == "SunSpreadAngle"
                    || matches!(key, "_inner_cone" | "_cone" | "_exponent" | "pitch")
                {
                    assert_eq!(
                        field.kind,
                        if key.ends_with("_distance") {
                            "string"
                        } else if matches!(key, "_inner_cone" | "_cone" | "_exponent" | "pitch") {
                            "integer"
                        } else {
                            "float"
                        }
                    );
                    assert!(field.components.is_empty() && field.range.is_none());
                }
                assert!(
                    field
                        .description
                        .contains("Unresolved template light value")
                );
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
                    .set_entity_properties(
                        actor,
                        id,
                        &[("_lightscaleHDR".into(), Some(value.clone()))]
                    )
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
        for key in [
            "_ambient",
            "_ambientHDR",
            "rendercolor",
            "_AmbientScaleHDR",
            "SunSpreadAngle",
            "_inner_cone",
            "_cone",
            "_exponent",
        ] {
            assert!(
                session
                    .set_entity_properties(actor, id, &[(key.into(), Some("$Lamp".into()))])
                    .is_err()
            );
        }
        assert!(
            session
                .create_entity(
                    actor,
                    "light_dynamic",
                    None,
                    BTreeMap::from([("_light".into(), "$Lamp".into())]),
                )
                .is_err()
        );
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
fn environment_bindings_keep_unrelated_color_parameters_literal() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = session(actor);
        let id = create(&mut session, actor, "light_environment");
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        assert!(
            session
                .set_entity_properties(actor, id, &[("rendercolor".into(), Some("$Sky".into()))])
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        assert!(
            session
                .set_entity_properties(actor, id, &[("pitch".into(), Some("$Pitch".into()))])
                .is_err()
        );
        for key in [
            "_ambient",
            "_ambientHDR",
            "_lightscaleHDR",
            "_AmbientScaleHDR",
            "SunSpreadAngle",
        ] {
            session
                .set_entity_properties(actor, id, &[(key.into(), Some("$SetSky".into()))])
                .unwrap();
            assert_eq!(
                session.document().entity(id).unwrap().properties.get(key),
                Some("$SetSky")
            );
        }
    }
}

#[test]
fn light_value_template_exception_preserves_readonly_schema_and_generic_color_validation() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = session(actor);
        let id = create(&mut session, actor, "light");
        let spot_id = create(&mut session, actor, "light_spot");
        let catalog = Catalog::parse(FGD).unwrap();
        let color = catalog.class("light").unwrap().property("_light").unwrap();
        assert!(color.validate("$Lamp").is_err());
        assert!(color.validate("255 220 180 400").is_ok());
        for (key, declarations) in [
            ("_light", ["color255) readonly", "float)", "color1)"]),
            ("_inner_cone", ["integer) readonly", "float)", "color255)"]),
            ("_cone", ["integer) readonly", "float)", "color255)"]),
            ("_exponent", ["integer) readonly", "float)", "color255)"]),
            ("pitch", ["integer) readonly", "float)", "color255)"]),
            (
                "_lightscaleHDR",
                ["float) readonly", "integer)", "color255)"],
            ),
            (
                "_fifty_percent_distance",
                ["string) readonly", "integer)", "float)"],
            ),
            (
                "_zero_percent_distance",
                ["string) readonly", "integer)", "float)"],
            ),
        ] {
            let classname = if matches!(key, "_inner_cone" | "_cone" | "_exponent" | "pitch") {
                "light_spot"
            } else {
                "light"
            };
            let id = if classname == "light_spot" {
                spot_id
            } else {
                id
            };
            for declaration in declarations {
                session.set_definitions(
                    actor,
                    Catalog::parse(&format!(
                        "@PointClass = {classname} [ {key}({declaration} : \"Brightness\" ]"
                    ))
                    .unwrap(),
                );
                let source = session.document().vmf().clone();
                assert!(
                    session
                        .set_entity_properties(actor, id, &[(key.into(), Some("$Lamp".into()))])
                        .is_err()
                );
                assert!(
                    session
                        .create_entity(
                            actor,
                            classname,
                            None,
                            BTreeMap::from([(key.into(), "$Lamp".into())]),
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
                                key: key.into(),
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
}
