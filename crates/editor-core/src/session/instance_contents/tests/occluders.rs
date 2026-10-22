use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const OCCLUDER_FGD: &str = r#"
@SolidClass = func_occluder [
    StartActive(choices) : "Start Active" : 1 = [ 0 : "No" 1 : "Yes" ]
]
"#;

struct Authored {
    _directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    child: PathBuf,
    owner: ObjectId,
    helper: u32,
}

fn authored(actor: Actor, mixed: bool) -> Authored {
    let (directory, game, source, child) = crate::test_assets::instance_map(false);
    if !mixed {
        std::fs::write(&child, Document::empty().vmf().write(&Default::default())).unwrap();
    }
    let definitions = crate::steam::find_css_fgd(&game)
        .map_or_else(|| Catalog::parse(OCCLUDER_FGD), |path| Catalog::load(&path))
        .unwrap();
    assert_eq!(
        definitions.class("func_occluder").unwrap().defaults()["StartActive"],
        "1"
    );
    let mut session = Session::new(Ok(game));
    session.set_definitions(actor, definitions);
    session.open(actor, &child, true).unwrap();
    let mut brushes = Vec::new();
    for x in [-64.0, 48.0] {
        brushes.push(
            session
                .create_block(
                    actor,
                    DVec3::new(x, -32.0, 16.0),
                    DVec3::new(x + 16.0, 32.0, 112.0),
                    "test/checker",
                    false,
                )
                .unwrap(),
        );
    }
    let ObjectId::Entity(helper) = session
        .tie_to_entity(actor, &brushes, "func_occluder", BTreeMap::new())
        .unwrap()
    else {
        unreachable!()
    };
    let entity = session.document().entity(helper).unwrap();
    assert_eq!(entity.properties.get("StartActive"), Some("1"));
    assert!(
        entity
            .properties
            .iter()
            .any(|(key, value)| key == "StartActive" && value == "1")
    );
    assert!(
        !entity
            .properties
            .iter()
            .any(|(key, _)| key == "startactive")
    );
    assert!(entity.properties.get("origin").is_none());
    assert!(entity.properties.get("angles").is_none());
    assert_eq!(entity.solids.len(), 2);
    session.save(actor, None).unwrap();
    session.open(actor, &source, false).unwrap();
    let owner = ObjectId::Entity(session.document().vmf().entities.last().unwrap().id);
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

fn ray() -> Ray {
    Ray {
        origin: DVec3::new(8.0, 8.0, 160.0),
        direction: DVec3::NEG_Z,
    }
}

#[test]
fn fgd_authored_occluders_have_counts_but_no_reference_geometry_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let child = std::fs::read(&fixture.child).unwrap();
        let source = std::fs::read(&fixture.source).unwrap();
        let session = &mut fixture.session;
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["occluders"], 1);
        assert_eq!(report["stats"]["world_brushes"], 0);
        assert_eq!(report["stats"]["detail_brushes"], 0);
        assert_eq!(report["occluder_preview"]["available"], false);
        assert_eq!(
            report["occluder_preview"]["reason"],
            OCCLUDER_PREVIEW_REASON
        );
        assert_eq!(report["available"], false);
        assert_eq!(report["displayed"], false);
        assert!(report["owners"][0]["bounds"].is_null());
        assert_eq!(report["brushes_total"], 0);
        assert_eq!(report["face_provenance_total"], 0);
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
        assert_eq!(report["stats"]["dependencies"][0]["bytes"], child.len());
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
        assert!(
            session
                .frame_instance_contents(
                    actor,
                    report["token"].as_u64().unwrap(),
                    &[fixture.owner],
                    1.0
                )
                .unwrap_err()
                .contains("no visible geometry")
        );
        assert_eq!(session.camera(), camera);
        std::fs::write(&fixture.child, [child.as_slice(), b"\n"].concat()).unwrap();
        let stale = session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale.get("occluder_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        std::fs::write(&fixture.child, &child).unwrap();
        assert_eq!(refresh(session, actor)["stats"]["occluders"], 1);
        let request = session.begin_instance_contents_refresh(actor).unwrap();
        session
            .cancel_instance_contents_refresh(actor, request.token())
            .unwrap();
        let cancelled = session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(cancelled.get("occluder_preview").is_none());
        assert!(!session.instance_workers_busy());
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
    }
}

#[test]
fn mixed_occluders_preserve_exact_physical_reference_mesh_and_owner_picking() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, true);
        let original = std::fs::read(&fixture.child).unwrap();
        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        child.entities.retain(|entity| entity.id != fixture.helper);
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let baseline = refresh(&mut fixture.session, actor);
        assert!(baseline["stats"].get("occluders").is_none());
        assert!(baseline.get("occluder_preview").is_none());
        let baseline_mesh = fixture
            .session
            .instance_contents_preview(true)
            .mesh
            .unwrap();
        std::fs::write(&fixture.child, &original).unwrap();
        let report = refresh(&mut fixture.session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["occluders"], 1);
        assert_eq!(report["stats"]["world_brushes"], 1);
        assert_eq!(report["stats"]["detail_brushes"], 1);
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
        let mesh = fixture
            .session
            .instance_contents_preview(true)
            .mesh
            .unwrap();
        assert_eq!(mesh.vertices, baseline_mesh.vertices);
        assert_eq!(mesh.indices, baseline_mesh.indices);
        assert_eq!(mesh.edges, baseline_mesh.edges);
        assert_eq!(mesh.brushes.len(), 2);
        let hits = fixture.session.instance_contents_hits(&ray(), Some(true));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, fixture.owner);
        assert_eq!(hits[0].point, DVec3::new(8.0, 8.0, 48.0));
        assert_eq!(std::fs::read(&fixture.child).unwrap(), original);
    }
}

#[test]
fn hidden_occluders_omit_zero_while_empty_helpers_fail_without_a_preview() {
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
                for brush in &mut helper.solids {
                    brush.hidden = true;
                }
            }
            std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
            let report = refresh(&mut fixture.session, actor);
            assert_eq!(report["state"], "ready", "{report}");
            assert!(report["stats"].get("occluders").is_none());
            assert!(report.get("occluder_preview").is_none());
            assert_eq!(report["geometry"]["triangles"], 0);
        }
        let mut child = Vmf::parse(&String::from_utf8(original.clone()).unwrap()).unwrap();
        child
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap()
            .solids
            .clear();
        std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
        let failed = refresh(&mut fixture.session, actor);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(failed.get("occluder_preview").is_none());
        assert!(
            fixture
                .session
                .instance_contents_preview(true)
                .mesh
                .is_none()
        );
        std::fs::write(&fixture.child, &original).unwrap();
        assert_eq!(
            refresh(&mut fixture.session, actor)["stats"]["occluders"],
            1
        );
    }
}
