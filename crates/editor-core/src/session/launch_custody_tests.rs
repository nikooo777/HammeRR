use super::*;
use source_formats::bsp::Bsp;
use std::fs::{File, FileTimes};
use std::sync::atomic::{AtomicBool, Ordering};

fn fixture(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf) {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let game_dir = game.game_dir.clone();
    let source = directory.path().join("room.vmf");
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.open(actor, &source, false).unwrap();
    let result = session.begin_compile(actor, None).unwrap().run();
    session.finish_compile(actor, &result);
    assert!(result.as_ref().unwrap().published());
    (directory, session, source, game_dir)
}

fn changed_output(path: &Path) -> Vec<u8> {
    let original = std::fs::read(path).unwrap();
    let mut bsp = Bsp::parse(&original).unwrap();
    bsp.map_revision += 1;
    let changed = bsp.to_bytes();
    assert_eq!(changed.len(), original.len());
    assert_ne!(changed, original);
    changed
}

fn staged_logs_exist(game: &Path) -> bool {
    std::fs::read_dir(game).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("hammerr_console_"))
    })
}

fn unstaged(session: &Session, game: &Path) {
    assert!(!session.launching() && !session.game_running());
    assert!(!game.join("downloadlists").exists());
    assert_eq!(std::fs::read_dir(game).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(game.join("maps")).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(game.join("maps/stock.bsp")).unwrap(),
        b"stock map"
    );
}

#[test]
fn metadata_restored_bsp_edits_reject_the_original_successful_build() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, game) = fixture(actor);
        let output = source.with_extension("bsp");
        let changed = changed_output(&output);
        let metadata = std::fs::metadata(&output).unwrap();
        let claim = session.compiled.as_ref().unwrap().claim.as_ref().unwrap();
        std::fs::write(&output, &changed).unwrap();
        File::options()
            .write(true)
            .open(&output)
            .unwrap()
            .set_times(FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        claim.destination().verify().unwrap();
        let history = session.history_snapshot();
        let memory = session.document().vmf().clone();
        let error = session.begin_launch(actor).err().unwrap();
        assert!(error.contains("Compiled BSP changed"), "{error}");
        assert_eq!(std::fs::read(&output).unwrap(), changed);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        unstaged(&session, &game);
    }
}

#[test]
fn accepted_launch_rechecks_output_before_reading_and_after_preparing_logs() {
    for actor in [Actor::User, Actor::Agent] {
        for final_checkpoint in [false, true] {
            let (directory, mut session, source, game) = fixture(actor);
            let output = source.with_extension("bsp");
            let replacement = directory.path().join("replacement.bsp");
            let changed = changed_output(&output);
            std::fs::write(&replacement, &changed).unwrap();
            let mut request = session.begin_launch(actor).unwrap();
            let origin = request.origin();
            let invoked = Arc::new(AtomicBool::new(false));
            let seen = invoked.clone();
            let checked_game = game.clone();
            let checked_output = output.clone();
            request.before_checkpoint = Some(Box::new(move || {
                if (!final_checkpoint || staged_logs_exist(&checked_game))
                    && !seen.swap(true, Ordering::AcqRel)
                {
                    std::fs::rename(&replacement, &checked_output).unwrap();
                }
            }));
            let history = session.history_snapshot();
            let memory = session.document().vmf().clone();
            let launch = request.run();
            assert!(invoked.load(Ordering::Acquire));
            let error = launch.as_ref().err().unwrap();
            assert!(error.contains("Compiled output binding changed"), "{error}");
            assert!(session.finish_launch(actor, &origin, launch).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), changed);
            assert_eq!(session.document().vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            unstaged(&session, &game);
        }
    }
}

#[test]
fn unavailable_published_identity_cannot_capture_a_replacement_baseline() {
    let (_directory, mut session, source, game) = fixture(Actor::Agent);
    let output = std::fs::read(source.with_extension("bsp")).unwrap();
    session.compiled.as_mut().unwrap().output_identity =
        Err("original identity was unavailable".into());
    let error = session.begin_launch(Actor::Agent).err().unwrap();
    assert!(error.contains("Compiled BSP identity is unavailable"));
    assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), output);
    unstaged(&session, &game);
}

#[test]
fn run_output_identity_does_not_inherit_the_smaller_inspection_budget() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("large.bsp");
    let mut expected = Bsp::new();
    expected.lumps[source_formats::bsp::lump::LIGHTING]
        .data
        .resize(baked_build::MAX_BSP_BYTES as usize, 0);
    expected.write(&output).unwrap();
    let run = baked_build::capture_output_identity(
        &output,
        Some(&expected),
        source_formats::bsp::PakLimits::default().max_bsp_bytes as u64,
        "Run",
    )
    .unwrap();
    run.verify_metadata().unwrap();
    let inspection = baked_build::BuildIdentity::capture(1, &output, Ok(None), Some(&expected));
    assert!(inspection.output.unwrap_err().contains("inspection budget"));
}

#[test]
#[cfg(unix)]
fn final_launch_checkpoint_refuses_retargeted_output_parent_without_touching_either_target() {
    use std::os::unix::fs::symlink;

    let (directory, mut session, _source, game) = fixture(Actor::Agent);
    let original = directory.path().join("original");
    let other = directory.path().join("other");
    let alias = directory.path().join("output");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&other).unwrap();
    symlink(&original, &alias).unwrap();
    let output = alias.join("room.bsp");
    let result = session
        .begin_compile(Actor::Agent, Some(&output))
        .unwrap()
        .run();
    session.finish_compile(Actor::Agent, &result);
    assert!(result.as_ref().unwrap().published());
    let bytes = std::fs::read(&output).unwrap();
    std::fs::write(other.join("room.bsp"), b"unrelated BSP").unwrap();
    let mut request = session.begin_launch(Actor::Agent).unwrap();
    let origin = request.origin();
    let invoked = Arc::new(AtomicBool::new(false));
    let seen = invoked.clone();
    let checked_game = game.clone();
    let checked_other = other.clone();
    let checked_alias = alias.clone();
    request.before_checkpoint = Some(Box::new(move || {
        if staged_logs_exist(&checked_game) && !seen.swap(true, Ordering::AcqRel) {
            std::fs::remove_file(&checked_alias).unwrap();
            symlink(&checked_other, &checked_alias).unwrap();
        }
    }));
    let launch = request.run();
    assert!(invoked.load(Ordering::Acquire));
    let error = launch.as_ref().err().unwrap();
    assert!(error.contains("Compiled output binding changed"), "{error}");
    assert!(
        session
            .finish_launch(Actor::Agent, &origin, launch)
            .is_err()
    );
    assert_eq!(std::fs::read(original.join("room.bsp")).unwrap(), bytes);
    assert_eq!(
        std::fs::read(other.join("room.bsp")).unwrap(),
        b"unrelated BSP"
    );
    assert_eq!(std::fs::read_link(alias).unwrap(), other);
    unstaged(&session, &game);
}
