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

pub struct CaptureRequest {
    pub environment: CaptureEnvironment,
    pub bsp: Vec<u8>,
    pub map_name: String,
    pub samples: Vec<CaptureSample>,
}

pub struct CaptureReceipt {
    pub bsp: Vec<u8>,
    pub staged_bsp: Vec<u8>,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
    pub preservation_passed: bool,
}

pub trait CaptureObserver {
    fn checkpoint(&mut self) -> io::Result<()>;
    fn sample_progress(&mut self, current: usize, total: usize);
}

mod adapter;
mod guard;
mod process;
mod util;
mod vdf;
mod worker;

pub use adapter::{capture_staging_bsp, preflight_vmf};
pub use guard::{resolve_environment, validate_environment};
pub use process::capture;

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
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Native runtime expects an owned capture-worker or read-only Steam-identity request",
        )),
    }
}

pub fn fingerprint(path: &std::path::Path) -> io::Result<String> {
    util::hash_file(path, util::MAX_BSP)
}
