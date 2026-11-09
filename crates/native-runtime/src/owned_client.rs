use std::fs::{self, File};
use std::io;
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use source_formats::bsp::{Bsp, PakArchive, PakLimits, lump};

use crate::isolation::{Identity, Launch};
use crate::process::WorkerRequest;
use crate::util::{self, invalid};
use crate::{CaptureFailure, CleanupDisposition};

const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn parse_bsp(
    bytes: &[u8],
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Bsp> {
    if bytes.len() > util::MAX_BSP {
        return Err(invalid("Native observation BSP exceeds 128 MiB"));
    }
    let bsp = Bsp::parse_bounded(bytes, 96 * 1024 * 1024, 4096)?;
    crate::adapter::validate_private_entities(bsp.entities()?)?;
    let archive = &bsp.lump(lump::PAKFILE).data;
    if !archive.is_empty() {
        let archive = PakArchive::parse_controlled(archive, PakLimits::default(), check)?;
        if archive.entries.iter().any(|entry| {
            let name = entry.name.to_ascii_lowercase();
            name.ends_with(".lmp")
                || !["materials/", "models/", "sound/", "particles/"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        }) {
            return Err(invalid(
                "Native observation BSP PAK contains alternate lump, script or configuration providers",
            ));
        }
    }
    Ok(bsp)
}

pub(crate) struct Client<'a> {
    pub launch: &'a Launch,
    pub child: &'a Child,
    pub executable: &'a Path,
    pub identity: &'a Identity,
    pub console: &'a str,
}

pub(crate) fn checkpoint(directory: &Path, request: &WorkerRequest) -> io::Result<()> {
    if crate::worker::cancelled(directory, request) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Native observation cancelled or caller exited",
        ));
    }
    Ok(())
}

fn launch(
    directory: &Path,
    request: &WorkerRequest,
    game: &Path,
    profile: &str,
) -> io::Result<(Launch, PathBuf)> {
    let root = request.environment.engine_root.canonicalize()?;
    for name in ["config", "cache", "data"] {
        fs::create_dir_all(directory.join("settings").join(name))?;
    }
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let port = socket.local_addr()?.port();
    drop(socket);
    let executable = root.join("cstrike_linux64").canonicalize()?;
    let mut launch = Launch::new(directory, &executable)?;
    let log = File::create(directory.join("launcher.log"))?;
    let command = &mut launch.command;
    command
        .current_dir(&root)
        .arg("-game")
        .arg(game)
        .args([
            "-windowed",
            "-w",
            "1280",
            "-h",
            "720",
            "-novid",
            "-nomaster",
            "-nojoy",
            "-console",
            "-port",
        ])
        .arg(port.to_string());
    crate::worker::private_launch_options(command);
    command
        .env("LD_LIBRARY_PATH", root.join("bin/linux64"))
        .env("LD_DEBUG", "files")
        .env("LD_DEBUG_OUTPUT", directory.join("loader"))
        .env("SteamAppId", "240")
        .env("SteamGameId", "240")
        .env("SDL_VIDEODRIVER", "x11")
        .env("XDG_CONFIG_HOME", directory.join("settings/config"))
        .env("XDG_CACHE_HOME", directory.join("settings/cache"))
        .env("XDG_DATA_HOME", directory.join("settings/data"))
        .env("HAMMERR_PLAYER_SUPPORT_DIRECTORY", directory)
        .env_remove("LD_PRELOAD")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    util::json_write(
        &directory.join("command.json"),
        &json!({"program":command.get_program().to_string_lossy(),"args":command.get_args().map(|argument|argument.to_string_lossy().into_owned()).collect::<Vec<_>>(),"native_program":executable,"cwd":root,"desktop_input":false,"profile":profile,"filesystem_isolation":{"external_paths":"read_only","writable_directory":directory}}),
    )?;
    crate::guard::ensure_no_client(&root)?;
    checkpoint(directory, request)?;
    Ok((launch, executable))
}

pub(crate) fn run<T>(
    directory: &Path,
    request: &WorkerRequest,
    game: &Path,
    console_file: &str,
    profile: &str,
    cleanup_file: &str,
    mut poll: impl FnMut(Client<'_>) -> Result<Option<T>, CaptureFailure>,
) -> io::Result<T> {
    let root = request.environment.engine_root.canonicalize()?;
    let (mut launch, executable) = launch(directory, request, game, profile)?;
    let mut child = launch.spawn()?;
    let started = Instant::now();
    let mut native = None;
    let mut cleanup_disposition = CleanupDisposition::Confirmed;
    let result = (|| {
        loop {
            checkpoint(directory, request)?;
            crate::worker::loader_budget(directory)?;
            util::private_budget(directory, &root)?;
            if fs::metadata(directory.join("launcher.log"))?.len() > 8 * 1024 * 1024 {
                return Err(invalid("Native observer launcher log exceeds eight MiB"));
            }
            if let Some(identity) = launch.identity(&child, &executable)? {
                if native.as_ref().is_some_and(|previous: &Identity| {
                    previous.pid != identity.pid || previous.start != identity.start
                }) {
                    return Err(invalid("Native isolated process identity changed"));
                }
                if native.is_none() {
                    util::json_write(&directory.join("native-process.json"), &identity)?;
                    native = Some(identity);
                }
            } else if native.is_some() {
                return Err(invalid("Native isolated process exited before observation"));
            }
            let text = match util::diagnostic_text(&game.join(console_file), util::MAX_LOG) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error),
            };
            crate::worker::verify_native_diagnostics(&text)?;
            if let Some(identity) = &native {
                let output = poll(Client {
                    launch: &launch,
                    child: &child,
                    executable: &executable,
                    identity,
                    console: &text,
                })
                .map_err(|failure| {
                    cleanup_disposition = failure.cleanup;
                    failure.error
                })?;
                if let Some(output) = output {
                    return Ok(output);
                }
            }
            if let Some(status) = child.try_wait()? {
                return Err(invalid(format!(
                    "Native client exited before observation: {status}"
                )));
            }
            if started.elapsed() >= LOAD_TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native observation exceeded its 120-second deadline",
                ));
            }
            thread::sleep(POLL_INTERVAL);
        }
    })();
    let cleanup = crate::worker::stop_client_group(&mut child);
    let evidence = util::json_write(
        &directory.join(cleanup_file),
        &json!({"snapshot_cleanup":cleanup_disposition,"client_group_exited":cleanup.is_ok(),"client_group_error":cleanup.as_ref().err().map(ToString::to_string),"snapshot_error":result.as_ref().err().map(ToString::to_string)}),
    );
    match (result, cleanup, evidence) {
        (Ok(output), Ok(()), Ok(())) => Ok(output),
        (result, cleanup, evidence) => {
            let failures = [
                result.err().map(|error| error.to_string()),
                cleanup
                    .err()
                    .map(|error| format!("Native client cleanup: {error}")),
                evidence
                    .err()
                    .map(|error| format!("Native cleanup evidence: {error}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            Err(invalid(failures.join("; ")))
        }
    }
}
