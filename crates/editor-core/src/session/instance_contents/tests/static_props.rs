use super::*;
use crate::test_assets::instance_static_props::{self, MODEL, PHYSICS};

fn prop_session(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf, ObjectId) {
    let (directory, game, source, child) = instance_static_props::fixture();
    let mut session = Session::new(Ok(game));
    session.open(actor, &source, true).unwrap();
    let owner = ObjectId::Entity(session.document.vmf().entities.last().unwrap().id);
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    session.select(&[owner], SelectMode::Replace).unwrap();
    (directory, session, source, child, owner)
}

#[test]
fn child_visibility_options_share_no_read_refresh_native_fields_and_guarded_refusal() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, source, child, owner) = prop_session(actor);
        session.open(actor, &child, true).unwrap();
        let prop = session.document().vmf().entities[0].id;
        session
            .set_entity_properties(
                actor,
                prop,
                &[
                    ("fademindist".into(), Some("256".into())),
                    ("fademaxdist".into(), Some("64".into())),
                    ("fadescale".into(), Some("0.5".into())),
                    ("screenspacefade".into(), Some("1".into())),
                    ("mindxlevel".into(), Some("80".into())),
                    ("maxdxlevel".into(), Some("95".into())),
                ],
            )
            .unwrap();
        session.save(actor, None).unwrap();
        session.open(actor, &source, false).unwrap();
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
                    value: "Retain the prop visibility review".into(),
                },
            )
            .unwrap();
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let draft = json!(session.property_draft());
        let source_bytes = std::fs::read(&source).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let model = session.game.game_dir.join(MODEL);
        let model_bytes = std::fs::read(&model).unwrap();
        std::fs::write(&model, b"unread model during visibility refresh").unwrap();
        let report = refresh(&mut session, actor);
        assert_model_preview_unavailable(&mut session, actor, &report, owner);
        assert_eq!(report["stats"]["static_props"], 1);
        assert_eq!(
            std::fs::read(&model).unwrap(),
            b"unread model during visibility refresh"
        );
        std::fs::write(&model, &model_bytes).unwrap();
        let output = directory.path().join("outputs/visibility.bsp");
        std::fs::write(&output, b"previous BSP").unwrap();
        std::fs::write(output.with_extension("lin"), b"visibility pointfile").unwrap();
        let request = session.begin_compile(actor, Some(&output)).unwrap();
        let result = if actor == Actor::Agent {
            let (result, native) = request.run_admitted();
            assert!(native.is_none());
            result
        } else {
            request.run()
        };
        let compiled = &result.as_ref().unwrap().output;
        assert!(compiled.bsp.is_some(), "{:?}", compiled.report);
        let props = compiled
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap();
        assert_eq!(props.props.len(), 1);
        let native = &props.props[0];
        assert_eq!(
            (native.fade_min, native.fade_max, native.fade_scale),
            (256.0, 64.0, 0.5)
        );
        assert_eq!(native.flags, 1 | 32);
        assert_eq!(native.dx_levels, [80, 95]);
        compiled
            .static_prop_sources
            .as_ref()
            .unwrap()
            .verify_sources(&session.game, &compiler::Control::default())
            .unwrap();
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        drop(result);
        let published = std::fs::read(&output).unwrap();
        let mut invalid = Vmf::parse(&String::from_utf8(child_bytes.clone()).unwrap()).unwrap();
        invalid.entities[0].properties.set("fadescale", "NaN");
        let invalid_bytes = invalid.write(&Default::default());
        std::fs::write(&child, &invalid_bytes).unwrap();
        std::fs::write(&model, b"unread model for malformed visibility").unwrap();
        let failed = refresh(&mut session, actor);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(
            failed["error"].as_str().unwrap().contains("fadescale"),
            "{failed}"
        );
        assert_eq!(failed["loading"], false);
        assert!(!session.instance_workers_busy());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        let result = session.begin_compile(actor, Some(&output)).unwrap().run();
        let refused = &result.as_ref().unwrap().output;
        assert!(refused.bsp.is_none() && refused.report.has_errors());
        assert!(
            refused
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("fadescale")),
            "{:?}",
            refused.report
        );
        session.finish_compile(actor, &result);
        assert!(!result.as_ref().unwrap().published());
        assert_eq!(std::fs::read(&output).unwrap(), published);
        assert_eq!(
            std::fs::read(output.with_extension("lin")).unwrap(),
            b"visibility pointfile"
        );
        assert_eq!(std::fs::read_to_string(&child).unwrap(), invalid_bytes);
        assert_eq!(
            std::fs::read(&model).unwrap(),
            b"unread model for malformed visibility"
        );
        std::fs::write(&child, &child_bytes).unwrap();
        std::fs::write(&model, &model_bytes).unwrap();
        assert_eq!(refresh(&mut session, actor)["state"], "ready");
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
    }
}

fn assert_model_preview_unavailable(
    session: &mut Session,
    actor: Actor,
    report: &Value,
    owner: ObjectId,
) {
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["available"], false);
    assert_eq!(report["displayed"], false);
    assert_eq!(report["static_prop_preview"]["available"], false);
    assert_eq!(
        report["static_prop_preview"]["reason"],
        STATIC_PROP_PREVIEW_REASON
    );
    assert!(report["owners"][0]["bounds"].is_null());
    assert_eq!(report["geometry"]["vertices"], 0);
    assert_eq!(report["geometry"]["indices"], 0);
    assert_eq!(report["geometry"]["line_vertices"], 0);
    assert_eq!(report["face_provenance_total"], 0);
    let preview = session.instance_contents_preview(true);
    let mesh = preview.mesh.unwrap();
    assert!(mesh.vertices.is_empty() && mesh.indices.is_empty() && mesh.edges.is_empty());
    assert!(
        session
            .instance_contents_hits(
                &Ray {
                    origin: DVec3::new(32.0, 32.0, 128.0),
                    direction: DVec3::NEG_Z,
                },
                Some(true)
            )
            .is_empty()
    );
    assert!(
        session
            .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.0)
            .unwrap_err()
            .contains("no visible geometry")
    );
}

#[test]
fn model_only_references_compile_real_props_without_inventing_preview_geometry() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, source, child, owner) = prop_session(actor);
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                crate::session::PropertyChange::Set {
                    key: "message".into(),
                    value: "Retain static-prop review".into(),
                },
            )
            .unwrap();
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let draft = json!(session.property_draft());
        let source_bytes = std::fs::read(&source).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let model_bytes = std::fs::read(session.game.game_dir.join(MODEL)).unwrap();
        let report = refresh(&mut session, actor);
        assert_model_preview_unavailable(&mut session, actor, &report, owner);
        assert_eq!(report["stats"]["static_props"], 1);
        assert_eq!(report["stats"]["instances"], 1);
        assert_eq!(report["stats"]["files"], 1);
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
        let output = directory.path().join("outputs/props.bsp");
        let result = session.begin_compile(actor, Some(&output)).unwrap().run();
        let compiled = &result.as_ref().unwrap().output;
        assert!(compiled.bsp.is_some(), "{:?}", compiled.report);
        assert_eq!(compiled.stats.instances.as_ref().unwrap().static_props, 1);
        assert_eq!(compiled.stats.static_props, 1);
        assert!(compiled.static_prop_sources.is_some());
        let props = compiled
            .bsp
            .as_ref()
            .unwrap()
            .static_props()
            .unwrap()
            .unwrap();
        assert_eq!(props.version, 6);
        assert_eq!(props.model_name(0).unwrap(), MODEL);
        assert_eq!(props.props[0].origin, [32.0, 32.0, 8.0]);
        assert_eq!(props.props[0].solid, 0);
        session.finish_compile(actor, &result);
        assert_eq!(session.compile_status()["job"]["result"]["published"], true);
        assert_eq!(
            session.compile_status()["job"]["result"]["stats"]["instances"]["static_props"],
            1
        );
        let report = refresh(&mut session, actor);
        assert_model_preview_unavailable(&mut session, actor, &report, owner);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        assert_eq!(
            std::fs::read(session.game.game_dir.join(MODEL)).unwrap(),
            model_bytes
        );
        let published = std::fs::read(&output).unwrap();
        let mut changed = model_bytes;
        changed[20] ^= 1;
        std::fs::write(session.game.game_dir.join(MODEL), changed).unwrap();
        let status = session.compile_status();
        assert_eq!(status["job"]["status"], "completed");
        assert_eq!(status["job"]["stale"], true);
        assert!(status["job"]["static_prop_source_issue"].is_string());
        assert_eq!(status["launch"]["ready"], false);
        assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
        assert_eq!(session.baked_prop_vertex_build_state()["status"], "stale");
        assert_eq!(
            session.inspect_instance_contents(0, 64).unwrap()["state"],
            "ready"
        );
        assert_eq!(std::fs::read(&output).unwrap(), published);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.compile_settings(), settings);
    }
}

#[test]
fn reference_resolution_does_not_read_missing_or_malformed_child_model_headers() {
    for missing in [false, true] {
        let (_directory, mut session, source, child, owner) = prop_session(Actor::Agent);
        let model = session.game.game_dir.join(MODEL);
        if missing {
            std::fs::remove_file(&model).unwrap();
        } else {
            std::fs::write(&model, b"not an MDL header").unwrap();
        }
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let settings = session.compile_settings();
        let child_bytes = std::fs::read(&child).unwrap();
        let report = refresh(&mut session, Actor::Agent);
        assert_model_preview_unavailable(&mut session, Actor::Agent, &report, owner);
        assert_eq!(report["stats"]["static_props"], 1);
        let result = session.begin_compile(Actor::Agent, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_none());
        let owner_label = format!(
            "entity:{}",
            match owner {
                ObjectId::Entity(id) => id,
                _ => unreachable!(),
            }
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.object.as_deref() == Some(owner_label.as_str()) })
        );
        session.finish_compile(Actor::Agent, &result);
        assert_eq!(session.compile_status()["job"]["status"], "failed");
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        assert_eq!(
            session.inspect_instance_contents(0, 64).unwrap()["state"],
            "ready"
        );
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
    }
}

#[test]
fn repeated_nested_prop_counts_precede_region_selection_and_keep_empty_preview_bounds() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, _source, child, owner) = prop_session(actor);
        std::fs::copy(&child, directory.path().join("leaf.vmf")).unwrap();
        let mut middle = Document::empty();
        middle
            .edit("repeated static prop references", |tx| {
                for x in [0.0, 64.0] {
                    edit::create_entity(
                        tx,
                        "func_instance",
                        Some(DVec3::new(x, 0.0, 0.0)),
                        &[("file".into(), "leaf.vmf".into())],
                        Vec::new(),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        std::fs::write(&child, middle.vmf().write(&Default::default())).unwrap();
        let region = map_core::compile_region::Definition {
            enabled: true,
            bounds: map_core::compile_region::Bounds {
                mins: DVec3::new(-64.0, -64.0, 0.0),
                maxs: DVec3::new(64.0, 64.0, 160.0),
            },
        };
        let stamp = session.compile_region_stamp();
        let token = session
            .preview_compile_region(actor, Some(region), &stamp)
            .unwrap()["draft"]["token"]
            .as_u64()
            .unwrap();
        session.apply_compile_region(actor, token).unwrap();
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let report = refresh(&mut session, actor);
        assert_model_preview_unavailable(&mut session, actor, &report, owner);
        assert_eq!(report["stats"]["static_props"], 2);
        assert_eq!(report["stats"]["instances"], 3);
        assert_eq!(report["stats"]["max_expansion_depth"], 2);
        assert_eq!(report["stats"]["files"], 2);
        assert_eq!(report["owners"][0]["file"], "child.vmf");
        let result = session.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert_eq!(output.stats.instances.as_ref().unwrap().static_props, 2);
        assert_eq!(output.stats.static_props, 1);
        assert_eq!(
            output
                .bsp
                .as_ref()
                .unwrap()
                .static_props()
                .unwrap()
                .unwrap()
                .props[0]
                .origin,
            [32.0, 32.0, 8.0]
        );
        session.finish_compile(actor, &result);
        assert_eq!(session.compile_status()["job"]["result"]["published"], true);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn static_props_preserve_real_terrain_preview_and_invalid_or_cancelled_refreshes_drop_it() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child, owner) = prop_session(actor);
        let mut document =
            Document::new(Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap());
        let solid = document
            .edit("child terrain cage", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.0, 0.0, 32.0),
                    DVec3::new(16.0, 16.0, 48.0),
                    "test/terrain",
                )
            })
            .unwrap();
        let ObjectId::Solid(solid) = solid else {
            unreachable!()
        };
        let side = document
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .id;
        document
            .edit("child terrain grid", |tx| {
                map_core::displacement::edit::create(tx, &[FaceId { solid, side }], 2, 0)
            })
            .unwrap();
        document
            .edit("child flat brush", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(24.0, 0.0, 32.0),
                    DVec3::new(40.0, 16.0, 48.0),
                    "test/terrain",
                )
            })
            .unwrap();
        let mut complete = document.into_vmf();
        let info = complete.world.solids[0]
            .sides
            .iter_mut()
            .find_map(|side| side.displacement.as_mut())
            .unwrap();
        info.normals = Some(vec![vec![DVec3::Z; 5]; 5]);
        let mut distances = vec![vec![0.0; 5]; 5];
        distances[2][2] = 12.0;
        info.distances = Some(distances);
        let mut without_props = complete.clone();
        without_props.entities.clear();
        std::fs::write(&child, without_props.write(&Default::default())).unwrap();
        let baseline = refresh(&mut session, actor);
        assert_eq!(baseline["state"], "ready");
        assert!(baseline.get("static_prop_preview").is_none());
        assert!(baseline["stats"].get("static_props").is_none());
        std::fs::write(&child, complete.write(&Default::default())).unwrap();
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let settings = session.compile_settings();
        let source_bytes = std::fs::read(&source).unwrap();
        let report = refresh(&mut session, actor);
        assert_eq!(report["stats"]["static_props"], 1);
        assert_eq!(report["available"], true);
        assert_eq!(report["static_prop_preview"]["available"], false);
        assert_eq!(report["geometry"]["vertices"], 49);
        assert_eq!(report["geometry"]["triangles"], 44);
        assert_eq!(report["geometry"], baseline["geometry"]);
        assert_eq!(report["face_provenance"], baseline["face_provenance"]);
        assert_eq!(
            report["owners"][0]["bounds"],
            baseline["owners"][0]["bounds"]
        );
        let hits = session.instance_contents_hits(
            &Ray {
                origin: DVec3::new(8.0, 8.0, 100.0),
                direction: DVec3::NEG_Z,
            },
            Some(true),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, owner);
        assert!((hits[0].point.z - 60.0).abs() < 1e-6);
        for (key, value) in [
            ("solid", "1"),
            ("skin", "-1"),
            ("rendercolor", "255 128 255"),
            ("targetname", "named"),
        ] {
            let mut invalid = complete.clone();
            invalid.entities[0].properties.set(key, value);
            std::fs::write(&child, invalid.write(&Default::default())).unwrap();
            let failed = refresh(&mut session, actor);
            assert_eq!(failed["state"], "failed", "{key}: {failed}");
            assert!(failed["error"].as_str().unwrap().contains(key));
            assert_eq!(failed["available"], false);
            assert!(session.instance_contents_preview(true).mesh.is_none());
        }
        let mut hidden = complete.clone();
        hidden.entities[0].hidden = true;
        hidden.entities[0].properties.set("solid", "1");
        std::fs::write(&child, hidden.write(&Default::default())).unwrap();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready");
        assert_eq!(report["geometry"], baseline["geometry"]);
        assert!(report["stats"].get("static_props").is_none());
        assert!(report.get("static_prop_preview").is_none());
        std::fs::write(&child, complete.write(&Default::default())).unwrap();
        assert_eq!(refresh(&mut session, actor)["stats"]["static_props"], 1);
        let request = session.begin_instance_contents_refresh(actor).unwrap();
        session
            .cancel_instance_contents_refresh(actor, request.token())
            .unwrap();
        let cancelled = session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(!session.instance_workers_busy());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(refresh(&mut session, actor)["stats"]["static_props"], 1);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    }
}

fn collision_session(
    actor: Actor,
    solid: Option<&str>,
) -> (tempfile::TempDir, Session, PathBuf, PathBuf, ObjectId) {
    let (directory, game, source, child) = instance_static_props::collision_fixture(solid);
    let mut session = Session::new(Ok(game));
    session.open(actor, &source, true).unwrap();
    let owner = ObjectId::Entity(session.document.vmf().entities.last().unwrap().id);
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    session.select(&[owner], SelectMode::Replace).unwrap();
    (directory, session, source, child, owner)
}

fn compile_collision_props(
    session: &mut Session,
    actor: Actor,
    background: bool,
) -> Result<crate::session::compilation::CompileResult, String> {
    let request = session.begin_compile(actor, None)?;
    if background {
        let (result, native) = request.run_admitted();
        assert!(native.is_none());
        result
    } else {
        request.run()
    }
}

#[test]
fn child_vphysics_first_read_guards_refuse_staging_and_queued_launch_drift_until_recompiled() {
    for (actor, background, solid) in [(Actor::User, false, Some("6")), (Actor::Agent, true, None)]
    {
        let (_directory, mut session, source, child, owner) = collision_session(actor, solid);
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                crate::session::PropertyChange::Set {
                    key: "message".into(),
                    value: "Retain child collision review".into(),
                },
            )
            .unwrap();
        assert_eq!(session.compile_settings()["lighting"], "fullbright");
        assert_eq!(session.compile_settings()["physics"], "none");
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let draft = json!(session.property_draft());
        let source_bytes = std::fs::read(&source).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let physics = session.game.game_dir.join(PHYSICS);
        let original = std::fs::read(&physics).unwrap();
        let mut changed = original.clone();
        changed[12] ^= 1;
        let result = compile_collision_props(&mut session, actor, background);
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert!(output.static_prop_sources.is_some());
        assert_eq!(
            output
                .bsp
                .as_ref()
                .unwrap()
                .static_props()
                .unwrap()
                .unwrap()
                .props[0]
                .solid,
            6
        );
        assert!(!result.as_ref().unwrap().published());
        std::fs::write(&physics, &changed).unwrap();
        session.finish_compile(actor, &result);
        assert!(!result.as_ref().unwrap().published());
        assert!(result.as_ref().unwrap().publication_error().is_some());
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        drop(result);
        std::fs::write(&physics, &original).unwrap();
        let result = compile_collision_props(&mut session, actor, background);
        session.finish_compile(actor, &result);
        assert!(
            result.as_ref().unwrap().published(),
            "{}",
            session.compile_status()
        );
        drop(result);
        let published = std::fs::read(source.with_extension("bsp")).unwrap();
        let pointfile = std::fs::read(source.with_extension("lin")).ok();
        let retained = session
            .compiled
            .as_ref()
            .unwrap()
            .static_prop_sources
            .as_ref()
            .unwrap()
            .clone();
        let launch = session.begin_launch(actor).unwrap();
        let origin = launch.origin();
        launch.verify_static_prop_sources().unwrap();
        std::fs::write(&physics, &changed).unwrap();
        assert!(retained.verify(&compiler::Control::default()).is_err());
        assert!(launch.verify_static_prop_sources().is_err());
        assert!(
            session
                .finish_launch(
                    actor,
                    &origin,
                    Err("test completed without spawning".into())
                )
                .is_err()
        );
        drop(launch);
        let status = session.compile_status();
        assert_eq!(status["job"]["status"], "completed");
        assert_eq!(status["job"]["stale"], true);
        assert!(status["job"]["static_prop_source_issue"].is_string());
        assert_eq!(status["launch"]["ready"], false);
        assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
        assert_eq!(session.baked_prop_vertex_build_state()["status"], "stale");
        std::fs::write(&physics, &original).unwrap();
        assert!(retained.verify(&compiler::Control::default()).is_err());
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert!(session.launch_readiness().is_err());
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            published
        );
        assert_eq!(std::fs::read(source.with_extension("lin")).ok(), pointfile);
        let report = refresh(&mut session, actor);
        assert_model_preview_unavailable(&mut session, actor, &report, owner);
        assert_eq!(report["stats"]["static_props"], 1);
        let result = compile_collision_props(&mut session, actor, !background);
        session.finish_compile(actor, &result);
        assert!(
            result.as_ref().unwrap().published(),
            "{}",
            session.compile_status()
        );
        drop(result);
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert!(session.launch_readiness().is_ok());
        assert!(retained.verify(&compiler::Control::default()).is_err());
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
    }
}

#[test]
fn child_collision_unused_phy_and_region_omissions_preserve_ready_original_builds() {
    for (actor, background, solid, omitted) in [
        (Actor::User, false, Some("0"), false),
        (Actor::Agent, true, Some("2"), false),
        (Actor::User, true, Some("6"), true),
        (Actor::Agent, false, None, true),
    ] {
        let (_directory, mut session, source, child, owner) = collision_session(actor, solid);
        let physics = session.game.game_dir.join(PHYSICS);
        std::fs::write(&physics, b"unused invalid PHY").unwrap();
        if omitted {
            let mut vmf = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
            vmf.entities[0].properties.set("origin", "96 32 8");
            std::fs::write(&child, vmf.write(&Default::default())).unwrap();
            std::fs::remove_file(session.game.game_dir.join(MODEL)).unwrap();
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
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let settings = session.compile_settings();
        let source_bytes = std::fs::read(&source).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let report = refresh(&mut session, actor);
        assert_model_preview_unavailable(&mut session, actor, &report, owner);
        assert_eq!(report["stats"]["static_props"], 1);
        let result = compile_collision_props(&mut session, actor, background);
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert_eq!(output.stats.instances.as_ref().unwrap().static_props, 1);
        assert_eq!(output.stats.static_props, usize::from(!omitted));
        if omitted {
            assert!(output.static_prop_sources.is_none());
        } else {
            assert_eq!(
                output
                    .bsp
                    .as_ref()
                    .unwrap()
                    .static_props()
                    .unwrap()
                    .unwrap()
                    .props[0]
                    .solid,
                solid.unwrap().parse::<u8>().unwrap()
            );
        }
        session.finish_compile(actor, &result);
        assert!(
            result.as_ref().unwrap().published(),
            "{}",
            session.compile_status()
        );
        drop(result);
        let published = std::fs::read(source.with_extension("bsp")).unwrap();
        std::fs::write(&physics, b"changed but still unused PHY").unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert!(session.compile_status()["job"]["static_prop_source_issue"].is_null());
        assert!(session.launch_readiness().is_ok());
        assert_eq!(session.baked_lightmap_build_state()["status"], "current");
        assert_eq!(session.baked_prop_vertex_build_state()["status"], "current");
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            published
        );
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
    }
}
