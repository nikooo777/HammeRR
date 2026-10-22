use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const APPEARANCES: [(&str, &str); 12] = [
    ("0", "Normal"),
    ("10", "Fluorescent flicker"),
    ("2", "Slow, strong pulse"),
    ("11", "Slow pulse, noblack"),
    ("5", "Gentle pulse"),
    ("1", "Flicker A"),
    ("6", "Flicker B"),
    ("3", "Candle A"),
    ("7", "Candle B"),
    ("8", "Candle C"),
    ("4", "Fast strobe"),
    ("9", "Slow strobe"),
];

const FGD: &str = r#"
@BaseClass = Light [
    _light(color255) : "Brightness" : "255 255 255 200"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _lightscaleHDR(float) : "BrightnessScaleHDR" : "1"
    style(choices) : "Appearance" : 0 = [
        0 : "Normal"
        10 : "Fluorescent flicker"
        2 : "Slow, strong pulse"
        11 : "Slow pulse, noblack"
        5 : "Gentle pulse"
        1 : "Flicker A"
        6 : "Flicker B"
        3 : "Candle A"
        7 : "Candle B"
        8 : "Candle C"
        4 : "Fast strobe"
        9 : "Slow strobe"
    ]
    pattern(string) : "Custom Appearance" : ""
    spawnflags(flags) = [ 1 : "Initially dark" : 0 ]
]
@PointClass base(Light) = light []
@PointClass base(Light) = light_spot [
    angles(angle) : "Pitch Yaw Roll (Y Z X)" : "0 0 0"
    pitch(integer) : "Pitch" : -90
    _inner_cone(integer) : "Inner (bright) angle" : 30
    _cone(integer) : "Outer (fading) angle" : 45
    _exponent(integer) : "Focus" : 1
]
"#;

#[test]
fn offered_child_light_patterns_preserve_values_and_preview_limits_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game) = crate::test_assets::game();
        let source = directory.path().join("parent.vmf");
        let child = directory.path().join("child.vmf");
        let definitions = crate::steam::find_css_fgd(&game)
            .map_or_else(|| Catalog::parse(FGD), |path| Catalog::load(&path))
            .unwrap();
        for classname in ["light", "light_spot"] {
            let style = definitions
                .class(classname)
                .unwrap()
                .property("style")
                .unwrap();
            assert_eq!(style.label, "Appearance");
            assert_eq!(
                style
                    .choices
                    .iter()
                    .map(|choice| (choice.value.as_str(), choice.label.as_str()))
                    .collect::<Vec<_>>(),
                APPEARANCES
            );
        }
        let mut session = Session::new(Ok(game));
        session.set_definitions(actor, definitions);
        let mut authored = Vec::new();
        for (row, classname) in ["light", "light_spot"].into_iter().enumerate() {
            for (column, (value, _)) in APPEARANCES.into_iter().enumerate() {
                let ObjectId::Entity(id) = session
                    .create_entity(
                        actor,
                        classname,
                        Some(DVec3::new(column as f64 * 8.0, row as f64 * 16.0, 96.0)),
                        BTreeMap::new(),
                    )
                    .unwrap()
                else {
                    unreachable!()
                };
                session
                    .set_entity_properties(actor, id, &[("style".into(), Some(value.into()))])
                    .unwrap();
                authored.push((id, classname, value));
            }
        }
        session.save(actor, Some(&child)).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let child_memory = session.document().vmf().clone();
        let child_history = session.history_snapshot();
        assert!(
            session
                .set_entity_properties(actor, authored[0].0, &[("style".into(), Some("12".into()))])
                .is_err()
        );
        assert_eq!(session.document().vmf(), &child_memory);
        assert_eq!(session.history_snapshot(), child_history);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        session.open(actor, &child, false).unwrap();
        for (id, classname, value) in authored {
            let entity = session.document().entity(id).unwrap();
            assert_eq!(entity.classname(), Some(classname));
            assert_eq!(entity.properties.get("style"), Some(value));
            assert_eq!(entity.properties.get("spawnflags"), Some("0"));
            assert!(
                entity
                    .properties
                    .get("targetname")
                    .is_none_or(str::is_empty)
            );
            assert!(entity.properties.get("pattern").is_none_or(str::is_empty));
            let inspector = crate::inspector::inspect(entity, session.definitions.class(classname));
            let style = inspector
                .properties
                .iter()
                .find(|field| field.key == "style")
                .unwrap();
            assert_eq!(style.label, "Light pattern");
            assert_eq!(style.value, value);
        }
        std::fs::write(&source, Document::empty().vmf().write(&Default::default())).unwrap();
        session.open(actor, &source, false).unwrap();
        let owner = session
            .create_entity(
                actor,
                "func_instance",
                Some(DVec3::new(32.0, 48.0, 16.0)),
                BTreeMap::from([("file".into(), "child.vmf".into())]),
            )
            .unwrap();
        session.save(actor, None).unwrap();
        std::fs::write(source.with_extension("bsp"), b"previous BSP").unwrap();
        std::fs::write(source.with_extension("lin"), b"previous pointfile").unwrap();
        session.set_view(render::ViewOptions {
            instance_contents: true,
            ..session.view()
        });
        session.select(&[owner], SelectMode::Replace).unwrap();
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                crate::session::PropertyChange::Set {
                    key: "message".into(),
                    value: "retained root draft".into(),
                },
            )
            .unwrap();
        let draft = json!(session.property_draft());
        let memory = session.document().vmf().clone();
        let parent_bytes = std::fs::read(&source).unwrap();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["point_lights"], APPEARANCES.len());
        assert_eq!(report["stats"]["spot_lights"], APPEARANCES.len());
        assert!(report["stats"].get("environment_lights").is_none());
        assert_eq!(
            report["static_light_preview"],
            json!({
                "available": false,
                "reason": "Child lights, illumination and icons are not shown in the reference or authored-lighting preview."
            })
        );
        assert_eq!(report["available"], false);
        assert_eq!(report["displayed"], false);
        assert!(report["owners"][0]["bounds"].is_null());
        assert_eq!(report["brushes_total"], 0);
        assert_eq!(report["face_provenance_total"], 0);
        assert_eq!(report["geometry"]["vertices"], 0);
        assert_eq!(report["geometry"]["triangles"], 0);
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
        assert_eq!(
            report["stats"]["dependencies"][0]["bytes"],
            child_bytes.len()
        );
        assert!(
            session
                .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.0)
                .unwrap_err()
                .contains("no visible geometry")
        );
        assert_eq!(session.camera(), camera);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(std::fs::read(&source).unwrap(), parent_bytes);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}
