use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const CSS_VISCLUSTER_FGD: &str = "@SolidClass color(180 180 0) = func_viscluster []";

struct Authored {
    _directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    child: PathBuf,
    owner: ObjectId,
    helper: u32,
}

fn authored(actor: Actor, mixed: bool) -> Authored {
    let (directory, game) = crate::test_assets::game();
    let source = directory.path().join("parent.vmf");
    let child = directory.path().join("child.vmf");
    let definitions = crate::steam::find_css_fgd(&game)
        .map_or_else(
            || Catalog::parse(CSS_VISCLUSTER_FGD),
            |path| Catalog::load(&path),
        )
        .unwrap();
    assert!(
        definitions
            .class("func_viscluster")
            .unwrap()
            .defaults()
            .is_empty()
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
    let brush = session
        .create_block(
            actor,
            DVec3::new(-64.0, -64.0, 16.0),
            DVec3::new(64.0, 64.0, 128.0),
            "test/checker",
            false,
        )
        .unwrap();
    let ObjectId::Entity(helper) = session
        .tie_to_entity(actor, &[brush], "func_viscluster", BTreeMap::new())
        .unwrap()
    else {
        unreachable!()
    };
    let entity = session.document().entity(helper).unwrap();
    assert_eq!(
        entity.properties.iter().collect::<Vec<_>>(),
        vec![("classname", "func_viscluster")]
    );
    assert!(entity.editor.is_some());
    session.save(actor, Some(&child)).unwrap();
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
    }
}

fn assert_helper_only(session: &mut Session, actor: Actor, owner: ObjectId, report: &Value) {
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["stats"]["visclusters"], 1);
    assert_eq!(report["stats"]["world_brushes"], 0);
    assert_eq!(report["stats"]["detail_brushes"], 0);
    assert_eq!(report["viscluster_preview"]["available"], false);
    assert_eq!(
        report["viscluster_preview"]["reason"],
        VISCLUSTER_PREVIEW_REASON
    );
    assert_eq!(report["available"], false);
    assert_eq!(report["displayed"], false);
    assert!(report["owners"][0]["bounds"].is_null());
    assert_eq!(report["owners"][0]["world_brushes"], 0);
    assert_eq!(report["owners"][0]["detail_brushes"], 0);
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
    let preview = session.instance_contents_preview(true);
    let mesh = preview.mesh.unwrap();
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
}

fn ray() -> Ray {
    Ray {
        origin: DVec3::new(8.0, 8.0, 160.0),
        direction: DVec3::NEG_Z,
    }
}

#[test]
fn fgd_authored_visibility_helpers_share_empty_preview_and_dependency_custody_for_both_actors() {
    let mut reference = None;
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let source = std::fs::read(&fixture.source).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
        let mut authored = Vmf::parse(&String::from_utf8(child.clone()).unwrap()).unwrap();
        authored.entities[0].solids[0]
            .editor
            .as_mut()
            .unwrap()
            .remove("color");
        if let Some(reference) = &reference {
            assert_eq!(&authored, reference);
        } else {
            reference = Some(authored);
        }
        let session = &mut fixture.session;
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let report = refresh(session, actor);
        assert_helper_only(session, actor, fixture.owner, &report);
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
        assert!(
            report["source"]["path"]
                .as_str()
                .unwrap()
                .ends_with("parent.vmf")
        );
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);

        let mut changed = child.clone();
        changed.push(b'\n');
        std::fs::write(&fixture.child, changed).unwrap();
        let stale = session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale.get("viscluster_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert!(
            session
                .instance_contents_hits(&ray(), Some(true))
                .is_empty()
        );
        std::fs::write(&fixture.child, &child).unwrap();
        let restored = refresh(session, actor);
        assert_helper_only(session, actor, fixture.owner, &restored);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
    }
}

#[test]
fn visibility_helpers_preserve_mixed_reference_mesh_counts_provenance_and_owner_picking() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, true);
        let original = std::fs::read(&fixture.child).unwrap();
        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        child.entities.retain(|entity| entity.id != fixture.helper);
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let baseline = refresh(&mut fixture.session, actor);
        assert!(baseline["stats"].get("visclusters").is_none());
        assert!(baseline.get("viscluster_preview").is_none());
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
        assert_eq!(report["stats"]["visclusters"], 1);
        assert_eq!(report["stats"]["world_brushes"], 1);
        assert_eq!(report["stats"]["detail_brushes"], 1);
        assert_eq!(
            report["viscluster_preview"]["reason"],
            VISCLUSTER_PREVIEW_REASON
        );
        assert_eq!(report["geometry"], baseline["geometry"]);
        assert_eq!(report["brushes"], baseline["brushes"]);
        assert_eq!(report["face_provenance"], baseline["face_provenance"]);
        assert_eq!(report["owners"], baseline["owners"]);
        let mesh = session.instance_contents_preview(true).mesh.unwrap();
        assert_eq!(mesh.vertices, baseline_mesh.vertices);
        assert_eq!(mesh.indices, baseline_mesh.indices);
        assert_eq!(mesh.edges, baseline_mesh.edges);
        assert_eq!(mesh.brushes.len(), 2);
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
fn hidden_and_all_hidden_solid_helpers_omit_count_and_cancelled_refresh_omits_preview() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let original = std::fs::read(&fixture.child).unwrap();
        for hidden_entity in [false, true] {
            let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
            let helper = child
                .entities
                .iter_mut()
                .find(|entity| entity.id == fixture.helper)
                .unwrap();
            if hidden_entity {
                helper.hidden = true;
            } else {
                helper.solids[0].hidden = true;
            }
            std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
            let report = refresh(&mut fixture.session, actor);
            assert_eq!(report["state"], "ready", "{report}");
            assert!(report["stats"].get("visclusters").is_none());
            assert!(report.get("viscluster_preview").is_none());
            assert_eq!(report["geometry"]["triangles"], 0);
            assert_eq!(report["brushes_total"], 0);
        }
        std::fs::write(&fixture.child, &original).unwrap();
        let session = &mut fixture.session;
        assert_eq!(refresh(session, actor)["stats"]["visclusters"], 1);
        let request = session.begin_instance_contents_refresh(actor).unwrap();
        session
            .cancel_instance_contents_refresh(actor, request.token())
            .unwrap();
        let cancelled = session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(cancelled.get("viscluster_preview").is_none());
        assert!(!session.instance_workers_busy());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
    }
}
