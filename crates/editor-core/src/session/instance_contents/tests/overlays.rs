use super::*;
use std::collections::BTreeMap;

struct Authored {
    _directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    child: PathBuf,
    owner: ObjectId,
    helper: u32,
    bare_child: Vec<u8>,
}

fn top(solid: &source_formats::vmf::Solid) -> FaceId {
    FaceId {
        solid: solid.id,
        side: solid
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    }
}

fn request(source: FaceId) -> crate::session::overlay::Request {
    crate::session::overlay::Request {
        source,
        options: map_core::overlay::Options {
            material: "test/checker".into(),
            render_order: 2,
            u: [-0.25, 1.75],
            v: [0.125, 0.875],
            fade_min: 64.0,
            fade_max: 512.0,
            ..Default::default()
        },
    }
}

fn apply(session: &mut Session, actor: Actor, source: FaceId) -> u32 {
    let token = session
        .preview_overlay(actor, request(source), &session.overlay_stamp())
        .unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    let reviewed = session.review_overlay(actor, token).unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    assert_ne!(reviewed, token);
    assert!(session.apply_overlay(actor, token).is_err());
    let result = session.apply_overlay(actor, reviewed).unwrap();
    let ObjectId::Entity(id) =
        crate::identity::parse_id(result["created"].as_str().unwrap()).unwrap()
    else {
        unreachable!()
    };
    id
}

fn authored(actor: Actor, detail: bool) -> Authored {
    let (directory, game) = crate::test_assets::game();
    let source = directory.path().join("parent.vmf");
    let child = directory.path().join("child.vmf");
    let mut session = Session::new(Ok(game));
    let world = session
        .create_block(
            actor,
            DVec3::new(0.0, 0.0, 32.0),
            DVec3::new(16.0, 16.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    let scenery = session
        .create_block(
            actor,
            DVec3::new(24.0, 0.0, 32.0),
            DVec3::new(40.0, 16.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .tie_to_entity(actor, &[scenery], "func_detail", BTreeMap::new())
        .unwrap();
    let ObjectId::Solid(id) = (if detail { scenery } else { world }) else {
        unreachable!()
    };
    let face = top(map_core::surface::solid(session.document(), id).unwrap());
    session.save(actor, Some(&child)).unwrap();
    let bare_child = std::fs::read(&child).unwrap();
    let helper = apply(&mut session, actor, face);
    let overlay = session.document().entity(helper).unwrap();
    assert_eq!(overlay.classname(), Some("info_overlay"));
    assert!(overlay.properties.get("targetname").is_none());
    assert_eq!(
        overlay.properties.get("sides"),
        Some(face.side.to_string().as_str())
    );
    assert_eq!(overlay.properties.get("RenderOrder"), Some("2"));
    assert!(overlay.properties.get("BasisOrigin").is_some());
    session.save(actor, None).unwrap();
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
            Some(DVec3::ZERO),
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
    Authored {
        _directory: directory,
        session,
        source,
        child,
        owner,
        helper,
        bare_child,
    }
}

#[test]
fn session_authored_child_overlays_share_counts_and_projection_limits_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for detail in [false, true] {
            let mut fixture = authored(actor, detail);
            let original = std::fs::read(&fixture.child).unwrap();
            let parent = std::fs::read(&fixture.source).unwrap();
            let session = &mut fixture.session;
            let memory = session.document().vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let settings = session.compile_settings();
            std::fs::write(&fixture.child, &fixture.bare_child).unwrap();
            let baseline = refresh(session, actor);
            assert!(baseline["stats"].get("overlays").is_none());
            assert!(baseline.get("overlay_preview").is_none());
            let baseline_mesh = session.instance_contents_preview(true).mesh.unwrap();
            session
                .frame_instance_contents(
                    actor,
                    baseline["token"].as_u64().unwrap(),
                    &[fixture.owner],
                    1.5,
                )
                .unwrap();
            let framed = session.camera();
            session.set_camera(camera);
            std::fs::write(&fixture.child, &original).unwrap();
            let report = refresh(session, actor);
            assert_eq!(report["state"], "ready", "{report}");
            assert_eq!(report["stats"]["overlays"], 1);
            assert_eq!(report["overlay_preview"]["available"], false);
            assert_eq!(report["overlay_preview"]["reason"], OVERLAY_PREVIEW_REASON);
            assert_eq!(report["available"], true);
            assert_eq!(report["displayed"], true);
            assert_eq!(report["stats"]["world_brushes"], 1);
            assert_eq!(report["stats"]["detail_brushes"], 1);
            assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
            assert_eq!(report["stats"]["dependencies"][0]["bytes"], original.len());
            for field in [
                "geometry",
                "brushes",
                "face_provenance",
                "owners",
                "available",
                "displayed",
            ] {
                assert_eq!(report[field], baseline[field], "{field}");
            }
            let mesh = session.instance_contents_preview(true).mesh.unwrap();
            assert_eq!(mesh.vertices, baseline_mesh.vertices);
            assert_eq!(mesh.indices, baseline_mesh.indices);
            assert_eq!(mesh.edges, baseline_mesh.edges);
            assert_eq!(mesh.brushes.len(), 2);
            assert_eq!(
                report["owners"][0]["bounds"],
                json!([[0.0, 0.0, 32.0], [40.0, 16.0, 48.0]])
            );
            let x = if detail { 32.0 } else { 8.0 };
            let hits = session.instance_contents_hits(
                &Ray {
                    origin: DVec3::new(x, 8.0, 160.0),
                    direction: DVec3::NEG_Z,
                },
                Some(true),
            );
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].object, fixture.owner);
            assert_eq!(hits[0].point, DVec3::new(x, 8.0, 48.0));
            assert_eq!(session.camera(), camera);
            session
                .frame_instance_contents(
                    actor,
                    report["token"].as_u64().unwrap(),
                    &[fixture.owner],
                    1.5,
                )
                .unwrap();
            assert_eq!(session.camera(), framed);
            assert_eq!(session.document().vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.compile_settings(), settings);
            assert_eq!(session.selected_ids(), vec![fixture.owner]);
            assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
            assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
            assert_eq!(
                std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(fixture.source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
        }
    }
}

#[test]
fn child_overlay_zero_stale_cancel_and_refusal_states_preserve_physical_ownership() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let original = std::fs::read(&fixture.child).unwrap();
        let parent = std::fs::read(&fixture.source).unwrap();
        let memory = fixture.session.document().vmf().clone();
        let history = fixture.session.history_snapshot();
        let camera = fixture.session.camera();
        let settings = fixture.session.compile_settings();
        let baseline = refresh(&mut fixture.session, actor);
        let mut hidden = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        hidden
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap()
            .hidden = true;
        std::fs::write(&fixture.child, hidden.write(&Default::default())).unwrap();
        let report = refresh(&mut fixture.session, actor);
        assert_eq!(report["state"], "ready");
        assert!(report["stats"].get("overlays").is_none());
        assert!(report.get("overlay_preview").is_none());
        assert_eq!(report["geometry"], baseline["geometry"]);
        assert_eq!(report["owners"], baseline["owners"]);
        std::fs::write(&fixture.child, &original).unwrap();
        assert_eq!(refresh(&mut fixture.session, actor)["stats"]["overlays"], 1);
        std::fs::write(&fixture.child, [original.as_slice(), b"\n"].concat()).unwrap();
        let stale = fixture.session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale.get("overlay_preview").is_none());
        assert!(
            fixture
                .session
                .instance_contents_preview(true)
                .mesh
                .is_none()
        );
        std::fs::write(&fixture.child, &original).unwrap();
        assert_eq!(refresh(&mut fixture.session, actor)["stats"]["overlays"], 1);
        let request = fixture
            .session
            .begin_instance_contents_refresh(actor)
            .unwrap();
        fixture
            .session
            .cancel_instance_contents_refresh(actor, request.token())
            .unwrap();
        let cancelled = fixture
            .session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(cancelled.get("overlay_preview").is_none());
        assert!(!fixture.session.instance_workers_busy());
        let mut invalid = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        invalid
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap()
            .properties
            .set("sides", "999999");
        std::fs::write(&fixture.child, invalid.write(&Default::default())).unwrap();
        let failed = refresh(&mut fixture.session, actor);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(failed.get("overlay_preview").is_none());
        let error = failed["error"].as_str().unwrap();
        assert!(
            error.contains(&format!("Child overlay {}", fixture.helper)),
            "{error}"
        );
        assert!(error.contains("child.vmf"), "{error}");
        assert!(error.contains("same child"), "{error}");
        assert!(
            fixture
                .session
                .instance_contents_preview(true)
                .mesh
                .is_none()
        );
        std::fs::write(&fixture.child, &original).unwrap();
        assert_eq!(refresh(&mut fixture.session, actor)["stats"]["overlays"], 1);
        assert_eq!(fixture.session.document().vmf(), &memory);
        assert_eq!(fixture.session.history_snapshot(), history);
        assert_eq!(fixture.session.camera(), camera);
        assert_eq!(fixture.session.compile_settings(), settings);
        assert_eq!(fixture.session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
        assert_eq!(
            std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(fixture.source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

#[test]
fn child_overlay_refresh_and_compile_preserve_owned_root_overlay_drafts() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let parent = std::fs::read(&fixture.source).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
        let session = &mut fixture.session;
        let root = top(&session.document().vmf().world.solids[0]);
        apply(session, actor, root);
        session
            .select(&[fixture.owner], SelectMode::Replace)
            .unwrap();
        let token = session
            .preview_overlay(actor, request(root), &session.overlay_stamp())
            .unwrap()["draft"]["token"]
            .as_u64()
            .unwrap();
        let draft = session.get_overlay_state();
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["overlays"], 1);
        assert_eq!(report["available"], true);
        assert!(report["suppression_reason"].is_null());
        assert_eq!(session.get_overlay_state(), draft);
        let result = session.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert_eq!(output.stats.instances.as_ref().unwrap().overlays, 1);
        assert_eq!(output.stats.overlays, 2);
        assert!(output.stats.overlay_face_refs >= 2);
        session.finish_compile(actor, &result);
        assert_eq!(session.get_overlay_state(), draft);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
        let reviewer = if actor == Actor::User {
            Actor::Agent
        } else {
            Actor::User
        };
        let reviewed = session.review_overlay(reviewer, token).unwrap()["draft"]["token"]
            .as_u64()
            .unwrap();
        assert!(session.cancel_overlay(actor, token).is_err());
        session.cancel_overlay(reviewer, reviewed).unwrap();
    }
}
