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
                    read_file_limited(&file, maximum).map(Some)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetOrigin {
    Directory { mount: PathBuf, file: PathBuf },
    Vpk { index: PathBuf, files: Vec<PathBuf> },
}

#[derive(Debug)]
pub struct MountedAsset {
    pub data: Vec<u8>,
    pub origin: AssetOrigin,
}

fn read_file_limited(path: &Path, maximum: usize) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
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
    Ok(data)
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
    find_case_insensitive_traced(root, relative, None)
}

fn find_case_insensitive_traced(
    root: &Path,
    relative: &str,
    mut trace: Option<&mut AssetSourcePaths>,
) -> io::Result<Option<PathBuf>> {
    if let Some(trace) = trace.as_deref_mut() {
        trace.directory(root)?;
    }
    let Some(boundary) = existing(root, root.canonicalize())? else {
        return Ok(None);
    };
    let exact = root.join(relative);
    if let Some(trace) = trace.as_deref_mut() {
        trace.symlinks(&exact, 0)?;
    }
    if let Some(canonical) = existing(&exact, exact.canonicalize())? {
        if let Some(trace) = trace.as_deref_mut() {
            let mut parent = root.to_path_buf();
            for part in relative.split('/').filter(|part| !part.is_empty()) {
                trace.directory(&parent)?;
                parent.push(part);
            }
        }
        return Ok(canonical.starts_with(&boundary).then_some(exact));
    }
    let mut current = root.to_path_buf();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        if let Some(trace) = trace.as_deref_mut() {
            trace.directory(&current)?;
        }
        let exact = current.join(part);
        if let Some(trace) = trace.as_deref_mut() {
            trace.symlinks(&exact, 0)?;
        }
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
        if let Some(trace) = trace.as_deref_mut() {
            trace.symlinks(&current, 0)?;
        }
        let Some(canonical) = existing(&current, current.canonicalize())? else {
            return Ok(None);
        };
        if !canonical.starts_with(&boundary) {
            return Ok(None);
        }
    }
    Ok(Some(current))
}

/// Physical sources and lookup dependencies for one asset in the mounted search paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetSourcePaths {
    /// Backing files, lookup targets and aliases, including higher-priority VPK indexes.
    pub files: Vec<PathBuf>,
    pub lookup_directories: Vec<PathBuf>,
    pub lookup_symlinks: Vec<PathBuf>,
}

impl AssetSourcePaths {
    fn file(&mut self, path: &Path) -> io::Result<()> {
        self.symlinks(path, 0)?;
        if let Some(parent) = path.parent() {
            self.directory(parent)?;
        }
        self.files.push(std::path::absolute(path)?);
        self.files.push(path.canonicalize()?);
        Ok(())
    }

    fn directory(&mut self, path: &Path) -> io::Result<()> {
        self.symlinks(path, 0)?;
        if existing(path, fs::metadata(path))?.is_some_and(|metadata| metadata.is_dir()) {
            self.lookup_directories.push(std::path::absolute(path)?);
            self.lookup_directories.push(path.canonicalize()?);
        }
        Ok(())
    }

    fn symlinks(&mut self, path: &Path, depth: usize) -> io::Result<()> {
        if depth > 40 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Asset provenance exceeds the symlink depth limit",
            ));
        }
        let mut current = PathBuf::new();
        for component in std::path::absolute(path)?.components() {
            current.push(component);
            let Some(metadata) = existing(&current, fs::symlink_metadata(&current))? else {
                if let Some(parent) = current.parent() {
                    self.lookup_directories.push(parent.to_path_buf());
                    self.lookup_directories.push(parent.canonicalize()?);
                }
                return Ok(());
            };
            if metadata.file_type().is_symlink() {
                if !self.lookup_symlinks.contains(&current) {
                    self.lookup_symlinks.push(current.clone());
                    let target = fs::read_link(&current)?;
                    let target = if target.is_absolute() {
                        target
                    } else {
                        current.parent().unwrap_or(Path::new("/")).join(target)
                    };
                    self.symlinks(&target, depth + 1)?;
                }
                let Some(canonical) = existing(&current, current.canonicalize())? else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Asset provenance encountered a dangling symlink",
                    ));
                };
                let metadata = fs::metadata(&canonical)?;
                if metadata.is_file() {
                    self.files.push(canonical);
                } else if metadata.is_dir() {
                    self.lookup_directories.push(canonical);
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Asset provenance requires regular file or directory symlink targets",
                    ));
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self) {
        for paths in [
            &mut self.files,
            &mut self.lookup_directories,
            &mut self.lookup_symlinks,
        ] {
            paths.sort();
            paths.dedup();
        }
    }
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
        let gameinfo = std::path::absolute(gameinfo).map_err(io_err(gameinfo))?;
        let gameinfo = gameinfo.as_path();
        let text = fs::read_to_string(gameinfo).map_err(io_err(gameinfo))?;
        let nodes = keyvalues::parse_lenient(&text).map_err(|source| Error::Syntax {
            path: gameinfo.to_path_buf(),
            source,
        })?;
        let search_paths = find_block(&nodes, &["GameInfo", "FileSystem", "SearchPaths"])
            .ok_or_else(|| Error::NoSearchPaths(gameinfo.to_path_buf()))?;

        let game_dir = gameinfo
            .parent()
            .expect("gameinfo names a file")
            .to_path_buf();
        let base_dir = game_dir.parent().unwrap_or(&game_dir).to_path_buf();
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
                    self.add(ids.clone(), Mount::Directory(child))?;
                } else if name.ends_with("_dir.vpk") {
                    let vpk = Vpk::open(&child).map_err(|e| Error::Io {
                        path: child,
                        source: e,
                    })?;
                    self.add(ids.clone(), Mount::Pak(vpk))?;
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
                self.add(ids, Mount::Pak(vpk))?;
            }
        } else if existing(&path, fs::metadata(&path))
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?
            .is_some_and(|metadata| metadata.is_dir())
        {
            self.add(ids, Mount::Directory(path))?;
        }
        Ok(())
    }

    fn add(&mut self, ids: Vec<String>, mount: Mount) -> Result<(), Error> {
        let canonical = |path: &Path| {
            fs::canonicalize(path).map_err(|source| Error::Io {
                path: path.into(),
                source,
            })
        };
        let key = canonical(mount.location())?;
        let locations = self
            .mounts
            .iter()
            .map(|mounted| canonical(mounted.mount.location()))
            .collect::<Result<Vec<_>, _>>()?;
        let existing = locations.iter().position(|location| *location == key);
        let mut deferred = Vec::new();
        for id in ids {
            if deferred.contains(&id)
                || locations
                    .iter()
                    .zip(&self.mounts)
                    .any(|(location, mounted)| *location == key && mounted.ids.contains(&id))
            {
                continue;
            }
            if let Some(index) = existing
                && !self.mounts[index + 1..]
                    .iter()
                    .any(|mounted| mounted.ids.contains(&id))
            {
                self.mounts[index].ids.push(id);
            } else {
                deferred.push(id);
            }
        }
        if !deferred.is_empty() {
            self.mounts.push(MountedPath {
                ids: deferred,
                mount,
            });
        }
        Ok(())
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
                Mount::Pak(pak) => {
                    pak.verify_index()?;
                    let found = pak.contains(&path);
                    pak.verify_index()?;
                    found
                }
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

    pub fn read_asset_limited(
        &self,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<MountedAsset>> {
        let path = asset_relative(path)?;
        if path.is_empty() {
            return Ok(None);
        }
        for mounted in self
            .mounts
            .iter()
            .filter(|mount| mount.ids.iter().any(|id| id == "game"))
        {
            let asset = match &mounted.mount {
                Mount::Directory(root) => match find_case_insensitive_traced(
                    root,
                    &path,
                    Some(&mut AssetSourcePaths::default()),
                )? {
                    Some(file) if fs::metadata(&file)?.is_file() => Some(MountedAsset {
                        data: read_file_limited(&file, maximum)?,
                        origin: AssetOrigin::Directory {
                            mount: root.clone(),
                            file,
                        },
                    }),
                    _ => None,
                },
                Mount::Pak(pak) => match pak.asset_source_paths(&path)? {
                    Some(files) => pak.read_limited(&path, maximum)?.map(|data| MountedAsset {
                        data,
                        origin: AssetOrigin::Vpk {
                            index: pak.path().to_owned(),
                            files,
                        },
                    }),
                    None => None,
                },
            };
            if asset.is_some() {
                return Ok(asset);
            }
        }
        Ok(None)
    }

    /// Resolve the same `game` asset as [`Self::read`], including higher-priority lookup dependencies.
    /// Changed VPK indexes and dangling lookup symlinks require a content reload.
    pub fn asset_source_paths(&self, path: &str) -> io::Result<Option<AssetSourcePaths>> {
        let (found, sources) = self.asset_lookup(path)?;
        Ok(found.then_some(sources))
    }

    /// Returns lookup dependencies even when the asset is absent.
    pub fn asset_lookup_paths(&self, path: &str) -> io::Result<AssetSourcePaths> {
        self.asset_lookup(path).map(|(_, sources)| sources)
    }

    fn asset_lookup(&self, path: &str) -> io::Result<(bool, AssetSourcePaths)> {
        let path = asset_relative(path)?;
        if path.is_empty() {
            return Ok((false, AssetSourcePaths::default()));
        }
        let mut sources = AssetSourcePaths::default();
        for mounted in self
            .mounts
            .iter()
            .filter(|mount| mount.ids.iter().any(|id| id == "game"))
        {
            let found = match &mounted.mount {
                Mount::Pak(pak) => {
                    sources.file(pak.path())?;
                    if let Some(paths) = pak.asset_source_paths(&path)? {
                        for file in paths {
                            sources.file(&file)?;
                        }
                        true
                    } else {
                        false
                    }
                }
                Mount::Directory(root) => {
                    if let Some(file) =
                        find_case_insensitive_traced(root, &path, Some(&mut sources))?
                        && fs::metadata(&file)?.is_file()
                    {
                        sources.file(&file)?;
                        true
                    } else {
                        false
                    }
                }
            };
            if found {
                sources.finish();
                return Ok((true, sources));
            }
        }
        sources.finish();
        Ok((false, sources))
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
                Mount::Pak(vpk) => {
                    vpk.verify_index()?;
                    files.extend(
                        vpk.paths()
                            .filter(|p| p.starts_with(&prefix) && p.ends_with(&extension))
                            .map(str::to_owned),
                    );
                    vpk.verify_index()?;
                }
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

    #[test]
    fn repeated_physical_mounts_preserve_each_search_id_precedence() {
        let root = tempfile::tempdir().unwrap();
        for name in ["first", "second"] {
            fs::create_dir_all(root.path().join(name).join("materials")).unwrap();
            fs::write(root.path().join(name).join("materials/test.vmt"), name).unwrap();
        }
        let gameinfo = root.path().join("gameinfo.txt");
        for (declarations, expected) in [
            (
                "mod |gameinfo_path|first game |gameinfo_path|second game |gameinfo_path|first",
                b"second".as_slice(),
            ),
            (
                "game |gameinfo_path|first game |gameinfo_path|second mod |gameinfo_path|first",
                b"first".as_slice(),
            ),
            (
                "game |gameinfo_path|second game |gameinfo_path|first game |gameinfo_path|second",
                b"second".as_slice(),
            ),
            (
                "mod |gameinfo_path|first game |gameinfo_path|second game |gameinfo_path|first mod+game |gameinfo_path|first",
                b"second".as_slice(),
            ),
        ] {
            fs::write(
                &gameinfo,
                format!("GameInfo {{ FileSystem {{ SearchPaths {{ {declarations} }} }} }}"),
            )
            .unwrap();
            let game = GameFs::from_gameinfo(&gameinfo).unwrap();
            assert_eq!(
                game.read("materials/test.vmt").unwrap().unwrap(),
                expected,
                "{declarations}"
            );
            let source = game
                .asset_source_paths("materials/test.vmt")
                .unwrap()
                .unwrap();
            assert!(
                source
                    .files
                    .iter()
                    .any(|path| path.ends_with(if expected == b"first" {
                        "first/materials/test.vmt"
                    } else {
                        "second/materials/test.vmt"
                    }))
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn canonical_alias_deduplication_preserves_order_and_reports_lost_mounts() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        for name in ["first", "second"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::write(root.path().join(name).join("asset.txt"), name).unwrap();
        }
        symlink("first", root.path().join("alias")).unwrap();
        let gameinfo = root.path().join("gameinfo.txt");
        fs::write(&gameinfo, "GameInfo { FileSystem { SearchPaths { mod |gameinfo_path|first game |gameinfo_path|second game |gameinfo_path|alias game |gameinfo_path|first } } }").unwrap();
        let game = GameFs::from_gameinfo(&gameinfo).unwrap();
        assert_eq!(game.read("asset.txt").unwrap().unwrap(), b"second");
        assert_eq!(
            game.read_from("asset.txt", Some("mod")).unwrap().unwrap(),
            b"first"
        );
        assert_eq!(game.mounts.len(), 3);
        let mut game = provenance_game(
            root.path(),
            vec![Mount::Directory(root.path().join("alias"))],
        );
        fs::remove_file(root.path().join("alias")).unwrap();
        assert!(
            game.add(
                vec!["game".into()],
                Mount::Directory(root.path().join("second"))
            )
            .unwrap_err()
            .to_string()
            .contains("alias")
        );
        assert_eq!(game.mounts.len(), 1);
    }

    fn provenance_game(root: &Path, mounts: Vec<Mount>) -> GameFs {
        GameFs {
            game_dir: root.into(),
            base_dir: root.into(),
            mounts: mounts
                .into_iter()
                .map(|mount| MountedPath {
                    ids: vec!["game".into()],
                    mount,
                })
                .collect(),
        }
    }

    #[test]
    fn provenance_watches_case_lookup_parents_before_shadow_files_appear() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir_all(first.join("Materials")).unwrap();
        fs::create_dir_all(second.join("Materials/Nested")).unwrap();
        let asset = second.join("Materials/Nested/ASSET.vmt");
        fs::write(&asset, b"second").unwrap();
        let game = provenance_game(
            root.path(),
            vec![
                Mount::Directory(first.clone()),
                Mount::Directory(second.clone()),
            ],
        );
        let before = game
            .asset_source_paths("MATERIALS/nested/asset.VMT")
            .unwrap()
            .unwrap();
        assert_eq!(
            game.read("materials/nested/asset.vmt").unwrap().unwrap(),
            b"second"
        );
        assert!(before.files.contains(&asset));
        assert!(before.files.contains(&asset.canonicalize().unwrap()));
        for directory in [
            &first,
            &first.join("Materials"),
            &second,
            &second.join("Materials"),
            &second.join("Materials/Nested"),
        ] {
            assert!(
                before.lookup_directories.contains(directory),
                "{}",
                directory.display()
            );
        }
        assert!(before.lookup_symlinks.is_empty());
        fs::create_dir(first.join("Materials/Nested")).unwrap();
        let shadow = first.join("Materials/Nested/asset.vmt");
        fs::write(&shadow, b"first").unwrap();
        let after = game
            .asset_source_paths("materials/nested/asset.vmt")
            .unwrap()
            .unwrap();
        assert!(after.files.contains(&shadow));
        assert!(after.files.contains(&shadow.canonicalize().unwrap()));
        assert_eq!(
            game.read("materials/nested/asset.vmt").unwrap().unwrap(),
            b"first"
        );
        assert_ne!(before, after);
        assert!(game.asset_source_paths("../escape").is_err());
    }

    #[test]
    fn provenance_includes_higher_priority_vpk_indexes_with_no_matching_entry() {
        let root = tempfile::tempdir().unwrap();
        let index = root.path().join("empty_dir.vpk");
        let mut bytes = 0x55aa_1234u32.to_le_bytes().to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.push(0);
        fs::write(&index, bytes).unwrap();
        let directory = root.path().join("directory");
        fs::create_dir(&directory).unwrap();
        let asset = directory.join("asset.txt");
        fs::write(&asset, b"loose").unwrap();
        let game = provenance_game(
            root.path(),
            vec![
                Mount::Pak(Vpk::open(&index).unwrap()),
                Mount::Directory(directory),
            ],
        );
        let sources = game.asset_source_paths("asset.txt").unwrap().unwrap();
        assert!(sources.files.contains(&index));
        assert!(sources.files.contains(&asset));
        fs::write(&index, b"replaced index").unwrap();
        assert!(
            game.asset_source_paths("asset.txt")
                .unwrap_err()
                .to_string()
                .contains("reload mounted content")
        );
    }

    #[cfg(unix)]
    #[test]
    fn provenance_tracks_case_paths_mount_links_and_internal_link_retargets() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("actual");
        for directory in ["one", "two"] {
            fs::create_dir_all(real.join(directory)).unwrap();
            fs::write(real.join(directory).join("payload.bin"), directory).unwrap();
            symlink("payload.bin", real.join(directory).join("ASSET.vmt")).unwrap();
        }
        let alias = root.path().join("linked");
        symlink("actual", &alias).unwrap();
        symlink("one", real.join("Materials")).unwrap();
        let game = provenance_game(root.path(), vec![Mount::Directory(alias.clone())]);
        let before = game
            .asset_source_paths("materials/asset.vmt")
            .unwrap()
            .unwrap();
        assert!(before.files.contains(&alias.join("Materials/ASSET.vmt")));
        assert!(before.files.contains(&real.join("one/payload.bin")));
        assert!(
            before
                .lookup_symlinks
                .contains(&alias.join("Materials/ASSET.vmt"))
        );
        assert!(before.lookup_symlinks.contains(&alias));
        assert!(before.lookup_symlinks.contains(&alias.join("Materials")));
        assert!(before.lookup_directories.contains(&real.join("one")));
        assert_eq!(game.read("materials/asset.vmt").unwrap().unwrap(), b"one");
        fs::remove_file(real.join("Materials")).unwrap();
        symlink("two", real.join("Materials")).unwrap();
        let after = game
            .asset_source_paths("materials/asset.vmt")
            .unwrap()
            .unwrap();
        assert!(after.lookup_directories.contains(&real.join("two")));
        assert_ne!(before, after);
        assert_eq!(game.read("materials/asset.vmt").unwrap().unwrap(), b"two");
        fs::remove_file(real.join("Materials")).unwrap();
        symlink("missing", real.join("Materials")).unwrap();
        assert!(
            game.asset_source_paths("materials/asset.vmt")
                .unwrap_err()
                .to_string()
                .contains("dangling symlink")
        );
    }

    #[cfg(unix)]
    #[test]
    fn provenance_watches_non_file_symlink_targets_that_can_become_shadows() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        let target = first.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(second.join("asset.txt"), b"fallback").unwrap();
        symlink("target", first.join("asset.txt")).unwrap();
        let game = provenance_game(
            root.path(),
            vec![Mount::Directory(first), Mount::Directory(second)],
        );
        let before = game.asset_source_paths("asset.txt").unwrap().unwrap();
        assert!(before.lookup_directories.contains(&target));
        assert_eq!(game.read("asset.txt").unwrap().unwrap(), b"fallback");
        fs::remove_dir(&target).unwrap();
        fs::write(&target, b"shadow").unwrap();
        assert_eq!(game.read("asset.txt").unwrap().unwrap(), b"shadow");
        assert_ne!(
            before,
            game.asset_source_paths("asset.txt").unwrap().unwrap()
        );
    }

    #[test]
    fn filename_gameinfo_resolves_content_from_its_directory() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new_in(".").unwrap();
        let text = "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }";
        file.write_all(text.as_bytes()).unwrap();
        let filename = Path::new(file.path().file_name().unwrap());
        for path in [filename.to_path_buf(), Path::new(".").join(filename)] {
            let game = GameFs::from_gameinfo(path).unwrap();
            assert_eq!(game.game_dir, std::env::current_dir().unwrap());
            assert_eq!(game.mounts.len(), 1);
            assert_eq!(
                game.read(filename.to_str().unwrap()).unwrap().as_deref(),
                Some(text.as_bytes())
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_gameinfo_uses_the_provided_content_directory() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let game = root.path().join("game");
        fs::create_dir(&config).unwrap();
        fs::create_dir(&game).unwrap();
        fs::write(
            config.join("shared.txt"),
            "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
        )
        .unwrap();
        fs::write(config.join("asset.txt"), b"wrong directory").unwrap();
        fs::write(game.join("asset.txt"), b"mounted content").unwrap();
        let gameinfo = game.join("gameinfo.txt");
        std::os::unix::fs::symlink(config.join("shared.txt"), &gameinfo).unwrap();
        let mounted = GameFs::from_gameinfo(gameinfo).unwrap();
        assert_eq!(mounted.game_dir, game);
        assert_eq!(
            mounted.read("asset.txt").unwrap().unwrap(),
            b"mounted content"
        );
    }

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
    fn source_reads_report_the_exact_directory_winner_and_preserve_byte_limits() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir_all(first.join("Materials")).unwrap();
        fs::create_dir_all(second.join("materials")).unwrap();
        let asset = first.join("Materials/TEST.vmt");
        fs::write(&asset, b"first").unwrap();
        fs::write(second.join("materials/test.vmt"), b"low").unwrap();
        let game = provenance_game(
            root.path(),
            vec![Mount::Directory(first.clone()), Mount::Directory(second)],
        );
        let source = game
            .read_asset_limited("MATERIALS/TEST.VMT", 5)
            .unwrap()
            .unwrap();
        assert_eq!(source.data, b"first");
        assert_eq!(
            source.origin,
            AssetOrigin::Directory {
                mount: first,
                file: asset
            }
        );
        assert!(game.read_asset_limited("materials/test.vmt", 4).is_err());
        assert!(
            game.read_asset_limited("materials/missing.vmt", 5)
                .unwrap()
                .is_none()
        );
        assert!(game.read_asset_limited("../outside", 5).is_err());
    }

    #[test]
    fn source_reads_report_vpk_storage_and_reject_changed_indexes() {
        let root = tempfile::tempdir().unwrap();
        let index = root.path().join("source_dir.vpk");
        let payload = b"UnlitGeneric {}";
        let mut tree = b"vmt\0materials\0test\0".to_vec();
        tree.extend(0u32.to_le_bytes());
        tree.extend(0u16.to_le_bytes());
        tree.extend(0x7fffu16.to_le_bytes());
        tree.extend(0u32.to_le_bytes());
        tree.extend((payload.len() as u32).to_le_bytes());
        tree.extend(0xffffu16.to_le_bytes());
        tree.extend(b"\0\0\0");
        let mut bytes = 0x55aa_1234u32.to_le_bytes().to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend((tree.len() as u32).to_le_bytes());
        bytes.extend(tree);
        bytes.extend(payload);
        fs::write(&index, bytes).unwrap();
        let fallback = root.path().join("fallback");
        fs::create_dir_all(fallback.join("materials")).unwrap();
        fs::write(fallback.join("materials/later.vmt"), b"fallback").unwrap();
        let game = provenance_game(
            root.path(),
            vec![
                Mount::Pak(Vpk::open(&index).unwrap()),
                Mount::Directory(fallback),
            ],
        );
        assert_eq!(
            game.read_limited("materials/later.vmt", 8)
                .unwrap()
                .unwrap(),
            b"fallback"
        );
        assert!(game.contains("materials/test.vmt").unwrap());
        assert_eq!(
            game.list("materials", ".vmt").unwrap(),
            ["materials/later.vmt", "materials/test.vmt"]
        );
        let source = game
            .read_asset_limited("materials/test.vmt", payload.len())
            .unwrap()
            .unwrap();
        assert_eq!(source.data, payload);
        let AssetOrigin::Vpk {
            index: mounted,
            files,
        } = source.origin
        else {
            panic!("expected VPK source")
        };
        assert_eq!(mounted, index);
        assert!(files.contains(&index));
        assert!(
            game.read_asset_limited("materials/test.vmt", payload.len() - 1)
                .is_err()
        );
        fs::write(&index, b"changed index").unwrap();
        for path in [
            "materials/test.vmt",
            "materials/later.vmt",
            "materials/missing.vmt",
        ] {
            assert!(
                game.read_limited(path, 64)
                    .unwrap_err()
                    .to_string()
                    .contains("reload mounted content")
            );
            assert!(
                game.contains(path)
                    .unwrap_err()
                    .to_string()
                    .contains("reload mounted content")
            );
        }
        assert!(
            game.list("materials", ".vmt")
                .unwrap_err()
                .to_string()
                .contains("reload mounted content")
        );
        assert!(
            game.read_asset_limited("materials/test.vmt", payload.len())
                .unwrap_err()
                .to_string()
                .contains("reload mounted content")
        );
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
