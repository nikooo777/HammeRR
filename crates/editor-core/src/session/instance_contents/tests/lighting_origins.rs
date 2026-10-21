use super::*;
use crate::test_assets::instance_lighting_origins::{self, Authored};
use crate::test_assets::instance_static_props::MODEL;
use std::collections::BTreeMap;

fn select_owner(fixture: &mut Authored) -> ObjectId {
    let owner = ObjectId::Entity(fixture.owner);
    fixture.session.set_view(render::ViewOptions {
        instance_contents: true,
        ..fixture.session.view()
    });
    fixture
        .session
        .select(&[owner], SelectMode::Replace)
        .unwrap();
    owner
}

fn region(session: &mut Session, actor: Actor) {
    let definition = map_core::compile_region::Definition {
        enabled: true,
        bounds: map_core::compile_region::Bounds {
            mins: DVec3::new(-64.0, -64.0, 0.0),
            maxs: DVec3::new(64.0, 64.0, 160.0),
        },
    };
    let stamp = session.compile_region_stamp();
    let token = session
        .preview_compile_region(actor, Some(definition), &stamp)
        .unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    session.apply_compile_region(actor, token).unwrap();
}

fn root_prop(session: &mut Session, actor: Actor) {
    session
        .create_entity(
            actor,
            "prop_static",
            Some(DVec3::new(-32.0, 32.0, 8.0)),
            BTreeMap::from([
                ("model".into(), MODEL.into()),
                ("solid".into(), "0".into()),
                ("lightingorigin".into(), "assembly-sample".into()),
            ]),
        )
        .unwrap();
}

fn assert_helper_disclosure(report: &Value) {
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["stats"]["lighting_origins"], 1);
    assert_eq!(report["limits"]["lighting_origin_name_bytes"], 127);
    assert_eq!(report["lighting_origin_preview"]["available"], false);
    assert_eq!(
        report["lighting_origin_preview"]["reason"],
        LIGHTING_ORIGIN_PREVIEW_REASON
    );
    assert_eq!(report["available"], false);
    assert_eq!(report["displayed"], false);
    assert!(report["owners"][0]["bounds"].is_null());
    assert_eq!(report["geometry"]["vertices"], 0);
    assert_eq!(report["geometry"]["indices"], 0);
    assert_eq!(report["face_provenance_total"], 0);
}

#[test]
fn saved_local_helpers_and_explicit_root_bindings_share_region_counts_and_staleness() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = instance_lighting_origins::fixture(actor);
        fixture
            .session
            .set_entity_properties(
                actor,
                fixture.owner,
                &[("origin".into(), Some("24 -16 16".into()))],
            )
            .unwrap();
        root_prop(&mut fixture.session, actor);
        fixture.session.save(actor, None).unwrap();
        region(&mut fixture.session, actor);
        let owner = select_owner(&mut fixture);
        let session = &mut fixture.session;
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                crate::session::PropertyChange::Set {
                    key: "message".into(),
                    value: "Retain the scoped-helper review".into(),
                },
            )
            .unwrap();
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let draft = json!(session.property_draft());
        let source_bytes = std::fs::read(&fixture.source).unwrap();
        let child_bytes = std::fs::read(&fixture.child).unwrap();
        let model_bytes = std::fs::read(session.game.game_dir.join(MODEL)).unwrap();
        let report = refresh(session, actor);
        assert_helper_disclosure(&report);
        assert_eq!(report["stats"]["static_props"], 1);
        let request = session.begin_compile(actor, None).unwrap();
        let result = if actor == Actor::Agent {
            let (result, native) = request.run_admitted();
            assert!(native.is_none());
            result
        } else {
            request.run()
        };
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        let stats = output.stats.instances.as_ref().unwrap();
        assert_eq!((stats.static_props, stats.lighting_origins), (1, 1));
        assert_eq!(output.stats.static_props, 2);
        let bsp = output.bsp.as_ref().unwrap();
        let props = bsp.static_props().unwrap().unwrap();
        assert_eq!(props.props.len(), 2);
        assert_eq!(props.props[0].origin, [-32.0, 32.0, 8.0]);
        assert_eq!(props.props[1].origin, [56.0, 16.0, 24.0]);
        for prop in &props.props {
            assert_eq!(prop.lighting_origin, [120.0, -48.0, 80.0]);
            assert_ne!(prop.flags & 2, 0);
        }
        assert!(!bsp.entities().unwrap().contains("info_lighting"));
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert_eq!(
            session.compile_status()["job"]["result"]["stats"]["instances"]["lighting_origins"],
            1
        );
        assert_eq!(session.compile_status()["job"]["stale"], false);
        drop(result);
        let published = std::fs::read(fixture.source.with_extension("bsp")).unwrap();
        let request = session.begin_launch(actor).unwrap();
        let origin = request.origin();
        request.verify_instance_dependencies().unwrap();
        let mut child = Vmf::parse(&String::from_utf8(child_bytes.clone()).unwrap()).unwrap();
        child
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap()
            .properties
            .set("origin", "97 -32 64");
        let changed_child = child.write(&Default::default());
        std::fs::write(&fixture.child, &changed_child).unwrap();
        assert!(request.verify_instance_dependencies().is_err());
        session
            .finish_launch(
                actor,
                &origin,
                Err("test stopped before native spawn".into()),
            )
            .err()
            .unwrap();
        drop(request);
        let status = session.compile_status();
        assert_eq!(status["job"]["status"], "completed");
        assert_eq!(status["job"]["stale"], true);
        assert_eq!(status["job"]["instance_dependencies"]["current"], false);
        assert_eq!(status["launch"]["ready"], false);
        assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
        assert_ne!(
            session.inspect_instance_contents(0, 64).unwrap()["state"],
            "ready"
        );
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(
            std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
            published
        );
        assert_eq!(
            std::fs::read(fixture.source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        assert_eq!(
            std::fs::read_to_string(&fixture.child).unwrap(),
            changed_child
        );
        std::fs::write(&fixture.child, &child_bytes).unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], true);
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert_helper_disclosure(&refresh(session, actor));
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child_bytes);
        assert_eq!(
            std::fs::read(session.game.game_dir.join(MODEL)).unwrap(),
            model_bytes
        );
    }
}

#[test]
fn helper_only_references_keep_identity_without_model_reads_geometry_or_hits() {
    let actor = Actor::User;
    let mut fixture = instance_lighting_origins::fixture(actor);
    let mut child = Vmf::parse(&std::fs::read_to_string(&fixture.child).unwrap()).unwrap();
    child
        .entities
        .retain(|entity| entity.classname() == Some("info_lighting"));
    std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
    std::fs::remove_file(fixture.session.game.game_dir.join(MODEL)).unwrap();
    let owner = select_owner(&mut fixture);
    let memory = fixture.session.document().vmf().clone();
    let history = fixture.session.history_snapshot();
    let source_bytes = std::fs::read(&fixture.source).unwrap();
    let child_bytes = std::fs::read(&fixture.child).unwrap();
    let report = refresh(&mut fixture.session, actor);
    assert_helper_disclosure(&report);
    assert!(report["stats"].get("static_props").is_none());
    assert!(report.get("static_prop_preview").is_none());
    assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
    assert!(
        fixture
            .session
            .instance_contents_hits(
                &Ray {
                    origin: DVec3::new(96.0, -32.0, 128.0),
                    direction: DVec3::NEG_Z,
                },
                Some(true)
            )
            .is_empty()
    );
    assert!(
        fixture
            .session
            .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.0)
            .unwrap_err()
            .contains("no visible geometry")
    );
    let result = fixture.session.begin_compile(actor, None).unwrap().run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().lighting_origins, 1);
    assert_eq!(output.stats.static_props, 0);
    assert!(output.static_prop_sources.is_none());
    assert!(
        !output
            .bsp
            .as_ref()
            .unwrap()
            .entities()
            .unwrap()
            .contains("info_lighting")
    );
    fixture.session.finish_compile(actor, &result);
    assert!(result.as_ref().unwrap().published());
    assert_eq!(fixture.session.document().vmf(), &memory);
    assert_eq!(fixture.session.history_snapshot(), history);
    assert_eq!(std::fs::read(&fixture.source).unwrap(), source_bytes);
    assert_eq!(std::fs::read(&fixture.child).unwrap(), child_bytes);
}

#[test]
fn child_binding_cannot_escape_to_root_helper_and_only_retained_props_require_it() {
    for hidden in [false, true] {
        for retained in [false, true] {
            let actor = Actor::Agent;
            let mut fixture = instance_lighting_origins::fixture(actor);
            let mut child = Vmf::parse(&std::fs::read_to_string(&fixture.child).unwrap()).unwrap();
            if hidden {
                child
                    .entities
                    .iter_mut()
                    .find(|entity| entity.id == fixture.helper)
                    .unwrap()
                    .hidden = true;
            } else {
                child.entities.retain(|entity| entity.id != fixture.helper);
            }
            if !retained {
                child.entities[0].properties.set("origin", "96 32 8");
            }
            std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
            fixture
                .session
                .create_entity(
                    actor,
                    "info_lighting",
                    Some(DVec3::new(-96.0, 32.0, 96.0)),
                    BTreeMap::from([("targetname".into(), "assembly-sample".into())]),
                )
                .unwrap();
            root_prop(&mut fixture.session, actor);
            fixture.session.save(actor, None).unwrap();
            region(&mut fixture.session, actor);
            let memory = fixture.session.document().vmf().clone();
            let history = fixture.session.history_snapshot();
            let settings = fixture.session.compile_settings();
            let source_bytes = std::fs::read(&fixture.source).unwrap();
            let child_bytes = std::fs::read(&fixture.child).unwrap();
            let result = fixture.session.begin_compile(actor, None).unwrap().run();
            let output = &result.as_ref().unwrap().output;
            if retained {
                assert!(
                    output.bsp.is_none(),
                    "A retained child must not bind the root helper"
                );
                let owner = format!("entity:{}", fixture.owner);
                assert!(
                    output
                        .report
                        .diagnostics
                        .iter()
                        .any(|diagnostic| { diagnostic.object.as_deref() == Some(owner.as_str()) }),
                    "{:?}",
                    output.report
                );
            } else {
                assert!(output.bsp.is_some(), "{:?}", output.report);
                let stats = output.stats.instances.as_ref().unwrap();
                assert_eq!((stats.static_props, stats.lighting_origins), (1, 0));
                assert_eq!(output.stats.static_props, 1);
                let props = output
                    .bsp
                    .as_ref()
                    .unwrap()
                    .static_props()
                    .unwrap()
                    .unwrap();
                assert_eq!(props.props.len(), 1);
                assert_eq!(props.props[0].lighting_origin, [-96.0, 32.0, 96.0]);
            }
            fixture.session.finish_compile(actor, &result);
            if retained {
                assert!(!result.as_ref().unwrap().published());
                assert_eq!(
                    std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
                    b"previous BSP"
                );
            } else {
                assert!(result.as_ref().unwrap().published());
            }
            assert_eq!(
                std::fs::read(fixture.source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
            assert_eq!(fixture.session.document().vmf(), &memory);
            assert_eq!(fixture.session.history_snapshot(), history);
            assert_eq!(fixture.session.compile_settings(), settings);
            assert_eq!(std::fs::read(&fixture.source).unwrap(), source_bytes);
            assert_eq!(std::fs::read(&fixture.child).unwrap(), child_bytes);
        }
    }
}

#[test]
fn helper_drift_after_worker_staging_preserves_previous_outputs_and_property_draft() {
    let actor = Actor::Agent;
    let mut fixture = instance_lighting_origins::fixture(actor);
    fixture
        .session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let draft = json!(fixture.session.property_draft());
    let memory = fixture.session.document().vmf().clone();
    let history = fixture.session.history_snapshot();
    let settings = fixture.session.compile_settings();
    let source_bytes = std::fs::read(&fixture.source).unwrap();
    let child_bytes = std::fs::read(&fixture.child).unwrap();
    let request = fixture.session.begin_compile(actor, None).unwrap();
    let (result, native) = request.run_admitted();
    assert!(native.is_none());
    assert!(result.as_ref().unwrap().output.bsp.is_some());
    let mut child = Vmf::parse(&String::from_utf8(child_bytes.clone()).unwrap()).unwrap();
    child
        .entities
        .iter_mut()
        .find(|entity| entity.id == fixture.helper)
        .unwrap()
        .properties
        .set("origin", "97 -32 64");
    let changed_child = child.write(&Default::default());
    std::fs::write(&fixture.child, &changed_child).unwrap();
    fixture.session.finish_compile(actor, &result);
    assert!(!result.as_ref().unwrap().published());
    assert!(result.as_ref().unwrap().publication_error().is_some());
    assert_eq!(
        std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
        b"previous BSP"
    );
    assert_eq!(
        std::fs::read(fixture.source.with_extension("lin")).unwrap(),
        b"previous pointfile"
    );
    assert_eq!(fixture.session.document().vmf(), &memory);
    assert_eq!(fixture.session.history_snapshot(), history);
    assert_eq!(fixture.session.compile_settings(), settings);
    assert_eq!(json!(fixture.session.property_draft()), draft);
    assert_eq!(std::fs::read(&fixture.source).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_to_string(&fixture.child).unwrap(),
        changed_child
    );
}
