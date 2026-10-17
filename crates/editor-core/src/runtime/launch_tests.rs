use super::*;
use crate::session::compilation::CompileResult;
use source_formats::vmf::Vmf;
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

fn fixture() -> (
    tempfile::TempDir,
    SharedSession,
    Result<CompileResult, String>,
    std::path::PathBuf,
) {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let game_dir = game.game_dir.clone();
    let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
    let vmf = directory.path().join("room.vmf");
    std::fs::write(&vmf, crate::test_assets::room().write(&Default::default())).unwrap();
    lock(&shared).open(Actor::User, &vmf, true).unwrap();
    let result = lock(&shared)
        .begin_compile(Actor::Agent, None)
        .unwrap()
        .run();
    assert!(result.as_ref().unwrap().output.bsp.is_some());
    (directory, shared, result, game_dir)
}

fn gated() -> (
    impl FnOnce(LaunchRequest) -> Result<game::Run, String> + Send + 'static,
    mpsc::Receiver<game::Log>,
    mpsc::Sender<()>,
) {
    let (started, observed) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let run = move |request: LaunchRequest| {
        let run = request.run()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let log = run.log(1)?;
            if log.console.contains("loaded ") {
                started.send(log).unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "fake game did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(run)
    };
    (run, observed, release)
}

fn replace(shared: &SharedSession) -> (Vmf, String) {
    let mut session = lock(shared);
    assert!(session.launching());
    session.new_map(Actor::User, true).unwrap();
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "audit/no-assets-needed",
            false,
        )
        .unwrap();
    (session.document().vmf().clone(), session.history_snapshot())
}

fn discarded(
    shared: &SharedSession,
    game_dir: &Path,
    log: &game::Log,
    document: &Vmf,
    history: &str,
) {
    let mut session = lock(shared);
    assert!(!session.launching() && !session.game_running());
    assert!(session.game_log(1).is_err());
    assert_eq!(session.document().vmf(), document);
    assert_eq!(session.history_snapshot(), history);
    assert!(!Path::new(&format!("/proc/{}", log.pid)).exists());
    assert!(
        !game_dir
            .join("maps")
            .join(format!("{}.bsp", log.map))
            .exists()
    );
    assert!(
        !game_dir
            .join("downloadlists")
            .join(format!("{}.lst", log.map))
            .exists()
    );
    assert_eq!(
        std::fs::read(game_dir.join("maps/stock.bsp")).unwrap(),
        b"stock map"
    );
}

#[test]
fn synchronous_launch_rejects_replacement_before_completion() {
    let (_directory, shared, result, game_dir) = fixture();
    lock(&shared).finish_compile(Actor::Agent, &result);
    let (run, started, release) = gated();
    let worker_session = shared.clone();
    let worker = std::thread::spawn(move || launch_with(&worker_session, Actor::Agent, run));
    let log = started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (document, history) = replace(&shared);
    release.send(()).unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .err()
            .unwrap()
            .contains("earlier map")
    );
    discarded(&shared, &game_dir, &log, &document, &history);
}

#[test]
fn asynchronous_launch_rejects_replacement_before_completion() {
    let (_directory, shared, result, game_dir) = fixture();
    lock(&shared).finish_compile(Actor::Agent, &result);
    let (run, started, release) = gated();
    let worker = start_launch_with(&shared, Actor::User, run).unwrap();
    let log = started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (document, history) = replace(&shared);
    release.send(()).unwrap();
    worker.join().unwrap();
    discarded(&shared, &game_dir, &log, &document, &history);
    assert!(
        lock(&shared)
            .recent_log()
            .next()
            .unwrap()
            .text
            .contains("earlier map")
    );
}

#[test]
fn compile_and_play_rejects_replacement_during_its_launch_completion() {
    let (_directory, shared, result, game_dir) = fixture();
    let (run, started, release) = gated();
    let worker_session = shared.clone();
    let worker = std::thread::spawn(move || {
        finish_compile_worker_with(&worker_session, Actor::Agent, &result, true, run)
    });
    let log = started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (document, history) = replace(&shared);
    release.send(()).unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .err()
            .unwrap()
            .contains("earlier map")
    );
    discarded(&shared, &game_dir, &log, &document, &history);
}
