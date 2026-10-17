use super::*;

fn fixture() -> (tempfile::TempDir, Session, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("definitions.fgd");
    std::fs::write(&path, "@PointClass = loaded []").unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse("@PointClass = original []").unwrap(),
    );
    (directory, session, path)
}

#[test]
fn definitions_load_keeps_failed_reports_and_accepts_ordinary_map_edits() {
    let (_directory, mut session, path) = fixture();
    let before = session.entity_definitions_report();
    let serial = session.definitions_serial();
    std::fs::write(&path, "@PointClass = broken [").unwrap();
    let request = session.begin_definitions_load(&path).unwrap();
    assert!(session.begin_definitions_load(&path).is_err());
    assert!(
        session
            .finish_definitions_load(Actor::Agent, request.run())
            .is_err()
    );
    assert_eq!(session.entity_definitions_report(), before);
    assert_eq!(session.definitions_serial(), serial);
    assert!(!session.definitions_load_busy());
    std::fs::write(&path, "@PointClass = loaded []").unwrap();
    let request = session.begin_definitions_load(&path).unwrap();
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "no/assets",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    session
        .finish_definitions_load(Actor::Agent, request.run())
        .unwrap();
    assert!(session.definitions().class("loaded").is_some());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn result_tickets_retain_work_until_cleanup_and_reject_foreign_or_newer_catalogs() {
    let (_directory, mut origin, path) = fixture();
    let (_other_directory, mut peer, peer_path) = fixture();
    let completed = origin.begin_definitions_load(&path).unwrap().run();
    let peer_request = peer.begin_definitions_load(&peer_path).unwrap();
    assert_eq!(completed.token, peer_request.token());
    let peer_before = peer.entity_definitions_report();
    assert!(origin.definitions_load_busy());
    assert!(origin.begin_definitions_load(&path).is_err());
    assert!(
        peer.finish_definitions_load(Actor::User, completed)
            .is_err()
    );
    assert_eq!(peer.entity_definitions_report(), peer_before);
    assert!(!origin.definitions_load_busy());
    assert!(peer.definitions_load_busy());
    peer.finish_definitions_load(Actor::Agent, peer_request.run())
        .unwrap();
    let completed = origin.begin_definitions_load(&path).unwrap().run();
    origin.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass = newer []").unwrap(),
    );
    let newer = origin.entity_definitions_report();
    let serial = origin.definitions_serial();
    assert!(
        origin
            .finish_definitions_load(Actor::User, completed)
            .is_err()
    );
    assert_eq!(origin.entity_definitions_report(), newer);
    assert_eq!(origin.definitions_serial(), serial);
    assert!(!origin.definitions_load_busy());
}

#[test]
fn shutdown_cancels_before_open_and_rejects_finished_catalog_adoption() {
    let (_directory, mut session, path) = fixture();
    let before = session.entity_definitions_report();
    let serial = session.definitions_serial();
    let request = session.begin_definitions_load(&path).unwrap();
    session.shutdown();
    assert!(session.begin_definitions_load(&path).is_err());
    let completed = request.run();
    assert!(completed.result.as_ref().unwrap_err().contains("cancelled"));
    assert!(session.definitions_load_busy());
    assert!(
        session
            .finish_definitions_load(Actor::User, completed)
            .is_err()
    );
    assert!(!session.definitions_load_busy());
    assert_eq!(session.entity_definitions_report(), before);
    assert_eq!(session.definitions_serial(), serial);
}
