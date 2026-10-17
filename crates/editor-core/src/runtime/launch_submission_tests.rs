use super::*;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, SharedSession, std::path::PathBuf) {
    let (directory, shared, result, game_dir, _completion) = super::launch_tests::fixture();
    lock(&shared).finish_compile(Actor::Agent, &result);
    (directory, shared, game_dir)
}

#[test]
fn queued_launch_keeps_its_captured_buffer_and_rejects_replacement() {
    let (_directory, shared, _game_dir) = fixture();
    let submission = LaunchSubmission::begin(&shared, Actor::Agent).unwrap();
    lock(&shared).new_map(Actor::User, true).unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    assert!(submission.run().err().unwrap().contains("earlier map"));
    let session = lock(&shared);
    assert!(!session.launching() && !session.game_running());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn never_executed_worker_drops_under_session_guard_without_retiring_newer_work() {
    let (_directory, shared, _game_dir) = fixture();
    let submission = LaunchSubmission::begin(&shared, Actor::Agent).unwrap();
    let old = submission.origin.clone();
    let queued = move || submission.run();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut session = lock(&shared);
        drop(queued);
        assert!(!session.launching());
        let request = session.begin_launch(Actor::User).unwrap();
        let current = request.origin();
        assert!(
            session
                .finish_launch(Actor::Agent, &old, Err("old spawn failure".into()))
                .is_err()
        );
        assert!(session.launching());
        drop(request);
        assert!(
            session
                .finish_launch(Actor::User, &current, Err("fixture stopped".into()))
                .is_err()
        );
        assert!(!session.launching());
        send.send(()).unwrap();
    });
    receive
        .recv_timeout(Duration::from_secs(3))
        .expect("dropping a queued launch deadlocked on Session");
    worker.join().unwrap();
}

#[test]
fn cancellation_before_scheduling_skips_native_work_and_panic_keeps_peer_live() {
    let (_directory, shared, _game_dir) = fixture();
    let (_peer_directory, peer, _peer_game_dir) = fixture();
    let peer_submission = LaunchSubmission::begin(&peer, Actor::User).unwrap();
    let submission = LaunchSubmission::begin(&shared, Actor::Agent).unwrap();
    drop(submission.cancellation());
    assert!(lock(&shared).launching());
    assert!(
        submission
            .run_with(unexpected_launch)
            .err()
            .unwrap()
            .contains("cancelled")
    );
    assert!(!lock(&shared).launching());
    assert!(lock(&peer).launching());
    let submission = LaunchSubmission::begin(&shared, Actor::Agent).unwrap();
    assert!(
        submission
            .run_with(panicking_launch)
            .err()
            .unwrap()
            .contains("panicked")
    );
    assert!(!lock(&shared).launching());
    assert!(lock(&peer).launching());
    drop(peer_submission);
    assert!(!lock(&peer).launching());
}

fn unexpected_launch(_: LaunchRequest) -> Result<game::Run, String> {
    panic!("cancelled queued launch must not execute")
}

fn panicking_launch(_: LaunchRequest) -> Result<game::Run, String> {
    panic!("injected worker panic")
}

#[test]
fn cancelled_waiter_keeps_physical_launch_busy_until_process_cleanup() {
    let (_directory, shared, game_dir) = fixture();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let submission = LaunchSubmission::begin(&shared, Actor::Agent).unwrap();
    let cancellation = submission.cancellation();
    let (run, started, release) = super::launch_tests::gated();
    let worker = std::thread::spawn(move || submission.run_with(run));
    let log = started.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(cancellation);
    assert!(lock(&shared).launching());
    release.send(()).unwrap();
    assert!(worker.join().unwrap().err().unwrap().contains("cancelled"));
    super::launch_tests::discarded(&shared, &game_dir, &log, &source, &history);
}
