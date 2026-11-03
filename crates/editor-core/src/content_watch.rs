use std::{
    collections::BTreeSet,
    fmt::{self, Write},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use notify::{Event, EventHandler, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;

const MAX_ROOTS: usize = 1024;
const MAX_DIRECTORIES: usize = 8192;
const MAX_ENTRIES: usize = 262_144;
const MAX_DEPTH: usize = 64;
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATHS: usize = 256;
const MAX_EVENT_PATHS: usize = 64;
const MAX_ERRORS: usize = 8;
const MAX_ERROR_BYTES: usize = 512;

#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub directories: Vec<PathBuf>,
    pub archives: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub discoveries: Vec<PathBuf>,
    pub literal_sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Limits {
    pub roots: usize,
    pub directories: usize,
    pub sentinels: usize,
    pub traversal_entries: usize,
    pub depth: usize,
    pub path_bytes: usize,
    pub paths: usize,
    pub event_paths: usize,
    pub errors: usize,
    pub error_bytes: usize,
    pub policy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub backend: &'static str,
    pub revision: u64,
    pub events: u64,
    pub ignored_events: u64,
    pub error_count: u64,
    pub paths: Vec<String>,
    pub errors: Vec<String>,
    pub complete: bool,
    pub overflow: bool,
    pub roots: usize,
    pub directories: usize,
    pub limits: Limits,
}

#[derive(Debug)]
pub struct NativeWatch {
    watcher: RecommendedWatcher,
    plan: Arc<Plan>,
    state: Arc<Mutex<State>>,
    backend: &'static str,
    roots: usize,
    directories: usize,
}

#[derive(Debug, Default)]
struct State {
    revision: u64,
    events: u64,
    ignored_events: u64,
    error_count: u64,
    paths: BTreeSet<String>,
    errors: Vec<String>,
    incomplete: bool,
    overflow: bool,
    registration_error: Option<String>,
}

impl State {
    fn advance(&mut self) {
        if let Some(revision) = self.revision.checked_add(1) {
            self.revision = revision;
        } else {
            self.issue("Native watch revision overflow; re-enable watching", true);
        }
    }

    fn issue(&mut self, error: &str, overflow: bool) {
        self.incomplete = true;
        self.overflow |= overflow;
        match self.error_count.checked_add(1) {
            Some(count) => self.error_count = count,
            None => self.overflow = true,
        }
        let error = bounded(error, MAX_ERROR_BYTES);
        if self.errors.contains(&error) {
            return;
        }
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(error);
        } else {
            self.overflow = true;
            self.errors[MAX_ERRORS - 1] = "Further native watch errors were omitted".into();
        }
    }

    fn ignore(&mut self) {
        if let Some(count) = self.ignored_events.checked_add(1) {
            self.ignored_events = count;
        } else {
            self.advance();
            self.issue("Native watch ignored-event counter overflow", true);
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Plan {
    mounts: BTreeSet<PathBuf>,
    files: BTreeSet<PathBuf>,
    archives: BTreeSet<PathBuf>,
    archive_parents: BTreeSet<PathBuf>,
    discoveries: BTreeSet<PathBuf>,
    literal_sources: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
    sentinels: BTreeSet<PathBuf>,
}

impl Plan {
    fn build(scope: &Scope) -> Result<Self, String> {
        scope
            .directories
            .len()
            .checked_add(scope.archives.len())
            .and_then(|count| count.checked_add(scope.files.len()))
            .and_then(|count| count.checked_add(scope.discoveries.len()))
            .and_then(|count| count.checked_add(scope.literal_sources.len()))
            .filter(|count| (1..=MAX_ROOTS).contains(count))
            .ok_or_else(|| format!("Native content watching requires 1–{MAX_ROOTS} sources"))?;
        let mut plan = Self::default();
        for path in &scope.directories {
            let path = plan.source(path)?;
            if !io_at(fs::metadata(&path), "inspect mount", &path)?.is_dir() {
                return Err(message(format_args!(
                    "Content watch mount is not a directory: {}",
                    path.display()
                )));
            }
            plan.mounts.insert(path);
        }
        for path in &scope.archives {
            let path = plan.source(path)?;
            if !io_at(fs::metadata(&path), "inspect archive", &path)?.is_file()
                || !extension(&path, "vpk")
            {
                return Err(message(format_args!(
                    "Content watch archive must be a VPK file: {}",
                    path.display()
                )));
            }
            plan.archive_parents.insert(path.parent().unwrap().into());
            plan.archives.insert(path);
        }
        for path in &scope.files {
            let path = plan.source(path)?;
            if !io_at(fs::metadata(&path), "inspect source", &path)?.is_file() {
                return Err(message(format_args!(
                    "Content watch source is not a file: {}",
                    path.display()
                )));
            }
            plan.files.insert(path);
        }
        for path in &scope.discoveries {
            plan.dependency(path, true)?;
        }
        for path in &scope.literal_sources {
            plan.dependency(path, false)?;
        }
        let mut visited = BTreeSet::new();
        let mut entries = 0;
        for root in plan.mounts.clone() {
            let mut pending = vec![(root.clone(), true)];
            plan.directory(root)?;
            while let Some((directory, mount_root)) = pending.pop() {
                if !visited.insert(directory.clone()) {
                    continue;
                }
                for entry in io_at(fs::read_dir(&directory), "read directory", &directory)? {
                    entries += 1;
                    if entries > MAX_ENTRIES {
                        return Err(format!(
                            "Native watch traversal exceeds {MAX_ENTRIES} directory entries"
                        ));
                    }
                    let entry = io_at(entry, "read directory entry", &directory)?;
                    if mount_root && !asset_directory(&entry.file_name()) {
                        continue;
                    }
                    let path = entry.path();
                    check_path(&path)?;
                    let kind = io_at(entry.file_type(), "inspect entry", &path)?;
                    if is_link(&entry, &kind)? {
                        return Err(message(format_args!(
                            "Native watch cannot cover symbolic links inside asset trees: {}",
                            path.display()
                        )));
                    }
                    if kind.is_dir() {
                        plan.directory(path.clone())?;
                        pending.push((path, false));
                    }
                }
            }
        }
        Ok(plan)
    }

    fn directory(&mut self, path: PathBuf) -> Result<(), String> {
        check_path(&path)?;
        if !self.directories.contains(&path) && self.directories.len() == MAX_DIRECTORIES {
            return Err(format!(
                "Native content watching exceeds {MAX_DIRECTORIES} directory registrations"
            ));
        }
        self.directories.insert(path);
        Ok(())
    }

    fn source(&mut self, path: &Path) -> Result<PathBuf, String> {
        check_path(path)?;
        let absolute = io_at(std::path::absolute(path), "resolve source", path)?;
        check_path(&absolute)?;
        let canonical = io_at(
            fs::canonicalize(&absolute),
            "canonicalize source",
            &absolute,
        )?;
        check_path(&canonical)?;
        for source in [&absolute, &canonical] {
            let mut current = source.as_path();
            while let Some(parent) = current.parent() {
                if let Some(name) = current.file_name() {
                    let parent = io_at(fs::canonicalize(parent), "canonicalize ancestor", parent)?;
                    let sentinel = parent.join(name);
                    check_path(&sentinel)?;
                    self.directory(parent)?;
                    if !self.sentinels.contains(&sentinel)
                        && self.sentinels.len() == MAX_DIRECTORIES
                    {
                        return Err(format!(
                            "Native content watching exceeds {MAX_DIRECTORIES} source ancestor paths"
                        ));
                    }
                    self.sentinels.insert(sentinel);
                }
                current = parent;
            }
        }
        Ok(canonical)
    }

    fn dependency(&mut self, path: &Path, namespace: bool) -> Result<(), String> {
        check_path(path)?;
        let absolute = io_at(std::path::absolute(path), "resolve discovery root", path)?;
        check_path(&absolute)?;
        let mut existing = absolute.as_path();
        let mut missing = Vec::new();
        loop {
            match fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(
                        existing
                            .file_name()
                            .ok_or("Invalid content discovery root")?,
                    );
                    existing = existing.parent().ok_or("Invalid content discovery root")?;
                }
                Err(error) => return io_at(Err(error), "inspect discovery root", existing),
            }
        }
        let mut resolved = self.source(existing)?;
        let metadata = io_at(
            fs::metadata(&resolved),
            "inspect discovery ancestor",
            &resolved,
        )?;
        if metadata.is_file() && missing.is_empty() {
            if namespace {
                return Err(message(format_args!(
                    "Native content discovery root must be a directory: {}",
                    resolved.display()
                )));
            }
            self.literal_sources.insert(resolved);
            return Ok(());
        }
        if !metadata.is_dir() {
            return Err(message(format_args!(
                "Native content discovery ancestor is not a directory: {}",
                resolved.display()
            )));
        }
        self.directory(resolved.clone())?;
        for name in missing.into_iter().rev() {
            resolved.push(name);
        }
        check_path(&resolved)?;
        if namespace {
            self.discoveries.insert(resolved);
        } else {
            self.literal_sources.insert(resolved);
        }
        Ok(())
    }

    fn relevant(&self, path: &Path) -> bool {
        if self.sentinels.contains(path) || self.files.contains(path) {
            return true;
        }
        if self
            .literal_sources
            .iter()
            .any(|root| root.starts_with(path))
        {
            return true;
        }
        if self.discoveries.iter().any(|root| {
            root.starts_with(path) || path.parent().is_some_and(|parent| parent == root)
        }) {
            return true;
        }
        if extension(path, "vpk")
            && path
                .parent()
                .is_some_and(|parent| self.archive_parents.contains(parent))
        {
            return true;
        }
        self.mounts.iter().any(|root| {
            let Ok(relative) = path.strip_prefix(root) else {
                return false;
            };
            let mut components = relative.components();
            let Some(first) = components.next() else {
                return true;
            };
            asset_directory(first.as_os_str())
                || (components.next().is_none()
                    && (extension(path, "fgd")
                        || extension(path, "vpk")
                        || os_eq(first.as_os_str(), "gameinfo.txt")))
        })
    }
}

struct Handler {
    plan: Arc<Plan>,
    state: Arc<Mutex<State>>,
}

impl EventHandler for Handler {
    fn handle_event(&mut self, event: notify::Result<Event>) {
        let mut state = lock(&self.state);
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                state.advance();
                state.issue(
                    &message(format_args!("Native watch failed: {error}")),
                    false,
                );
                return;
            }
        };
        if event.need_rescan() {
            state.advance();
            state.issue(
                "Native watch lost events; Reload content or re-enable watching",
                true,
            );
            return;
        }
        if matches!(event.kind, EventKind::Access(_))
            || matches!(
                event.kind,
                EventKind::Modify(notify::event::ModifyKind::Metadata(
                    notify::event::MetadataKind::AccessTime
                ))
            )
        {
            state.ignore();
            return;
        }
        if event.paths.is_empty() || event.paths.len() > MAX_EVENT_PATHS {
            state.advance();
            state.issue("Native watch event has missing or excessive paths", true);
            return;
        }
        if event
            .paths
            .iter()
            .any(|path| !path.is_absolute() || check_path(path).is_err())
        {
            state.advance();
            state.issue(
                "Native watch event has an unsupported or oversized path",
                true,
            );
            return;
        }
        let paths = event
            .paths
            .iter()
            .filter(|path| self.plan.relevant(path))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            state.ignore();
            return;
        }
        state.advance();
        if let Some(count) = state.events.checked_add(1) {
            state.events = count;
        } else {
            state.issue("Native watch event counter overflow", true);
        }
        for path in paths {
            let path = path.to_string_lossy();
            if path.len() > MAX_PATH_BYTES {
                state.issue("Native watch displayed path exceeds its byte limit", true);
            } else if !state.paths.contains(path.as_ref()) && state.paths.len() == MAX_PATHS {
                state.issue(
                    "Native watch changed-path capacity exceeded; Reload content",
                    true,
                );
            } else {
                state.paths.insert(path.into_owned());
            }
        }
        if matches!(
            event.kind,
            EventKind::Any
                | EventKind::Other
                | EventKind::Create(
                    notify::event::CreateKind::Any
                        | notify::event::CreateKind::Folder
                        | notify::event::CreateKind::Other
                )
                | EventKind::Remove(
                    notify::event::RemoveKind::Any
                        | notify::event::RemoveKind::Folder
                        | notify::event::RemoveKind::Other
                )
                | EventKind::Modify(notify::event::ModifyKind::Name(_))
        ) {
            state.issue(
                "Native directory or unclassified namespace change requires watch re-registration; Reload content or re-enable watching",
                false,
            );
        }
    }
}

impl NativeWatch {
    pub fn start(scope: Scope) -> Result<Self, String> {
        if !cfg!(any(target_os = "linux", windows)) {
            return Err("Native content watching supports Linux and Windows only".into());
        }
        let backend = match RecommendedWatcher::kind() {
            notify::WatcherKind::Inotify => "inotify",
            notify::WatcherKind::ReadDirectoryChangesWatcher => "ReadDirectoryChangesW",
            _ => return Err("Native content watching is unavailable on this platform".into()),
        };
        let plan = Arc::new(Plan::build(&scope)?);
        let state = Arc::new(Mutex::new(State::default()));
        let mut watcher = RecommendedWatcher::new(
            Handler {
                plan: plan.clone(),
                state: state.clone(),
            },
            notify::Config::default().with_follow_symlinks(false),
        )
        .map_err(|error| message(format_args!("Native watch initialization failed: {error}")))?;
        for path in &plan.directories {
            watcher
                .watch(path, RecursiveMode::NonRecursive)
                .map_err(|error| {
                    message(format_args!(
                        "Native watch registration failed at {}: {error}",
                        path.display()
                    ))
                })?;
        }
        if *plan != Plan::build(&scope)? {
            return Err(
                "Content directory coverage changed during native watch setup; retry".into(),
            );
        }
        verify_registration(&watcher, &plan)?;
        {
            let state = lock(&state);
            if state.incomplete {
                return Err(message(format_args!(
                    "Native watch lost coverage during setup: {}",
                    state.errors.first().map(String::as_str).unwrap_or("retry")
                )));
            }
        }
        Ok(Self {
            watcher,
            plan: plan.clone(),
            state,
            backend,
            roots: plan.mounts.len()
                + plan.archives.len()
                + plan.files.len()
                + plan.discoveries.len()
                + plan.literal_sources.len(),
            directories: plan.directories.len(),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        let registration = verify_registration(&self.watcher, &self.plan);
        let mut state = lock(&self.state);
        if let Err(error) = registration
            && state.registration_error.as_ref() != Some(&error)
        {
            state.registration_error = Some(error.clone());
            state.advance();
            state.issue(&error, false);
        }
        Snapshot {
            backend: self.backend,
            revision: state.revision,
            events: state.events,
            ignored_events: state.ignored_events,
            error_count: state.error_count,
            paths: state.paths.iter().cloned().collect(),
            errors: state.errors.clone(),
            complete: !state.incomplete,
            overflow: state.overflow,
            roots: self.roots,
            directories: self.directories,
            limits: Limits {
                roots: MAX_ROOTS,
                directories: MAX_DIRECTORIES,
                sentinels: MAX_DIRECTORIES,
                traversal_entries: MAX_ENTRIES,
                depth: MAX_DEPTH,
                path_bytes: MAX_PATH_BYTES,
                paths: MAX_PATHS,
                event_paths: MAX_EVENT_PATHS,
                errors: MAX_ERRORS,
                error_bytes: MAX_ERROR_BYTES,
                policy: "Linux and Windows native notify events only; no polling fallback. Mounted materials/models/sound/scripts/particles trees, root FGD/gameinfo/VPK files, archive parents, immediate wildcard-discovery children and exact effective/literal source paths are covered. Missing dependencies retain creation sentinels; literal directories do not broaden namespace matching. Source/alias ancestor parents detect replacement. Descendant asset symlinks/reparse points refuse registration. Two bounded topology passes surround fixed nonrecursive registrations; traversal or registration errors retain no partial watcher. Reports compare the native registration registry against the admitted directory set without reading filesystem state. Directory/unclassified namespace changes, uncertain renames or lost registrations mark coverage incomplete until Reload content or re-enable watching; known file creation/deletion retains parent coverage. Event counts describe callbacks, not distinct filesystem operations; snapshots retain changes and bounded paths/errors since registration. Events never reload content or mutate map state.",
            },
        }
    }
}

fn verify_registration(watcher: &RecommendedWatcher, plan: &Plan) -> Result<(), String> {
    let paths = watcher.watched_paths().map_err(|error| {
        message(format_args!(
            "Native watch registration status failed: {error}"
        ))
    })?;
    if paths.len() > MAX_DIRECTORIES {
        return Err("Native watch registration count exceeds its admitted limit".into());
    }
    let mut actual = BTreeSet::new();
    for (path, mode) in paths {
        if mode != RecursiveMode::NonRecursive || !plan.directories.contains(&path) {
            return Err("Native watch registrations differ from the admitted directory set; Reload content or re-enable watching".into());
        }
        actual.insert(path);
    }
    if actual != plan.directories {
        return Err(
            "Native watch lost directory registrations; Reload content or re-enable watching"
                .into(),
        );
    }
    Ok(())
}

fn is_link(entry: &fs::DirEntry, kind: &fs::FileType) -> Result<bool, String> {
    if kind.is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let metadata = io_at(entry.metadata(), "inspect reparse point", &entry.path())?;
        Ok(metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        Ok(false)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    match state.lock() {
        Ok(state) => state,
        Err(error) => {
            let mut state = error.into_inner();
            state.advance();
            state.issue("Native watch state was poisoned; re-enable watching", false);
            state
        }
    }
}

fn check_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.as_os_str().len() > MAX_PATH_BYTES
        || path.components().count() > MAX_DEPTH
    {
        return Err(format!(
            "Native watch paths require 1–{MAX_PATH_BYTES} bytes and at most {MAX_DEPTH} components"
        ));
    }
    Ok(())
}

fn extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .is_some_and(|extension| os_eq(extension, expected))
}

fn asset_directory(name: &std::ffi::OsStr) -> bool {
    ["materials", "models", "sound", "scripts", "particles"]
        .iter()
        .any(|expected| os_eq(name, expected))
}

fn os_eq(value: &std::ffi::OsStr, expected: &str) -> bool {
    value
        .as_encoded_bytes()
        .eq_ignore_ascii_case(expected.as_bytes())
}

fn io_at<T>(result: std::io::Result<T>, operation: &str, path: &Path) -> Result<T, String> {
    result.map_err(|error| {
        message(format_args!(
            "Native watch could not {operation} at {}: {error}",
            path.display()
        ))
    })
}

fn bounded(value: &str, bytes: usize) -> String {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

fn message(value: impl fmt::Display) -> String {
    struct Message(String);
    impl Write for Message {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            self.0
                .push_str(&bounded(value, MAX_ERROR_BYTES - self.0.len()));
            Ok(())
        }
    }
    let mut result = Message(String::new());
    let _ = write!(result, "{value}");
    result.0
}
