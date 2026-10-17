use super::*;
use crate::runtime::{DocumentHandle, lock};
use std::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture(instances: bool) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, game, source) = if instances {
        let (directory, game, source, _) = crate::test_assets::instance_map(false);
        (directory, game, source)
    } else {
        let (directory, game) = crate::test_assets::game();
        let source = directory.path().join("room.vmf");
        std::fs::write(
            &source,
            crate::test_assets::room().write(&Default::default()),
        )
        .unwrap();
        (directory, game, source)
    };
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    (directory, session, source)
}

fn stable(session: &Session) -> Value {
    let mut status = session.compile_status();
    status["job"].as_object_mut().unwrap().remove("elapsed_ms");
    json!({"compile":status,"log":session.log_serial(),"history":session.history_snapshot()})
}

#[test]
fn raw_abandoned_request_retires_under_session_guard_and_keeps_exact_terminal_status() {
    let (directory, mut state, source) = fixture(false);
    let authority = crate::path_claims::Authority::default();
    state.file_claims.source = None;
    state.use_application_path_claims(authority.clone());
    let path = directory.path().join("abandoned.bsp");
    let request = state.begin_compile(Actor::User, Some(&path)).unwrap();
    let origin = request.origin();
    let before = state.document.vmf().clone();
    let history = state.history_snapshot();
    let bytes = std::fs::read(&source).unwrap();
    let shared = Arc::new(Mutex::new(state));
    let (send, receive) = mpsc::channel();
    let worker_session = shared.clone();
    let worker_path = path.clone();
    let thread = std::thread::spawn(move || {
        let mut state = lock(&worker_session);
        assert!(state.compiling());
        drop(request);
        assert!(!state.compiling());
        let terminal = state.compile_status();
        assert_eq!(terminal["job"]["status"], "failed");
        assert!(
            terminal["job"]["result"]["error"]
                .as_str()
                .unwrap()
                .contains("before execution")
        );
        assert_eq!(
            state.compile_status()["job"]["elapsed_ms"],
            terminal["job"]["elapsed_ms"]
        );
        assert!(!state.compile_cancellable());
        let token = state.compile_token().unwrap();
        assert!(state.cancel_compile(Actor::User, token).is_err());
        assert_eq!(state.compile_settings()["editable"], true);
        let newer = state
            .begin_compile(Actor::Agent, Some(&worker_path))
            .unwrap();
        let running = stable(&state);
        assert!(!state.finish_compile_for_origin(
            Actor::Agent,
            &origin,
            &Err("old failure".into())
        ));
        assert_eq!(stable(&state), running);
        assert!(state.compiling());
        drop(newer);
        send.send(()).unwrap();
    });
    receive
        .recv_timeout(TIMEOUT)
        .expect("request Drop took the Session lock");
    thread.join().unwrap();
    let state = lock(&shared);
    assert_eq!(state.document.vmf(), &before);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    drop(state);
    let mut peer = Session::new(Err("no assets needed".into()));
    peer.use_application_path_claims(authority);
    peer.save(Actor::Agent, Some(&path)).unwrap();
    peer.save(Actor::Agent, Some(&path.with_extension("lin")))
        .unwrap();
    let document = DocumentHandle::start(shared).unwrap();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(document.shutdown()).unwrap());
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
}

#[test]
fn foreign_equal_counter_results_preserve_both_origins_and_pending_publication() {
    for guarded in [false, true] {
        let (_first_directory, mut first, source) = fixture(guarded);
        let (_second_directory, mut second, _) = fixture(guarded);
        let first_request = first.begin_compile(Actor::Agent, None).unwrap();
        let second_request = second.begin_compile(Actor::User, None).unwrap();
        let result = first_request.run();
        let output = result.as_ref().unwrap();
        let job = second.compilation.job.as_ref().unwrap();
        assert_eq!(job.token, output.token);
        assert_eq!(job.snapshot.generation, output.generation);
        assert_eq!(job.snapshot.revision, output.revision);
        assert_eq!(job.options, output.options);
        assert_eq!(job.content_epoch, output.content_epoch);
        let first_before = stable(&first);
        let second_before = stable(&second);
        let bytes = std::fs::read(&output.path).unwrap();
        second.finish_compile(Actor::Agent, &result);
        assert_eq!(stable(&first), first_before);
        assert_eq!(stable(&second), second_before);
        assert!(first.compiling() && second.compiling());
        let publication = output.publication.lock().unwrap();
        assert_eq!(publication.pending.is_some(), guarded);
        assert_eq!(publication.claims.is_some(), guarded);
        assert_eq!(publication.artifact.is_some(), !guarded);
        assert!(publication.ticket.is_some());
        drop(publication);
        assert_eq!(std::fs::read(&output.path).unwrap(), bytes);
        first.finish_compile(Actor::Agent, &result);
        assert!(result.as_ref().unwrap().published());
        assert!(
            result
                .as_ref()
                .unwrap()
                .publication
                .lock()
                .unwrap()
                .artifact
                .is_none()
        );
        assert!(!first.compiling() && second.compiling());
        let next = first
            .begin_compile(Actor::User, Some(&source.with_file_name("next.bsp")))
            .unwrap();
        let running = stable(&first);
        drop(result);
        assert_eq!(stable(&first), running);
        assert!(first.compiling());
        drop(next);
        drop(second_request);
        assert!(!first.compiling() && !second.compiling());
    }
}

#[test]
fn guarded_result_drop_cleans_publication_before_shutdown_and_final_recovery() {
    let (directory, mut state, source) = fixture(true);
    let authority = crate::path_claims::Authority::default();
    state.file_claims.source = None;
    state.use_application_path_claims(authority.clone());
    state.configure_recovery(Ok(directory.path().join("recovery")));
    state
        .create_block(
            Actor::User,
            DVec3::splat(64.0),
            DVec3::splat(80.0),
            "test",
            false,
        )
        .unwrap();
    let result = state
        .begin_compile(Actor::User, None)
        .unwrap()
        .run()
        .unwrap();
    assert!(result.publication.lock().unwrap().pending.is_some());
    let old = std::fs::read(source.with_extension("bsp")).unwrap();
    let before = state.document.vmf().clone();
    let shared = Arc::new(Mutex::new(state));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    let draining = document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while !lock(&shared).closing {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let mut peer = Session::new(Err("no assets needed".into()));
    peer.use_application_path_claims(authority);
    assert!(
        peer.save(Actor::Agent, Some(&source.with_extension("bsp")))
            .is_err()
    );
    assert!(
        peer.save(Actor::Agent, Some(&source.with_extension("lin")))
            .is_err()
    );
    {
        let state = lock(&shared);
        assert!(state.compiling());
        drop(result);
        assert!(!state.compiling());
        assert_eq!(state.document.vmf(), &before);
        assert_eq!(state.compile_status()["job"]["status"], "failed");
    }
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), old);
    assert_eq!(lock(&shared).recovery_entries().unwrap().entries.len(), 1);
    peer.save(Actor::Agent, Some(&source.with_extension("bsp")))
        .unwrap();
    peer.save(Actor::Agent, Some(&source.with_extension("lin")))
        .unwrap();
}

#[test]
fn running_cancel_and_panic_retain_physical_busy_until_worker_cleanup() {
    for panic in [false, true] {
        let (_directory, mut state, _) = fixture(false);
        let request = state.begin_compile(Actor::Agent, None).unwrap();
        let origin = request.origin();
        let token = state.compile_token().unwrap();
        let shared = Arc::new(Mutex::new(state));
        let worker_session = shared.clone();
        let (started, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = request.run_with(|request| {
                started.send(()).unwrap();
                wait.recv().unwrap();
                assert!(!panic, "injected compile panic");
                request.run_inner()
            });
            assert!(result.is_err());
            assert!(lock(&worker_session).finish_compile_for_origin(
                Actor::Agent,
                &origin,
                &result
            ));
        });
        ready.recv_timeout(TIMEOUT).unwrap();
        lock(&shared).cancel_compile(Actor::User, token).unwrap();
        assert!(lock(&shared).compiling());
        let document = DocumentHandle::start(shared.clone()).unwrap();
        let (send, receive) = mpsc::channel();
        let draining = std::thread::spawn(move || send.send(document.shutdown()).unwrap());
        let deadline = Instant::now() + TIMEOUT;
        while !lock(&shared).closing {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(receive.try_recv().is_err());
        release.send(()).unwrap();
        worker.join().unwrap();
        receive.recv_timeout(TIMEOUT).unwrap().unwrap();
        draining.join().unwrap();
        assert!(!lock(&shared).compiling());
        assert_eq!(lock(&shared).compile_status()["job"]["status"], "cancelled");
    }
}
