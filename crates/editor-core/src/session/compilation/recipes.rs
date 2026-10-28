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
