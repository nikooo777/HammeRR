use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use source_formats::keyvalues::WriteOptions;
use source_formats::vmf::Vmf;

use super::{Actor, Document, Session, Snapshot};
use crate::recovery::{Entry, Kind, Listing, Settings, Store};

pub(super) struct State {
    store: Option<Store>,
    settings: Settings,
    settings_revision: u64,
    document_key: String,
    source_path: Option<PathBuf>,
    title: String,
    last_attempt: Instant,
    captured: Option<Snapshot>,
    last_entry: Option<Entry>,
    error: Option<String>,
    busy: bool,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: None,
            settings: Settings::default(),
            settings_revision: 0,
            document_key: format!("untitled:{}", crate::recovery::unique_id()),
            source_path: None,
            title: "Untitled map".into(),
            last_attempt: Instant::now(),
            captured: None,
            last_entry: None,
            error: None,
            busy: false,
            serial: 0,
        }
    }
}

pub struct RecoveryRequest {
    store: Store,
    snapshot: Snapshot,
    document_key: String,
    source_path: Option<PathBuf>,
    title: String,
    kind: Kind,
    vmf: Vmf,
    keep: usize,
}

impl RecoveryRequest {
    pub fn run(&self) -> Result<Entry, String> {
        self.store.write(
            &self.document_key,
            self.source_path.as_deref(),
            &self.title,
            self.kind.clone(),
            self.vmf.write(&WriteOptions::default()).as_bytes(),
        )
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

fn path_key(path: &Path) -> String {
    format!("map:{}", absolute_path(path).display())
}

impl Session {
    pub fn configure_recovery(&mut self, directory: Result<PathBuf, String>) {
        self.configure_build_recipes(directory.clone());
        self.configure_material_favorites(directory.clone());
        let store = match directory {
            Ok(directory) => Store { directory },
            Err(error) => {
                self.recovery.error = Some(error);
                return;
            }
        };
        match store.load_settings() {
            Ok(settings) => self.recovery.settings = settings,
            Err(error) => {
                self.recovery.settings.enabled = false;
                self.recovery.error = Some(error.clone());
                self.log(
                    Actor::System,
                    format!("Recovery settings could not load: {error}"),
                );
            }
        }
        self.recovery.store = Some(store);
        self.recovery.settings_revision += 1;
        self.recovery.serial += 1;
    }

    pub fn recovery_serial(&self) -> u64 {
        self.recovery.serial
    }

    pub fn recovery_settings(&self) -> &Settings {
        &self.recovery.settings
    }

    pub fn recovery_busy(&self) -> bool {
        self.recovery.busy
    }

    pub fn recovery_settings_revision(&self) -> u64 {
        self.recovery.settings_revision
    }

    pub fn recovery_status(&self) -> Value {
        let pending = self.document.is_dirty()
            && !self
                .recovery
                .captured
                .as_ref()
                .is_some_and(|s| self.current_snapshot(s));
        json!({
            "generation": self.document_generation, "revision": self.document.revision(),
            "available": self.recovery.store.is_some(), "settings": self.recovery.settings,
            "settings_revision": self.recovery.settings_revision,
            "directory": self.recovery.store.as_ref().map(|store| &store.directory),
            "busy": self.recovery.busy, "pending_changes": pending,
            "next_autosave_seconds": (pending && self.recovery.store.is_some() && self.recovery.settings.enabled && !self.recovery.busy && !self.closing).then(|| Duration::from_secs(self.recovery.settings.interval_seconds).saturating_sub(self.recovery.last_attempt.elapsed()).as_secs()),
            "last_snapshot": self.recovery.last_entry, "error": self.recovery.error,
            "document_key": self.recovery.document_key,
        })
    }

    pub fn set_recovery_settings(
        &mut self,
        actor: Actor,
        settings: Settings,
        expected_revision: Option<u64>,
    ) -> Result<(), String> {
        if expected_revision.is_some_and(|version| version != self.recovery.settings_revision) {
            return Err(
                "Recovery settings changed elsewhere; reload the settings before applying".into(),
            );
        }
        let store = self
            .recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?;
        store.save_settings(&settings)?;
        self.recovery.settings = settings;
        self.recovery.settings_revision += 1;
        self.recovery.error = None;
        self.recovery.serial += 1;
        self.log(actor, "updated autosave and recovery settings");
        Ok(())
    }

    pub fn recovery_entries(&self) -> Result<Listing, String> {
        self.recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?
            .list()
    }

    pub fn autosave_due(&self) -> bool {
        self.recovery.store.is_some()
            && self.recovery.settings.enabled
            && !self.recovery.busy
            && !self.closing
            && self.document.is_dirty()
            && self.recovery.last_attempt.elapsed()
                >= Duration::from_secs(self.recovery.settings.interval_seconds)
            && !self
                .recovery
                .captured
                .as_ref()
                .is_some_and(|s| self.current_snapshot(s))
    }

    pub fn begin_recovery_snapshot(&mut self, kind: Kind) -> Result<RecoveryRequest, String> {
        if self.closing && kind != Kind::Autosave {
            return Err("The editor is closing".into());
        }
        if self.recovery.busy {
            return Err("A recovery snapshot is already being written".into());
        }
        let store = self
            .recovery
            .store
            .clone()
            .ok_or("Recovery storage is unavailable")?;
        self.recovery.busy = true;
        self.recovery.last_attempt = Instant::now();
        self.recovery.serial += 1;
        Ok(RecoveryRequest {
            store,
            snapshot: Snapshot {
                generation: self.document_generation,
                revision: self.document.revision(),
            },
            document_key: self.recovery.document_key.clone(),
            source_path: self.recovery.source_path.clone(),
            title: self.recovery.title.clone(),
            kind,
            vmf: self.document.vmf().clone(),
            keep: self.recovery.settings.keep_per_map,
        })
    }

    pub fn finish_recovery_snapshot(
        &mut self,
        actor: Actor,
        request: &RecoveryRequest,
        result: Result<Entry, String>,
    ) -> Result<Entry, String> {
        self.recovery.busy = false;
        self.recovery.last_attempt = Instant::now();
        self.recovery.serial += 1;
        match result {
            Ok(entry) => {
                self.recovery.error = None;
                if request.snapshot.generation == self.document_generation
                    && request.document_key == self.recovery.document_key
                {
                    self.recovery.captured = Some(request.snapshot.clone());
                    self.recovery.last_entry = Some(entry.clone());
                }
                if let Err(error) = request.store.prune(&request.document_key, request.keep) {
                    self.recovery.error = Some(error.clone());
                    self.log(
                        Actor::System,
                        format!("Recovery snapshot saved, but retention failed: {error}"),
                    );
                } else {
                    self.log(
                        actor,
                        format!("recovery snapshot saved for {}", entry.title),
                    );
                }
                Ok(entry)
            }
            Err(error) => {
                self.recovery.error = Some(error.clone());
                self.log(actor, format!("Recovery snapshot failed: {error}"));
                Err(error)
            }
        }
    }

    pub fn delete_recovery_snapshot(&mut self, actor: Actor, id: &str) -> Result<(), String> {
        if self.recovery.busy {
            return Err("Wait for the current recovery snapshot to finish".into());
        }
        self.recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?
            .delete(id)?;
        if self
            .recovery
            .last_entry
            .as_ref()
            .is_some_and(|entry| entry.id == id)
        {
            self.recovery.last_entry = None;
            self.recovery.captured = None;
        }
        self.recovery.serial += 1;
        self.log(actor, "deleted recovery snapshot");
        Ok(())
    }

    pub fn restore_recovery_snapshot(
        &mut self,
        actor: Actor,
        id: &str,
        discard_changes: bool,
        expected: Option<(u64, u64)>,
    ) -> Result<(), String> {
        if expected.is_some_and(|(generation, revision)| {
            generation != self.document_generation || revision != self.document.revision()
        }) {
            return Err(
                "Map changed since recovery was inspected; refresh before replacing it".into(),
            );
        }
        self.ensure_discardable(discard_changes)?;
        let (entry, text) = self
            .recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?
            .read(id)?;
        let vmf = Vmf::parse(&text).map_err(|e| format!("Recovery map could not be read: {e}"))?;
        let mut document = Document::try_new(vmf)
            .map_err(|error| format!("Recovery map has invalid IDs: {error}"))?;
        document.context().map_err(|error| error.to_string())?;
        document.mark_unsaved();
        self.replace_document(document, None);
        self.recovery.title = format!("Recovered {}", entry.title);
        self.recovery.source_path = entry.source_path;
        self.log(
            actor,
            format!(
                "recovered {} as an unsaved copy; choose Save As to keep it",
                entry.title
            ),
        );
        Ok(())
    }

    pub(super) fn reset_recovery_document(&mut self) {
        self.recovery.document_key = format!("untitled:{}", crate::recovery::unique_id());
        self.recovery.source_path = None;
        self.recovery.title = "Untitled map".into();
        self.recovery.last_attempt = Instant::now();
        self.recovery.captured = None;
        self.recovery.last_entry = None;
        self.recovery.serial += 1;
        self.update_recovery_path();
    }

    pub(super) fn update_recovery_path(&mut self) {
        if let Some(path) = &self.path {
            self.recovery.source_path = Some(absolute_path(path));
            self.recovery.document_key = path_key(path);
            self.recovery.title = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into();
            self.recovery.serial += 1;
        }
    }

    pub(super) fn backup_before_save(&mut self, path: &Path) -> Result<Option<Entry>, String> {
        let Some(store) = self.recovery.store.as_ref() else {
            return Ok(None);
        };
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Could not read previous save: {error}")),
        };
        let source = absolute_path(path);
        let key = path_key(path);
        let title = path.file_name().unwrap_or_default().to_string_lossy();
        let result = store
            .write(&key, Some(&source), &title, Kind::BeforeSave, &data)
            .map(Some);
        self.recovery.serial += 1;
        if let Err(error) = &result {
            self.recovery.error = Some(error.clone());
        }
        result
    }

    pub(super) fn finish_save_backup(
        &mut self,
        entry: Option<&Entry>,
        saved: bool,
    ) -> Result<(), String> {
        let (Some(store), Some(entry)) = (self.recovery.store.as_ref(), entry) else {
            return Ok(());
        };
        let result = if saved {
            store.prune(&entry.document_key, self.recovery.settings.keep_per_map)
        } else {
            store.delete(&entry.id)
        };
        self.recovery.serial += 1;
        if let Err(error) = &result {
            self.recovery.error = Some(error.clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::{ObjectId, surface};

    fn fixture() -> (tempfile::TempDir, Session, ObjectId) {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        let id = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        (directory, session, id)
    }

    fn capture(session: &mut Session) -> Entry {
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let result = request.run();
        session
            .finish_recovery_snapshot(Actor::User, &request, result)
            .unwrap()
    }

    #[test]
    fn duplicate_ids_in_open_and_recovery_preserve_the_active_document() {
        for actor in [Actor::User, Actor::Agent] {
            let (directory, mut session, id) = fixture();
            session
                .select(&[id], super::super::SelectMode::Replace)
                .unwrap();
            session
                .edit(actor, "temporary", |tx| {
                    tx.world_properties_mut().set("skyname", "temporary");
                    Ok(())
                })
                .unwrap();
            session.undo(actor).unwrap();
            session
                .create_primitive(actor, map_core::primitive::Spec::default(), true)
                .unwrap();
            let before = session.document.vmf().clone();
            let preview = session.render_document().vmf().clone();
            let state = (
                session.document_generation,
                session.document.revision(),
                session.selected_ids(),
                session.camera,
            );
            let text = "world { id 1 } entity { id 2 classname light } entity { id 2 classname info_target }";
            let path = directory.path().join("duplicate.vmf");
            std::fs::write(&path, text).unwrap();
            assert!(
                session
                    .open(actor, &path, true)
                    .unwrap_err()
                    .contains("duplicate object ID 2")
            );
            let entry = session
                .recovery
                .store
                .as_ref()
                .unwrap()
                .write(
                    "invalid",
                    None,
                    "Invalid IDs",
                    Kind::Manual,
                    text.as_bytes(),
                )
                .unwrap();
            assert!(
                session
                    .restore_recovery_snapshot(actor, &entry.id, true, None)
                    .unwrap_err()
                    .contains("duplicate object ID 2")
            );
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.render_document().vmf(), &preview);
            assert_eq!(
                (
                    session.document_generation,
                    session.document.revision(),
                    session.selected_ids(),
                    session.camera
                ),
                state
            );
            assert_eq!(session.document.undo_labels().count(), 1);
            assert_eq!(session.document.redo_labels().count(), 1);
            assert!(session.path().is_none());
        }
    }

    #[test]
    fn snapshots_exclude_previews_and_preserve_unsaved_state_history_and_versions() {
        let (_directory, mut session, ObjectId::Solid(id)) = fixture() else {
            panic!()
        };
        let face = surface::FaceId {
            solid: id,
            side: session.document().solid(id).unwrap().sides[0].id,
        };
        let original = session.document().vmf().clone();
        let revision = session.document().revision();
        let undo = session.document().undo_labels().count();
        session
            .edit_faces(
                Actor::Agent,
                &[face],
                surface::SurfaceEdit {
                    material: Some("metal/preview".into()),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        let entry = capture(&mut session);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.document().revision(), revision);
        assert_eq!(session.document().undo_labels().count(), undo);
        assert!(session.document().is_dirty() && session.has_face_preview());
        let text = session
            .recovery
            .store
            .as_ref()
            .unwrap()
            .read(&entry.id)
            .unwrap()
            .1;
        let recovered = Vmf::parse(&text).unwrap();
        assert_eq!(recovered.world.solids[0].sides[0].material, "BRICK/TEST");
        assert_eq!(recovered.world.properties, original.world.properties);
        assert_eq!(session.recovery_status()["pending_changes"], false);
    }

    #[test]
    fn restore_requires_discard_and_snapshot_guards_and_opens_a_dirty_copy() {
        let (directory, mut session, _) = fixture();
        let path = directory.path().join("original.vmf");
        session.save(Actor::User, Some(&path)).unwrap();
        let saved = std::fs::read(&path).unwrap();
        let entry = capture(&mut session);
        let expected = Some((session.document_generation(), session.document().revision()));
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        let before = session.document().vmf().clone();
        assert!(
            session
                .restore_recovery_snapshot(Actor::Agent, &entry.id, false, None)
                .is_err()
        );
        assert!(
            session
                .restore_recovery_snapshot(Actor::Agent, &entry.id, true, expected)
                .is_err()
        );
        assert!(
            session
                .restore_recovery_snapshot(Actor::Agent, "../outside", true, None)
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
        session
            .restore_recovery_snapshot(Actor::Agent, &entry.id, true, None)
            .unwrap();
        assert!(session.path().is_none() && session.document().is_dirty());
        assert_eq!(session.document().objects().count(), 1);
        assert_eq!(session.document().undo_labels().count(), 0);
        assert!(session.save(Actor::Agent, None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }

    #[test]
    fn older_snapshot_completion_does_not_mark_newer_edits_or_replacement_as_captured() {
        let (_directory, mut session, _) = fixture();
        let request = session.begin_recovery_snapshot(Kind::Autosave).unwrap();
        assert!(session.begin_recovery_snapshot(Kind::Manual).is_err());
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        session
            .finish_recovery_snapshot(Actor::System, &request, request.run())
            .unwrap();
        assert_eq!(session.recovery_status()["pending_changes"], true);
        let request = session.begin_recovery_snapshot(Kind::Autosave).unwrap();
        session.new_map(Actor::User, true).unwrap();
        session
            .finish_recovery_snapshot(Actor::System, &request, request.run())
            .unwrap();
        assert!(session.recovery.last_entry.is_none());
        assert!(!session.recovery_busy());
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 2);
    }

    #[test]
    fn autosave_scheduling_avoids_duplicate_snapshots_and_retries_errors() {
        let (_directory, mut session, _) = fixture();
        assert!(!session.autosave_due());
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        assert!(session.autosave_due());
        let request = session.begin_recovery_snapshot(Kind::Autosave).unwrap();
        assert!(!session.autosave_due());
        assert!(
            session
                .finish_recovery_snapshot(Actor::System, &request, Err("disk full".into()))
                .is_err()
        );
        assert_eq!(session.recovery_status()["error"], "disk full");
        assert!(!session.autosave_due());
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        assert!(session.autosave_due());
        let entry = capture(&mut session);
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        assert!(!session.autosave_due());
        session
            .delete_recovery_snapshot(Actor::Agent, &entry.id)
            .unwrap();
        assert!(session.autosave_due());
        let mut settings = session.recovery_settings().clone();
        settings.enabled = false;
        session
            .set_recovery_settings(Actor::User, settings, None)
            .unwrap();
        assert!(!session.autosave_due());
    }

    #[test]
    fn failed_save_preserves_versions_dirty_state_and_existing_recovery() {
        let (directory, mut session, _) = fixture();
        let path = directory.path().join("map.vmf");
        session.save(Actor::User, Some(&path)).unwrap();
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        let mut settings = session.recovery_settings().clone();
        settings.keep_per_map = 1;
        session
            .set_recovery_settings(Actor::User, settings, None)
            .unwrap();
        let checkpoint = capture(&mut session);
        let before = session.document().vmf().clone();
        let disk = std::fs::read(&path).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(session.save(Actor::User, None).is_err());
        assert_eq!(session.document().vmf(), &before);
        assert!(session.document().is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        let entries = session.recovery_entries().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, checkpoint.id);
        assert!(
            session
                .save(Actor::User, Some(&directory.path().join("missing/map.vmf")))
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
    }

    #[test]
    fn successful_save_keeps_previous_bytes_and_backup_failure_does_not_lose_the_save() {
        let (directory, mut session, _) = fixture();
        let path = directory.path().join("map.vmf");
        session.save(Actor::User, Some(&path)).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        session.save(Actor::Agent, None).unwrap();
        let entries = session.recovery_entries().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, Kind::BeforeSave);
        assert_eq!(
            session
                .recovery
                .store
                .as_ref()
                .unwrap()
                .read(&entries[0].id)
                .unwrap()
                .1,
            first
        );
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        session.configure_recovery(Ok(blocked));
        session
            .create_block(
                Actor::User,
                DVec3::splat(256.0),
                DVec3::splat(320.0),
                "extra",
                false,
            )
            .unwrap();
        session.save(Actor::User, None).unwrap();
        assert!(!session.document().is_dirty());
        assert_eq!(
            Vmf::parse(&std::fs::read_to_string(path).unwrap())
                .unwrap()
                .world
                .solids
                .len(),
            3
        );
        assert!(
            session
                .recent_log()
                .next()
                .unwrap()
                .text
                .contains("backup failed")
        );
    }

    #[test]
    fn settings_guards_preserve_newer_values_and_persist_across_sessions() {
        let (directory, mut session, _) = fixture();
        let version = session.recovery_settings_revision();
        let settings = Settings {
            enabled: false,
            interval_seconds: 60,
            keep_per_map: 4,
        };
        session
            .set_recovery_settings(Actor::Agent, settings.clone(), Some(version))
            .unwrap();
        assert!(
            session
                .set_recovery_settings(Actor::User, Settings::default(), Some(version))
                .is_err()
        );
        assert_eq!(session.recovery_settings(), &settings);
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recovery(Ok(directory.path().join("recovery")));
        assert_eq!(reopened.recovery_settings(), &settings);
    }
}
