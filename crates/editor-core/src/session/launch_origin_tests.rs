use super::*;
use map_core::edit;
use std::time::{Duration, Instant};

fn fixture() -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let game_dir = game.game_dir.clone();
    let mut session = Session::new(Ok(game));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    compile(&mut session);
    (directory, session, game_dir)
}

fn compile(session: &mut Session) {
    let result = session.begin_compile(Actor::Agent, None).unwrap().run();
    assert!(result.as_ref().unwrap().output.bsp.is_some());
    session.finish_compile(Actor::Agent, &result);
}

fn ready(run: &game::Run) -> game::Log {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let log = run.log(1).unwrap();
        if log.console.contains("loaded ") {
            return log;
        }
        assert!(Instant::now() < deadline, "fake game did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn stopped(game_dir: &Path, log: &game::Log) {
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
fn new_and_open_reject_late_launch_and_preserve_replacement_history() {
    for open in [false, true] {
        let (directory, mut session, game_dir) = fixture();
        let request = session.begin_launch(Actor::Agent).unwrap();
        let origin = request.origin();
        let run = request.run().unwrap();
        let log = ready(&run);
        let replacement = directory.path().join("replacement.vmf");
        std::fs::write(
            &replacement,
            crate::test_assets::room().write(&Default::default()),
        )
        .unwrap();
        if open {
            session.open(Actor::User, &replacement, true).unwrap();
        } else {
            session.new_map(Actor::User, true).unwrap();
        }
        session
            .edit(Actor::User, "replacement edit", |tx| {
                edit::create_entity(tx, "info_target", None, &[], Vec::new())
            })
            .unwrap();
        let document = session.document().vmf().clone();
        let history = session.history_snapshot();
        let path = session.path.clone();
        assert!(session.launching());
        assert!(
            session
                .finish_launch(Actor::Agent, &origin, Ok(run))
                .err()
                .unwrap()
                .contains("earlier map")
        );
        assert!(!session.launching() && !session.game_running());
        assert_eq!(session.document().vmf(), &document);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.path, path);
        assert!(session.game_log(1).is_err());
        stopped(&game_dir, &log);

        session.open(Actor::User, &replacement, true).unwrap();
        compile(&mut session);
        let request = session.begin_launch(Actor::User).unwrap();
        session
            .finish_launch(Actor::User, &request.origin(), request.run())
            .unwrap();
        assert!(session.game_running());
        session.stop_game(Actor::User).unwrap();
    }
}

#[test]
fn foreign_and_duplicate_completions_preserve_the_current_launch_and_run() {
    let (_first_directory, mut first, first_game) = fixture();
    let (_second_directory, mut second, _) = fixture();
    assert_eq!(first.document_generation, second.document_generation);
    let first_request = first.begin_launch(Actor::Agent).unwrap();
    let first_origin = first_request.origin();
    let second_request = second.begin_launch(Actor::User).unwrap();
    let second_origin = second_request.origin();
    let run = first_request.run().unwrap();
    let log = ready(&run);
    assert!(
        second
            .finish_launch(Actor::Agent, &first_origin, Ok(run))
            .is_err()
    );
    assert!(second.launching() && !second.game_running());
    stopped(&first_game, &log);
    assert!(
        first
            .finish_launch(
                Actor::Agent,
                &first_origin,
                Err("foreign completion rejected".into())
            )
            .is_err()
    );
    assert!(!first.launching());
    let second_log = second
        .finish_launch(Actor::User, &second_origin, second_request.run())
        .unwrap();
    assert!(
        second
            .finish_launch(
                Actor::Agent,
                &first_origin,
                Err("late foreign error".into())
            )
            .is_err()
    );
    assert!(second.game_running());
    assert_eq!(second.game_log(1).unwrap().pid, second_log.pid);
    second.stop_game(Actor::User).unwrap();

    let next = second.begin_launch(Actor::User).unwrap();
    assert!(
        second
            .finish_launch(
                Actor::User,
                &second_origin,
                Err("duplicate old completion".into())
            )
            .is_err()
    );
    assert!(second.launching());
    second
        .finish_launch(Actor::User, &next.origin(), next.run())
        .unwrap();
    assert!(second.game_running());
    second.stop_game(Actor::User).unwrap();
}

#[test]
fn same_buffer_edits_do_not_expire_an_admitted_launch() {
    let (_directory, mut session, _) = fixture();
    let request = session.begin_launch(Actor::Agent).unwrap();
    session
        .edit(Actor::User, "edit during launch", |tx| {
            edit::create_entity(tx, "info_target", None, &[], Vec::new())
        })
        .unwrap();
    let history = session.history_snapshot();
    session
        .finish_launch(Actor::Agent, &request.origin(), request.run())
        .unwrap();
    assert!(session.game_running());
    assert_eq!(session.history_snapshot(), history);
    session.stop_game(Actor::Agent).unwrap();
}

#[test]
fn replacement_also_discards_failed_launch_completion_and_releases_busy_state() {
    let (_directory, mut session, _) = fixture();
    let request = session.begin_launch(Actor::Agent).unwrap();
    session.new_map(Actor::User, true).unwrap();
    let history = session.history_snapshot();
    assert!(
        session
            .finish_launch(
                Actor::Agent,
                &request.origin(),
                Err("launcher failed".into())
            )
            .err()
            .unwrap()
            .contains("earlier map")
    );
    assert!(!session.launching() && !session.game_running());
    assert_eq!(session.history_snapshot(), history);
}
