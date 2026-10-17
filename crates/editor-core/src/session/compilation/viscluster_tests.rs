use super::*;
use source_formats::fgd::Catalog;

fn authored_session(actor: Actor) -> (tempfile::TempDir, Session, u32) {
    let (directory, game) = crate::test_assets::game();
    let path = directory.path().join("viscluster.vmf");
    let mut document = map_core::Document::new(crate::test_assets::room());
    document
        .edit("doorway", |tx| {
            for (mins, maxs) in [
                ([-8.0, -256.0, 0.0], [8.0, -64.0, 256.0]),
                ([-8.0, 64.0, 0.0], [8.0, 256.0, 256.0]),
            ] {
                map_core::edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "test/checker",
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&path, document.vmf().write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(actor, &path, true).unwrap();
    session.set_definitions(
        actor,
        Catalog::parse("@SolidClass color(180 180 0) = func_viscluster []").unwrap(),
    );
    assert_eq!(
        session
            .search_entity_classes(
                "func_viscluster",
                Some(source_formats::fgd::ClassKind::Solid),
                10
            )
            .len(),
        1
    );
    let brush = session
        .create_block(
            actor,
            DVec3::new(-250.0, -250.0, 8.0),
            DVec3::new(250.0, 250.0, 248.0),
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
    (directory, session, helper)
}

#[test]
fn actor_compile_snapshots_share_authored_grouping_without_saving_or_editing_history() {
    let mut reference = None;
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, helper) = authored_session(actor);
        let path = directory.path().join("viscluster.vmf");
        let saved = std::fs::read(&path).unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        let result = result.unwrap();
        assert!(result.output.bsp.is_some(), "{:?}", result.output.report);
        assert_eq!(result.output.stats.viscluster_groups, 1);
        assert!(result.output.stats.clusters < result.output.stats.visibility_solved_clusters);
        let bytes = std::fs::read(path.with_extension("bsp")).unwrap();
        if let Some(reference) = &reference {
            assert_eq!(&bytes, reference);
        } else {
            reference = Some(bytes);
        }
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(std::fs::read(&path).unwrap(), saved);

        let request = session.begin_compile(actor, None).unwrap();
        session
            .delete_objects(actor, &[ObjectId::Entity(helper)])
            .unwrap();
        let edited_source = session.document().vmf().clone();
        let edited_history = session.history_snapshot();
        let result = request.run();
        session.finish_compile(actor, &result);
        assert_eq!(result.unwrap().output.stats.viscluster_groups, 1);
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert_eq!(session.document().vmf(), &edited_source);
        assert_eq!(session.history_snapshot(), edited_history);
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        assert_eq!(result.unwrap().output.stats.viscluster_helpers, 0);
        assert_eq!(session.compile_status()["job"]["stale"], false);
    }
}
