use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const FGD: &str = r#"
@BaseClass = Light [
    _light(color255) : "Brightness" : "255 255 255 200"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _fifty_percent_distance(string) : "50 percent distance" : "0"
    _zero_percent_distance(string) : "Near-zero distance" : "0"
    _constant_attn(string) : "Constant" : "0"
    _linear_attn(string) : "Linear" : "0"
    _quadratic_attn(string) : "Quadratic" : "1"
    _distance(integer) : "Maximum distance" : 0
    _hardfalloff(integer) : "hard falloff" : 0
]
@PointClass base(Light) = light []
@PointClass base(Light) = light_spot [
    pitch(integer) : "Pitch" : -90
    _inner_cone(integer) : "Inner (bright) angle" : 30
    _cone(integer) : "Outer (fading) angle" : 45
    _exponent(integer) : "Focus" : 1
]
"#;

fn bound(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf, u32) {
    let (directory, game) = crate::test_assets::game();
    let child = directory.path().join("child.vmf");
    let parent = directory.path().join("parent.vmf");
    let mut session = Session::new(Ok(game));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    for classname in ["light", "light_spot"] {
        let mut properties = BTreeMap::from([
            ("_light".into(), "$Lamp".into()),
            ("_lightHDR".into(), "$HDR".into()),
            ("_fifty_percent_distance".into(), "$Half".into()),
            ("_zero_percent_distance".into(), "$End".into()),
        ]);
        properties.insert("_distance".into(), "$Radius".into());
        properties.insert(
            "_hardfalloff".into(),
            if classname == "light" {
                "0"
            } else {
                "$Coefficient"
            }
            .into(),
        );
        if classname == "light" {
            properties.insert("_fifty_percent_distance".into(), "0".into());
            properties.insert("_zero_percent_distance".into(), "0".into());
        }
        for key in ["_constant_attn", "_linear_attn", "_quadratic_attn"] {
            properties.insert(key.into(), "$Coefficient".into());
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
        session
            .create_entity(actor, classname, Some(DVec3::Z * 64.0), properties)
            .unwrap();
    }
    session.save(actor, Some(&child)).unwrap();
    std::fs::write(
        &parent,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    session.open(actor, &parent, false).unwrap();
    let mut owners = Vec::new();
    for (row, (ldr, hdr)) in [
        ("$lamp 255 220 180 400", "$hdr 255 100 50 300"),
        ("$LAMP 10 20 255 200", "$HDR 50 100 255 600"),
    ]
    .into_iter()
    .enumerate()
    {
        let ObjectId::Entity(id) = session
            .create_entity(
                actor,
                "func_instance",
                Some(DVec3::X * row as f64 * 32.0),
                BTreeMap::from([
                    ("file".into(), "child.vmf".into()),
                    ("replace01".into(), ldr.into()),
                    ("replace02".into(), hdr.into()),
                    (
                        "replace03".into(),
                        format!("$Tilt {}", if row == 0 { -45 } else { 0 }),
                    ),
                    (
                        "replace04".into(),
                        format!("$Inner {}", if row == 0 { 15 } else { 30 }),
                    ),
                    (
                        "replace05".into(),
                        format!("$Outer {}", if row == 0 { 60 } else { 0 }),
                    ),
                    (
                        "replace06".into(),
                        format!("$Focus {}", if row == 0 { 2 } else { 0 }),
                    ),
                    (
                        "replace07".into(),
                        format!("$Coefficient {}", if row == 0 { "0" } else { "1" }),
                    ),
                    (
                        "replace08".into(),
                        format!("$Half {}", if row == 0 { 64 } else { 96 }),
                    ),
                    (
                        "replace09".into(),
                        format!("$End {}", if row == 0 { 160 } else { 0 }),
                    ),
                    (
                        "replace10".into(),
                        format!("$Radius {}", if row == 0 { 192 } else { 384 }),
                    ),
                ]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        owners.push(id);
    }
    session.save(actor, None).unwrap();
    (directory, session, parent, child, owners[0])
}

#[test]
fn immediate_light_bindings_refresh_after_reference_edits_and_reopen_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, parent, child, owner) = bound(actor);
        let child_bytes = std::fs::read(&child).unwrap();
        let original = session.document().entity(owner).unwrap().clone();
        let form = session
            .begin_instance_reference(actor, Some(owner))
            .unwrap();
        let token = form["token"].as_u64().unwrap();
        assert_eq!(form["valid"], true, "{form}");
        assert_eq!(form["contents_validation"], "not_checked");
        session
            .update_instance_reference(
                actor,
                token,
                crate::session::instance_reference::Changes {
                    origin: Some("16 0 0".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        session.apply_instance_reference(actor, token).unwrap();
        let current = session.document().entity(owner).unwrap();
        for key in [
            "replace01",
            "replace02",
            "replace03",
            "replace04",
            "replace05",
            "replace06",
            "replace07",
            "replace08",
            "replace09",
            "replace10",
        ] {
            assert_eq!(current.properties.get(key), original.properties.get(key));
        }
        session.save(actor, None).unwrap();
        session.open(actor, &parent, false).unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let disk = std::fs::read(&parent).unwrap();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["point_lights"], 2);
        assert_eq!(report["stats"]["spot_lights"], 2);
        assert_eq!(report["stats"]["dependencies"].as_array().unwrap().len(), 1);
        assert_eq!(report["available"], false);
        assert_eq!(report["static_light_preview"]["available"], false);
        assert_eq!(report["geometry"]["vertices"], 0);
        assert!(
            report["owners"]
                .as_array()
                .unwrap()
                .iter()
                .all(|owner| owner["bounds"].is_null())
        );
        assert_eq!(
            report["light_value_bindings"],
            session.compile_settings()["light_value_bindings"]
        );
        assert_eq!(
            report["light_value_bindings"],
            session.get_instance_reference()["light_value_bindings"]
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(std::fs::read(parent).unwrap(), disk);
        assert_eq!(std::fs::read(child).unwrap(), child_bytes);
    }
}

#[test]
fn unused_and_invalid_bindings_still_fail_refresh_and_compile_without_publication() {
    for actor in [Actor::User, Actor::Agent] {
        for (field, binding, invalid_syntax) in [
            ("replace03", "$Unused 255 220 180 400", false),
            ("replace03", "$1invalid 255 220 180 400", true),
            ("replace07", "$Coefficient NaN", false),
            ("replace07", "$Coefficient 1e-100", false),
            ("replace10", "$Radius -1", false),
            ("replace10", "$Radius 1e-60", false),
            ("replace07", "$Coefficient 0.5", false),
        ] {
            let (_directory, mut session, parent, child, owner) = bound(actor);
            session
                .set_entity_properties(actor, owner, &[(field.into(), Some(binding.into()))])
                .unwrap();
            let source = session.document().vmf().clone();
            let history = session.history_snapshot();
            let parent_bytes = std::fs::read(&parent).unwrap();
            let child_bytes = std::fs::read(&child).unwrap();
            std::fs::write(parent.with_extension("bsp"), b"previous BSP").unwrap();
            std::fs::write(parent.with_extension("lin"), b"previous pointfile").unwrap();
            let report = if invalid_syntax {
                let error = match session.begin_instance_contents_refresh(actor) {
                    Ok(_) => panic!("Malformed binding reached the refresh worker"),
                    Err(error) => error,
                };
                assert!(error.contains("replace03") && error.contains("ASCII identifier"));
                session.inspect_instance_contents(0, MAX_ROWS).unwrap()
            } else {
                refresh(&mut session, actor)
            };
            assert_eq!(report["state"], "failed", "{binding}: {report}");
            let result = session.begin_compile(actor, None).unwrap().run();
            session.finish_compile(actor, &result);
            let result = result.as_ref().unwrap();
            assert!(result.output.bsp.is_none());
            assert!(result.output.leak.is_none());
            assert!(!result.published());
            assert_eq!(session.compile_status()["job"]["status"], "failed");
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
            assert_eq!(std::fs::read(child).unwrap(), child_bytes);
            assert_eq!(
                std::fs::read(parent.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(parent.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
        }
    }
}
