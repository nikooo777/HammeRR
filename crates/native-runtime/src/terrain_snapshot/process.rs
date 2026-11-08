use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::isolation::{Identity, Launch};
use crate::util::invalid;
use crate::{CaptureFailure, CleanupDisposition};

const MAX_THREADS: usize = 1024;
const MAX_MAPS: usize = 16384;
const MAX_MAP_BYTES: usize = 4 * 1024 * 1024;
const MAX_READ_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_READ_BYTES: usize = 64 * 1024 * 1024;
const MAX_PROC_BYTES: usize = 16 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
const RESUME_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Serialize)]
pub(super) struct ProcessReceipt {
    pub pid: u32,
    pub start: String,
    pub group: u32,
    pub session: u32,
    pub uid: u32,
    pub executable: PathBuf,
    pub executable_device: u64,
    pub executable_inode: u64,
    pub stopped_threads: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Mapping {
    pub start: u64,
    pub end: u64,
    pub offset: u64,
    pub inode: u64,
    pub device_major: u32,
    pub device_minor: u32,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub private: bool,
    pub path: Option<PathBuf>,
}

#[derive(PartialEq, Eq)]
struct ThreadState {
    tid: u32,
    start: String,
    state: char,
}

enum ThreadScan {
    Complete(Vec<ThreadState>),
    Invalidated(String),
}

struct Status {
    pid: u32,
    tgid: u32,
    uid: [u32; 4],
    tracer: u32,
    state: char,
}

struct Lease {
    directory: File,
    receipt: ProcessReceipt,
    stopped: Vec<ThreadState>,
    armed: bool,
}

pub(super) struct StoppedProcess<'a> {
    lease: &'a Lease,
    memory: File,
    mappings: Vec<Mapping>,
    maps_text: String,
    checkpoint: &'a mut dyn FnMut() -> io::Result<()>,
    started: Instant,
    read_bytes: usize,
}

fn opened_at(directory: &File, name: &str, flags: i32) -> io::Result<File> {
    let name = CString::new(name).map_err(|_| invalid("Invalid owned process path"))?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | flags,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn bytes_at(directory: &File, name: &str, limit: usize) -> io::Result<Vec<u8>> {
    let file = opened_at(directory, name, libc::O_NOFOLLOW)?;
    let capacity = limit
        .checked_add(1)
        .ok_or_else(|| invalid("Owned process read limit overflow"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(io::Error::other)?;
    file.take(capacity as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid(format!(
            "Owned process {name} exceeds its read budget"
        )));
    }
    Ok(bytes)
}

fn text_at(directory: &File, name: &str, limit: usize) -> io::Result<String> {
    String::from_utf8(bytes_at(directory, name, limit)?).map_err(io::Error::other)
}

fn status_number(text: &str, name: &str) -> io::Result<u32> {
    let field = text
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .ok_or_else(|| invalid(format!("Missing owned process {name}")))?;
    field.trim().parse().map_err(io::Error::other)
}

fn status(text: &str) -> io::Result<Status> {
    let state = text
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .and_then(|field| field.split_whitespace().next())
        .ok_or_else(|| invalid("Missing owned process state"))?;
    let state = process_state(state)?;
    let fields = text
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or_else(|| invalid("Missing owned process UID"))?;
    let mut fields = fields.split_whitespace();
    let mut uid = [0; 4];
    for value in &mut uid {
        *value = fields
            .next()
            .ok_or_else(|| invalid("Incomplete owned process UID"))?
            .parse()
            .map_err(io::Error::other)?;
    }
    if fields.next().is_some() {
        return Err(invalid("Invalid owned process UID"));
    }
    Ok(Status {
        pid: status_number(text, "Pid:")?,
        tgid: status_number(text, "Tgid:")?,
        uid,
        tracer: status_number(text, "TracerPid:")?,
        state,
    })
}

fn process_state(value: &str) -> io::Result<char> {
    let mut characters = value.chars();
    let state = characters
        .next()
        .ok_or_else(|| invalid("Missing owned process state"))?;
    if characters.next().is_some()
        || !matches!(
            state,
            'R' | 'S' | 'D' | 'Z' | 'T' | 't' | 'X' | 'x' | 'K' | 'W' | 'P' | 'I'
        )
    {
        return Err(invalid("Invalid owned process state"));
    }
    Ok(state)
}

fn context(error: io::Error, stage: impl std::fmt::Display) -> io::Error {
    io::Error::new(error.kind(), format!("{stage}: {error}"))
}

fn vanished_thread(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH))
}

fn stat_identity(text: &str) -> io::Result<(u32, u32, String, char)> {
    let (_, fields) = text
        .rsplit_once(") ")
        .ok_or_else(|| invalid("Invalid owned process stat"))?;
    let mut fields = fields.split_whitespace();
    let state = process_state(
        fields
            .clone()
            .next()
            .ok_or_else(|| invalid("Missing owned process stat state"))?,
    )?;
    let group = fields
        .nth(2)
        .ok_or_else(|| invalid("Missing owned process group"))?
        .parse()
        .map_err(io::Error::other)?;
    let session = fields
        .next()
        .ok_or_else(|| invalid("Missing owned process session"))?
        .parse()
        .map_err(io::Error::other)?;
    let start = fields
        .nth(15)
        .ok_or_else(|| invalid("Missing owned process start time"))?
        .to_owned();
    Ok((group, session, start, state))
}

fn executable_path(directory: &File) -> io::Result<PathBuf> {
    let mut bytes = vec![0; MAX_PATH_BYTES];
    let length = unsafe {
        libc::readlinkat(
            directory.as_raw_fd(),
            c"exe".as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize == bytes.len() {
        return Err(invalid("Owned native executable path exceeds its budget"));
    }
    bytes.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

impl Lease {
    fn open(
        launch: &Launch,
        child: &Child,
        executable: &Path,
        expected: &Identity,
    ) -> io::Result<Self> {
        let actual = launch
            .identity(child, executable)?
            .ok_or_else(|| invalid("Owned native process is unavailable before snapshot"))?;
        if actual.pid != expected.pid
            || actual.start != expected.start
            || actual.group != expected.group
            || actual.session != expected.session
            || actual.uid != expected.uid
            || actual.pid < 2
            || actual.uid != unsafe { libc::geteuid() }
        {
            return Err(invalid(
                "Owned native process identity changed before snapshot",
            ));
        }
        let executable = executable.canonicalize()?;
        let metadata = fs::metadata(&executable)?;
        if !metadata.is_file() {
            return Err(invalid("Owned native executable is not a regular file"));
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(format!("/proc/{}", actual.pid))?;
        let lease = Self {
            directory,
            receipt: ProcessReceipt {
                pid: actual.pid,
                start: actual.start,
                group: actual.group,
                session: actual.session,
                uid: actual.uid,
                executable,
                executable_device: metadata.dev(),
                executable_inode: metadata.ino(),
                stopped_threads: 0,
            },
            stopped: Vec::new(),
            armed: false,
        };
        lease.verify_identity()?;
        Ok(lease)
    }

    fn verify_status(&self, status: &Status, tid: u32) -> io::Result<()> {
        if status.pid != tid
            || status.tgid != self.receipt.pid
            || status.uid != [self.receipt.uid; 4]
            || status.tracer != 0
            || status.state == 't'
            || (tid == self.receipt.pid && matches!(status.state, 'Z' | 'X' | 'x'))
        {
            return Err(invalid(format!(
                "Owned native thread {tid} identity, tracing or main exit state changed"
            )));
        }
        Ok(())
    }

    fn verify_identity(&self) -> io::Result<()> {
        if self
            .directory
            .metadata()
            .map_err(|error| context(error, "Owned native main process directory metadata"))?
            .uid()
            != self.receipt.uid
        {
            return Err(invalid("Owned native process directory changed user"));
        }
        let current = text_at(&self.directory, "status", MAX_PROC_BYTES)
            .map_err(|error| context(error, "Owned native main process status read"))?;
        let current = status(&current)
            .map_err(|error| context(error, "Owned native main process status parse"))?;
        self.verify_status(&current, self.receipt.pid)?;
        let stat = text_at(&self.directory, "stat", MAX_PROC_BYTES)
            .map_err(|error| context(error, "Owned native main process stat read"))?;
        let (group, session, start, state) = stat_identity(&stat)
            .map_err(|error| context(error, "Owned native main process stat parse"))?;
        let executable = opened_at(&self.directory, "exe", 0)
            .map_err(|error| context(error, "Owned native main process executable open"))?
            .metadata()
            .map_err(|error| context(error, "Owned native main process executable metadata"))?;
        if start != self.receipt.start
            || group != self.receipt.group
            || session != self.receipt.session
            || matches!(state, 't' | 'Z' | 'X' | 'x')
            || executable_path(&self.directory)
                .map_err(|error| context(error, "Owned native main process executable link"))?
                != self.receipt.executable
            || executable.dev() != self.receipt.executable_device
            || executable.ino() != self.receipt.executable_inode
        {
            return Err(invalid(
                "Owned native process or executable identity changed",
            ));
        }
        Ok(())
    }

    fn threads(&self, reject_stopped: bool) -> io::Result<ThreadScan> {
        let directory = opened_at(
            &self.directory,
            "task",
            libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
        .map_err(|error| context(error, "Owned native task directory open"))?;
        let mut threads = Vec::new();
        threads
            .try_reserve_exact(MAX_THREADS)
            .map_err(io::Error::other)?;
        for entry in fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            .map_err(|error| context(error, "Owned native task directory scan"))?
        {
            if threads.len() == MAX_THREADS {
                return Err(invalid("Owned native thread count exceeds 1024"));
            }
            let tid = entry
                .map_err(|error| context(error, "Owned native task directory entry"))?
                .file_name()
                .to_str()
                .ok_or_else(|| invalid("Invalid owned native thread entry"))?
                .parse::<u32>()
                .map_err(io::Error::other)?;
            let current = match text_at(&directory, &format!("{tid}/status"), MAX_PROC_BYTES) {
                Ok(text) => text,
                Err(error) if tid != self.receipt.pid && vanished_thread(&error) => {
                    return Ok(ThreadScan::Invalidated(format!(
                        "Non-main owned native thread {tid} status read: {error}"
                    )));
                }
                Err(error) => {
                    return Err(context(
                        error,
                        format!("Owned native thread {tid} status read"),
                    ));
                }
            };
            let current = status(&current).map_err(|error| {
                context(error, format!("Owned native thread {tid} status parse"))
            })?;
            self.verify_status(&current, tid)?;
            if reject_stopped && current.state == 'T' {
                return Err(invalid(format!(
                    "Owned native client thread {tid} is already stopped"
                )));
            }
            if tid != self.receipt.pid && matches!(current.state, 'Z' | 'X' | 'x') {
                return Ok(ThreadScan::Invalidated(format!(
                    "Non-main owned native thread {tid} exited in status state {}",
                    current.state
                )));
            }
            let stat = match text_at(&directory, &format!("{tid}/stat"), MAX_PROC_BYTES) {
                Ok(text) => text,
                Err(error) if tid != self.receipt.pid && vanished_thread(&error) => {
                    return Ok(ThreadScan::Invalidated(format!(
                        "Non-main owned native thread {tid} stat read: {error}"
                    )));
                }
                Err(error) => {
                    return Err(context(
                        error,
                        format!("Owned native thread {tid} stat read"),
                    ));
                }
            };
            let (group, session, start, state) = stat_identity(&stat)
                .map_err(|error| context(error, format!("Owned native thread {tid} stat parse")))?;
            if group != self.receipt.group || session != self.receipt.session {
                return Err(invalid("Owned native thread changed group or session"));
            }
            if state == 't' || (tid == self.receipt.pid && matches!(state, 'Z' | 'X' | 'x')) {
                return Err(invalid(format!(
                    "Owned native thread {tid} stat tracing or main exit state changed"
                )));
            }
            if reject_stopped && state == 'T' {
                return Err(invalid(format!(
                    "Owned native client thread {tid} is already stopped"
                )));
            }
            if tid != self.receipt.pid && matches!(state, 'Z' | 'X' | 'x') {
                return Ok(ThreadScan::Invalidated(format!(
                    "Non-main owned native thread {tid} exited in stat state {state}"
                )));
            }
            threads.push(ThreadState { tid, start, state });
        }
        threads.sort_unstable_by_key(|thread| thread.tid);
        if threads.is_empty()
            || !threads.iter().any(|thread| thread.tid == self.receipt.pid)
            || threads.windows(2).any(|pair| pair[0].tid == pair[1].tid)
        {
            return Err(invalid("Incomplete owned native thread-group scan"));
        }
        Ok(ThreadScan::Complete(threads))
    }

    fn signal(&self, signal: i32) -> io::Result<()> {
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.directory.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0u32,
            )
        };
        if result != 0 {
            return Err(context(
                io::Error::last_os_error(),
                format!(
                    "Owned native pidfd signal {signal} for main PID {}",
                    self.receipt.pid
                ),
            ));
        }
        Ok(())
    }

    fn stop(&mut self, checkpoint: &mut dyn FnMut() -> io::Result<()>) -> io::Result<()> {
        let started = Instant::now();
        let mut invalidation = None;
        loop {
            checkpoint()?;
            self.verify_identity()?;
            let scan = self.threads(true)?;
            if let ThreadScan::Invalidated(reason) = &scan {
                invalidation = Some(reason.clone());
            }
            if started.elapsed() >= STOP_TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Owned native pre-stop scan did not complete within five seconds; last invalidation: {invalidation:?}"
                    ),
                ));
            }
            if matches!(scan, ThreadScan::Complete(_)) {
                break;
            }
            thread::sleep(POLL_INTERVAL);
        }
        self.verify_identity()?;
        self.signal(libc::SIGSTOP)?;
        self.armed = true;
        let mut previous = None;
        loop {
            checkpoint()?;
            self.verify_identity()?;
            let scan = self.threads(false)?;
            if let ThreadScan::Invalidated(reason) = &scan {
                invalidation = Some(reason.clone());
            }
            if started.elapsed() >= STOP_TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Owned native thread group did not stop within five seconds; last invalidation: {invalidation:?}"
                    ),
                ));
            }
            match scan {
                ThreadScan::Complete(current) => {
                    if current.iter().all(|thread| thread.state == 'T')
                        && previous.as_ref() == Some(&current)
                    {
                        self.receipt.stopped_threads = current.len();
                        self.stopped = current;
                        return Ok(());
                    }
                    previous = Some(current);
                }
                ThreadScan::Invalidated(_) => previous = None,
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn verify_stopped(&self) -> io::Result<()> {
        self.verify_identity()?;
        match self.threads(false)? {
            ThreadScan::Complete(current) if current == self.stopped => Ok(()),
            ThreadScan::Complete(_) => {
                Err(invalid("Owned native thread group changed during snapshot"))
            }
            ThreadScan::Invalidated(reason) => Err(invalid(format!(
                "Owned native stopped scan was incomplete: {reason}"
            ))),
        }
    }

    fn resume(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        let identity = self.verify_identity();
        let signal = self.signal(libc::SIGCONT);
        match (identity, signal) {
            (Ok(()), Ok(())) => {}
            (Err(identity), Err(signal)) => {
                return Err(invalid(format!(
                    "Identity before resume: {identity}; SIGCONT: {signal}"
                )));
            }
            (Err(error), _) | (_, Err(error)) => return Err(error),
        }
        let started = Instant::now();
        let mut invalidation = None;
        loop {
            self.verify_identity()?;
            let scan = self.threads(false)?;
            if let ThreadScan::Invalidated(reason) = &scan {
                invalidation = Some(reason.clone());
            }
            if started.elapsed() >= RESUME_TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Owned native thread group did not resume within three seconds; last invalidation: {invalidation:?}"
                    ),
                ));
            }
            if let ThreadScan::Complete(current) = scan
                && current
                    .iter()
                    .all(|thread| !matches!(thread.state, 'T' | 't'))
            {
                self.verify_identity()?;
                self.armed = false;
                return Ok(());
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn field<'a>(tail: &mut &'a str) -> io::Result<&'a str> {
    *tail = tail.trim_start_matches(char::is_whitespace);
    let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
    if end == 0 {
        return Err(invalid("Incomplete owned native memory mapping"));
    }
    let value = &tail[..end];
    *tail = &tail[end..];
    Ok(value)
}

fn hexadecimal(value: &str) -> io::Result<u64> {
    u64::from_str_radix(value, 16).map_err(io::Error::other)
}

fn mapping_path(value: &str) -> io::Result<Option<PathBuf>> {
    if value.is_empty() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(value.len())
        .map_err(io::Error::other)?;
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let digits = [input.next(), input.next(), input.next()];
        let mut decoded = 0u16;
        for digit in digits {
            let digit = digit
                .filter(|digit| matches!(digit, b'0'..=b'7'))
                .ok_or_else(|| invalid("Invalid owned native mapping path escape"))?;
            decoded = decoded * 8 + u16::from(digit - b'0');
        }
        bytes.push(u8::try_from(decoded).map_err(io::Error::other)?);
    }
    Ok(Some(PathBuf::from(OsString::from_vec(bytes))))
}

fn mappings(text: &str) -> io::Result<Vec<Mapping>> {
    let mut mappings = Vec::new();
    mappings
        .try_reserve_exact(MAX_MAPS)
        .map_err(io::Error::other)?;
    for mut line in text.lines() {
        if mappings.len() == MAX_MAPS {
            return Err(invalid("Owned native mapping count exceeds 16384"));
        }
        let (start, end) = field(&mut line)?
            .split_once('-')
            .ok_or_else(|| invalid("Invalid owned native mapping address"))?;
        let start = hexadecimal(start)?;
        let end = hexadecimal(end)?;
        let permissions = field(&mut line)?.as_bytes();
        if start >= end
            || permissions.len() != 4
            || !matches!(permissions[0], b'r' | b'-')
            || !matches!(permissions[1], b'w' | b'-')
            || !matches!(permissions[2], b'x' | b'-')
            || !matches!(permissions[3], b'p' | b's')
            || mappings
                .last()
                .is_some_and(|previous: &Mapping| previous.end > start)
        {
            return Err(invalid("Invalid owned native mapping range or permissions"));
        }
        let offset = hexadecimal(field(&mut line)?)?;
        let (major, minor) = field(&mut line)?
            .split_once(':')
            .ok_or_else(|| invalid("Invalid owned native mapping device"))?;
        let device_major = u32::try_from(hexadecimal(major)?).map_err(io::Error::other)?;
        let device_minor = u32::try_from(hexadecimal(minor)?).map_err(io::Error::other)?;
        let inode = field(&mut line)?.parse().map_err(io::Error::other)?;
        mappings.push(Mapping {
            start,
            end,
            offset,
            inode,
            device_major,
            device_minor,
            readable: permissions[0] == b'r',
            writable: permissions[1] == b'w',
            executable: permissions[2] == b'x',
            private: permissions[3] == b'p',
            path: mapping_path(line.trim_start_matches(char::is_whitespace))?,
        });
    }
    if mappings.is_empty() || !text.ends_with('\n') {
        return Err(invalid("Incomplete owned native mapping table"));
    }
    Ok(mappings)
}

impl<'a> StoppedProcess<'a> {
    fn open(
        lease: &'a Lease,
        checkpoint: &'a mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        lease.verify_stopped()?;
        let maps_text = text_at(&lease.directory, "maps", MAX_MAP_BYTES)?;
        let mappings = mappings(&maps_text)?;
        let memory = opened_at(&lease.directory, "mem", libc::O_NOFOLLOW)?;
        lease.verify_stopped()?;
        Ok(Self {
            lease,
            memory,
            mappings,
            maps_text,
            checkpoint,
            started: Instant::now(),
            read_bytes: 0,
        })
    }

    fn check(&mut self) -> io::Result<()> {
        (self.checkpoint)()?;
        if self.started.elapsed() >= SNAPSHOT_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Owned native snapshot exceeded five seconds",
            ));
        }
        Ok(())
    }

    fn verify(&mut self) -> io::Result<()> {
        self.check()?;
        self.lease.verify_stopped()?;
        if text_at(&self.lease.directory, "maps", MAX_MAP_BYTES)? != self.maps_text {
            return Err(invalid("Owned native mappings changed during snapshot"));
        }
        self.check()
    }

    pub fn receipt(&self) -> &ProcessReceipt {
        &self.lease.receipt
    }

    pub fn maps(&self) -> &[Mapping] {
        &self.mappings
    }

    pub fn checkpoint(&mut self) -> io::Result<()> {
        self.check()
    }

    pub fn open_namespace_file(&mut self, path: &Path) -> io::Result<File> {
        self.check()?;
        self.lease.verify_stopped()?;
        let mut canonical = PathBuf::from("/");
        let mut relative = PathBuf::new();
        for component in path.components() {
            match component {
                Component::RootDir if relative.as_os_str().is_empty() => {}
                Component::Normal(name) => {
                    canonical.push(name);
                    relative.push(name);
                }
                _ => {
                    return Err(invalid(
                        "Native namespace file needs an absolute canonical path",
                    ));
                }
            }
        }
        if !path.is_absolute()
            || relative.as_os_str().is_empty()
            || canonical.as_os_str() != path.as_os_str()
        {
            return Err(invalid(
                "Native namespace file needs an absolute canonical path",
            ));
        }
        let relative = relative
            .to_str()
            .ok_or_else(|| invalid("Native namespace file path is not UTF-8"))?;
        let file = opened_at(
            &self.lease.directory,
            &format!("root/{relative}"),
            libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )?;
        if !file.metadata()?.is_file() {
            return Err(invalid("Native namespace image is not a regular file"));
        }
        self.lease.verify_stopped()?;
        self.check()?;
        Ok(file)
    }

    pub fn require_range(&self, address: u64, length: usize, write: bool) -> io::Result<()> {
        let end = address
            .checked_add(u64::try_from(length).map_err(io::Error::other)?)
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or_else(|| invalid("Owned native memory range overflow"))?;
        if address == 0 || length == 0 {
            return Err(invalid("Empty or null owned native memory range"));
        }
        let mut cursor = address;
        for mapping in &self.mappings {
            if mapping.end <= cursor {
                continue;
            }
            if mapping.start > cursor
                || !mapping.readable
                || !mapping.private
                || (write && !mapping.writable)
            {
                break;
            }
            cursor = mapping.end.min(end);
            if cursor == end {
                return Ok(());
            }
        }
        Err(invalid(
            "Owned native memory range is not completely readable and private",
        ))
    }

    pub fn checked_array(
        &self,
        address: u64,
        count: usize,
        stride: usize,
        maximum_count: usize,
    ) -> io::Result<usize> {
        if count == 0 || count > maximum_count || stride == 0 {
            return Err(invalid(
                "Owned native array count or stride is out of bounds",
            ));
        }
        let length = count
            .checked_mul(stride)
            .ok_or_else(|| invalid("Owned native array byte length overflow"))?;
        self.require_range(address, length, true)?;
        Ok(length)
    }

    pub fn read_exact(&mut self, address: u64, length: usize) -> io::Result<Vec<u8>> {
        self.check()?;
        self.require_range(address, length, false)?;
        let total = self
            .read_bytes
            .checked_add(length)
            .filter(|total| *total <= MAX_TOTAL_READ_BYTES)
            .ok_or_else(|| invalid("Owned native snapshot exceeds its 64 MiB read budget"))?;
        if length > MAX_READ_BYTES {
            return Err(invalid("Owned native snapshot read exceeds one MiB"));
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(io::Error::other)?;
        bytes.resize(length, 0);
        self.read_bytes = total;
        let count = self.memory.read_at(&mut bytes, address)?;
        if count != length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("Owned native memory read was incomplete: {count} of {length} bytes"),
            ));
        }
        self.check()?;
        Ok(bytes)
    }

    pub fn read_u8(&mut self, address: u64) -> io::Result<u8> {
        Ok(self.read_exact(address, 1)?[0])
    }

    pub fn read_u32(&mut self, address: u64) -> io::Result<u32> {
        let bytes = self.read_exact(address, size_of::<u32>())?;
        Ok(u32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| invalid("Incomplete native u32"))?,
        ))
    }

    pub fn read_u64(&mut self, address: u64) -> io::Result<u64> {
        let bytes = self.read_exact(address, size_of::<u64>())?;
        Ok(u64::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| invalid("Incomplete native u64"))?,
        ))
    }

    pub fn read_c_string(&mut self, address: u64, maximum: usize) -> io::Result<String> {
        if maximum == 0 || maximum > MAX_PATH_BYTES {
            return Err(invalid("Owned native string length is out of bounds"));
        }
        let mut value = Vec::new();
        value.try_reserve_exact(maximum).map_err(io::Error::other)?;
        let mut cursor = address;
        while value.len() < maximum {
            let mapping = self
                .mappings
                .iter()
                .find(|mapping| mapping.start <= cursor && cursor < mapping.end)
                .ok_or_else(|| invalid("Owned native string crosses an unmapped range"))?;
            let length = (mapping.end - cursor).min((maximum - value.len()) as u64) as usize;
            let bytes = self.read_exact(cursor, length)?;
            if let Some(end) = bytes.iter().position(|byte| *byte == 0) {
                value.extend_from_slice(&bytes[..end]);
                return String::from_utf8(value).map_err(io::Error::other);
            }
            value.extend_from_slice(&bytes);
            cursor = cursor
                .checked_add(length as u64)
                .ok_or_else(|| invalid("Owned native string address overflow"))?;
        }
        Err(invalid(
            "Owned native string has no terminator within its bound",
        ))
    }
}

pub(super) fn with_stopped<T>(
    launch: &Launch,
    child: &Child,
    executable: &Path,
    expected: &Identity,
    checkpoint: &mut dyn FnMut() -> io::Result<()>,
    operation: impl FnOnce(&mut StoppedProcess<'_>) -> io::Result<T>,
) -> Result<T, CaptureFailure> {
    let mut lease =
        Lease::open(launch, child, executable, expected).map_err(|error| CaptureFailure {
            error,
            cleanup: CleanupDisposition::Confirmed,
        })?;
    let operation = catch_unwind(AssertUnwindSafe(|| {
        lease.stop(checkpoint)?;
        let mut process = StoppedProcess::open(&lease, checkpoint)?;
        let value = operation(&mut process)?;
        process.verify()?;
        Ok(value)
    }))
    .unwrap_or_else(|_| Err(invalid("Owned native snapshot operation panicked")));
    let resume = lease.resume();
    match (operation, resume) {
        (Ok(value), Ok(())) => {
            let checked = catch_unwind(AssertUnwindSafe(checkpoint))
                .unwrap_or_else(|_| Err(invalid("Owned native snapshot checkpoint panicked")));
            checked.map(|()| value).map_err(|error| CaptureFailure {
                error,
                cleanup: CleanupDisposition::Confirmed,
            })
        }
        (Err(error), Ok(())) => Err(CaptureFailure {
            error,
            cleanup: CleanupDisposition::Confirmed,
        }),
        (operation, Err(error)) => Err(CaptureFailure {
            error: invalid(match operation {
                Ok(_) => format!("Owned native snapshot resume failed: {error}"),
                Err(operation) => {
                    format!("{operation}; owned native snapshot resume failed: {error}")
                }
            }),
            cleanup: CleanupDisposition::Unconfirmed,
        }),
    }
}
