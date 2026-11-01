//! Finding installed games through Steam's library folders.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use source_formats::gamefs::GameFs;
use source_formats::keyvalues::{self, Node};

const CSS_GAMEINFO: &str = "steamapps/common/Counter-Strike Source/cstrike/gameinfo.txt";

#[cfg(not(windows))]
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

#[cfg(windows)]
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(std::env::var_os("STEAM_DIR").map(PathBuf::from));
    roots.extend(windows_steam_root());
    for variable in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        roots.extend(std::env::var_os(variable).map(|path| PathBuf::from(path).join("Steam")));
    }
    roots.retain(|path| path.is_dir());
    roots
}

#[cfg(windows)]
fn windows_steam_root() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

    let key: Vec<u16> = "Software\\Valve\\Steam\0".encode_utf16().collect();
    let value: Vec<u16> = "SteamPath\0".encode_utf16().collect();
    let mut length = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut length,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut buffer = vec![0u16; (length as usize).div_ceil(std::mem::size_of::<u16>())];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    buffer.truncate(length as usize / std::mem::size_of::<u16>());
    let path = buffer.strip_suffix(&[0]).unwrap_or(&buffer);
    Some(PathBuf::from(OsString::from_wide(path)))
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
    let mut seen = HashSet::new();
    libraries.retain(|path| seen.insert(path.canonicalize().unwrap_or_else(|_| path.clone())));
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

pub fn find_css_fgd(game: &GameFs) -> Option<PathBuf> {
    let mut paths = vec![game.game_dir.join("cstrike.fgd")];
    if let Some(common) = game.game_dir.parent().and_then(Path::parent) {
        paths.extend(
            ["SourceSDK", "sourcesdk"]
                .map(|sdk| common.join(sdk).join("bin/source2009/bin/cstrike.fgd")),
        );
    }
    for library in libraries() {
        paths.extend(["SourceSDK", "sourcesdk"].map(|sdk| {
            library
                .join("steamapps/common")
                .join(sdk)
                .join("bin/source2009/bin/cstrike.fgd")
        }));
    }
    paths.into_iter().find(|path| path.is_file())
}
