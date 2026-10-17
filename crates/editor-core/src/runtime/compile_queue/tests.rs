use super::*;
use crate::runtime::{DocumentAccess, DocumentHandle};
use crate::session::Session;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

struct Fixture {
    document: DocumentHandle,
    output: PathBuf,
    source: PathBuf,
    recovery: crate::recovery::Store,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new(queue: &CompileQueue, claims: &crate::path_claims::Authority) -> Self {
        let (directory, game, _) = crate::test_assets::game_process(false);
        let source = directory.path().join("source.vmf");
        let output = directory.path().join("output.bsp");
        let recovery = crate::recovery::Store {
            directory: directory.path().join("recovery"),
        };
        std::fs::write(
            &source,
            crate::test_assets::room().write(&Default::default()),
        )
        .unwrap();
        let mut state = Session::new(Ok(game));
        state.use_application_path_claims(claims.clone());
        state.configure_recovery(Ok(recovery.directory.clone()));
        state.open(Actor::User, &source, true).unwrap();
        let document = DocumentHandle::start_managed(Arc::new(Mutex::new(state)), queue).unwrap();
        Self {
            document,
            output,
            source,
            recovery,
            _directory: directory,
        }
    }

    fn access(&self) -> DocumentAccess {
        DocumentAccess::for_document(self.document.clone())
    }

    fn submit(
        &self,
        queue: &QueueClient,
        before_run: impl FnOnce() + Send + 'static,
    ) -> CompileHandle {
        queue
            .submit_with(
                self.document.id(),
                self.document.session(),
                self.document.admit().unwrap(),
                CompileInput {
                    actor: Actor::Agent,
                    path: Some(&self.output),
                    play: false,
                    generation: None,
                },
                before_run,
            )
            .unwrap()
    }

    fn token(&self) -> u64 {
        lock(self.document.session()).compile_token().unwrap()
    }

    fn prepare(&self, queue: &QueueClient) -> (Reservation, CompileSubmission) {
        let reservation = queue.reserve(self.document.id()).unwrap();
        let operation = self.document.admit().unwrap();
        let mut submission = CompileSubmission::begin(
            self.document.session(),
            Actor::Agent,
            Some(&self.output),
            false,
            None,
        )
        .unwrap();
        submission._operation = Some(operation);
        (reservation, submission)
    }
}

fn wait(handle: CompileHandle) -> Outcome {
    let state = handle.result.state.lock().unwrap();
    let (mut state, timeout) = handle
        .result
        .ready
        .wait_timeout_while(state, TIMEOUT, |state| state.value.is_none())
        .unwrap();
    assert!(
        !timeout.timed_out(),
        "compile queue did not return its owned outcome"
    );
    state.value.take().unwrap()
}

fn edit(session: &SharedSession) {
    lock(session)
        .create_block(
            Actor::User,
            glam::DVec3::splat(32.0),
            glam::DVec3::splat(48.0),
            "test",
            false,
        )
        .unwrap();
}

#[test]
fn managed_queue_bounds_admission_serializes_fifo_and_preserves_captured_content() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let c = Fixture::new(&queue, &claims);
    let d = Fixture::new(&queue, &claims);
    let e = Fixture::new(&queue, &claims);
    let order = Arc::new(Mutex::new(Vec::new()));
    let (entered, started) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    let first_order = order.clone();
    let first = a.submit(&client, move || {
        first_order.lock().unwrap().push("a");
        entered.send(()).unwrap();
        blocked.recv_timeout(TIMEOUT).unwrap();
    });
    started.recv_timeout(TIMEOUT).unwrap();
    let frozen = lock(b.document.session()).document().vmf().clone();
    let source = std::fs::read(&b.source).unwrap();
    let second_order = order.clone();
    let second = b.submit(&client, move || second_order.lock().unwrap().push("b"));
    let third_order = order.clone();
    let third = c.submit(&client, move || third_order.lock().unwrap().push("c"));
    let fourth = d.submit(&client, || {
        panic!("cancelled queued job entered compiler work")
    });
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["status"],
        "queued"
    );
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["queue"]["position"],
        1
    );
    assert_eq!(
        lock(c.document.session()).compile_status()["job"]["queue"]["position"],
        2
    );
    let previous = lock(e.document.session()).compile_status();
    assert!(
        e.access()
            .start_compile(Actor::User, Some(&e.output), false, None)
            .unwrap_err()
            .contains("full")
    );
    assert_eq!(lock(e.document.session()).compile_status(), previous);
    let token = b.token();
    assert!(
        b.access()
            .start_compile(Actor::User, Some(&b.output), false, None)
            .unwrap_err()
            .contains("already")
    );
    assert_eq!(b.token(), token);
    assert!(!a.output.exists());
    assert!(!b.output.exists());
    edit(b.document.session());
    let newer = lock(b.document.session()).document().vmf().clone();
    let history = lock(b.document.session()).history_snapshot();
    assert!(
        !lock(b.document.session())
            .document()
            .vmf()
            .world
            .solids
            .is_empty()
    );
    d.access().cancel_compile(Actor::User, d.token()).unwrap();
    assert!(wait(fourth).err().unwrap().contains("cancelled"));
    assert_eq!(
        lock(d.document.session()).compile_status()["job"]["status"],
        "cancelled"
    );
    assert_eq!(client.0.state.lock().unwrap().records.len(), 3);
    assert_eq!(&*order.lock().unwrap(), &["a"]);
    let fifth = e
        .access()
        .submit_compile(Actor::User, Some(&e.output), false, None)
        .unwrap();
    e.access().cancel_compile(Actor::User, e.token()).unwrap();
    assert!(wait(fifth).is_err());
    release.send(()).unwrap();
    assert!(wait(first).unwrap().0.published());
    let second = wait(second).unwrap().0;
    assert!(second.published());
    assert_eq!(second.output.stats.brushes, frozen.world.solids.len());
    assert!(wait(third).unwrap().0.published());
    assert_eq!(&*order.lock().unwrap(), &["a", "b", "c"]);
    let state = lock(b.document.session());
    assert_eq!(state.document().vmf(), &newer);
    assert_eq!(state.history_snapshot(), history);
    assert!(state.document().is_dirty());
    assert_eq!(std::fs::read(&b.source).unwrap(), source);
    assert!(state.compile_status()["job"]["queue"]["position"].is_null());
    drop(state);
    queue.shutdown().unwrap();
}

#[test]
fn detached_queued_waiter_stays_owned_until_document_cancel_and_final_recovery() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let peer = Fixture::new(&queue, &claims);
    let (entered, started) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    let first = a.submit(&client, move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(TIMEOUT).unwrap();
    });
    started.recv_timeout(TIMEOUT).unwrap();
    edit(b.document.session());
    let expected = lock(b.document.session()).document().vmf().clone();
    drop(b.submit(&client, || {
        panic!("drained queued job entered compiler work")
    }));
    assert!(lock(b.document.session()).compiling());
    assert!(
        lock(peer.document.session())
            .save(Actor::Agent, Some(&b.output))
            .unwrap_err()
            .contains("conflicts")
    );
    let draining = b.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert!(!b.output.exists());
    assert!(!b.output.with_extension("lin").exists());
    assert!(!lock(b.document.session()).compiling());
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["status"],
        "cancelled"
    );
    let entries = b.recovery.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        source_formats::vmf::Vmf::parse(&b.recovery.read(&entries[0].id).unwrap().1).unwrap(),
        expected
    );
    lock(peer.document.session())
        .save(Actor::Agent, Some(&b.output))
        .unwrap();
    assert!(lock(a.document.session()).compiling());
    assert!(!a.output.exists());
    release.send(()).unwrap();
    assert!(wait(first).unwrap().0.published());
    let next = peer
        .access()
        .submit_compile(Actor::User, Some(&peer.output), false, None)
        .unwrap();
    assert!(wait(next).unwrap().0.published());
    b.document.shutdown().unwrap();
    assert_eq!(b.recovery.list().unwrap().entries.len(), 1);
    queue.shutdown().unwrap();
}

#[test]
fn queue_reservations_retire_under_session_lock_and_failed_begin_releases_capacity() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let document = Fixture::new(&queue, &claims);
    let reservation = client.reserve(document.document.id()).unwrap();
    let state = lock(document.document.session());
    let before = state.compile_status();
    drop(reservation);
    assert_eq!(state.compile_status(), before);
    assert!(client.0.state.lock().unwrap().records.is_empty());
    drop(state);
    assert!(
        document
            .access()
            .submit_compile(
                Actor::User,
                Some(&document.output.with_extension("vmf")),
                false,
                None
            )
            .is_err()
    );
    assert!(client.0.state.lock().unwrap().records.is_empty());
    assert_eq!(lock(document.document.session()).compile_status(), before);
    document.document.shutdown().unwrap();
    assert!(client.reserve(document.document.id()).is_err());
    queue.shutdown().unwrap();
    assert_eq!(queue.shutdown(), Ok(()));
}

#[test]
fn queue_submission_panic_retires_exact_origin_and_worker_serves_next_document() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let failed = a.submit(&client, || panic!("injected queue execution panic"));
    assert_eq!(
        wait(failed).err().unwrap(),
        "Compiler queue submission panicked"
    );
    assert!(!lock(a.document.session()).compiling());
    assert_eq!(
        lock(a.document.session()).compile_status()["job"]["status"],
        "failed"
    );
    assert!(!a.output.exists());
    let handle = b
        .access()
        .submit_compile(Actor::User, Some(&b.output), false, None)
        .unwrap();
    assert!(wait(handle).unwrap().0.published());
    queue.shutdown().unwrap();
}

#[test]
fn cancellation_between_capture_and_enqueue_retires_without_waiting_for_active_peer() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let (entered, started) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    let first = a.submit(&client, move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(TIMEOUT).unwrap();
    });
    started.recv_timeout(TIMEOUT).unwrap();
    let (reservation, submission) = b.prepare(&client);
    b.access().cancel_compile(Actor::User, b.token()).unwrap();
    assert!(
        client
            .enqueue(reservation, submission, || panic!(
                "cancelled before enqueue"
            ))
            .err()
            .unwrap()
            .contains("cancelled")
    );
    assert!(!lock(b.document.session()).compiling());
    assert_eq!(b.document.active_operations(), 0);
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["status"],
        "cancelled"
    );
    assert_eq!(client.0.state.lock().unwrap().records.len(), 1);
    let terminal_queue = lock(b.document.session()).compile_status()["job"]["queue"].clone();
    assert!(terminal_queue["position"].is_null());
    assert_eq!(terminal_queue["admitted"], 1);
    assert!(!b.output.exists());
    assert!(lock(a.document.session()).compiling());
    release.send(()).unwrap();
    assert!(wait(first).unwrap().0.published());
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["queue"],
        terminal_queue
    );
    queue.shutdown().unwrap();
}

#[test]
fn popped_worker_failure_notifies_active_and_queued_origins_and_memoizes_rejection() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let c = Fixture::new(&queue, &claims);
    let peer = Fixture::new(&queue, &claims);
    let (entered, started) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    queue.before_next_execute(move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(TIMEOUT).unwrap();
        panic!("injected outer worker failure after pop");
    });
    let first = a.submit(&client, || panic!("popped job reached execution"));
    started.recv_timeout(TIMEOUT).unwrap();
    let second = b.submit(&client, || panic!("queued peer reached execution"));
    let (reservation, submission) = c.prepare(&client);
    release.send(()).unwrap();
    let error = "Compiler queue worker panicked";
    assert_eq!(wait(first).err().unwrap(), error);
    assert_eq!(wait(second).err().unwrap(), error);
    assert_eq!(
        client
            .enqueue(reservation, submission, || {})
            .err()
            .unwrap(),
        error
    );
    for fixture in [&a, &b, &c] {
        assert!(!lock(fixture.document.session()).compiling());
        assert_eq!(fixture.document.active_operations(), 0);
        assert_eq!(
            lock(fixture.document.session()).compile_status()["job"]["status"],
            "failed"
        );
        assert!(!fixture.output.exists());
        lock(peer.document.session())
            .save(Actor::Agent, Some(&fixture.output))
            .unwrap();
    }
    assert!(client.0.state.lock().unwrap().records.is_empty());
    assert_eq!(client.reserve(peer.document.id()).err().unwrap(), error);
    assert_eq!(queue.shutdown(), Err(error.to_owned()));
    assert_eq!(queue.shutdown(), Err(error.to_owned()));
}

#[test]
fn exact_failure_completion_keeps_document_operation_until_after_failure_mutation() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let (entered, started) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    let first = a.submit(&client, move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(TIMEOUT).unwrap();
    });
    started.recv_timeout(TIMEOUT).unwrap();
    edit(b.document.session());
    let second = b.submit(&client, || panic!("failed queued job reached execution"));
    let mut entry = client.0.state.lock().unwrap().queue.pop_front().unwrap();
    let (failure_entered, failure_started) = mpsc::sync_channel(1);
    let (finish_failure, failure_blocked) = mpsc::sync_channel(1);
    entry.guard.before_failure = Some(Box::new(move || {
        failure_entered.send(()).unwrap();
        failure_blocked.recv_timeout(TIMEOUT).unwrap();
    }));
    let failure = std::thread::spawn(move || entry.fail("owned scheduling failure".into()));
    failure_started.recv_timeout(TIMEOUT).unwrap();
    assert!(!lock(b.document.session()).compiling());
    assert_eq!(b.document.active_operations(), 1);
    let draining = b.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while b.document.lifecycle() == crate::runtime::DocumentLifecycle::Live {
        assert!(Instant::now() < deadline, "document did not begin draining");
        std::thread::yield_now();
    }
    assert!(completed.try_recv().is_err());
    assert_eq!(b.recovery.list().unwrap().entries.len(), 0);
    finish_failure.send(()).unwrap();
    assert_eq!(wait(second).err().unwrap(), "owned scheduling failure");
    failure.join().unwrap();
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    assert_eq!(b.document.active_operations(), 0);
    assert_eq!(b.recovery.list().unwrap().entries.len(), 1);
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["result"]["error"],
        "owned scheduling failure"
    );
    release.send(()).unwrap();
    assert!(wait(first).unwrap().0.published());
    queue.shutdown().unwrap();
}

#[test]
fn production_queue_rejects_second_owner_until_joined_owner_is_released() {
    let mut queue = CompileQueue::process().unwrap();
    let error = CompileQueue::process().err().unwrap();
    assert!(error.contains("already has an application owner"));
    queue.shutdown().unwrap();
    assert_eq!(CompileQueue::process().err().unwrap(), error);
    drop(queue);
    let mut successor = CompileQueue::process().unwrap();
    successor.shutdown().unwrap();
}
