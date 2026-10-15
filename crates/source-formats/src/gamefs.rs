//! The engine's layered game filesystem, configured from `gameinfo.txt`.
//!
//! Files are looked up by game path (`materials/tools/toolsnodraw.vmt`)
//! through an ordered list of mounts; the first mount that has the file wins.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::keyvalues::{self, Node};
use crate::vpk::Vpk;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    Syntax {
        path: PathBuf,
        source: keyvalues::ParseError,
    },
    #[error("{0}: no GameInfo/FileSystem/SearchPaths block")]
    NoSearchPaths(PathBuf),
}

/// Lowercase, forward slashes, no leading `/` or `./`.
pub fn normalize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for part in path.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&part.to_ascii_lowercase());
    }
    out
}

pub fn asset_relative(path: &str) -> io::Result<String> {
    if path.starts_with(['/', '\\'])
        || path.contains([':', '\0'])
        || path.split(['/', '\\']).any(|part| part == "..")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "asset paths must be relative to a content mount and cannot contain '..'",
        ));
    }
    Ok(normalize(path))
}

#[derive(Debug)]
pub enum Mount {
    Directory(PathBuf),
    Pak(Vpk),
}

impl Mount {
    fn read_limited(&self, path: &str, maximum: usize) -> io::Result<Option<Vec<u8>>> {
        match self {
            Self::Pak(vpk) => vpk.read_limited(path, maximum),
            Self::Directory(root) => match find_case_insensitive(root, path)? {
                Some(file) if fs::metadata(&file)?.is_file() => {
                    let file = fs::File::open(file)?;
                    let size = file.metadata()?.len();
                    if size > maximum as u64 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "asset exceeds the caller's byte read limit",
                        ));
                    }
                    let mut data = Vec::new();
                    data.try_reserve_exact(size as usize)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    file.take(maximum.saturating_add(1) as u64)
                        .read_to_end(&mut data)?;
                    if data.len() > maximum {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "asset grew beyond its byte read limit",
                        ));
                    }
                    Ok(Some(data))
                }
                Some(_) => Ok(None),
                None => Ok(None),
            },
        }
    }

    pub fn location(&self) -> &Path {
        match self {
            Self::Directory(root) => root,
            Self::Pak(vpk) => vpk.path(),
        }
    }
}

fn existing<T>(path: &Path, result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}

/// Resolves `relative` under `root`, matching each component without regard
/// to case when the exact path doesn't exist.
fn find_case_insensitive(root: &Path, relative: &str) -> io::Result<Option<PathBuf>> {
    let Some(boundary) = existing(root, root.canonicalize())? else {
        return Ok(None);
    };
    let exact = root.join(relative);
    if let Some(canonical) = existing(&exact, exact.canonicalize())? {
        return Ok(canonical.starts_with(&boundary).then_some(exact));
    }
    let mut current = root.to_path_buf();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        let exact = current.join(part);
        current = if existing(&exact, fs::metadata(&exact))?.is_some() {
            exact
        } else {
            let Some(entries) = existing(&current, fs::read_dir(&current))? else {
                return Ok(None);
            };
            let mut matched = None;
            for entry in entries {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(part))
                {
                    matched = Some(entry.path());
                    break;
                }
            }
            let Some(matched) = matched else {
                return Ok(None);
            };
            matched
        };
        let Some(canonical) = existing(&current, current.canonicalize())? else {
            return Ok(None);
        };
        if !canonical.starts_with(&boundary) {
            return Ok(None);
        }
    }
    Ok(Some(current))
}

#[derive(Debug)]
pub struct MountedPath {
    /// Search path IDs such as `game`, `mod`, `platform`, lowercase.
    pub ids: Vec<String>,
    pub mount: Mount,
}

#[derive(Debug)]
pub struct GameFs {
    /// The folder containing `gameinfo.txt` (e.g. `.../cstrike`).
    pub game_dir: PathBuf,
    /// The install root that relative search paths start from.
    pub base_dir: PathBuf,
    pub mounts: Vec<MountedPath>,
}

impl GameFs {
    pub fn from_gameinfo(gameinfo: impl AsRef<Path>) -> Result<Self, Error> {
        let gameinfo = gameinfo.as_ref();
        let io_err = |path: &Path| {
            let path = path.to_path_buf();
            move |source| Error::Io { path, source }
        };
        let text = fs::read_to_string(gameinfo).map_err(io_err(gameinfo))?;
        let nodes = keyvalues::parse_lenient(&text).map_err(|source| Error::Syntax {
            path: gameinfo.to_path_buf(),
            source,
        })?;
        let search_paths = find_block(&nodes, &["GameInfo", "FileSystem", "SearchPaths"])
            .ok_or_else(|| Error::NoSearchPaths(gameinfo.to_path_buf()))?;

        let game_dir = gameinfo.parent().map(Path::to_path_buf).unwrap_or_default();
        let base_dir = game_dir.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut gamefs = Self {
            game_dir,
            base_dir,
            mounts: Vec::new(),
        };
        for node in search_paths {
            let Some(value) = node.as_str() else {
                continue;
            };
            let ids: Vec<String> = node
                .key
                .split('+')
                .map(|id| id.trim().to_ascii_lowercase())
                .collect();
            gamefs.mount_search_path(ids, value)?;
        }
        Ok(gamefs)
    }

    fn resolve(&self, value: &str) -> PathBuf {
        let absolute = PathBuf::from(value.replace('\\', "/"));
        if absolute.is_absolute() {
            return absolute;
        }
        let mut rest = value;
        let mut base = &self.base_dir;
        for (token, dir) in [
            ("|gameinfo_path|", &self.game_dir),
            ("|all_source_engine_paths|", &self.base_dir),
        ] {
            if let Some(stripped) = rest.strip_prefix(token) {
                rest = stripped;
                base = dir;
            }
        }
        rest.split(['/', '\\'])
            .filter(|p| !p.is_empty() && *p != ".")
            .fold(base.clone(), |path, part| path.join(part))
    }

    fn mount_search_path(&mut self, ids: Vec<String>, value: &str) -> Result<(), Error> {
        if let Some(dir) = value
            .strip_suffix("/*")
            .or_else(|| value.strip_suffix("\\*"))
        {
            let dir = self.resolve(dir);
            let io_err = |source| Error::Io {
                path: dir.clone(),
                source,
            };
            let Some(entries) = existing(&dir, fs::read_dir(&dir)).map_err(io_err)? else {
                return Ok(());
            };
            let mut children = entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()
                .map_err(io_err)?;
            children.sort();
            for child in children {
                let name = child
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let Some(metadata) =
                    existing(&child, fs::metadata(&child)).map_err(|source| Error::Io {
                        path: child.clone(),
                        source,
                    })?
                else {
                    continue;
                };
                if metadata.is_dir() {
                    self.add(ids.clone(), Mount::Directory(child));
                } else if name.ends_with("_dir.vpk") {
                    let vpk = Vpk::open(&child).map_err(|e| Error::Io {
                        path: child,
                        source: e,
                    })?;
                    self.add(ids.clone(), Mount::Pak(vpk));
                }
            }
            return Ok(());
        }

        let path = self.resolve(value);
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vpk"))
        {
            let index = Vpk::dir_file_for(&path);
            if existing(&index, fs::metadata(&index))
                .map_err(|source| Error::Io {
                    path: index,
                    source,
                })?
                .is_some_and(|metadata| metadata.is_file())
            {
                let vpk = Vpk::open(&path).map_err(|e| Error::Io { path, source: e })?;
                self.add(ids, Mount::Pak(vpk));
            }
        } else if existing(&path, fs::metadata(&path))
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?
            .is_some_and(|metadata| metadata.is_dir())
        {
            self.add(ids, Mount::Directory(path));
        }
        Ok(())
    }

    /// Adds a mount, merging IDs into an existing mount of the same location.
    fn add(&mut self, ids: Vec<String>, mount: Mount) {
        let key = fs::canonicalize(mount.location()).unwrap_or_else(|_| mount.location().into());
        let existing = self.mounts.iter_mut().find(|m| {
            fs::canonicalize(m.mount.location()).unwrap_or_else(|_| m.mount.location().into())
                == key
        });
        match existing {
            Some(existing) => {
                for id in ids {
                    if !existing.ids.contains(&id) {
                        existing.ids.push(id);
                    }
                }
            }
            None => self.mounts.push(MountedPath { ids, mount }),
        }
    }

    /// Reads a file from the `game` search path, where the engine looks for
    /// materials, models and other content.
    pub fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        self.read_from(path, Some("game"))
    }

    /// Reads a file from mounts tagged with `id`, or from every mount.
    pub fn read_from(&self, path: &str, id: Option<&str>) -> io::Result<Option<Vec<u8>>> {
        self.read_from_limited(path, id, usize::MAX)
    }

    pub fn contains(&self, path: &str) -> io::Result<bool> {
        let path = asset_relative(path)?;
        if path.is_empty() {
            return Ok(false);
        }
        for mounted in self
            .mounts
            .iter()
            .filter(|mount| mount.ids.iter().any(|id| id == "game"))
        {
            let found = match &mounted.mount {
                Mount::Pak(pak) => pak.contains(&path),
                Mount::Directory(root) => match find_case_insensitive(root, &path)? {
                    Some(file) => fs::metadata(file)?.is_file(),
                    None => false,
                },
            };
            if found {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn read_limited(&self, path: &str, maximum: usize) -> io::Result<Option<Vec<u8>>> {
        self.read_from_limited(path, Some("game"), maximum)
    }

    pub fn read_from_limited(
        &self,
        path: &str,
        id: Option<&str>,
        maximum: usize,
    ) -> io::Result<Option<Vec<u8>>> {
        let path = asset_relative(path)?;
        if path.is_empty() {
            return Ok(None);
        }
        for mounted in &self.mounts {
            if id.is_some_and(|id| !mounted.ids.iter().any(|m| m == id)) {
                continue;
            }
            if let Some(data) = mounted.mount.read_limited(&path, maximum)? {
                return Ok(Some(data));
            }
        }
        Ok(None)
    }

    /// Every file on the `game` search path under `dir` (e.g. `materials`)
    /// ending in `extension` (e.g. `.vmt`), normalized, sorted, without
    /// duplicates.
    pub fn list(&self, dir: &str, extension: &str) -> io::Result<Vec<String>> {
        let dir = asset_relative(dir)?;
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let extension = extension.to_ascii_lowercase();
        let mut files = Vec::new();
        for mounted in self
            .mounts
            .iter()
            .filter(|m| m.ids.iter().any(|id| id == "game"))
        {
            match &mounted.mount {
                Mount::Pak(vpk) => files.extend(
                    vpk.paths()
                        .filter(|p| p.starts_with(&prefix) && p.ends_with(&extension))
                        .map(str::to_owned),
                ),
                Mount::Directory(root) => {
                    let Some(start) = find_case_insensitive(root, &dir)? else {
                        continue;
                    };
                    let boundary = root.canonicalize()?;
                    let mut seen = HashSet::new();
                    let mut stack = vec![start];
                    while let Some(dir) = stack.pop() {
                        let Some(canonical) = existing(&dir, dir.canonicalize())? else {
                            continue;
                        };
                        if !canonical.starts_with(&boundary) || !seen.insert(canonical) {
                            continue;
                        }
                        let Some(entries) = existing(&dir, fs::read_dir(&dir))? else {
                            continue;
                        };
                        for entry in entries {
                            let entry = entry?;
                            let path = entry.path();
                            let Some(canonical) = existing(&path, path.canonicalize())? else {
                                continue;
                            };
                            if !canonical.starts_with(&boundary) {
                                continue;
                            }
                            let Some(metadata) = existing(&path, fs::metadata(&path))? else {
                                continue;
                            };
                            if metadata.is_dir() {
                                stack.push(path);
                            } else if metadata.is_file()
                                && let Some(relative) = path
                                    .strip_prefix(root)
                                    .ok()
                                    .and_then(|p| p.to_str())
                                    .map(normalize)
                                    .filter(|p| p.ends_with(&extension))
                            {
                                files.push(relative);
                            }
                        }
                    }
                }
            }
        }
        files.sort_unstable();
        files.dedup();
        Ok(files)
    }
}

fn find_block<'a>(nodes: &'a [Node], path: &[&str]) -> Option<&'a [Node]> {
    let (first, rest) = path.split_first()?;
    let children = nodes
        .iter()
        .find(|n| n.key.eq_ignore_ascii_case(first))?
        .children()?;
    if rest.is_empty() {
        Some(children)
    } else {
        find_block(children, rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn housekeeping_directory_errors_do_not_select_lower_priority_assets() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir_all(second.join("materials")).unwrap();
        fs::write(second.join("materials/test.vmt"), b"wrong mount").unwrap();
        symlink("materials", first.join("materials")).unwrap();
        let game = GameFs {
            game_dir: first.clone(),
            base_dir: root.path().into(),
            mounts: [first.clone(), second]
                .map(|path| MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(path),
                })
                .into(),
        };
        assert!(game.read("materials/test.vmt").is_err());
        assert!(game.contains("materials/test.vmt").is_err());
        assert!(game.list("materials", ".vmt").is_err());
        fs::remove_file(first.join("materials")).unwrap();
        assert_eq!(
            game.read("materials/test.vmt").unwrap().unwrap(),
            b"wrong mount"
        );
        fs::create_dir(first.join("Materials")).unwrap();
        fs::write(first.join("Materials/TEST.vmt"), b"first mount").unwrap();
        assert_eq!(
            game.read("materials/test.vmt").unwrap().unwrap(),
            b"first mount"
        );
        assert!(game.contains("materials/test.vmt").unwrap());
        assert_eq!(
            game.list("materials", ".vmt").unwrap(),
            ["materials/test.vmt"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn wildcard_mount_failures_are_reported_but_absent_optional_paths_are_allowed() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let gameinfo = root.path().join("gameinfo.txt");
        fs::write(
            &gameinfo,
            "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|absent/* game |gameinfo_path|. } } }",
        )
        .unwrap();
        assert_eq!(GameFs::from_gameinfo(&gameinfo).unwrap().mounts.len(), 1);
        symlink("absent", root.path().join("absent")).unwrap();
        let error = GameFs::from_gameinfo(&gameinfo).unwrap_err().to_string();
        assert!(error.contains("absent"), "{error}");
    }

    #[test]
    fn bounded_reads_preserve_mount_lookup_and_reject_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("small.mdl"), b"data").unwrap();
        let gameinfo = root.path().join("gameinfo.txt");
        fs::write(
            &gameinfo,
            "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
        )
        .unwrap();
        let game = GameFs::from_gameinfo(gameinfo).unwrap();
        assert!(game.contains("SMALL.MDL").unwrap());
        assert!(!game.contains("missing.mdl").unwrap());
        assert_eq!(game.read_limited("SMALL.MDL", 4).unwrap().unwrap(), b"data");
        assert_eq!(
            game.read_limited("small.mdl", 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::File::create(root.path().join("large.mdl"))
            .unwrap()
            .set_len(100_000_000)
            .unwrap();
        assert!(
            game.read_limited("large.mdl", 64)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert!(game.read_limited("../outside", 64).is_err());
    }

    #[test]
    fn normalizes_game_paths() {
        assert_eq!(
            normalize("\\Materials\\DE_CBBLE//./CobbleRoad01.vmt"),
            "materials/de_cbble/cobbleroad01.vmt"
        );
    }

    #[test]
    fn absolute_content_and_wildcard_mounts_resolve_from_gameinfo() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        let content = root.path().join("external content");
        let addon = root.path().join("addons/first");
        for folder in [&game, &content, &addon] {
            fs::create_dir_all(folder.join("materials")).unwrap();
        }
        fs::write(content.join("materials/absolute.vmt"), b"absolute content").unwrap();
        fs::write(addon.join("materials/addon.vmt"), b"wildcard content").unwrap();
        fs::write(game.join("materials/local.vmt"), b"local content").unwrap();
        let gameinfo = game.join("gameinfo.txt");
        fs::write(&gameinfo, format!(
            "GameInfo {{ FileSystem {{ SearchPaths {{ game \"{}\" game \"{}/*\" game \"|gameinfo_path|.\" }} }} }}",
            content.display(), root.path().join("addons").display()
        )).unwrap();
        let mounted = GameFs::from_gameinfo(&gameinfo).unwrap();
        for (name, expected) in [
            ("absolute", b"absolute content".as_slice()),
            ("addon", b"wildcard content".as_slice()),
            ("local", b"local content".as_slice()),
        ] {
            assert_eq!(
                mounted
                    .read(&format!("materials/{name}.vmt"))
                    .unwrap()
                    .as_deref(),
                Some(expected)
            );
        }
    }
}
