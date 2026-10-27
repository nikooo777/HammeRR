use std::fs::{self, File};
use std::io;
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::isolation::Identity;
use crate::process::WorkerRequest;
use crate::util::{self, invalid};
use crate::{CaptureEnvironment, CaptureObserver};

mod hardware;
mod midrange;
mod mips;
mod profile;
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
    util::json_write(
        &directory.join("command.json"),
        &json!({
            "profile":case.profile_name(), "case":case,"program":command.get_program().to_string_lossy(),
            "args":command.get_args().map(|arg|arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "native_program":executable,"cwd":root,"desktop_input":false,
            "filesystem_isolation":{"external_paths":"read_only","writable_capture_directory":directory},
            "observer_sha256":PROBE_SHA,"hardware_channel":"observed_ldr_required","timeout_seconds":TIMEOUT.as_secs(),
        }),
    )?;
    crate::guard::ensure_no_client(&root)?;
    check_cancelled(directory, request)?;
    let mut child = isolated.spawn()?;
    let started = Instant::now();
    let console = game.join("hammerr-runtime.log");
    let mut native = None;
    let mut captured = false;
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
                util::atomic_write(
                    &directory.join("probe-ready"),
                    b"verified post-map controls\n",
                )?;
            }
            let records = hardware_records(&directory.join("hardware.jsonl"))?;
            if !captured
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
                util::json_write(&directory.join("diagnostic-observation.json"), &observation)?;
                private_write(
                    &game.join("cfg/hammerr_diagnostic_once.cfg"),
                    &format!("echo {COMPLETE}\ndisconnect\necho {DISCONNECTED}\nquit\n"),
                )?;
                captured = true;
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() || !captured {
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

#[cfg(test)]
mod tests {
    use super::profile::{CONTROLS, HARDWARE_VERSION, MAX_HARDWARE, PREMAP, PROBE, SAMPLE_COUNT};
    use super::*;
    use serde_json::Value;

    fn native() -> Identity {
        Identity {
            pid: 1234,
            start: "9876".into(),
            group: 1200,
            session: 1100,
            uid: 1000,
        }
    }

    fn records() -> Vec<Value> {
        let mut records = vec![
            json!({"event":"start","pid":1234,"hardware_version":HARDWARE_VERSION}),
            json!({"event":"hardware_factory","module":"materialsystem.so","route":"VMaterialSystem082.GetHardwareConfig","version":HARDWARE_VERSION,"code":0,"present":true}),
            json!({"event":"hardware_implementation","module":"shaderapivk.so"}),
        ];
        for index in 0..SAMPLE_COUNT {
            records.push(
                json!({"event":"sample","sequence":index,"monotonic_ns":1000+index,
                "dx_support_level":100,"hardware_hdr_type":1,"hdr_type":0,"hdr_enabled":false}),
            );
        }
        records.push(json!({"event":"complete"}));
        records
    }

    #[test]
    fn complete_observation_requires_owned_native_identity_and_exact_hardware_samples() {
        assert!(verify_hardware(&records(), &native()).unwrap().is_some());
        let mut other = native();
        other.pid += 1;
        assert!(verify_hardware(&records(), &other).is_err());
        for mutate in [0, 1, 2, 3, 4, 5, 6, 7] {
            let mut changed = records();
            match mutate {
                0 => {
                    changed.remove(0);
                }
                1 => {
                    changed.remove(1);
                }
                2 => {
                    changed.remove(3);
                }
                3 => {
                    changed.insert(3, changed[0].clone());
                }
                4 => {
                    changed[3]["dx_support_level"] = json!(80);
                }
                5 => {
                    changed[3]["hardware_hdr_type"] = json!(0);
                }
                6 => {
                    changed[3]["hdr_enabled"] = json!(true);
                }
                7 => {
                    changed[3]["hdr_type"] = json!(1);
                }
                _ => unreachable!(),
            }
            assert!(
                verify_hardware(&changed, &native()).is_err(),
                "mutation {mutate}"
            );
        }
        let mut incomplete = records();
        incomplete.pop();
        assert!(verify_hardware(&incomplete, &native()).unwrap().is_none());
    }

    #[test]
    fn observer_failures_and_unstable_sequences_cannot_be_hidden_by_completion() {
        for event in [
            "invalid_factory",
            "invalid_hardware_vtable",
            "acquisition_failed",
            "thread_failed",
            "ready_timeout",
            "unknown",
        ] {
            let mut changed = records();
            changed.insert(3, json!({"event":event}));
            assert!(verify_hardware(&changed, &native()).is_err());
        }
        for (field, value) in [
            ("sequence", json!(0)),
            ("monotonic_ns", json!(1000)),
            ("hardware_hdr_type", json!(2)),
            ("dx_support_level", json!(90)),
        ] {
            let mut changed = records();
            changed[4][field] = value;
            assert!(verify_hardware(&changed, &native()).is_err(), "{field}");
        }
        let mut after = records();
        after.push(json!({"event":"sample"}));
        assert!(verify_hardware(&after, &native()).is_err());
    }

    #[test]
    fn hardware_stream_bounds_partial_records_and_preserves_failures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hardware.jsonl");
        assert!(hardware_records(&path).unwrap().is_empty());
        fs::write(&path, b"{\"event\":\"start\"}\n{\"event\":").unwrap();
        assert_eq!(hardware_records(&path).unwrap().len(), 1);
        fs::write(&path, b"{\"event\":\"thread_failed\"}\n").unwrap();
        assert!(hardware_records(&path).is_err());
        fs::write(&path, vec![b'x'; MAX_HARDWARE + 1]).unwrap();
        assert!(hardware_records(&path).is_err());
    }

    #[test]
    fn console_readiness_does_not_substitute_for_native_hardware() {
        let controls = CONTROLS
            .iter()
            .map(|(name, value)| format!("\"{name}\" = \"{value}\"\n"))
            .collect::<String>();
        let ready = format!("{PREMAP}\nHAMMERR_MAPSPAWN\n{controls}{READY}\n");
        verify_order(&ready, false).unwrap();
        verify_controls(&ready).unwrap();
        assert!(verify_hardware(&[], &native()).unwrap().is_none());
        assert!(verify_order(&ready.replace("HAMMERR_MAPSPAWN", ""), false).is_err());
        assert!(verify_order(&format!("{ready}{READY}\n"), false).is_err());
        assert!(verify_controls(&format!("{ready}\"sv_lan\" = \"0\"\n")).is_err());
        assert!(verify_loads("Error reading texture data woodcrates01a.vtf").is_err());
        assert!(verify_loads("Host_WriteConfiguration: Wrote config.cfg").is_err());
    }

    #[test]
    fn arbitrary_observer_library_is_rejected_before_native_launch() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(PROBE), b"unapproved observer").unwrap();
        let mut command = Command::new("not-launched");
        assert!(
            observer_arguments(
                &mut command,
                directory.path(),
                Path::new("/engine/cstrike_linux64")
            )
            .is_err()
        );
        assert_eq!(command.get_args().count(), 0);
        assert!(!directory.path().join("hardware.jsonl").exists());
    }

    #[test]
    fn midrange_profile_admits_only_six_fixed_cases_and_keeps_uv_profile_separate() {
        let cases = [
            (
                "target-032",
                PplMidrange909Case::Target032,
                "adf714015cbc1c0bfc4c001474f60d73045a412b5f64221908f16fd11953a379",
            ),
            (
                "reference-032",
                PplMidrange909Case::Reference032,
                "76ea7591eddd1262cf0c8f409078430c49ab01219695084bbea0554839e6c8c0",
            ),
            (
                "target-064",
                PplMidrange909Case::Target064,
                "e829bc5d419cea49f035bba9079c50f33b112c25155ce640ab745fe0c0ce171e",
            ),
            (
                "reference-064",
                PplMidrange909Case::Reference064,
                "7f0b49548e11bc529d7fdbcdf62a84496c3389693fd4b73df5b3789d099265e5",
            ),
            (
                "target-096",
                PplMidrange909Case::Target096,
                "985e2766595ebdc477442d541e31971d0d0b4caa543e589373a81b03ad2b56a2",
            ),
            (
                "reference-096",
                PplMidrange909Case::Reference096,
                "7d90a2bfb370e166a68ebcca937d4b1791486874792ab41524f7596bab655d19",
            ),
        ];
        let uv_cases = [
            PplUv857Case::MissingUv,
            PplUv857Case::UniformR,
            PplUv857Case::UniformB,
            PplUv857Case::UniformG,
            PplUv857Case::UniformW,
            PplUv857Case::QuadrantR,
            PplUv857Case::QuadrantB,
            PplUv857Case::QuadrantG,
            PplUv857Case::QuadrantW,
        ];
        let directory = tempfile::tempdir().unwrap();
        let bsp = directory.path().join("wrong-case.bsp");
        fs::write(&bsp, b"unapproved BSP").unwrap();
        for (name, case, expected) in cases {
            assert_eq!(serde_json::to_value(case).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<PplMidrange909Case>(json!(name)).unwrap(),
                case
            );
            assert_eq!(case.bsp_sha(), expected);
            assert!(pinned_bytes(&bsp, case.bsp_sha(), MAX_INPUT).is_err());
            assert!(serde_json::from_value::<PplUv857Case>(json!(name)).is_err());
            assert!(
                serde_json::from_value::<crate::process::WorkerProfile>(
                    json!({"kind":"ppl_uv857", "parameters":name})
                )
                .is_err()
            );
            let profile = crate::process::WorkerProfile::PplMidrange909(case);
            assert_eq!(
                serde_json::to_value(profile).unwrap(),
                json!({"kind":"ppl_midrange909", "parameters":name})
            );
            assert!(
                serde_json::from_value::<crate::process::WorkerProfile>(
                    json!({"kind":"ppl_midrange909", "parameters":name})
                )
                .is_ok()
            );
            for uv in uv_cases {
                assert_ne!(case.bsp_sha(), uv.bsp_sha());
            }
        }
        for uv in uv_cases {
            let value = serde_json::to_value(uv).unwrap();
            assert!(serde_json::from_value::<PplMidrange909Case>(value).is_err());
        }
        for value in [
            json!({"kind":"ppl_midrange909","parameters":"target-128"}),
            json!({"kind":"ppl_midrange909","parameters":{"case":"target-032","bsp_sha256":"anything"}}),
            json!({"kind":"ppl_midrange909","parameters":"target-032","command":"anything"}),
        ] {
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_err());
        }
        assert_eq!(fs::read(bsp).unwrap(), b"unapproved BSP");
    }

    #[test]
    fn midrange_cli_rejects_hash_override_and_uv_case() {
        let request = json!({
            "case":"target-032", "engine_root":"/engine", "steam_root":"/steam",
            "bsp":"/case.bsp", "fixture_game":"/fixture", "observer_library":"/observer.so",
            "artifacts_parent":"/artifacts",
        });
        assert!(serde_json::from_value::<CliRequest<PplMidrange909Case>>(request.clone()).is_ok());
        let mut changed = request.clone();
        changed["bsp_sha256"] = json!("anything");
        assert!(serde_json::from_value::<CliRequest<PplMidrange909Case>>(changed).is_err());
        let mut changed = request;
        changed["case"] = json!("missing-uv");
        assert!(serde_json::from_value::<CliRequest<PplMidrange909Case>>(changed).is_err());
    }

    #[test]
    fn spatial_profile_rejects_cross_profile_cases_and_admission_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let bsp = directory.path().join("wrong-case.bsp");
        fs::write(&bsp, b"unapproved BSP").unwrap();
        for (name, case, expected) in [
            (
                "chart-a",
                PplSpatial919Case::ChartA,
                "3d2d7a21581dfe4ae8b989b4c65d2ad1ca97bead82392b21f99efb1dd0158c76",
            ),
            (
                "chart-b",
                PplSpatial919Case::ChartB,
                "f9dff566c480057c016afa510a5ccc34048ca86ecd1bdfddf2c18a7cd9852610",
            ),
        ] {
            assert_eq!(case.bsp_sha(), expected);
            assert!(pinned_bytes(&bsp, case.bsp_sha(), MAX_INPUT).is_err());
            assert!(serde_json::from_value::<PplUv857Case>(json!(name)).is_err());
            assert!(serde_json::from_value::<PplMidrange909Case>(json!(name)).is_err());
            let profile = crate::process::WorkerProfile::PplSpatial919(case);
            let value = json!({"kind":"ppl_spatial919", "parameters":name});
            assert_eq!(serde_json::to_value(profile).unwrap(), value);
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_ok());
        }
        for case in ["target-032", "missing-uv", "chart-c"] {
            assert!(serde_json::from_value::<PplSpatial919Case>(json!(case)).is_err());
        }
        for value in [
            json!({"kind":"ppl_spatial919", "parameters":"target-032"}),
            json!({"kind":"ppl_spatial919", "parameters":"chart-a", "command":"anything"}),
            json!({"kind":"ppl_spatial919", "parameters":{"case":"chart-a", "bsp_sha256":"anything"}}),
        ] {
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_err());
        }
        let request = json!({
            "case":"chart-a", "engine_root":"/engine", "steam_root":"/steam",
            "bsp":"/case.bsp", "fixture_game":"/fixture", "observer_library":"/observer.so",
            "artifacts_parent":"/artifacts",
        });
        assert!(serde_json::from_value::<CliRequest<PplSpatial919Case>>(request.clone()).is_ok());
        let mut changed = request.clone();
        changed["bsp_sha256"] = json!("anything");
        assert!(serde_json::from_value::<CliRequest<PplSpatial919Case>>(changed).is_err());
        let mut changed = request;
        changed["case"] = json!("target-032");
        assert!(serde_json::from_value::<CliRequest<PplSpatial919Case>>(changed).is_err());
    }

    #[test]
    fn mips_profile_rejects_other_cases_hashes_and_admission_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let bsp = directory.path().join("wrong-case.bsp");
        fs::write(&bsp, b"unapproved BSP").unwrap();
        for (name, case, expected) in [
            (
                "tail-032",
                PplMips931Case::Tail032,
                "a5ebd9d7d915faa8d56d0aa5b1a366821b53151d731e630960a265e9d0cd3ebc",
            ),
            (
                "tail-096",
                PplMips931Case::Tail096,
                "191d6e17e900fd56ab6713f73c296b8131405957394b3456231f4137ac7f13aa",
            ),
            (
                "reference-032",
                PplMips931Case::Reference032,
                "76ea7591eddd1262cf0c8f409078430c49ab01219695084bbea0554839e6c8c0",
            ),
            (
                "reference-096",
                PplMips931Case::Reference096,
                "7d90a2bfb370e166a68ebcca937d4b1791486874792ab41524f7596bab655d19",
            ),
        ] {
            assert_eq!(case.bsp_sha(), expected);
            assert!(pinned_bytes(&bsp, expected, MAX_INPUT).is_err());
            assert!(serde_json::from_value::<PplUv857Case>(json!(name)).is_err());
            assert!(serde_json::from_value::<PplSpatial919Case>(json!(name)).is_err());
            let profile = crate::process::WorkerProfile::PplMips931(case);
            let value = json!({"kind":"ppl_mips931", "parameters":name});
            assert_eq!(serde_json::to_value(profile).unwrap(), value);
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_ok());
            let request = json!({
                "case":name, "engine_root":"/engine", "steam_root":"/steam",
                "bsp":"/case.bsp", "fixture_game":"/fixture", "observer_library":"/observer.so",
                "artifacts_parent":"/artifacts",
            });
            assert!(serde_json::from_value::<CliRequest<PplMips931Case>>(request.clone()).is_ok());
            let mut changed = request;
            changed["bsp_sha256"] = json!("anything");
            assert!(serde_json::from_value::<CliRequest<PplMips931Case>>(changed).is_err());
        }
        for case in [
            "target-032",
            "reference-064",
            "chart-a",
            "missing-uv",
            "tail-064",
        ] {
            assert!(serde_json::from_value::<PplMips931Case>(json!(case)).is_err());
            let value = json!({"kind":"ppl_mips931", "parameters":case});
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_err());
        }
        for value in [
            json!({"kind":"ppl_mips931", "parameters":"tail-032", "command":"anything"}),
            json!({"kind":"ppl_mips931", "parameters":{"case":"tail-032", "bsp_sha256":"anything"}}),
        ] {
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_err());
        }
    }

    #[test]
    fn fixed_profile_rejects_commands_and_reflection_fields() {
        let profile = crate::process::WorkerProfile::PplUv857(PplUv857Case::MissingUv);
        assert_eq!(
            serde_json::to_value(profile).unwrap(),
            json!({"kind":"ppl_uv857","parameters":"missing-uv"})
        );
        for value in [
            json!({"kind":"ppl_uv857","parameters":"missing-uv","command":"anything"}),
            json!({"kind":"ppl_uv857","parameters":{"case":"missing-uv","samples":[]}}),
            json!({"kind":"ppl_uv857","parameters":"arbitrary-fixture"}),
        ] {
            assert!(serde_json::from_value::<crate::process::WorkerProfile>(value).is_err());
        }
    }
}
