use super::*;
use crate::runtime::{DocumentHandle, lock};
use glam::DVec3;

fn fixture() -> (tempfile::TempDir, Session) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    edit(&mut session, 0.0);
    (directory, session)
}

fn edit(session: &mut Session, offset: f64) {
    session
        .create_block(
            Actor::User,
            DVec3::splat(offset),
            DVec3::splat(offset + 64.0),
            "test",
            false,
        )
        .unwrap();
}

fn state(session: &Session) -> (Value, Instant, u64, u64, Option<(u64, u64)>) {
    let mut status = session.recovery_status();
    status
        .as_object_mut()
        .unwrap()
        .remove("next_autosave_seconds");
    (
        status,
        session.recovery.last_attempt,
        session.recovery.serial,
        session.log_serial(),
        session
            .recovery
            .captured
            .as_ref()
            .map(|snapshot| (snapshot.generation, snapshot.revision)),
    )
}

fn seed(request: &RecoveryRequest) -> Entry {
    request
        .store
        .write(
            &request.document_key,
            request.source_path.as_deref(),
            "prior",
            Kind::Manual,
            b"prior snapshot",
        )
        .unwrap()
}

#[test]
fn recovery_unrun_drop_under_session_guard_retires_without_erasing_state() {
    let (_directory, mut session) = fixture();
    session.recovery.snapshot_error = Some("prior snapshot failure".into());
    session.recovery.retention_error = Some("prior retention failure".into());
    session.recovery.storage_error = Some("prior storage failure".into());
    session.recovery.drain_error = Some("prior drain failure".into());
    let shared = Arc::new(Mutex::new(session));
    let mut session = lock(&shared);
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let before = state(&session);
    let observed = session.recovery_serial();
    assert!(!session.autosave_due());
    drop(request);
    let after = state(&session);
    assert!(!session.recovery_busy());
    assert!(session.recovery_serial() > observed);
    assert_eq!(
        (&after.1, &after.2, &after.3, &after.4),
        (&before.1, &before.2, &before.3, &before.4)
    );
    for field in [
        "snapshot_error",
        "retention_error",
        "storage_error",
        "drain_error",
        "last_snapshot",
        "pending_changes",
    ] {
        assert_eq!(after.0[field], before.0[field]);
    }
    assert!(!session.autosave_due());
    assert!(session.recovery_entries().unwrap().entries.is_empty());
    let next = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    session
        .finish_recovery_snapshot(Actor::User, &next, next.run())
        .unwrap();
}

#[test]
fn recovery_abandoned_ready_request_preserves_its_file_without_adopting_or_pruning() {
    let (_directory, mut session) = fixture();
    let mut request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    request.keep = 1;
    let prior = seed(&request);
    let entry = request.run().unwrap();
    let before = state(&session);
    let observed = session.recovery_serial();
    drop(request);
    assert!(!session.recovery_busy());
    assert!(session.recovery_serial() > observed);
    let after = state(&session);
    assert_eq!(
        (&after.1, &after.2, &after.3, &after.4),
        (&before.1, &before.2, &before.3, &before.4)
    );
    assert!(after.0["last_snapshot"].is_null());
    assert_eq!(after.0["pending_changes"], true);
    let store = session.recovery.store.as_ref().unwrap();
    assert!(store.read(&entry.id).is_ok());
    assert!(store.read(&prior.id).is_ok());
    assert_eq!(store.list().unwrap().entries.len(), 2);
    drop(session.begin_recovery_snapshot(Kind::Manual).unwrap());
}

#[test]
fn recovery_foreign_and_mismatched_results_cannot_finish_equal_local_requests() {
    let (_a_directory, mut a) = fixture();
    let (_b_directory, mut b) = fixture();
    b.recovery.document_key = a.recovery.document_key.clone();
    assert_eq!(a.document_generation, b.document_generation);
    assert_eq!(a.document().revision(), b.document().revision());
    let mut first = a.begin_recovery_snapshot(Kind::Manual).unwrap();
    first.keep = 1;
    let mut peer = b.begin_recovery_snapshot(Kind::Manual).unwrap();
    peer.keep = 1;
    let first_prior = seed(&first);
    let peer_prior = seed(&peer);
    let first_result = first.run();
    let peer_result = peer.run();
    let before_a = state(&a);
    let before_b = state(&b);
    assert!(
        b.finish_recovery_snapshot(Actor::Agent, &first, first_result.clone())
            .is_err()
    );
    assert!(
        a.finish_recovery_snapshot(Actor::Agent, &first, peer_result.clone())
            .is_err()
    );
    let mut forged = first_result.clone().unwrap();
    forged.title = "unowned title".into();
    assert!(
        a.finish_recovery_snapshot(Actor::Agent, &first, Ok(forged))
            .is_err()
    );
    assert_eq!(state(&a), before_a);
    assert_eq!(state(&b), before_b);
    assert_eq!(first.store.list().unwrap().entries.len(), 2);
    assert_eq!(peer.store.list().unwrap().entries.len(), 2);
    a.finish_recovery_snapshot(Actor::User, &first, first_result)
        .unwrap();
    b.finish_recovery_snapshot(Actor::User, &peer, peer_result)
        .unwrap();
    assert!(first.store.read(&first_prior.id).is_err());
    assert!(peer.store.read(&peer_prior.id).is_err());
}

#[test]
fn recovery_duplicate_finish_and_old_request_drop_leave_new_origin_live() {
    let (_directory, mut session) = fixture();
    let mut first = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    first.keep = 1;
    let result = first.run();
    session
        .finish_recovery_snapshot(Actor::User, &first, result.clone())
        .unwrap();
    let terminal = state(&session);
    for result in [result.clone(), Err("duplicate failure".into())] {
        assert!(
            session
                .finish_recovery_snapshot(Actor::Agent, &first, result)
                .is_err()
        );
        assert_eq!(state(&session), terminal);
    }
    let next = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let next_result = next.run();
    let live = state(&session);
    for result in [result, Err("late failure".into())] {
        assert!(
            session
                .finish_recovery_snapshot(Actor::Agent, &first, result)
                .is_err()
        );
        assert_eq!(state(&session), live);
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 2);
    }
    drop(first);
    assert_eq!(state(&session), live);
    assert!(session.recovery_busy());
    session
        .finish_recovery_snapshot(Actor::User, &next, next_result)
        .unwrap();
    assert_eq!(session.recovery_entries().unwrap().entries.len(), 2);
}

#[test]
fn recovery_unrun_success_is_rejected_without_claiming_or_pruning() {
    let (_directory, mut session) = fixture();
    let mut request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    request.keep = 1;
    let first = seed(&request);
    let second = seed(&request);
    let before = state(&session);
    assert!(
        session
            .finish_recovery_snapshot(Actor::User, &request, Ok(second))
            .is_err()
    );
    assert_eq!(state(&session), before);
    assert!(request.store.read(&first.id).is_ok());
    assert_eq!(request.store.list().unwrap().entries.len(), 2);
    session
        .finish_recovery_snapshot(Actor::User, &request, request.run())
        .unwrap();
    assert_eq!(request.store.list().unwrap().entries.len(), 1);
}

#[test]
fn recovery_finish_during_writer_rejects_errors_and_preserves_exclusion() {
    let (_directory, mut session) = fixture();
    let request = Arc::new(session.begin_recovery_snapshot(Kind::Manual).unwrap());
    let writer = request.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        writer.run_with(|| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let before = state(&session);
    assert!(
        session
            .finish_recovery_snapshot(Actor::User, &request, Err("premature failure".into()))
            .is_err()
    );
    assert_eq!(state(&session), before);
    assert!(session.recovery_busy());
    assert!(session.begin_recovery_snapshot(Kind::Manual).is_err());
    assert!(!session.autosave_due());
    assert!(
        session
            .delete_recovery_snapshot(Actor::User, "unused")
            .unwrap_err()
            .contains("Wait for")
    );
    assert!(session.recovery_entries().unwrap().entries.is_empty());
    release_tx.send(()).unwrap();
    let result = thread.join().unwrap();
    session
        .finish_recovery_snapshot(Actor::User, &request, result)
        .unwrap();
    assert!(!session.recovery_busy());
    assert_eq!(session.recovery_entries().unwrap().entries.len(), 1);
}

#[test]
fn recovery_prestart_error_terminalizes_exact_origin_before_any_write() {
    let (_directory, mut session) = fixture();
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let admitted = session.recovery.last_attempt;
    assert_eq!(
        session
            .finish_recovery_snapshot(Actor::User, &request, Err("spawn failed".into()))
            .unwrap_err(),
        "spawn failed"
    );
    assert!(!session.recovery_busy());
    assert!(session.recovery.last_attempt >= admitted);
    assert_eq!(session.recovery_status()["snapshot_error"], "spawn failed");
    assert!(request.run().is_err());
    assert!(session.recovery_entries().unwrap().entries.is_empty());
    let next = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    drop(request);
    assert!(session.recovery_busy());
    drop(next);
}

#[test]
fn recovery_run_scope_reaches_ready_on_unwind_without_completion_helper() {
    let (_directory, mut session) = fixture();
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        request.run_with(|| panic!("injected write-scope panic"))
    }));
    assert!(panic.is_err());
    assert!(session.recovery_busy());
    assert_eq!(
        request.origin.phase.load(Ordering::Acquire),
        Phase::Ready as u8
    );
    assert!(request.run().is_err());
    assert_eq!(
        session
            .finish_recovery_snapshot(
                Actor::User,
                &request,
                Err("Recovery snapshot worker panicked".into())
            )
            .unwrap_err(),
        "Recovery snapshot worker panicked"
    );
    assert!(!session.recovery_busy());
    assert!(session.recovery_entries().unwrap().entries.is_empty());
}

#[test]
fn recovery_old_generation_finishes_origin_store_and_policy_without_adopting_replacement() {
    let (directory, mut session) = fixture();
    let mut settings = session.recovery_settings();
    settings.keep_per_map = 1;
    session
        .set_recovery_settings(Actor::User, settings.clone(), None)
        .unwrap();
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let original = session.document().vmf().clone();
    let prior = seed(&request);
    session.new_map(Actor::User, true).unwrap();
    edit(&mut session, 128.0);
    session.configure_recovery(Ok(directory.path().join("replacement")));
    settings.keep_per_map = 5;
    session
        .set_recovery_settings(Actor::User, settings, None)
        .unwrap();
    let replacement = session.document().vmf().clone();
    let history = session.history_snapshot();
    let entry = session
        .finish_recovery_snapshot(Actor::User, &request, request.run())
        .unwrap();
    assert!(request.store.read(&prior.id).is_err());
    let (_, text) = request.store.read(&entry.id).unwrap();
    assert_eq!(Vmf::parse(&text).unwrap(), original);
    assert_eq!(request.store.list().unwrap().entries.len(), 1);
    assert!(session.recovery_entries().unwrap().entries.is_empty());
    assert!(session.recovery_status()["last_snapshot"].is_null());
    assert_eq!(session.recovery_status()["pending_changes"], true);
    assert_eq!(session.document().vmf(), &replacement);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn recovery_document_shutdown_waits_for_exact_manual_finish_or_drop_then_captures_newer_revision() {
    for finish in [false, true] {
        let (_directory, mut session) = fixture();
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let original = session.document().vmf().clone();
        edit(&mut session, 128.0);
        let expected = session.document().vmf().clone();
        let store = session.recovery.store.clone().unwrap();
        let shared = Arc::new(Mutex::new(session));
        let document = DocumentHandle::start(shared.clone()).unwrap();
        let draining = document.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || done_tx.send(draining.shutdown()).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let failure = lock(&shared)
                .begin_recovery_snapshot(Kind::Manual)
                .err()
                .unwrap();
            if failure == "The editor is closing" {
                break;
            }
            assert!(Instant::now() < deadline, "shutdown did not start");
            std::thread::yield_now();
        }
        assert!(matches!(
            done_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert!(store.list().unwrap().entries.is_empty());
        let (_peer_directory, mut peer) = fixture();
        drop(peer.begin_recovery_snapshot(Kind::Manual).unwrap());
        assert!(!peer.recovery_busy());
        let manual = finish
            .then(|| crate::recovery::complete_capture(&shared, Actor::User, &request).unwrap());
        drop(request);
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        thread.join().unwrap();
        let entries = store.list().unwrap().entries;
        assert_eq!(entries.len(), if finish { 2 } else { 1 });
        if let Some(manual) = manual {
            assert_eq!(
                Vmf::parse(&store.read(&manual.id).unwrap().1).unwrap(),
                original
            );
        }
        let final_entry = entries
            .iter()
            .find(|entry| entry.kind == Kind::Autosave)
            .unwrap();
        assert_eq!(
            Vmf::parse(&store.read(&final_entry.id).unwrap().1).unwrap(),
            expected
        );
        assert_eq!(lock(&shared).recovery_status()["pending_changes"], false);
        document.shutdown().unwrap();
        assert_eq!(store.list().unwrap().entries.len(), entries.len());
    }
}
