use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::json;

use crate::guard::{self, Baseline};
use crate::process::{WorkerProfile, WorkerRequest, WorkerResult};
use crate::util::{self, invalid};

const PREMAP: &str = "HAMMERR_PRIVATE_PREMAP_READY";
const READY: &str = "HAMMERR_CUBEMAP_READY";
const BEGIN: &str = "HAMMERR_CUBEMAP_CAPTURE_BEGIN";
const RETURNED: &str = "HAMMERR_CUBEMAP_CAPTURE_RETURNED";
const DISCONNECTED: &str = "HAMMERR_CUBEMAP_DISCONNECTED";
const PRIVATE_PROFILE: &str = "hammerr_private_launch.cfg";
const OBSERVER: &str = r#"
stage <- 0;
ready_at <- 0.0;
function HammeRRCaptureThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.5;
    if (stage == 0) {
        SendToConsole("toggle sv_cheats 1; toggle sv_lan 1; toggle joystick 0; toggle bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; toggle r_drawvgui 0; gameui_hide; toggle cl_drawhud 0; toggle r_drawviewmodel 0; toggle mat_fullbright 0; toggle mat_envmapsize 128");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() > 0) {
        SendToConsole("noclip; help sv_lan; help mat_hdr_level; help mat_fullbright; help mat_envmapsize");
        ready_at = Time() + 3.0;
        stage = 2;
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_CUBEMAP_READY");
        stage = 3;
    }
    if (stage == 3) SendToConsole("exec hammerr_capture_once.cfg");
    return 0.5;
}
"#;

pub(crate) fn game_directory(directory: &Path, request: &WorkerRequest) -> io::Result<PathBuf> {
    let game = directory.join("cstrike");
    if game.exists() {
        return Err(invalid("Private native game directory already exists"));
    }
    let root = request.environment.engine_root.canonicalize()?;
    if directory.starts_with(&root) || root.starts_with(directory) {
        return Err(invalid(
            "Private native output overlaps configured installation",
        ));
    }
    for subdirectory in [
        "maps",
        "cfg",
        "scripts/vscripts",
        "download",
        "materials",
        "materialsrc",
    ] {
        fs::create_dir_all(game.join(subdirectory))?;
    }
    let engine_bin = root.join("bin");
    std::os::unix::fs::symlink(engine_bin, directory.join("bin"))?;
    let mut mounts = vec![
        format!("\"game\" \"{}\"", game.display()),
        "\"mod+mod_write+default_write_path\" \"|gameinfo_path|.\"".into(),
        format!("\"game+game_write\" \"{}\"", game.display()),
        format!("\"game+download\" \"{}\"", game.join("download").display()),
        format!("\"gamebin\" \"{}\"", root.join("cstrike/bin").display()),
    ];
    for mount in &request.environment.mounts {
        let path = mount.path.canonicalize()?;
        let ids = ["game", "mod", "platform"]
            .into_iter()
            .filter(|id| mount.ids.iter().any(|mounted| mounted.as_str() == *id))
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            mounts.push(format!("\"{}\" \"{}\"", ids.join("+"), path.display()));
        }
    }
    let info = format!(
        "\"GameInfo\" {{ game \"HammeRR private capture\" title \"Counter-Strike Source\" type multiplayer_only nomodels 1 nohimodel 1 bots 1 nodegraph 0 FileSystem {{ SteamAppId 240 SearchPaths {{\n{}\n}} }} }}\n",
        mounts.join("\n")
    );
    util::atomic_write(&game.join("gameinfo.txt"), info.as_bytes())?;
    for name in ["config.cfg", "autoexec.cfg"] {
        util::atomic_write(&game.join("cfg").join(name), b"")?;
    }
    let diagnostic_case = match &request.profile {
        WorkerProfile::PplUv857(case) => Some(crate::diagnostic::DiagnosticCase::Uv857(*case)),
        WorkerProfile::PplMidrange909(case) => {
            Some(crate::diagnostic::DiagnosticCase::Midrange909(*case))
        }
        WorkerProfile::PplSpatial919(case) => {
            Some(crate::diagnostic::DiagnosticCase::Spatial919(*case))
        }
        WorkerProfile::PplMips931(case) => Some(crate::diagnostic::DiagnosticCase::Mips931(*case)),
        WorkerProfile::Reflection(_) => None,
    };
    if let Some(case) = diagnostic_case {
        crate::diagnostic::configure_game(directory, &game, case)?;
        fs::copy(
            directory.join("staged.bsp"),
            game.join("maps/hammerr_runtime.bsp"),
        )?;
        return Ok(game);
    }
    let WorkerProfile::Reflection(profile) = &request.profile else {
        return Err(invalid("Expected reflection profile"));
    };
    let postmap=b"toggle sv_lan 1\ntoggle mat_fullbright 0\ntoggle mat_envmapsize 128\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\nhelp mat_envmapsize\n";
    for name in [
        "server.cfg",
        "listenserver.cfg",
        "hammerr_capture_postmap.cfg",
    ] {
        util::atomic_write(&game.join("cfg").join(name), postmap)?;
    }
    util::atomic_write(
        &game.join("motd.txt"),
        b"HammeRR private reflection capture\n",
    )?;
    util::atomic_write(
        &game.join("scripts/vscripts/mapspawn.nut"),
        b"printl(\"HAMMERR_MAPSPAWN\");\n",
    )?;
    guard::reject_writes(OBSERVER, false)?;
    util::atomic_write(
        &game.join("scripts/vscripts/hammerr_capture.nut"),
        OBSERVER.as_bytes(),
    )?;
    util::atomic_write(
        &game.join("cfg/hammerr_capture_once.cfg"),
        b"echo HAMMERR_CUBEMAP_WAITING\n",
    )?;
    let launch_profile = format!(
        "toggle con_logfile hammerr-capture.log\ntoggle sv_cheats 1\ntoggle sv_lan 1\ntoggle mat_hdr_level {}\ntoggle mat_fullbright 0\ntoggle mat_envmapsize 128\ntoggle fps_max 60\ntoggle servercfgfile hammerr_capture_postmap.cfg\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\nhelp mat_envmapsize\necho {PREMAP}\nmap {}\n",
        profile.channel.hdr_level(),
        profile.map_name
    );
    guard::reject_writes(&launch_profile, false)?;
    util::atomic_write(
        &game.join("cfg").join(PRIVATE_PROFILE),
        launch_profile.as_bytes(),
    )?;
    util::atomic_write(&game.join("cfg/valve.rc"), b"stuffcmds\n")?;
    fs::copy(
        directory.join("staged.bsp"),
        game.join("maps").join(format!("{}.bsp", profile.map_name)),
    )?;
    Ok(game)
}

fn verify_controls(text: &str, channel: crate::CaptureChannel) -> io::Result<()> {
    for (name, expected) in [
        ("sv_lan", 1.0),
        ("mat_hdr_level", f64::from(channel.hdr_level())),
        ("mat_fullbright", 0.0),
        ("mat_envmapsize", 128.0),
    ] {
        let pattern = Regex::new(&format!(r#""{name}" = "([^"]+)""#)).map_err(io::Error::other)?;
        let actual = pattern
            .captures_iter(text)
            .last()
            .and_then(|capture| capture.get(1))
            .and_then(|value| value.as_str().parse::<f64>().ok());
        if actual != Some(expected) {
            return Err(invalid(format!(
                "Native capture did not prove {name}={expected}"
            )));
        }
    }
    Ok(())
}

fn verify_markers(text: &str, channel: crate::CaptureChannel) -> io::Result<()> {
    let mut previous = None;
    for marker in [
        PREMAP,
        "HAMMERR_MAPSPAWN",
        READY,
        BEGIN,
        RETURNED,
        DISCONNECTED,
    ] {
        let offset = text
            .find(marker)
            .ok_or_else(|| invalid(format!("Missing native capture marker {marker}")))?;
        if previous.is_some_and(|previous| offset <= previous) {
            return Err(invalid("Native capture handshake is out of order"));
        }
        previous = Some(offset);
    }
    if text.matches(PREMAP).count() != 1 || text.matches(BEGIN).count() != 1 {
        return Err(invalid("Native launch profile or capture repeated"));
    }
    if text.contains("Host_WriteConfiguration: Wrote") {
        return Err(invalid("Native capture wrote archived configuration"));
    }
    verify_controls(text, channel)
}

pub(crate) fn private_launch_options(command: &mut Command) {
    command.args([
        "-default",
        "-condebug",
        "+exec",
        PRIVATE_PROFILE,
        "-insecure",
    ]);
}

pub(crate) fn verify_native_diagnostics(text: &str) -> io::Result<()> {
    for message in [
        "Host_Error:",
        "ValidateLump:",
        "vtex failed to compile cubemap",
        "Can't load vtex_dll",
        "Can't load bsppack",
        "Cube map buffer size",
        "Host_WriteConfiguration: Wrote",
        "trying a fall back to a non-HDR version.",
    ] {
        if text.contains(message) {
            return Err(invalid(format!(
                "Native client reported a capture/configuration failure: {message}"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureViewport {
    width: u32,
    height: u32,
}

const DEFAULT_VIEWPORT: CaptureViewport = CaptureViewport {
    width: 1280,
    height: 720,
};
const LARGE_VIEWPORT: CaptureViewport = CaptureViewport {
    height: 1024,
    ..DEFAULT_VIEWPORT
};
const VIEWPORT_TOOL_BYTES: usize = 64 * 1024;
const DEFAULT_SAMPLE_EDGE: u16 = 128;

impl CaptureViewport {
    fn for_largest(largest: u16) -> io::Result<Self> {
        if !largest.is_power_of_two() || largest > crate::MAX_CUBE_EDGE {
            return Err(invalid(
                "Unsupported private reflection viewport sample size",
            ));
        }
        Ok(if largest > DEFAULT_SAMPLE_EDGE {
            LARGE_VIEWPORT
        } else {
            DEFAULT_VIEWPORT
        })
    }

    fn verify_png(self, image: &[u8], largest: u16) -> io::Result<()> {
        if image.len() < 24 || image[..8] != *b"\x89PNG\r\n\x1a\n" || image[12..16] != *b"IHDR" {
            return Err(invalid("Private viewport observation is not a PNG"));
        }
        let width = u32::from_be_bytes(image[16..20].try_into().map_err(io::Error::other)?);
        let height = u32::from_be_bytes(image[20..24].try_into().map_err(io::Error::other)?);
        if width != self.width
            || height != self.height
            || width.min(height) < u32::from(largest) * 4
        {
            return Err(invalid(format!(
                "Actual private viewport {width}×{height} does not match the verified {}×{} capture setup and native sample-size gate",
                self.width, self.height
            )));
        }
        Ok(())
    }
}

fn viewport_tool(
    directory: &Path,
    request: &WorkerRequest,
    command: &mut Command,
    label: &str,
) -> io::Result<Vec<u8>> {
    if cancelled(directory, request) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Private viewport command cancelled before launch",
        ));
    }
    let stdout_path = directory.join(format!("{label}.stdout"));
    let stderr_path = directory.join(format!("{label}.stderr"));
    let stdout = File::create(&stdout_path)?;
    let stderr = File::create(&stderr_path)?;
    command
        .current_dir(directory)
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
    util::json_write(
        &directory.join(format!("{label}.json")),
        &json!({"program":command.get_program().to_string_lossy(),"args":command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),"private_runtime":directory.join("wayland"),"wayland_display":std::env::var_os("WAYLAND_DISPLAY").map(|value| value.to_string_lossy().into_owned())}),
    )?;
    let mut child = command.spawn()?;
    let started = Instant::now();
    let result = (|| {
        loop {
            if cancelled(directory, request) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Private viewport command cancelled",
                ));
            }
            if fs::metadata(&stdout_path)?.len() > VIEWPORT_TOOL_BYTES as u64
                || fs::metadata(&stderr_path)?.len() > VIEWPORT_TOOL_BYTES as u64
            {
                return Err(invalid(
                    "Private viewport command exceeds its output budget",
                ));
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(invalid(format!(
                        "Private viewport command {label} failed with {status}"
                    )));
                }
                return Ok(());
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Private viewport command timed out",
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    let cleanup = crate::process::stop_group(&mut child);
    match (result, cleanup) {
        (Ok(()), Ok(())) => {}
        (Err(a), Err(b)) => return Err(invalid(format!("{a}; viewport command cleanup: {b}"))),
        (Err(error), _) | (_, Err(error)) => return Err(error),
    }
    util::read(&stderr_path, VIEWPORT_TOOL_BYTES)?;
    util::read(&stdout_path, VIEWPORT_TOOL_BYTES)
}

#[derive(serde::Deserialize)]
struct PrivateOutput {
    name: String,
    enabled: bool,
    scale: f64,
    transform: String,
    modes: Vec<PrivateMode>,
}

#[derive(serde::Deserialize)]
struct PrivateMode {
    width: u32,
    height: u32,
    current: bool,
}

fn private_output(bytes: &[u8]) -> io::Result<PrivateOutput> {
    let outputs: Vec<PrivateOutput> = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let [output]: [PrivateOutput; 1] = outputs.try_into().map_err(|_: Vec<PrivateOutput>| {
        invalid("Private capture requires exactly one headless output")
    })?;
    if !output.enabled
        || output.scale != 1.0
        || output.transform != "normal"
        || !output.name.strip_prefix("HEADLESS-").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 8
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(invalid(
            "Private capture output identity or transform is unsupported",
        ));
    }
    Ok(output)
}

fn configure_viewport(
    directory: &Path,
    request: &WorkerRequest,
    selected: CaptureViewport,
) -> io::Result<()> {
    if selected == DEFAULT_VIEWPORT {
        return Ok(());
    }
    if std::env::var_os("WAYLAND_DISPLAY").as_deref() != Some(std::ffi::OsStr::new("wayland-0")) {
        return Err(invalid(
            "Private output management requires the owned Wayland socket",
        ));
    }
    let before = viewport_tool(
        directory,
        request,
        Command::new("wlr-randr").arg("--json"),
        "output-before",
    )?;
    let output = private_output(&before)?;
    let mode = format!("{}x{}@60Hz", selected.width, selected.height);
    viewport_tool(
        directory,
        request,
        Command::new("wlr-randr").args(["--output", &output.name, "--custom-mode", &mode]),
        "output-mode",
    )?;
    let after = viewport_tool(
        directory,
        request,
        Command::new("wlr-randr").arg("--json"),
        "output-after",
    )?;
    let resized = private_output(&after)?;
    let current = resized
        .modes
        .iter()
        .filter(|mode| mode.current)
        .collect::<Vec<_>>();
    if resized.name != output.name
        || current.len() != 1
        || current[0].width != selected.width
        || current[0].height != selected.height
    {
        return Err(invalid(
            "Private headless output did not accept the selected capture mode",
        ));
    }
    Ok(())
}

fn viewport(
    directory: &Path,
    request: &WorkerRequest,
    selected: CaptureViewport,
    largest: u16,
) -> io::Result<()> {
    let image = directory.join("viewport.png");
    viewport_tool(
        directory,
        request,
        Command::new("grim").arg(&image),
        "viewport-command",
    )?;
    let image = util::read(&image, 16 * 1024 * 1024)?;
    selected.verify_png(&image, largest)
}

pub(crate) fn loader_budget(directory: &Path) -> io::Result<()> {
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("loader.") {
            count += 1;
            bytes = bytes
                .checked_add(entry.metadata()?.len())
                .ok_or_else(|| invalid("Loader byte count overflow"))?;
            if count > 16 || bytes > 8 * 1024 * 1024 {
                return Err(invalid(
                    "Native loader diagnostics exceed their live budget",
                ));
            }
        }
    }
    Ok(())
}

fn modules(directory: &Path, root: &Path, native_pid: u32) -> io::Result<()> {
    let mut found = BTreeSet::new();
    let mut count = 0;
    let mut bytes = 0;
    let pattern = Regex::new(r"file=(.+?) \[\d+\];\s+generating link map|calling init:\s*(.+)$")
        .map_err(io::Error::other)?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name() != format!("loader.{native_pid}").as_str() {
            continue;
        }
        count += 1;
        bytes += entry.metadata()?.len();
        if count > 16 || bytes > 8 * 1024 * 1024 {
            return Err(invalid("Native loader diagnostics exceed their budget"));
        }
        for line in util::diagnostic_text(&entry.path(), 8 * 1024 * 1024)?.lines() {
            let Some(captures) = pattern.captures(line) else {
                continue;
            };
            let Some(path) = captures.get(1).or_else(|| captures.get(2)) else {
                continue;
            };
            let path = PathBuf::from(path.as_str().trim());
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !matches!(name, "engine.so" | "vtex_dll.so" | "bsppack.so") {
                continue;
            }
            let actual = if path.is_absolute() {
                path.clone()
            } else {
                root.join(&path)
            }
            .canonicalize()?;
            if actual != root.join("bin/linux64").join(name).canonicalize()? {
                return Err(invalid(format!(
                    "Native module {name} loaded from an unexpected path"
                )));
            }
            found.insert(name.to_owned());
        }
    }
    if found.len() != 3 {
        return Err(invalid(
            "Did not positively observe all pinned native capture modules",
        ));
    }
    Ok(())
}

pub(crate) fn cancelled(directory: &Path, request: &WorkerRequest) -> bool {
    directory.join("cancel").exists()
        || crate::process::start_time(request.supervisor)
            .ok()
            .as_deref()
            != Some(&request.supervisor_start)
}

fn execute(directory: &Path, request: &WorkerRequest) -> io::Result<()> {
    if request.version != crate::process::WORKER_VERSION
        || directory.file_name().and_then(|n| n.to_str()) != Some(&request.nonce)
    {
        return Err(invalid("Invalid private worker protocol identity"));
    }
    util::validate_name(request.map_name())?;
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        || std::env::var_os("XDG_RUNTIME_DIR").as_deref()
            != Some(directory.join("wayland").as_os_str())
    {
        return Err(invalid("Native worker requires its private compositor"));
    }
    guard::validate_environment(&request.environment)?;
    match &request.profile {
        WorkerProfile::PplUv857(case) => {
            return crate::diagnostic::execute(
                directory,
                request,
                crate::diagnostic::DiagnosticCase::Uv857(*case),
            );
        }
        WorkerProfile::PplMidrange909(case) => {
            return crate::diagnostic::execute(
                directory,
                request,
                crate::diagnostic::DiagnosticCase::Midrange909(*case),
            );
        }
        WorkerProfile::PplSpatial919(case) => {
            return crate::diagnostic::execute(
                directory,
                request,
                crate::diagnostic::DiagnosticCase::Spatial919(*case),
            );
        }
        WorkerProfile::PplMips931(case) => {
            return crate::diagnostic::execute(
                directory,
                request,
                crate::diagnostic::DiagnosticCase::Mips931(*case),
            );
        }
        WorkerProfile::Reflection(_) => {}
    }
    let WorkerProfile::Reflection(profile) = &request.profile else {
        return Err(invalid("Expected reflection profile"));
    };
    let largest = profile
        .samples
        .iter()
        .map(|sample| sample.dimension)
        .max()
        .unwrap_or(DEFAULT_SAMPLE_EDGE);
    let selected_viewport = CaptureViewport::for_largest(largest)?;
    configure_viewport(directory, request, selected_viewport)?;
    let game = game_directory(directory, request)?;
    for name in ["config", "cache", "data"] {
        fs::create_dir_all(directory.join("settings").join(name))?;
    }
    let root = request.environment.engine_root.canonicalize()?;
    let port_socket = UdpSocket::bind("127.0.0.1:0")?;
    let port = port_socket.local_addr()?.port();
    drop(port_socket);
    let log = File::create(directory.join("launcher.log"))?;
    let executable = root.join("cstrike_linux64").canonicalize()?;
    let mut isolated = crate::isolation::Launch::new(directory, &executable)?;
    let command = &mut isolated.command;
    command
        .current_dir(&root)
        .args(["-game"])
        .arg(&game)
        .args(["-windowed", "-w"])
        .arg(selected_viewport.width.to_string())
        .arg("-h")
        .arg(selected_viewport.height.to_string())
        .arg("-novid");
    command
        .args(["-nomaster", "-nojoy", "-console", "-port"])
        .arg(port.to_string());
    private_launch_options(command);
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
        &json!({"program":command.get_program().to_string_lossy(),"args":command.get_args().map(|arg|arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),"native_program":executable,"cwd":root,"native_default_sample_size":DEFAULT_SAMPLE_EDGE,"largest_sample":largest,"viewport":{"width":selected_viewport.width,"height":selected_viewport.height},"desktop_input":false,"filesystem_isolation":{"external_paths":"read_only","writable_capture_directory":directory}}),
    )?;
    guard::ensure_no_client(&root)?;
    if cancelled(directory, request) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Native capture caller exited or cancelled before launch",
        ));
    }
    let mut child = isolated.spawn()?;
    let started = Instant::now();
    let console = game.join("hammerr-capture.log");
    let mut armed = false;
    let mut disarmed = false;
    let mut current = 0;
    let mut native = None;
    let progress = Regex::new(r"bounce: 1/1 sample: (\d+)/(\d+)").map_err(io::Error::other)?;
    let result = (|| {
        loop {
            if cancelled(directory, request) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Native capture cancelled or parent exited",
                ));
            }
            if native.is_none()
                && let Some(identity) = isolated.identity(&child, &executable)?
            {
                util::json_write(&directory.join("native-process.json"), &identity)?;
                native = Some(identity);
            }
            let text = if console.exists() {
                util::diagnostic_text(&console, util::MAX_LOG)?
            } else {
                String::new()
            };
            verify_native_diagnostics(&text)?;
            if fs::metadata(directory.join("launcher.log"))?.len() > 8 * 1024 * 1024 {
                return Err(invalid("Native launcher log exceeds 8 MiB"));
            }

            loader_budget(directory)?;
            util::private_budget(directory, &root)?;
            if !armed && text.contains(READY) {
                if native.is_none() {
                    return Err(invalid(
                        "Native capture did not establish its isolated host process identity",
                    ));
                }
                if text
                    .find(PREMAP)
                    .zip(text.find("HAMMERR_MAPSPAWN"))
                    .is_none_or(|(a, b)| a >= b)
                {
                    return Err(invalid("Private controls were not applied before map load"));
                }
                verify_controls(&text, profile.channel)?;
                viewport(directory, request, selected_viewport, largest)?;
                for sample in &profile.samples {
                    let name = sample
                        .texture
                        .trim_start_matches("materials/")
                        .trim_end_matches(".vtf");
                    if text.contains(&format!("Error reading texture data \"{name}\""))
                        || text.contains(&format!("couldn't find {name}"))
                    {
                        return Err(invalid(format!("Native engine rejected bootstrap {name}")));
                    }
                }
                let cfg = format!(
                    "echo {BEGIN}\nbuildcubemaps 1\necho {RETURNED}\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\nhelp mat_envmapsize\ndisconnect\necho {DISCONNECTED}\nquit\n"
                );
                util::atomic_write(&game.join("cfg/hammerr_capture_once.cfg"), cfg.as_bytes())?;
                armed = true;
            }
            if armed && !disarmed && text.contains(BEGIN) {
                util::atomic_write(
                    &game.join("cfg/hammerr_capture_once.cfg"),
                    b"echo HAMMERR_CUBEMAP_DISARMED\n",
                )?;
                disarmed = true;
            }
            for capture in progress.captures_iter(&text) {
                let count = capture[1].parse::<usize>().map_err(io::Error::other)?;
                let total = capture[2].parse::<usize>().map_err(io::Error::other)?;
                if total != profile.samples.len() || count == 0 || count > total {
                    return Err(invalid("Native sample progress differs from the manifest"));
                }
                current = current.max(count);
            }
            util::json_write(
                &directory.join("progress.json"),
                &[current, profile.samples.len()],
            )?;
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(invalid(format!(
                        "Isolated native client exited with {status}; inspect {}",
                        directory.join("launcher.log").display()
                    )));
                }
                break;
            }
            if started.elapsed() > Duration::from_secs(180) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native reflection capture exceeded 180 seconds",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
        let text = util::diagnostic_text(&console, util::MAX_LOG)?;
        verify_native_diagnostics(&text)?;
        verify_markers(&text, profile.channel)?;
        for index in 1..=profile.samples.len() {
            if !text.contains(&format!(
                "bounce: 1/1 sample: {index}/{}",
                profile.samples.len()
            )) {
                return Err(invalid("Native capture did not report every sample"));
            }
        }
        let native =
            native.ok_or_else(|| invalid("Native capture has no isolated process receipt"))?;
        modules(directory, &root, native.pid)?;
        stop_client_group(&mut child)?;
        let captured = game.join("maps").join(format!("{}.bsp", profile.map_name));
        if fs::metadata(&captured)?.len() > util::MAX_BSP as u64 {
            return Err(invalid("Native captured BSP exceeds its file budget"));
        }
        fs::rename(captured, directory.join("captured.bsp"))?;
        util::private_budget(directory, &root)?;
        Ok(())
    })();
    let cleanup = stop_client_group(&mut child);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => Err(invalid(format!("{a}; native process cleanup: {b}"))),
        (Err(e), _) | (_, Err(e)) => Err(e),
    }
}

pub(crate) fn stop_client_group(child: &mut std::process::Child) -> io::Result<()> {
    crate::process::stop_group(child)
}

pub(crate) fn run(directory: &Path) -> io::Result<()> {
    let directory = directory.canonicalize()?;
    let request: WorkerRequest = util::json_read(&directory.join("request.json"), 2 * 1024 * 1024)?;
    let baseline: Baseline = util::json_read(
        &directory.join("private-preservation-before.json"),
        util::MAX_CONFIG,
    )?;
    let started = Instant::now();
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &limit);
    }
    let result = execute(&directory, &request);
    let audit = guard::finish(&baseline, &directory, "worker");
    let error = match (&result, &audit) {
        (Ok(()), Ok(())) => None,
        (Err(a), Err(b)) => Some(format!("{a}; preservation audit: {b}")),
        (Err(e), _) | (_, Err(e)) => Some(e.to_string()),
    };
    util::json_write(
        &directory.join("result.json"),
        &WorkerResult {
            success: error.is_none(),
            error: error.clone(),
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            verified_controls: result.is_ok(),
        },
    )?;
    if let Some(error) = error {
        Err(invalid(error))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_mount_ids_keep_mod_reads_without_game_shadowing_or_write_access() {
        let directory = tempfile::tempdir().unwrap();
        let engine = directory.path().join("engine");
        let output = directory.path().join("capture");
        let game_content = directory.path().join("actual-game-content");
        let mod_only = directory.path().join("excluded-mod-content");
        let archive = directory.path().join("game-content_dir.vpk");
        for path in [
            engine.join("bin"),
            engine.join("cstrike/bin"),
            output.clone(),
            game_content.clone(),
            mod_only.clone(),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(output.join("staged.bsp"), b"staged fixture").unwrap();
        fs::write(mod_only.join("shared.txt"), b"mod only").unwrap();
        fs::write(game_content.join("shared.txt"), b"game content").unwrap();
        let mut index = 0x55aa_1234u32.to_le_bytes().to_vec();
        index.extend(1u32.to_le_bytes());
        index.extend(1u32.to_le_bytes());
        index.push(0);
        fs::write(&archive, index).unwrap();
        let request = WorkerRequest {
            version: crate::process::WORKER_VERSION,
            nonce: "fixture".into(),
            supervisor: 1,
            supervisor_start: "0".into(),
            environment: crate::CaptureEnvironment {
                engine_root: engine,
                steam_root: PathBuf::new(),
                mounts: vec![
                    crate::ContentMount {
                        ids: vec![
                            "mod".into(),
                            "mod_write".into(),
                            "default_write_path".into(),
                        ],
                        path: mod_only.clone(),
                    },
                    crate::ContentMount {
                        ids: vec!["game".into(), "mod".into(), "game_write".into()],
                        path: game_content.clone(),
                    },
                    crate::ContentMount {
                        ids: vec!["game".into(), "platform".into(), "game_write".into()],
                        path: archive.clone(),
                    },
                ],
            },
            profile: WorkerProfile::Reflection(crate::process::ReflectionProfile {
                channel: crate::CaptureChannel::Ldr,
                map_name: "fixture".into(),
                samples: vec![],
            }),
        };
        let game = game_directory(&output, &request).unwrap();
        assert_eq!(fs::read(game.join("cfg/valve.rc")).unwrap(), b"stuffcmds\n");
        let profile = fs::read_to_string(game.join("cfg").join(PRIVATE_PROFILE)).unwrap();
        assert_eq!(profile.matches(PREMAP).count(), 1);
        assert_eq!(profile.matches("\nmap fixture\n").count(), 1);
        assert!(profile.contains("toggle sv_lan 1\n"));
        assert!(profile.contains("toggle mat_hdr_level 0\n"));
        assert!(!profile.contains("stuffcmds"));
        assert!(!profile.contains("exec "));
        let gameinfo = fs::read_to_string(game.join("gameinfo.txt")).unwrap();
        assert!(gameinfo.contains(&format!(
            "\"game+platform\" \"{}\"",
            archive.canonicalize().unwrap().display()
        )));
        let mounted =
            source_formats::gamefs::GameFs::from_gameinfo(game.join("gameinfo.txt")).unwrap();
        let source_ids = |path: &Path| {
            &mounted
                .mounts
                .iter()
                .find(|mount| mount.mount.location() == path)
                .unwrap()
                .ids
        };
        assert_eq!(source_ids(&game_content), &["game", "mod"]);
        assert_eq!(source_ids(&mod_only), &["mod"]);
        assert_eq!(source_ids(&archive), &["game", "platform"]);
        let first_mod = mounted
            .mounts
            .iter()
            .find(|mount| mount.ids.iter().any(|id| id == "mod"))
            .unwrap();
        assert_eq!(first_mod.mount.location(), game.as_path());
        assert_eq!(
            mounted.read("shared.txt").unwrap().unwrap(),
            b"game content"
        );
    }

    #[test]
    fn private_launch_options_select_one_profile_with_configuration_guards() {
        let mut command = Command::new("not-launched");
        command.args(["-game", "private/cstrike", "-port", "27015"]);
        private_launch_options(&mut command);
        let arguments: Vec<_> = command
            .get_args()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(
            arguments,
            [
                "-game",
                "private/cstrike",
                "-port",
                "27015",
                "-default",
                "-condebug",
                "+exec",
                "hammerr_private_launch.cfg",
                "-insecure"
            ]
        );
    }

    #[test]
    fn actual_controls_and_order_are_required() {
        for (edge, expected) in [
            (1, DEFAULT_VIEWPORT),
            (128, DEFAULT_VIEWPORT),
            (256, LARGE_VIEWPORT),
        ] {
            let selected = CaptureViewport::for_largest(edge).unwrap();
            assert_eq!(selected, expected);
            let mut png = vec![0; 24];
            png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
            png[12..16].copy_from_slice(b"IHDR");
            png[16..20].copy_from_slice(&expected.width.to_be_bytes());
            png[20..24].copy_from_slice(&expected.height.to_be_bytes());
            selected.verify_png(&png, edge).unwrap();
            if edge == 128 {
                assert!(selected.verify_png(&png, 256).is_err());
            }
            png[20..24].copy_from_slice(&719u32.to_be_bytes());
            assert!(selected.verify_png(&png, edge).is_err());
        }
        for edge in [0, 129, 512] {
            assert!(CaptureViewport::for_largest(edge).is_err());
        }
        let output = json!({"name":"HEADLESS-1","enabled":true,"scale":1.0,"transform":"normal","modes":[{"width":1280,"height":1024,"current":true}]});
        let bytes = serde_json::to_vec(&vec![output.clone()]).unwrap();
        assert_eq!(private_output(&bytes).unwrap().name, "HEADLESS-1");
        let bytes = serde_json::to_vec(&vec![output.clone(), output.clone()]).unwrap();
        assert!(private_output(&bytes).is_err());
        for (key, value) in [
            ("name", json!("DP-1")),
            ("enabled", json!(false)),
            ("scale", json!(2.0)),
            ("transform", json!("90")),
        ] {
            let mut changed = output.clone();
            changed[key] = value;
            assert!(private_output(&serde_json::to_vec(&vec![changed]).unwrap()).is_err());
        }

        let controls = "\"sv_lan\" = \"1\"\n\"mat_hdr_level\" = \"0\"\n\"mat_fullbright\" = \"0\"\n\"mat_envmapsize\" = \"128\"\n";
        let ordered = format!(
            "{PREMAP}\nHAMMERR_MAPSPAWN\n{READY}\n{BEGIN}\n{RETURNED}\n{DISCONNECTED}\n{controls}"
        );
        assert!(verify_markers(&ordered, crate::CaptureChannel::Ldr).is_ok());
        for marker in [PREMAP, BEGIN] {
            let duplicate = ordered.replace(marker, &format!("{marker}\n{marker}"));
            let error = verify_markers(&duplicate, crate::CaptureChannel::Ldr).unwrap_err();
            assert!(error.to_string().contains("repeated"));
        }
        assert!(
            verify_markers(
                &ordered.replace("\"sv_lan\" = \"1\"", "\"sv_lan\" = \"0\""),
                crate::CaptureChannel::Ldr
            )
            .is_err()
        );
        assert!(
            verify_markers(&format!("{BEGIN}\n{ordered}"), crate::CaptureChannel::Ldr).is_err()
        );
        assert!(
            verify_markers(&ordered.replace(RETURNED, ""), crate::CaptureChannel::Ldr).is_err()
        );
        assert!(verify_markers(&ordered, crate::CaptureChannel::Hdr).is_err());
        let hdr = ordered.replace("\"mat_hdr_level\" = \"0\"", "\"mat_hdr_level\" = \"2\"");
        assert!(verify_markers(&hdr, crate::CaptureChannel::Hdr).is_ok());
        assert!(
            verify_markers(
                &format!("{hdr}\n\"mat_hdr_level\" = \"0\"\n"),
                crate::CaptureChannel::Hdr
            )
            .is_err()
        );
        assert!(
            verify_native_diagnostics(
                "A custom HDR cubemap cannot be found, trying a fall back to a non-HDR version."
            )
            .is_err()
        );
    }

    #[test]
    fn external_diagnostics_keep_markers_and_failures_after_non_utf8_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.log");
        let mut bytes = b"Native diagnostic: \xe2\x02'\n".to_vec();
        bytes.extend_from_slice(format!("{PREMAP}\nHAMMERR_MAPSPAWN\n{READY}\n{BEGIN}\n{RETURNED}\n{DISCONNECTED}\n\"sv_lan\" = \"1\"\n\"mat_hdr_level\" = \"0\"\n\"mat_fullbright\" = \"0\"\n\"mat_envmapsize\" = \"128\"\n").as_bytes());
        fs::write(&path, &bytes).unwrap();
        let text = util::diagnostic_text(&path, util::MAX_LOG).unwrap();
        verify_native_diagnostics(&text).unwrap();
        verify_markers(&text, crate::CaptureChannel::Ldr).unwrap();
        assert!(util::text(&path, util::MAX_LOG).is_err());
        assert!(util::diagnostic_text(&path, bytes.len() - 1).is_err());
        bytes.extend_from_slice(b"Host_Error: Bad string: \xe2\x02'\n");
        fs::write(&path, &bytes).unwrap();
        let text = util::diagnostic_text(&path, util::MAX_LOG).unwrap();
        assert!(
            verify_native_diagnostics(&text)
                .unwrap_err()
                .to_string()
                .contains("Host_Error:")
        );
    }
}
