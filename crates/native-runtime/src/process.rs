use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerRequest {
    pub version: u32,
    pub nonce: String,
    pub supervisor: u32,
    pub supervisor_start: String,
    pub environment: CaptureEnvironment,
    pub map_name: String,
    pub samples: Vec<CaptureSample>,
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

pub(crate) fn supervise(directory: &Path) -> io::Result<()> {
    let directory = directory.canonicalize()?;
    let request: WorkerRequest = util::json_read(&directory.join("request.json"), 2 * 1024 * 1024)?;
    if request.version != 1
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
            let audit = guard::finish_preflight(&directory);
            return Err(match audit {
                Ok(()) => error,
                Err(audit) => invalid(format!("{error}; preflight preservation: {audit}")),
            });
        }
    };
    let start = Instant::now();
    let operation = (|| {
        check_caller(&request, &directory)?;
        let wayland = directory.join("wayland");
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
                if !parent_alive || directory.join("cancel").exists() {
                    if cancel_at.is_none() {
                        cancel_at = Some(Instant::now());
                        util::atomic_write(&directory.join("cancel"), b"cancel\n")?;
                    }
                }
                util::private_budget(&directory, &request.environment.engine_root)?;
                if fs::metadata(directory.join("compositor.log"))?.len() > 8 * 1024 * 1024 {
                    return Err(invalid("Native compositor log exceeds 8 MiB"));
                }
                if let Some(status) = owned.child.try_wait()? {
                    if !status.success() {
                        return Err(invalid(format!("Private worker exited with {status}")));
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
    mut request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> io::Result<CaptureReceipt> {
    observer.checkpoint()?;
    let lease = lease()?;
    util::validate_name(&request.map_name)?;
    if request.samples.is_empty() || request.samples.len() > 32 {
        return Err(invalid("Generate reflections requires 1–32 samples"));
    }
    let mut positions = std::collections::BTreeSet::new();
    for sample in &request.samples {
        if sample.dimension == 0
            || !sample.dimension.is_power_of_two()
            || sample.dimension > 128
            || !positions.insert(sample.origin)
        {
            return Err(invalid(
                "Capture requires distinct sample positions and cube edges 1–128; 256 is unavailable",
            ));
        }
        let expected = format!(
            "materials/maps/{}/c{}_{}_{}.vtf",
            request.map_name, sample.origin[0], sample.origin[1], sample.origin[2]
        );
        if sample.texture != expected || sample.bootstrap_sha256.len() != 64 {
            return Err(invalid("Invalid native sample identity"));
        }
    }
    request.environment = guard::resolve_environment(&request.environment)?;
    guard::validate_environment(&request.environment)?;
    let helper = helper()?;
    let directory = tempfile::Builder::new()
        .prefix("hammerr_capture_")
        .tempdir()?
        .keep();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let staged = crate::adapter::capture_staging_bsp(&request.bsp)?;
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
        version: 1,
        nonce: directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("Invalid capture directory"))?
            .to_owned(),
        supervisor: std::process::id(),
        supervisor_start: start_time(std::process::id())?,
        environment: request.environment,
        map_name: request.map_name,
        samples: request.samples,
    };
    util::json_write(&directory.join("request.json"), &worker)?;
    let log = File::create(directory.join("supervisor.log"))?;
    let mut command = Command::new(&helper);
    command
        .arg("--capture-supervisor")
        .arg(&directory)
        .current_dir(&directory)
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
    let mut child = command.spawn()?;
    let start = Instant::now();
    let mut cancel_at = None;
    let mut cancellation = None;
    let result = (|| {
        loop {
            if cancellation.is_none() {
                if let Err(error) = observer.checkpoint() {
                    cancellation = Some(error);
                    cancel_at = Some(Instant::now());
                    util::atomic_write(&directory.join("cancel"), b"cancel\n")?;
                }
            }
            if let Ok(progress) =
                util::json_read::<[usize; 2]>(&directory.join("progress.json"), 1024)
            {
                observer.sample_progress(progress[0], progress[1]);
            }
            if fs::metadata(directory.join("supervisor.log"))?.len() > util::MAX_LOG as u64 {
                return Err(invalid("Native supervisor log exceeds 2 MiB"));
            }
            if let Some(status) = child.try_wait()? {
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
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let reaped = reap(&mut child);
    let drain = drain_record(&directory);
    let audit = if directory.join("private-preservation-before.json").exists() {
        util::json_read::<Baseline>(
            &directory.join("private-preservation-before.json"),
            util::MAX_CONFIG,
        )
        .and_then(|baseline| guard::finish(&baseline, &directory, "caller"))
    } else {
        guard::finish_preflight(&directory)
    };
    let evidence = util::json_write(
        &directory.join("private-caller-cleanup.json"),
        &json!({
            "supervisor_reaped":reaped.is_ok(),
            "session_drained":drain.is_ok(),
            "preservation_passed":audit.is_ok(),
            "reap_error":reaped.as_ref().err().map(ToString::to_string),
            "drain_error":drain.as_ref().err().map(ToString::to_string),
            "preservation_error":audit.as_ref().err().map(ToString::to_string),
        }),
    );
    let cleanup = combine([
        ("supervisor reap".into(), reaped),
        ("owned session drain".into(), drain),
        ("preservation audit".into(), audit),
        ("cleanup evidence".into(), evidence),
    ]);
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
    observer.checkpoint()?;
    if util::hash_file(&directory.join("staged.bsp"), util::MAX_BSP)? != input_hash {
        return Err(invalid("Native capture mutated its staged baseline"));
    }
    let bsp = util::read(&directory.join("captured.bsp"), util::MAX_BSP)?;
    Ok(CaptureReceipt {
        bsp,
        staged_bsp: staged,
        artifacts: directory,
        elapsed_ms: result.elapsed_ms,
        verified_controls: true,
        preservation_passed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    #[test]
    fn lease_namespace_is_independent_of_job_temporary_paths() {
        assert_eq!(lease_directory().parent(), Some(Path::new("/tmp")));
        let directory = private_directory();
        let lease = lease_at(directory.path()).unwrap();
        assert_eq!(
            lease_at(directory.path()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(lease);
        lease_at(directory.path()).unwrap();
    }

    #[test]
    fn lease_refuses_fifo_without_waiting() {
        let directory = private_directory();
        let path =
            std::ffi::CString::new(directory.path().join("capture.lock").as_os_str().as_bytes())
                .unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(lease_at(directory.path()).is_err());
    }

    #[test]
    fn cleanup_errors_preserve_each_failure() {
        let error = combine([
            ("operation".into(), Err(invalid("original"))),
            ("audit".into(), Err(invalid("changed configuration"))),
        ])
        .unwrap_err()
        .to_string();
        assert!(error.contains("operation: original"));
        assert!(error.contains("audit: changed configuration"));
    }

    #[test]
    #[ignore]
    fn inherited_lease_fixture() {
        let Some(directory) = std::env::var_os("HAMMERR_LEASE_FIXTURE").map(PathBuf::from) else {
            return;
        };
        let file = unsafe { File::from_raw_fd(LEASE_FD) };
        let _lease = validate_inherited_lease(file, &directory).unwrap();
        util::atomic_write(&directory.join("ready"), b"ready").unwrap();
        thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn inherited_lease_survives_either_owner_until_cleanup_finishes() {
        for kill_supervisor in [false, true] {
            let directory = private_directory();
            let lease = lease_at(directory.path()).unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process::tests::inherited_lease_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HAMMERR_LEASE_FIXTURE", directory.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            let fd = lease.0.as_raw_fd();
            unsafe {
                command.pre_exec(move || {
                    if libc::dup2(fd, LEASE_FD) < 0 || libc::fcntl(LEASE_FD, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().unwrap();
            let start = Instant::now();
            while !directory.path().join("ready").exists()
                && start.elapsed() < Duration::from_secs(3)
            {
                thread::sleep(Duration::from_millis(10));
            }
            if !directory.path().join("ready").exists() {
                let _ = stop_group(&mut child);
                panic!("Inherited-lease fixture did not start");
            }
            if kill_supervisor {
                stop_group(&mut child).unwrap();
                assert_eq!(
                    lease_at(directory.path()).err().unwrap().kind(),
                    io::ErrorKind::WouldBlock
                );
                drop(lease);
            } else {
                drop(lease);
                assert_eq!(
                    lease_at(directory.path()).err().unwrap().kind(),
                    io::ErrorKind::WouldBlock
                );
                stop_group(&mut child).unwrap();
            }
            lease_at(directory.path()).unwrap();
        }
    }

    #[test]
    #[ignore]
    fn owned_session_fixture() {
        let Some(directory) = std::env::var_os("HAMMERR_OWNERSHIP_FIXTURE").map(PathBuf::from)
        else {
            return;
        };
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        util::atomic_write(&directory.join("child"), child.id().to_string().as_bytes()).unwrap();
        if std::env::var_os("HAMMERR_OWNERSHIP_EARLY_EXIT").is_none() {
            child.wait().unwrap();
        }
    }

    #[test]
    fn cleanup_finds_other_process_groups_after_leader_exit() {
        for early in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process::tests::owned_session_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HAMMERR_OWNERSHIP_FIXTURE", directory.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if early {
                command.env("HAMMERR_OWNERSHIP_EARLY_EXIT", "1");
            }
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            let mut child = command.spawn().unwrap();
            let session = child.id();
            let started = Instant::now();
            while !directory.path().join("child").exists()
                && started.elapsed() < Duration::from_secs(3)
            {
                thread::sleep(Duration::from_millis(10));
            }
            let marker = util::text(&directory.path().join("child"), 32);
            if marker.is_err() {
                let _ = drain(&mut child, session);
                panic!("Owned-session fixture did not start");
            }
            let descendant = marker.unwrap().parse::<i32>().unwrap();
            assert_ne!(descendant, session as i32);
            if early {
                reap(&mut child).unwrap();
            }
            let before = members(session).unwrap();
            let cleanup = drain(&mut child, session);
            assert!(before.contains(&descendant));
            cleanup.unwrap();
            assert!(members(session).unwrap().is_empty());
        }
    }
}
