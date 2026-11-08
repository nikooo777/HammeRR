use std::ffi::OsString;
use std::io;

use crate::{
    CaptureEnvironment, CaptureFailure, CaptureObserver, CaptureReceipt, CaptureRequest,
    CleanupDisposition,
};

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Native reflection capture requires the Linux Steam client, Cage, grim and Bubblewrap; editing, rendering and compilation without generated reflections remain available",
    )
}

pub fn resolve_environment(_environment: &CaptureEnvironment) -> io::Result<CaptureEnvironment> {
    Err(unsupported())
}

pub fn validate_environment(_environment: &CaptureEnvironment) -> io::Result<()> {
    Err(unsupported())
}

pub fn capture(
    request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> io::Result<CaptureReceipt> {
    capture_with_cleanup(request, observer).map_err(|failure| failure.error)
}

pub fn capture_with_cleanup(
    _request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> Result<CaptureReceipt, CaptureFailure> {
    let error = observer.checkpoint().err().unwrap_or_else(unsupported);
    Err(CaptureFailure {
        error,
        cleanup: CleanupDisposition::Confirmed,
    })
}

pub fn worker_entry(_arguments: Vec<OsString>) -> io::Result<()> {
    Err(unsupported())
}

pub fn observe_movement_with_cleanup(
    _request: crate::MovementRequest,
    observer: &mut dyn CaptureObserver,
) -> Result<crate::MovementReceipt, CaptureFailure> {
    let result = observer.checkpoint();
    let error = match result {
        Err(error) => error,
        Ok(()) => io::Error::new(
            io::ErrorKind::Unsupported,
            "Native movement observation requires the verified Linux64 CS:S client and private Cage/Bubblewrap runtime",
        ),
    };
    Err(CaptureFailure {
        error,
        cleanup: CleanupDisposition::Confirmed,
    })
}

pub fn observe_terrain_snapshot_with_cleanup(
    _request: crate::TerrainSnapshotRequest,
    observer: &mut dyn CaptureObserver,
) -> Result<crate::TerrainSnapshotReceipt, CaptureFailure> {
    let error = match observer.checkpoint() {
        Err(error) => error,
        Ok(()) => io::Error::new(
            io::ErrorKind::Unsupported,
            "Native terrain inspection requires the verified Linux64 CS:S client and private Cage/Bubblewrap runtime",
        ),
    };
    Err(CaptureFailure {
        error,
        cleanup: CleanupDisposition::Confirmed,
    })
}
