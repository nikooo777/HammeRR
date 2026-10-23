use super::*;
use crate::grid_preferences::Grid;
use crate::navigation_preferences::Navigation;
use crate::pivot_preferences::Pivot;
use crate::preview_preferences::Preview;
use crate::session::{Actor, Session, selection_filters::SelectionFilters};
use glam::DVec3;
use source_formats::keyvalues::WriteOptions;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Grid,
    Preview,
    Navigation,
    Selection,
    Pivot,
}
impl Kind {
    const ALL: [Self; 5] = [
        Self::Grid,
        Self::Preview,
        Self::Navigation,
        Self::Selection,
        Self::Pivot,
    ];
    fn state(self, session: &Session) -> Value {
        match self {
            Self::Grid => session.grid_preferences_state(),
            Self::Preview => session.preview_preferences_state(),
            Self::Navigation => session.navigation_preferences_state(),
            Self::Selection => session.selection_preferences_state(),
            Self::Pivot => session.pivot_preferences_state(),
        }
    }
    fn action(
        self,
        session: &mut Session,
        action: &str,
        snapshot: Option<&str>,
    ) -> Result<Value, String> {
        let stamp = self.state(session)["snapshot"].as_str().unwrap().to_owned();
        let snapshot = snapshot.unwrap_or(&stamp);
        match self {
            Self::Grid => session.change_grid_preferences(Actor::Agent, action, snapshot),
            Self::Preview => session.change_preview_preferences(Actor::Agent, action, snapshot),
            Self::Navigation => {
                session.change_navigation_preferences(Actor::Agent, action, snapshot)
            }
            Self::Selection => session.change_selection_preferences(Actor::Agent, action, snapshot),
            Self::Pivot => session.change_pivot_preferences(Actor::Agent, action, snapshot),
        }
    }
}

fn session(defaults: &SavedDefaults) -> Session {
    Session::with_saved_defaults(Err("no assets needed".into()), defaults.clone())
}

fn current(session: &Session) -> [Value; 5] {
    Kind::ALL.map(|kind| kind.state(session)["current"].clone())
}

fn set_current(session: &mut Session, grid: f64, speed: f32, outlines: bool) {
    session
        .set_transform_tool(
            Actor::User,
            crate::session::TransformTool {
                pivot: Some(DVec3::new(grid, 0.0, 0.0)),
                ..session.transform_tool()
            },
        )
        .unwrap();
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(grid),
                ..Default::default()
            },
        )
        .unwrap();
    session.set_fly_speed(speed).unwrap();
    let view = session
        .resolve_view_update(crate::session::ViewUpdate {
            selection_outlines: Some(outlines),
            ..Default::default()
        })
        .unwrap();
    session.set_view(view);
    session
        .set_selection_filters(
            Actor::User,
            SelectionFilters {
                world_brushes: outlines,
                ..Default::default()
            },
        )
        .unwrap();
}

fn source_state(session: &Session) -> Value {
    let doc = session.document();
    json!({"vmf":doc.vmf().write(&WriteOptions::default()),"rendered_vmf":session.render_document().vmf().write(&WriteOptions::default()),"revision":doc.revision(),"generation":session.document_generation(),
        "dirty":doc.is_dirty(),"undo":doc.undo_labels().collect::<Vec<_>>(),"redo":doc.redo_labels().collect::<Vec<_>>(),
        "selection":format!("{:?}",session.selected_ids()),"camera":format!("{:?}",session.camera()),"scene":session.scene_serial(),
        "view":session.view_serial(),"camera_serial":session.camera_serial(),"measurements":session.measurement_state()})
}

fn owned_pick(session: &mut Session) {
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "tools/toolsnodraw",
            true,
        )
        .unwrap();
    let draft = session
        .preview_measurement(
            Actor::Agent,
            crate::session::measurements::Request {
                id: None,
                name: "Shared default isolation".into(),
                start: DVec3::new(-64.0, 0.0, 0.0),
                end: DVec3::X,
            },
            &session.measurement_stamp(),
        )
        .unwrap();
    session
        .begin_measurement_pick(
            Actor::Agent,
            draft["draft"]["token"].as_u64().unwrap(),
            crate::session::measurements::Endpoint::End,
            session.camera(),
            (128, 128),
            &session.measurement_pick_stamp(),
        )
        .unwrap();
    assert_eq!(
        session.measurement_state()["draft"]["picking"]["valid"],
        true
    );
}

#[test]
fn attached_sessions_share_saved_values_and_errors_without_changing_current_values_or_owned_picks()
{
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let mut first = session(&defaults);
    let mut second = session(&defaults);
    set_current(&mut first, 17.375, 987.625, false);
    set_current(&mut second, 31.5, 1500.0, true);
    let brush = first
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "tools/toolsnodraw",
            true,
        )
        .unwrap();
    first
        .preview_transform(
            Actor::User,
            &[brush],
            map_core::edit::Transform {
                translation: DVec3::Y * 16.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let first_source = source_state(&first);
    assert_ne!(first.document().vmf(), first.render_document().vmf());
    owned_pick(&mut second);
    let source = source_state(&second);
    let local = current(&second);
    for kind in Kind::ALL {
        let old = kind.state(&second)["snapshot"].as_str().unwrap().to_owned();
        let result = kind.action(&mut first, "save", None).unwrap();
        let shared = kind.state(&second);
        assert_eq!(shared["saved"], result["saved"]);
        assert_ne!(shared["snapshot"], old);
        assert!(kind.action(&mut second, "reset", Some(&old)).is_err());
        assert_eq!(source_state(&second), source);
        assert_eq!(current(&second), local);
        if matches!(kind, Kind::Grid | Kind::Preview | Kind::Pivot) {
            assert!(kind.action(&mut second, "apply", None).is_err());
            assert_eq!(kind.state(&first)["ready"], true);
            assert_eq!(kind.state(&second)["ready"], true);
        }
        kind.action(&mut second, "reload", None).unwrap();
        assert_eq!(current(&second), local);
        kind.action(&mut first, "reset", None).unwrap();
        assert_eq!(
            kind.state(&second)["defaults"],
            kind.state(&second)["factory_defaults"]
        );
        assert_eq!(current(&second), local);
        assert_eq!(source_state(&second), source);
        assert_eq!(source_state(&first), first_source);
    }
    Kind::Navigation.action(&mut second, "apply", None).unwrap();
    assert_eq!(second.fly_speed(), 600.0);
    assert_eq!(source_state(&second), source);
}

#[test]
fn constructors_apply_retained_startup_once_and_attached_recovery_cannot_rebind_or_reapply() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("shared");
    crate::grid_preferences::Store {
        directory: root.clone(),
    }
    .save(
        Grid {
            grid_size: 17.375,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    crate::preview_preferences::Store {
        directory: root.clone(),
    }
    .save(
        Preview {
            selection_outlines: false,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    crate::navigation_preferences::Store {
        directory: root.clone(),
    }
    .save(Navigation { fly_speed: 987.625 }, false)
    .unwrap();
    crate::selection_preferences::Store {
        directory: root.clone(),
    }
    .save(
        SelectionFilters {
            world_brushes: false,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let defaults = SavedDefaults::configured(Ok(root.clone()));
    let mut first = session(&defaults);
    assert_eq!(current(&first)[0]["grid_size"], 17.375);
    assert_eq!(current(&first)[1]["selection_outlines"], false);
    assert_eq!(first.fly_speed(), 987.625);
    assert!(!first.selection_filters().world_brushes);
    set_current(&mut first, 31.5, 1500.0, true);
    let other = directory.path().join("other");
    first.configure_recovery(Ok(other.clone()));
    first.configure_recovery(Err("document recovery unavailable".into()));
    assert_eq!(current(&first)[0]["grid_size"], 31.5);
    assert_eq!(current(&first)[1]["selection_outlines"], true);
    assert_eq!(first.fly_speed(), 1500.0);
    for kind in Kind::ALL {
        assert!(
            kind.state(&first)["path"]
                .as_str()
                .unwrap()
                .starts_with(root.to_str().unwrap())
        );
        kind.action(&mut first, "save", None).unwrap();
    }
    assert!(!other.join(crate::grid_preferences::FILE_NAME).exists());
    let later = session(&defaults);
    assert_eq!(current(&later), current(&first));
    let mut independent = Session::new(Err("no assets needed".into()));
    independent.configure_recovery(Ok(other));
    assert_eq!(independent.fly_speed(), 600.0);
    assert_eq!(independent.grid_preferences_state()["stored"], false);
    assert_ne!(
        independent.grid_preferences_state()["path"],
        first.grid_preferences_state()["path"]
    );
}

#[test]
fn shared_reload_retains_reviewed_bad_bytes_then_explicit_reset_recovers_for_both_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let mut first = session(&defaults);
    let mut second = session(&defaults);
    set_current(&mut second, 31.5, 1500.0, false);
    let local = current(&second);
    for kind in Kind::ALL {
        let path = PathBuf::from(kind.state(&first)["path"].as_str().unwrap());
        let bytes = vec![b'x'; 4097];
        std::fs::write(&path, &bytes).unwrap();
        let stale = kind.state(&second)["snapshot"].as_str().unwrap().to_owned();
        assert!(kind.action(&mut first, "reload", None).is_err());
        assert_eq!(kind.state(&first)["error"], kind.state(&second)["error"]);
        assert_eq!(kind.state(&second)["ready"], false);
        assert!(kind.action(&mut second, "reset", Some(&stale)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            kind.action(&mut second, "reset", None).unwrap()["write_committed"],
            true
        );
        assert_eq!(kind.state(&first)["ready"], true);
        assert_eq!(current(&second), local);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn concurrent_authority_writers_admit_one_reviewed_revision_and_reject_the_loser_before_publication()
 {
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let revision = defaults.grid.snapshot().serial;
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers = [17.375, 31.5].map(|grid_size| {
        let authority = defaults.grid.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            (
                grid_size,
                authority.save(
                    revision,
                    "stale reviewed defaults",
                    Grid {
                        grid_size,
                        ..Default::default()
                    },
                    false,
                ),
            )
        })
    });
    let results = workers.map(|worker| worker.join().unwrap());
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|(_, result)| result
                .as_ref()
                .is_err_and(|error| error == "stale reviewed defaults"))
            .count(),
        1
    );
    let winner = results.iter().find(|(_, result)| result.is_ok()).unwrap().0;
    assert_eq!(defaults.grid.snapshot().saved.unwrap().grid_size, winner);
    assert_eq!(
        crate::grid_preferences::Store {
            directory: directory.path().into()
        }
        .load()
        .unwrap()
        .unwrap()
        .grid_size,
        winner
    );
    assert!(
        defaults
            .grid
            .checked_value(revision, "stale reviewed defaults")
            .is_err()
    );
    assert!(
        defaults
            .grid
            .reload(revision, "stale reviewed defaults")
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn committed_verification_error_is_shared_and_retains_original_directory_for_all_defaults() {
    use std::os::unix::fs::symlink;
    for kind in Kind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original");
        let other = directory.path().join("other");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&other).unwrap();
        symlink(&original, &alias).unwrap();
        let defaults = SavedDefaults::configured(Ok(alias.clone()));
        let mut first = session(&defaults);
        let mut second = session(&defaults);
        owned_pick(&mut second);
        let before = source_state(&second);
        let local = current(&second);
        let (filename, lockname, expected, data) = match kind {
            Kind::Grid => (
                crate::grid_preferences::FILE_NAME,
                ".grid-defaults.lock",
                defaults.grid.snapshot().source.unwrap(),
                json!({"version":1,"grid":Grid{grid_size:17.375,..Default::default()}}),
            ),
            Kind::Preview => (
                crate::preview_preferences::FILE_NAME,
                ".preview-defaults.lock",
                defaults.preview.snapshot().source.unwrap(),
                json!({"version":1,"preview":Preview{selection_outlines:false,..Default::default()}}),
            ),
            Kind::Navigation => (
                crate::navigation_preferences::FILE_NAME,
                ".navigation-defaults.lock",
                defaults.navigation.snapshot().source.unwrap(),
                json!({"version":1,"navigation":Navigation{fly_speed:987.625}}),
            ),
            Kind::Pivot => (
                crate::pivot_preferences::FILE_NAME,
                ".pivot-defaults.lock",
                defaults.pivot.snapshot().source.unwrap(),
                json!({"version":1,"pivot":Pivot::Custom { point: [1.25, 2.5, 3.75] }}),
            ),
            Kind::Selection => (
                crate::selection_preferences::FILE_NAME,
                ".selection-defaults.lock",
                defaults.selection.snapshot().source.unwrap(),
                json!({"version":1,"filters":SelectionFilters{world_brushes:false,..Default::default()}}),
            ),
        };
        let target = crate::preference_file::CoordinatedFile {
            directory: &alias,
            file_name: filename,
            lock_name: lockname,
            label: "Saved defaults",
            reload: "Reload defaults",
            saved_description: "saved defaults",
            max_bytes: 4096,
        };
        let bytes = serde_json::to_vec(&data).unwrap();
        let publication = target
            .publish_with(&bytes, &expected, |path, data| {
                crate::recovery::atomic_write(path, data)?;
                std::fs::remove_file(&alias).map_err(|error| error.to_string())?;
                symlink(&other, &alias).map_err(|error| error.to_string())
            })
            .unwrap();
        assert!(publication.source.is_err());
        match kind {
            Kind::Grid => defaults.grid.accept(
                Grid {
                    grid_size: 17.375,
                    ..Default::default()
                },
                publication,
            ),
            Kind::Preview => defaults.preview.accept(
                Preview {
                    selection_outlines: false,
                    ..Default::default()
                },
                publication,
            ),
            Kind::Navigation => defaults
                .navigation
                .accept(Navigation { fly_speed: 987.625 }, publication),
            Kind::Pivot => defaults.pivot.accept(
                Pivot::Custom {
                    point: [1.25, 2.5, 3.75],
                },
                publication,
            ),
            Kind::Selection => defaults.selection.accept(
                SelectionFilters {
                    world_brushes: false,
                    ..Default::default()
                }
                .into(),
                publication,
            ),
        }
        assert_eq!(kind.state(&first)["ready"], false);
        assert_eq!(kind.state(&first)["error"], kind.state(&second)["error"]);
        assert!(
            kind.state(&second)["error"]
                .as_str()
                .unwrap()
                .contains("were written")
        );
        assert_eq!(
            kind.state(&second)["saved"],
            data[match kind {
                Kind::Grid => "grid",
                Kind::Preview => "preview",
                Kind::Navigation => "navigation",
                Kind::Selection => "filters",
                Kind::Pivot => "pivot",
            }]
        );
        for _ in 0..2 {
            assert!(
                kind.action(&mut first, "reset", None)
                    .unwrap_err()
                    .contains("directory")
            );
        }
        assert!(!other.join(filename).exists());
        assert_eq!(std::fs::read(original.join(filename)).unwrap(), bytes);
        assert_eq!(current(&second), local);
        assert_eq!(source_state(&second), before);
        kind.action(&mut second, "reload", None).unwrap();
        assert_eq!(
            kind.action(&mut second, "reset", None).unwrap()["write_committed"],
            true
        );
        assert_eq!(kind.state(&first)["ready"], true);
    }
}

#[cfg(unix)]
#[test]
fn failed_publication_and_reload_share_errors_without_changing_saved_bytes_or_current_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let mut first = session(&defaults);
    let mut second = session(&defaults);
    set_current(&mut first, 17.375, 987.625, false);
    set_current(&mut second, 31.5, 1500.0, true);
    owned_pick(&mut second);
    let source = source_state(&second);
    let local = current(&second);
    for kind in Kind::ALL {
        kind.action(&mut first, "save", None).unwrap();
        let saved = kind.state(&first)["saved"].clone();
        let path = PathBuf::from(kind.state(&first)["path"].as_str().unwrap());
        let bytes = std::fs::read(&path).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        assert!(
            kind.action(&mut first, "save", None)
                .unwrap_err()
                .contains("read-only")
        );
        assert_eq!(kind.state(&second)["ready"], false);
        assert_eq!(kind.state(&second)["error"], kind.state(&first)["error"]);
        assert_eq!(kind.state(&second)["saved"], saved);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(current(&second), local);
        assert_eq!(source_state(&second), source);
        std::fs::set_permissions(&path, permissions).unwrap();
        kind.action(&mut second, "reload", None).unwrap();
        assert_eq!(kind.state(&first)["ready"], true);
        assert_eq!(kind.state(&second)["saved"], saved);
        assert_eq!(current(&second), local);
        assert_eq!(source_state(&second), source);
    }
}
