use std::fs::{self, File};
use std::io;
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::isolation::Identity;
use crate::process::WorkerRequest;
use crate::util::{self, invalid};
use crate::{CaptureEnvironment, CaptureObserver};

mod hardware;
mod midrange;
mod mips;
mod profile;
mod sampler_profile;
mod spatial;

use hardware::{hardware_records, loaded_observer, verify_hardware};
pub use midrange::{PplMidrange909Case, PplMidrange909Receipt, PplMidrange909Request};
pub use mips::{PplMips931Case, PplMips931Receipt, PplMips931Request};
use profile::{
    COMPLETE, DISCONNECTED, MAX_INPUT, PROBE_SHA, READY, TIMEOUT, pinned_bytes, private_write,
    verify_controls, verify_installed, verify_loads, verify_order, verify_private_fixture,
    verify_staging,
};
pub(crate) use profile::{
    DiagnosticCase, DiagnosticRequest, MAP_NAME, configure_game, environment, observer_arguments,
    path_text, stage, validate_request_paths, verify_sources,
};
pub use profile::{PplUv857Case, PplUv857Receipt, PplUv857Request};
pub use spatial::{PplSpatial919Case, PplSpatial919Receipt, PplSpatial919Request};

#[derive(Serialize)]
struct DiagnosticObservation {
    #[serde(flatten)]
    capture: serde_json::Value,
    sampler_delivery: sampler_profile::SamplerDelivery,
}

fn live_identity(
    launch: &crate::isolation::Launch,
    child: &std::process::Child,
    program: &Path,
    expected: &Identity,
) -> io::Result<()> {
    let actual = launch
        .identity(child, program)?
        .ok_or_else(|| invalid("Owned diagnostic native process exited"))?;
    if actual.pid != expected.pid || actual.start != expected.start {
        return Err(invalid("Owned diagnostic native process identity changed"));
    }
    Ok(())
}

fn snapshot(
    directory: &Path,
    request: &WorkerRequest,
    ppm: bool,
    deadline: Instant,
) -> io::Result<()> {
    let mut command = Command::new("grim");
    if ppm {
        command.args(["-t", "ppm"]);
    }
    let path = directory.join(if ppm { "game.ppm" } else { "game.png" });
    let mut child = command
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let started = Instant::now();
    let result = (|| {
        loop {
            check_cancelled(directory, request)?;
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(invalid(format!(
                        "Private diagnostic screenshot failed: {status}"
                    )));
                }
                break;
            }
            if started.elapsed() > Duration::from_secs(10) || Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Private diagnostic screenshot timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let bytes = util::read(&path, 8 * 1024 * 1024)?;
        if ppm {
            let pixels = bytes
                .strip_prefix(b"P6\n1280 720\n255\n")
                .ok_or_else(|| invalid("Diagnostic PPM is not the fixed 1280x720 RGB frame"))?;
            if pixels.len() != 1280 * 720 * 3 {
                return Err(invalid("Diagnostic PPM has an incomplete RGB frame"));
            }
        } else if bytes.len() < 24
            || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
            || bytes[16..24] != [0, 0, 5, 0, 0, 0, 2, 208]
        {
            return Err(invalid("Diagnostic PNG is not the fixed 1280x720 frame"));
        }
        Ok(())
    })();
    let cleanup = crate::process::stop_group(&mut child);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => Err(invalid(format!("{a}; screenshot cleanup: {b}"))),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn check_cancelled(directory: &Path, request: &WorkerRequest) -> io::Result<()> {
    if crate::worker::cancelled(directory, request) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Native diagnostic cancelled or caller exited",
        ));
    }
    Ok(())
}

fn verify_case_controls(
    text: &str,
    case: DiagnosticCase,
) -> io::Result<std::collections::BTreeMap<String, f64>> {
    match case {
        DiagnosticCase::Spatial919(_) | DiagnosticCase::Mips931(_) => {
            Ok(spatial::verify_phase(text, spatial::Phase::Readiness)?.controls)
        }
        _ => verify_controls(text),
    }
}

fn observe_spatial_phase(
    directory: &Path,
    request: &WorkerRequest,
    console: &Path,
    phase: spatial::Phase,
    deadline: Instant,
) -> io::Result<spatial::Observation> {
    let before = util::diagnostic_text(console, util::MAX_LOG)?;
    if phase.started(&before) {
        return Err(invalid(
            "Spatial diagnostic requires a fresh phase observation",
        ));
    }
    private_write(
        &directory.join("cstrike/cfg/hammerr_diagnostic_once.cfg"),
        phase.request()?,
    )?;
    loop {
        check_cancelled(directory, request)?;
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Spatial diagnostic observation exceeded the native deadline",
            ));
        }
        let text = util::diagnostic_text(console, util::MAX_LOG)?;
        verify_loads(&text)?;
        crate::worker::loader_budget(directory)?;
        util::private_budget(directory, &request.environment.engine_root)?;
        if fs::metadata(directory.join("launcher.log"))?.len() > 8 * 1024 * 1024 {
            return Err(invalid("Native diagnostic launcher log exceeds 8 MiB"));
        }
        if phase.complete(&text) {
            return spatial::verify_phase(&text, phase);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn execute(
    directory: &Path,
    request: &WorkerRequest,
    case: DiagnosticCase,
) -> io::Result<()> {
    path_text(directory, "artifact directory")?;
    verify_staging(directory, case)?;
    let root = request.environment.engine_root.canonicalize()?;
    path_text(&root, "canonical engine root")?;
    verify_installed(&root)?;
    let game = crate::worker::game_directory(directory, request)?;
    for name in ["config", "cache", "data"] {
        fs::create_dir_all(directory.join("settings").join(name))?;
    }
    let port_socket = UdpSocket::bind("127.0.0.1:0")?;
    let port = port_socket.local_addr()?.port();
    drop(port_socket);
    let executable = root.join("cstrike_linux64").canonicalize()?;
    path_text(&executable, "canonical native executable")?;
    let mut isolated = crate::isolation::Launch::ppl_uv857(directory, &executable)?;
    let log = File::create(directory.join("launcher.log"))?;
    let command = &mut isolated.command;
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
    let sampler = sampler_profile::Prepared::prepare(directory, command)?;
    util::json_write(
        &directory.join("command.json"),
        &json!({
            "profile":case.profile_name(), "case":case,"program":command.get_program().to_string_lossy(),
            "args":command.get_args().map(|arg|arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "native_program":executable,"cwd":root,"desktop_input":false,
            "filesystem_isolation":{"external_paths":"read_only","writable_capture_directory":directory},
            "observer_sha256":PROBE_SHA,"hardware_channel":"observed_ldr_required","timeout_seconds":TIMEOUT.as_secs(),
            "sampler_profile":sampler_profile::PROFILE,"dxvk_environment":sampler.expected_environment(),
        }),
    )?;
    crate::guard::ensure_no_client(&root)?;
    check_cancelled(directory, request)?;
    let mut child = isolated.spawn()?;
    let started = Instant::now();
    let console = game.join("hammerr-runtime.log");
    let mut native = None;
    let mut capture_observation = None;
    let mut sampler_ready = None;
    let result = (|| {
        loop {
            check_cancelled(directory, request)?;
            if started.elapsed() > TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native diagnostic exceeded 65 seconds",
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
            verify_loads(&text)?;
            if fs::metadata(directory.join("launcher.log"))?.len() > 8 * 1024 * 1024 {
                return Err(invalid("Native diagnostic launcher log exceeds 8 MiB"));
            }
            crate::worker::loader_budget(directory)?;
            util::private_budget(directory, &root)?;
            if !directory.join("probe-ready").exists() && text.contains(READY) {
                verify_order(&text, false)?;
                verify_case_controls(&text, case)?;
                let identity = native.as_ref().ok_or_else(|| {
                    invalid("Diagnostic did not establish isolated native identity")
                })?;
                live_identity(&isolated, &child, &executable, identity)?;
                sampler_ready = Some(sampler.observe_ready(&root, identity)?);
                live_identity(&isolated, &child, &executable, identity)?;
                util::atomic_write(
                    &directory.join("probe-ready"),
                    b"verified post-map controls\n",
                )?;
            }
            let records = hardware_records(&directory.join("hardware.jsonl"))?;
            if capture_observation.is_none()
                && let Some(identity) = &native
                && let Some(hardware) = verify_hardware(&records, identity)?
            {
                if !directory.join("probe-ready").exists() {
                    return Err(invalid(
                        "Hardware observer completed before post-map readiness",
                    ));
                }
                live_identity(&isolated, &child, &executable, identity)?;
                loaded_observer(directory, &root, identity)?;
                let spatial_capture = match case {
                    DiagnosticCase::Spatial919(_) | DiagnosticCase::Mips931(_) => {
                        let observation = observe_spatial_phase(
                            directory,
                            request,
                            &console,
                            spatial::Phase::Capture,
                            started + TIMEOUT,
                        )?;
                        live_identity(&isolated, &child, &executable, identity)?;
                        Some(observation)
                    }
                    _ => None,
                };
                snapshot(directory, request, false, started + TIMEOUT)?;
                live_identity(&isolated, &child, &executable, identity)?;
                snapshot(directory, request, true, started + TIMEOUT)?;
                live_identity(&isolated, &child, &executable, identity)?;
                let mut text = util::diagnostic_text(&console, util::MAX_LOG)?;
                let spatial_observations = match spatial_capture {
                    Some(capture) => {
                        let final_observation = observe_spatial_phase(
                            directory,
                            request,
                            &console,
                            spatial::Phase::Final,
                            started + TIMEOUT,
                        )?;
                        live_identity(&isolated, &child, &executable, identity)?;
                        text = util::diagnostic_text(&console, util::MAX_LOG)?;
                        Some(vec![
                            spatial::verify_phase(&text, spatial::Phase::Readiness)?,
                            capture,
                            final_observation,
                        ])
                    }
                    None => None,
                };
                verify_loads(&text)?;
                let controls = match &spatial_observations {
                    Some(observations) => observations[2].controls.clone(),
                    None => verify_case_controls(&text, case)?,
                };
                let mut observation = json!({
                    "profile":case.profile_name(),"case":case,"camera_origin":[-192,-240,160],"camera_angles":[25,50,0],
                    "controls":controls,"hardware":hardware,"bsp_sha256":case.bsp_sha(),
                    "png_sha256":util::hash_file(&directory.join("game.png"),8*1024*1024)?,
                    "ppm_sha256":util::hash_file(&directory.join("game.ppm"),8*1024*1024)?,
                    "pixel_acceptance_evaluated":false,
                });
                if let Some(observations) = spatial_observations {
                    observation["spatial_camera_observed"] = json!(&observations[1].camera);
                    observation["spatial_phase_observations"] = json!(observations);
                }
                private_write(
                    &game.join("cfg/hammerr_diagnostic_once.cfg"),
                    &format!("echo {COMPLETE}\ndisconnect\necho {DISCONNECTED}\nquit\n"),
                )?;
                capture_observation = Some(observation);
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() || capture_observation.is_none() {
                    return Err(invalid(format!(
                        "Native diagnostic exited without a completed capture: {status}"
                    )));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let text = util::diagnostic_text(&console, util::MAX_LOG)?;
        verify_loads(&text)?;
        verify_order(&text, true)?;
        match case {
            DiagnosticCase::Spatial919(_) | DiagnosticCase::Mips931(_) => {
                spatial::verify_final(&text)?;
            }
            _ => {
                verify_case_controls(&text, case)?;
            }
        }
        let identity = native
            .as_ref()
            .ok_or_else(|| invalid("Missing diagnostic native identity"))?;
        let records = hardware_records(&directory.join("hardware.jsonl"))?;
        verify_hardware(&records, identity)?
            .ok_or_else(|| invalid("Hardware observer did not complete"))?;
        verify_staging(directory, case)?;
        verify_private_fixture(&game)?;
        pinned_bytes(
            &game.join("maps/hammerr_runtime.bsp"),
            case.bsp_sha(),
            MAX_INPUT,
        )?;
        verify_installed(&root)?;
        let sampler_delivery =
            sampler
                .finish(sampler_ready.ok_or_else(|| {
                    invalid("Missing required DXVK sampler readiness evidence")
                })?)?;
        let observation = DiagnosticObservation {
            capture: capture_observation
                .ok_or_else(|| invalid("Missing diagnostic capture observation"))?,
            sampler_delivery,
        };
        util::json_write(&directory.join("diagnostic-observation.json"), &observation)?;
        util::json_write(&directory.join("progress.json"), &[1, 1])?;
        util::private_budget(directory, &root)
    })();
    let cleanup = crate::worker::stop_client_group(&mut child);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => Err(invalid(format!("{a}; native process cleanup: {b}"))),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliRequest<C> {
    case: C,
    engine_root: PathBuf,
    steam_root: PathBuf,
    bsp: PathBuf,
    fixture_game: PathBuf,
    observer_library: PathBuf,
    artifacts_parent: PathBuf,
}

struct CliObserver;

impl CaptureObserver for CliObserver {
    fn checkpoint(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sample_progress(&mut self, _current: usize, _total: usize) {}
}

pub(crate) fn cli(path: &Path) -> io::Result<()> {
    let request: CliRequest<PplUv857Case> = util::json_read(path, 16 * 1024)?;
    let request = PplUv857Request {
        case: request.case,
        environment: CaptureEnvironment {
            engine_root: request.engine_root,
            steam_root: request.steam_root,
            mounts: Vec::new(),
        },
        bsp: request.bsp,
        fixture_game: request.fixture_game,
        observer_library: request.observer_library,
        artifacts_parent: request.artifacts_parent,
    };
    let receipt = crate::diagnose_ppl_uv857_with_cleanup(request, &mut CliObserver)
        .map_err(|failure| failure.error)?;
    println!(
        "HAMMERR_PPL_UV857 {}",
        serde_json::to_string(&receipt).map_err(io::Error::other)?
    );
    Ok(())
}

pub(crate) fn cli_midrange(path: &Path) -> io::Result<()> {
    let request: CliRequest<PplMidrange909Case> = util::json_read(path, 16 * 1024)?;
    let request = PplMidrange909Request {
        case: request.case,
        environment: CaptureEnvironment {
            engine_root: request.engine_root,
            steam_root: request.steam_root,
            mounts: Vec::new(),
        },
        bsp: request.bsp,
        fixture_game: request.fixture_game,
        observer_library: request.observer_library,
        artifacts_parent: request.artifacts_parent,
    };
    let receipt = crate::diagnose_ppl_midrange909_with_cleanup(request, &mut CliObserver)
        .map_err(|failure| failure.error)?;
    println!(
        "HAMMERR_PPL_MIDRANGE909 {}",
        serde_json::to_string(&receipt).map_err(io::Error::other)?
    );
    Ok(())
}

pub(crate) fn cli_spatial(path: &Path) -> io::Result<()> {
    let request: CliRequest<PplSpatial919Case> = util::json_read(path, 16 * 1024)?;
    let request = PplSpatial919Request {
        case: request.case,
        environment: CaptureEnvironment {
            engine_root: request.engine_root,
            steam_root: request.steam_root,
            mounts: Vec::new(),
        },
        bsp: request.bsp,
        fixture_game: request.fixture_game,
        observer_library: request.observer_library,
        artifacts_parent: request.artifacts_parent,
    };
    let receipt = crate::diagnose_ppl_spatial919_with_cleanup(request, &mut CliObserver)
        .map_err(|failure| failure.error)?;
    println!(
        "HAMMERR_PPL_SPATIAL919 {}",
        serde_json::to_string(&receipt).map_err(io::Error::other)?
    );
    Ok(())
}

pub(crate) fn cli_mips(path: &Path) -> io::Result<()> {
    let request: CliRequest<PplMips931Case> = util::json_read(path, 16 * 1024)?;
    let request = PplMips931Request {
        case: request.case,
        environment: CaptureEnvironment {
            engine_root: request.engine_root,
            steam_root: request.steam_root,
            mounts: Vec::new(),
        },
        bsp: request.bsp,
        fixture_game: request.fixture_game,
        observer_library: request.observer_library,
        artifacts_parent: request.artifacts_parent,
    };
    let receipt = crate::diagnose_ppl_mips931_with_cleanup(request, &mut CliObserver)
        .map_err(|failure| failure.error)?;
    println!(
        "HAMMERR_PPL_MIPS931 {}",
        serde_json::to_string(&receipt).map_err(io::Error::other)?
    );
    Ok(())
}
