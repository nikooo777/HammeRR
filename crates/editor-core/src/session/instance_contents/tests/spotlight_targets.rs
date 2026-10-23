use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const FGD: &str = r#"
@PointClass = info_target [
    targetname(target_source) : "Name" : ""
    angles(angle) : "Pitch Yaw Roll (Y Z X)" : "0 0 0"
    spawnflags(flags) = [ 1 : "Transmit to client" : 0 ]
]
@PointClass = light_spot [
    target(target_destination) : "Target" : ""
    angles(angle) : "Pitch Yaw Roll (Y Z X)" : "0 0 0"
    pitch(integer) : "Pitch" : -45
    _light(color255) : "Brightness" : "255 255 255 200"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _lightscaleHDR(float) : "BrightnessScaleHDR" : "1"
    _inner_cone(integer) : "Inner cone" : 20
    _cone(integer) : "Outer cone" : 35
    _exponent(float) : "Focus" : "1"
]
"#;

#[test]
fn shared_authored_spotlight_targets_report_counts_without_parent_geometry_and_preserve_state() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game) = crate::test_assets::game();
        let child = directory.path().join("child.vmf");
        let source = directory.path().join("parent.vmf");
        let mut session = Session::new(Ok(game));
        session.set_definitions(actor, Catalog::parse(FGD).unwrap());
        session
            .create_entity(
                actor,
                "info_target",
                Some(DVec3::new(8.0, 24.0, 64.0)),
                BTreeMap::from([("targetname".into(), "aim".into())]),
            )
            .unwrap();
        session
            .create_entity(
                actor,
                "light_spot",
                Some(DVec3::new(8.0, 24.0, 128.0)),
                BTreeMap::from([("target".into(), "aim".into())]),
            )
            .unwrap();
        session.save(actor, Some(&child)).unwrap();
        let bytes = std::fs::read(&child).unwrap();
        std::fs::write(
            &source,
            crate::test_assets::room().write(&Default::default()),
        )
        .unwrap();
        session.open(actor, &source, false).unwrap();
        let owner = session
            .create_entity(
                actor,
                "func_instance",
                Some(DVec3::new(56.0, 0.0, 0.0)),
                BTreeMap::from([
                    ("file".into(), "child.vmf".into()),
                    ("targetname".into(), "assembly".into()),
                    ("fixup_style".into(), "0".into()),
                ]),
            )
            .unwrap();
        session.save(actor, None).unwrap();
        session.set_view(render::ViewOptions {
            instance_contents: true,
            ..session.view()
        });
        session.select(&[owner], SelectMode::Replace).unwrap();
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let draft = json!(session.property_draft());
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["spotlight_targets"], 1);
        assert_eq!(report["stats"]["spot_lights"], 1);
        assert_eq!(report["limits"]["spotlight_target_name_bytes"], 127);
        assert_eq!(report["spotlight_target_preview"]["available"], false);
        assert_eq!(
            report["spotlight_target_preview"]["reason"],
            SPOTLIGHT_TARGET_PREVIEW_REASON
        );
        assert_eq!(report["available"], false);
        assert_eq!(report["displayed"], false);
        assert_eq!(report["geometry"]["vertices"], 0);
        assert_eq!(report["face_provenance_total"], 0);
        assert!(report["owners"][0]["bounds"].is_null());
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(std::fs::read(&child).unwrap(), bytes);
        let mut changed = Vmf::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
        changed
            .entities
            .iter_mut()
            .find(|entity| entity.classname() == Some("light_spot"))
            .unwrap()
            .properties
            .set("target", "missing");
        std::fs::write(&child, changed.write(&Default::default())).unwrap();
        let stale = session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale["stats"].get("spotlight_targets").is_none());
        let failed = refresh(&mut session, actor);
        assert_eq!(failed["state"], "failed");
        assert!(failed["stats"].get("spotlight_targets").is_none());
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .contains("Spotlight target must name")
        );
    }
}
