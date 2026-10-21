use super::Session;
pub(super) use crate::build_recipes::Identity;
use crate::build_recipes::{
    self, Authority, Edit, MAX_FILE_BYTES, MAX_NAME_BYTES, MAX_RECIPES, Snapshot, builtins,
};
use crate::build_settings::BuildSettings as Settings;
use crate::session::Actor;
use serde_json::{Value, json};
use std::cell::Cell;
use std::path::PathBuf;

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

pub(super) struct State {
    binding: Binding,
    pub(super) active: Option<Identity>,
    local_revision: u64,
    observed: Cell<(u64, u64)>,
    revision: Cell<u64>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            binding: Binding::default(),
            active: Some(builtins()[0].identity()),
            local_revision: 0,
            observed: Cell::new((0, 0)),
            revision: Cell::new(0),
        }
    }
}
impl State {
    fn revision(&self, shared: u64) -> u64 {
        let current = (shared, self.local_revision);
        if self.observed.get() != current {
            self.revision.set(
                self.revision
                    .get()
                    .checked_add(1)
                    .expect("Build recipe form revisions exhausted"),
            );
            self.observed.set(current);
        }
        self.revision.get()
    }
    pub(super) fn set_active(&mut self, active: Option<Identity>) {
        if self.active != active {
            self.active = active;
            self.changed_local();
        }
    }
    pub(super) fn changed_local(&mut self) {
        self.local_revision = self
            .local_revision
            .checked_add(1)
            .expect("Build recipe local revisions exhausted");
    }
}

impl Session {
    pub fn with_build_recipes(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_build_recipes(authority);
        session
    }
    pub(crate) fn use_application_build_recipes(&mut self, authority: Authority) {
        self.compilation.recipes.binding = Binding::Application(authority);
        self.compilation.recipes.changed_local();
        if let Some(error) = self
            .compilation
            .recipes
            .binding
            .authority()
            .snapshot()
            .error
        {
            self.log(Actor::System, error);
        }
    }
    pub fn configure_build_recipes(&mut self, directory: Result<PathBuf, String>) {
        if matches!(self.compilation.recipes.binding, Binding::Independent(_)) {
            self.configure_build_recipes_at(build_recipes::configured_path(directory));
        }
    }
    fn configure_build_recipes_at(&mut self, path: Result<PathBuf, String>) {
        let result = match &self.compilation.recipes.binding {
            Binding::Independent(authority) => authority.configure_at(path),
            Binding::Application(_) => return,
        };
        match result {
            Ok(()) => {
                if self
                    .compilation
                    .recipes
                    .active
                    .as_ref()
                    .is_some_and(|active| active.id.starts_with("custom:"))
                {
                    self.compilation.recipes.set_active(None);
                }
            }
            Err(error) => self.log(Actor::System, error),
        }
    }
    pub(super) fn active_build_recipe(&self) -> Option<Identity> {
        let snapshot = self.compilation.recipes.binding.authority().snapshot();
        snapshot.identity(
            self.compilation.recipes.active.as_ref(),
            self.compilation.options,
        )
    }
    pub fn build_recipes(&self) -> Value {
        self.build_recipes_observation().0
    }
    pub fn build_recipes_observation(&self) -> (Value, u64) {
        let snapshot = self.compilation.recipes.binding.authority().snapshot();
        (
            self.build_recipes_with_snapshot(&snapshot),
            snapshot.revision,
        )
    }
    fn build_recipes_with_snapshot(&self, snapshot: &Snapshot) -> Value {
        let recipes = builtins()
            .iter()
            .map(|recipe| recipe.json(true))
            .chain(
                snapshot
                    .file
                    .recipes
                    .iter()
                    .map(|recipe| recipe.json(false)),
            )
            .collect::<Vec<_>>();
        json!({"revision":self.compilation.recipes.revision(snapshot.revision),"recipes":recipes,"active":snapshot.identity(self.compilation.recipes.active.as_ref(),self.compilation.options),
            "editable":!self.compiling()&&!self.closing,"custom_editable":!self.compiling()&&!self.closing&&!snapshot.load_failed,
            "persistence":if snapshot.load_failed{"blocked"}else if snapshot.path.is_some(){"persistent"}else{"session"},"path":snapshot.path,"error":snapshot.error,
            "limits":{"custom_recipes":MAX_RECIPES,"name_bytes":MAX_NAME_BYTES,"file_bytes":MAX_FILE_BYTES},"settings":Settings::from(self.compilation.options).report(),
            "job_rule":"Active builds block settings and recipe actions in their originating document. Peers may edit saved recipes; jobs retain their captured recipe name and options. Applying different settings makes previous output stale; names alone do not."})
    }
    fn check_build_recipe_edit(&self, expected: Option<u64>) -> Result<Option<u64>, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.compiling() {
            return Err(
                "Wait for the active compile to finish before changing build recipes".into(),
            );
        }
        if let Some(expected) = expected {
            let snapshot = self.compilation.recipes.binding.authority().snapshot();
            if expected != self.compilation.recipes.revision(snapshot.revision) {
                return Err(
                    "Build recipes changed; reload the list before applying this action".into(),
                );
            }
            Ok(Some(snapshot.revision))
        } else {
            Ok(None)
        }
    }
    fn recipe_result<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if let Err(error) = &result
            && error.starts_with("Build recipes could not save:")
        {
            self.log(Actor::System, error);
        }
        result
    }
    pub fn reload_build_recipes(
        &mut self,
        actor: Actor,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let expected = self.check_build_recipe_edit(expected_revision)?;
        let result = self
            .compilation
            .recipes
            .binding
            .authority()
            .reload(expected);
        match result {
            Ok(snapshot) => {
                let active = snapshot.identity(
                    self.compilation.recipes.active.as_ref(),
                    self.compilation.options,
                );
                self.compilation.recipes.set_active(active);
                self.log(
                    actor,
                    "Reloaded build recipes; current build settings are unchanged",
                );
                Ok(self.build_recipes_with_snapshot(&snapshot))
            }
            Err(error) => {
                if error.starts_with("Build recipes could not reload:") {
                    self.log(actor, &error);
                }
                Err(error)
            }
        }
    }
    pub fn save_build_recipe(
        &mut self,
        actor: Actor,
        name: &str,
        replace_id: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let expected = self.check_build_recipe_edit(expected_revision)?;
        let result = self.compilation.recipes.binding.authority().edit(
            expected,
            Edit::Save {
                name,
                replace: replace_id,
                settings: self.compilation.options.into(),
            },
        );
        let (recipe, snapshot) = self.recipe_result(result)?;
        self.compilation.recipes.set_active(Some(recipe.identity()));
        self.log(actor, format!("Saved build recipe {}", recipe.name));
        Ok(self.build_recipes_with_snapshot(&snapshot))
    }
    pub fn rename_build_recipe(
        &mut self,
        actor: Actor,
        id: &str,
        name: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let expected = self.check_build_recipe_edit(expected_revision)?;
        let result = self
            .compilation
            .recipes
            .binding
            .authority()
            .edit(expected, Edit::Rename { id, name });
        let (recipe, snapshot) = self.recipe_result(result)?;
        if self
            .compilation
            .recipes
            .active
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            self.compilation.recipes.set_active(Some(recipe.identity()));
        }
        self.log(actor, format!("Renamed build recipe to {}", recipe.name));
        Ok(self.build_recipes_with_snapshot(&snapshot))
    }
    pub fn delete_build_recipe(
        &mut self,
        actor: Actor,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let expected = self.check_build_recipe_edit(expected_revision)?;
        let result = self
            .compilation
            .recipes
            .binding
            .authority()
            .edit(expected, Edit::Delete { id });
        let (recipe, snapshot) = self.recipe_result(result)?;
        if self
            .compilation
            .recipes
            .active
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            self.compilation.recipes.set_active(None);
        }
        self.log(actor, format!("Deleted build recipe {}", recipe.name));
        Ok(self.build_recipes_with_snapshot(&snapshot))
    }
    pub fn apply_build_recipe(
        &mut self,
        actor: Actor,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let expected = self.check_build_recipe_edit(expected_revision)?;
        let (recipe, snapshot) = self
            .compilation
            .recipes
            .binding
            .authority()
            .find(id, expected)?;
        self.update_build_settings(actor, recipe.settings.update())?;
        self.compilation.recipes.set_active(Some(recipe.identity()));
        self.log(actor, format!("Applied build recipe {}", recipe.name));
        Ok(self.build_recipes_with_snapshot(&snapshot))
    }
}

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod lighting_work_tests;
#[cfg(test)]
mod reload_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_recipes::{FILE_NAME, File, Recipe, VERSION, validate};
    use crate::session::compilation::BuildSettingsUpdate;
    use map_core::Document;
    use source_formats::keyvalues::WriteOptions;
    use std::fs;

    fn session() -> Session {
        Session::new(Err("no game needed".into()))
    }

    fn active_id(state: &Value) -> &str {
        state["active"]["id"].as_str().unwrap()
    }

    fn map_state(session: &Session) -> Value {
        let document = session.document();
        json!({
            "vmf":document.vmf().write(&WriteOptions::default()),
            "generation":session.document_generation(),"revision":document.revision(),
            "dirty":document.is_dirty(),"path":session.path(),
            "undo":document.undo_labels().collect::<Vec<_>>(),
            "redo":document.redo_labels().collect::<Vec<_>>(),
            "selection":format!("{:?}",session.selected_ids()),
        })
    }

    #[test]
    fn recipes_share_settings_and_leave_map_history_unchanged() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = session();
            let before = map_state(&session);
            let initial = session.build_recipes();
            assert_eq!(initial["recipes"].as_array().unwrap().len(), 2);
            assert_eq!(initial["persistence"], "session");
            session.apply_build_recipe(actor, "final", None).unwrap();
            let settings = session.compile_settings();
            assert_eq!(settings["lighting"], "direct_ldr");
            assert_eq!(settings["physics"], "engine");
            assert_eq!(settings["visibility"], "detailed");
            assert_eq!(settings["sky_quality"], "detailed");
            assert_eq!(settings["bounces"], 4);
            assert_eq!(settings["smoothing_angle"], 45);
            assert_eq!(settings["recipe"]["id"], "final");
            session
                .update_compile_settings(actor, None, Some("none"))
                .unwrap();
            assert!(session.compile_settings()["recipe"].is_null());
            let saved = session
                .save_build_recipe(actor, " Lighting pass ", None, None)
                .unwrap();
            let id = active_id(&saved).to_owned();
            let revision = saved["revision"].as_u64().unwrap();
            session.apply_build_recipe(actor, "fast", None).unwrap();
            assert_eq!(session.compile_settings()["lighting"], "fullbright");
            assert!(
                session
                    .apply_build_recipe(actor, &id, Some(revision))
                    .is_err()
            );
            let revision = session.build_recipes()["revision"].as_u64().unwrap();
            session
                .apply_build_recipe(actor, &id, Some(revision))
                .unwrap();
            assert_eq!(session.compile_settings()["lighting"], "direct_ldr");
            assert_eq!(session.compile_settings()["physics"], "none");
            let revision = session.build_recipes()["revision"].as_u64().unwrap();
            session
                .rename_build_recipe(actor, &id, "Night pass", Some(revision))
                .unwrap();
            assert!(
                session
                    .delete_build_recipe(actor, &id, Some(revision))
                    .is_err()
            );
            assert!(
                session
                    .apply_build_recipe(actor, "fast", Some(revision))
                    .is_err()
            );
            session
                .save_build_recipe(actor, "Night pass", Some(&id), None)
                .unwrap();
            assert_eq!(
                session.build_recipes()["recipes"].as_array().unwrap().len(),
                3
            );
            assert!(
                session
                    .save_build_recipe(actor, "FAST PREVIEW", None, None)
                    .is_err()
            );
            assert!(
                session
                    .save_build_recipe(actor, "Night pass", None, None)
                    .is_err()
            );
            assert!(session.save_build_recipe(actor, "", None, None).is_err());
            assert!(
                session
                    .save_build_recipe(actor, "bad\nname", None, None)
                    .is_err()
            );
            assert!(
                session
                    .save_build_recipe(actor, &"é".repeat(MAX_NAME_BYTES), None, None)
                    .is_err()
            );
            assert!(
                session
                    .rename_build_recipe(actor, "fast", "Changed", None)
                    .is_err()
            );
            assert!(session.delete_build_recipe(actor, "final", None).is_err());
            assert!(
                session
                    .save_build_recipe(actor, "Changed", Some("final"), None)
                    .is_err()
            );
            let options = session.compile_options();
            session.delete_build_recipe(actor, &id, None).unwrap();
            assert_eq!(session.compile_options(), options);
            assert!(session.compile_settings()["recipe"].is_null());
            assert_eq!(map_state(&session), before);
        }
    }

    #[test]
    fn recipes_freeze_active_jobs_and_preserve_completed_job_names() {
        for actor in [Actor::User, Actor::Agent] {
            let (directory, game, _) = crate::test_assets::game_process(true);
            let mut session = Session::new(Ok(game));
            session.replace_document(
                Document::new(crate::test_assets::room()),
                Some(directory.path().join("room.vmf")),
            );
            let saved = session
                .save_build_recipe(actor, "Geometry", None, None)
                .unwrap();
            let id = active_id(&saved).to_owned();
            let before = map_state(&session);
            let request = session.begin_compile(actor, None).unwrap();
            assert_eq!(
                session.compile_status()["job"]["recipe"]["name"],
                "Geometry"
            );
            assert!(!session.build_recipes()["editable"].as_bool().unwrap());
            assert!(
                session
                    .save_build_recipe(actor, "Other", None, None)
                    .is_err()
            );
            assert!(
                session
                    .rename_build_recipe(actor, &id, "Renamed", None)
                    .is_err()
            );
            assert!(session.delete_build_recipe(actor, &id, None).is_err());
            assert!(session.apply_build_recipe(actor, "final", None).is_err());
            let result = request.run();
            session.finish_compile(actor, &result);
            assert!(result.as_ref().unwrap().output.bsp.is_some());
            assert!(session.launch_readiness().is_ok());
            session
                .rename_build_recipe(actor, &id, "Renamed", None)
                .unwrap();
            assert_eq!(session.compile_settings()["recipe"]["name"], "Renamed");
            assert_eq!(
                session.compile_status()["job"]["recipe"]["name"],
                "Geometry"
            );
            assert!(!session.compile_status()["job"]["stale"].as_bool().unwrap());
            assert!(session.launch_readiness().is_ok());
            session.set_compile_settings(actor, "direct_ldr").unwrap();
            assert!(session.compile_settings()["recipe"].is_null());
            assert!(session.launch_readiness().is_err());
            session.apply_build_recipe(actor, &id, None).unwrap();
            assert!(session.launch_readiness().is_ok());
            session.delete_build_recipe(actor, &id, None).unwrap();
            assert_eq!(
                session.compile_status()["job"]["recipe"]["name"],
                "Geometry"
            );
            assert!(session.launch_readiness().is_ok());
            let request = session.begin_compile(actor, None).unwrap();
            let token = session.compile_token().unwrap();
            session.cancel_compile(actor, token).unwrap();
            assert!(session.apply_build_recipe(actor, "fast", None).is_err());
            let result = request.run();
            session.finish_compile(actor, &result);
            assert!(result.is_err());
            assert!(session.build_recipes()["editable"].as_bool().unwrap());
            assert_eq!(map_state(&session), before);
        }
    }

    #[test]
    fn recipe_persistence_roundtrips_and_retries_failed_writes_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences").join(FILE_NAME);
        let mut session = session();
        session.configure_build_recipes_at(Ok(path.clone()));
        session
            .apply_build_recipe(Actor::User, "final", None)
            .unwrap();
        session
            .update_compile_options(
                Actor::User,
                Some("direct_ldr_hdr"),
                Some("none"),
                Some("balanced"),
                None,
                Some(2),
            )
            .unwrap();
        let saved = session
            .save_build_recipe(Actor::User, "Geometry", None, None)
            .unwrap();
        let id = active_id(&saved).to_owned();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let original = fs::read(&path).unwrap();
        let mut restarted = Session::new(Err("no game needed".into()));
        restarted.configure_build_recipes_at(Ok(path.clone()));
        assert_eq!(restarted.build_recipes()["recipes"][2]["name"], "Geometry");
        assert_eq!(restarted.build_recipes()["active"]["id"], "fast");
        restarted
            .apply_build_recipe(Actor::Agent, &id, None)
            .unwrap();
        assert_eq!(restarted.compile_options(), session.compile_options());
        assert_eq!(restarted.compile_settings()["lighting"], "direct_ldr_hdr");
        assert_eq!(
            restarted.build_recipes()["recipes"][0]["settings"]["lighting"],
            "fullbright"
        );
        assert_eq!(
            restarted.build_recipes()["recipes"][1]["settings"]["lighting"],
            "direct_ldr"
        );
        assert!(
            restarted.build_recipes()["recipes"][2]["description"]
                .as_str()
                .unwrap()
                .contains("Direct LDR + HDR")
        );
        let permissions = fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        restarted.reload_build_recipes(Actor::Agent, None).unwrap();
        let before = restarted.build_recipes();
        assert!(
            restarted
                .rename_build_recipe(Actor::Agent, &id, "Renamed", None)
                .unwrap_err()
                .contains("read-only")
        );
        assert_eq!(restarted.build_recipes()["recipes"], before["recipes"]);
        assert_eq!(restarted.build_recipes()["active"], before["active"]);
        assert_eq!(restarted.build_recipes()["revision"], before["revision"]);
        assert!(restarted.build_recipes()["error"].is_string());
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::set_permissions(&path, permissions).unwrap();
        restarted.reload_build_recipes(Actor::Agent, None).unwrap();
        restarted
            .rename_build_recipe(Actor::Agent, &id, "Renamed", None)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        assert!(restarted.build_recipes()["error"].is_null());
        assert!(
            session
                .delete_build_recipe(Actor::User, &id, None)
                .unwrap_err()
                .contains("changed on disk")
        );
        assert_eq!(session.build_recipes()["recipes"][2]["name"], "Geometry");
        let entries = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            entries,
            [FILE_NAME, build_recipes::LOCK_FILE_NAME]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect()
        );
    }

    #[test]
    fn malformed_oversized_or_unsupported_recipes_block_writes_without_replacing_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut file = File {
            next_id: 2,
            ..File::default()
        };
        file.recipes.push(Recipe {
            id: "custom:1".into(),
            name: "Invalid".into(),
            settings: Settings {
                bounces: 5,
                ..compiler::Options::default().into()
            },
        });
        let invalid_bounces = serde_json::to_vec(&file).unwrap();
        file.recipes[0].settings.bounces = 0;
        file.version = VERSION + 1;
        let invalid_version = serde_json::to_vec(&file).unwrap();
        file.version = VERSION;
        file.recipes[0].settings.physics = "arbitrary-command".into();
        for bytes in [
            b"broken".to_vec(),
            vec![b' '; MAX_FILE_BYTES + 1],
            invalid_bounces,
            invalid_version,
            serde_json::to_vec(&file).unwrap(),
        ] {
            fs::write(&path, &bytes).unwrap();
            let mut session = session();
            session.configure_build_recipes_at(Ok(path.clone()));
            assert_eq!(session.build_recipes()["persistence"], "blocked");
            assert!(session.build_recipes()["error"].is_string());
            assert!(
                session
                    .save_build_recipe(Actor::User, "Safe", None, None)
                    .is_err()
            );
            session
                .apply_build_recipe(Actor::User, "fast", None)
                .unwrap();
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn reflection_recipes_roundtrip_and_legacy_files_default_off() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut current = session();
        current.configure_build_recipes_at(Ok(path.clone()));
        current
            .update_build_settings(
                Actor::User,
                BuildSettingsUpdate {
                    lighting: Some("direct_ldr"),
                    generate_reflections: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        current
            .save_build_recipe(Actor::User, "Reflections", None, None)
            .unwrap();
        let mut restarted = session();
        restarted.configure_build_recipes_at(Ok(path.clone()));
        restarted
            .apply_build_recipe(Actor::Agent, "custom:1", None)
            .unwrap();
        assert!(restarted.compile_options().generate_reflections);
        assert!(
            restarted.build_recipes()["recipes"][2]["description"]
                .as_str()
                .unwrap()
                .contains("reflections on")
        );
        for builtin in ["fast", "final"] {
            restarted
                .apply_build_recipe(Actor::Agent, builtin, None)
                .unwrap();
            assert!(!restarted.compile_options().generate_reflections);
        }
        let mut legacy: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        legacy["recipes"][0]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("generate_reflections");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mut restored = session();
        restored.configure_build_recipes_at(Ok(path));
        restored
            .apply_build_recipe(Actor::Agent, "custom:1", None)
            .unwrap();
        assert!(!restored.compile_options().generate_reflections);
        assert_eq!(
            restored.compile_options().lighting,
            compiler::LightingMode::DirectLdr
        );
    }

    #[test]
    fn asset_packing_legacy_recipe_bytes_and_builtin_defaults_stay_compatible() {
        let legacy = br#"{"version":1,"next_id":2,"recipes":[{"id":"custom:1","name":"Legacy","settings":{"lighting":"fullbright","physics":"none","sky_quality":"fast","visibility":"fast","bounces":0,"smoothing_angle":0,"generate_reflections":false}}]}"#;
        let file: File = serde_json::from_slice(legacy).unwrap();
        validate(&file).unwrap();
        assert_eq!(
            file.recipes[0].settings.options().unwrap(),
            compiler::Options::default()
        );
        assert_eq!(serde_json::to_vec(&file).unwrap(), legacy.as_slice());
        let mut current = session();
        for recipe in builtins() {
            assert!(!recipe.settings.pack_sky_assets);
            assert!(!recipe.settings.pack_water_assets);
            assert!(!recipe.settings.pack_static_prop_assets);
            assert!(
                recipe.json(true)["description"]
                    .as_str()
                    .unwrap()
                    .contains("sky packing off")
            );
            assert!(
                recipe.json(true)["description"]
                    .as_str()
                    .unwrap()
                    .contains("water packing off")
            );
            assert!(
                recipe.json(true)["description"]
                    .as_str()
                    .unwrap()
                    .ends_with("static-prop packing off")
            );
            current
                .update_build_settings(
                    Actor::Agent,
                    BuildSettingsUpdate {
                        pack_sky_assets: Some(true),
                        pack_water_assets: Some(true),
                        pack_static_prop_assets: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            current
                .apply_build_recipe(Actor::User, &recipe.id, None)
                .unwrap();
            assert_eq!(
                current.compile_options(),
                recipe.settings.options().unwrap()
            );
        }
    }

    #[test]
    fn prop_vertex_lighting_recipes_restart_restore_and_reject_incompatible_saved_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut current = session();
        current.configure_build_recipes_at(Ok(path.clone()));
        current
            .update_build_settings(
                Actor::User,
                BuildSettingsUpdate {
                    lighting: Some("direct_ldr_hdr"),
                    bake_static_prop_vertices: Some(true),
                    bounces: Some(compiler::MAX_LIGHT_BOUNCES),
                    ..Default::default()
                },
            )
            .unwrap();
        current
            .save_build_recipe(Actor::User, "Lit props", None, None)
            .unwrap();
        let retained = current.compile_options();
        let bytes = fs::read(&path).unwrap();
        let mut restarted = session();
        restarted.configure_build_recipes_at(Ok(path.clone()));
        restarted
            .apply_build_recipe(Actor::Agent, "custom:1", None)
            .unwrap();
        assert_eq!(restarted.compile_options(), retained);
        assert!(
            restarted.build_recipes()["recipes"][2]["description"]
                .as_str()
                .unwrap()
                .contains("static-prop vertex lighting on")
        );
        for id in ["fast", "final"] {
            restarted.apply_build_recipe(Actor::User, id, None).unwrap();
            assert!(!restarted.compile_options().bake_static_prop_vertices);
        }
        let mut invalid: Value = serde_json::from_slice(&bytes).unwrap();
        invalid["recipes"][0]["settings"]["bounces"] = (compiler::MAX_LIGHT_BOUNCES + 1).into();
        let invalid = serde_json::to_vec(&invalid).unwrap();
        fs::write(&path, &invalid).unwrap();
        let before = restarted.build_recipes()["recipes"].clone();
        let options = restarted.compile_options();
        assert!(restarted.reload_build_recipes(Actor::Agent, None).is_err());
        assert_eq!(restarted.build_recipes()["recipes"], before);
        assert_eq!(restarted.compile_options(), options);
        assert_eq!(fs::read(&path).unwrap(), invalid);
        fs::write(&path, bytes).unwrap();
        restarted.reload_build_recipes(Actor::User, None).unwrap();
        restarted
            .apply_build_recipe(Actor::User, "custom:1", None)
            .unwrap();
        restarted
            .apply_build_recipe(Actor::User, "fast", None)
            .unwrap();
        restarted
            .save_build_recipe(Actor::User, "Lit props", Some("custom:1"), None)
            .unwrap();
        assert!(
            !String::from_utf8(fs::read(&path).unwrap())
                .unwrap()
                .contains("bake_static_prop_vertices")
        );
    }

    #[test]
    fn asset_packing_custom_recipes_restart_restore_replace_and_freeze() {
        for actor in [Actor::User, Actor::Agent] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(FILE_NAME);
            let mut current = session();
            current.configure_build_recipes_at(Ok(path.clone()));
            let before = map_state(&current);
            current
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        lighting: Some("direct_ldr_hdr"),
                        physics: Some("engine"),
                        sky_quality: Some("balanced"),
                        visibility: Some("detailed"),
                        lighting_work_limit: None,
                        bounces: Some(2),
                        smoothing_angle: Some(35),
                        generate_reflections: Some(true),
                        pack_sky_assets: Some(true),
                        pack_water_assets: Some(true),
                        pack_static_prop_assets: Some(true),
                        bake_static_prop_vertices: Some(false),
                    },
                )
                .unwrap();
            assert!(current.compile_options().pack_water_assets);
            assert!(current.compile_options().pack_static_prop_assets);
            let retained = current.compile_options();
            current
                .save_build_recipe(actor, "Portable", None, None)
                .unwrap();
            let bytes = fs::read(&path).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["recipes"][0]["settings"]["pack_sky_assets"],
                true
            );
            let mut restarted = session();
            restarted.configure_build_recipes_at(Ok(path.clone()));
            assert!(!restarted.compile_options().pack_sky_assets);
            assert!(!restarted.compile_options().pack_water_assets);
            assert!(!restarted.compile_options().pack_static_prop_assets);
            let saved = restarted.build_recipes();
            assert_eq!(saved["recipes"][2]["settings"]["pack_sky_assets"], true);
            assert_eq!(saved["recipes"][2]["settings"]["pack_water_assets"], true);
            assert_eq!(
                saved["recipes"][2]["settings"]["pack_static_prop_assets"],
                true
            );
            assert!(
                saved["recipes"][2]["description"]
                    .as_str()
                    .unwrap()
                    .contains("sky packing on")
            );
            assert!(
                saved["recipes"][2]["description"]
                    .as_str()
                    .unwrap()
                    .contains("water packing on")
            );
            assert!(
                saved["recipes"][2]["description"]
                    .as_str()
                    .unwrap()
                    .ends_with("static-prop packing on")
            );
            restarted
                .apply_build_recipe(actor, "custom:1", None)
                .unwrap();
            assert_eq!(restarted.compile_options(), retained);
            restarted.compiling = true;
            let frozen = restarted.build_recipes();
            assert!(restarted.apply_build_recipe(actor, "fast", None).is_err());
            assert!(
                restarted
                    .save_build_recipe(actor, "Changed", Some("custom:1"), None)
                    .is_err()
            );
            assert!(
                restarted
                    .rename_build_recipe(actor, "custom:1", "Changed", None)
                    .is_err()
            );
            assert!(
                restarted
                    .delete_build_recipe(actor, "custom:1", None)
                    .is_err()
            );
            assert_eq!(restarted.build_recipes(), frozen);
            assert_eq!(restarted.compile_options(), retained);
            assert_eq!(fs::read(&path).unwrap(), bytes);
            restarted.compiling = false;
            restarted.apply_build_recipe(actor, "fast", None).unwrap();
            assert!(!restarted.compile_options().pack_sky_assets);
            assert!(!restarted.compile_options().pack_water_assets);
            assert!(!restarted.compile_options().pack_static_prop_assets);
            restarted
                .save_build_recipe(actor, "Portable", Some("custom:1"), None)
                .unwrap();
            let replaced = String::from_utf8(fs::read(&path).unwrap()).unwrap();
            assert!(!replaced.contains("pack_sky_assets"));
            assert!(!replaced.contains("pack_water_assets"));
            assert!(!replaced.contains("pack_static_prop_assets"));
            let mut restored = session();
            restored.configure_build_recipes_at(Ok(path));
            restored
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_sky_assets: Some(true),
                        pack_water_assets: Some(true),
                        pack_static_prop_assets: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            restored
                .apply_build_recipe(actor, "custom:1", None)
                .unwrap();
            assert_eq!(restored.compile_options(), compiler::Options::default());
            assert_eq!(map_state(&current), before);
            assert_eq!(map_state(&restarted), before);
            assert_eq!(map_state(&restored), before);
        }
    }

    #[test]
    fn invalid_asset_packing_recipe_records_block_writes_and_preserve_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut current = session();
        current.configure_build_recipes_at(Ok(path.clone()));
        current
            .update_build_settings(
                Actor::Agent,
                BuildSettingsUpdate {
                    pack_sky_assets: Some(true),
                    pack_water_assets: Some(true),
                    pack_static_prop_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        current
            .save_build_recipe(Actor::Agent, "Portable", None, None)
            .unwrap();
        let original = fs::read(&path).unwrap();
        let valid: Value = serde_json::from_slice(&original).unwrap();
        let retained = current.build_recipes()["recipes"].clone();
        let options = current.compile_options();
        let before = map_state(&current);
        let mut invalid = Vec::new();
        for value in [json!(null), json!("true"), json!(1), json!([]), json!({})] {
            for field in [
                "pack_sky_assets",
                "pack_water_assets",
                "pack_static_prop_assets",
            ] {
                let mut file = valid.clone();
                file["recipes"][0]["settings"][field] = value.clone();
                invalid.push(serde_json::to_vec(&file).unwrap());
            }
        }
        let mut unknown = valid;
        unknown["recipes"][0]["settings"]["pack_sky_asset"] = json!(true);
        invalid.push(serde_json::to_vec(&unknown).unwrap());
        for field in [
            "pack_sky_assets",
            "pack_water_assets",
            "pack_static_prop_assets",
        ] {
            let duplicate = String::from_utf8(original.clone()).unwrap().replace(
                &format!("\"{field}\": true"),
                &format!("\"{field}\": true, \"{field}\": false"),
            );
            invalid.push(duplicate.into_bytes());
        }
        for bytes in invalid {
            fs::write(&path, &bytes).unwrap();
            current.configure_build_recipes_at(Ok(path.clone()));
            let state = current.build_recipes();
            assert_eq!(state["persistence"], "blocked");
            assert_eq!(state["custom_editable"], false);
            assert!(state["error"].as_str().unwrap().contains("could not load"));
            assert_eq!(state["recipes"], retained);
            assert!(
                current
                    .save_build_recipe(Actor::Agent, "Changed", Some("custom:1"), None)
                    .is_err()
            );
            assert!(
                current
                    .delete_build_recipe(Actor::Agent, "custom:1", None)
                    .is_err()
            );
            assert_eq!(current.compile_options(), options);
            assert_eq!(map_state(&current), before);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn custom_recipe_limit_and_failed_replacement_preserve_registry() {
        let mut session = session();
        for index in 0..MAX_RECIPES {
            session
                .save_build_recipe(Actor::User, &format!("Recipe {index}"), None, None)
                .unwrap();
        }
        let before = session.build_recipes();
        assert!(
            session
                .save_build_recipe(Actor::Agent, "Overflow", None, None)
                .is_err()
        );
        assert!(
            session
                .save_build_recipe(Actor::Agent, "Recipe 0", Some("custom:2"), None)
                .is_err()
        );
        assert_eq!(session.build_recipes(), before);
        session
            .save_build_recipe(Actor::Agent, "Replacement", Some("custom:1"), None)
            .unwrap();
        session
            .delete_build_recipe(Actor::Agent, "custom:2", None)
            .unwrap();
        let state = session
            .save_build_recipe(Actor::Agent, "New", None, None)
            .unwrap();
        assert_eq!(active_id(&state), "custom:33");
    }
}
