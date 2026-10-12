use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub folder: String,
    pub category: String,
    pub used: bool,
    pub favorite: bool,
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
