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
                "reopen_path": entry.reopen_path(),
                "available": entry.reopen_path().is_file(),
                "game_directory": entry.game_directory,
                "context_compatible": entry.game_directory == game_directory,
                "build_settings": entry.build_settings.report(),
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
        let (path, reopen_path) = match recent_maps::source_paths(path) {
            Ok(paths) => paths,
            Err(error) => {
                self.recent_map_error(error);
                return;
            }
        };
        let entry = Entry {
            path,
            reopen_path,
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
        self.open_inner(
            actor,
            entry.reopen_path(),
            discard_changes,
            true,
            Some(&entry.path),
        )
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
