use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use source_formats::bsp::{Bsp, PakLimits};
use source_formats::gamefs::GameFs;
use tempfile::NamedTempFile;

use crate::runtime::native_activity::{Permit, Phase, Target};

const MAX_LOG_BYTES: u64 = 256 * 1024;
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_LOG_LINES: usize = 200;
pub const MAX_LOG_LINES: usize = 2000;

#[derive(Clone)]
pub struct Install {
    root: PathBuf,
    game: PathBuf,
    executable: PathBuf,
    binding: Target,
}

impl Install {
    pub fn from_game(fs: &GameFs) -> Result<Self, String> {
        let game = fs.game_dir.canonicalize().map_err(|e| e.to_string())?;
        let root = game
            .parent()
            .ok_or("game directory has no parent")?
            .to_owned();
        let executable = root.join("cstrike_linux64");
        if !executable.is_file() {
            return Err(format!(
                "Linux CS:S client not found at {}",
                executable.display()
            ));
        }
        let binding = Target::play(&root, &fs.game_dir, &executable)?;
        Ok(Self {
            root,
            game,
            executable,
            binding,
        })
    }

    pub(crate) fn target(&self) -> &Target {
        &self.binding
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        self.binding.verify()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Exited { code: Option<i32>, success: bool },
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Self::Running => "CS:S is running".into(),
            Self::Exited { code, .. } => match code {
                Some(code) => format!("CS:S exited with code {code}"),
                None => "CS:S terminated by a signal".into(),
            },
            Self::Failed(error) => format!("CS:S cleanup failed: {error}"),
        }
    }
}

pub struct Log {
    pub native_activity: Option<serde_json::Value>,
    pub status: Status,
    pub pid: u32,
    pub map: String,
    pub console: String,
    pub launcher: String,
    pub truncated: bool,
}

impl Log {
    pub fn text(&self) -> String {
        let mut text = format!(
            "{}\nMap: {}\n\nEngine console\n{}\n\nLauncher output\n{}",
            self.status.label(),
            self.map,
            self.console,
            self.launcher
        );
        if let Some(activity) = &self.native_activity
            && activity["managed"] == true
            && let Some(message) = activity["message"].as_str()
        {
            text.push_str(&format!("\n\n{message}"));
        }
        text
    }
}

pub struct Run {
    pid: u32,
    map: String,
    console: Arc<NamedTempFile>,
    launcher: Arc<NamedTempFile>,
    status: Arc<Mutex<Status>>,
    process: Arc<Mutex<Process>>,
    monitor: Option<std::thread::JoinHandle<()>>,
}

struct Process {
    child: Child,
    startup: Startup,
    cleanup: Option<Result<ExitStatus, String>>,
}

impl Process {
    fn stop(&mut self) -> Result<ExitStatus, String> {
        if let Some(result) = &self.cleanup {
            return result.clone();
        }
        let exit = match self.child.try_wait() {
            Ok(Some(exit)) => Ok(exit),
            Ok(None) => stop_child(&mut self.child),
            Err(error) => {
                let cleanup = stop_child(&mut self.child);
                cleanup.map_err(|cleanup| format!("{error}; owned child cleanup: {cleanup}"))
            }
        };
        let staging = self.startup.cleanup();
        let result = match (exit, staging) {
            (Ok(exit), Ok(())) => Ok(exit),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(exit), Err(staging)) => Err(format!("{exit}; {staging}")),
        };
        if let Some(permit) = &mut self.startup.permit {
            match &result {
                Ok(_) => permit.confirmed_clean(),
                Err(error) => permit.failed_cleanup(error),
            }
        }
        drop(self.startup.permit.take());
        self.cleanup = Some(result.clone());
        result
    }
}

fn stop_child(child: &mut Child) -> Result<ExitStatus, String> {
    stop_child_with(child, Child::kill)
}

fn stop_child_with(
    child: &mut Child,
    kill: impl FnOnce(&mut Child) -> std::io::Result<()>,
) -> Result<ExitStatus, String> {
    let killed = kill(child);
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => return Ok(exit),
            Err(waited) => {
                return Err(match &killed {
                    Ok(()) => waited.to_string(),
                    Err(killed) => format!("{killed}; child wait: {waited}"),
                });
            }
            Ok(None) => {}
        }
        if let Err(error) = &killed {
            return Err(format!(
                "Could not stop owned game child: {error}; child exit is unconfirmed"
            ));
        }
        if started.elapsed() >= STOP_TIMEOUT {
            return Err("Owned game child did not exit within the cleanup deadline".into());
        }
        std::thread::sleep(WATCH_INTERVAL);
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Default)]
struct Startup {
    map: Option<NamedTempFile>,
    download_list: Option<PathBuf>,
    permit: Option<Permit>,
    cleanup: Option<Result<(), String>>,
}

impl Startup {
    fn cleanup(&mut self) -> Result<(), String> {
        if let Some(result) = &self.cleanup {
            return result.clone();
        }
        let mut failures = Vec::new();
        if let Some(map) = self.map.take() {
            let path = map.path().to_owned();
            if let Err(error) = map.close() {
                failures.push(format!("Owned map cleanup at {}: {error}", path.display()));
            }
        }
        if let Some(path) = self.download_list.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => failures.push(format!(
                    "Owned download-list cleanup at {}: {error}",
                    path.display()
                )),
            }
        }
        let result = if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        };
        self.cleanup = Some(result.clone());
        result
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup()
            && let Some(permit) = &mut self.permit
        {
            permit.failed_cleanup(&error);
        }
    }
}

fn temporary(directory: &Path, prefix: &str, suffix: &str) -> Result<NamedTempFile, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let prefix = format!("{prefix}{}_{timestamp}", std::process::id());
    tempfile::Builder::new()
        .prefix(&prefix)
        .rand_bytes(0)
        .suffix(suffix)
        .tempfile_in(directory)
        .map_err(|e| format!("{}: {e}", directory.display()))
}

fn file_name(path: &Path) -> Result<&str, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "temporary filename is not UTF-8".into())
}

fn tail(path: &Path, lines: usize) -> Result<(String, bool), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let offset = length.saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let mut all: Vec<&str> = text.lines().collect();
    if offset > 0 && !all.is_empty() {
        all.remove(0);
    }
    let start = all.len().saturating_sub(lines.clamp(1, MAX_LOG_LINES));
    Ok((all[start..].join("\n"), offset > 0 || start > 0))
}

pub fn stage_map_bytes(bytes: &[u8], map: &str) -> Result<Vec<u8>, String> {
    let control = compiler::Control::default();
    let original = Bsp::parse(bytes).map_err(|e| e.to_string())?;
    let sky =
        compiler::sky_assets::read_packed_sky(&original, &control).map_err(|e| e.to_string())?;
    let water = compiler::water_assets::read_packed_water(&original, &control)
        .map_err(|e| e.to_string())?;
    let props = compiler::static_prop_assets::read_packed_static_props(&original, &control)
        .map_err(|e| e.to_string())?;
    let bytes =
        compiler::rebase_generated_cubemaps(bytes, map, &control).map_err(|e| e.to_string())?;
    let staged = Bsp::parse(&bytes).map_err(|e| e.to_string())?;
    let staged_sky =
        compiler::sky_assets::read_packed_sky(&staged, &control).map_err(|e| e.to_string())?;
    let staged_water =
        compiler::water_assets::read_packed_water(&staged, &control).map_err(|e| e.to_string())?;
    let staged_props = compiler::static_prop_assets::read_packed_static_props(&staged, &control)
        .map_err(|e| e.to_string())?;
    if props != staged_props {
        return Err("Packed static-prop manifest changed during Run staging".into());
    }
    if sky != staged_sky {
        return Err("Packed sky manifest changed during Run staging".into());
    }
    if water != staged_water {
        return Err("Packed Water manifest changed during Run staging".into());
    }
    if sky.is_some() || water.is_some() || props.is_some() {
        let original = original
            .pak_data(PakLimits::default())
            .map_err(|e| e.to_string())?;
        let staged = staged
            .pak_data(PakLimits::default())
            .map_err(|e| e.to_string())?;
        let sky_paths = sky.iter().flat_map(|manifest| {
            std::iter::once(compiler::sky_assets::PACKED_SKY_MANIFEST_PATH)
                .chain(manifest.assets.iter().map(|asset| asset.path.as_str()))
        });
        let water_paths = water.iter().flat_map(|manifest| {
            std::iter::once(compiler::water_assets::PACKED_WATER_MANIFEST_PATH)
                .chain(manifest.assets.iter().map(|asset| asset.path.as_str()))
        });
        let prop_paths = props.iter().flat_map(|manifest| {
            std::iter::once(compiler::static_prop_assets::PACKED_STATIC_PROPS_MANIFEST_PATH)
                .chain(manifest.assets.iter().map(|asset| asset.path.as_str()))
        });
        for path in sky_paths.chain(water_paths).chain(prop_paths) {
            let original = original.entries.iter().find(|entry| entry.name == path);
            let staged = staged.entries.iter().find(|entry| entry.name == path);
            if original.is_none() || original != staged {
                return Err(format!(
                    "Packed asset bytes changed during Run staging: {path}"
                ));
            }
        }
    }
    Ok(bytes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchCheckpoint {
    Progress,
    Boundary,
}

impl Run {
    pub(crate) fn close_identity(&self) -> serde_json::Value {
        serde_json::json!({"pid": self.pid, "map": self.map, "status": self.status().label()})
    }

    pub fn launch(install: &Install, bsp: &Path) -> Result<Self, String> {
        Self::launch_controlled(install, bsp, || Ok(()))
    }

    pub(crate) fn launch_controlled(
        install: &Install,
        bsp: &Path,
        checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::launch_admitted(install, bsp, None, checkpoint)
    }

    pub(crate) fn launch_admitted(
        install: &Install,
        bsp: &Path,
        permit: Option<Permit>,
        checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::launch_with_monitor(install, bsp, permit, checkpoint, spawn_monitor)
    }

    pub(crate) fn launch_published(
        install: &Install,
        bsp: &Path,
        expected: &compiler::FileIdentity,
        permit: Option<Permit>,
        checkpoint: impl FnMut(LaunchCheckpoint) -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::launch_expected_with_monitor(
            install,
            bsp,
            Some(expected),
            permit,
            checkpoint,
            spawn_monitor,
        )
    }

    fn launch_with_monitor(
        install: &Install,
        bsp: &Path,
        permit: Option<Permit>,
        mut checkpoint: impl FnMut() -> Result<(), String>,
        monitor: impl FnOnce(
            Arc<Mutex<Process>>,
            Arc<Mutex<Status>>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) -> Result<Self, String> {
        Self::launch_expected_with_monitor(install, bsp, None, permit, |_| checkpoint(), monitor)
    }

    fn launch_expected_with_monitor(
        install: &Install,
        bsp: &Path,
        expected: Option<&compiler::FileIdentity>,
        permit: Option<Permit>,
        mut checkpoint: impl FnMut(LaunchCheckpoint) -> Result<(), String>,
        monitor: impl FnOnce(
            Arc<Mutex<Process>>,
            Arc<Mutex<Status>>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) -> Result<Self, String> {
        let mut startup = Startup {
            map: None,
            download_list: None,
            permit,
            cleanup: None,
        };
        let result = Self::launch_prepared(
            install,
            bsp,
            expected,
            &mut startup,
            &mut checkpoint,
            monitor,
        );
        match result {
            Ok(run) => Ok(run),
            Err(error) => match startup.cleanup() {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    if let Some(permit) = &mut startup.permit {
                        permit.failed_cleanup(&cleanup);
                    }
                    Err(format!("{error}; {cleanup}"))
                }
            },
        }
    }

    fn launch_prepared(
        install: &Install,
        bsp: &Path,
        expected: Option<&compiler::FileIdentity>,
        startup: &mut Startup,
        checkpoint: &mut impl FnMut(LaunchCheckpoint) -> Result<(), String>,
        monitor: impl FnOnce(
            Arc<Mutex<Process>>,
            Arc<Mutex<Status>>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) -> Result<Self, String> {
        checkpoint(LaunchCheckpoint::Progress)?;
        install.verify()?;
        let source =
            crate::bounded_file::Source::capture(bsp, PakLimits::default().max_bsp_bytes as u64)
                .map_err(|error| format!("{}: {error}", bsp.display()))?;
        let bytes = source
            .read_controlled(|| {
                checkpoint(LaunchCheckpoint::Progress).map_err(std::io::Error::other)
            })
            .map_err(|error| format!("{}: {error}", bsp.display()))?;
        if let Some(expected) = expected {
            expected.verify_bytes(&bytes).map_err(|error| {
                format!("Compiled BSP bytes changed; compile again before launching: {error}")
            })?;
            expected.verify_metadata().map_err(|error| {
                format!("Compiled BSP changed; compile again before launching: {error}")
            })?;
        }
        Bsp::parse(&bytes).map_err(|e| format!("{}: {e}", bsp.display()))?;
        checkpoint(LaunchCheckpoint::Progress)?;
        let maps = install.game.join("maps");
        std::fs::create_dir_all(&maps).map_err(|e| e.to_string())?;
        startup.map = Some(temporary(&maps, "hammerr_", ".bsp")?);
        let map_file = startup.map.as_mut().expect("startup owns the staged map");
        let map = map_file
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid temporary map filename")?
            .to_owned();
        let bytes = stage_map_bytes(&bytes, &map)?;
        checkpoint(LaunchCheckpoint::Progress)?;
        map_file.write_all(&bytes).map_err(|e| e.to_string())?;
        map_file.flush().map_err(|e| e.to_string())?;
        let console = Arc::new(temporary(&install.game, "hammerr_console_", ".log")?);
        let launcher = Arc::new(temporary(
            &std::env::temp_dir(),
            "hammerr_launcher_",
            ".log",
        )?);
        let mut library_paths = vec![install.root.join("bin/linux64")];
        if let Some(paths) = std::env::var_os("LD_LIBRARY_PATH") {
            library_paths.extend(std::env::split_paths(&paths));
        }
        let libraries = std::env::join_paths(library_paths).map_err(|e| e.to_string())?;
        checkpoint(LaunchCheckpoint::Boundary)?;
        install.verify()?;
        let child = Command::new(&install.executable)
            .current_dir(&install.root)
            .env("LD_LIBRARY_PATH", libraries)
            .env("SteamAppId", "240")
            .env("SteamGameId", "240")
            .args(["-game"])
            .arg(&install.game)
            .args([
                "-windowed",
                "-w",
                "1280",
                "-h",
                "720",
                "-novid",
                "-insecure",
                "-nomaster",
                "-console",
                "+con_logfile",
            ])
            .arg(file_name(console.path())?)
            .args(["+sv_lan", "1", "+servercfgfile"])
            .arg(format!("{map}_absent.cfg"))
            .args(["+map", &map])
            .stdin(Stdio::null())
            .stdout(launcher.as_file().try_clone().map_err(|e| e.to_string())?)
            .stderr(launcher.as_file().try_clone().map_err(|e| e.to_string())?)
            .spawn()
            .map_err(|e| format!("{}: {e}", install.executable.display()))?;
        let pid = child.id();
        startup.download_list = Some(
            install
                .game
                .join("downloadlists")
                .join(format!("{map}.lst")),
        );
        if let Some(permit) = &mut startup.permit {
            permit.phase(Phase::Play);
            permit.started();
        }
        let process = Arc::new(Mutex::new(Process {
            child,
            startup: std::mem::take(startup),
            cleanup: None,
        }));
        let status = Arc::new(Mutex::new(Status::Running));
        let monitor = monitor(process.clone(), status.clone());
        let monitor = match monitor {
            Ok(monitor) => monitor,
            Err(error) => {
                let cleanup = process
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
                return Err(match cleanup {
                    Ok(_) => format!("could not monitor CS:S: {error}"),
                    Err(cleanup) => format!("could not monitor CS:S: {error}; {cleanup}"),
                });
            }
        };
        Ok(Self {
            pid,
            map,
            console,
            launcher,
            status,
            process,
            monitor: Some(monitor),
        })
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn running(&self) -> bool {
        self.status() == Status::Running
    }

    pub fn stop(&self) -> Result<(), String> {
        let result = self
            .process
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = match &result {
            Ok(exit) => exited(*exit),
            Err(error) => Status::Failed(error.clone()),
        };
        result.map(drop)
    }

    pub fn log(&self, lines: usize) -> Result<Log, String> {
        let (console, console_truncated) = tail(self.console.path(), lines)?;
        let (launcher, launcher_truncated) = tail(self.launcher.path(), lines)?;
        Ok(Log {
            native_activity: None,
            status: self.status(),
            pid: self.pid,
            map: self.map.clone(),
            console,
            launcher,
            truncated: console_truncated || launcher_truncated,
        })
    }
}

fn spawn_monitor(
    process: Arc<Mutex<Process>>,
    status: Arc<Mutex<Status>>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("game-monitor".into())
        .spawn(move || monitor_game(process, status))
}

fn monitor_game(process: Arc<Mutex<Process>>, status: Arc<Mutex<Status>>) {
    let result = loop {
        let mut process = process
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = &process.cleanup {
            break result.clone();
        }
        match process.child.try_wait() {
            Ok(None) => {
                drop(process);
                std::thread::sleep(WATCH_INTERVAL);
            }
            Ok(Some(_)) | Err(_) => break process.stop(),
        }
    };
    let finished = match result {
        Ok(exit) => exited(exit),
        Err(error) => Status::Failed(error),
    };
    *status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = finished;
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = self.stop();
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}

fn exited(exit: ExitStatus) -> Status {
    Status::Exited {
        code: exit.code(),
        success: exit.success(),
    }
}

#[cfg(test)]
mod launch_read_tests;
#[cfg(test)]
mod native_activity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use crate::test_assets::game_process as fixture;

    fn wait_until(check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !check() {
            assert!(Instant::now() < deadline, "game watcher timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn exited_game_releases_map_and_preserves_logs() {
        let (_directory, fs, bsp) = fixture(false);
        let run = Run::launch(&Install::from_game(&fs).unwrap(), &bsp).unwrap();
        wait_until(|| !run.running());
        let log = run.log(DEFAULT_LOG_LINES).unwrap();
        assert_eq!(
            log.status,
            Status::Exited {
                code: Some(23),
                success: false
            }
        );
        assert!(log.console.contains(&format!("loaded {}", log.map)));
        assert!(log.launcher.contains("launcher-output"));
        assert_eq!(log.map, log.map.to_lowercase());
        assert!(
            !fs.game_dir
                .join("downloadlists")
                .join(format!("{}.lst", log.map))
                .exists()
        );
        assert!(
            !fs.game_dir
                .join("maps")
                .join(format!("{}.bsp", log.map))
                .exists()
        );
        assert_eq!(
            std::fs::read(fs.game_dir.join("maps/stock.bsp")).unwrap(),
            b"stock map"
        );
        assert!(bsp.exists());
    }

    #[test]
    fn stopping_game_cleans_only_owned_files() {
        let (_directory, fs, bsp) = fixture(true);
        let run = Run::launch(&Install::from_game(&fs).unwrap(), &bsp).unwrap();
        let map = fs.game_dir.join("maps").join(format!("{}.bsp", run.map));
        let console = run.console.path().to_owned();
        let launcher = run.launcher.path().to_owned();
        let download_list = fs
            .game_dir
            .join("downloadlists")
            .join(format!("{}.lst", run.map));
        wait_until(|| {
            run.log(DEFAULT_LOG_LINES)
                .unwrap()
                .console
                .contains("loaded")
        });
        assert!(download_list.exists());
        let stock_list = fs.game_dir.join("downloadlists/stock.lst");
        std::fs::write(&stock_list, b"stock downloads").unwrap();
        assert!(map.exists());
        run.stop().unwrap();
        assert!(!run.running());
        assert!(!map.exists());
        assert!(!download_list.exists());
        assert_eq!(std::fs::read(stock_list).unwrap(), b"stock downloads");
        assert!(console.exists() && launcher.exists());
        drop(run);
        wait_until(|| !console.exists() && !launcher.exists());
        assert!(fs.game_dir.join("maps/stock.bsp").exists() && bsp.exists());
    }

    #[test]
    fn dropping_game_handle_stops_the_child() {
        let (_directory, fs, bsp) = fixture(true);
        let run = Run::launch(&Install::from_game(&fs).unwrap(), &bsp).unwrap();
        let process = run.process.clone();
        let map = fs.game_dir.join("maps").join(format!("{}.bsp", run.map));
        drop(run);
        assert!(process.lock().unwrap().child.try_wait().unwrap().is_some());
        assert!(!map.exists());
    }

    #[test]
    fn failed_launch_does_not_leave_files() {
        let (_directory, fs, bsp) = fixture(false);
        let install = Install::from_game(&fs).unwrap();
        std::fs::set_permissions(&install.executable, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(Run::launch(&install, &bsp).is_err());
        assert_eq!(
            std::fs::read_dir(fs.game_dir.join("maps")).unwrap().count(),
            1
        );
        assert_eq!(std::fs::read_dir(&fs.game_dir).unwrap().count(), 1);
    }

    #[test]
    fn invalid_bsp_is_rejected_before_launch() {
        let (_directory, fs, bsp) = fixture(false);
        std::fs::write(&bsp, "not a BSP").unwrap();
        assert!(Run::launch(&Install::from_game(&fs).unwrap(), &bsp).is_err());
        assert_eq!(
            std::fs::read_dir(fs.game_dir.join("maps")).unwrap().count(),
            1
        );
    }

    #[test]
    fn log_tails_are_bounded_and_accept_utf8() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("😇 line\n".repeat(40000).as_bytes())
            .unwrap();
        let (text, truncated) = tail(file.path(), 3).unwrap();
        assert_eq!(text, "😇 line\n😇 line\n😇 line");
        assert!(truncated);
        let (text, _) = tail(file.path(), usize::MAX).unwrap();
        assert_eq!(text.lines().count(), MAX_LOG_LINES);
        assert!(text.len() <= MAX_LOG_BYTES as usize);
    }
}
