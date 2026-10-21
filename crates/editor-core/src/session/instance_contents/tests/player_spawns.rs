use super::*;
use crate::test_assets::instance_player_spawns;
use std::collections::BTreeMap;

fn assert_unavailable(session: &mut Session, actor: Actor, owner: ObjectId, report: &Value) {
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["stats"]["player_spawns"], 2);
    assert_eq!(report["player_spawn_preview"]["available"], false);
    assert_eq!(
        report["player_spawn_preview"]["reason"],
        PLAYER_SPAWN_PREVIEW_REASON
    );
    assert_eq!(report["available"], false);
    assert_eq!(report["displayed"], false);
    assert!(report["owners"][0]["bounds"].is_null());
    for field in ["vertices", "indices", "line_vertices", "triangles"] {
        assert_eq!(report["geometry"][field], 0);
    }
    assert_eq!(report["face_provenance_total"], 0);
    assert!(
        session
            .instance_contents_hits(
                &Ray {
                    origin: DVec3::new(40.0, -8.0, 128.0),
                    direction: DVec3::NEG_Z,
                },
                Some(true)
            )
            .is_empty()
    );
    let camera = session.camera();
    assert!(
        session
            .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.0)
            .unwrap_err()
            .contains("no visible geometry")
    );
    assert_eq!(session.camera(), camera);
}

fn spawn_rows(bsp: &source_formats::bsp::Bsp) -> Vec<BTreeMap<String, String>> {
    source_formats::keyvalues::parse_flat_blocks_bounded(
        bsp.entities().unwrap(),
        source_formats::keyvalues::FlatBlockLimits {
            max_bytes: 1024 * 1024,
            max_blocks: 1024,
            max_pairs: 16_384,
        },
    )
    .unwrap()
    .into_iter()
    .map(|row| row.into_iter().collect::<BTreeMap<_, _>>())
    .filter(|row| {
        matches!(
            row.get("classname").map(String::as_str),
            Some("info_player_terrorist" | "info_player_counterterrorist")
        )
    })
    .collect()
}

#[test]
fn saved_spawns_share_actor_compilation_identity_and_explicit_missing_preview() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = instance_player_spawns::fixture(actor, false);
        let owner = ObjectId::Entity(fixture.owner);
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
                    value: "Retain the spawn review".into(),
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
        let report = refresh(session, actor);
        assert_unavailable(session, actor, owner, &report);
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
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
        assert_eq!(output.stats.instances.as_ref().unwrap().player_spawns, 2);
        assert!(output.static_prop_sources.is_none());
        let rows = spawn_rows(output.bsp.as_ref().unwrap());
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|row| {
            row.get("classname").map(String::as_str) == Some("info_player_terrorist")
                && row
                    .get("origin")
                    .and_then(|raw| parse_vec3(raw))
                    .is_some_and(|point| point.abs_diff_eq(DVec3::new(40.0, -8.0, 48.0), 1e-6))
        }));
        let counter = rows
            .iter()
            .find(|row| {
                row.get("classname").map(String::as_str) == Some("info_player_counterterrorist")
            })
            .unwrap();
        assert!(
            parse_vec3(&counter["origin"])
                .unwrap()
                .abs_diff_eq(DVec3::new(8.0, -8.0, 48.0), 1e-6)
        );
        let angles = parse_vec3(&counter["angles"]).unwrap().map(f64::to_radians);
        let rotation = glam::DQuat::from_euler(glam::EulerRot::ZYX, angles.y, angles.x, angles.z);
        assert!((rotation * DVec3::X).abs_diff_eq(DVec3::X, 1e-6));
        assert!((rotation * DVec3::Y).abs_diff_eq(DVec3::NEG_Z, 1e-6));
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert_eq!(
            session.compile_status()["job"]["result"]["stats"]["instances"]["player_spawns"],
            2
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
            .find(|entity| entity.id == fixture.spawns[0])
            .unwrap()
            .properties
            .set("origin", "17 8 32");
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        assert!(request.verify_instance_dependencies().is_err());
        session
            .finish_launch(
                actor,
                &origin,
                Err("test stopped before game launch".into()),
            )
            .err()
            .unwrap();
        drop(request);
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert_eq!(session.compile_status()["launch"]["ready"], false);
        let stale = session.inspect_instance_contents(0, 64).unwrap();
        assert_ne!(stale["state"], "ready");
        assert!(stale.get("player_spawn_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(
            std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
            published
        );
        assert_eq!(
            std::fs::read(fixture.source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        std::fs::write(&fixture.child, &child_bytes).unwrap();
        let restored = refresh(session, actor);
        assert_unavailable(session, actor, owner, &restored);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child_bytes);
    }
}

#[test]
fn spawn_counts_preserve_actual_brush_provenance_and_failed_or_cancelled_refresh_cleanup() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = instance_player_spawns::fixture(actor, true);
        let original = std::fs::read(&fixture.child).unwrap();
        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        child
            .entities
            .retain(|entity| !fixture.spawns.contains(&entity.id));
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let baseline = refresh(&mut fixture.session, actor);
        assert!(baseline["stats"].get("player_spawns").is_none());
        assert!(baseline.get("player_spawn_preview").is_none());
        std::fs::write(&fixture.child, &original).unwrap();
        let session = &mut fixture.session;
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let source = std::fs::read(&fixture.source).unwrap();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready");
        assert_eq!(report["stats"]["player_spawns"], 2);
        assert_eq!(report["player_spawn_preview"]["available"], false);
        assert_eq!(report["geometry"], baseline["geometry"]);
        assert_eq!(report["face_provenance"], baseline["face_provenance"]);
        assert_eq!(
            report["owners"][0]["bounds"],
            baseline["owners"][0]["bounds"]
        );
        assert_eq!(report["available"], true);
        let ray = Ray {
            origin: DVec3::new(32.0, -8.0, 128.0),
            direction: DVec3::NEG_Z,
        };
        let hits = session.instance_contents_hits(&ray, Some(true));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, ObjectId::Entity(fixture.owner));
        for (key, value) in [("enabled", "1"), ("origin", "NaN 0 0")] {
            let mut invalid = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
            invalid
                .entities
                .iter_mut()
                .find(|entity| entity.id == fixture.spawns[0])
                .unwrap()
                .properties
                .set(key, value);
            let invalid_bytes = invalid.write(&Default::default());
            std::fs::write(&fixture.child, &invalid_bytes).unwrap();
            let failed = refresh(session, actor);
            assert_eq!(failed["state"], "failed", "{failed}");
            assert!(failed["error"].as_str().unwrap().contains(key), "{failed}");
            assert!(failed.get("player_spawn_preview").is_none());
            assert_eq!(failed["loading"], false);
            assert!(!session.instance_workers_busy());
            assert!(session.instance_contents_preview(true).mesh.is_none());
            assert!(session.instance_contents_hits(&ray, Some(true)).is_empty());
            assert_eq!(
                std::fs::read_to_string(&fixture.child).unwrap(),
                invalid_bytes
            );
        }
        std::fs::write(&fixture.child, &original).unwrap();
        assert_eq!(refresh(session, actor)["state"], "ready");
        let request = session.begin_instance_contents_refresh(actor).unwrap();
        let token = request.token();
        session
            .cancel_instance_contents_refresh(actor, token)
            .unwrap();
        let cancelled = session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert_eq!(cancelled["loading"], false);
        assert!(cancelled.get("player_spawn_preview").is_none());
        assert!(!session.instance_workers_busy());
        assert!(session.instance_contents_hits(&ray, Some(true)).is_empty());
        let ready = refresh(session, actor);
        assert_eq!(ready["stats"]["player_spawns"], 2);
        assert_eq!(ready["geometry"], baseline["geometry"]);
        session
            .clear_instance_contents(actor, ready["token"].as_u64().unwrap())
            .unwrap();
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
    }
}

#[test]
fn spawn_inspection_counts_precede_region_filtering_and_hidden_invalid_spawns_are_ignored() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = instance_player_spawns::fixture(actor, false);
        let mut child = Vmf::parse(&std::fs::read_to_string(&fixture.child).unwrap()).unwrap();
        child
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.spawns[1])
            .unwrap()
            .properties
            .set("origin", "192 8 32");
        let mut hidden = child.entities[0].clone();
        hidden.id = child.object_ids().chain(child.side_ids()).max().unwrap() + 1;
        hidden.hidden = true;
        hidden.properties.set("enabled", "1");
        hidden.properties.set("origin", "NaN 0 0");
        child.entities.push(hidden);
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let session = &mut fixture.session;
        let stamp = session.compile_region_stamp();
        let token = session
            .preview_compile_region(
                actor,
                Some(map_core::compile_region::Definition {
                    enabled: true,
                    bounds: map_core::compile_region::Bounds {
                        mins: DVec3::new(-64.0, -64.0, 0.0),
                        maxs: DVec3::new(64.0, 64.0, 160.0),
                    },
                }),
                &stamp,
            )
            .unwrap()["draft"]["token"]
            .as_u64()
            .unwrap();
        session.apply_compile_region(actor, token).unwrap();
        let report = refresh(session, actor);
        assert_unavailable(session, actor, ObjectId::Entity(fixture.owner), &report);
        let result = session.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert_eq!(output.stats.instances.as_ref().unwrap().player_spawns, 2);
        let rows = spawn_rows(output.bsp.as_ref().unwrap());
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().all(
                |row| row.get("classname").map(String::as_str) == Some("info_player_terrorist")
            )
        );
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
    }
}
