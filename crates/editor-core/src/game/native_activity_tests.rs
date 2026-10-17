use super::*;
use crate::runtime::DocumentId;
use crate::runtime::native_activity::Authority;
use std::time::Instant;

const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn failed_child_signal_reports_unconfirmed_without_waiting_on_a_live_child() {
    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    let started = Instant::now();
    let failure = stop_child_with(&mut child, |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected signal failure",
        ))
    });
    assert!(failure.unwrap_err().contains("child exit is unconfirmed"));
    assert!(started.elapsed() < DEADLINE);
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
}

fn wait_until(check: impl Fn() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !check() {
        assert!(
            Instant::now() < deadline,
            "fake game did not reach its checkpoint"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn normal_child_exit_releases_admission_while_logs_and_run_remain_retained() {
    let (_directory, game, bsp) = crate::test_assets::game_process(false);
    let install = Install::from_game(&game).unwrap();
    let authority = Authority::isolated();
    let client = authority.client(DocumentId::allocate().unwrap());
    let peer = authority.client(DocumentId::allocate().unwrap());
    let permit = client
        .reserve(Phase::PlayStartup, None, Some(install.target()))
        .unwrap();
    let run = Run::launch_admitted(&install, &bsp, Some(permit), || Ok(())).unwrap();
    wait_until(|| !run.running());
    let log = run.log(DEFAULT_LOG_LINES).unwrap();
    assert!(log.console.contains("loaded "));
    assert_eq!(peer.status()["available"], true);
    let replacement = peer
        .reserve(Phase::Capture, Some(install.target()), None)
        .unwrap();
    assert!(
        run.log(DEFAULT_LOG_LINES)
            .unwrap()
            .launcher
            .contains("launcher-output")
    );
    drop(run);
    assert_eq!(
        client.status()["owner"]["document_id"],
        peer.status()["owner"]["document_id"]
    );
    drop(replacement);
}

#[test]
fn checked_cleanup_failure_is_memoized_and_keeps_native_capacity_occupied() {
    let (_directory, game, bsp) = crate::test_assets::game_process(true);
    let install = Install::from_game(&game).unwrap();
    let authority = Authority::isolated();
    let client = authority.client(DocumentId::allocate().unwrap());
    let peer = authority.client(DocumentId::allocate().unwrap());
    let permit = client
        .reserve(Phase::PlayStartup, None, Some(install.target()))
        .unwrap();
    let run = Run::launch_admitted(&install, &bsp, Some(permit), || Ok(())).unwrap();
    wait_until(|| run.log(1).unwrap().console.contains("loaded "));
    let list = game
        .game_dir
        .join("downloadlists")
        .join(format!("{}.lst", run.map));
    std::fs::remove_file(&list).unwrap();
    std::fs::create_dir(&list).unwrap();
    let error = run.stop().unwrap_err();
    assert!(error.contains("download-list cleanup"));
    assert_eq!(run.stop().unwrap_err(), error);
    assert!(matches!(run.status(), Status::Failed(_)));
    assert!(
        run.process
            .lock()
            .unwrap()
            .child
            .try_wait()
            .unwrap()
            .is_some()
    );
    assert!(
        !game
            .game_dir
            .join("maps")
            .join(format!("{}.bsp", run.map))
            .exists()
    );
    drop(run);
    assert_eq!(client.status()["owner"]["phase"], "failed_cleanup");
    assert!(
        peer.reserve(Phase::Capture, Some(install.target()), None)
            .is_err()
    );
    assert!(
        authority
            .cleanup_result()
            .unwrap_err()
            .contains("download-list cleanup")
    );
}

fn rejected_monitor(
    _process: Arc<Mutex<Process>>,
    _status: Arc<Mutex<Status>>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    Err(std::io::Error::other("injected monitor creation failure"))
}

#[test]
fn monitor_creation_and_staged_startup_failure_release_only_after_checked_cleanup() {
    let (_directory, game, bsp) = crate::test_assets::game_process(true);
    let install = Install::from_game(&game).unwrap();
    let authority = Authority::isolated();
    let client = authority.client(DocumentId::allocate().unwrap());
    for monitor_failure in [false, true] {
        let permit = client
            .reserve(Phase::PlayStartup, None, Some(install.target()))
            .unwrap();
        let result = if monitor_failure {
            Run::launch_with_monitor(&install, &bsp, Some(permit), || Ok(()), rejected_monitor)
        } else {
            Run::launch_admitted(&install, &bsp, Some(permit), || {
                if std::fs::read_dir(game.game_dir.join("maps"))
                    .unwrap()
                    .count()
                    > 1
                {
                    Err("injected cancellation after staging".into())
                } else {
                    Ok(())
                }
            })
        };
        assert!(result.is_err());
        assert_eq!(client.status()["available"], true);
        assert_eq!(
            std::fs::read_dir(game.game_dir.join("maps"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[test]
fn replaced_frozen_installation_rejects_before_game_staging() {
    let (directory, game, bsp) = crate::test_assets::game_process(true);
    let install = Install::from_game(&game).unwrap();
    let authority = Authority::isolated();
    let client = authority.client(DocumentId::allocate().unwrap());
    let permit = client
        .reserve(Phase::PlayStartup, None, Some(install.target()))
        .unwrap();
    std::fs::rename(&install.game, directory.path().join("original-game")).unwrap();
    std::fs::create_dir(&install.game).unwrap();
    let result = Run::launch_admitted(&install, &bsp, Some(permit), || Ok(()));
    assert!(result.err().unwrap().contains("binding changed"));
    assert_eq!(std::fs::read_dir(&install.game).unwrap().count(), 0);
    assert_eq!(client.status()["available"], true);
}
