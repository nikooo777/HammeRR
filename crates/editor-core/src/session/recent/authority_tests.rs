use super::*;
use crate::recent_maps::Store;
use crate::session::compilation::BuildSettingsUpdate;
use glam::DVec3;
use std::fs;

fn map_file(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, "world { id 1 classname worldspawn }").unwrap();
    path
}

fn mounted(directory: &Path) -> Result<source_formats::gamefs::GameFs, String> {
    fs::create_dir_all(directory).unwrap();
    Ok(source_formats::gamefs::GameFs {
        game_dir: directory.to_owned(),
        base_dir: directory.to_owned(),
        mounts: Vec::new(),
    })
}

fn bounces(session: &mut Session, value: u8) {
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                bounces: Some(value),
                ..Default::default()
            },
        )
        .unwrap();
}

#[test]
fn attached_sessions_share_history_without_rebinding_recovery_or_current_build_settings() {
    let directory = tempfile::tempdir().unwrap();
    let storage = directory.path().join("shared");
    let authority = Authority::configured(Ok(storage.clone()));
    let game = directory.path().join("game");
    let mut first = Session::with_recent_maps(mounted(&game), authority.clone());
    let mut second = Session::with_recent_maps(mounted(&game), authority.clone());
    first.configure_recovery(Ok(directory.path().join("first-recovery")));
    second.configure_recovery(Ok(directory.path().join("second-recovery")));
    let a = map_file(directory.path(), "a.vmf");
    let b = map_file(directory.path(), "b.vmf");
    first.open(Actor::User, &a, false).unwrap();
    bounces(&mut first, 2);
    second.open(Actor::Agent, &b, false).unwrap();
    bounces(&mut second, 3);
    assert_eq!(first.compile_options().bounces, 2);
    assert_eq!(second.compile_options().bounces, 3);
    assert_eq!(
        first.recent_maps()["entries"],
        second.recent_maps()["entries"]
    );
    assert_eq!(
        first.recent_maps()["directory"],
        storage.display().to_string()
    );
    assert_eq!(first.recent_maps_snapshot().entries.len(), 2);
    let revision = first.recent_maps_serial();
    second.configure_recovery(Err("unrelated recovery failure".into()));
    second.configure_recent_maps(Ok(directory.path().join("unrelated-store")));
    assert_eq!(first.recent_maps_serial(), revision);
    assert_eq!(second.recent_maps_serial(), revision);
    first.open_recent_map(Actor::User, &b, false).unwrap();
    assert_eq!(first.compile_options().bounces, 3);
    assert_eq!(second.path(), Some(b.as_path()));
    let frozen = first.recent_maps_snapshot();
    bounces(&mut second, 4);
    assert_eq!(first.compile_options().bounces, 3);
    assert_eq!(
        first.recent_maps_with_snapshot(&frozen)["entries"][0]["build_settings"]["bounces"],
        3
    );
    assert_eq!(
        first.recent_maps()["entries"][0]["build_settings"]["bounces"],
        4
    );
    first.remove_recent_map(&a).unwrap();
    assert_eq!(second.recent_maps_snapshot().entries.len(), 1);
    drop(first);
    second.clear_recent_maps().unwrap();
    assert!(authority.snapshot().entries.is_empty());
    assert_eq!(Store { directory: storage }.load().unwrap(), Vec::new());
    for name in ["first-recovery", "second-recovery", "unrelated-store"] {
        assert!(
            !directory
                .path()
                .join(name)
                .join(recent_maps::FILE_NAME)
                .exists()
        );
    }
}

#[test]
fn shared_entries_restore_only_the_originating_compatible_idle_session() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::default();
    let mut first =
        Session::with_recent_maps(mounted(&directory.path().join("game-a")), authority.clone());
    let mut second =
        Session::with_recent_maps(mounted(&directory.path().join("game-b")), authority);
    let map = map_file(directory.path(), "a.vmf");
    first.open(Actor::User, &map, false).unwrap();
    bounces(&mut first, 2);
    bounces(&mut second, 1);
    let retained = first.recent_maps_snapshot().entries[0].clone();
    second.open_recent_map(Actor::Agent, &map, false).unwrap();
    assert_eq!(second.compile_options().bounces, 1);
    assert_eq!(first.compile_options().bounces, 2);
    assert_eq!(first.recent_maps_snapshot().entries[0], retained);
    assert_eq!(
        first.recent_maps()["entries"][0]["context_compatible"],
        true
    );
    assert_eq!(
        second.recent_maps()["entries"][0]["context_compatible"],
        false
    );
    second.save(Actor::User, None).unwrap();
    assert_eq!(
        first.recent_maps_snapshot().entries[0]
            .build_settings
            .bounces,
        1
    );
    first.compiling = true;
    first.open_recent_map(Actor::User, &map, false).unwrap();
    assert_eq!(first.compile_options().bounces, 2);
    assert_eq!(
        first.recent_maps_snapshot().entries[0]
            .build_settings
            .bounces,
        1
    );
    first.compiling = false;
    assert_eq!(second.compile_options().bounces, 1);
}

#[test]
fn shared_storage_errors_reload_and_primary_success_preserve_other_owned_document_state() {
    let directory = tempfile::tempdir().unwrap();
    let storage = directory.path().join("shared");
    let authority = Authority::configured(Ok(storage.clone()));
    let mut first = Session::with_recent_maps(Err("no assets".into()), authority.clone());
    let mut second = Session::with_recent_maps(Err("no assets".into()), authority);
    let a = map_file(directory.path(), "a.vmf");
    let b = map_file(directory.path(), "b.vmf");
    first.open(Actor::User, &a, false).unwrap();
    second
        .preview_measurement(
            Actor::Agent,
            crate::session::measurements::Request {
                id: None,
                name: "Owned clearance".into(),
                start: DVec3::ZERO,
                end: DVec3::X,
            },
            &second.measurement_stamp(),
        )
        .unwrap();
    let source = second.document().vmf().clone();
    let history = second.history_snapshot();
    let draft = second.measurement_state()["draft"].clone();
    let camera = second.camera();
    let serials = (
        second.scene_serial(),
        second.view_serial(),
        second.camera_serial(),
    );
    let mut outside = Session::new(Err("independent writer".into()));
    outside.configure_recent_maps(Ok(storage.clone()));
    outside.open(Actor::Agent, &b, false).unwrap();
    let path = storage.join(recent_maps::FILE_NAME);
    let saved = fs::read(&path).unwrap();
    assert!(first.remove_recent_map(&a).is_err());
    assert_eq!(second.recent_maps()["persistence"], "blocked");
    assert_eq!(first.recent_maps()["error"], second.recent_maps()["error"]);
    assert_eq!(fs::read(&path).unwrap(), saved);
    second.reload_recent_maps().unwrap();
    assert_eq!(first.recent_maps_snapshot().entries.len(), 2);
    fs::write(&path, b"broken").unwrap();
    assert!(second.reload_recent_maps().is_err());
    assert_eq!(first.recent_maps()["persistence"], "blocked");
    assert_eq!(first.recent_maps_snapshot().entries.len(), 2);
    first.open(Actor::User, &b, false).unwrap();
    let copy = directory.path().join("copy.vmf");
    first.save(Actor::User, Some(&copy)).unwrap();
    assert!(copy.is_file());
    assert!(!first.document().is_dirty());
    assert_eq!(first.path(), Some(copy.as_path()));
    assert_eq!(fs::read(&path).unwrap(), b"broken");
    fs::write(&path, &saved).unwrap();
    first.reload_recent_maps().unwrap();
    assert!(second.recent_maps()["error"].is_null());
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.history_snapshot(), history);
    assert_eq!(second.measurement_state()["draft"], draft);
    assert_eq!(second.camera(), camera);
    assert_eq!(
        (
            second.scene_serial(),
            second.view_serial(),
            second.camera_serial()
        ),
        serials
    );
}

#[test]
fn concurrent_document_remembering_retains_every_entry_with_one_authority() {
    let directory = tempfile::tempdir().unwrap();
    let storage = directory.path().join("shared");
    let authority = Authority::configured(Ok(storage.clone()));
    let paths: Vec<_> = (0..16)
        .map(|index| map_file(directory.path(), &format!("map-{index}.vmf")))
        .collect();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for paths in paths.chunks(8) {
        let paths = paths.to_vec();
        let authority = authority.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut session = Session::with_recent_maps(Err("no assets".into()), authority);
            barrier.wait();
            for path in paths {
                session.open(Actor::Agent, &path, false).unwrap();
            }
            assert!(session.recent_maps()["error"].is_null());
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let loaded = Store { directory: storage }.load().unwrap();
    assert_eq!(loaded.len(), 16);
    assert_eq!(loaded, authority.snapshot().entries);
    for path in paths {
        assert!(loaded.iter().any(|entry| entry.path == path));
    }
}
