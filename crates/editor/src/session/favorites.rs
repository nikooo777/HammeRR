use std::path::PathBuf;

use crate::material_favorites::{MAX_FAVORITES, Persistence, Status, Store, canonical_name};

use super::{Actor, Session};

#[derive(Default)]
pub(super) struct State {
    store: Option<Store>,
    load_failed: bool,
    error: Option<String>,
}

impl Session {
    pub(super) fn configure_material_favorites(&mut self, directory: Result<PathBuf, String>) {
        let store = directory.map(|directory| Store { directory });
        let loaded = match &store {
            Ok(store) => store.load(),
            Err(error) => Err(error.clone()),
        };
        self.favorite_preferences.store = store.ok();
        match loaded {
            Ok(materials) => {
                self.material_favorites = materials;
                self.favorite_preferences.load_failed = false;
                self.favorite_preferences.error = None;
                self.view_serial += 1;
            }
            Err(error) => {
                self.favorite_preferences.load_failed = true;
                self.material_favorite_error(format!("Material favorites could not load: {error}"));
            }
        }
    }

    pub fn material_favorite_status(&self) -> Status {
        let mut status = Status::session(self.material_favorites.len());
        status.directory = self
            .favorite_preferences
            .store
            .as_ref()
            .map(|store| store.directory.clone());
        status.error = self.favorite_preferences.error.clone();
        status.persistence = if self.favorite_preferences.load_failed {
            Persistence::Blocked
        } else if self.favorite_preferences.store.is_some() {
            Persistence::Persistent
        } else {
            Persistence::Session
        };
        status
    }

    fn material_favorite_error(&mut self, error: String) -> String {
        self.favorite_preferences.error = Some(error.clone());
        self.view_serial += 1;
        self.log(Actor::System, &error);
        error
    }

    pub fn favorite_material(&mut self, material: &str, favorite: bool) -> Result<(), String> {
        if self.favorite_preferences.load_failed {
            let error = format!(
                "{}; fix storage and restart the editor before changing favorites",
                self.favorite_preferences
                    .error
                    .as_deref()
                    .unwrap_or("Material favorites did not load")
            );
            self.log(Actor::System, &error);
            return Err(error);
        }
        let name = canonical_name(material).map_err(|error| self.material_favorite_error(error))?;
        if self.material_favorites.contains(&name) == favorite {
            return Ok(());
        }
        let mut materials = self.material_favorites.clone();
        if favorite {
            if materials.len() == MAX_FAVORITES {
                return Err(self.material_favorite_error(format!(
                    "Keep at most {MAX_FAVORITES} material favorites"
                )));
            }
            materials.insert(name);
        } else {
            materials.remove(&name);
        }
        if let Some(store) = &self.favorite_preferences.store
            && let Err(error) = store.save(&materials)
        {
            return Err(
                self.material_favorite_error(format!("Material favorites could not save: {error}"))
            );
        }
        self.material_favorites = materials;
        self.favorite_preferences.error = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn is_material_favorite(&self, material: &str) -> bool {
        canonical_name(material).is_ok_and(|name| self.material_favorites.contains(&name))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use glam::DVec3;
    use serde_json::{Value, json};
    use source_formats::keyvalues::WriteOptions;

    use crate::material_favorites::{FILE_NAME, MAX_MATERIAL_BYTES};

    use super::*;

    fn fixture() -> (tempfile::TempDir, Session) {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        (directory, session)
    }

    fn map_state(session: &Session) -> Value {
        let document = session.document();
        json!({
            "vmf": document.vmf().write(&WriteOptions::default()),
            "revision": document.revision(), "generation": session.document_generation(),
            "dirty": document.is_dirty(), "path": session.path(),
            "undo": document.undo_labels().collect::<Vec<_>>(),
            "redo": document.redo_labels().collect::<Vec<_>>(),
            "selection": format!("{:?}", session.selected_ids()),
            "recent": session.material_recent,
        })
    }

    #[test]
    fn favorites_restart_and_reconfigure_without_persisting_recent_applications() {
        let (directory, mut session) = fixture();
        let root = directory.path().join("recovery");
        session
            .favorite_material("Materials\\BRICK\\Wall.VMT", true)
            .unwrap();
        session.favorite_material("custom/unmounted", true).unwrap();
        let before_noop = fs::read(root.join(FILE_NAME)).unwrap();
        let serial = session.view_serial();
        session.favorite_material("BRICK/WALL", true).unwrap();
        assert_eq!(session.view_serial(), serial);
        assert_eq!(fs::read(root.join(FILE_NAME)).unwrap(), before_noop);
        session.remember_material("materials/wood/floor.vmt");
        assert_eq!(session.material_recent[0], "wood/floor");
        let mut restarted = Session::new(Err("no assets needed".into()));
        restarted.configure_recovery(Ok(root.clone()));
        assert!(restarted.material_recent.is_empty());
        assert!(restarted.is_material_favorite(" brick/wall "));
        let catalog = restarted.material_catalog("", "favorites", 0, 36).unwrap();
        assert_eq!(catalog.total, 2);
        assert!(catalog.entries.iter().all(|entry| entry.favorite));
        assert_eq!(catalog.favorites.persistence, Persistence::Persistent);
        assert_eq!(catalog.favorites.directory, Some(root.clone()));
        restarted
            .favorite_material("BRICK/WALL.vmt", false)
            .unwrap();
        session.configure_recovery(Ok(root));
        assert!(!session.is_material_favorite("brick/wall"));
        assert!(session.is_material_favorite("custom/unmounted"));
        assert_eq!(session.material_recent[0], "wood/floor");
        session.configure_recovery(Ok(directory.path().join("another-store")));
        assert!(session.material_favorites.is_empty());
        assert_eq!(session.material_recent[0], "wood/floor");
    }

    #[test]
    fn favorites_remain_available_in_isolated_sessions_without_storage() {
        let mut session = Session::new(Err("no assets needed".into()));
        let before = map_state(&session);
        session
            .favorite_material("Materials/BRICK/WALL.vmt", true)
            .unwrap();
        let catalog = session.material_catalog("", "favorites", 0, 36).unwrap();
        assert_eq!(catalog.entries[0].name, "brick/wall");
        assert_eq!(catalog.favorites.persistence, Persistence::Session);
        assert_eq!(catalog.favorites.directory, None);
        assert_eq!(catalog.favorites.count, 1);
        assert_eq!(map_state(&session), before);
        assert!(!Session::new(Err("no assets needed".into())).is_material_favorite("brick/wall"));
    }

    #[test]
    fn failed_favorite_save_preserves_preferences_map_and_history_then_retries() {
        let (directory, mut session) = fixture();
        session.favorite_material("brick/wall", true).unwrap();
        let id = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/wall",
                false,
            )
            .unwrap();
        session
            .apply_material(
                Actor::Agent,
                &[map_core::edit::FaceTarget::Object(id)],
                "wood/floor",
            )
            .unwrap();
        session.undo(Actor::User).unwrap();
        let before = map_state(&session);
        let materials = session.material_favorites.clone();
        let path = directory.path().join("recovery").join(FILE_NAME);
        let original = fs::read(&path).unwrap();
        let permissions = fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        for (name, favorite) in [("wood/floor", true), ("brick/wall", false)] {
            assert!(
                session
                    .favorite_material(name, favorite)
                    .unwrap_err()
                    .contains("could not save")
            );
            assert_eq!(session.material_favorites, materials);
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(map_state(&session), before);
            let catalog = session.material_catalog("", "favorites", 0, 36).unwrap();
            assert_eq!(catalog.total, 1);
            assert!(catalog.favorites.error.unwrap().contains("read-only"));
            assert!(
                session
                    .recent_log()
                    .next()
                    .unwrap()
                    .text
                    .contains("could not save")
            );
        }
        fs::set_permissions(&path, permissions).unwrap();
        session.favorite_material("wood/floor", true).unwrap();
        assert!(session.material_favorite_status().error.is_none());
        assert_eq!(map_state(&session), before);
    }

    #[test]
    fn favorites_limits_reject_without_publishing_or_mutating_the_map() {
        let (_directory, mut session) = fixture();
        session.material_favorites = (0..MAX_FAVORITES)
            .map(|index| format!("test/{index}"))
            .collect();
        session
            .favorite_preferences
            .store
            .as_ref()
            .unwrap()
            .save(&session.material_favorites)
            .unwrap();
        let path = session
            .favorite_preferences
            .store
            .as_ref()
            .unwrap()
            .directory
            .join(FILE_NAME);
        let original = fs::read(&path).unwrap();
        let before = map_state(&session);
        session.favorite_material("test/0", true).unwrap();
        assert!(session.favorite_material("extra/wall", true).is_err());
        assert!(
            session
                .favorite_material(&"x".repeat(MAX_MATERIAL_BYTES + 1), true)
                .is_err()
        );
        assert!(session.favorite_material("../wall", false).is_err());
        assert_eq!(session.material_favorites.len(), MAX_FAVORITES);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(map_state(&session), before);
        session.favorite_material("test/0", false).unwrap();
        session.favorite_material("extra/wall", true).unwrap();
        assert_eq!(session.material_favorites.len(), MAX_FAVORITES);
    }

    #[test]
    fn failed_reconfigure_preserves_favorites_and_reports_blocked_storage() {
        let (directory, mut session) = fixture();
        session.favorite_material("brick/wall", true).unwrap();
        let invalid_root = directory.path().join("invalid");
        fs::create_dir(&invalid_root).unwrap();
        let invalid = b"broken preferences";
        fs::write(invalid_root.join(FILE_NAME), invalid).unwrap();
        let before = map_state(&session);
        session.configure_recovery(Ok(invalid_root.clone()));
        assert!(session.is_material_favorite("brick/wall"));
        let status = session.material_favorite_status();
        assert_eq!(status.persistence, Persistence::Blocked);
        assert_eq!(status.directory, Some(invalid_root.clone()));
        assert!(status.error.unwrap().contains("could not load"));
        assert!(session.favorite_material("wood/floor", true).is_err());
        assert_eq!(fs::read(invalid_root.join(FILE_NAME)).unwrap(), invalid);
        assert_eq!(map_state(&session), before);
        session.configure_recovery(Err("state directory unavailable".into()));
        assert_eq!(
            session.material_favorite_status().persistence,
            Persistence::Blocked
        );
        assert!(session.is_material_favorite("brick/wall"));
        assert!(session.favorite_material("brick/wall", false).is_err());
        session.configure_recovery(Ok(directory.path().join("recovery")));
        session.favorite_material("wood/floor", true).unwrap();
        assert_eq!(
            session.material_favorite_status().persistence,
            Persistence::Persistent
        );
    }

    #[test]
    fn corrupt_recovery_settings_and_favorites_load_independently() {
        let (directory, mut session) = fixture();
        let root = directory.path().join("recovery");
        session.favorite_material("brick/wall", true).unwrap();
        fs::write(root.join("settings.json"), "broken recovery settings").unwrap();
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recovery(Ok(root.clone()));
        assert!(reopened.recovery_status()["error"].is_string());
        assert_eq!(
            reopened.material_favorite_status().persistence,
            Persistence::Persistent
        );
        reopened.favorite_material("wood/floor", true).unwrap();
        let settings = crate::recovery::Settings {
            enabled: false,
            interval_seconds: 60,
            keep_per_map: 3,
        };
        reopened
            .set_recovery_settings(Actor::Agent, settings.clone(), None)
            .unwrap();
        fs::write(root.join(FILE_NAME), "broken favorites").unwrap();
        let mut again = Session::new(Err("no assets needed".into()));
        again.configure_recovery(Ok(root));
        assert_eq!(again.recovery_settings(), &settings);
        assert_eq!(
            again.material_favorite_status().persistence,
            Persistence::Blocked
        );
        assert_eq!(again.material_favorites, BTreeSet::new());
    }
}
