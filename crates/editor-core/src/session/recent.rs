use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::recent_maps::{self, Authority, BuildSettings, Entry, Snapshot};

use super::{Actor, Session};

pub(super) enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Default for Binding {
    fn default() -> Self {
        Self::Independent(Authority::default())
    }
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

impl Session {
    pub fn with_recent_maps(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_recent_maps(authority);
        session
    }

    pub(crate) fn use_application_recent_maps(&mut self, authority: Authority) {
        self.recent_maps = Binding::Application(authority);
        if let Some(error) = self.recent_maps_snapshot().error {
            self.log(Actor::System, error);
        }
    }

    pub fn configure_recent_maps(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.recent_maps
            && let Err(error) = authority.configure(directory)
        {
            self.log(Actor::System, error);
        }
    }

    pub fn recent_maps_snapshot(&self) -> Snapshot {
        self.recent_maps.authority().snapshot()
    }

    pub fn recent_maps_serial(&self) -> u64 {
        self.recent_maps.authority().serial()
    }

    pub fn reload_recent_maps(&mut self) -> Result<Value, String> {
        let result = self.recent_maps.authority().reload();
        self.log_recent_map_result(result)
            .map(|snapshot| self.recent_maps_with_snapshot(&snapshot))
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
        self.recent_maps_with_snapshot(&self.recent_maps_snapshot())
    }

    pub fn recent_maps_with_snapshot(&self, snapshot: &Snapshot) -> Value {
        let game_directory = self.recent_game_directory();
        json!({
            "persistence": snapshot.persistence(),
            "directory": snapshot.directory,
            "error": snapshot.error,
            "reload_available": snapshot.directory.is_some(),
            "max_maps": recent_maps::MAX_RECENT_MAPS,
            "max_path_bytes": recent_maps::MAX_PATH_BYTES,
            "max_file_bytes": recent_maps::MAX_FILE_BYTES,
            "entries": snapshot.entries.iter().map(|entry| json!({
                "path": entry.path,
                "title": entry.path.file_name().map(|name| name.to_string_lossy()),
                "available": entry.path.is_file(),
                "game_directory": entry.game_directory,
                "context_compatible": entry.game_directory == game_directory,
                "build_settings": entry.build_settings,
            })).collect::<Vec<_>>(),
        })
    }

    fn log_recent_map_result<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if let Err(error) = &result {
            self.log(Actor::System, error);
        }
        result
    }

    fn recent_map_error(&mut self, error: String) {
        let error = self.recent_maps.authority().report_error(error);
        self.log(Actor::System, error);
    }

    pub(crate) fn remember_recent_map(&mut self, path: &Path) {
        self.remember_map(path, true, true);
    }

    pub(super) fn remember_saved_map(&mut self, path: &Path) {
        self.remember_map(path, true, false);
    }

    fn remember_map(&mut self, path: &Path, reorder: bool, preserve_incompatible: bool) {
        if self.recent_maps.authority().blocked() {
            return;
        }
        let path = match recent_maps::absolute_path(path) {
            Ok(path) => path,
            Err(error) => {
                self.recent_map_error(error);
                return;
            }
        };
        let entry = Entry {
            path,
            game_directory: self.recent_game_directory(),
            build_settings: BuildSettings::from_options(self.compile_options()),
        };
        let result = self.recent_maps.authority().remember(
            entry,
            reorder,
            preserve_incompatible,
            self.compiling(),
        );
        let _ = self.log_recent_map_result(result);
    }

    pub(super) fn remember_current_map_context(&mut self) {
        if let Some(path) = self.path.clone() {
            self.remember_map(&path, false, false);
        }
    }

    pub(super) fn restore_recent_map_context(&mut self, path: &Path, remember: bool) {
        if self.compiling() || self.closing {
            return;
        }
        let Ok(path) = recent_maps::absolute_path(path) else {
            return;
        };
        let Some(entry) = self.recent_maps.authority().entry(&path) else {
            return;
        };
        if entry.game_directory != self.recent_game_directory() {
            self.log(
                Actor::System,
                "Recent map content differs from the mounted game; kept current build settings",
            );
            return;
        }
        if let Err(error) =
            self.update_build_settings_inner(Actor::System, entry.build_settings.update(), remember)
        {
            let error = format!("Recent map build settings could not restore: {error}");
            if remember {
                self.recent_map_error(error);
            } else {
                self.log(Actor::System, error);
            }
        }
    }

    pub fn open_recent_map(
        &mut self,
        actor: Actor,
        path: &Path,
        discard_changes: bool,
    ) -> Result<String, String> {
        self.ensure_not_closing()?;
        let entry = self
            .recent_maps
            .authority()
            .entry(path)
            .ok_or("This map is no longer in Recent maps; refresh the list")?;
        self.open(actor, &entry.path, discard_changes)
    }

    pub fn remove_recent_map(&mut self, path: &Path) -> Result<(), String> {
        let result = self.recent_maps.authority().remove(path);
        self.log_recent_map_result(result)
    }

    pub fn clear_recent_maps(&mut self) -> Result<(), String> {
        let result = self.recent_maps.authority().clear();
        self.log_recent_map_result(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::session::compilation::BuildSettingsUpdate;
    use std::fs;

    use glam::DVec3;
    use source_formats::keyvalues::WriteOptions;

    use super::*;
    use crate::recent_maps::Store;

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
    fn stale_editors_preserve_primary_open_save_and_recover_through_reload() {
        for actor in [Actor::User, Actor::Agent] {
            let (directory, mut first) = fixture();
            let mut second = Session::new(Err("no assets needed".into()));
            second.configure_recent_maps(Ok(directory.path().join("preferences")));
            let a = map_file(directory.path(), "a.vmf");
            let b = map_file(directory.path(), "b.vmf");
            first.open(actor, &a, false).unwrap();
            let storage = directory
                .path()
                .join("preferences")
                .join(recent_maps::FILE_NAME);
            let saved = fs::read(&storage).unwrap();
            second.open(actor, &b, false).unwrap();
            assert_eq!(second.path(), Some(b.as_path()));
            assert!(second.recent_maps_snapshot().entries.is_empty());
            assert_eq!(second.recent_maps()["persistence"], "blocked");
            assert!(
                second.recent_maps()["error"]
                    .as_str()
                    .unwrap()
                    .contains("Reload saved list")
            );
            let copy = directory.path().join("copy.vmf");
            second.save(actor, Some(&copy)).unwrap();
            assert!(copy.is_file() && !second.document.is_dirty());
            assert_eq!(fs::read(&storage).unwrap(), saved);
            second
                .preview_block(DVec3::ZERO, DVec3::splat(16.0))
                .unwrap();
            let preview = second.block_preview();
            let before = map_state(&second);
            let camera = second.camera();
            let options = second.compile_options();
            second.reload_recent_maps().unwrap();
            assert_eq!(
                second.recent_maps_snapshot().entries,
                first.recent_maps_snapshot().entries
            );
            assert_eq!(map_state(&second), before);
            assert_eq!(second.block_preview(), preview);
            assert_eq!(second.camera(), camera);
            assert_eq!(second.compile_options(), options);
            second.cancel_block_preview();
            second.save(actor, None).unwrap();
            let paths = second
                .recent_maps_snapshot()
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>();
            assert_eq!(paths, vec![copy, a]);
            first.reload_recent_maps().unwrap();
            assert_eq!(
                first.recent_maps_snapshot().entries,
                second.recent_maps_snapshot().entries
            );
        }
    }

    #[test]
    fn stale_remove_clear_and_context_updates_cannot_replace_other_editor_changes() {
        for operation in ["remove", "clear", "context"] {
            let (directory, mut first) = fixture();
            let a = map_file(directory.path(), "a.vmf");
            let b = map_file(directory.path(), "b.vmf");
            first.open(Actor::User, &a, false).unwrap();
            let mut second = Session::new(Err("no assets needed".into()));
            second.configure_recent_maps(Ok(directory.path().join("preferences")));
            second.open(Actor::Agent, &a, false).unwrap();
            first.reload_recent_maps().unwrap();
            first.open(Actor::User, &b, false).unwrap();
            let storage = directory
                .path()
                .join("preferences")
                .join(recent_maps::FILE_NAME);
            let bytes = fs::read(&storage).unwrap();
            let entries = second.recent_maps_snapshot().entries.clone();
            let map = map_state(&second);
            match operation {
                "remove" => assert!(second.remove_recent_map(&a).is_err()),
                "clear" => assert!(second.clear_recent_maps().is_err()),
                "context" => {
                    second
                        .update_build_settings(
                            Actor::Agent,
                            BuildSettingsUpdate {
                                sky_quality: Some("detailed"),
                                ..Default::default()
                            },
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert_eq!(fs::read(&storage).unwrap(), bytes);
            assert_eq!(second.recent_maps_snapshot().entries, entries);
            assert_eq!(map_state(&second), map);
            assert_eq!(second.recent_maps()["persistence"], "blocked");
            second.reload_recent_maps().unwrap();
            assert_eq!(
                second.recent_maps_snapshot().entries,
                first.recent_maps_snapshot().entries
            );
        }
    }

    #[test]
    fn failed_explicit_reload_preserves_valid_list_and_map_until_storage_is_repaired() {
        let (directory, mut session) = fixture();
        let path = map_file(directory.path(), "map.vmf");
        session.open(Actor::User, &path, false).unwrap();
        session
            .preview_block(DVec3::ZERO, DVec3::splat(16.0))
            .unwrap();
        let preview = session.block_preview();
        let entries = session.recent_maps_snapshot().entries.clone();
        let before = map_state(&session);
        let storage = directory
            .path()
            .join("preferences")
            .join(recent_maps::FILE_NAME);
        let valid = fs::read(&storage).unwrap();
        for invalid in [
            b"invalid".to_vec(),
            vec![b' '; recent_maps::MAX_FILE_BYTES as usize + 1],
        ] {
            fs::write(&storage, &invalid).unwrap();
            assert!(session.reload_recent_maps().is_err());
            assert_eq!(session.recent_maps_snapshot().entries, entries);
            assert_eq!(map_state(&session), before);
            assert_eq!(session.block_preview(), preview);
            assert_eq!(fs::read(&storage).unwrap(), invalid);
            assert!(session.clear_recent_maps().is_err());
        }
        fs::write(&storage, &valid).unwrap();
        session.reload_recent_maps().unwrap();
        assert_eq!(session.recent_maps_snapshot().entries, entries);
        assert!(session.recent_maps_snapshot().error.is_none());
        assert_eq!(session.block_preview(), preview);
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
            session.recent_maps_snapshot().entries.len(),
            recent_maps::MAX_RECENT_MAPS
        );
        assert_eq!(
            session.recent_maps_snapshot().entries[0].path,
            *paths.last().unwrap()
        );
        assert!(
            !session
                .recent_maps_snapshot()
                .entries
                .iter()
                .any(|entry| entry.path == paths[0])
        );
        session
            .open_recent_map(Actor::Agent, &paths[4], false)
            .unwrap();
        assert_eq!(session.recent_maps_snapshot().entries[0].path, paths[4]);
        assert_eq!(
            session
                .recent_maps_snapshot()
                .entries
                .iter()
                .filter(|entry| entry.path == paths[4])
                .count(),
            1
        );
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recent_maps(Ok(directory.path().join("preferences")));
        assert_eq!(
            reopened.recent_maps_snapshot().entries,
            session.recent_maps_snapshot().entries
        );
        assert_eq!(reopened.recent_maps()["persistence"], "persistent");
        fs::remove_file(&paths[4]).unwrap();
        assert_eq!(reopened.recent_maps()["entries"][0]["available"], false);
        let before = reopened.recent_maps_snapshot().entries.clone();
        assert!(
            reopened
                .open_recent_map(Actor::Agent, &paths[4], false)
                .is_err()
        );
        assert_eq!(reopened.recent_maps_snapshot().entries, before);
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
        let entries = session.recent_maps_snapshot().entries.clone();
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
        assert_eq!(session.recent_maps_snapshot().entries, entries);
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
            restarted.recent_maps_snapshot().entries[0].build_settings,
            BuildSettings::from_options(retained)
        );
        restarted.compiling = false;
        restarted
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(restarted.compile_options(), retained);
        let mut entries = restarted.recent_maps_snapshot().entries;
        entries[0].game_directory = Some(directory.path().join("other-game"));
        Store {
            directory: directory.path().join("preferences"),
        }
        .save(&entries)
        .unwrap();
        restarted.reload_recent_maps().unwrap();
        restarted.new_map(Actor::Agent, false).unwrap();
        restarted
            .set_compile_settings(Actor::Agent, "fullbright")
            .unwrap();
        let incompatible = restarted.recent_maps_snapshot().entries[0].clone();
        restarted
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(
            restarted.compile_options().lighting,
            compiler::LightingMode::Fullbright
        );
        assert_eq!(restarted.recent_maps_snapshot().entries[0], incompatible);
        assert_eq!(
            restarted.recent_maps()["entries"][0]["context_compatible"],
            false
        );
        restarted.save(Actor::User, None).unwrap();
        assert!(
            restarted.recent_maps_snapshot().entries[0]
                .game_directory
                .is_none()
        );
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
    fn asset_packing_recent_context_restarts_and_restores_only_for_the_same_game() {
        fn mounted_session(base_directory: &Path, game_directory: &Path) -> Session {
            Session::new(Ok(source_formats::gamefs::GameFs {
                game_dir: game_directory.to_owned(),
                base_dir: base_directory.to_owned(),
                mounts: Vec::new(),
            }))
        }

        for actor in [Actor::User, Actor::Agent] {
            let directory = tempfile::tempdir().unwrap();
            let first = map_file(directory.path(), "portable.vmf");
            let second = map_file(directory.path(), "default.vmf");
            let game_directory = directory.path().join("cstrike");
            let other_game = directory.path().join("other-game");
            fs::create_dir(&game_directory).unwrap();
            fs::create_dir(&other_game).unwrap();
            let preferences = directory.path().join("preferences");
            let mut current = mounted_session(directory.path(), &game_directory);
            current.configure_recent_maps(Ok(preferences.clone()));
            current.open(actor, &first, false).unwrap();
            let source = fs::read(&first).unwrap();
            current
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        lighting: Some("direct_ldr_hdr"),
                        physics: Some("engine"),
                        sky_quality: Some("balanced"),
                        visibility: Some("detailed"),
                        bounces: Some(2),
                        smoothing_angle: Some(35),
                        pack_sky_assets: Some(true),
                        pack_water_assets: Some(true),
                        pack_static_prop_assets: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            let retained = current.compile_options();
            assert!(
                current.recent_maps_snapshot().entries[0]
                    .build_settings
                    .pack_sky_assets
            );
            current.new_map(actor, false).unwrap();
            current
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        lighting: Some("fullbright"),
                        pack_sky_assets: Some(false),
                        pack_water_assets: Some(false),
                        pack_static_prop_assets: Some(false),
                        ..Default::default()
                    },
                )
                .unwrap();
            current.open(actor, &second, false).unwrap();
            let disabled = current.compile_options();
            let mut restarted = mounted_session(directory.path(), &game_directory);
            restarted.configure_recent_maps(Ok(preferences.clone()));
            assert!(!restarted.compile_options().pack_sky_assets);
            restarted.open_recent_map(actor, &first, false).unwrap();
            assert_eq!(restarted.compile_options(), retained);
            assert_eq!(restarted.compile_settings()["pack_sky_assets"], true);
            assert_eq!(restarted.compile_settings()["pack_water_assets"], true);
            assert_eq!(
                restarted.compile_settings()["pack_static_prop_assets"],
                true
            );
            assert_eq!(
                restarted.recent_maps()["entries"][0]["context_compatible"],
                true
            );
            assert!(!restarted.document().is_dirty());
            assert_eq!(restarted.document().undo_labels().count(), 0);
            restarted.open_recent_map(actor, &second, false).unwrap();
            assert_eq!(restarted.compile_options(), disabled);
            restarted.compiling = true;
            restarted.open_recent_map(actor, &first, false).unwrap();
            assert_eq!(restarted.compile_options(), disabled);
            assert!(
                restarted.recent_maps_snapshot().entries[0]
                    .build_settings
                    .pack_sky_assets
            );
            restarted.compiling = false;
            restarted.open_recent_map(actor, &first, false).unwrap();
            assert_eq!(restarted.compile_options(), retained);
            let stored = restarted.recent_maps_snapshot().entries[0].clone();
            let mut incompatible = mounted_session(directory.path(), &other_game);
            incompatible.configure_recent_maps(Ok(preferences.clone()));
            let options = incompatible.compile_options();
            incompatible.open_recent_map(actor, &first, false).unwrap();
            assert_eq!(incompatible.compile_options(), options);
            assert_eq!(incompatible.recent_maps_snapshot().entries[0], stored);
            assert_eq!(
                incompatible.recent_maps()["entries"][0]["context_compatible"],
                false
            );
            let mut unmounted = Session::new(Err("no mounted game".into()));
            unmounted.configure_recent_maps(Ok(preferences));
            let options = unmounted.compile_options();
            unmounted.open_recent_map(actor, &first, false).unwrap();
            assert_eq!(unmounted.compile_options(), options);
            assert_eq!(unmounted.recent_maps_snapshot().entries[0], stored);
            assert_eq!(fs::read(&first).unwrap(), source);
        }
    }

    #[test]
    fn invalid_sky_packing_recent_records_block_persistence_without_losing_context() {
        let (directory, mut current) = fixture();
        let path = map_file(directory.path(), "portable.vmf");
        current.open(Actor::User, &path, false).unwrap();
        current
            .update_build_settings(
                Actor::Agent,
                BuildSettingsUpdate {
                    pack_sky_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let preferences = directory.path().join("preferences");
        let storage = preferences.join(recent_maps::FILE_NAME);
        let original: Value = serde_json::from_slice(&fs::read(&storage).unwrap()).unwrap();
        let entries = current.recent_maps_snapshot().entries.clone();
        let options = current.compile_options();
        let before = map_state(&current);
        let mut invalid = Vec::new();
        for value in [json!(null), json!("true"), json!(1)] {
            let mut file = original.clone();
            file["entries"][0]["build_settings"]["pack_sky_assets"] = value;
            invalid.push(file);
        }
        let mut unknown = original;
        unknown["entries"][0]["build_settings"]["pack_sky_asset"] = json!(true);
        invalid.push(unknown);
        for file in invalid {
            let bytes = serde_json::to_vec(&file).unwrap();
            fs::write(&storage, &bytes).unwrap();
            current.configure_recent_maps(Ok(preferences.clone()));
            assert_eq!(current.recent_maps()["persistence"], "blocked");
            assert!(
                current.recent_maps()["error"]
                    .as_str()
                    .unwrap()
                    .contains("could not load")
            );
            assert_eq!(current.recent_maps_snapshot().entries, entries);
            assert_eq!(current.compile_options(), options);
            assert_eq!(map_state(&current), before);
            assert!(current.clear_recent_maps().is_err());
            assert!(current.remove_recent_map(&path).is_err());
            assert_eq!(fs::read(&storage).unwrap(), bytes);
        }
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
        let entries = session.recent_maps_snapshot().entries.clone();
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
        assert_eq!(session.recent_maps_snapshot().entries, entries);
        assert_eq!(map_state(&session), state);
        let copy = directory.path().join("copy.vmf");
        session.save(Actor::User, Some(&copy)).unwrap();
        assert!(copy.is_file());
        assert!(!session.document().is_dirty());
        assert_eq!(session.recent_maps_snapshot().entries, entries);
        assert_eq!(fs::read(&history).unwrap(), bytes);
        fs::set_permissions(&history, original).unwrap();
        session.save(Actor::User, None).unwrap();
        assert_eq!(session.recent_maps_snapshot().entries[0].path, copy);
        assert!(session.recent_maps()["error"].is_null());
    }

    #[test]
    fn corrupt_load_blocks_changes_without_erasing_existing_history_or_map() {
        let (directory, mut session) = fixture();
        let first = map_file(directory.path(), "first.vmf");
        session.open(Actor::User, &first, false).unwrap();
        let entries = session.recent_maps_snapshot().entries.clone();
        let broken = directory.path().join("broken");
        fs::create_dir(&broken).unwrap();
        let path = broken.join(recent_maps::FILE_NAME);
        fs::write(&path, "broken preferences").unwrap();
        let before = map_state(&session);
        session.configure_recent_maps(Ok(broken));
        assert_eq!(session.recent_maps()["persistence"], "blocked");
        assert!(session.clear_recent_maps().is_err());
        assert!(session.remove_recent_map(&first).is_err());
        assert_eq!(session.recent_maps_snapshot().entries, entries);
        assert_eq!(map_state(&session), before);
        session
            .open_recent_map(Actor::Agent, &first, false)
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "broken preferences");
        assert_eq!(session.recent_maps_snapshot().entries, entries);
        session.configure_recent_maps(Ok(directory.path().join("preferences")));
        session.clear_recent_maps().unwrap();
        assert!(session.recent_maps_snapshot().entries.is_empty());
        assert!(first.is_file());
        assert_eq!(session.path(), Some(first.as_path()));
    }
}

#[cfg(test)]
mod authority_tests;
