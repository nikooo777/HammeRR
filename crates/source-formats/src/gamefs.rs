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

    fn identity(&self) -> Result<(PathBuf, Option<PathBuf>), Error> {
        let canonical = |path: &Path| {
            fs::canonicalize(path).map_err(|source| Error::Io {
                path: path.into(),
                source,
            })
        };
        let namespace = match self {
            Self::Directory(_) => None,
            Self::Pak(vpk) => {
                let archive = vpk.archive_path(0);
                Some(
                    canonical(&archive.with_file_name("."))?
                        .join(archive.file_name().expect("Archive path has a filename")),
                )
            }
        };
        Ok((canonical(self.location())?, namespace))
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
    check_file_size(size, maximum)?;
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

fn check_file_size(size: u64, maximum: usize) -> io::Result<()> {
    if size > maximum as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "asset exceeds the caller's byte read limit",
        ));
    }
    Ok(())
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
            if matches!(component, std::path::Component::Prefix(_)) {
                continue;
            }
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
        let key = mount.identity()?;
        let locations = self
            .mounts
            .iter()
            .map(|mounted| mounted.mount.identity())
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

    pub fn asset_origin(&self, path: &str) -> io::Result<Option<AssetOrigin>> {
        self.asset_lookup(path, usize::MAX)
            .map(|(origin, _)| origin)
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
                Mount::Pak(pak) => match pak.asset_source_paths_limited(&path, maximum)? {
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
        self.asset_source_paths_limited(path, usize::MAX)
    }

    pub fn asset_source_paths_limited(
        &self,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<AssetSourcePaths>> {
        let (found, sources) = self.asset_lookup(path, maximum)?;
        Ok(found.map(|_| sources))
    }

    /// Returns lookup dependencies even when the asset is absent.
    pub fn asset_lookup_paths(&self, path: &str) -> io::Result<AssetSourcePaths> {
        self.asset_lookup(path, usize::MAX)
            .map(|(_, sources)| sources)
    }

    fn asset_lookup(
        &self,
        path: &str,
        maximum: usize,
    ) -> io::Result<(Option<AssetOrigin>, AssetSourcePaths)> {
        let path = asset_relative(path)?;
        if path.is_empty() {
            return Ok((None, AssetSourcePaths::default()));
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
                    if let Some(paths) = pak.asset_source_paths_limited(&path, maximum)? {
                        for file in &paths {
                            sources.file(file)?;
                        }
                        Some(AssetOrigin::Vpk {
                            index: pak.path().to_owned(),
                            files: paths,
                        })
                    } else {
                        None
                    }
                }
                Mount::Directory(root) => {
                    match find_case_insensitive_traced(root, &path, Some(&mut sources))? {
                        Some(file) => {
                            let metadata = fs::metadata(&file)?;
                            if metadata.is_file() {
                                check_file_size(metadata.len(), maximum)?;
                                sources.file(&file)?;
                                Some(AssetOrigin::Directory {
                                    mount: root.clone(),
                                    file,
                                })
                            } else {
                                None
                            }
                        }
                        None => None,
                    }
                }
            };
            if found.is_some() {
                sources.finish();
                return Ok((found, sources));
            }
        }
        sources.finish();
        Ok((None, sources))
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
