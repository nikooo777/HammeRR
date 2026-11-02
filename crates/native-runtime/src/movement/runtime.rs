use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::json;

use super::{MovementProfile, parse, verify_server};
use crate::process::WorkerRequest;
use crate::util::{self, invalid};

pub(super) fn verify_log(text: &str) -> io::Result<()> {
    crate::worker::verify_native_diagnostics(text)?;
    for marker in [
        "HAMMERR_MOVEMENT_ERROR",
        "SCRIPT ERROR",
        "AN ERROR HAS OCCURRED",
        "Failed to execute script",
        "Error running script",
        "Failed to create script",
        "VScript is not available",
        "unhandled input",
        "Unhandled input",
    ] {
        if text.contains(marker) {
            return Err(invalid(format!("Native movement script failed: {marker}")));
        }
    }
    Ok(())
}

fn verify_controls(text: &str) -> io::Result<()> {
    let pattern = Regex::new(r#""sv_lan" = "([^"]+)""#).map_err(io::Error::other)?;
    let values = pattern
        .captures_iter(text)
        .map(|capture| capture[1].parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;
    if values.len() < 2 || values.iter().any(|value| *value != 1.0) {
        return Err(invalid(
            "Native movement did not prove sv_lan=1 before and after map load",
        ));
    }
    Ok(())
}

fn verify_modules(directory: &Path, root: &Path, pid: u32) -> io::Result<()> {
    let log = util::diagnostic_text(&directory.join(format!("loader.{pid}")), 8 * 1024 * 1024)?;
    let pattern = Regex::new(r"file=(.+?) \[\d+\];\s+generating link map|calling init:\s*(.+)$")
        .map_err(io::Error::other)?;
    let mut found = BTreeSet::new();
    for line in log.lines() {
        let Some(captures) = pattern.captures(line) else {
            continue;
        };
        let Some(value) = captures.get(1).or_else(|| captures.get(2)) else {
            continue;
        };
        let path = Path::new(value.as_str().trim());
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let expected = match name {
            "engine.so" => root.join("bin/linux64/engine.so"),
            "server.so" => root.join("cstrike/bin/linux64/server.so"),
            _ => continue,
        }
        .canonicalize()?;
        let actual = if path.is_absolute() {
            path.to_owned()
        } else {
            root.join(path)
        }
        .canonicalize()?;
        if actual != expected {
            return Err(invalid(format!(
                "Native movement loaded {name} from an unexpected path"
            )));
        }
        found.insert(name.to_owned());
    }
    if found.len() != 2 {
        return Err(invalid(
            "Native movement did not positively observe the pinned engine and server modules",
        ));
    }
    util::json_write(
        &directory.join("movement-modules.json"),
        &json!({"native_pid":pid,"loaded":found,"server_sha256":super::SERVER_SHA}),
    )
}

pub(crate) fn execute(
    directory: &Path,
    request: &WorkerRequest,
    profile: &MovementProfile,
) -> io::Result<()> {
    profile.validate()?;
    let root = request.environment.engine_root.canonicalize()?;
    verify_server(&root)?;
    if util::hash_file(&directory.join("staged.bsp"), util::MAX_BSP)? != profile.staged_bsp_sha256 {
        return Err(invalid(
            "Movement staged BSP identity changed before launch",
        ));
    }
    let game = crate::worker::game_directory(directory, request)?;
    for name in ["config", "cache", "data"] {
        fs::create_dir_all(directory.join("settings").join(name))?;
    }
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let port = socket.local_addr()?.port();
    drop(socket);
    let executable = root.join("cstrike_linux64").canonicalize()?;
    let mut launch = crate::isolation::Launch::new(directory, &executable)?;
    let log = File::create(directory.join("launcher.log"))?;
    let command = &mut launch.command;
    command
        .current_dir(&root)
        .arg("-game")
        .arg(&game)
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
        .env_remove("LD_PRELOAD")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    util::json_write(
        &directory.join("command.json"),
        &json!({"program":command.get_program().to_string_lossy(),"args":command.get_args().map(|arg|arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),"native_program":executable,"cwd":root,"desktop_input":false,"profile":"movement","filesystem_isolation":{"external_paths":"read_only","writable_directory":directory}}),
    )?;
    crate::guard::ensure_no_client(&root)?;
    if crate::worker::cancelled(directory, request) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Native movement cancelled before launch",
        ));
    }
    let mut child = launch.spawn()?;
    let started = Instant::now();
    let console = game.join("hammerr-movement.log");
    let mut native = None;
    let result = (|| {
        loop {
            if crate::worker::cancelled(directory, request) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Native movement cancelled or caller exited",
                ));
            }
            if let Some(identity) = launch.identity(&child, &executable)? {
                if native
                    .as_ref()
                    .is_some_and(|previous: &crate::isolation::Identity| {
                        previous.pid != identity.pid || previous.start != identity.start
                    })
                {
                    return Err(invalid("Native movement isolated process identity changed"));
                }
                if native.is_none() {
                    util::json_write(&directory.join("native-process.json"), &identity)?;
                    native = Some(identity);
                }
            } else if native.is_some() {
                return Err(invalid(
                    "Native movement isolated process exited before observation completed",
                ));
            }
            let text = if console.exists() {
                util::diagnostic_text(&console, util::MAX_LOG)?
            } else {
                String::new()
            };
            verify_log(&text)?;
            crate::worker::loader_budget(directory)?;
            util::private_budget(directory, &root)?;
            if fs::metadata(directory.join("launcher.log"))?.len() > 8 * 1024 * 1024 {
                return Err(invalid("Native movement launcher log exceeds 8 MiB"));
            }
            let count = text
                .lines()
                .filter(|line| line.trim().starts_with("HAMMERR_MOVEMENT_SAMPLE "))
                .count();
            let total = profile.phases().len() * usize::from(profile.samples_per_phase);
            if count > total {
                return Err(invalid(
                    "Native movement sample stream exceeds its finite request",
                ));
            }
            util::json_write(&directory.join("progress.json"), &[count, total])?;
            if text
                .lines()
                .any(|line| line.trim().starts_with("HAMMERR_MOVEMENT_COMPLETE "))
            {
                let native = native
                    .as_ref()
                    .ok_or_else(|| invalid("Native movement has no isolated process identity"))?;
                verify_controls(&text)?;
                let observation = parse(&text, profile)?;
                verify_modules(directory, &root, native.pid)?;
                verify_server(&root)?;
                util::json_write(&directory.join("movement-observation.json"), &observation)?;
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                return Err(invalid(format!(
                    "Native movement exited before its finite sample stream completed: {status}"
                )));
            }
            if started.elapsed() > Duration::from_secs(120) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native movement exceeded its 120-second launch/observation deadline; inspect the console for missing entities or script initialization failure",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    let cleanup = crate::worker::stop_client_group(&mut child);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Err(cleanup)) => Err(invalid(format!(
            "{error}; native movement cleanup: {cleanup}"
        ))),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}
