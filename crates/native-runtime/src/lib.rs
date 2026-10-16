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
