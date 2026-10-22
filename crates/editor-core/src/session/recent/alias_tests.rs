use super::*;
use crate::session::compilation::BuildSettingsUpdate;
use std::fs;
use std::os::unix::fs::symlink;

#[test]
fn alias_reopen_and_restart_preserve_the_referring_child_and_compiled_dependency() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game, source, child) = crate::test_assets::instance_map(false);
        let physical_directory = directory.path().join("physical");
        fs::create_dir(&physical_directory).unwrap();
        let physical = physical_directory.join("parent.vmf");
        fs::rename(&source, &physical).unwrap();
        symlink(&physical, &source).unwrap();
        let decoy = physical_directory.join("child.vmf");
        let mut decoy_bytes = fs::read(&child).unwrap();
        decoy_bytes.extend_from_slice(b"\n// Different child identity under the physical parent\n");
        fs::write(&decoy, decoy_bytes).unwrap();
        let preferences = directory.path().join("preferences");
        let content = crate::session::SessionContent::new(Ok(game));
        let mut current = Session::with_shared_content(content.clone());
        current.configure_recent_maps(Ok(preferences.clone()));
        current.open(actor, &source, false).unwrap();
        current
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    bounces: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();
        let options = current.compile_options();
        for path in [&physical, &source] {
            current.open(actor, path, false).unwrap();
            assert_eq!(current.recent_maps_snapshot().entries.len(), 1);
        }
        current.new_map(actor, false).unwrap();
        current.open_recent_map(actor, &physical, false).unwrap();
        assert_eq!(current.path(), Some(source.as_path()));
        let location =
            compiler::instances::SourceLocation::capture(current.path().unwrap()).unwrap();
        assert_eq!(location.reference("child.vmf").unwrap().path(), child);
        current.save(actor, None).unwrap();
        assert!(fs::symlink_metadata(&source).unwrap().is_symlink());
        let parent_bytes = fs::read(&physical).unwrap();
        drop(current);

        let mut restarted = Session::with_shared_content(content);
        restarted.configure_recent_maps(Ok(preferences));
        let report = restarted.recent_maps();
        assert_eq!(report["entries"].as_array().unwrap().len(), 1);
        assert_eq!(report["entries"][0]["path"], physical.display().to_string());
        assert_eq!(
            report["entries"][0]["reopen_path"],
            source.display().to_string()
        );
        restarted.open_recent_map(actor, &physical, false).unwrap();
        assert_eq!(restarted.path(), Some(source.as_path()));
        assert_eq!(restarted.compile_options(), options);
        let vmf = restarted.document().vmf().clone();
        let history = restarted.history_snapshot();
        let child_identity =
            compiler::FileIdentity::capture(&child, &compiler::Control::default()).unwrap();
        let decoy_identity =
            compiler::FileIdentity::capture(&decoy, &compiler::Control::default()).unwrap();
        assert_ne!(child_identity.sha256(), decoy_identity.sha256());
        let result = restarted.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        let instances = output.stats.instances.as_ref().unwrap();
        assert_eq!(instances.dependencies.len(), 1);
        assert_eq!(instances.dependencies[0].path, "child.vmf");
        assert_eq!(
            Some(instances.dependencies[0].sha256.as_str()),
            child_identity.sha256()
        );
        restarted.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert!(source.with_extension("bsp").is_file());
        assert!(!physical.with_extension("bsp").exists());
        assert_eq!(restarted.document().vmf(), &vmf);
        assert_eq!(restarted.history_snapshot(), history);
        assert_eq!(fs::read(&physical).unwrap(), parent_bytes);
    }
}

#[test]
fn conflicting_alias_updates_preserve_the_saved_reopen_path_until_reload_and_retry() {
    let directory = tempfile::tempdir().unwrap();
    let physical = directory.path().join("parent.vmf");
    fs::write(&physical, "world { id 1 classname worldspawn }").unwrap();
    let first_alias = directory.path().join("first.vmf");
    let second_alias = directory.path().join("second.vmf");
    symlink(&physical, &first_alias).unwrap();
    symlink(&physical, &second_alias).unwrap();
    let preferences = directory.path().join("preferences");
    let mut stale = Session::new(Err("no assets needed".into()));
    stale.configure_recent_maps(Ok(preferences.clone()));
    stale.open(Actor::User, &physical, false).unwrap();
    let mut writer = Session::new(Err("no assets needed".into()));
    writer.configure_recent_maps(Ok(preferences.clone()));
    writer.open(Actor::Agent, &first_alias, false).unwrap();
    writer
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                bounces: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
    let storage = preferences.join(recent_maps::FILE_NAME);
    let saved = fs::read(&storage).unwrap();
    stale.open(Actor::User, &second_alias, false).unwrap();
    assert_eq!(stale.path(), Some(second_alias.as_path()));
    assert_eq!(stale.recent_maps()["persistence"], "blocked");
    assert_eq!(fs::read(&storage).unwrap(), saved);
    stale
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bounces: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    stale
        .preview_block(glam::DVec3::ZERO, glam::DVec3::splat(16.0))
        .unwrap();
    let preview = stale.block_preview();
    let source = stale.document().vmf().clone();
    let history = stale.history_snapshot();
    let selected = stale.selected_ids();
    let camera = stale.camera();
    let settings = stale.compile_options();
    stale.reload_recent_maps().unwrap();
    assert_eq!(
        stale.recent_maps_snapshot().entries[0].reopen_path(),
        first_alias
    );
    assert_eq!(stale.block_preview(), preview);
    assert_eq!(stale.document().vmf(), &source);
    assert_eq!(stale.history_snapshot(), history);
    assert_eq!(stale.selected_ids(), selected);
    assert_eq!(stale.camera(), camera);
    assert_eq!(stale.compile_options(), settings);
    stale.cancel_block_preview();
    stale.compiling = true;
    stale.open(Actor::User, &second_alias, false).unwrap();
    stale.compiling = false;
    let entries = stale.recent_maps_snapshot().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reopen_path(), second_alias);
    assert_eq!(entries[0].build_settings.bounces, 3);
    assert_eq!(stale.compile_options(), settings);
    writer.reload_recent_maps().unwrap();
    writer
        .open_recent_map(Actor::Agent, &physical, false)
        .unwrap();
    assert_eq!(writer.path(), Some(second_alias.as_path()));

    let authority = Authority::configured(Ok(preferences));
    let mut first = Session::with_recent_maps(Err("no assets needed".into()), authority.clone());
    let mut second = Session::with_recent_maps(Err("no assets needed".into()), authority.clone());
    first.open(Actor::User, &first_alias, false).unwrap();
    second.open(Actor::Agent, &second_alias, false).unwrap();
    first
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bounces: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
    let entries = authority.snapshot().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reopen_path(), second_alias);
    assert_eq!(entries[0].build_settings.bounces, 4);
    second.new_map(Actor::Agent, false).unwrap();
    second
        .open_recent_map(Actor::Agent, &physical, false)
        .unwrap();
    assert_eq!(second.path(), Some(second_alias.as_path()));
    let persisted = recent_maps::Store {
        directory: authority.snapshot().directory.unwrap(),
    }
    .load()
    .unwrap();
    assert_eq!(persisted, entries);
}

#[test]
fn missing_or_retargeted_reopen_alias_preserves_the_current_map_and_recent_list() {
    let directory = tempfile::tempdir().unwrap();
    let physical = directory.path().join("parent.vmf");
    let other = directory.path().join("other.vmf");
    for path in [&physical, &other] {
        fs::write(path, "world { id 1 classname worldspawn }").unwrap();
    }
    let alias = directory.path().join("alias.vmf");
    symlink(&physical, &alias).unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.open(Actor::User, &alias, false).unwrap();
    session.new_map(Actor::User, false).unwrap();
    let before = session.document().vmf().clone();
    let history = session.history_snapshot();
    let entries = session.recent_maps_snapshot().entries;
    fs::remove_file(&alias).unwrap();
    assert_eq!(session.recent_maps()["entries"][0]["available"], false);
    assert!(
        session
            .open_recent_map(Actor::Agent, &physical, false)
            .is_err()
    );
    symlink(&other, &alias).unwrap();
    assert!(
        session
            .open_inner(Actor::Agent, &alias, false, true, Some(&physical))
            .unwrap_err()
            .contains("changed its destination")
    );
    assert!(
        session
            .open_recent_map(Actor::Agent, &physical, false)
            .unwrap_err()
            .contains("changed its destination")
    );
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.recent_maps_snapshot().entries, entries);
    assert!(session.path().is_none());
}
