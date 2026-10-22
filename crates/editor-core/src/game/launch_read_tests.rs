use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt, symlink};
#[cfg(target_os = "linux")]
use std::sync::mpsc;
use std::time::Instant;

use crate::test_assets::game_process as fixture;

const DEADLINE: Duration = Duration::from_secs(3);

#[test]
fn published_bytes_are_checked_before_decoding_or_creating_staging_files() {
    let (_directory, game, bsp) = fixture(true);
    let install = Install::from_game(&game).unwrap();
    let expected = compiler::FileIdentity::capture(&bsp, &compiler::Control::default()).unwrap();
    let mut changed = std::fs::read(&bsp).unwrap();
    changed[..4].copy_from_slice(b"NOPE");
    std::fs::write(&bsp, &changed).unwrap();
    let error = Run::launch_published(&install, &bsp, &expected, None, |_| Ok(()))
        .err()
        .unwrap();
    assert!(error.contains("Compiled BSP bytes changed"), "{error}");
    assert_eq!(std::fs::read(&bsp).unwrap(), changed);
    assert_eq!(std::fs::read_dir(&install.game).unwrap().count(), 1);
    assert_eq!(
        std::fs::read_dir(install.game.join("maps"))
            .unwrap()
            .count(),
        1
    );
    assert!(!install.game.join("downloadlists").exists());
}

fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    result
}

fn running(install: &mut Install, bsp: &Path) -> Run {
    let run = Run::launch(install, bsp).unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        let log = run.log(DEFAULT_LOG_LINES).unwrap();
        if log.console.contains("loaded") && log.launcher.contains("launcher-output") {
            break;
        }
        assert!(Instant::now() < deadline, "fake game did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    install.executable = install.root.join("rejected_launch_client");
    std::fs::write(
        &install.executable,
        "#!/bin/sh\n: > \"$0.invoked\"\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&install.executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    install.binding = Target::play(&install.root, &install.game, &install.executable).unwrap();
    run
}

fn artifacts(install: &Install, run: &Run) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = files(&install.game);
    result.insert(
        run.launcher.path().to_owned(),
        std::fs::read(run.launcher.path()).unwrap(),
    );
    result
}

fn preserved(install: &Install, run: &Run, before: &BTreeMap<PathBuf, Vec<u8>>) {
    assert!(run.running());
    assert!(
        run.process
            .lock()
            .unwrap()
            .child
            .try_wait()
            .unwrap()
            .is_none()
    );
    assert_eq!(&artifacts(install, run), before);
    assert!(!install.executable.with_extension("invoked").exists());
}

#[test]
fn regular_compiled_bsp_aliases_remain_launchable() {
    let (directory, game, bsp) = fixture(true);
    let install = Install::from_game(&game).unwrap();
    let original = std::fs::read(&bsp).unwrap();
    let linked = directory.path().join("linked.bsp");
    let hard = directory.path().join("hard.bsp");
    let parent = directory.path().join("output-alias");
    symlink(&bsp, &linked).unwrap();
    std::fs::hard_link(&bsp, &hard).unwrap();
    symlink(directory.path(), &parent).unwrap();
    let parent_bsp = parent.join("source.bsp");
    for alias in [&linked, &hard, &parent_bsp] {
        let run = Run::launch(&install, alias).unwrap();
        let staged = install.game.join("maps").join(format!("{}.bsp", run.map));
        assert_eq!(
            std::fs::read(&staged).unwrap(),
            stage_map_bytes(&original, &run.map).unwrap()
        );
        drop(run);
        assert!(!staged.exists());
        assert_eq!(std::fs::read(alias).unwrap(), original);
    }
    assert!(std::fs::symlink_metadata(linked).unwrap().is_symlink());
    assert!(std::fs::symlink_metadata(parent).unwrap().is_symlink());
    assert_eq!(std::fs::read(&bsp).unwrap(), original);
}

#[test]
#[cfg(target_os = "linux")]
fn bounded_launch_read_failures_preserve_the_running_game_and_staging() {
    let (directory, game, bsp) = fixture(true);
    let mut install = Install::from_game(&game).unwrap();
    let run = running(&mut install, &bsp);
    let before = artifacts(&install, &run);
    let oversized = directory.path().join("oversized.bsp");
    let oversized_file = std::fs::File::create(&oversized).unwrap();
    oversized_file
        .set_len(PakLimits::default().max_bsp_bytes as u64 + 1)
        .unwrap();
    let folder = directory.path().join("directory.bsp");
    std::fs::create_dir(&folder).unwrap();
    let fifo = directory.path().join("fifo.bsp");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let linked_fifo = directory.path().join("linked-fifo.bsp");
    symlink(&fifo, &linked_fifo).unwrap();
    let socket = directory.path().join("socket.bsp");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    for path in [
        oversized,
        folder,
        fifo,
        linked_fifo,
        socket,
        PathBuf::from("/dev/null"),
    ] {
        let launch_install = install.clone();
        let requested = path.clone();
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = Run::launch(&launch_install, &requested).map(drop);
            send.send(result).unwrap();
        });
        let result = receive
            .recv_timeout(DEADLINE)
            .unwrap_or_else(|error| panic!("{} did not reject promptly: {error}", path.display()));
        worker.join().unwrap();
        assert!(result.is_err(), "{} was accepted", path.display());
        preserved(&install, &run, &before);
    }
    run.stop().unwrap();
}

#[derive(Clone, Copy)]
enum CancelAt {
    Read,
    StagedMap,
    PreparedLogs,
}

#[test]
fn cancelled_bsp_preparation_preserves_prior_run_and_cleans_new_staging() {
    let (_directory, game, bsp) = fixture(true);
    let mut install = Install::from_game(&game).unwrap();
    let run = running(&mut install, &bsp);
    let before = artifacts(&install, &run);
    let maps = install.game.join("maps");
    let map_count = std::fs::read_dir(&maps).unwrap().count();
    let game_count = std::fs::read_dir(&install.game).unwrap().count();
    for cancel_at in [CancelAt::Read, CancelAt::StagedMap, CancelAt::PreparedLogs] {
        let mut cancelled = false;
        let result = Run::launch_controlled(&install, &bsp, || {
            let reached = match cancel_at {
                CancelAt::Read => true,
                CancelAt::StagedMap => std::fs::read_dir(&maps).unwrap().count() > map_count,
                CancelAt::PreparedLogs => {
                    std::fs::read_dir(&install.game).unwrap().count() > game_count
                }
            };
            if reached {
                cancelled = true;
                Err("game launch cancelled".into())
            } else {
                Ok(())
            }
        });
        assert!(cancelled);
        assert!(result.err().unwrap().contains("cancelled"));
        preserved(&install, &run, &before);
    }
    run.stop().unwrap();
}
