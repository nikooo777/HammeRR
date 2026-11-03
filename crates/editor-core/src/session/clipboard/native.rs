use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use super::Transport;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

const READ_TIMEOUT: Duration = Duration::from_secs(5);
static READING: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
pub(super) struct Native {
    clipboard: Option<arboard::Clipboard>,
}

struct ReadPermit;

impl Drop for ReadPermit {
    fn drop(&mut self) {
        READING.store(false, Ordering::Release);
    }
}

impl Native {
    fn open(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.clipboard.is_none() {
            self.clipboard = Some(
                arboard::Clipboard::new()
                    .map_err(|error| format!("System clipboard unavailable: {error}"))?,
            );
        }
        Ok(self.clipboard.as_mut().unwrap())
    }
}

fn read_worker(sender: mpsc::SyncSender<Result<String, String>>, deadline: Instant) {
    let permit = ReadPermit;
    let result = read_native(deadline);
    drop(permit);
    let _ = sender.send(result);
}

#[cfg(target_os = "linux")]
fn read_native(deadline: Instant) -> Result<String, String> {
    linux::read(deadline)
}

#[cfg(windows)]
fn read_native(_deadline: Instant) -> Result<String, String> {
    windows::read()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn read_native(_deadline: Instant) -> Result<String, String> {
    Err("Bounded system clipboard reads are supported on Linux and Windows".into())
}

impl Transport for Native {
    fn read(&mut self) -> Result<String, String> {
        READING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "A system clipboard read is still in progress; retry after it finishes")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let deadline = Instant::now() + READ_TIMEOUT;
        let worker = std::thread::Builder::new()
            .name("clipboard-read".into())
            .spawn(move || read_worker(sender, deadline));
        if let Err(error) = worker {
            READING.store(false, Ordering::Release);
            return Err(format!("Cannot start system clipboard read: {error}"));
        }
        receiver
            .recv_timeout(READ_TIMEOUT)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    String::from("System clipboard read timed out after 5 seconds")
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    String::from("System clipboard read stopped unexpectedly")
                }
            })?
    }

    fn write(&mut self, text: &str) -> Result<(), String> {
        if text.len() > map_core::transfer::MAX_BYTES {
            return Err("System clipboard text exceeds 8 MiB".into());
        }
        #[cfg(target_os = "linux")]
        if linux::wayland_configured() {
            return linux::write_wayland(text);
        }
        self.open()?
            .set_text(text)
            .map_err(|error| format!("Cannot publish system clipboard text: {error}"))
    }
}
