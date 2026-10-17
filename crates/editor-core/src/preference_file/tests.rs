use super::*;
use crate::session::{Actor, Session};
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Grid,
    Preview,
    Navigation,
    Shortcuts,
}

impl Kind {
    const ALL: [Self; 4] = [Self::Grid, Self::Preview, Self::Navigation, Self::Shortcuts];

    fn file_name(self) -> &'static str {
        match self {
            Self::Grid => crate::grid_preferences::FILE_NAME,
            Self::Preview => crate::preview_preferences::FILE_NAME,
            Self::Navigation => crate::navigation_preferences::FILE_NAME,
            Self::Shortcuts => crate::shortcuts::FILE_NAME,
        }
    }

    fn lock_name(self) -> &'static str {
        match self {
            Self::Grid => ".grid-defaults.lock",
            Self::Preview => ".preview-defaults.lock",
            Self::Navigation => ".navigation-defaults.lock",
            Self::Shortcuts => ".shortcuts.lock",
        }
    }

    fn state(self, session: &Session) -> Value {
        match self {
            Self::Grid => session.grid_preferences_state(),
            Self::Preview => session.preview_preferences_state(),
            Self::Navigation => session.navigation_preferences_state(),
            Self::Shortcuts => session.keyboard_shortcuts_state(),
        }
    }

    fn action(self, session: &mut Session, action: &str) -> Result<Value, String> {
        match self {
            Self::Grid => session.change_grid_preferences(
                Actor::Agent,
                action,
                &session.grid_preferences_stamp(),
            ),
            Self::Preview => session.change_preview_preferences(
                Actor::Agent,
                action,
                &session.preview_preferences_stamp(),
            ),
            Self::Navigation => session.change_navigation_preferences(
                Actor::Agent,
                action,
                &session.navigation_preferences_stamp(),
            ),
            Self::Shortcuts => session.change_keyboard_shortcuts(
                Actor::Agent,
                &session.shortcuts_stamp(),
                action,
                (action == "save").then(crate::shortcuts::Bindings::default),
            ),
        }
    }

    fn observe(self, directory: &Path) -> ObservedSource {
        match self {
            Self::Grid => crate::grid_preferences::Store {
                directory: directory.into(),
            }
            .read_source(),
            Self::Preview => crate::preview_preferences::Store {
                directory: directory.into(),
            }
            .read_source(),
            Self::Navigation => crate::navigation_preferences::Store {
                directory: directory.into(),
            }
            .read_source(),
            Self::Shortcuts => crate::shortcuts::Store {
                directory: directory.into(),
            }
            .read_source(),
        }
        .unwrap()
    }

    fn reset_checked(
        self,
        directory: &Path,
        source: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        match self {
            Self::Grid => crate::grid_preferences::Store {
                directory: directory.into(),
            }
            .save_checked(Default::default(), true, source),
            Self::Preview => crate::preview_preferences::Store {
                directory: directory.into(),
            }
            .save_checked(Default::default(), true, source),
            Self::Navigation => crate::navigation_preferences::Store {
                directory: directory.into(),
            }
            .save_checked(Default::default(), true, source),
            Self::Shortcuts => crate::shortcuts::Store {
                directory: directory.into(),
            }
            .save_checked(Default::default(), source),
        }
    }
}

#[cfg(unix)]
#[test]
fn missing_sources_reject_alias_retarget_and_directory_replacement_until_explicit_reload() {
    use std::os::unix::fs::symlink;
    for kind in Kind::ALL {
        for replace in [false, true] {
            for action in ["save", "apply", "reset"] {
                if matches!(kind, Kind::Shortcuts) && action == "apply" {
                    continue;
                }
                let directory = tempfile::tempdir().unwrap();
                let original = directory.path().join("original");
                let other = directory.path().join("other");
                fs::create_dir(&original).unwrap();
                fs::create_dir(&other).unwrap();
                let requested = directory.path().join("requested");
                symlink(&original, &requested).unwrap();
                let mut session = Session::new(Err("no assets needed".into()));
                session.configure_recovery(Ok(requested.clone()));
                let draft = session
                    .preview_measurement(
                        Actor::User,
                        crate::session::measurements::Request {
                            id: None,
                            name: "retained review".into(),
                            start: glam::DVec3::ZERO,
                            end: glam::DVec3::X,
                        },
                        &session.measurement_stamp(),
                    )
                    .unwrap()["draft"]
                    .clone();
                let source = session.document().vmf().clone();
                let camera = session.camera();
                let current: Vec<_> = Kind::ALL
                    .map(|kind| kind.state(&session)["current"].clone())
                    .into();
                if replace {
                    fs::rename(&original, directory.path().join("old-directory")).unwrap();
                    fs::create_dir(&original).unwrap();
                } else {
                    fs::remove_file(&requested).unwrap();
                    symlink(&other, &requested).unwrap();
                }
                let error = kind.action(&mut session, action).unwrap_err();
                assert!(
                    error.contains("directory"),
                    "{kind:?}/{replace}/{action}: {error}"
                );
                for _ in 0..2 {
                    assert!(
                        kind.action(&mut session, "reset")
                            .unwrap_err()
                            .contains("directory")
                    );
                }
                assert!(!original.join(kind.file_name()).exists());
                assert!(!other.join(kind.file_name()).exists());
                assert_eq!(
                    Kind::ALL
                        .map(|kind| kind.state(&session)["current"].clone())
                        .to_vec(),
                    current
                );
                assert_eq!(session.document().vmf(), &source);
                assert_eq!(session.document().undo_labels().count(), 0);
                assert_eq!(session.camera(), camera);
                assert_eq!(session.measurement_state()["draft"], draft);
                kind.action(&mut session, "reload").unwrap();
                assert_eq!(
                    kind.action(&mut session, "reset").unwrap()["write_committed"],
                    true
                );
                assert!(requested.join(kind.file_name()).is_file());
                assert_eq!(session.measurement_state()["draft"], draft);
            }
        }
    }
}

#[test]
fn retained_missing_ancestor_accepts_regular_creation_and_independent_store_writes() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("new/config");
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(missing.clone()));
    fs::create_dir_all(&missing).unwrap();
    for kind in Kind::ALL {
        assert_eq!(
            kind.action(&mut session, "save").unwrap()["write_committed"],
            true
        );
        assert!(missing.join(kind.file_name()).is_file());
        assert!(missing.join(kind.lock_name()).is_file());
    }
}

#[test]
fn all_store_publishers_reject_intervening_writer_after_the_retained_observation() {
    for kind in Kind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let expected = kind.observe(directory.path());
        let mut writer = Session::new(Err("no assets needed".into()));
        writer.configure_recovery(Ok(directory.path().into()));
        kind.action(&mut writer, "save").unwrap();
        let path = directory.path().join(kind.file_name());
        let bytes = fs::read(&path).unwrap();
        let error = kind.reset_checked(directory.path(), &expected).unwrap_err();
        assert!(error.conflict, "{kind:?}: {}", error.message);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let reviewed = kind.observe(directory.path());
        assert!(
            kind.reset_checked(directory.path(), &reviewed)
                .unwrap()
                .source
                .is_ok()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn busy_store_locks_preserve_files_and_allow_explicit_recovery_after_release() {
    for kind in Kind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().into()));
        let current = kind.state(&session)["current"].clone();
        let guard = WriteLock::acquire(
            directory.path(),
            kind.lock_name(),
            "test preferences",
            "Reload defaults",
        )
        .unwrap();
        assert!(
            kind.action(&mut session, "save")
                .unwrap_err()
                .contains("locked")
        );
        assert!(!directory.path().join(kind.file_name()).exists());
        assert_eq!(kind.state(&session)["current"], current);
        assert_eq!(kind.state(&session)["ready"], false);
        drop(guard);
        kind.action(&mut session, "reload").unwrap();
        assert_eq!(
            kind.action(&mut session, "save").unwrap()["write_committed"],
            true
        );
    }
}

#[test]
fn oversized_replacement_is_reviewed_before_explicit_reset_for_every_store() {
    for kind in Kind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().into()));
        let path = directory.path().join(kind.file_name());
        let bytes = vec![b'x'; crate::shortcuts::MAX_FILE_BYTES as usize + 1];
        fs::write(&path, &bytes).unwrap();
        let current = kind.state(&session)["current"].clone();
        assert!(kind.action(&mut session, "reset").is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(kind.state(&session)["current"], current);
        assert_eq!(
            kind.action(&mut session, "reset").unwrap()["write_committed"],
            true
        );
        assert_eq!(kind.state(&session)["ready"], true);
    }
}
