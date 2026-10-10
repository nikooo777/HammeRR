//! Finding installed games through Steam's library folders.

use std::path::{Path, PathBuf};

use source_formats::keyvalues::{self, Node};

const CSS_GAMEINFO: &str = "steamapps/common/Counter-Strike Source/cstrike/gameinfo.txt";

fn steam_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        ".local/share/Steam",
        ".steam/steam",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    ]
    .iter()
    .map(|p| home.join(p))
    .filter(|p| p.is_dir())
    .collect()
}

/// Every Steam library: each root plus the extra folders listed in its
/// `libraryfolders.vdf`.
fn libraries() -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    for root in steam_roots() {
        libraries.push(root.clone());
        let Ok(text) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) else {
            continue;
        };
        let Ok(nodes) = keyvalues::parse_lenient(&text) else {
            continue;
        };
        let folders = nodes.iter().filter_map(Node::children).flatten();
        for folder in folders.filter_map(Node::children) {
            if let Some(path) = folder.iter().find(|n| n.key.eq_ignore_ascii_case("path")) {
                libraries.extend(path.as_str().map(PathBuf::from));
            }
        }
    }
    let mut seen = Vec::new();
    libraries.retain(|p| {
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        let new = !seen.contains(&canonical);
        seen.push(canonical);
        new
    });
    libraries
}

/// `gameinfo.txt` of the first Counter-Strike: Source install found.
pub fn find_css_gameinfo() -> Option<PathBuf> {
    libraries()
        .into_iter()
        .map(|lib| lib.join(CSS_GAMEINFO))
        .find(|p| p.is_file())
}

/// The SDK's sample map sources, a sensible place to start browsing.
pub fn sample_maps_dir(gameinfo: &Path) -> Option<PathBuf> {
    let common = gameinfo.parent()?.parent()?.parent()?;
    let dir = common.join("sourcesdk_content/cstrike/mapsrc");
    dir.is_dir().then_some(dir)
}
