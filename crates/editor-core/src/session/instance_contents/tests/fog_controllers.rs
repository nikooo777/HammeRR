use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const FGD: &str = r#"
@PointClass = env_fog_controller [
    fogenable(choices) : "Fog Enable" : 0 = [ 0 : "No" 1 : "Yes" ]
    fogblend(choices) : "Fog Blend" : 0 = [ 0 : "No" 1 : "Yes" ]
    use_angles(choices) : "Use Angles for Fog Dir" : 0 = [ 0 : "No" 1 : "Yes" ]
    fogcolor(color255) : "Primary Fog Color" : "255 255 255"
    fogcolor2(color255) : "Secondary Fog Color" : "255 255 255"
    fogdir(string) : "Primary Fog Direction" : "1 0 0"
    fogstart(string) : "Fog Start" : "500.0"
    fogend(string) : "Fog End" : "2000.0"
    fogmaxdensity(float) : "Fog Max Density" : "1"
    foglerptime(float) : "Interpolate time" : "0"
    farz(string) : "Far Z Clip Plane" : "-1"
]
"#;

fn authored(
    actor: Actor,
    mixed: bool,
) -> (tempfile::TempDir, Session, PathBuf, PathBuf, ObjectId, u32) {
    let (directory, mut session, source, child, owner) = opened(actor);
    let definitions = crate::steam::find_css_fgd(session.game())
        .map_or_else(|| Catalog::parse(FGD), |path| Catalog::load(&path))
        .unwrap();
    session.set_definitions(actor, definitions);
    session.new_map(actor, true).unwrap();
    if mixed {
        session
            .create_block(
                actor,
                DVec3::new(0.0, 0.0, 32.0),
                DVec3::new(16.0, 16.0, 48.0),
                "dev/dev_measuregeneric01",
                false,
            )
            .unwrap();
    }
    let ObjectId::Entity(fog) = session
        .create_entity(
            actor,
            "env_fog_controller",
            Some(DVec3::new(16.0, 8.0, 64.0)),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        unreachable!()
    };
    session
        .set_entity_properties(actor, fog, &[("fogenable".into(), Some("1".into()))])
        .unwrap();
    session.save(actor, Some(&child)).unwrap();
    session.open(actor, &source, false).unwrap();
    session.select(&[owner], SelectMode::Replace).unwrap();
    (directory, session, source, child, owner, fog)
}

#[test]
fn saved_child_fog_shares_actor_refresh_compile_and_unavailable_parent_preview() {
    for actor in [Actor::User, Actor::Agent] {
        for mixed in [false, true] {
            let (_directory, mut session, source, child, owner, _) = authored(actor, mixed);
            session
                .begin_property_edit(crate::session::PropertyTarget::World)
                .unwrap();
            let token = session.property_draft().unwrap().token;
            session
                .append_property_change(
                    token,
                    crate::session::PropertyChange::Set {
                        key: "message".into(),
                        value: "Retain the fog review".into(),
                    },
                )
                .unwrap();
            let memory = session.document().vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let draft = json!(session.property_draft());
            let parent_bytes = std::fs::read(&source).unwrap();
            let child_bytes = std::fs::read(&child).unwrap();
            let report = refresh(&mut session, actor);
            assert_eq!(report["state"], "ready", "{report}");
            assert_eq!(report["stats"]["fog_controllers"], 1);
            assert_eq!(report["fog_preview"]["available"], false);
            assert_eq!(report["fog_preview"]["reason"], FOG_PREVIEW_REASON);
            assert_eq!(session.fog_preview_state()["controllers"], json!([]));
            assert!(!session.fog_preview_state()["active"].as_bool().unwrap());
            if mixed {
                assert_eq!(report["available"], true);
                assert_eq!(report["brushes_total"], 1);
                assert_eq!(
                    session.instance_contents_hits(
                        &Ray {
                            origin: DVec3::new(8.0, 8.0, 128.0),
                            direction: DVec3::NEG_Z
                        },
                        Some(true)
                    )[0]
                    .object,
                    owner
                );
            } else {
                assert_eq!(report["available"], false);
                assert!(report["owners"][0]["bounds"].is_null());
                assert_eq!(report["geometry"]["vertices"], 0);
                assert!(
                    session
                        .frame_instance_contents(
                            actor,
                            report["token"].as_u64().unwrap(),
                            &[owner],
                            1.0
                        )
                        .is_err()
                );
                assert_eq!(session.camera(), camera);
            }
            let result = session.begin_compile(actor, None).unwrap().run();
            let output = &result.as_ref().unwrap().output;
            assert!(output.bsp.is_some(), "{:?}", output.report);
            assert_eq!(output.stats.instances.as_ref().unwrap().fog_controllers, 1);
            let entities = output.bsp.as_ref().unwrap().entities().unwrap();
            assert!(entities.contains("\"classname\" \"env_fog_controller\""));
            assert!(entities.contains("\"origin\" \"16 8 64\""));
            session.finish_compile(actor, &result);
            assert!(result.as_ref().unwrap().published());
            assert_eq!(session.document().vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(json!(session.property_draft()), draft);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.selected_ids(), vec![owner]);
            assert_eq!(std::fs::read(&source).unwrap(), parent_bytes);
            assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        }
    }
}

#[test]
fn child_fog_source_changes_fail_refresh_and_cancel_without_retaining_preview() {
    let (_directory, mut session, source, child, _, fog) = authored(Actor::Agent, false);
    let original = std::fs::read_to_string(&child).unwrap();
    let parent_bytes = std::fs::read(&source).unwrap();
    assert_eq!(
        refresh(&mut session, Actor::Agent)["stats"]["fog_controllers"],
        1
    );
    let memory = session.document().vmf().clone();
    for (key, value) in [
        ("use_angles", "1"),
        ("fogstart", "$Start"),
        ("origin", "16385 0 0"),
    ] {
        let mut invalid = Vmf::parse(&original).unwrap();
        invalid
            .entities
            .iter_mut()
            .find(|entity| entity.id == fog)
            .unwrap()
            .properties
            .set(key, value);
        std::fs::write(&child, invalid.write(&Default::default())).unwrap();
        let failed = refresh(&mut session, Actor::Agent);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(failed.get("fog_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(session.document().vmf(), &memory);
    }
    std::fs::write(&child, &original).unwrap();
    let request = session
        .begin_instance_contents_refresh(Actor::Agent)
        .unwrap();
    session
        .cancel_instance_contents_refresh(Actor::Agent, request.token())
        .unwrap();
    let cancelled = session
        .finish_instance_contents_refresh(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    assert!(cancelled.get("fog_preview").is_none());
    assert!(!session.instance_workers_busy());
    assert_eq!(
        refresh(&mut session, Actor::Agent)["stats"]["fog_controllers"],
        1
    );
    assert_eq!(std::fs::read(&source).unwrap(), parent_bytes);
    assert_eq!(std::fs::read_to_string(&child).unwrap(), original);
}
