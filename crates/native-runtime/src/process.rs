use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::guard::{self, Baseline};
use crate::util::{self, invalid};
use crate::{CaptureEnvironment, CaptureObserver, CaptureReceipt, CaptureRequest, CaptureSample};

pub(crate) const WORKER_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerRequest {
    pub version: u32,
    pub nonce: String,
    pub supervisor: u32,
    pub supervisor_start: String,
    pub environment: CaptureEnvironment,
    pub profile: WorkerProfile,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "parameters",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum WorkerProfile {
    Reflection(ReflectionProfile),
    PplUv857(crate::PplUv857Case),
    PplMidrange909(crate::PplMidrange909Case),
    PplSpatial919(crate::PplSpatial919Case),
    PplMips931(crate::PplMips931Case),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReflectionProfile {
    pub channel: crate::CaptureChannel,
    pub map_name: String,
    pub samples: Vec<CaptureSample>,
}

impl WorkerRequest {
    pub fn map_name(&self) -> &str {
        match &self.profile {
            WorkerProfile::Reflection(profile) => &profile.map_name,
            WorkerProfile::PplUv857(_)
            | WorkerProfile::PplMidrange909(_)
            | WorkerProfile::PplSpatial919(_)
            | WorkerProfile::PplMips931(_) => crate::diagnostic::MAP_NAME,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerResult {
    pub success: bool,
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
}

const LEASE_FD: i32 = 198;
const WAYLAND_DIRECTORY: &str = "wayland";
const WAYLAND_SOCKET_NAME: &str = "wayland-0";
const WAYLAND_SOCKET_PATH_CAPACITY: usize = 108;

struct Lease(File);

fn lease_directory() -> PathBuf {
    PathBuf::from("/tmp").join(format!("hammerr-native-{}", unsafe { libc::geteuid() }))
}

fn lease() -> io::Result<Lease> {
    lease_at(&lease_directory())
}

fn lease_at(directory: &Path) -> io::Result<Lease> {
    let uid = unsafe { libc::geteuid() };
    match fs::create_dir(directory) {
        Ok(()) => fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(invalid("Native lease directory is not private and owned"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join("capture.lock"))?;
    if !file.metadata()?.is_file()
        || file.metadata()?.uid() != uid
        || file.metadata()?.mode() & 0o077 != 0
    {
        return Err(invalid("Native capture lock must be an owned regular file"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Another native capture is active",
        ));
    }
    Ok(Lease(file))
}

fn inherited_lease() -> io::Result<Lease> {
    if unsafe { libc::fcntl(LEASE_FD, libc::F_GETFD) } < 0 {
        return Err(invalid(
            "Native supervisor requires the caller's inherited lease",
        ));
    }
    let file = unsafe { File::from_raw_fd(LEASE_FD) };
    validate_inherited_lease(file, &lease_directory())
}

fn validate_inherited_lease(file: File, directory: &Path) -> io::Result<Lease> {
    let actual = file.metadata()?;
    let expected = fs::symlink_metadata(directory.join("capture.lock"))?;
    if !actual.is_file()
        || actual.uid() != unsafe { libc::geteuid() }
        || actual.dev() != expected.dev()
        || actual.ino() != expected.ino()
    {
        return Err(invalid(
            "Native supervisor inherited the wrong capture lease",
        ));
    }
    if unsafe { libc::fcntl(LEASE_FD, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let competitor = lease_at(directory);
    match competitor {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Lease(file)),
        Err(error) => Err(error),
        Ok(_) => Err(invalid(
            "Native supervisor inherited an unlocked capture lease",
        )),
    }
}

fn combine(errors: impl IntoIterator<Item = (String, io::Result<()>)>) -> io::Result<()> {
    let failures: Vec<String> = errors
        .into_iter()
        .filter_map(|(label, result)| result.err().map(|error| format!("{label}: {error}")))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(invalid(failures.join("; ")))
    }
}

fn caller_alive(request: &WorkerRequest) -> bool {
    start_time(request.supervisor).ok().as_deref() == Some(&request.supervisor_start)
}

fn check_caller(request: &WorkerRequest, directory: &Path) -> io::Result<()> {
    if !caller_alive(request) || directory.join("cancel").exists() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Native capture caller exited or cancelled",
        ));
    }
    Ok(())
}

pub(crate) fn start_time(pid: u32) -> io::Result<String> {
    let stat = util::text(&PathBuf::from(format!("/proc/{pid}/stat")), 4096)?;
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| invalid("Invalid process stat"))?;
    fields
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
        .ok_or_else(|| invalid("Missing process start time"))
}

fn matching_members(group: u32, field: usize) -> io::Result<Vec<i32>> {
    let mut members = Vec::new();
    for (index, entry) in fs::read_dir("/proc")?.enumerate() {
        if index >= 32768 {
            return Err(invalid("Process scan exceeds 32768 entries"));
        }
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|p| p.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = util::text(&entry.path().join("stat"), 4096) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields: Vec<&str> = fields.split_whitespace().take(4).collect();
        if fields.len() == 4
            && !matches!(fields[0], "Z" | "X")
            && fields[field].parse::<u32>().ok() == Some(group)
        {
            members.push(pid);
        }
    }
    Ok(members)
}

fn members(session: u32) -> io::Result<Vec<i32>> {
    matching_members(session, 3)
}

pub(crate) fn reap(child: &mut Child) -> io::Result<()> {
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(3) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Owned child did not become waitable",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn stop_group(child: &mut Child) -> io::Result<()> {
    let group = child.id();
    for (signal, seconds) in [(libc::SIGTERM, 5), (libc::SIGKILL, 3)] {
        if !matching_members(group, 2)?.is_empty() {
            unsafe {
                libc::kill(-(group as i32), signal);
            }
        }
        let start = Instant::now();
        while !matching_members(group, 2)?.is_empty()
            && start.elapsed() < Duration::from_secs(seconds)
        {
            let _ = child.try_wait();
            thread::sleep(Duration::from_millis(100));
        }
        if matching_members(group, 2)?.is_empty() {
            if child.try_wait()?.is_none() {
                let _ = child.kill();
            }
            return reap(child);
        }
    }
    Err(invalid("Owned child process group did not exit"))
}

fn drain(child: &mut Child, session: u32) -> io::Result<()> {
    for (signal, seconds) in [(libc::SIGTERM, 5), (libc::SIGKILL, 3)] {
        for pid in members(session)?
            .into_iter()
            .filter(|pid| *pid != std::process::id() as i32)
        {
            unsafe {
                libc::kill(pid, signal);
            }
        }
        let started = Instant::now();
        while members(session)?
            .iter()
            .any(|pid| *pid != std::process::id() as i32)
            && started.elapsed() < Duration::from_secs(seconds)
        {
            thread::sleep(Duration::from_millis(100));
        }
        if !members(session)?
            .iter()
            .any(|pid| *pid != std::process::id() as i32)
        {
            return reap(child);
        }
    }
    Err(invalid("Owned native process group did not exit"))
}

struct Supervised {
    child: Child,
    session: u32,
    baseline: Baseline,
    directory: PathBuf,
    audited: bool,
}
impl Supervised {
    fn finish(&mut self) -> io::Result<()> {
        let drain_result = drain(&mut self.child, self.session);
        let audit = guard::finish(&self.baseline, &self.directory, "supervisor");
        self.audited = true;
        let evidence = util::json_write(
            &self.directory.join("private-cleanup.json"),
            &json!({"group_exited":drain_result.is_ok(),"audit_passed":audit.is_ok(),"group_error":drain_result.as_ref().err().map(ToString::to_string),"audit_error":audit.as_ref().err().map(ToString::to_string)}),
        );
        combine([
            ("owned session drain".into(), drain_result),
            ("preservation audit".into(), audit),
            ("cleanup evidence".into(), evidence),
        ])
    }
}
impl Drop for Supervised {
    fn drop(&mut self) {
        if !self.audited {
            let _ = self.finish();
        }
    }
}

fn helper() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let mut directory = executable
        .parent()
        .ok_or_else(|| invalid("Executable has no parent"))?
        .to_owned();
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory.pop();
    }
    let path = directory.join("hammerr-native-runtime");
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Native capture helper missing: {}", path.display()),
        ));
    }
    path.canonicalize()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedSession {
    pid: u32,
    start: String,
}

fn drain_record(directory: &Path) -> io::Result<()> {
    let record_path = directory.join("owned-session.json");
    if !record_path.exists() {
        return Ok(());
    }
    let record: OwnedSession = util::json_read(&record_path, 4096)?;
    if record.pid < 2 {
        return Err(invalid("Invalid owned native session identity"));
    }
    if start_time(record.pid).is_ok_and(|time| time != record.start) {
        return Err(invalid("Owned native session leader identity changed"));
    }
    for (signal, seconds) in [(libc::SIGTERM, 5), (libc::SIGKILL, 3)] {
        for pid in members(record.pid)?
            .into_iter()
            .filter(|pid| *pid != std::process::id() as i32)
        {
            unsafe {
                libc::kill(pid, signal);
            }
        }
        let started = Instant::now();
        while members(record.pid)?
            .iter()
            .any(|pid| *pid != std::process::id() as i32)
            && started.elapsed() < Duration::from_secs(seconds)
        {
            thread::sleep(Duration::from_millis(100));
        }
        if !members(record.pid)?
            .iter()
            .any(|pid| *pid != std::process::id() as i32)
        {
            return Ok(());
        }
    }
    Err(invalid("Owned native session did not exit"))
}

struct CallerChild {
    child: Child,
    directory: PathBuf,
    audited: bool,
}

impl CallerChild {
    fn finish(&mut self) -> io::Result<()> {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let reaped = reap(&mut self.child);
        let drain = drain_record(&self.directory);
        let audit = if self
            .directory
            .join("private-preservation-before.json")
            .exists()
        {
            util::json_read::<Baseline>(
                &self.directory.join("private-preservation-before.json"),
                util::MAX_CONFIG,
            )
            .and_then(|baseline| guard::finish(&baseline, &self.directory, "caller"))
        } else {
            guard::finish_preflight(&self.directory)
        };
        let evidence = util::json_write(
            &self.directory.join("private-caller-cleanup.json"),
            &json!({
                "supervisor_reaped":reaped.is_ok(),"session_drained":drain.is_ok(),"preservation_passed":audit.is_ok(),
                "reap_error":reaped.as_ref().err().map(ToString::to_string),
                "drain_error":drain.as_ref().err().map(ToString::to_string),
                "preservation_error":audit.as_ref().err().map(ToString::to_string),
            }),
        );
        self.audited = true;
        combine([
            ("supervisor reap".into(), reaped),
            ("owned session drain".into(), drain),
            ("preservation audit".into(), audit),
            ("cleanup evidence".into(), evidence),
        ])
    }
}

impl Drop for CallerChild {
    fn drop(&mut self) {
        if !self.audited {
            let _ = self.finish();
        }
    }
}

fn finish_caller(
    owned: &mut CallerChild,
    disposition: &mut crate::CleanupDisposition,
) -> io::Result<()> {
    owned.finish()?;
    *disposition = crate::CleanupDisposition::Confirmed;
    Ok(())
}

pub(crate) fn supervise(directory: &Path) -> io::Result<()> {
    let directory = directory.canonicalize()?;
    validate_capture_socket_path(&directory)?;
    let request: WorkerRequest = util::json_read(&directory.join("request.json"), 2 * 1024 * 1024)?;
    if request.version != WORKER_VERSION
        || directory.file_name().and_then(|name| name.to_str()) != Some(&request.nonce)
    {
        return Err(invalid("Invalid native supervisor request"));
    }
    util::json_write(
        &directory.join("owned-session.json"),
        &OwnedSession {
            pid: std::process::id(),
            start: start_time(std::process::id())?,
        },
    )?;
    let _lease = inherited_lease()?;
    check_caller(&request, &directory)?;
    guard::validate_environment(&request.environment)?;
    guard::ensure_no_client(&request.environment.engine_root)?;
    let helper = helper()?;
    check_caller(&request, &directory)?;
    let baseline = match guard::begin(&request.environment, &helper, &directory) {
        Ok(baseline) => baseline,
        Err(error) => {
            let drain = drain_record(&directory);
            let audit = guard::finish_preflight(&directory);
            let cleanup = combine([
                ("preflight session drain".into(), drain),
                ("preflight preservation".into(), audit),
            ]);
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => invalid(format!("{error}; {cleanup}")),
            });
        }
    };
    let start = Instant::now();
    let operation = (|| {
        check_caller(&request, &directory)?;
        let wayland = directory.join(WAYLAND_DIRECTORY);
        fs::create_dir(&wayland)?;
        fs::set_permissions(&wayland, fs::Permissions::from_mode(0o700))?;
        let log = File::create(directory.join("compositor.log"))?;
        let mut command = Command::new("cage");
        command
            .arg("--")
            .arg(&helper)
            .arg("--capture-worker")
            .arg(&directory)
            .current_dir(&directory)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_HEADLESS_OUTPUTS", "1")
            .env("WLR_RENDERER", "pixman")
            .env("XDG_RUNTIME_DIR", &wayland)
            .env("SDL_AUDIODRIVER", "dummy")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("LD_PRELOAD")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        command.process_group(0);
        let child = command.spawn()?;
        let mut owned = Supervised {
            child,
            session: std::process::id(),
            baseline: baseline.clone(),
            directory: directory.clone(),
            audited: false,
        };
        let waiting = (|| {
            let mut cancel_at = None;
            loop {
                let parent_alive = caller_alive(&request);
                if (!parent_alive || directory.join("cancel").exists()) && cancel_at.is_none() {
                    cancel_at = Some(Instant::now());
                    util::atomic_write(&directory.join("cancel"), b"cancel\n")?;
                }
                util::private_budget(&directory, &request.environment.engine_root)?;
                if fs::metadata(directory.join("compositor.log"))?.len() > 8 * 1024 * 1024 {
                    return Err(invalid("Native compositor log exceeds 8 MiB"));
                }
                if let Some(status) = owned.child.try_wait()? {
                    if !status.success() {
                        let report = util::json_read::<WorkerResult>(
                            &directory.join("result.json"),
                            64 * 1024,
                        );
                        let detail = match report {
                            Ok(report) => report
                                .error
                                .unwrap_or_else(|| "Worker did not report its failure".into()),
                            Err(error) => format!("Cannot read worker failure report: {error}"),
                        };
                        return Err(invalid(format!(
                            "Private worker exited with {status}: {detail}"
                        )));
                    }
                    break;
                }
                if start.elapsed() > Duration::from_secs(210)
                    || cancel_at.is_some_and(|time| time.elapsed() > Duration::from_secs(20))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Native capture or cleanup timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
            let result: WorkerResult = util::json_read(&directory.join("result.json"), 64 * 1024)?;
            if !result.success || !result.verified_controls {
                return Err(invalid(result.error.unwrap_or_else(|| {
                    "Native worker returned incomplete capture".into()
                })));
            }
            if cancel_at.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Native capture cancelled or caller exited",
                ));
            }
            Ok(result)
        })();
        let cleanup = owned.finish();
        if waiting.is_ok() && cleanup.is_ok() {
            guard::validate_environment(&request.environment)?;
        }
        match (waiting, cleanup) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(a), Err(b)) => Err(invalid(format!("{a}; cleanup: {b}"))),
            (Err(e), _) | (_, Err(e)) => Err(e),
        }
    })();
    let operation = if let Err(error) = operation {
        let drain = drain_record(&directory);
        let audit = guard::finish(&baseline, &directory, "supervisor-failure");
        let cleanup = combine([
            ("drain".into(), drain),
            ("preservation audit".into(), audit),
        ]);
        Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => invalid(format!("{error}; {cleanup}")),
        })
    } else {
        operation
    };
    let record = match &operation {
        Ok(result) => WorkerResult {
            success: true,
            error: None,
            elapsed_ms: result.elapsed_ms,
            verified_controls: true,
        },
        Err(error) => WorkerResult {
            success: false,
            error: Some(error.to_string()),
            elapsed_ms: start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            verified_controls: false,
        },
    };
    util::json_write(&directory.join("supervisor-result.json"), &record)?;
    operation.map(|_| ())
}

pub fn capture(
    request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> io::Result<CaptureReceipt> {
    capture_with_cleanup(request, observer).map_err(|failure| failure.error)
}

fn validate_capture_socket_path(directory: &Path) -> io::Result<()> {
    let socket = directory.join(WAYLAND_DIRECTORY).join(WAYLAND_SOCKET_NAME);
    let bytes = socket.as_os_str().as_bytes().len() + 1;
    if bytes > WAYLAND_SOCKET_PATH_CAPACITY {
        return Err(invalid(format!(
            "Native capture Wayland socket path requires {bytes} bytes including its NUL terminator, exceeding the Linux limit of {WAYLAND_SOCKET_PATH_CAPACITY} bytes; choose a shorter lowercase TMPDIR (planned socket: {})",
            socket.display(),
        )));
    }
    Ok(())
}

fn capture_directory_in(parent: &Path) -> io::Result<tempfile::TempDir> {
    let parent = parent.canonicalize()?;
    if parent
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(u8::is_ascii_uppercase)
    {
        return Err(invalid(
            "Native capture requires a temporary directory path without uppercase ASCII characters; choose a lowercase TMPDIR",
        ));
    }
    let nonce = getrandom::u64().map_err(io::Error::other)?;
    let prefix = format!("hammerr_capture_{nonce:016x}");
    validate_capture_socket_path(&parent.join(&prefix))?;
    tempfile::Builder::new()
        .prefix(&prefix)
        .rand_bytes(0)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
}

pub fn capture_with_cleanup(
    request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> Result<CaptureReceipt, crate::CaptureFailure> {
    let mut cleanup = crate::CleanupDisposition::Confirmed;
    capture_inner(request, observer, &mut cleanup, &std::env::temp_dir())
        .map_err(|error| crate::CaptureFailure { error, cleanup })
}

fn capture_inner(
    mut request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
    disposition: &mut crate::CleanupDisposition,
    temporary_parent: &Path,
) -> io::Result<CaptureReceipt> {
    observer.checkpoint()?;
    let directory = capture_directory_in(temporary_parent)?;
    let lease = lease()?;
    util::validate_name(&request.map_name)?;
    if request.samples.is_empty() || request.samples.len() > 32 {
        return Err(invalid("Generate reflections requires 1–32 samples"));
    }
    let mut positions = std::collections::BTreeSet::new();
    for sample in &request.samples {
        if sample.dimension == 0
            || !sample.dimension.is_power_of_two()
            || sample.dimension > crate::MAX_CUBE_EDGE
            || !positions.insert(sample.origin)
        {
            return Err(invalid(format!(
                "Capture requires distinct sample positions and power-of-two cube edges 1–{}",
                crate::MAX_CUBE_EDGE
            )));
        }
        let expected = format!(
            "materials/maps/{}/c{}_{}_{}{}",
            request.map_name,
            sample.origin[0],
            sample.origin[1],
            sample.origin[2],
            request.channel.texture_suffix()
        );
        if sample.texture != expected || sample.bootstrap_sha256.len() != 64 {
            return Err(invalid("Invalid native sample identity"));
        }
    }
    request.environment = guard::resolve_environment(&request.environment)?;
    guard::validate_environment(&request.environment)?;
    let directory = directory.keep().canonicalize()?;
    let staged = crate::adapter::capture_staging_bsp_for_channel(&request.bsp, request.channel)?;
    if staged
        .len()
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(64 * 1024 * 1024))
        .is_none_or(|bytes| bytes > 256 * 1024 * 1024)
    {
        return Err(invalid(
            "Native reflection capture admits at most 96 MiB of BSP staging plus 64 MiB scratch within 256 MiB",
        ));
    }
    drop(request.bsp);
    util::atomic_write(&directory.join("staged.bsp"), &staged)?;
    let input_hash = util::hash(&staged);
    let worker = WorkerRequest {
        version: WORKER_VERSION,
        nonce: directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("Invalid capture directory"))?
            .to_owned(),
        supervisor: std::process::id(),
        supervisor_start: start_time(std::process::id())?,
        environment: request.environment,
        profile: WorkerProfile::Reflection(ReflectionProfile {
            channel: request.channel,
            map_name: request.map_name,
            samples: request.samples,
        }),
    };
    let result = run_owned(&directory, &worker, &lease, observer, disposition)?;
    observer.checkpoint()?;
    if util::hash_file(&directory.join("staged.bsp"), util::MAX_BSP)? != input_hash {
        return Err(invalid("Native capture mutated its staged baseline"));
    }
    let bsp = util::read(&directory.join("captured.bsp"), util::MAX_BSP)?;
    Ok(CaptureReceipt {
        channel: request.channel,
        bsp,
        staged_bsp: staged,
        artifacts: directory,
        elapsed_ms: result.elapsed_ms,
        verified_controls: true,
        preservation_passed: true,
    })
}

fn run_owned(
    directory: &Path,
    worker: &WorkerRequest,
    lease: &Lease,
    observer: &mut dyn CaptureObserver,
    disposition: &mut crate::CleanupDisposition,
) -> io::Result<WorkerResult> {
    let helper = helper()?;
    util::json_write(&directory.join("request.json"), worker)?;
    let log = File::create(directory.join("supervisor.log"))?;
    let mut command = Command::new(&helper);
    command
        .arg("--capture-supervisor")
        .arg(directory)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    let lease_fd = lease.0.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(lease_fd, LEASE_FD) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(LEASE_FD, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut owned = CallerChild {
        child: command.spawn()?,
        directory: directory.to_owned(),
        audited: false,
    };
    *disposition = crate::CleanupDisposition::Unconfirmed;
    let start = Instant::now();
    let mut cancel_at = None;
    let mut cancellation = None;
    let result = (|| {
        loop {
            if cancellation.is_none()
                && let Err(error) = observer.checkpoint()
            {
                cancellation = Some(error);
                cancel_at = Some(Instant::now());
                util::atomic_write(&directory.join("cancel"), b"cancel\n")?;
            }
            if let Ok(progress) =
                util::json_read::<[usize; 2]>(&directory.join("progress.json"), 1024)
            {
                observer.sample_progress(progress[0], progress[1]);
            }
            if fs::metadata(directory.join("supervisor.log"))?.len() > util::MAX_LOG as u64 {
                return Err(invalid("Native supervisor log exceeds 2 MiB"));
            }
            if let Some(status) = owned.child.try_wait()? {
                if !status.success() {
                    let report = util::json_read::<WorkerResult>(
                        &directory.join("supervisor-result.json"),
                        64 * 1024,
                    )
                    .ok();
                    return Err(invalid(
                        report
                            .and_then(|report| report.error)
                            .unwrap_or_else(|| format!("Native supervisor exited with {status}")),
                    ));
                }
                break;
            }
            if start.elapsed() > Duration::from_secs(280)
                || cancel_at.is_some_and(|at| at.elapsed() > Duration::from_secs(40))
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native supervisor timed out",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
        if let Some(error) = cancellation.take() {
            return Err(error);
        }
        util::json_read::<WorkerResult>(&directory.join("supervisor-result.json"), 64 * 1024)
    })();
    let cleanup = finish_caller(&mut owned, disposition);
    if let Err(error) = cleanup {
        return Err(invalid(format!(
            "{}; cleanup/audit: {error}; inspect {}",
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
            directory.display()
        )));
    }
    let result =
        result.map_err(|error| invalid(format!("{error}; inspect {}", directory.display())))?;
    if !result.success || !result.verified_controls {
        return Err(invalid(
            "Native supervisor returned incomplete capture evidence",
        ));
    }
    Ok(result)
}

pub fn diagnose_ppl_uv857_with_cleanup(
    request: crate::PplUv857Request,
    observer: &mut dyn CaptureObserver,
) -> Result<crate::PplUv857Receipt, crate::CaptureFailure> {
    let (artifacts, elapsed_ms) = diagnose_ppl_with_cleanup((&request).into(), observer)?;
    Ok(crate::PplUv857Receipt {
        case: request.case,
        artifacts,
        elapsed_ms,
        verified_controls: true,
        hardware_observed: true,
        preservation_passed: true,
    })
}

pub fn diagnose_ppl_midrange909_with_cleanup(
    request: crate::PplMidrange909Request,
    observer: &mut dyn CaptureObserver,
) -> Result<crate::PplMidrange909Receipt, crate::CaptureFailure> {
    let (artifacts, elapsed_ms) = diagnose_ppl_with_cleanup((&request).into(), observer)?;
    Ok(crate::PplMidrange909Receipt {
        case: request.case,
        artifacts,
        elapsed_ms,
        verified_controls: true,
        hardware_observed: true,
        preservation_passed: true,
    })
}

pub fn diagnose_ppl_spatial919_with_cleanup(
    request: crate::PplSpatial919Request,
    observer: &mut dyn CaptureObserver,
) -> Result<crate::PplSpatial919Receipt, crate::CaptureFailure> {
    let (artifacts, elapsed_ms) = diagnose_ppl_with_cleanup((&request).into(), observer)?;
    Ok(crate::PplSpatial919Receipt {
        case: request.case,
        artifacts,
        elapsed_ms,
        verified_controls: true,
        hardware_observed: true,
        preservation_passed: true,
    })
}

pub fn diagnose_ppl_mips931_with_cleanup(
    request: crate::PplMips931Request,
    observer: &mut dyn CaptureObserver,
) -> Result<crate::PplMips931Receipt, crate::CaptureFailure> {
    let (artifacts, elapsed_ms) = diagnose_ppl_with_cleanup((&request).into(), observer)?;
    Ok(crate::PplMips931Receipt {
        case: request.case,
        artifacts,
        elapsed_ms,
        verified_controls: true,
        hardware_observed: true,
        preservation_passed: true,
    })
}

fn diagnose_ppl_with_cleanup(
    request: crate::diagnostic::DiagnosticRequest<'_>,
    observer: &mut dyn CaptureObserver,
) -> Result<(PathBuf, u64), crate::CaptureFailure> {
    let mut cleanup = crate::CleanupDisposition::Confirmed;
    diagnose_ppl_inner(request, observer, &mut cleanup)
        .map_err(|error| crate::CaptureFailure { error, cleanup })
}

fn diagnose_ppl_inner(
    request: crate::diagnostic::DiagnosticRequest<'_>,
    observer: &mut dyn CaptureObserver,
    disposition: &mut crate::CleanupDisposition,
) -> io::Result<(PathBuf, u64)> {
    observer.checkpoint()?;
    crate::diagnostic::validate_request_paths(&request)?;
    let parent = request.artifacts_parent.canonicalize()?;
    crate::diagnostic::path_text(&parent, "canonical artifact parent")?;
    if parent.starts_with("/tmp") || parent.starts_with("/dev/shm") {
        return Err(invalid(
            "Native diagnostics require retained artifacts on disk",
        ));
    }
    let directory = capture_directory_in(&parent)?;
    let lease = lease()?;
    let environment = crate::diagnostic::environment(request.environment)?;
    let directory = directory.keep().canonicalize()?;
    let prepare = crate::diagnostic::stage(&directory, &request);
    if let Err(error) = prepare {
        return Err(invalid(format!("{error}; inspect {}", directory.display())));
    }
    let worker = WorkerRequest {
        version: WORKER_VERSION,
        nonce: directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("Invalid diagnostic directory"))?
            .to_owned(),
        supervisor: std::process::id(),
        supervisor_start: start_time(std::process::id())?,
        environment,
        profile: match request.case {
            crate::diagnostic::DiagnosticCase::Uv857(case) => WorkerProfile::PplUv857(case),
            crate::diagnostic::DiagnosticCase::Midrange909(case) => {
                WorkerProfile::PplMidrange909(case)
            }
            crate::diagnostic::DiagnosticCase::Spatial919(case) => {
                WorkerProfile::PplSpatial919(case)
            }
            crate::diagnostic::DiagnosticCase::Mips931(case) => WorkerProfile::PplMips931(case),
        },
    };
    let result = run_owned(&directory, &worker, &lease, observer, disposition);
    let sources = crate::diagnostic::verify_sources(&request);
    let sources_evidence = util::json_write(
        &directory.join("source-preservation.json"),
        &json!({
            "success":sources.is_ok(),"error":sources.as_ref().err().map(ToString::to_string),
        }),
    );
    let audit = combine([
        ("frozen source preservation".into(), sources),
        ("source preservation evidence".into(), sources_evidence),
    ]);
    match (result, audit) {
        (Ok(result), Ok(())) => {
            observer.checkpoint()?;
            Ok((directory, result.elapsed_ms))
        }
        (Err(a), Err(b)) => Err(invalid(format!(
            "{a}; {b}; inspect {}",
            directory.display()
        ))),
        (Err(error), _) | (_, Err(error)) => {
            Err(invalid(format!("{error}; inspect {}", directory.display())))
        }
    }
}
