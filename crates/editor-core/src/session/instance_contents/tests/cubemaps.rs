use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const CSS_CUBEMAP_FGD: &str = r#"
@PointClass color(0 0 255) sidelist(sides) iconsprite("editor/env_cubemap.vmt") = env_cubemap [
    cubemapsize(choices) : "Cubemap Size" : 0 = [
        0 : "Default" 1 : "1x1" 2 : "2x2" 3 : "4x4" 4 : "8x8"
        5 : "16x16" 6 : "32x32" 7 : "64x64" 8 : "128x128" 9 : "256x256"
    ]
    sides(sidelist) : "Brush faces"
]
"#;

struct Authored {
    _directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    child: PathBuf,
    owner: ObjectId,
    samples: [u32; 2],
}

fn authored(actor: Actor, mixed: bool) -> Authored {
    let (directory, game) = crate::test_assets::game();
    let source = directory.path().join("parent.vmf");
    let child = directory.path().join("child.vmf");
    let tools = game.game_dir.join("materials/tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("toolsblack.vmt"), "LightmappedGeneric {}").unwrap();
    let definitions = crate::steam::find_css_fgd(&game)
        .map_or_else(
            || Catalog::parse(CSS_CUBEMAP_FGD),
            |path| Catalog::load(&path),
        )
        .unwrap();
    assert_eq!(
        definitions.class("env_cubemap").unwrap().defaults(),
        BTreeMap::from([("cubemapsize".into(), "0".into())])
    );
    let mut session = Session::new(Ok(game));
    session.set_definitions(actor, definitions);
    if mixed {
        session
            .create_block(
                actor,
                DVec3::new(0.0, 0.0, 32.0),
                DVec3::new(16.0, 16.0, 48.0),
                "test/checker",
                false,
            )
            .unwrap();
        let detail = session
            .create_block(
                actor,
                DVec3::new(24.0, 0.0, 32.0),
                DVec3::new(40.0, 16.0, 48.0),
                "test/checker",
                false,
            )
            .unwrap();
        session
            .tie_to_entity(actor, &[detail], "func_detail", BTreeMap::new())
            .unwrap();
    }
    let mut samples = [0; 2];
    for (id, x) in samples.iter_mut().zip([16.0, 192.0]) {
        let ObjectId::Entity(created) = session
            .create_entity(
                actor,
                "env_cubemap",
                Some(DVec3::new(x, 8.0, 64.0)),
                BTreeMap::new(),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let entity = session.document().entity(created).unwrap();
        assert_eq!(entity.properties.get("cubemapsize"), Some("0"));
        assert!(entity.properties.get("sides").is_none());
        assert!(entity.properties.get("angles").is_none());
        assert!(entity.properties.get("angle").is_none());
        assert_eq!(entity.properties.iter().count(), 3);
        *id = created;
    }
    session.save(actor, Some(&child)).unwrap();
    let mut parent = crate::test_assets::room();
    for solid in &mut parent.world.solids {
        for side in &mut solid.sides {
            side.material = "test/checker".into();
        }
    }
    std::fs::write(&source, parent.write(&Default::default())).unwrap();
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
        samples,
    }
}

fn ray() -> Ray {
    Ray {
        origin: DVec3::new(8.0, 8.0, 160.0),
        direction: DVec3::NEG_Z,
    }
}

fn assert_sample_only(session: &mut Session, actor: Actor, owner: ObjectId, report: &Value) {
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["stats"]["cubemaps"], 2);
    assert_eq!(report["stats"]["world_brushes"], 0);
    assert_eq!(report["stats"]["detail_brushes"], 0);
    assert_eq!(report["cubemap_preview"]["available"], false);
    assert_eq!(report["cubemap_preview"]["reason"], CUBEMAP_PREVIEW_REASON);
    assert_eq!(report["available"], false);
    assert_eq!(report["displayed"], false);
    assert!(report["owners"][0]["bounds"].is_null());
    assert_eq!(report["brushes_total"], 0);
    assert_eq!(report["face_provenance_total"], 0);
    for field in [
        "vertices",
        "indices",
        "line_vertices",
        "triangles",
        "gpu_bytes",
    ] {
        assert_eq!(report["geometry"][field], 0);
    }
    let mesh = session.instance_contents_preview(true).mesh.unwrap();
    assert!(mesh.vertices.is_empty());
    assert!(mesh.indices.is_empty());
    assert!(mesh.edges.is_empty());
    assert!(mesh.brushes.is_empty());
    assert!(
        session
            .instance_contents_hits(&ray(), Some(true))
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
    let bindings = session
        .get_cubemap_bindings(crate::session::cubemap_bindings::Query::default())
        .unwrap();
    assert_eq!(bindings["available"], false);
    assert_eq!(bindings["assignments"], json!([]));
}

#[test]
fn fgd_authored_cubemap_samples_share_counts_empty_preview_and_source_custody_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let source = std::fs::read(&fixture.source).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
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
                    value: "Keep the cubemap sample review".into(),
                },
            )
            .unwrap();
        let state = (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.camera(),
            session.compile_settings(),
            json!(session.property_draft()),
            session.selected_ids(),
            session.view(),
        );
        let report = refresh(session, actor);
        assert_sample_only(session, actor, fixture.owner, &report);
        assert_eq!(report["owners"][0]["file"], "child.vmf");
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
        assert_eq!(report["stats"]["dependencies"][0]["bytes"], child.len());
        assert_eq!(
            report["stats"]["dependencies"][0]["sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        let mut changed = child.clone();
        changed.push(b'\n');
        std::fs::write(&fixture.child, changed).unwrap();
        let stale = session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale.get("cubemap_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        std::fs::write(&fixture.child, &child).unwrap();
        let restored = refresh(session, actor);
        assert_sample_only(session, actor, fixture.owner, &restored);
        assert_eq!(
            (
                session.document().vmf().clone(),
                session.history_snapshot(),
                session.camera(),
                session.compile_settings(),
                json!(session.property_draft()),
                session.selected_ids(),
                session.view(),
            ),
            state
        );
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
    }
}

#[test]
fn cubemap_samples_preserve_actual_mixed_mesh_provenance_counts_and_owner_picking() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, true);
        let original = std::fs::read(&fixture.child).unwrap();
        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        child
            .entities
            .retain(|entity| !fixture.samples.contains(&entity.id));
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let baseline = refresh(&mut fixture.session, actor);
        assert!(baseline["stats"].get("cubemaps").is_none());
        assert!(baseline.get("cubemap_preview").is_none());
        let baseline_mesh = fixture
            .session
            .instance_contents_preview(true)
            .mesh
            .unwrap();
        std::fs::write(&fixture.child, &original).unwrap();
        let session = &mut fixture.session;
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let source = std::fs::read(&fixture.source).unwrap();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["cubemaps"], 2);
        assert_eq!(report["stats"]["world_brushes"], 1);
        assert_eq!(report["stats"]["detail_brushes"], 1);
        assert_eq!(report["cubemap_preview"]["reason"], CUBEMAP_PREVIEW_REASON);
        assert_eq!(report["geometry"], baseline["geometry"]);
        assert_eq!(report["brushes"], baseline["brushes"]);
        assert_eq!(report["face_provenance"], baseline["face_provenance"]);
        assert_eq!(report["owners"], baseline["owners"]);
        let mesh = session.instance_contents_preview(true).mesh.unwrap();
        assert_eq!(mesh.vertices, baseline_mesh.vertices);
        assert_eq!(mesh.indices, baseline_mesh.indices);
        assert_eq!(mesh.edges, baseline_mesh.edges);
        let hits = session.instance_contents_hits(&ray(), Some(true));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, fixture.owner);
        assert_eq!(hits[0].point, DVec3::new(8.0, 8.0, 48.0));
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
    }
}

#[test]
fn cubemap_counts_precede_regions_and_failed_or_cancelled_refresh_clears_preview() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let original = std::fs::read(&fixture.child).unwrap();
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
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let source = std::fs::read(&fixture.source).unwrap();
        let report = refresh(session, actor);
        assert_sample_only(session, actor, fixture.owner, &report);
        let result = session.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 2);
        assert_eq!(output.stats.cubemap_samples, 1);
        session.finish_compile(actor, &result);

        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        let large = child
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.samples[0])
            .unwrap();
        large.properties.set("cubemapsize", "9");
        large.properties.set("sides", " \t ");
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let report = refresh(session, actor);
        assert_sample_only(session, actor, fixture.owner, &report);

        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        let hidden = child
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.samples[1])
            .unwrap();
        hidden.hidden = true;
        hidden.properties.set("origin", "NaN 0 0");
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        assert_eq!(refresh(session, actor)["stats"]["cubemaps"], 1);
        std::fs::write(&fixture.child, &original).unwrap();
        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        child
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.samples[0])
            .unwrap()
            .properties
            .set("sides", "17");
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let failed = refresh(session, actor);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(failed.get("cubemap_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        std::fs::write(&fixture.child, &original).unwrap();
        let request = session.begin_instance_contents_refresh(actor).unwrap();
        session
            .cancel_instance_contents_refresh(actor, request.token())
            .unwrap();
        let cancelled = session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(cancelled.get("cubemap_preview").is_none());
        assert!(!session.instance_workers_busy());
        assert!(
            session
                .instance_contents_hits(&ray(), Some(true))
                .is_empty()
        );
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
    }
}
