use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use source_formats::keyvalues::{FlatBlockLimits, parse_flat_blocks_bounded};

use crate::process::WorkerRequest;
use crate::terrain_snapshot::{self, EngineImage, PinnedFile, TerrainSnapshotProfile};
use crate::util::{self, invalid};
use crate::{
    CaptureChannel, CaptureFailure, CleanupDisposition, PlayerSupportObservationReceipt,
    PlayerSupportSample, PlayerSupportSettings,
};

mod memory;
mod plugin;

pub(crate) const PROFILE: &str = "css_linux64_build25338318_passive_player_support_v1";
pub(crate) const SERVER_SHA: &str =
    "7887c8cc24a3dec6b8739bfebcad48c00b95423ebcbfc647f49fef1ab904dbe2";
pub(crate) const VPHYSICS_SHA: &str =
    "2676cdc02be4b4188f7121c201d5f5a681f9c1c7c924fe7f9159cefd5a718277";
const OUTPUT: &str = "player-support-publication.json";
const MAX_OUTPUT: usize = 2 * 1024 * 1024;

fn check_callback_failure(directory: &Path) -> io::Result<()> {
    match util::read(&directory.join("player-support-fatal.txt"), 4096) {
        Ok(bytes) => {
            let reason = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
            Err(invalid(format!("Player support observer failed: {reason}")))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlayerSupportProfile {
    pub map_name: String,
    pub bsp_sha256: String,
    pub settings: PlayerSupportSettings,
}

impl PlayerSupportProfile {
    pub fn new(map_name: String, bsp: &[u8], settings: PlayerSupportSettings) -> io::Result<Self> {
        let value = Self {
            map_name,
            bsp_sha256: util::hash(bsp),
            settings,
        };
        value.validate()?;
        validate_bsp(bsp, settings.team, &mut || Ok(()))?;
        Ok(value)
    }
    pub fn validate(&self) -> io::Result<()> {
        self.settings.validate()?;
        TerrainSnapshotProfile {
            map_name: self.map_name.clone(),
            bsp_sha256: self.bsp_sha256.clone(),
            channel: CaptureChannel::Ldr,
        }
        .validate()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginRequest {
    profile: PlayerSupportProfile,
    root: PathBuf,
    observer_path: PathBuf,
    observer_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    profile: String,
    map_name: String,
    bsp_sha256: String,
    samples: Vec<PlayerSupportSample>,
    observer: serde_json::Value,
    error: Option<String>,
    sampling_closed: bool,
}

fn validate_bsp(
    bytes: &[u8],
    team: u8,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let bsp = crate::owned_client::parse_bsp(bytes, check)?;
    let entities = parse_flat_blocks_bounded(
        bsp.entities()?,
        FlatBlockLimits {
            max_bytes: 4 * 1024 * 1024,
            max_blocks: 16384,
            max_pairs: 131072,
        },
    )
    .map_err(io::Error::other)?;
    let spawn_class = if team == 2 {
        "info_player_terrorist"
    } else {
        "info_player_counterterrorist"
    };
    let spawns = entities
        .iter()
        .filter(|entity| {
            entity.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("classname") && value.eq_ignore_ascii_case(spawn_class)
            })
        })
        .count();
    if spawns == 0 {
        return Err(invalid(
            "Player support observation requires an eligible spawn for the selected team",
        ));
    }
    Ok(())
}

fn plugin_binary() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let parent = executable
        .parent()
        .ok_or_else(|| invalid("Native helper has no parent"))?;
    let path = parent.join("libnative_runtime.so").canonicalize()?;
    if !fs::metadata(&path)?.is_file() {
        return Err(invalid("Player support observer library is missing"));
    }
    Ok(path)
}

pub(crate) fn configure_game(
    directory: &Path,
    game: &Path,
    profile: &PlayerSupportProfile,
) -> io::Result<()> {
    profile.validate()?;
    terrain_snapshot::configure_game(
        directory,
        game,
        &TerrainSnapshotProfile {
            map_name: profile.map_name.clone(),
            bsp_sha256: profile.bsp_sha256.clone(),
            channel: CaptureChannel::Ldr,
        },
    )?;
    let library = plugin_binary()?;
    let destination = directory.join("player-support-observer.so");
    let plugin_argument = destination
        .to_str()
        .ok_or_else(|| invalid("Observer plugin path is not UTF-8"))?;
    if plugin_argument
        .chars()
        .any(|character| character.is_control() || ['"', '\\', ';'].contains(&character))
    {
        return Err(invalid(
            "Observer plugin path contains unsupported console syntax",
        ));
    }
    fs::copy(&library, &destination)?;
    let plugin_sha = util::hash_file(&library, util::MAX_BSP)?;
    if util::hash_file(&destination, util::MAX_BSP)? != plugin_sha {
        return Err(invalid("Player support observer copy differs"));
    }
    util::json_write(
        &directory.join("player-support-plugin.json"),
        &json!({"source":library,"path":destination,"sha256":plugin_sha}),
    )?;
    let launch = game.join("cfg/hammerr_private_launch.cfg");
    let original = fs::read_to_string(&launch)?;
    for name in [
        "server.cfg",
        "listenserver.cfg",
        "hammerr_capture_postmap.cfg",
    ] {
        let path = game.join("cfg").join(name);
        let config = fs::read_to_string(&path)?;
        util::atomic_write(
            &path,
            format!("toggle mp_freezetime 0\n{config}").as_bytes(),
        )?;
    }
    let configured = format!(
        "toggle mp_freezetime 0\nplugin_load \"{}\"\n{original}",
        plugin_argument
    );
    util::atomic_write(&launch, configured.as_bytes())?;
    Ok(())
}

pub(crate) fn execute(
    directory: &Path,
    request: &WorkerRequest,
    profile: &PlayerSupportProfile,
) -> io::Result<()> {
    profile.validate()?;
    let root = request.environment.engine_root.canonicalize()?;
    let staged = PinnedFile::open(&directory.join("staged.bsp"), &profile.bsp_sha256)?;
    let bytes = util::read(&directory.join("staged.bsp"), util::MAX_BSP)?;
    validate_bsp(&bytes, profile.settings.team, &mut || {
        crate::owned_client::checkpoint(directory, request)
    })?;
    let images = [
        EngineImage::open_at(&root.join("bin/linux64/engine.so"), util::ENGINE_SHA)?,
        EngineImage::open_at(&root.join("cstrike/bin/linux64/server.so"), SERVER_SHA)?,
        EngineImage::open_at(&root.join("bin/linux64/vphysics.so"), VPHYSICS_SHA)?,
    ];
    let game = crate::worker::game_directory(directory, request)?;
    let consumed = PinnedFile::open(
        &game.join("maps").join(format!("{}.bsp", profile.map_name)),
        &profile.bsp_sha256,
    )?;
    let observer_path = directory.join("player-support-observer.so");
    let observer_sha256 = util::hash_file(&observer_path, util::MAX_BSP)?;
    let observer_image = EngineImage::open_at(&observer_path, &observer_sha256)?;
    terrain_snapshot::verify_providers(&request.environment, &game, &profile.map_name)?;
    util::json_write(
        &directory.join("player-support-observer-request.json"),
        &PluginRequest {
            profile: profile.clone(),
            root: root.clone(),
            observer_path,
            observer_sha256,
        },
    )?;
    let (publication, process) = crate::owned_client::run(
        directory,
        request,
        &game,
        "hammerr-terrain.log",
        PROFILE,
        "player-support-process-cleanup.json",
        |client| {
            let result = (|| {
                check_callback_failure(directory)?;
                if !terrain_snapshot::verify_controls(client.console, CaptureChannel::Ldr)? {
                    return Ok(None);
                }
                let launcher =
                    util::diagnostic_text(&directory.join("launcher.log"), 8 * 1024 * 1024)?;
                for marker in [
                    "SCRIPT ERROR",
                    "AN ERROR HAS OCCURRED",
                    "HAMMERR_PLAYER_SUPPORT_ERROR",
                ] {
                    if let Some(line) = client
                        .console
                        .lines()
                        .chain(launcher.lines())
                        .find(|line| line.contains(marker))
                    {
                        return Err(invalid(format!("Player support observer failed: {line}")));
                    }
                }
                let path = directory.join(OUTPUT);
                if !path.exists() {
                    return Ok(None);
                }
                let publication: Publication = util::json_read(&path, MAX_OUTPUT)?;
                validate_publication(&publication, profile)?;
                staged.verify()?;
                consumed.verify()?;
                Ok(Some(publication))
            })()
            .map_err(|error| CaptureFailure {
                error,
                cleanup: CleanupDisposition::Confirmed,
            })?;
            let Some(publication) = result else {
                return Ok(None);
            };
            let mut check = || crate::owned_client::checkpoint(directory, request);
            let receipt = terrain_snapshot::process::with_stopped(
                client.launch,
                client.child,
                client.executable,
                client.identity,
                &mut check,
                |process| {
                    check_callback_failure(directory)?;
                    let mut bindings = Vec::new();
                    for image in &images {
                        image.input.verify()?;
                        let binding = image.bind(process)?;
                        bindings.push(binding.receipt(&image.input));
                        image.verify_namespace(&binding, process)?;
                    }
                    let binding = observer_image.bind(process)?;
                    bindings.push(binding.receipt(&observer_image.input));
                    observer_image.verify_namespace(&binding, process)?;
                    if publication
                        .observer
                        .get("publication_pid")
                        .and_then(serde_json::Value::as_u64)
                        != Some(u64::from(process.receipt().pid))
                    {
                        return Err(invalid(
                            "Player support publication came from a different process",
                        ));
                    }
                    let frozen: Publication = util::json_read(&directory.join(OUTPUT), MAX_OUTPUT)?;
                    if serde_json::to_vec(&frozen).map_err(io::Error::other)?
                        != serde_json::to_vec(&publication).map_err(io::Error::other)?
                    {
                        return Err(invalid("Player support closed publication changed"));
                    }
                    Ok(
                        json!({"owned":process.receipt(),"modules":bindings,"bsp":consumed.receipt()}),
                    )
                },
            )?;
            Ok(Some((publication, receipt)))
        },
    )?;
    check_callback_failure(directory)?;
    staged.verify()?;
    consumed.verify()?;
    terrain_snapshot::verify_providers(&request.environment, &game, &profile.map_name)?;
    let receipt = PlayerSupportObservationReceipt {
        profile: PROFILE.into(),
        engine_sha256: util::ENGINE_SHA.into(),
        server_sha256: SERVER_SHA.into(),
        vphysics_sha256: VPHYSICS_SHA.into(),
        bsp_sha256: profile.bsp_sha256.clone(),
        map_name: profile.map_name.clone(),
        process,
        readiness: json!({"setup":format!("jointeam {}; joinclass 1",profile.settings.team),"setup_api":"IServerPluginCallbacks003::ClientActive -> next GameFrame / VEngineServer023::ClientCommand","setup_retired_before_measurement":true,"phase":"public_gameframe_before_next_server_simulation","exact_bsp":true}),
        observer: publication.observer.clone(),
        resumed: true,
    };
    util::json_write(&directory.join("player-support-observation.json"), &receipt)
}

fn validate_publication(
    publication: &Publication,
    profile: &PlayerSupportProfile,
) -> io::Result<()> {
    if let Some(error) = &publication.error {
        return Err(invalid(format!("Player support observer refused: {error}")));
    }
    if publication.profile != PROFILE
        || publication.map_name != profile.map_name
        || publication.bsp_sha256 != profile.bsp_sha256
        || !publication.sampling_closed
        || publication.samples.len() < 3
        || publication.samples.len() > profile.settings.max_samples
    {
        return Err(invalid(
            "Player support publication identity, completion or count differs",
        ));
    }
    crate::validate_player_support_samples(&publication.samples, &profile.settings)
}

pub(crate) fn decode_output(
    directory: &Path,
    profile: &PlayerSupportProfile,
) -> io::Result<(Vec<PlayerSupportSample>, PlayerSupportObservationReceipt)> {
    check_callback_failure(directory)?;
    if util::diagnostic_text(&directory.join("launcher.log"), 8 * 1024 * 1024)?
        .contains("HAMMERR_PLAYER_SUPPORT_ERROR")
    {
        return Err(invalid(
            "Player support observer callback failure latch is set",
        ));
    }
    let publication: Publication = util::json_read(&directory.join(OUTPUT), MAX_OUTPUT)?;
    validate_publication(&publication, profile)?;
    let observation: PlayerSupportObservationReceipt = util::json_read(
        &directory.join("player-support-observation.json"),
        MAX_OUTPUT,
    )?;
    if observation.profile != PROFILE
        || observation.bsp_sha256 != profile.bsp_sha256
        || observation.engine_sha256 != util::ENGINE_SHA
        || observation.server_sha256 != SERVER_SHA
        || observation.vphysics_sha256 != VPHYSICS_SHA
        || observation.map_name != profile.map_name
        || !observation.resumed
    {
        return Err(invalid("Player support observation receipt differs"));
    }
    Ok((publication.samples, observation))
}
