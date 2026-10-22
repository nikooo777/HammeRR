use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use std::sync::{Arc, Mutex, MutexGuard};

use crate::preference_file::{CoordinatedFile, ObservedSource, Source};

pub(crate) const FILE_NAME: &str = "build-recipes.json";
pub(crate) const LOCK_FILE_NAME: &str = ".build-recipes.lock";
pub(crate) const VERSION: u32 = 1;
pub(crate) const MAX_RECIPES: usize = 32;
pub(crate) const MAX_NAME_BYTES: usize = 80;
pub(crate) const MAX_FILE_BYTES: usize = 65_536;
pub(crate) const MAX_ERROR_BYTES: usize = 512;

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Identity {
    pub(crate) id: String,
    pub(crate) name: String,
}

use crate::build_settings::BuildSettings as Settings;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipe {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) settings: Settings,
}

impl Recipe {
    pub(crate) fn identity(&self) -> Identity {
        Identity {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    pub(crate) fn json(&self, builtin: bool) -> Value {
        let options = self
            .settings
            .options()
            .expect("recipe settings are validated");
        let description = format!(
            "{} · {} · {} · {} · {} · {}° brush smoothing · {} lighting work units · ambient probe refinement {} · brush texture shadows {} · static-prop vertex lighting {} · reflections {} · sky packing {} · water packing {} · static-prop packing {}",
            options.lighting.label(),
            options.physics.label(),
            options.visibility.label(),
            options.sky_quality.label(),
            compiler::bounce_label(options.bounces),
            options.smoothing_angle,
            options.lighting_work_limit,
            if options.refine_ambient_probes {
                "on"
            } else {
                "off"
            },
            if options.brush_texture_shadows {
                "on"
            } else {
                "off"
            },
            if options.bake_static_prop_vertices {
                "on"
            } else {
                "off"
            },
            if options.generate_reflections {
                "on"
            } else {
                "off"
            },
            if options.pack_sky_assets { "on" } else { "off" },
            if options.pack_water_assets {
                "on"
            } else {
                "off"
            },
            if options.pack_static_prop_assets {
                "on"
            } else {
                "off"
            }
        );
        json!({"id":self.id,"name":self.name,"builtin":builtin,"settings":self.settings.report(),"description":description})
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct File {
    pub(crate) version: u32,
    pub(crate) next_id: u64,
    pub(crate) recipes: Vec<Recipe>,
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

struct Registry {
    file: File,
    revision: u64,
    path: Option<PathBuf>,
    disk: Option<ObservedSource>,
    load_failed: bool,
    error: Option<String>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            file: File::default(),
            revision: 1,
            path: None,
            disk: None,
            load_failed: false,
            error: None,
        }
    }
}

pub(crate) fn builtins() -> [Recipe; 2] {
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
                lighting_work_limit: compiler::DEFAULT_LIGHTING_WORK_LIMIT,
                generate_reflections: false,
                pack_sky_assets: false,
                pack_water_assets: false,
                pack_static_prop_assets: false,
                bake_static_prop_vertices: false,
                brush_texture_shadows: false,
                refine_ambient_probes: false,
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

pub(crate) fn validate(file: &File) -> Result<(), String> {
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

fn target(path: &Path) -> Result<CoordinatedFile<'_, &OsStr>, String> {
    Ok(CoordinatedFile {
        directory: path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
        file_name: path
            .file_name()
            .ok_or("Build recipe storage must have a file name")?,
        lock_name: LOCK_FILE_NAME,
        label: "Build recipes",
        reload: "Reload recipes",
        saved_description: "saved recipes",
        max_bytes: MAX_FILE_BYTES as u64,
    })
}

fn load(path: &Path) -> Result<(File, ObservedSource), String> {
    let disk = target(path)?.observe_following()?;
    let file = match &disk.source {
        Source::File { bytes, length, .. } => {
            if *length > MAX_FILE_BYTES as u64 {
                return Err(format!("Build recipe file exceeds {MAX_FILE_BYTES} bytes"));
            }
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?
        }
        Source::Missing => File::default(),
    };
    validate(&file)?;
    Ok((file, disk))
}

#[derive(Clone, Default)]
pub struct Authority {
    state: Arc<Mutex<Registry>>,
}

#[derive(Clone)]
pub(crate) struct Snapshot {
    pub file: File,
    pub revision: u64,
    pub path: Option<PathBuf>,
    pub load_failed: bool,
    pub error: Option<String>,
}

pub(crate) enum Edit<'a> {
    Save {
        name: &'a str,
        replace: Option<&'a str>,
        settings: Settings,
    },
    Rename {
        id: &'a str,
        name: &'a str,
    },
    Delete {
        id: &'a str,
    },
}

pub(crate) fn configured_path(directory: Result<PathBuf, String>) -> Result<PathBuf, String> {
    configured_path_with_override(directory, std::env::var_os("HAMMERR_BUILD_RECIPES"))
}

fn configured_path_with_override(
    directory: Result<PathBuf, String>,
    override_path: Option<OsString>,
) -> Result<PathBuf, String> {
    let path = match override_path {
        Some(path) if path.is_empty() => {
            return Err("HAMMERR_BUILD_RECIPES must be a file path".into());
        }
        Some(path) => PathBuf::from(path),
        None => directory?.join(FILE_NAME),
    };
    if path.to_str().is_none() {
        return Err("Build recipe storage path must be valid UTF-8; set HAMMERR_BUILD_RECIPES to a UTF-8 path and restart".into());
    }
    Ok(path)
}

impl Snapshot {
    pub fn find(&self, id: &str) -> Result<Recipe, String> {
        builtins()
            .into_iter()
            .chain(self.file.recipes.iter().cloned())
            .find(|recipe| recipe.id == id)
            .ok_or_else(|| "Build recipe no longer exists".into())
    }
    pub fn identity(
        &self,
        active: Option<&Identity>,
        options: compiler::Options,
    ) -> Option<Identity> {
        let active = active?;
        self.find(&active.id)
            .ok()
            .filter(|recipe| {
                recipe
                    .settings
                    .options()
                    .is_ok_and(|saved| saved == options)
            })
            .map(|recipe| recipe.identity())
    }
}

impl Authority {
    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        let _ = authority.configure_at(configured_path(directory));
        authority
    }
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.state
            .lock()
            .expect("Build recipes authority was poisoned")
    }
    pub(crate) fn configure_at(&self, path: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        let loaded = path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|path| load(path));
        state.path = path.ok();
        state.revision += 1;
        match loaded {
            Ok((file, disk)) => {
                state.file = file;
                state.disk = Some(disk);
                state.load_failed = false;
                state.error = None;
                Ok(())
            }
            Err(error) => {
                state.load_failed = true;
                let error = format!("Build recipes could not load: {error}");
                state.error = Some(error.clone());
                Err(error)
            }
        }
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.lock().snapshot()
    }
    pub(crate) fn find(
        &self,
        id: &str,
        expected: Option<u64>,
    ) -> Result<(Recipe, Snapshot), String> {
        let state = self.lock();
        state.check_revision(expected)?;
        let snapshot = state.snapshot();
        let recipe = snapshot.find(id)?;
        Ok((recipe, snapshot))
    }
    pub(crate) fn reload(&self, expected: Option<u64>) -> Result<Snapshot, String> {
        let mut state = self.lock();
        state.check_revision(expected)?;
        let path = state
            .path
            .as_deref()
            .ok_or("Build recipe storage is unavailable; session-only recipes remain editable")?;
        let loaded = load(path);
        state.revision += 1;
        match loaded {
            Ok((file, disk)) => {
                state.file = file;
                state.disk = Some(disk);
                state.load_failed = false;
                state.error = None;
                Ok(state.snapshot())
            }
            Err(error) => {
                state.load_failed = true;
                let error = map_core::entity_helpers::bounded_preview(
                    &format!("Build recipes could not reload: {error}"),
                    MAX_ERROR_BYTES,
                );
                state.error = Some(error.clone());
                Err(error)
            }
        }
    }
    pub(crate) fn edit(
        &self,
        expected: Option<u64>,
        edit: Edit<'_>,
    ) -> Result<(Recipe, Snapshot), String> {
        let mut state = self.lock();
        state.check_revision(expected)?;
        let mut file = state.file.clone();
        let recipe = match edit {
            Edit::Save {
                name,
                replace,
                settings,
            } => {
                let name = recipe_name(name)?;
                let id = match replace {
                    Some(id) => {
                        if !file.recipes.iter().any(|recipe| recipe.id == id) {
                            return Err(
                                "Choose an existing custom recipe; built-in recipes cannot change"
                                    .into(),
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
                    settings,
                };
                file.recipes.retain(|recipe| recipe.id != id);
                file.recipes.push(recipe.clone());
                recipe
            }
            Edit::Rename { id, name } => {
                let name = recipe_name(name)?;
                let recipe = file
                    .recipes
                    .iter_mut()
                    .find(|recipe| recipe.id == id)
                    .ok_or("Choose an existing custom recipe; built-in recipes cannot change")?;
                recipe.name = name;
                recipe.clone()
            }
            Edit::Delete { id } => {
                let index = file
                    .recipes
                    .iter()
                    .position(|recipe| recipe.id == id)
                    .ok_or(
                        "Choose an existing custom recipe; built-in recipes cannot be deleted",
                    )?;
                file.recipes.remove(index)
            }
        };
        state.publish(file)?;
        Ok((recipe, state.snapshot()))
    }
}

impl Registry {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            file: self.file.clone(),
            revision: self.revision,
            path: self.path.clone(),
            load_failed: self.load_failed,
            error: self.error.clone(),
        }
    }
    fn check_revision(&self, expected: Option<u64>) -> Result<(), String> {
        if expected.is_some_and(|revision| revision != self.revision) {
            return Err(
                "Build recipes changed; reload the list before applying this action".into(),
            );
        }
        Ok(())
    }
    fn publish(&mut self, file: File) -> Result<(), String> {
        validate(&file)?;
        let state = self;
        let mut write_committed = false;
        let result: Result<(), String> = (|| {
            if state.load_failed {
                return Err("Build recipes did not load; fix storage and Reload recipes before editing custom recipes".into());
            }
            let bytes = serde_json::to_vec_pretty(&file).map_err(|error| error.to_string())?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err("Build recipes exceed the storage limit".into());
            }
            if let Some(path) = &state.path {
                let expected = state
                    .disk
                    .as_ref()
                    .ok_or("Reload recipes before writing saved recipes")?;
                let publication = target(path)?.publish_following(&bytes, expected).map_err(
                    |error| {
                        if error.conflict {
                            format!(
                                "Build recipes changed on disk; Reload recipes before editing: {}",
                                error.message
                            )
                        } else {
                            error.message
                        }
                    },
                )?;
                write_committed = true;
                match publication.source {
                    Ok(source) => state.disk = Some(source),
                    Err(error) => {
                        state.load_failed = true;
                        return Err(format!(
                            "The recipe file was written but verification failed; Reload recipes before editing: {error}"
                        ));
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let error = if write_committed {
                error
            } else {
                format!("Build recipes could not save: {error}")
            };
            state.error = Some(error.clone());
            return Err(error);
        }
        state.file = file;
        state.revision += 1;
        state.error = None;
        Ok(())
    }
}

#[cfg(test)]
#[path = "build_recipes/coordination_tests.rs"]
mod coordination_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Actor, Session};

    #[cfg(unix)]
    #[test]
    fn unrepresentable_configured_paths_report_blocked_storage_without_panicking() {
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(b"recipes-\xff.json".to_vec());
        for (directory, override_path) in [
            (Ok(PathBuf::from("unused")), Some(invalid.clone())),
            (Ok(PathBuf::from(invalid.clone())), None),
        ] {
            let authority = Authority::default();
            let error = authority
                .configure_at(configured_path_with_override(directory, override_path))
                .unwrap_err();
            assert!(error.contains("UTF-8") && error.contains("HAMMERR_BUILD_RECIPES"));
            let mut session =
                Session::with_build_recipes(Err("no assets needed".into()), authority);
            let options = session.compile_options();
            let history = session.history_snapshot();
            let report = session.build_recipes();
            assert!(report["path"].is_null());
            assert_eq!(report["persistence"], "blocked");
            assert_eq!(report["custom_editable"], false);
            assert_eq!(report["active"]["id"], "fast");
            assert!(report["error"].as_str().unwrap().contains("UTF-8"));
            assert_eq!(
                report["recipes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|recipe| recipe["id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["fast", "final"]
            );
            assert!(
                session
                    .save_build_recipe(Actor::Agent, "Blocked", None, None)
                    .is_err()
            );
            assert_eq!(session.compile_options(), options);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.build_recipes()["recipes"], report["recipes"]);
        }
        assert_eq!(
            configured_path_with_override(Err("no default".into()), Some("chosen.json".into()))
                .unwrap(),
            PathBuf::from("chosen.json")
        );
        assert_eq!(
            configured_path_with_override(Ok("chosen".into()), None).unwrap(),
            PathBuf::from("chosen/build-recipes.json")
        );
        assert!(
            configured_path_with_override(Ok("unused".into()), Some(OsString::new()))
                .unwrap_err()
                .contains("must be a file path")
        );
    }

    #[test]
    fn high_registry_revisions_and_under_lock_guards_do_not_alias_local_action_tokens() {
        let authority = Authority::default();
        authority.lock().revision = 1 << 63;
        let mut session = Session::with_build_recipes(Err("no assets".into()), authority.clone());
        let token = session.build_recipes()["revision"].as_u64().unwrap();
        let shared = authority.snapshot().revision;
        authority
            .edit(
                None,
                Edit::Save {
                    name: "Peer",
                    replace: None,
                    settings: compiler::Options::default().into(),
                },
            )
            .unwrap();
        assert!(authority.find("fast", Some(shared)).is_err());
        assert!(
            authority
                .edit(Some(shared), Edit::Delete { id: "custom:1" })
                .is_err()
        );
        assert!(
            session
                .apply_build_recipe(Actor::User, "fast", Some(token))
                .is_err()
        );
        let token = session.build_recipes()["revision"].as_u64().unwrap();
        session
            .apply_build_recipe(Actor::User, "fast", Some(token))
            .unwrap();
    }
}
