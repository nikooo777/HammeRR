use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};

pub(in crate::mcp) type ProcessRead = Pin<Box<dyn AsyncRead + Send>>;
pub(in crate::mcp) type ProcessWrite = Pin<Box<dyn AsyncWrite + Send>>;

pub(in crate::mcp) struct ProcessGuard {
    #[cfg(unix)]
    flags: Vec<Flags>,
}

#[cfg(unix)]
struct Flags {
    fd: Option<std::os::fd::OwnedFd>,
    flags: rustix::fs::OFlags,
}

#[cfg(unix)]
impl Flags {
    fn restore(&mut self) -> Result<(), String> {
        if let Some(fd) = self.fd.take() {
            rustix::fs::fcntl_setfl(&fd, self.flags)
                .map_err(|error| format!("Could not restore MCP standard stream flags: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for Flags {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("{error}");
        }
    }
}

impl ProcessGuard {
    pub(in crate::mcp) fn finish(self) -> Result<(), String> {
        #[cfg(unix)]
        {
            let mut guard = self;
            let errors: Vec<_> = guard
                .flags
                .iter_mut()
                .filter_map(|flags| flags.restore().err())
                .collect();
            if !errors.is_empty() {
                return Err(errors.join("; "));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn preserve(fd: &std::os::fd::OwnedFd, guard: &mut ProcessGuard) -> Result<(), String> {
    guard.flags.push(Flags {
        fd: Some(fd.try_clone().map_err(|error| error.to_string())?),
        flags: rustix::fs::fcntl_getfl(fd).map_err(|error| error.to_string())?,
    });
    Ok(())
}

#[cfg(unix)]
fn from_owned_fds(
    read: std::os::fd::OwnedFd,
    write: std::os::fd::OwnedFd,
) -> Result<(ProcessRead, ProcessWrite, ProcessGuard), String> {
    use rustix::fs::{FileType, fstat};
    let mut guard = ProcessGuard { flags: Vec::new() };
    let read_kind =
        FileType::from_raw_mode(fstat(&read).map_err(|error| error.to_string())?.st_mode);
    let write_kind =
        FileType::from_raw_mode(fstat(&write).map_err(|error| error.to_string())?.st_mode);
    if read_kind == FileType::Fifo {
        preserve(&read, &mut guard)?;
    }
    if write_kind == FileType::Fifo {
        preserve(&write, &mut guard)?;
    }
    let read: ProcessRead = match read_kind {
        FileType::RegularFile => Box::pin(tokio::fs::File::from_std(std::fs::File::from(read))),
        FileType::Fifo => {
            Box::pin(tokio::net::unix::pipe::Receiver::from_owned_fd(read)
                .map_err(|error| format!("Cannot use MCP stdin pipe: {error}"))?)
        }
        _ => return Err("MCP stdin must be a readable pipe or regular file; terminals, sockets and devices are unsupported".into()),
    };
    let write: ProcessWrite = match write_kind {
        FileType::RegularFile => Box::pin(tokio::fs::File::from_std(std::fs::File::from(write))),
        FileType::Fifo => {
            Box::pin(tokio::net::unix::pipe::Sender::from_owned_fd(write)
                .map_err(|error| format!("Cannot use MCP stdout pipe: {error}"))?)
        }
        _ => return Err("MCP stdout must be a writable pipe or regular file; terminals, sockets and devices are unsupported".into()),
    };
    Ok((read, write, guard))
}

#[cfg(unix)]
pub(in crate::mcp) fn process_transport()
-> Result<(ProcessRead, ProcessWrite, ProcessGuard), String> {
    use std::os::fd::AsFd;
    let read = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("Could not own MCP stdin: {error}"))?;
    let write = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("Could not own MCP stdout: {error}"))?;
    from_owned_fds(read, write)
}

#[cfg(not(unix))]
pub(in crate::mcp) fn process_transport()
-> Result<(ProcessRead, ProcessWrite, ProcessGuard), String> {
    Err("Owned cancellable MCP standard streams require Unix pipes or regular files".into())
}

#[cfg(all(test, unix))]
mod tests;
