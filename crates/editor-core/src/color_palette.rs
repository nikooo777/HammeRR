use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::{Converted, Input, Space};
use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};

pub const FILE_NAME: &str = "color-palette.json";
pub const MAX_ENTRIES: usize = 16;
pub const MAX_NAME_CHARACTERS: usize = 48;
pub const MAX_NAME_BYTES: usize = 128;
pub const MAX_FILE_BYTES: u64 = 16 * 1024;
const MAX_ERROR_BYTES: usize = 512;
const VERSION: u32 = 1;
pub(crate) type Entries = BTreeMap<String, [f64; 3]>;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PaletteSource {
    #[default]
    User,
    Map,
}

impl PaletteSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Map => "map",
        }
    }
}

pub struct PaletteChange<'a> {
    pub action: &'a str,
    pub snapshot: &'a str,
    pub name: Option<&'a str>,
    pub color: Option<Input>,
    pub new_name: Option<&'a str>,
    pub property_token: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    rgb1: [f64; 3],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    entries: Vec<Entry>,
}

struct Store {
    directory: PathBuf,
}

impl Store {
    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".color-palette.lock",
            label: "Color palette",
            reload: "Reload palette",
            saved_description: "saved colors",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    fn load(&self) -> Result<(BTreeMap<String, [f64; 3]>, ObservedSource), String> {
        let source = self.target().observe()?;
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok((BTreeMap::new(), source));
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "Color palette exceeds {MAX_FILE_BYTES} bytes; file is preserved"
            ));
        }
        let preferences: Preferences =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if preferences.version != VERSION {
            return Err("Unsupported color palette version; file is preserved".into());
        }
        let entries = validated_entries(
            preferences
                .entries
                .into_iter()
                .map(|entry| (entry.name, entry.rgb1)),
        )?;
        Ok((entries, source))
    }

    fn save(
        &self,
        entries: &BTreeMap<String, [f64; 3]>,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        let preferences = Preferences {
            version: VERSION,
            entries: entries
                .iter()
                .map(|(name, rgb1)| Entry {
                    name: name.clone(),
                    rgb1: *rgb1,
                })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&preferences)
            .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&bytes, expected)
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.trim() != name
        || name.len() > MAX_NAME_BYTES
        || name.chars().count() > MAX_NAME_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "Color names require 1–{MAX_NAME_CHARACTERS} characters, at most {MAX_NAME_BYTES} UTF-8 bytes, without control characters or surrounding whitespace"
        ));
    }
    Ok(())
}

pub(crate) fn validated_entries(
    colors: impl IntoIterator<Item = (String, [f64; 3])>,
) -> Result<BTreeMap<String, [f64; 3]>, String> {
    let mut entries = BTreeMap::new();
    for (name, rgb1) in colors {
        if entries.len() == MAX_ENTRIES {
            return Err(format!(
                "Color palette contains more than {MAX_ENTRIES} colors"
            ));
        }
        validate_name(&name)?;
        let color = crate::color::convert(Input {
            space: Space::Rgb1,
            channels: rgb1,
        })?;
        if entries.insert(name, color.rgb1).is_some() {
            return Err("Color palette has duplicate names; stored data is preserved".into());
        }
    }
    Ok(entries)
}

pub(crate) fn prepare_change(
    action: &str,
    name: Option<&str>,
    input: Option<Input>,
    new_name: Option<&str>,
) -> Result<Option<Converted>, String> {
    if action == "reload" {
        if name.is_some() || input.is_some() || new_name.is_some() {
            return Err("Reload accepts only action, source and snapshot".into());
        }
        return Ok(None);
    }
    let name = name.ok_or("Choose a saved color name")?;
    validate_name(name)?;
    if action == "rename" {
        let new_name = new_name.ok_or("Enter a new saved color name")?;
        validate_name(new_name)?;
        if new_name == name {
            return Err("Choose a different saved color name".into());
        }
    } else if new_name.is_some() {
        return Err("Only Rename accepts a new saved color name".into());
    }
    match action {
        "save" | "replace" => {
            crate::color::convert(input.ok_or("Save and Replace require a color")?).map(Some)
        }
        "delete" | "use" | "rename" if input.is_none() => Ok(None),
        "delete" | "use" | "rename" => Err("Use, Delete and Rename do not accept a color".into()),
        _ => Err("Choose Save, Replace, Rename, Delete, Reload or Use".into()),
    }
}

pub(crate) fn change_entries(
    entries: &mut BTreeMap<String, [f64; 3]>,
    action: &str,
    name: &str,
    converted: Option<Converted>,
    new_name: Option<&str>,
) -> Result<(), String> {
    match action {
        "save" => {
            if entries.contains_key(name) {
                return Err("This color name exists; choose Replace saved color explicitly".into());
            }
            if entries.len() == MAX_ENTRIES {
                return Err(format!(
                    "Keep at most {MAX_ENTRIES} saved colors; replace or delete one"
                ));
            }
            entries.insert(name.into(), converted.expect("Save has a color").rgb1);
        }
        "replace" => {
            let entry = entries
                .get_mut(name)
                .ok_or("Choose an existing color to replace")?;
            *entry = converted.expect("Replace has a color").rgb1;
        }
        "delete" => {
            entries
                .remove(name)
                .ok_or("Choose an existing color to delete")?;
        }
        "rename" => {
            let new_name = new_name.expect("Rename has a new name");
            if entries.contains_key(new_name) {
                return Err("This color name exists; choose a different saved color name".into());
            }
            let color = entries
                .remove(name)
                .ok_or("Choose an existing color to rename")?;
            entries.insert(new_name.into(), color);
        }
        _ => unreachable!(),
    }
    Ok(())
}

pub(crate) fn entries_report(entries: &BTreeMap<String, [f64; 3]>) -> Vec<Value> {
    entries
        .iter()
        .map(|(name, rgb1)| {
            let Converted {
                rgb255, hsv, hsl, ..
            } = crate::color::convert(Input {
                space: Space::Rgb1,
                channels: *rgb1,
            })
            .expect("Saved palette colors are validated");
            json!({"name":name,"rgb1":rgb1,"rgb255":rgb255,"hsv":hsv,"hsl":hsl})
        })
        .collect()
}

#[derive(Clone)]
pub struct Authority(Arc<Mutex<State>>);

struct State {
    scope: String,
    serial: u64,
    store: Result<Store, String>,
    entries: BTreeMap<String, [f64; 3]>,
    source: Option<ObservedSource>,
    ready: bool,
    error: Option<String>,
    unverified_write: Option<String>,
    write_committed: bool,
}

impl Default for Authority {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            scope: crate::recovery::unique_id(),
            serial: 0,
            store: Err("Color palette storage is not configured".into()),
            entries: BTreeMap::new(),
            source: None,
            ready: false,
            error: Some("Color palette storage is not configured".into()),
            unverified_write: None,
            write_committed: false,
        })))
    }
}

impl Authority {
    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        let _ = authority.configure(directory);
        authority
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().expect("Color palette authority was poisoned")
    }

    pub(crate) fn configure(&self, directory: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        state.store = directory.map(|directory| Store { directory });
        state.reload()
    }

    pub(crate) fn report(&self) -> Value {
        self.lock().report()
    }

    pub(crate) fn change(
        &self,
        action: &str,
        snapshot: &str,
        name: Option<&str>,
        input: Option<Input>,
    ) -> Result<Value, String> {
        self.change_named(action, snapshot, name, input, None)
    }

    pub(crate) fn rename(
        &self,
        snapshot: &str,
        name: &str,
        new_name: &str,
    ) -> Result<Value, String> {
        self.change_named("rename", snapshot, Some(name), None, Some(new_name))
    }

    pub(crate) fn change_named(
        &self,
        action: &str,
        snapshot: &str,
        name: Option<&str>,
        input: Option<Input>,
        new_name: Option<&str>,
    ) -> Result<Value, String> {
        let converted = prepare_change(action, name, input, new_name)?;
        if action == "reload" {
            let mut state = self.lock();
            state.check_snapshot(snapshot)?;
            state.reload()?;
            return Ok(state.report());
        }
        let name = name.expect("Palette action has a name");
        let mut state = self.lock();
        state.check_snapshot(snapshot)?;
        state.check_source()?;
        if action == "use" {
            let color = state
                .entries
                .get(name)
                .ok_or("The saved color no longer exists")?;
            let color = crate::color::convert(Input {
                space: Space::Rgb1,
                channels: *color,
            })?;
            return Ok(json!({"color":color,"palette":state.report()}));
        }
        let mut entries = state.entries.clone();
        change_entries(&mut entries, action, name, converted, new_name)?;
        state.write_committed = false;
        let publication = {
            let store = state.store.as_ref().map_err(Clone::clone)?;
            let source = state
                .source
                .as_ref()
                .ok_or("Reload palette before writing")?;
            store.save(&entries, source)
        };
        match publication {
            Ok(publication) => state.accept(entries, publication),
            Err(error) => {
                Err(state.fail(format!("Color palette could not save: {}", error.message)))
            }
        }
    }
}

impl State {
    fn error(&self) -> Option<String> {
        match (&self.unverified_write, &self.error) {
            (Some(committed), Some(error)) if committed != error => {
                Some(format!("{committed}; {error}"))
            }
            (Some(error), _) | (None, Some(error)) => Some(error.clone()),
            (None, None) => None,
        }
    }

    fn accept(
        &mut self,
        entries: BTreeMap<String, [f64; 3]>,
        publication: Publication,
    ) -> Result<Value, String> {
        self.entries = entries;
        self.write_committed = true;
        match publication.source {
            Ok(source) => {
                self.source = Some(source);
                self.ready = true;
                self.error = None;
                self.unverified_write = None;
                self.serial += 1;
                Ok(self.report())
            }
            Err(error) => {
                let error = map_core::entity_helpers::bounded_preview(&error, MAX_ERROR_BYTES);
                self.source = None;
                self.unverified_write = Some(error.clone());
                Err(self.fail(error))
            }
        }
    }

    fn snapshot(&self) -> String {
        format!("palette-v1:{}:{}", self.scope, self.serial)
    }

    fn check_snapshot(&self, snapshot: &str) -> Result<(), String> {
        if snapshot == self.snapshot() {
            Ok(())
        } else {
            Err("Color palette changed; refresh the saved color choices before retrying".into())
        }
    }

    fn fail(&mut self, error: String) -> String {
        let error = map_core::entity_helpers::bounded_preview(&error, MAX_ERROR_BYTES);
        self.ready = false;
        self.error = Some(error.clone());
        self.serial += 1;
        self.error().expect("Palette failure has an error")
    }

    fn reload(&mut self) -> Result<(), String> {
        self.write_committed = self.unverified_write.is_some();
        let result = self
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(Store::load);
        match result {
            Ok((entries, source)) => {
                self.entries = entries;
                self.source = Some(source);
                self.ready = true;
                self.error = None;
                self.unverified_write = None;
                self.write_committed = false;
                self.serial += 1;
                Ok(())
            }
            Err(error) => Err(self.fail(format!("Color palette could not load: {error}"))),
        }
    }

    fn check_source(&mut self) -> Result<(), String> {
        if !self.ready {
            return Err(self.error().unwrap_or_else(|| {
                "Color palette is unavailable; repair storage and Reload palette".into()
            }));
        }
        self.write_committed = false;
        let result = self.store.as_ref().map_err(Clone::clone).and_then(|store| {
            let expected = self.source.as_ref().ok_or("Reload palette before using saved colors")?;
            let current = store.target().observe_from(expected)?;
            if current.source != expected.source {
                return Err("Color palette changed outside this editor; Reload palette before using or changing it".into());
            }
            Ok(())
        });
        result.map_err(|error| self.fail(error))
    }

    fn report(&self) -> Value {
        let entries = entries_report(&self.entries);
        json!({"source":"user","snapshot":self.snapshot(),"entries":entries,"ready":self.ready,
            "stored":matches!(self.source.as_ref().map(|source|&source.source),Some(Source::File{..})),
            "error":self.error(),"path":self.store.as_ref().ok().map(|store|store.directory.join(FILE_NAME).display().to_string()),
            "reload_available":self.store.is_ok(),"write_committed":self.write_committed,
            "limits":{"entries":MAX_ENTRIES,"name_characters":MAX_NAME_CHARACTERS,"name_bytes":MAX_NAME_BYTES,"file_bytes":MAX_FILE_BYTES},
            "policy":"One shared user palette of exact normalized RGB colors. Choose the map source for undoable map-local saved colors. Brightness is independent. Explicit palette writes persist even when a map color draft is Cancelled. No gamma conversion or automatic file reload. Participating Linux writers coordinate; external changes require Reload palette."})
    }
}
