use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentMount {
    pub ids: Vec<String>,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureEnvironment {
    pub engine_root: PathBuf,
    pub steam_root: PathBuf,
    pub mounts: Vec<ContentMount>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSample {
    pub origin: [i32; 3],
    pub dimension: u16,
    pub texture: String,
    pub bootstrap_sha256: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureChannel {
    #[default]
    Ldr,
    Hdr,
}

impl CaptureChannel {
    pub const fn hdr_level(self) -> u8 {
        match self {
            Self::Ldr => 0,
            Self::Hdr => 2,
        }
    }

    pub const fn texture_suffix(self) -> &'static str {
        match self {
            Self::Ldr => ".vtf",
            Self::Hdr => ".hdr.vtf",
        }
    }
}

pub struct CaptureRequest {
    pub channel: CaptureChannel,
    pub environment: CaptureEnvironment,
    pub bsp: Vec<u8>,
    pub map_name: String,
    pub samples: Vec<CaptureSample>,
}

pub struct CaptureReceipt {
    pub channel: CaptureChannel,
    pub bsp: Vec<u8>,
    pub staged_bsp: Vec<u8>,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
    pub preservation_passed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupDisposition {
    Confirmed,
    Unconfirmed,
}

#[derive(Debug)]
pub struct CaptureFailure {
    pub error: io::Error,
    pub cleanup: CleanupDisposition,
}

impl std::fmt::Display for CaptureFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, formatter)
    }
}

impl std::error::Error for CaptureFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

pub trait CaptureObserver {
    fn checkpoint(&mut self) -> io::Result<()>;
    fn sample_progress(&mut self, current: usize, total: usize);
}

mod adapter;
mod diagnostic;
mod guard;
mod isolation;
mod process;
mod util;
mod vdf;
mod worker;

pub use adapter::{capture_staging_bsp, capture_staging_bsp_for_channel, preflight_vmf};
pub use diagnostic::{PplMidrange909Case, PplMidrange909Receipt, PplMidrange909Request};
pub use diagnostic::{PplUv857Case, PplUv857Receipt, PplUv857Request};
pub use guard::{resolve_environment, validate_environment};
pub use process::{capture, capture_with_cleanup};
pub use process::{diagnose_ppl_midrange909_with_cleanup, diagnose_ppl_uv857_with_cleanup};

pub fn worker_entry(arguments: Vec<std::ffi::OsString>) -> io::Result<()> {
    match arguments.as_slice() {
        [mode, root, steam] if mode == "--steam-identity" => {
            let identity =
                guard::identity_worker(std::path::Path::new(root), std::path::Path::new(steam))?;
            println!(
                "HAMMERR_STEAM_IDENTITY {}",
                serde_json::to_string(&identity).map_err(io::Error::other)?
            );
            Ok(())
        }
        [mode, directory] if mode == "--capture-supervisor" => {
            process::supervise(std::path::Path::new(directory))
        }
        [mode, directory] if mode == "--capture-worker" => {
            worker::run(std::path::Path::new(directory))
        }
        [mode, request] if mode == "--diagnose-ppl-uv857" => {
            diagnostic::cli(std::path::Path::new(request))
        }
        [mode, request] if mode == "--diagnose-ppl-midrange909" => {
            diagnostic::cli_midrange(std::path::Path::new(request))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Native runtime expects an owned worker, read-only Steam identity, or frozen857 diagnostic request",
        )),
    }
}

pub fn fingerprint(path: &std::path::Path) -> io::Result<String> {
    util::hash_file(path, util::MAX_BSP)
}
