use super::*;
use crate::session::{Actor, Session};

#[test]
fn document_shutdown_waits_for_instance_cleanup_before_final_capture_and_leaves_peer_live() {
    let (directory, game, parent, selected) = crate::test_assets::instance_extraction_map();
    let source = std::fs::read(&parent).unwrap();
    let store = recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let mut state = Session::new(Ok(game));
    state.open(Actor::User, &parent, true).unwrap();
    state
        .select(&selected, crate::session::SelectMode::Replace)
        .unwrap();
    state.configure_recovery(Ok(store.directory.clone()));
    let contents = state.begin_instance_contents_refresh(Actor::Agent).unwrap();
    let extraction = state
        .begin_instance_extraction(Actor::User, "drained.vmf")
        .unwrap()
        .run();
    let stages = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hammerr-extract-")
        })
        .collect::<Vec<_>>();
    assert_eq!(stages.len(), 1);
    state
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(256.0),
            glam::DVec3::splat(320.0),
            "newer",
            false,
        )
        .unwrap();
    let expected = state.document().vmf().clone();
    let session = Arc::new(Mutex::new(state));
    let document = DocumentHandle::start(session.clone()).unwrap();
    let draining = document.clone();
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let result = draining.shutdown();
        done_tx.send(result).unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let error = match lock(&session).begin_instance_contents_refresh(Actor::User) {
            Ok(_) => panic!("overlapping contents worker was admitted"),
            Err(error) => error,
        };
        if error == "The editor is closing" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "shutdown never cancelled instance work"
        );
        std::thread::yield_now();
    }
    assert!(matches!(
        done_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(store.list().unwrap().entries.is_empty());
    assert!(stages[0].exists());
    let mut peer = Session::new(Err("no assets".into()));
    let peer_request = peer.begin_instance_contents_refresh(Actor::User).unwrap();
    peer.finish_instance_contents_refresh(Actor::User, peer_request.run())
        .unwrap();
    assert!(!peer.instance_workers_busy());
    let contents = contents.run();
    assert!(
        lock(&session)
            .finish_instance_contents_refresh(Actor::Agent, contents)
            .is_err()
    );
    assert!(matches!(
        done_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(store.list().unwrap().entries.is_empty());
    drop(extraction);
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    thread.join().unwrap();
    assert!(!stages[0].exists());
    assert!(!directory.path().join("drained.vmf").exists());
    assert_eq!(std::fs::read(&parent).unwrap(), source);
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let (_, text) = store.read(&entries[0].id).unwrap();
    assert_eq!(source_formats::vmf::Vmf::parse(&text).unwrap(), expected);
    assert_eq!(lock(&session).recovery_status()["pending_changes"], false);
    document.shutdown().unwrap();
    assert_eq!(store.list().unwrap().entries.len(), 1);
}

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    recovery::Store,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let mut state = Session::new(Err("no assets needed".into()));
    state.configure_recovery(Ok(store.directory.clone()));
    state
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test",
            false,
        )
        .unwrap();
    let session = Arc::new(Mutex::new(state));
    let document = DocumentHandle::start(session.clone()).unwrap();
    (directory, document, session, store)
}

#[test]
fn owning_handles_retain_fixed_runtime_until_the_last_owner_drains() {
    let (_directory, document, session, store) = fixture();
    let runtime = Arc::downgrade(&document.0);
    let original = lock(&session).document().vmf().clone();
    let retained = document.clone();
    let second = retained.clone();
    assert!(Arc::ptr_eq(document.session(), &session));
    drop(document);
    assert!(runtime.upgrade().is_some());
    assert!(store.list().unwrap().entries.is_empty());
    drop(retained);
    assert!(runtime.upgrade().is_some());
    assert!(store.list().unwrap().entries.is_empty());
    drop(second);
    assert!(runtime.upgrade().is_none());
    assert_eq!(store.list().unwrap().entries.len(), 1);
    let state = lock(&session);
    assert_eq!(state.document().vmf(), &original);
    assert_eq!(state.recovery_status()["pending_changes"], false);
    assert_eq!(state.recovery_status()["busy"], false);
    assert!(state.recovery_status()["error"].is_null());
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_clones_retain_the_same_document_runtime() {
    let (_directory, document, _session, store) = fixture();
    let runtime = Arc::downgrade(&document.0);
    let server = crate::mcp::EditorServer::for_document(document.clone(), None);
    let retained = server.clone();
    drop(document);
    drop(server);
    assert!(runtime.upgrade().is_some());
    assert!(store.list().unwrap().entries.is_empty());
    drop(retained);
    assert!(runtime.upgrade().is_none());
    assert_eq!(store.list().unwrap().entries.len(), 1);
}

#[test]
fn finished_handles_retain_exact_failure_without_retrying_after_storage_repair() {
    let (_directory, document, session, store) = fixture();
    let retained = document.clone();
    std::fs::write(&store.directory, b"not a directory").unwrap();
    let failure = document.shutdown().unwrap_err();
    assert!(failure.contains("Exit recovery snapshot failed"));
    std::fs::remove_file(&store.directory).unwrap();
    std::fs::create_dir(&store.directory).unwrap();
    lock(&session)
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(128.0),
            glam::DVec3::splat(192.0),
            "late",
            false,
        )
        .unwrap();
    assert_eq!(retained.shutdown().unwrap_err(), failure);
    drop(document);
    assert_eq!(retained.shutdown().unwrap_err(), failure);
    drop(retained);
    assert!(store.list().unwrap().entries.is_empty());
    let status = lock(&session).recovery_status();
    assert_eq!(status["pending_changes"], true);
    assert_eq!(status["busy"], false);
    assert_eq!(status["drain_error"], failure);
}
