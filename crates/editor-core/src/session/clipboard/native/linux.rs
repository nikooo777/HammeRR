use std::{
    fmt::Display,
    io::{ErrorKind, Read},
    os::fd::AsFd,
    time::Instant,
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
};
use wl_clipboard_rs::{
    copy::{self, MimeType, Options, Source},
    paste::{self, ClipboardType, Seat},
};

use map_core::transfer::MAX_BYTES;

mod x11;

pub(super) fn wayland_configured() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("WAYLAND_SOCKET").is_some()
}

fn wayland_error(error: impl Display) -> String {
    format!("Cannot read configured Wayland clipboard: {error}")
}

pub(super) fn write_wayland(text: &str) -> Result<(), String> {
    copy::copy(
        Options::new(),
        Source::Bytes(text.as_bytes().into()),
        MimeType::Text,
    )
    .map_err(|error| format!("Cannot publish to configured Wayland clipboard: {error}"))
}

pub(super) fn read(deadline: Instant) -> Result<String, String> {
    if wayland_configured() {
        read_wayland(deadline)
    } else if std::env::var_os("DISPLAY").is_some() {
        x11::read(deadline)
    } else {
        Err("System clipboard requires a configured Wayland or X11 display".into())
    }
}

pub(super) fn wait_readable(fd: &impl AsFd, deadline: Instant) -> Result<(), String> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("System clipboard read timed out after 5 seconds")?;
        let timeout =
            Timespec::try_from(remaining).map_err(|_| "Invalid system clipboard read timeout")?;
        let mut descriptors = [PollFd::new(fd, PollFlags::IN)];
        let result = poll(&mut descriptors, Some(&timeout));
        match result {
            Ok(0) => return Err("System clipboard read timed out after 5 seconds".into()),
            Ok(_) if descriptors[0].revents().contains(PollFlags::NVAL) => {
                return Err("System clipboard connection closed while reading".into());
            }
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(format!("Cannot wait for system clipboard data: {error}")),
        }
    }
}

fn read_wayland(deadline: Instant) -> Result<String, String> {
    let (mut pipe, _) = paste::get_contents(
        ClipboardType::Regular,
        Seat::Unspecified,
        paste::MimeType::Text,
    )
    .map_err(wayland_error)?;
    let flags = fcntl_getfl(&pipe).map_err(wayland_error)?;
    fcntl_setfl(&pipe, flags | OFlags::NONBLOCK).map_err(wayland_error)?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        wait_readable(&pipe, deadline)?;
        let capacity = buffer.len().min(MAX_BYTES - bytes.len() + 1);
        match pipe.read(&mut buffer[..capacity]) {
            Ok(0) => break,
            Ok(count) => {
                if count > MAX_BYTES - bytes.len() {
                    return Err("Wayland clipboard text exceeds 8 MiB".into());
                }
                bytes
                    .try_reserve_exact(count)
                    .map_err(|_| "Cannot allocate bounded Wayland clipboard text")?;
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(wayland_error(error)),
        }
    }
    String::from_utf8(bytes).map_err(|_| "Wayland clipboard text is not UTF-8".into())
}
