use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;
use sha2::{Digest, Sha256};
use source_formats::gamefs::{AssetOrigin, GameFs, Mount};

use crate::session::material_source;

#[cfg(test)]
#[path = "materials/provenance_tests.rs"]
mod provenance_tests;

pub const MAX_CATALOG_ENTRIES: usize = 250_000;
pub const MAX_CATALOG_SOURCES: usize = 256;
pub const MAX_QUERY_BYTES: usize = 256;
pub const MAX_GROUP_BYTES: usize = 1024;
pub const MAX_MATERIAL_PAGE: usize = 200;

#[derive(Debug, Default, Clone, Serialize)]
pub struct CatalogFilter {
    pub source: Option<String>,
    pub exclude_generated_map_materials: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub id: String,
    pub kind: &'static str,
    pub mount: String,
    pub paths_truncated: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Provenance {
    pub source_id: Option<String>,
    pub source: Option<material_source::Origin>,
}

pub(crate) fn source_id(origin: &AssetOrigin) -> String {
    let (kind, path) = match origin {
        AssetOrigin::Directory { mount, .. } => (b"directory".as_slice(), mount),
        AssetOrigin::Vpk { index, .. } => (b"vpk".as_slice(), index),
    };
    let mut digest = Sha256::new();
    digest.update(kind);
    digest.update([0]);
    digest.update(path.as_os_str().as_encoded_bytes());
    format!("source:{:x}", digest.finalize())
}

pub(crate) fn provenance(origin: AssetOrigin) -> Provenance {
    Provenance {
        source_id: Some(source_id(&origin)),
        source: Some(material_source::origin(origin)),
    }
}

pub(crate) fn catalog_sources(game: &GameFs) -> Result<Vec<Source>, String> {
    let mut sources = Vec::new();
    for mounted in game
        .mounts
        .iter()
        .filter(|mount| mount.ids.iter().any(|id| id == "game"))
    {
        if sources.len() == MAX_CATALOG_SOURCES {
            return Err(format!(
                "Asset catalogs support at most {MAX_CATALOG_SOURCES} game mounts"
            ));
        }
        let origin = match &mounted.mount {
            Mount::Directory(mount) => AssetOrigin::Directory {
                mount: mount.clone(),
                file: mount.clone(),
            },
            Mount::Pak(pak) => AssetOrigin::Vpk {
                index: pak.path().to_owned(),
                files: Vec::new(),
            },
        };
        let id = source_id(&origin);
        let origin = material_source::origin(origin);
        sources.push(Source {
            id,
            kind: origin.kind,
            mount: origin.mount,
            paths_truncated: origin.paths_truncated,
        });
    }
    Ok(sources)
}

pub(crate) fn validate_source_filter(
    source: Option<&str>,
    sources: &[Source],
) -> Result<(), String> {
    if source.is_some_and(|source| {
        !matches!(source, "all" | "directory" | "vpk" | "unavailable")
            && !sources.iter().any(|candidate| candidate.id == source)
    }) {
        return Err("Choose all, directory, vpk, unavailable or an exact ID from sources".into());
    }
    Ok(())
}

pub(crate) fn source_matches(source: Option<&str>, provenance: &Provenance) -> bool {
    match source {
        None | Some("all") => true,
        Some("unavailable") => provenance.source.is_none(),
        Some("directory" | "vpk") => provenance
            .source
            .as_ref()
            .is_some_and(|origin| Some(origin.kind) == source),
        Some(id) => provenance.source_id.as_deref() == Some(id),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub folder: String,
    pub category: String,
    pub used: bool,
    pub favorite: bool,
    #[serde(flatten)]
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct Catalog {
    pub groups: Vec<Group>,
    pub entries: Vec<Entry>,
    pub total: usize,
    pub offset: usize,
    pub favorites: crate::material_favorites::Status,
    pub sources: Vec<Source>,
    pub filter: CatalogFilter,
    pub provenance_scope: &'static str,
}

pub fn category(name: &str) -> &'static str {
    if name.starts_with("tools/") {
        return "Tools";
    }
    if name.starts_with("models/") {
        return "Models";
    }
    for (label, words) in [
        ("Brick", &["brick"][..]),
        ("Concrete", &["concrete", "cement"][..]),
        ("Metal", &["metal", "steel", "iron"][..]),
        ("Wood", &["wood", "timber"][..]),
        ("Glass", &["glass", "window"][..]),
        ("Liquids", &["water", "liquid"][..]),
        ("Ground", &["grass", "dirt", "sand", "gravel"][..]),
        ("Stone", &["stone", "rock", "marble"][..]),
        ("Tiles", &["tile"][..]),
    ] {
        if words.iter().any(|word| name.contains(word)) {
            return label;
        }
    }
    "Other"
}

pub fn browse(
    names: &[String],
    used: &BTreeSet<String>,
    favorites: &BTreeSet<String>,
    recent: &VecDeque<String>,
    query: &str,
    group: &str,
    page: (usize, usize),
) -> Result<Catalog, String> {
    let (offset, limit) = page;
    let query = query.trim().to_ascii_lowercase();
    let all: BTreeSet<_> = names
        .iter()
        .chain(used)
        .chain(favorites)
        .chain(recent)
        .cloned()
        .collect();
    let mut entries: Vec<_> = all
        .into_iter()
        .filter(|name| name.contains(&query))
        .map(|name| Entry {
            folder: name.split('/').next().unwrap_or("Other").into(),
            category: category(&name).into(),
            used: used.contains(&name),
            favorite: favorites.contains(&name),
            provenance: Provenance::default(),
            name,
        })
        .collect();
    let mut counts = BTreeMap::new();
    for entry in &entries {
        *counts
            .entry((
                format!("folder:{}", entry.folder),
                format!("Folder · {}", entry.folder),
            ))
            .or_insert(0) += 1;
        *counts
            .entry((
                format!("category:{}", entry.category),
                entry.category.clone(),
            ))
            .or_insert(0) += 1;
    }
    let mut groups = vec![
        Group {
            id: "all".into(),
            label: "All materials".into(),
            count: entries.len(),
        },
        Group {
            id: "used".into(),
            label: "Used in this map".into(),
            count: entries.iter().filter(|entry| entry.used).count(),
        },
        Group {
            id: "recent".into(),
            label: "Recently applied".into(),
            count: entries
                .iter()
                .filter(|entry| recent.contains(&entry.name))
                .count(),
        },
        Group {
            id: "favorites".into(),
            label: "Favorites".into(),
            count: entries.iter().filter(|entry| entry.favorite).count(),
        },
    ];
    groups.extend(
        counts
            .into_iter()
            .map(|((id, label), count)| Group { id, label, count }),
    );
    if !groups.iter().any(|candidate| candidate.id == group)
        && !group.starts_with("folder:")
        && !group.starts_with("category:")
    {
        return Err(format!("unknown material group {group}"));
    }
    entries.retain(|entry| match group {
        "all" => true,
        "used" => entry.used,
        "favorites" => entry.favorite,
        "recent" => recent.contains(&entry.name),
        other => {
            other
                .strip_prefix("folder:")
                .is_some_and(|folder| entry.folder == folder)
                || other
                    .strip_prefix("category:")
                    .is_some_and(|category| entry.category == category)
        }
    });
    if group == "recent" {
        entries.sort_by_key(|entry| recent.iter().position(|name| *name == entry.name));
    }
    let total = entries.len();
    Ok(Catalog {
        groups,
        entries: entries
            .into_iter()
            .skip(offset)
            .take(limit.clamp(1, 200))
            .collect(),
        total,
        offset,
        favorites: crate::material_favorites::Status::session(favorites.len()),
        sources: Vec::new(),
        filter: CatalogFilter::default(),
        provenance_scope: "current_winning_vmt; display previews have independent cached identities",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_groups_searches_pages_and_keeps_unmounted_map_materials() {
        let names = vec![
            "brick/wall".into(),
            "concrete/floor".into(),
            "models/brick/prop".into(),
            "tools/toolsnodraw".into(),
        ];
        let used = BTreeSet::from(["custom/floor".into(), "brick/wall".into()]);
        let favorites = BTreeSet::from(["brick/wall".into()]);
        let recent = VecDeque::from(["concrete/floor".into(), "brick/wall".into()]);
        let all = browse(&names, &used, &favorites, &recent, "", "all", (1, 2)).unwrap();
        assert_eq!(all.total, 5);
        assert_eq!(all.entries.len(), 2);
        assert_eq!(
            all.groups
                .iter()
                .find(|group| group.id == "used")
                .unwrap()
                .count,
            2
        );
        let bricks = browse(
            &names,
            &used,
            &favorites,
            &recent,
            "BRICK",
            "category:Brick",
            (0, 36),
        )
        .unwrap();
        assert_eq!(bricks.total, 1);
        assert!(bricks.entries[0].favorite);
        let used = browse(&names, &used, &favorites, &recent, "", "used", (0, 36)).unwrap();
        assert_eq!(used.entries[1].name, "custom/floor");
        let recent = browse(
            &names,
            &BTreeSet::new(),
            &favorites,
            &recent,
            "",
            "recent",
            (0, 36),
        )
        .unwrap();
        assert_eq!(recent.entries[0].name, "concrete/floor");
        assert_eq!(category("models/brick/prop"), "Models");
    }
}
