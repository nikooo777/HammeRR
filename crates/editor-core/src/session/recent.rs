use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::recent_maps::{self, BuildSettings, Entry, Store};

use super::compilation::BuildSettingsUpdate;
use super::{Actor, Session};

#[derive(Default)]
pub(super) struct State {
    entries: Vec<Entry>,
    store: Option<Store>,
    load_failed: bool,
    error: Option<String>,
    serial: u64,
}

impl Session {
    pub fn configure_recent_maps(&mut self, directory: Result<PathBuf, String>) {
        let store = directory.map(|directory| Store { directory });
        let loaded = match &store {
            Ok(store) => store.load(),
            Err(error) => Err(error.clone()),
        };
        self.recent_maps.store = store.ok();
        match loaded {
            Ok(entries) => {
                self.recent_maps.entries = entries;
                self.recent_maps.load_failed = false;
                self.recent_maps.error = None;
                self.recent_maps.serial += 1;
            }
            Err(error) => {
                self.recent_maps.load_failed = true;
                self.recent_map_error(format!("Recent maps could not load: {error}"));
            }
        }
    }

    pub fn recent_maps_serial(&self) -> u64 {
        self.recent_maps.serial
    }

    fn recent_game_directory(&self) -> Option<PathBuf> {
        if self.game_error.is_some() {
            return None;
        }
        let path = if self.game.game_dir.is_absolute() {
            self.game.game_dir.clone()
        } else {
            std::env::current_dir().ok()?.join(&self.game.game_dir)
        };
        Some(std::fs::canonicalize(&path).unwrap_or(path))
    }

    pub fn recent_maps(&self) -> Value {
        let game_directory = self.recent_game_directory();
        json!({
            "persistence": if self.recent_maps.load_failed { "blocked" } else if self.recent_maps.store.is_some() { "persistent" } else { "session" },
            "directory": self.recent_maps.store.as_ref().map(|store| &store.directory),
            "error": self.recent_maps.error,
            "max_maps": recent_maps::MAX_RECENT_MAPS,
            "max_path_bytes": recent_maps::MAX_PATH_BYTES,
            "max_file_bytes": recent_maps::MAX_FILE_BYTES,
            "entries": self.recent_maps.entries.iter().map(|entry| json!({
                "path": entry.path,
                "title": entry.path.file_name().map(|name| name.to_string_lossy()),
                "available": entry.path.is_file(),
                "game_directory": entry.game_directory,
                "context_compatible": entry.game_directory == game_directory,
                "build_settings": entry.build_settings,
            })).collect::<Vec<_>>(),
        })
    }

    fn recent_map_error(&mut self, error: String) -> String {
        self.recent_maps.error = Some(error.clone());
        self.recent_maps.serial += 1;
        self.log(Actor::System, &error);
        error
    }

    fn ensure_recent_maps_writable(&self) -> Result<(), String> {
        if self.recent_maps.load_failed {
            return Err(format!(
                "{}; fix storage and restart before changing recent maps",
                self.recent_maps
                    .error
                    .as_deref()
                    .unwrap_or("Recent maps did not load")
            ));
        }
        Ok(())
    }

    fn publish_recent_maps(&mut self, entries: Vec<Entry>) -> Result<(), String> {
        self.ensure_recent_maps_writable()?;
        if let Some(store) = &self.recent_maps.store
            && let Err(error) = store.save(&entries)
        {
            return Err(self.recent_map_error(format!("Recent maps could not save: {error}")));
        }
        self.recent_maps.entries = entries;
        self.recent_maps.error = None;
        self.recent_maps.serial += 1;
        Ok(())
    }

    pub(super) fn remember_recent_map(&mut self, path: &Path) {
        self.remember_map(path, true);
    }

    pub(super) fn remember_saved_map(&mut self, path: &Path) {
        self.remember_map(path, false);
    }

    fn remember_map(&mut self, path: &Path, preserve_incompatible: bool) {
        if self.recent_maps.load_failed {
            return;
        }
        let result = self.try_remember_recent_map(path, true, preserve_incompatible);
        if let Err(error) = result
            && self.recent_maps.error.as_deref() != Some(error.as_str())
        {
            self.recent_map_error(error);
        }
    }

    fn try_remember_recent_map(
        &mut self,
        path: &Path,
        reorder: bool,
        preserve_incompatible: bool,
    ) -> Result<(), String> {
        self.ensure_recent_maps_writable()?;
        let path = recent_maps::absolute_path(path)?;
        let existing = self
            .recent_maps
            .entries
            .iter()
            .find(|entry| entry.path == path);
        if !reorder && existing.is_none() {
            return Ok(());
        }
        let entry = if let Some(existing) = existing.filter(|entry| {
            self.compiling
                || (preserve_incompatible && entry.game_directory != self.recent_game_directory())
        }) {
            existing.clone()
        } else {
            Entry {
                path: path.clone(),
                game_directory: self.recent_game_directory(),
                build_settings: BuildSettings::from_options(self.compile_options()),
            }
        };
        let mut entries = self.recent_maps.entries.clone();
        if reorder {
            entries.retain(|entry| entry.path != path);
            entries.insert(0, entry);
            entries.truncate(recent_maps::MAX_RECENT_MAPS);
        } else if let Some(existing) = entries.iter_mut().find(|entry| entry.path == path) {
            *existing = entry;
        }
        if entries == self.recent_maps.entries && self.recent_maps.error.is_none() {
            return Ok(());
        }
        self.publish_recent_maps(entries)
    }

    pub(super) fn remember_current_map_context(&mut self) {
        if self.recent_maps.load_failed {
            return;
        }
        if let Some(path) = self.path.clone() {
            let result = self.try_remember_recent_map(&path, false, false);
            if let Err(error) = result
                && self.recent_maps.error.as_deref() != Some(error.as_str())
            {
                self.recent_map_error(error);
            }
        }
    }

    pub(super) fn restore_recent_map_context(&mut self, path: &Path) {
        if self.compiling || self.closing {
            return;
        }
        let Ok(path) = recent_maps::absolute_path(path) else {
            return;
        };
        let Some(entry) = self
            .recent_maps
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
        else {
            return;
        };
        if entry.game_directory != self.recent_game_directory() {
            self.log(
                Actor::System,
                "Recent map content differs from the mounted game; kept current build settings",
            );
            return;
        }
        let settings = &entry.build_settings;
        let result = self.update_build_settings(
            Actor::System,
            BuildSettingsUpdate {
                lighting: Some(&settings.lighting),
                physics: Some(&settings.physics),
                sky_quality: Some(&settings.sky_quality),
                visibility: Some(&settings.visibility),
                bounces: Some(settings.bounces),
                smoothing_angle: Some(settings.smoothing_angle),
            },
        );
        if let Err(error) = result {
            self.recent_map_error(format!(
                "Recent map build settings could not restore: {error}"
            ));
        }
    }

    pub fn open_recent_map(
        &mut self,
        actor: Actor,
        path: &Path,
        discard_changes: bool,
    ) -> Result<String, String> {
        let entry = self
            .recent_maps
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or("This map is no longer in Recent maps; refresh the list")?;
        let path = entry.path.clone();
        self.open(actor, &path, discard_changes)
    }

    pub fn remove_recent_map(&mut self, path: &Path) -> Result<(), String> {
        self.ensure_recent_maps_writable()?;
        let mut entries = self.recent_maps.entries.clone();
        entries.retain(|entry| entry.path != path);
        if entries.len() == self.recent_maps.entries.len() {
            return Err("This map is no longer in Recent maps; refresh the list".into());
        }
        self.publish_recent_maps(entries)
    }

    pub fn clear_recent_maps(&mut self) -> Result<(), String> {
        self.ensure_recent_maps_writable()?;
        if self.recent_maps.entries.is_empty() && self.recent_maps.error.is_none() {
            return Ok(());
        }
        self.publish_recent_maps(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use glam::DVec3;
    use source_formats::keyvalues::WriteOptions;

    use super::*;

    fn fixture() -> (tempfile::TempDir, Session) {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recent_maps(Ok(directory.path().join("preferences")));
        (directory, session)
    }

    fn map_file(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(
            &path,
            map_core::Document::empty()
                .vmf()
                .write(&WriteOptions::default()),
        )
        .unwrap();
        path
    }

    fn map_state(session: &Session) -> Value {
        json!({
            "vmf": session.document().vmf().write(&WriteOptions::default()),
            "generation": session.document_generation(),
            "revision": session.document().revision(),
            "dirty": session.document().is_dirty(),
            "path": session.path(),
            "selection": format!("{:?}", session.selected_ids()),
            "undo": session.document().undo_labels().collect::<Vec<_>>(),
            "redo": session.document().redo_labels().collect::<Vec<_>>(),
        })
    }

    #[test]
    fn recent_maps_remain_bounded_deduplicated_and_survive_restart() {
        let (directory, mut session) = fixture();
        let mut paths = Vec::new();
        for index in 0..recent_maps::MAX_RECENT_MAPS + 3 {
            let path = map_file(directory.path(), &format!("map-{index}.vmf"));
            session.open(Actor::User, &path, false).unwrap();
            paths.push(path);
        }
        assert_eq!(
            session.recent_maps.entries.len(),
            recent_maps::MAX_RECENT_MAPS
        );
        assert_eq!(session.recent_maps.entries[0].path, *paths.last().unwrap());
        assert!(
            !session
                .recent_maps
                .entries
                .iter()
                .any(|entry| entry.path == paths[0])
        );
        session
            .open_recent_map(Actor::Agent, &paths[4], false)
            .unwrap();
        assert_eq!(session.recent_maps.entries[0].path, paths[4]);
        assert_eq!(
            session
                .recent_maps
                .entries
                .iter()
                .filter(|entry| entry.path == paths[4])
                .count(),
            1
        );
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recent_maps(Ok(directory.path().join("preferences")));
        assert_eq!(reopened.recent_maps.entries, session.recent_maps.entries);
        assert_eq!(reopened.recent_maps()["persistence"], "persistent");
        fs::remove_file(&paths[4]).unwrap();
        assert_eq!(reopened.recent_maps()["entries"][0]["available"], false);
        let before = reopened.recent_maps.entries.clone();
        assert!(
            reopened
                .open_recent_map(Actor::Agent, &paths[4], false)
                .is_err()
        );
        assert_eq!(reopened.recent_maps.entries, before);
    }

    #[test]
    fn failed_open_dirty_refusal_and_failed_save_preserve_map_and_recent_list() {
        let (directory, mut session) = fixture();
        let first = map_file(directory.path(), "first.vmf");
        let second = map_file(directory.path(), "second.vmf");
        session.open(Actor::User, &first, false).unwrap();
        session.open(Actor::User, &second, false).unwrap();
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let before = map_state(&session);
        let entries = session.recent_maps.entries.clone();
        let bytes = fs::read(
            directory
                .path()
                .join("preferences")
                .join(recent_maps::FILE_NAME),
        )
        .unwrap();
        assert!(
            session
                .open_recent_map(Actor::Agent, &first, false)
                .unwrap_err()
                .contains("unsaved")
        );
        let invalid = directory.path().join("invalid.vmf");
        fs::write(&invalid, "world { ").unwrap();
        assert!(session.open(Actor::Agent, &invalid, true).is_err());
        assert!(
            session
                .save(
                    Actor::Agent,
                    Some(&directory.path().join("missing/map.vmf"))
                )
                .is_err()
        );
        assert_eq!(map_state(&session), before);
        assert_eq!(session.recent_maps.entries, entries);
        assert_eq!(
            fs::read(
                directory
                    .path()
                    .join("preferences")
                    .join(recent_maps::FILE_NAME)
            )
            .unwrap(),
            bytes
        );
        session.open_recent_map(Actor::Agent, &first, true).unwrap();
        assert_eq!(session.path(), Some(first.as_path()));
    }

    #[test]
    fn build_context_restores_shared_settings_and_keeps_incompatible_context() {
        let (directory, mut session) = fixture();
        let first = map_file(directory.path(), "first.vmf");
        let second = map_file(directory.path(), "second.vmf");
        session.open(Actor::User, &first, false).unwrap();
        session
            .update_build_settings(
                Actor::Agent,
                BuildSettingsUpdate {
                    lighting: Some("direct_ldr_hdr"),
                    sky_quality: Some("balanced"),
                    visibility: Some("detailed"),
                    bounces: Some(2),
                    smoothing_angle: Some(35),
                    ..Default::default()
                },
            )
            .unwrap();
        let retained = session.compile_options();
        session.open(Actor::User, &second, false).unwrap();
        session
            .set_compile_settings(Actor::Agent, "fullbright")
            .unwrap();
        session
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(session.compile_options(), retained);
        assert!(!session.document().is_dirty());
        assert_eq!(session.document().undo_labels().count(), 0);
        let mut restarted = Session::new(Err("no assets needed".into()));
        restarted.configure_recent_maps(Ok(directory.path().join("preferences")));
        restarted
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(restarted.compile_options(), retained);
        restarted
            .open_recent_map(Actor::Agent, &second, false)
            .unwrap();
        assert_eq!(
            restarted.compile_options().lighting,
            compiler::LightingMode::Fullbright
        );
        restarted.compiling = true;
        restarted
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(
            restarted.compile_options().lighting,
            compiler::LightingMode::Fullbright
        );
        assert_eq!(
            restarted.recent_maps.entries[0].build_settings,
            BuildSettings::from_options(retained)
        );
        restarted.compiling = false;
        restarted
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(restarted.compile_options(), retained);
        restarted.recent_maps.entries[0].game_directory = Some(directory.path().join("other-game"));
        restarted.new_map(Actor::Agent, false).unwrap();
        restarted
            .set_compile_settings(Actor::Agent, "fullbright")
            .unwrap();
        let incompatible = restarted.recent_maps.entries[0].clone();
        restarted
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(
            restarted.compile_options().lighting,
            compiler::LightingMode::Fullbright
        );
        assert_eq!(restarted.recent_maps.entries[0], incompatible);
        assert_eq!(
            restarted.recent_maps()["entries"][0]["context_compatible"],
            false
        );
        restarted.save(Actor::User, None).unwrap();
        assert!(restarted.recent_maps.entries[0].game_directory.is_none());
        assert_eq!(
            restarted.recent_maps()["entries"][0]["context_compatible"],
            true
        );
    }

    #[test]
    fn relative_mounted_content_retains_an_absolute_game_identity() {
        let session = Session::new(Ok(source_formats::gamefs::GameFs {
            game_dir: PathBuf::new(),
            base_dir: PathBuf::new(),
            mounts: Vec::new(),
        }));
        assert_eq!(
            session.recent_game_directory(),
            Some(std::fs::canonicalize(".").unwrap())
        );
    }

    #[test]
    fn failed_history_write_preserves_list_and_does_not_fail_a_successful_map_save() {
        let (directory, mut session) = fixture();
        let first = map_file(directory.path(), "first.vmf");
        let second = map_file(directory.path(), "second.vmf");
        session.open(Actor::User, &first, false).unwrap();
        let history = directory
            .path()
            .join("preferences")
            .join(recent_maps::FILE_NAME);
        let bytes = fs::read(&history).unwrap();
        let mut permissions = fs::metadata(&history).unwrap().permissions();
        let original = permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&history, permissions).unwrap();
        session.open(Actor::Agent, &second, false).unwrap();
        assert_eq!(session.path(), Some(second.as_path()));
        let entries = session.recent_maps.entries.clone();
        assert_eq!(entries[0].path, first);
        assert!(
            session.recent_maps()["error"]
                .as_str()
                .unwrap()
                .contains("could not save")
        );
        let state = map_state(&session);
        assert!(session.clear_recent_maps().is_err());
        assert!(session.remove_recent_map(&first).is_err());
        assert_eq!(session.recent_maps.entries, entries);
        assert_eq!(map_state(&session), state);
        let copy = directory.path().join("copy.vmf");
        session.save(Actor::User, Some(&copy)).unwrap();
        assert!(copy.is_file());
        assert!(!session.document().is_dirty());
        assert_eq!(session.recent_maps.entries, entries);
        assert_eq!(fs::read(&history).unwrap(), bytes);
        fs::set_permissions(&history, original).unwrap();
        session.save(Actor::User, None).unwrap();
        assert_eq!(session.recent_maps.entries[0].path, copy);
        assert!(session.recent_maps()["error"].is_null());
    }

    #[test]
    fn corrupt_load_blocks_changes_without_erasing_existing_history_or_map() {
        let (directory, mut session) = fixture();
        let first = map_file(directory.path(), "first.vmf");
        session.open(Actor::User, &first, false).unwrap();
        let entries = session.recent_maps.entries.clone();
        let broken = directory.path().join("broken");
        fs::create_dir(&broken).unwrap();
        let path = broken.join(recent_maps::FILE_NAME);
        fs::write(&path, "broken preferences").unwrap();
        let before = map_state(&session);
        session.configure_recent_maps(Ok(broken));
        assert_eq!(session.recent_maps()["persistence"], "blocked");
        assert!(session.clear_recent_maps().is_err());
        assert!(session.remove_recent_map(&first).is_err());
        assert_eq!(session.recent_maps.entries, entries);
        assert_eq!(map_state(&session), before);
        session
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "broken preferences");
        assert_eq!(session.recent_maps.entries, entries);
        session.configure_recent_maps(Ok(directory.path().join("preferences")));
        session.clear_recent_maps().unwrap();
        assert!(session.recent_maps.entries.is_empty());
        assert!(first.is_file());
        assert_eq!(session.path(), Some(first.as_path()));
    }
}
