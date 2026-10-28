use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use serde::{Deserialize, Serialize};

use crate::util::{self, invalid};

const BWRAP: &str = "/usr/bin/bwrap";
const INFO_FD: i32 = 199;

pub(crate) fn preflight() -> io::Result<PathBuf> {
    let path = Path::new(BWRAP).canonicalize().map_err(|error| {
        io::Error::new(error.kind(), format!("Native capture requires bubblewrap at {BWRAP} for read-only source isolation: {error}"))
    })?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(invalid(
            "Native capture requires an executable bubblewrap for read-only source isolation",
        ));
    }
    Ok(path)
}

#[derive(Deserialize)]
struct Info {
    #[serde(rename = "child-pid")]
    child_pid: u32,
}

#[derive(Serialize)]
pub(crate) struct Identity {
    pub pid: u32,
    pub start: String,
    pub group: u32,
    pub session: u32,
    pub uid: u32,
}

pub(crate) struct Launch {
    pub command: Command,
    info: File,
    info_path: PathBuf,
}

impl Launch {
    pub fn new(directory: &Path, program: &Path) -> io::Result<Self> {
        Self::configured(directory, program, false)
    }

    pub fn ppl_uv857(directory: &Path, program: &Path) -> io::Result<Self> {
        Self::configured(directory, program, true)
    }

    fn configured(directory: &Path, program: &Path, diagnostic: bool) -> io::Result<Self> {
        if directory.canonicalize()? != directory {
            return Err(invalid(
                "Native isolation requires its canonical owned capture directory",
            ));
        }
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid(
                "Native isolation requires a private owned capture directory",
            ));
        }
        let scratch = directory.join("tmp");
        fs::create_dir(&scratch)?;
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700))?;
        let info_path = directory.join("isolation-info.json");
        let info = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&info_path)?;
        let mut command = Command::new(preflight()?);
        command
            .env("TMPDIR", &scratch)
            .args([
                "--die-with-parent",
                "--ro-bind",
                "/",
                "/",
                "--dev-bind",
                "/dev",
                "/dev",
                "--proc",
                "/proc",
                "--bind",
            ])
            .arg(directory)
            .arg(directory)
            .args(["--info-fd", &INFO_FD.to_string()]);
        if diagnostic {
            crate::diagnostic::observer_arguments(&mut command, directory, program)?;
        }
        command.arg("--").arg(program);
        Ok(Self {
            command,
            info,
            info_path,
        })
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        let info = self.info.as_raw_fd();
        unsafe {
            self.command.pre_exec(move || {
                if libc::dup2(info, INFO_FD) < 0 || libc::fcntl(INFO_FD, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        self.command.spawn().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Cannot start mandatory native filesystem isolation: {error}"),
            )
        })
    }

    pub fn identity(&self, wrapper: &Child, program: &Path) -> io::Result<Option<Identity>> {
        let bytes = util::read(&self.info_path, 16 * 1024)?;
        let info: Info = match serde_json::from_slice(&bytes) {
            Ok(info) => info,
            Err(error) if error.is_eof() => return Ok(None),
            Err(error) => {
                return Err(invalid(format!(
                    "Invalid native isolation process report: {error}"
                )));
            }
        };
        if info.child_pid < 2 || info.child_pid > i32::MAX as u32 || info.child_pid == wrapper.id()
        {
            return Err(invalid(
                "Native isolation returned an invalid host child PID",
            ));
        }
        let process = PathBuf::from(format!("/proc/{}", info.child_pid));
        let executable = match fs::read_link(process.join("exe")) {
            Ok(path) => path,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if executable != program.canonicalize()? {
            return Ok(None);
        }
        let pid = info.child_pid as i32;
        let group = unsafe { libc::getpgid(pid) };
        let session = unsafe { libc::getsid(pid) };
        let uid = fs::metadata(&process)?.uid();
        if group != wrapper.id() as i32
            || session != unsafe { libc::getsid(0) }
            || uid != unsafe { libc::geteuid() }
        {
            return Err(invalid(
                "Native isolation child changed its owned process group, session or user",
            ));
        }
        Ok(Some(Identity {
            pid: info.child_pid,
            start: crate::process::start_time(info.child_pid)?,
            group: group as u32,
            session: session as u32,
            uid,
        }))
    }
}
