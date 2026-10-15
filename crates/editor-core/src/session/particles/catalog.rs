use source_formats::{gamefs::GameFs, particle};
use std::{
    collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_SYSTEMS: usize = 16384;
const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Entry {
    pub definition: particle::Definition,
    pub source: String,
    pub precache: bool,
    pub encoded_bytes: usize,
    pub elements: usize,
    pub fingerprint: u64,
}

pub(super) struct Catalog {
    pub entries: Vec<Entry>,
    pub aliases: BTreeMap<String, usize>,
    pub ambiguous: BTreeSet<String>,
    pub files: usize,
}

pub(super) fn read(game: &GameFs, path: &str, limit: usize) -> Result<Vec<u8>, String> {
    game.read_limited(path, limit)
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("{path} is missing from the content mounts"))
}

pub(super) fn load(game: &GameFs) -> Result<Catalog, String> {
    let manifest = read(
        game,
        "particles/particles_manifest.txt",
        particle::MAX_MANIFEST_BYTES,
    )?;
    let files =
        particle::manifest(&manifest).map_err(|error| format!("Particle manifest: {error}"))?;
    let mut catalog = Catalog {
        entries: Vec::new(),
        aliases: BTreeMap::new(),
        ambiguous: BTreeSet::new(),
        files: files.len(),
    };
    let mut total = 0usize;
    let mut metadata_bytes = 0usize;
    for file in files {
        let data = read(
            game,
            &file.path,
            particle::MAX_FILE_BYTES.min(MAX_TOTAL_BYTES - total),
        )?;
        total += data.len();
        let parsed = particle::parse(&data).map_err(|error| format!("{}: {error}", file.path))?;
        if catalog.entries.len() + parsed.definitions.len() > MAX_SYSTEMS {
            return Err("Particle catalog exceeds 16,384 named systems".into());
        }
        let mut fingerprint = DefaultHasher::new();
        data.hash(&mut fingerprint);
        let fingerprint = fingerprint.finish();
        for definition in parsed.definitions {
            metadata_bytes += definition.name.len()
                + file.path.len()
                + definition.material.as_ref().map_or(0, String::len)
                + definition
                    .functions
                    .values()
                    .flatten()
                    .map(String::len)
                    .sum::<usize>()
                + definition.children.iter().map(String::len).sum::<usize>();
            if metadata_bytes > MAX_METADATA_BYTES {
                return Err("Particle catalog metadata exceeds 16 MiB".into());
            }
            catalog.entries.push(Entry {
                definition,
                source: file.path.clone(),
                precache: file.precache,
                encoded_bytes: data.len(),
                elements: parsed.elements,
                fingerprint,
            });
        }
    }
    catalog.entries.sort_by_cached_key(|entry| {
        (
            entry.definition.name.to_ascii_lowercase(),
            entry.source.clone(),
        )
    });
    for (index, entry) in catalog.entries.iter().enumerate() {
        let key = entry.definition.name.to_ascii_lowercase();
        if catalog.aliases.insert(key.clone(), index).is_some() {
            catalog.ambiguous.insert(key);
        }
    }
    Ok(catalog)
}
