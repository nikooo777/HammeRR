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
mod profile;

use hardware::{hardware_records, loaded_observer, verify_hardware};
use profile::{
    COMPLETE, DISCONNECTED, MAX_INPUT, PROBE_SHA, READY, TIMEOUT, pinned_bytes, private_write,
    verify_controls, verify_installed, verify_loads, verify_order, verify_private_fixture,
    verify_staging,
};
pub(crate) use profile::{
    MAP_NAME, configure_game, environment, observer_arguments, path_text, stage,
    validate_request_paths, verify_sources,
};
pub use profile::{PplUv857Case, PplUv857Receipt, PplUv857Request};

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

pub(crate) fn execute(
    directory: &Path,
    request: &WorkerRequest,
    case: PplUv857Case,
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
            "profile":"frozen_ppl_uv857", "case":case,"program":command.get_program().to_string_lossy(),
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
                verify_controls(&text)?;
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
                snapshot(directory, request, false, started + TIMEOUT)?;
                live_identity(&isolated, &child, &executable, identity)?;
                snapshot(directory, request, true, started + TIMEOUT)?;
                live_identity(&isolated, &child, &executable, identity)?;
                let text = util::diagnostic_text(&console, util::MAX_LOG)?;
                verify_loads(&text)?;
                let controls = verify_controls(&text)?;
                util::json_write(
                    &directory.join("diagnostic-observation.json"),
                    &json!({
                        "profile":"frozen_ppl_uv857","case":case,"camera_origin":[-192,-240,160],"camera_angles":[25,50,0],
                        "controls":controls,"hardware":hardware,"bsp_sha256":case.bsp_sha(),
                        "png_sha256":util::hash_file(&directory.join("game.png"),8*1024*1024)?,
                        "ppm_sha256":util::hash_file(&directory.join("game.ppm"),8*1024*1024)?,
                        "pixel_acceptance_evaluated":false,
                    }),
                )?;
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
        verify_controls(&text)?;
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
struct CliRequest {
    case: PplUv857Case,
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
    let request: CliRequest = util::json_read(path, 16 * 1024)?;
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
