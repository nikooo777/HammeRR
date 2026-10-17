use super::*;

fn fixture() -> (tempfile::TempDir, SharedSession) {
    let (directory, game) = crate::test_assets::game();
    let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
    let path = directory.path().join("room.vmf");
    std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
    lock(&shared).open(Actor::User, &path, false).unwrap();
    (directory, shared)
}

fn identity(shared: &SharedSession) -> (Value, u64, bool) {
    let session = lock(shared);
    let status = session.compile_status();
    (
        serde_json::json!({
            "token": status["job"]["token"],
            "status": status["job"]["status"],
            "result": status["job"]["result"],
        }),
        session.log_serial(),
        session.compiling(),
    )
}

#[test]
fn owned_compile_failures_update_only_their_running_job() {
    let (_directory, shared) = fixture();
    let first = CompileSubmission::begin(&shared, Actor::Agent, None, false, None).unwrap();
    let failed = first.completion();
    assert!(failed.fail("worker could not start".into()));
    let terminal = identity(&shared);
    assert_eq!(terminal.0["status"], "failed");
    assert!(terminal.2);
    assert!(CompileSubmission::begin(&shared, Actor::User, None, false, None).is_err());
    assert!(!failed.fail("late worker join failure".into()));
    assert_eq!(identity(&shared), terminal);
    drop(first);
    let second = CompileSubmission::begin(&shared, Actor::User, None, false, None).unwrap();
    let running = identity(&shared);
    assert_ne!(running.0["token"], terminal.0["token"]);
    assert!(!failed.fail("old worker failure after a new compile".into()));
    assert_eq!(identity(&shared), running);
    assert!(
        second
            .completion()
            .fail("fixture stopped before compiling".into())
    );
}

#[test]
fn abandoned_submission_accepts_one_exact_scheduling_failure() {
    let (_directory, shared) = fixture();
    let submission = CompileSubmission::begin(&shared, Actor::Agent, None, false, None).unwrap();
    let completion = submission.completion();
    let queued = move || submission.run();
    let session = lock(&shared);
    drop(queued);
    assert!(!session.compiling());
    drop(session);
    assert!(
        identity(&shared).0["result"]["error"]
            .as_str()
            .unwrap()
            .contains("before execution")
    );
    assert!(completion.fail("worker could not be scheduled".into()));
    let terminal = identity(&shared);
    let job = lock(&shared).compile_status()["job"].clone();
    assert_eq!(
        terminal.0["result"]["error"],
        "worker could not be scheduled"
    );
    assert!(!completion.fail("duplicate failure".into()));
    assert_eq!(identity(&shared), terminal);
    assert_eq!(lock(&shared).compile_status()["job"], job);
    let newer = CompileSubmission::begin(&shared, Actor::User, None, false, None).unwrap();
    let running = identity(&shared);
    assert!(!completion.fail("old failure after a new compile".into()));
    assert_eq!(identity(&shared), running);
    drop(newer);
}

#[test]
fn duplicate_success_and_late_failures_do_not_relaunch_or_change_newer_jobs() {
    let (_directory, shared) = fixture();
    let first = CompileSubmission::begin(&shared, Actor::Agent, None, true, None).unwrap();
    let completion = first.completion();
    let result = first.request.run();
    assert!(result.as_ref().unwrap().output.bsp.is_some());
    assert!(lock(&shared).finish_compile_for_origin(Actor::Agent, &completion.origin, &result));
    let terminal = identity(&shared);
    assert_eq!(terminal.0["status"], "completed");
    assert!(!completion.fail("late blocking-task join failure".into()));
    assert!(
        completion
            .finish_with(&result, unexpected_launch)
            .unwrap()
            .is_none()
    );
    assert_eq!(identity(&shared), terminal);
    let second = CompileSubmission::begin(&shared, Actor::User, None, true, None).unwrap();
    let running = identity(&shared);
    assert!(!completion.fail("old completion failure".into()));
    assert!(
        completion
            .finish_with(&result, unexpected_launch)
            .unwrap()
            .is_none()
    );
    assert!(
        second
            .completion()
            .finish_with(&result, unexpected_launch)
            .unwrap()
            .is_none()
    );
    assert_eq!(identity(&shared), running);
    assert!(!lock(&shared).launching());
    assert!(!lock(&shared).game_running());
    assert!(
        second
            .completion()
            .fail("fixture stopped before compiling".into())
    );
}

fn unexpected_launch(_: LaunchRequest) -> Result<game::Run, String> {
    panic!("an ignored compile completion must not launch the game")
}

#[test]
fn compile_submissions_retain_the_originating_session_and_status_snapshot() {
    let (_directory, origin) = fixture();
    let (_other_directory, other) = fixture();
    let first = CompileSubmission::begin(&origin, Actor::Agent, None, false, None).unwrap();
    let second = CompileSubmission::begin(&other, Actor::User, None, false, None).unwrap();
    assert_eq!(
        first.status()["job"]["token"],
        second.status()["job"]["token"]
    );
    let other_before = identity(&other);
    let status = first.status().clone();
    let late_failure = first.completion();
    let (result, game) = first.run().unwrap();
    assert!(result.output.bsp.is_some());
    assert!(game.is_none());
    assert_eq!(status["job"]["status"], "running");
    assert_eq!(identity(&origin).0["status"], "completed");
    assert_eq!(identity(&other), other_before);
    assert!(!late_failure.fail("old origin worker failure".into()));
    assert_eq!(identity(&other), other_before);
    assert!(
        second
            .completion()
            .fail("fixture stopped before compiling".into())
    );
}
