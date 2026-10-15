use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{BuildSettingsUpdate, Session};
use crate::session::Actor;

const FILE_NAME: &str = "build-recipes.json";
const VERSION: u32 = 1;
const MAX_RECIPES: usize = 32;
const MAX_NAME_BYTES: usize = 80;
const MAX_FILE_BYTES: usize = 65_536;

#[derive(Clone, Serialize)]
pub(super) struct Identity {
    id: String,
    name: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    lighting: String,
    physics: String,
    sky_quality: String,
    visibility: String,
    bounces: u8,
    smoothing_angle: u8,
}

impl From<compiler::Options> for Settings {
    fn from(options: compiler::Options) -> Self {
        Self {
            lighting: options.lighting.name().into(),
            physics: options.physics.name().into(),
            sky_quality: options.sky_quality.name().into(),
            visibility: options.visibility.name().into(),
            bounces: options.bounces,
            smoothing_angle: options.smoothing_angle,
        }
    }
}

impl Settings {
    fn options(&self) -> Result<compiler::Options, String> {
        compiler::Options {
            lighting: compiler::LightingMode::parse(&self.lighting)?,
            physics: compiler::PhysicsMode::parse(&self.physics)?,
            sky_quality: compiler::SkyQuality::parse(&self.sky_quality)?,
            visibility: compiler::VisibilityMode::parse(&self.visibility)?,
            bounces: self.bounces,
            smoothing_angle: self.smoothing_angle,
        }
        .validate()
    }

    fn update(&self) -> BuildSettingsUpdate<'_> {
        BuildSettingsUpdate {
            lighting: Some(&self.lighting),
            physics: Some(&self.physics),
            sky_quality: Some(&self.sky_quality),
            visibility: Some(&self.visibility),
            bounces: Some(self.bounces),
            smoothing_angle: Some(self.smoothing_angle),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    id: String,
    name: String,
    settings: Settings,
}

impl Recipe {
    fn identity(&self) -> Identity {
        Identity {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    fn json(&self, builtin: bool) -> Value {
        let options = self
            .settings
            .options()
            .expect("recipe settings are validated");
        let description = format!(
            "{} · {} · {} · {} · {} · {}° brush smoothing",
            options.lighting.label(),
            options.physics.label(),
            options.visibility.label(),
            options.sky_quality.label(),
            compiler::bounce_label(options.bounces),
            options.smoothing_angle
        );
        json!({"id":self.id,"name":self.name,"builtin":builtin,"settings":self.settings,"description":description})
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    next_id: u64,
    recipes: Vec<Recipe>,
}

impl Default for File {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            recipes: Vec::new(),
        }
    }
}

pub(super) struct State {
    file: File,
    revision: u64,
    path: Option<PathBuf>,
    disk: Option<Vec<u8>>,
    load_failed: bool,
    error: Option<String>,
    pub(super) active: Option<Identity>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            file: File::default(),
            revision: 1,
            path: None,
            disk: None,
            load_failed: false,
            error: None,
            active: Some(builtins()[0].identity()),
        }
    }
}

fn builtins() -> [Recipe; 2] {
    [
        Recipe {
            id: "fast".into(),
            name: "Fast preview".into(),
            settings: compiler::Options::default().into(),
        },
        Recipe {
            id: "final".into(),
            name: "Final lighting".into(),
            settings: compiler::Options {
                lighting: compiler::LightingMode::DirectLdr,
                physics: compiler::PhysicsMode::Engine,
                sky_quality: compiler::SkyQuality::Detailed,
                visibility: compiler::VisibilityMode::Detailed,
                bounces: compiler::MAX_LIGHT_BOUNCES,
                smoothing_angle: 45,
            }
            .into(),
        },
    ]
}

fn recipe_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(format!(
            "Recipe name must contain 1–{MAX_NAME_BYTES} bytes without control characters"
        ));
    }
    Ok(name.into())
}

fn validate(file: &File) -> Result<(), String> {
    if file.version != VERSION || file.recipes.len() > MAX_RECIPES || file.next_id == 0 {
        return Err("Unsupported or oversized build recipe file".into());
    }
    let mut ids = HashSet::new();
    let mut names = builtins()
        .map(|recipe| recipe.name.to_lowercase())
        .into_iter()
        .collect::<HashSet<_>>();
    for recipe in &file.recipes {
        let id = recipe
            .id
            .strip_prefix("custom:")
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|id| *id > 0 && *id < file.next_id)
            .ok_or("Invalid custom recipe identifier")?;
        if !ids.insert(id) || !names.insert(recipe.name.to_lowercase()) {
            return Err("Duplicate build recipe identifier or name".into());
        }
        if recipe_name(&recipe.name)? != recipe.name {
            return Err("Stored recipe name must not have surrounding whitespace".into());
        }
        recipe.settings.options()?;
    }
    Ok(())
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("Build recipe file exceeds {MAX_FILE_BYTES} bytes"));
    }
    Ok(Some(bytes))
}

impl Session {
    pub fn configure_build_recipes(&mut self, directory: Result<PathBuf, String>) {
        let path = match std::env::var_os("HAMMERR_BUILD_RECIPES") {
            Some(path) if path.is_empty() => {
                Err("HAMMERR_BUILD_RECIPES must be a file path".into())
            }
            Some(path) => Ok(PathBuf::from(path)),
            None => directory.map(|directory| directory.join(FILE_NAME)),
        };
        self.configure_build_recipes_at(path);
    }

    fn configure_build_recipes_at(&mut self, path: Result<PathBuf, String>) {
        let loaded = path.as_ref().map_err(Clone::clone).and_then(|path| {
            let disk = read(path)?;
            let file = match &disk {
                Some(bytes) => serde_json::from_slice(bytes).map_err(|error| error.to_string())?,
                None => File::default(),
            };
            validate(&file)?;
            Ok((file, disk))
        });
        let state = &mut self.compilation.recipes;
        state.path = path.ok();
        state.revision += 1;
        match loaded {
            Ok((file, disk)) => {
                state.file = file;
                state.disk = disk;
                state.load_failed = false;
                state.error = None;
                if state
                    .active
                    .as_ref()
                    .is_some_and(|active| active.id.starts_with("custom:"))
                {
                    state.active = None;
                }
            }
            Err(error) => {
                state.load_failed = true;
                let error = format!("Build recipes could not load: {error}");
                state.error = Some(error.clone());
                self.log(Actor::System, error);
            }
        }
    }

    pub fn build_recipes(&self) -> Value {
        let state = &self.compilation.recipes;
        let recipes = builtins()
            .iter()
            .map(|recipe| recipe.json(true))
            .chain(state.file.recipes.iter().map(|recipe| recipe.json(false)))
            .collect::<Vec<_>>();
        json!({
            "revision":state.revision,"recipes":recipes,"active":state.active,
            "editable":!self.compiling && !self.closing,
            "custom_editable":!self.compiling && !self.closing && !state.load_failed,
            "persistence":if state.load_failed {"blocked"} else if state.path.is_some() {"persistent"} else {"session"},
            "path":state.path,"error":state.error,
            "limits":{"custom_recipes":MAX_RECIPES,"name_bytes":MAX_NAME_BYTES,"file_bytes":MAX_FILE_BYTES},
            "settings":Settings::from(self.compilation.options),
            "job_rule":"Active builds freeze settings and recipe changes. Jobs retain their original recipe name and options. Applying different settings makes previous output stale; names alone do not.",
        })
    }

    fn check_build_recipe_edit(&self, expected_revision: Option<u64>) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.compiling {
            return Err(
                "Wait for the active compile to finish before changing build recipes".into(),
            );
        }
        if expected_revision.is_some_and(|revision| revision != self.compilation.recipes.revision) {
            return Err(
                "Build recipes changed; reload the list before applying this action".into(),
            );
        }
        Ok(())
    }

    fn find_build_recipe(&self, id: &str) -> Result<Recipe, String> {
        builtins()
            .into_iter()
            .chain(self.compilation.recipes.file.recipes.iter().cloned())
            .find(|recipe| recipe.id == id)
            .ok_or_else(|| "Build recipe no longer exists".into())
    }

    fn publish_build_recipes(&mut self, file: File) -> Result<(), String> {
        validate(&file)?;
        let state = &mut self.compilation.recipes;
        let result: Result<(), String> = (|| {
            if state.load_failed {
                return Err("Build recipes did not load; fix storage and restart before editing custom recipes".into());
            }
            let bytes = serde_json::to_vec_pretty(&file).map_err(|error| error.to_string())?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err("Build recipes exceed the storage limit".into());
            }
            if let Some(path) = &state.path {
                if read(path)? != state.disk {
                    return Err(
                        "Build recipes changed on disk; restart to reload before editing".into(),
                    );
                }
                if fs::metadata(path).is_ok_and(|metadata| metadata.permissions().readonly()) {
                    return Err("Build recipe storage is read-only".into());
                }
                let directory = path
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                fs::create_dir_all(directory).map_err(|error| error.to_string())?;
                let mut temporary = tempfile::NamedTempFile::new_in(directory)
                    .map_err(|error| error.to_string())?;
                temporary
                    .write_all(&bytes)
                    .map_err(|error| error.to_string())?;
                temporary
                    .as_file()
                    .sync_all()
                    .map_err(|error| error.to_string())?;
                temporary.persist(path).map_err(|error| error.to_string())?;
                state.disk = Some(bytes);
            }
            Ok(())
        })();
        if let Err(error) = result {
            let error = format!("Build recipes could not save: {error}");
            state.error = Some(error.clone());
            self.log(Actor::System, &error);
            return Err(error);
        }
        state.file = file;
        state.revision += 1;
        state.error = None;
        Ok(())
    }

    pub fn save_build_recipe(
        &mut self,
        actor: Actor,
        name: &str,
        replace_id: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        self.check_build_recipe_edit(expected_revision)?;
        let name = recipe_name(name)?;
        let mut file = self.compilation.recipes.file.clone();
        let id = match replace_id {
            Some(id) => {
                if !file.recipes.iter().any(|recipe| recipe.id == id) {
                    return Err(
                        "Choose an existing custom recipe; built-in recipes cannot change".into(),
                    );
                }
                id.to_owned()
            }
            None => {
                if file.recipes.len() == MAX_RECIPES {
                    return Err(format!("Keep at most {MAX_RECIPES} custom build recipes"));
                }
                let id = format!("custom:{}", file.next_id);
                file.next_id = file
                    .next_id
                    .checked_add(1)
                    .ok_or("Build recipe identifiers exhausted")?;
                id
            }
        };
        let recipe = Recipe {
            id: id.clone(),
            name,
            settings: self.compilation.options.into(),
        };
        file.recipes.retain(|recipe| recipe.id != id);
        file.recipes.push(recipe.clone());
        self.publish_build_recipes(file)?;
        self.compilation.recipes.active = Some(recipe.identity());
        self.log(actor, format!("Saved build recipe {}", recipe.name));
        Ok(self.build_recipes())
    }

    pub fn rename_build_recipe(
        &mut self,
        actor: Actor,
        id: &str,
        name: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        self.check_build_recipe_edit(expected_revision)?;
        let name = recipe_name(name)?;
        let mut file = self.compilation.recipes.file.clone();
        let recipe = file
            .recipes
            .iter_mut()
            .find(|recipe| recipe.id == id)
            .ok_or("Choose an existing custom recipe; built-in recipes cannot change")?;
        recipe.name = name.clone();
        self.publish_build_recipes(file)?;
        if let Some(active) = &mut self.compilation.recipes.active
            && active.id == id
        {
            active.name = name.clone();
        }
        self.log(actor, format!("Renamed build recipe to {name}"));
        Ok(self.build_recipes())
    }

    pub fn delete_build_recipe(
        &mut self,
        actor: Actor,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        self.check_build_recipe_edit(expected_revision)?;
        let mut file = self.compilation.recipes.file.clone();
        let index = file
            .recipes
            .iter()
            .position(|recipe| recipe.id == id)
            .ok_or("Choose an existing custom recipe; built-in recipes cannot be deleted")?;
        let recipe = file.recipes.remove(index);
        self.publish_build_recipes(file)?;
        if self
            .compilation
            .recipes
            .active
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            self.compilation.recipes.active = None;
        }
        self.log(actor, format!("Deleted build recipe {}", recipe.name));
        Ok(self.build_recipes())
    }

    pub fn apply_build_recipe(
        &mut self,
        actor: Actor,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        self.check_build_recipe_edit(expected_revision)?;
        let recipe = self.find_build_recipe(id)?;
        self.update_build_settings(actor, recipe.settings.update())?;
        self.compilation.recipes.active = Some(recipe.identity());
        self.log(actor, format!("Applied build recipe {}", recipe.name));
        Ok(self.build_recipes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game;
    use map_core::Document;
    use source_formats::keyvalues::WriteOptions;

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
            session
                .apply_build_recipe(actor, &id, Some(revision))
                .unwrap();
            assert_eq!(session.compile_settings()["lighting"], "direct_ldr");
            assert_eq!(session.compile_settings()["physics"], "none");
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
            let (directory, game, _) = game::tests::fixture(true);
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
                None,
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
        let original = fs::read(&path).unwrap();
        let mut restarted = Session::new(Err("no game needed".into()));
        restarted.configure_build_recipes_at(Ok(path.clone()));
        assert_eq!(restarted.build_recipes()["recipes"][2]["name"], "Geometry");
        assert_eq!(restarted.build_recipes()["active"]["id"], "fast");
        restarted
            .apply_build_recipe(Actor::Agent, &id, None)
            .unwrap();
        assert_eq!(restarted.compile_options(), session.compile_options());
        let before = restarted.build_recipes();
        let permissions = fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
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
        restarted
            .rename_build_recipe(Actor::Agent, &id, "Renamed", None)
            .unwrap();
        assert!(restarted.build_recipes()["error"].is_null());
        assert!(
            session
                .delete_build_recipe(Actor::User, &id, None)
                .unwrap_err()
                .contains("changed on disk")
        );
        assert_eq!(session.build_recipes()["recipes"][2]["name"], "Geometry");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
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
