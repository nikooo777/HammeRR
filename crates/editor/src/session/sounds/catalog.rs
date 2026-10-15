use std::collections::{BTreeMap, BTreeSet};

use source_formats::{
    gamefs::{GameFs, asset_relative},
    sound::{self, Cue},
};

const MAX_FILES: usize = 100_000;
const MAX_SCRIPTS: usize = 128;
const MAX_SCRIPT_TOTAL: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 32768;

#[derive(Clone)]
pub(super) struct Entry {
    pub value: String,
    pub source: Option<String>,
    pub cue: Option<Cue>,
}

impl Entry {
    pub fn group(&self) -> String {
        self.source.as_ref().map_or_else(
            || {
                format!(
                    "folder:{}",
                    self.value.rsplit_once('/').map_or("", |(folder, _)| folder)
                )
            },
            |source| format!("script:{source}"),
        )
    }
    pub fn kind(&self) -> &'static str {
        if self.cue.is_some() { "script" } else { "file" }
    }
    pub fn wave(&self, variant: usize) -> Result<&str, String> {
        match &self.cue {
            Some(cue) => cue
                .waves
                .get(variant)
                .map(String::as_str)
                .ok_or("Choose a valid sound-script wave variant".into()),
            None if variant == 0 => Ok(&self.value),
            None => Err("Sound files have only variant 0".into()),
        }
    }
}

pub(super) struct Catalog {
    pub entries: Vec<Entry>,
    pub aliases: BTreeMap<String, usize>,
    pub ambiguous: BTreeSet<String>,
    pub warnings: Vec<String>,
}

pub(super) fn load(game: &GameFs) -> Result<Catalog, String> {
    let mut files = game.list("sound", ".wav");
    files.extend(game.list("sound", ".mp3"));
    files.sort();
    files.dedup();
    if files.len() > MAX_FILES {
        return Err("Sound catalog exceeds 100,000 mounted files".into());
    }
    let mut catalog = Catalog {
        entries: files
            .into_iter()
            .filter_map(|path| {
                path.strip_prefix("sound/").map(|value| Entry {
                    value: value.into(),
                    source: None,
                    cue: None,
                })
            })
            .collect(),
        aliases: BTreeMap::new(),
        ambiguous: BTreeSet::new(),
        warnings: Vec::new(),
    };
    let manifest =
        match game.read_limited("scripts/game_sounds_manifest.txt", sound::MAX_SCRIPT_BYTES) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => {
                catalog.warnings.push(
                    "No game_sounds_manifest.txt is mounted; only sound files are listed".into(),
                );
                return Ok(catalog);
            }
            Err(error) => {
                catalog.warnings.push(format!("Sound manifest: {error}"));
                return Ok(catalog);
            }
        };
    let roots = match std::str::from_utf8(&manifest)
        .map_err(|error| error.to_string())
        .and_then(|text| sound::document(text).map_err(|error| error.to_string()))
    {
        Ok(roots) => roots,
        Err(error) => {
            catalog.warnings.push(format!("Sound manifest: {error}"));
            return Ok(catalog);
        }
    };
    let Some(fields) = roots
        .iter()
        .find(|node| node.key.eq_ignore_ascii_case("game_sounds_manifest"))
        .and_then(|node| node.children())
    else {
        catalog
            .warnings
            .push("Sound manifest root is missing; only sound files are listed".into());
        return Ok(catalog);
    };
    let scripts = fields
        .iter()
        .filter(|node| {
            node.key.eq_ignore_ascii_case("preload_file")
                || node.key.eq_ignore_ascii_case("precache_file")
        })
        .filter_map(|node| node.as_str())
        .collect::<Vec<_>>();
    if scripts.len() > MAX_SCRIPTS {
        return Err("Sound manifest exceeds 128 scripts".into());
    }
    let mut bytes = manifest.len();
    let mut seen = BTreeSet::new();
    let mut cue_count = 0;
    for path in scripts {
        let path = match asset_relative(path) {
            Ok(path) if path.starts_with("scripts/") && path.ends_with(".txt") => path,
            _ => {
                catalog
                    .warnings
                    .push(format!("Unsupported sound script path: {path}"));
                continue;
            }
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        let result = (|| {
            let data = game
                .read_limited(&path, sound::MAX_SCRIPT_BYTES)
                .map_err(|error| error.to_string())?
                .ok_or("Script is missing")?;
            bytes += data.len();
            if bytes > MAX_SCRIPT_TOTAL {
                return Err("Sound scripts exceed 32 MiB".into());
            }
            let text = std::str::from_utf8(&data).map_err(|_| "Script is not UTF-8")?;
            sound::parse_script(text).map_err(|error| error.to_string())
        })();
        if bytes > MAX_SCRIPT_TOTAL {
            return Err("Sound scripts exceed 32 MiB".into());
        }
        let cues = match result {
            Ok(cues) => cues,
            Err(error) => {
                catalog.warnings.push(format!("{path}: {error}"));
                continue;
            }
        };
        cue_count += cues.len();
        if cue_count > MAX_ENTRIES {
            return Err("Sound catalog exceeds 32,768 script entries".into());
        }
        for cue in cues {
            let key = cue.name.to_ascii_lowercase();
            if catalog.aliases.contains_key(&key) {
                catalog.ambiguous.insert(key.clone());
            }
            catalog.aliases.insert(key, catalog.entries.len());
            catalog.entries.push(Entry {
                value: cue.name.clone(),
                source: Some(path.clone()),
                cue: Some(cue),
            });
        }
    }
    if !catalog.ambiguous.is_empty() {
        catalog.warnings.push(format!("{} ambiguous script names cannot be auditioned or assigned by this browser; choose a sound file instead",catalog.ambiguous.len()));
    }
    Ok(catalog)
}
